//! 候选栏面板「点击即上屏」的注入通道。
//!
//! server 不能直接把文本塞进前台应用的编辑会话：宿主 TSF 是**请求驱动**的，
//! 每条 `ProcessKeyEvent` 请求的回包里才能带 `commit`。所以这里走「自注入触发键」：
//!
//! 1. 把待上屏文本登记到 [`PENDING`]；
//! 2. `SendInput` 一个 [`VK_PASTE_TRIGGER`]（VK_F24，实体键盘上基本不存在）；
//! 3. 宿主 TSF 照常把这个键上报给 server（它不区分硬件键和注入键）；
//! 4. server 在 `ProcessKeyEvent` 里认出触发键，把待上屏文本当 `commit` 回包
//!    （见 `ipc_server::handle_paste_trigger`）。
//!
//! 这样文本走的是**和打字/选词完全同一条 TSF 编辑会话**上屏，不需要扩 IPC 协议、
//! 不需要改宿主，也不像模拟 Ctrl+V 那样会和「半成品编码串」打架
//! （宿主拿到 commit 时会用提交文本替换掉当前 composition）。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY,
};

/// 触发键：VK_F24。宿主会把它吃掉（server 回 `success: true`）；
/// 万一宿主没接住（前台应用不是 TSF 应用）漏给应用，F24 也没有任何副作用。
pub const VK_PASTE_TRIGGER: u16 = 0x87;

/// `librime::vk_to_xk(VK_F24)`：0x87 不在 librime 的映射表里，原样返回，
/// 所以 server 收到的是 keycode == 0x87。
pub const XK_PASTE_TRIGGER: i32 = 0x87;

/// 待上屏文本的有效期。触发键注入失败（前台应用完整性更高被 UIPI 拦下、
/// 宿主没在跑、用户此刻在英文态）时，这份文本不该在很久以后被别的触发键取走。
const PENDING_TTL: Duration = Duration::from_millis(1500);

/// 待上屏文本 + 登记时刻。
static PENDING: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// 请求把 `text` 上屏到前台应用的光标处。
///
/// 返回 `false` 表示注入触发键被系统拒绝（典型场景：前台应用以更高完整性级别
/// 运行，UIPI 不允许本进程注入输入）——调用方据此回退到「已复制到剪贴板」提示。
pub fn request_commit(text: &str) -> bool {
    queue_pending(text);
    inject_trigger()
}

/// 取走未过期的待上屏文本（`ProcessKeyEvent` 认出触发键时调用）。
pub fn take_pending() -> Option<String> {
    let taken = match PENDING.lock() {
        Ok(mut slot) => slot.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    match taken {
        Some((text, at)) if at.elapsed() <= PENDING_TTL => Some(text),
        Some((text, _)) => {
            tracing::warn!("待上屏文本已过期（{} 字），丢弃", text.chars().count());
            None
        }
        None => None,
    }
}

/// 登记待上屏文本（不注入按键，便于单测）。
fn queue_pending(text: &str) {
    let entry = Some((text.to_string(), Instant::now()));
    match PENDING.lock() {
        Ok(mut slot) => *slot = entry,
        Err(poisoned) => *poisoned.into_inner() = entry,
    }
}

/// 注入触发键（按下 + 抬起）。
fn inject_trigger() -> bool {
    let key = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_PASTE_TRIGGER),
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(false), key(true)];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        tracing::warn!(
            "上屏触发键注入失败（{}/{} 个事件被接受，可能被 UIPI 拦下）",
            sent,
            inputs.len()
        );
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 待上屏文本「取一次就没了」且**过期作废**。
    ///
    /// 两个断言写在同一个测试里：`PENDING` 是进程级静态量，并行跑的两个测试会互相
    /// 抢同一份文本。这里也刻意不走 `request_commit()`——那会真的往用户桌面注入按键。
    #[test]
    fn pending_commit_is_taken_once_and_expires() {
        queue_pending("第一段文本");
        assert_eq!(take_pending().as_deref(), Some("第一段文本"));
        assert_eq!(take_pending(), None, "取过一次之后不能再被第二次取走");

        // 手工塞一条过期条目：过期的待上屏文本不该再被触发键取走。
        let expired = Instant::now() - PENDING_TTL - Duration::from_millis(1);
        match PENDING.lock() {
            Ok(mut slot) => *slot = Some(("过期文本".to_string(), expired)),
            Err(poisoned) => *poisoned.into_inner() = Some(("过期文本".to_string(), expired)),
        }
        assert_eq!(take_pending(), None);
    }

    /// 触发键的 keycode 必须和宿主 `vk_to_xk` 的换算结果一致，否则 server 认不出来。
    #[test]
    fn trigger_keycode_matches_host_conversion() {
        assert_eq!(librime::vk_to_xk(VK_PASTE_TRIGGER), XK_PASTE_TRIGGER);
        // 不能落在字母/数字 keysym 区间：万一触发键漏进 rime，也不会被当成一次输入。
        assert!(
            XK_PASTE_TRIGGER > 'z' as i32,
            "触发键 keycode 不能与可打印字符重叠"
        );
    }
}