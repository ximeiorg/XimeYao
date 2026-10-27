//! 内置计算器（B1：`=` 触发前缀）。
//!
//! 设计（DECISIONS 2026-10-01「接管键盘的模式」红线 + 2026-10-23 遗留清单 B1）：
//! - **触发**：组词串为空时按 `=`（rime 社区计算器的惯例前缀），此后算式就是
//!   输入法自己的组词串（宿主在光标处画下划线），候选栏实时显示结果。
//! - **模式即缓冲**：[`BUFFER`] 有值 = 激活。没有独立开关，结构上不存在
//!   「模式开着忘了关」——上屏、退格到底、任意非算式键都会清缓冲退出。
//! - **与方案共存（动态让位）**：当前方案若用 `=` 做自己的事（自带计算器触发、
//!   `recognizer/patterns` 以 `=` 开头、key_binder 无条件绑定 `equal`），内置
//!   计算器让位，`=` 照常交给 rime——方案自带的 lua 计算器由此照常工作
//!   （moqi 的 `V` 触发是字母，永不冲突，两者可同时可用）。
//! - **随时无声退出**：字母、方向键、Ctrl/Alt 组合、任何没列出的键 → 清缓冲 +
//!   把键交给正常流程（rime），绝不吞键、绝不把字母塞进文档。
//! - **上屏即退出**：空格 / 回车 / `=` 只在算式有结果时提交；空算式按这些键
//!   收掉组词串退出；未成形的算式保持打开（不丢已输入内容，Esc/退格仍可退出）。
//! - 候选 label 为空：数字是往算式里输入用的，画「1.」会让人误以为按 1 能选中。
//! - 候选区实时显示「结果（默认高亮）+ 原式」；结果走空格/回车/`=` 上屏，
//!   原式可鼠标点击上屏（点选只接计算器；通用候选点选是独立功能点）。
//! - 候选栏锚在**进入点**、不跟组词串末端滑动。
//! - 进出都写 info 日志；逐键只 trace。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use winxime_ipc::{CandidateInfo, Context, IpcResponse, Text};
use xime_rime::RimeEngine;

use crate::context::SharedInputContext;
use crate::ui::CandidateWindow;

/// 触发字符。
const TRIGGER: char = '=';

/// 算式长度上限（键盘敲出来的算式都在这个量级；超长后忽略继续输入）。
const MAX_EXPR_LEN: usize = 48;

/// 不可上屏的错误提示。
const ERR_DIV_ZERO: &str = "⚠ 不能除以 0";
const ERR_INVALID: &str = "⚠ 算式有误";

/// 小键盘数字 keysym 上界（librime 只定义了 XK_KP_0；0xFFB0..=0xFFB9 连续）。
const XK_KP_9: i32 = librime::key::XK_KP_0 + 9;

/// librime 的这几个常量是 `u32`（RimeKeyCode），而 IPC 的 keycode 是 `i32`。
/// match 的模式必须是常量，所以先转换成本模块的 i32 常量。
const XK_BACKSPACE: i32 = librime::key::XK_BACK_SPACE as i32;
const XK_ESC: i32 = librime::key::XK_ESCAPE as i32;
const XK_ENTER: i32 = librime::key::XK_RETURN as i32;
const XK_SPC: i32 = librime::key::XK_SPACE as i32;
const XK_LEFT_I: i32 = librime::key::XK_LEFT as i32;
const XK_RIGHT_I: i32 = librime::key::XK_RIGHT as i32;
const XK_UP_I: i32 = librime::key::XK_UP as i32;
const XK_DOWN_I: i32 = librime::key::XK_DOWN as i32;

/// 方案文件解析缓存：`(方案 id, 是否占用触发键)`。
static TRIGGER_BLOCKED: Mutex<Option<(String, bool)>> = Mutex::new(None);

/// rime 数据目录（main.rs 启动时注入；未注入时让位检测按「未占用」处理）。
static RIME_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 计算缓冲：`Some(expr)` = 激活。expr 不含前导 `=`，刚进入时是空串。
static BUFFER: Mutex<Option<String>> = Mutex::new(None);

/// 候选栏锚点：进入计算器那一刻的插入点坐标。
///
/// 宿主 TSF 每次按键上报的是**组词串末端**的光标；跟着它走，候选栏会随着算式
/// 变长每按一键向右滑一格——就是实机上看到的「抖动」。计算器会话内位置固定，
/// 锚在进入点，只让宽度随结果变化。
static ANCHOR: Mutex<Option<(i32, i32)>> = Mutex::new(None);

/// 候选栏是否已显示。只在计算器会话内使用：已显示时只 `update` 内容，
/// 不再每键 `show`（重复 `ShowWindow`/`SetWindowPos` 会造成可见的闪动）。
static WINDOW_SHOWN: AtomicBool = AtomicBool::new(false);

/// 与当前候选一一对应的上屏文本：`Some(text)` = 点击该候选就提交 text；
/// `None` = 只展示（错误提示），点了不上屏。由 [`compose_response`] 同步更新，
/// UI 线程点候选时通过 [`candidate_commit_text`] 读取。
static COMMITTABLE: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());

/// 当前选中的候选下标（原始下标，不是「可选列表」里的序号）。默认 0 = 结果；
/// 方向键在本布局对应的轴上切换；任何输入/退格都回到结果。
static SELECTED: Mutex<usize> = Mutex::new(0);

/// B2 数字开头的影子缓冲。`Some("12")` = 已吞下数字、等待运算符。
/// 默认开启；`calculator.auto_detect: false` 可关，开发期也可用
/// `%APPDATA%\xime\auto_detect.flag` / 环境变量强制打开。
static SHADOW: Mutex<Option<String>> = Mutex::new(None);

/// 影子缓冲的「代」：每次内容/状态变化 +1，旧的空闲提交线程据此放弃。
static SHADOW_GEN: AtomicU64 = AtomicU64::new(0);

/// 影子缓冲空闲多久后自动把数字上屏（毫秒）。只兜底「用户打完数字就不动了、
/// 也不切窗口」的场景；正常退出靠非算式键回放 / 失焦提交。
const SHADOW_IDLE_COMMIT_MS: u64 = 2500;

static AUTO_DETECT: OnceLock<bool> = OnceLock::new();

fn lock_shadow() -> MutexGuard<'static, Option<String>> {
    match SHADOW.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 取缓冲（容忍锁中毒，与 paste.rs 同一纪律）。
fn lock_buffer() -> MutexGuard<'static, Option<String>> {
    match BUFFER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn lock_committable() -> MutexGuard<'static, Vec<Option<String>>> {
    match COMMITTABLE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn lock_selected() -> MutexGuard<'static, usize> {
    match SELECTED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 在「可上屏」的候选中移动选中项；返回 false = 少于两条（调用方按旧行为退出）。
fn move_selection(next: bool) -> bool {
    let commits = lock_committable();
    let selectable: Vec<usize> = commits
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.is_some().then_some(i))
        .collect();
    if selectable.len() < 2 {
        return false;
    }
    let mut selected = lock_selected();
    let len = selectable.len();
    let pos = selectable.iter().position(|i| *i == *selected).unwrap_or(0);
    let next_pos = if next {
        (pos + 1) % len
    } else {
        (pos + len - 1) % len
    };
    *selected = selectable[next_pos];
    true
}

fn lock_anchor() -> MutexGuard<'static, Option<(i32, i32)>> {
    match ANCHOR.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn lock_trigger_memo() -> MutexGuard<'static, Option<(String, bool)>> {
    match TRIGGER_BLOCKED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 记录 rime 数据目录（main.rs 启动时调用一次）。
pub fn init_rime_dir(dir: &Path) {
    let _ = RIME_DIR.set(dir.to_path_buf());
}

/// 计算器当前是否激活（缓冲非 None）。
pub fn is_active() -> bool {
    lock_buffer().is_some()
}

/// 清空缓冲。所有「引擎的组词串被外部清掉」的路径都要调（FocusOut / 切中英 /
/// 写词条 / 面板上屏 / 结束会话），否则缓冲会残留一个没人管的组词串。
/// 只在真的退出时记日志，避免刷屏。
pub fn reset(reason: &str) {
    let _ = lock_shadow().take();
    cancel_shadow_idle_commit();
    *lock_anchor() = None;
    WINDOW_SHOWN.store(false, Ordering::Relaxed);
    lock_committable().clear();
    *lock_selected() = 0;
    let mut buffer = lock_buffer();
    if buffer.take().is_some() {
        tracing::info!("计算器：退出（{reason}）");
    }
}

/// 点击候选 `index` 要上屏的文本；`None` = 不可上屏（错误提示）或下标越界。
pub fn candidate_commit_text(index: usize) -> Option<String> {
    lock_committable().get(index).cloned().flatten()
}

/// B2 原型开关：环境变量或 `%APPDATA%\xime\auto_detect.flag` 存在即启用。
/// 只读一次（重启 server 生效）——热路径上不做文件 IO。
pub fn auto_detect_enabled() -> bool {
    *AUTO_DETECT.get_or_init(|| {
        if std::env::var_os("XIME_CALC_AUTODETECT").is_some() {
            return true;
        }
        if std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("xime").join("auto_detect.flag").exists())
            .unwrap_or(false)
        {
            return true;
        }
        auto_detect_from_config()
    })
}

/// 正式开关：`xime.custom.yaml` 的 `calculator.auto_detect`，**默认开启**；
/// 显式写 `false` 才关闭。
///
/// 直接读原始 YAML（不进 `xime-config` 的 typed struct）：这样 XimeYao 的 CI
/// 仍可以依赖 upstream 的 `xime-config`，不会因为多一个字段而编译失败。
fn auto_detect_from_config() -> bool {
    match std::fs::read_to_string(xime_config::XimeConfig::config_path()) {
        Ok(text) => auto_detect_from_yaml(&text),
        // 没有配置文件也按默认开启；显式写 false 才关。
        Err(_) => true,
    }
}

fn auto_detect_from_yaml(text: &str) -> bool {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(text) else {
        return true;
    };
    value
        .get("calculator")
        .and_then(|calculator| calculator.get("auto_detect"))
        .and_then(serde_yaml::Value::as_bool)
        .unwrap_or(true)
}

/// 这颗键能不能作为影子缓冲的起点（数字行 + 小键盘数字）。
pub fn is_shadow_starter(keysym: i32) -> bool {
    (0x30..=0x39).contains(&keysym) || (librime::key::XK_KP_0..=XK_KP_9).contains(&keysym)
}

/// 影子缓冲是否激活（已吞下数字、还没遇到运算符）。
pub fn shadow_active() -> bool {
    lock_shadow().is_some()
}

/// 让位检测缓存失效（SelectSchema / ReloadConfig 时调）。
pub fn invalidate_trigger_cache() {
    *lock_trigger_memo() = None;
}

/// 当前方案是否占用 `=`（占用则内置计算器让位）。按方案 id 记忆检测结果。
pub fn trigger_blocked(eng: &RimeEngine) -> bool {
    let Some(schema_id) = eng.get_current_schema() else {
        return false;
    };
    if let Some((id, blocked)) = lock_trigger_memo().as_ref() {
        if *id == schema_id {
            return *blocked;
        }
    }
    let blocked = compute_trigger_blocked(&schema_id);
    tracing::info!(
        "计算器：触发键 `{}` 检测，方案 {} —— {}",
        TRIGGER,
        schema_id,
        if blocked {
            "已占用，让位给方案自带功能"
        } else {
            "未占用，内置计算器生效"
        }
    );
    *lock_trigger_memo() = Some((schema_id, blocked));
    blocked
}

fn compute_trigger_blocked(schema_id: &str) -> bool {
    let Some(dir) = RIME_DIR.get() else {
        return false;
    };
    read_schema_text(dir, schema_id)
        .map(|text| schema_uses_trigger(&text, TRIGGER))
        .unwrap_or(false)
}

/// 读方案配置：优先 rime 根目录的 `<id>.schema.yaml`，缺失时回退 build/ 编译产物
/// （与 schema_switches 同一优先级）。
fn read_schema_text(rime_dir: &Path, schema_id: &str) -> Option<String> {
    let candidates = [
        rime_dir.join(format!("{schema_id}.schema.yaml")),
        rime_dir
            .join("build")
            .join(format!("{schema_id}.schema.yaml")),
    ];
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return Some(text);
        }
    }
    None
}

/// 判断方案配置是否把 `trigger` 用作自己的功能入口。
///
/// 检测四类占用（命中任意一条即让位）：
/// 1. `calculator.trigger` 以该字符开头（moqi 形态的自带计算器）；
/// 2. `recognizer.patterns` 有正则以该字符开头（如 `^=[0-9]`）；
/// 3. `key_binder.bindings` 无条件绑定 `accept: equal`（有 `when: has_menu` /
///    `paging` / `composing` 限定的不算——组词串为空时它们不会触发）；
/// 4. `speller.alphabet` 含该字符（该字符是方案自己的编码字符，组词串为空时
///    按它应当开始组词）。
fn schema_uses_trigger(text: &str, trigger: char) -> bool {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(text) else {
        return false;
    };

    if let Some(alphabet) = value
        .get("speller")
        .and_then(|s| s.get("alphabet"))
        .and_then(|v| v.as_str())
    {
        if alphabet.contains(trigger) {
            return true;
        }
    }

    if let Some(t) = value
        .get("calculator")
        .and_then(|c| c.get("trigger"))
        .and_then(|v| v.as_str())
    {
        if t.trim().starts_with(trigger) {
            return true;
        }
    }

    if let Some(patterns) = value
        .get("recognizer")
        .and_then(|r| r.get("patterns"))
        .and_then(|p| p.as_mapping())
    {
        for (_name, pattern) in patterns.iter() {
            if let Some(re) = pattern.as_str() {
                if re.trim().trim_start_matches('^').starts_with(trigger) {
                    return true;
                }
            }
        }
    }

    if let Some(bindings) = value
        .get("key_binder")
        .and_then(|k| k.get("bindings"))
        .and_then(|b| b.as_sequence())
    {
        for binding in bindings {
            let accept = binding
                .get("accept")
                .and_then(|v| v.as_str())
                .map(str::trim);
            let restricted = binding
                .get("when")
                .and_then(|v| v.as_str())
                .is_some_and(|w| matches!(w, "has_menu" | "paging" | "composing"));
            if matches!(accept, Some("equal") | Some("=")) && !restricted {
                return true;
            }
        }
    }

    false
}

/// 激活期间的按键处置结果。`Handled` 装箱：`IpcResponse` 很大（700+ 字节），
/// 不装箱会让这个枚举在 `handle_key_event` 的栈帧里白占一大块。
pub enum ActiveKeyOutcome {
    /// 计算器处理了这颗键（含「关闭组词串」），调用方直接返回。
    Handled(Box<IpcResponse>),
    /// 键不归计算器、缓冲已清空——调用方继续走原有流程（建议词/英文态/rime），
    /// 后续流程自会用 rime 的（空）状态回包，宿主随之结束组词串。
    Delegated,
    /// 不是 key-down（key-up 等），计算器不处理也不退出，继续原有流程。
    NotAKeyDown,
}

/// 激活期间处理一颗键。未激活时立即 [`ActiveKeyOutcome::Delegated`]（每颗键
/// 都会进来一次，只是一次缓冲锁的代价）。
pub fn handle_active_key(
    keycode: i32,
    modifiers: i32,
    session_id: u32,
    eng: &RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
) -> ActiveKeyOutcome {
    let mut buffer = lock_buffer();
    let Some(expr) = buffer.as_mut() else {
        return ActiveKeyOutcome::Delegated;
    };

    let horizontal = crate::config::get().style.horizontal;
    let Some(action) = action_for_key(keycode, modifiers, horizontal) else {
        return ActiveKeyOutcome::NotAKeyDown;
    };

    match action {
        CalcAction::Input(ch) => {
            *lock_selected() = 0;
            if expr.chars().count() >= MAX_EXPR_LEN {
                tracing::debug!("计算器：算式已达 {MAX_EXPR_LEN} 字符上限，忽略输入");
            } else {
                expr.push(ch);
                tracing::trace!("计算器：算式 = {expr:?}");
            }
            ActiveKeyOutcome::Handled(Box::new(compose_response(
                session_id, eng, context, window, expr,
            )))
        }
        CalcAction::Backspace => {
            *lock_selected() = 0;
            if expr.pop().is_none() {
                // 缓冲只剩前导 `=`：组词串是计算器自己画的，从未落进文档——
                // 必须自己收掉再回包，不能把退格交给应用去删真正的文本。
                let _ = buffer.take();
                tracing::info!("计算器：退出（退格删空前缀）");
                return ActiveKeyOutcome::Handled(Box::new(close_response(
                    session_id, eng, context, window,
                )));
            }
            ActiveKeyOutcome::Handled(Box::new(compose_response(
                session_id, eng, context, window, expr,
            )))
        }
        CalcAction::SelectPrev => {
            if move_selection(false) {
                ActiveKeyOutcome::Handled(Box::new(compose_response(
                    session_id, eng, context, window, expr,
                )))
            } else {
                // 没有第二条可选项：方向键照旧退出，把光标移动还给用户。
                let _ = buffer.take();
                tracing::info!("计算器：退出（方向键没有可切换的候选）");
                ActiveKeyOutcome::Delegated
            }
        }
        CalcAction::SelectNext => {
            if move_selection(true) {
                ActiveKeyOutcome::Handled(Box::new(compose_response(
                    session_id, eng, context, window, expr,
                )))
            } else {
                let _ = buffer.take();
                tracing::info!("计算器：退出（方向键没有可切换的候选）");
                ActiveKeyOutcome::Delegated
            }
        }
        CalcAction::Clear => {
            let _ = buffer.take();
            tracing::info!("计算器：退出（Esc）");
            ActiveKeyOutcome::Handled(Box::new(close_response(
                session_id, eng, context, window,
            )))
        }
        CalcAction::CommitAttempt => {
            let outcome = eval_outcome(expr);
            // 空格/回车/`=` 上屏**选中的那一条**（结果或原式）；未成形/出错时
            // 没有可上屏的内容，维持原来的「保持打开」语义。
            let selected_text = if outcome.value().is_some() {
                let selected = *lock_selected();
                lock_committable().get(selected).cloned().flatten()
            } else {
                None
            };
            match selected_text {
                Some(text) => {
                    let _ = buffer.take();
                    tracing::info!("计算器：上屏（选中）{text}");
                    context.update(|c| {
                        c.suggestion_state = None;
                        c.commit_text = text.clone();
                        c.is_composing = false;
                        c.composition.preedit.clear();
                    });
                    *lock_anchor() = None;
                    WINDOW_SHOWN.store(false, Ordering::Relaxed);
                    lock_committable().clear();
                    *lock_selected() = 0;
                    window.hide();
                    ActiveKeyOutcome::Handled(Box::new(IpcResponse {
                        context: Some(Context {
                            preedit: Text { str: String::new() },
                            commit: Some(text),
                            candidates: CandidateInfo::default(),
                        }),
                        status: Some(crate::ipc_server::get_ipc_status(eng)),
                        ..IpcResponse::ok(session_id)
                    }))
                }
                None => {
                    if expr.is_empty() {
                        // 只有前导 `=`：上屏键没有可提交的内容，收掉组词串退出
                        //（不把键交给应用——那会在旧组词串还开着时插一个空格）。
                        let _ = buffer.take();
                        tracing::info!("计算器：退出（空算式按上屏键）");
                        return ActiveKeyOutcome::Handled(Box::new(close_response(
                            session_id, eng, context, window,
                        )));
                    }
                    // 算式未成形（结尾是运算符/括号未配平）或有错：**不退出**，
                    // 让用户接着改。否则已输入的数字会被静默丢掉，宿主里还会留下
                    // 一条没人收的组词串。Esc / 退格 / 字母仍随时可退出。
                    tracing::debug!("计算器：算式尚无结果，忽略上屏键");
                    ActiveKeyOutcome::Handled(Box::new(compose_response(
                        session_id, eng, context, window, expr,
                    )))
                }
            }
        }
        CalcAction::Delegate => {
            let _ = buffer.take();
            tracing::info!("计算器：退出（按键不归计算器，交给正常流程）");
            ActiveKeyOutcome::Delegated
        }
    }
}

/// 组词串为空时的 `=` 入口。条件不满足（正在组词 / 方案占用触发键 / 已激活）
/// 返回 `None`，调用方继续原有流程。
pub fn try_enter(
    eng: &RimeEngine,
    session_id: u32,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
) -> Option<IpcResponse> {
    if eng.is_composing() || trigger_blocked(eng) {
        return None;
    }
    {
        let mut buffer = lock_buffer();
        // 激活期间的 `=` 已由 handle_active_key 处理；这里只负责未激活的进入。
        if buffer.is_some() {
            return None;
        }
        *buffer = Some(String::new());
    }
    tracing::info!("计算器：进入（`=` 前缀，Esc 退出；空格/回车/`=` 上屏结果）");
    let (anchor_x, anchor_y) = context.read(|c| (c.caret_x, c.caret_y));
    *lock_anchor() = Some((anchor_x, anchor_y));
    WINDOW_SHOWN.store(false, Ordering::Relaxed);
    *lock_selected() = 0;
    context.update(|c| {
        c.suggestion_state = None;
        c.is_composing = true;
        c.composition.preedit = TRIGGER.to_string();
    });
    window.hide();
    Some(IpcResponse {
        context: Some(Context {
            preedit: Text {
                str: TRIGGER.to_string(),
            },
            commit: None,
            candidates: CandidateInfo::default(),
        }),
        status: Some(calculator_status(eng)),
        ..IpcResponse::ok(session_id)
    })
}

/// 候选栏左上角：会话锚点优先；没有锚点（测试/异常路径）才用当前光标。
fn window_origin(anchor: Option<(i32, i32)>, caret: (i32, i32)) -> (i32, i32) {
    anchor.unwrap_or(caret)
}

// ---------------------------------------------------------------------------
// B2：数字开头的影子缓冲（默认开启，`auto_detect: false` 可关）
// ---------------------------------------------------------------------------

/// 影子缓冲里的按键分类。
#[derive(Debug, PartialEq, Eq)]
enum ShadowAction {
    Append(char),
    Operator(char),
    /// 直接结束输入（`=`）：把已输入的数字当完整算式接管。
    Finish,
    Backspace,
    Clear,
    Replay,
    /// 回车：把数字直接提交，不再尝试注入当前键（注入 Enter 会触发
    /// “该按键无法重放”的假失败通知）。
    CommitDigits,
    /// 功能键（F1–F24）：先提交数字，再把当前键交还宿主，不参与注入。
    PassThrough,
}

fn classify_shadow_key(keysym: i32) -> ShadowAction {
    match keysym {
        0x30..=0x39 => ShadowAction::Append(keysym as u8 as char),
        0x2E => ShadowAction::Append('.'),
        0x2B | 0x2D | 0x2A | 0x2F | 0x25 | 0x5E | 0x28 | 0x29 => {
            ShadowAction::Operator(keysym as u8 as char)
        }
        librime::key::XK_KP_0..=XK_KP_9 => {
            let digit = (keysym - librime::key::XK_KP_0) as u8;
            ShadowAction::Append((b'0' + digit) as char)
        }
        librime::key::XK_KP_DECIMAL => ShadowAction::Append('.'),
        librime::key::XK_KP_ADD => ShadowAction::Operator('+'),
        librime::key::XK_KP_SUBTRACT => ShadowAction::Operator('-'),
        librime::key::XK_KP_MULTIPLY => ShadowAction::Operator('*'),
        librime::key::XK_KP_DIVIDE => ShadowAction::Operator('/'),
        librime::key::XK_EQUAL => ShadowAction::Finish,
        XK_ENTER | 0xFF8D => ShadowAction::CommitDigits,
        0xFFBE..=0xFFD5 => ShadowAction::PassThrough,
        XK_BACKSPACE => ShadowAction::Backspace,
        XK_ESC => ShadowAction::Clear,
        _ => ShadowAction::Replay,
    }
}

/// 影子缓冲内容变了：作废旧空闲提交线程，并安排一个新的。
fn schedule_shadow_idle_commit() {
    let generation = SHADOW_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(SHADOW_IDLE_COMMIT_MS));
        if SHADOW_GEN.load(Ordering::SeqCst) != generation {
            return;
        }
        let digits = lock_shadow().take();
        let Some(digits) = digits else {
            return;
        };
        let count = digits.chars().count();
        tracing::info!("自动识别：空闲 {SHADOW_IDLE_COMMIT_MS}ms 提交 {count} 字");
        if !crate::paste::request_commit(&digits) {
            let _ = crate::paste::take_pending();
            // 注入失败：把数字放回影子缓冲，等下一次按键/失焦再处理。
            let mut shadow = lock_shadow();
            if shadow.is_none() {
                *shadow = Some(digits);
            }
        }
    });
}

/// 取消当前的空闲提交（内容变化/退出时调用）。
fn cancel_shadow_idle_commit() {
    SHADOW_GEN.fetch_add(1, Ordering::SeqCst);
}

/// 失焦时取走暂存数字（由宿主在旧 context 上提交）。返回 `None` = 没有影子缓冲。
pub fn take_shadow_for_flush() -> Option<String> {
    let digits = lock_shadow().take()?;
    cancel_shadow_idle_commit();
    *lock_anchor() = None;
    WINDOW_SHOWN.store(false, Ordering::Relaxed);
    *lock_selected() = 0;
    let count = digits.chars().count();
    tracing::info!("自动识别：失焦提交 {count} 字");
    Some(digits)
}

/// 进入影子缓冲：组词串为空时按下数字（调用方已校验英文态/组词中/联想态）。
pub fn try_enter_shadow(
    keysym: i32,
    modifiers: i32,
    session_id: u32,
    eng: &RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
) -> Option<IpcResponse> {
    if !auto_detect_enabled() {
        return None;
    }
    if modifiers
        & (librime::key::K_CONTROL_MASK
            | librime::key::K_ALT_MASK
            | librime::key::K_RELEASE_MASK) as i32
        != 0
    {
        return None;
    }
    if !is_shadow_starter(keysym) {
        return None;
    }
    {
        let mut shadow = lock_shadow();
        if shadow.is_some() {
            return None;
        }
        let first = if (0x30..=0x39).contains(&keysym) {
            keysym as u8 as char
        } else {
            (b'0' + (keysym - librime::key::XK_KP_0) as u8) as char
        };
        *shadow = Some(first.to_string());
    }
    let (x, y) = context.read(|c| (c.caret_x, c.caret_y));
    *lock_anchor() = Some((x, y));
    WINDOW_SHOWN.store(false, Ordering::Relaxed);
    *lock_selected() = 0;
    tracing::info!("自动识别：进入影子缓冲（数字开头）");
    let value = lock_shadow().clone().unwrap_or_default();
    schedule_shadow_idle_commit();
    Some(shadow_response(&value, session_id, eng, context, window))
}

/// 影子缓冲的按键处置（只在 `shadow_active()` 时调用）。
pub fn handle_shadow_key(
    keysym: i32,
    modifiers: i32,
    session_id: u32,
    eng: &RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
) -> ActiveKeyOutcome {
    let mut shadow = lock_shadow();
    let Some(value) = shadow.as_mut() else {
        return ActiveKeyOutcome::Delegated;
    };
    if modifiers & librime::key::K_RELEASE_MASK as i32 != 0 {
        return ActiveKeyOutcome::NotAKeyDown;
    }
    if modifiers & (librime::key::K_CONTROL_MASK | librime::key::K_ALT_MASK) as i32 != 0 {
        let digits = shadow.take().unwrap_or_default();
        drop(shadow);
        return shadow_replay(digits, keysym, modifiers, session_id, eng, context, window);
    }

    match classify_shadow_key(keysym) {
        ShadowAction::Append(ch) => {
            if value.chars().count() < MAX_EXPR_LEN {
                value.push(ch);
            }
            let next = value.clone();
            drop(shadow);
            schedule_shadow_idle_commit();
            ActiveKeyOutcome::Handled(Box::new(shadow_response(
                &next, session_id, eng, context, window,
            )))
        }
        ShadowAction::Operator(ch) => {
            let digits = shadow.take().unwrap_or_default();
            drop(shadow);
            let expr = format!("{digits}{ch}");
            tracing::info!("自动识别：识别为算式 `{expr}`");
            handoff_to_calculator(expr, session_id, eng, context, window)
        }
        ShadowAction::Finish => {
            let digits = shadow.take().unwrap_or_default();
            drop(shadow);
            if digits.is_empty() {
                return ActiveKeyOutcome::Handled(Box::new(close_response(
                    session_id, eng, context, window,
                )));
            }
            tracing::info!("自动识别：`=` 结束，按算式 `{digits}` 接管");
            handoff_to_calculator(digits, session_id, eng, context, window)
        }
        ShadowAction::Backspace => {
            value.pop();
            if value.is_empty() {
                let _ = shadow.take();
                drop(shadow);
                return ActiveKeyOutcome::Handled(Box::new(close_response(
                    session_id, eng, context, window,
                )));
            }
            let next = value.clone();
            drop(shadow);
            schedule_shadow_idle_commit();
            ActiveKeyOutcome::Handled(Box::new(shadow_response(
                &next, session_id, eng, context, window,
            )))
        }
        ShadowAction::Clear => {
            let _ = shadow.take();
            drop(shadow);
            ActiveKeyOutcome::Handled(Box::new(close_response(
                session_id, eng, context, window,
            )))
        }
        ShadowAction::CommitDigits => {
            let digits = shadow.take().unwrap_or_default();
            drop(shadow);
            cancel_shadow_idle_commit();
            tracing::info!("自动识别：回车提交 {} 字", digits.chars().count());
            let mut response = close_response(session_id, eng, context, window);
            if let Some(ctx) = response.context.as_mut() {
                ctx.commit = if digits.is_empty() {
                    None
                } else {
                    Some(digits)
                };
            }
            ActiveKeyOutcome::Handled(Box::new(response))
        }
        ShadowAction::PassThrough => {
            let digits = shadow.take().unwrap_or_default();
            drop(shadow);
            cancel_shadow_idle_commit();
            *lock_anchor() = None;
            WINDOW_SHOWN.store(false, Ordering::Relaxed);
            *lock_selected() = 0;
            context.update(|c| {
                c.suggestion_state = None;
                c.is_composing = false;
                c.composition.preedit.clear();
            });
            window.hide();
            if !digits.is_empty() && !crate::paste::request_commit(&digits) {
                tracing::warn!("自动识别：功能键前的数字提交失败，已放行当前键");
            }
            // 当前键交给正常流程/宿主；F2、F5 等不能再被注入或用字母 keysym 吞掉。
            ActiveKeyOutcome::Delegated
        }
        ShadowAction::Replay => {
            let digits = shadow.take().unwrap_or_default();
            drop(shadow);
            shadow_replay(digits, keysym, modifiers, session_id, eng, context, window)
        }
    }
}

/// 影子缓冲切成正式算式（复用 B1 的缓冲/候选/点选）。
fn handoff_to_calculator(
    expr: String,
    session_id: u32,
    eng: &RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
) -> ActiveKeyOutcome {
    cancel_shadow_idle_commit();
    {
        let mut buffer = lock_buffer();
        *buffer = Some(expr);
    }
    *lock_selected() = 0;
    let expr = lock_buffer().clone().unwrap_or_default();
    ActiveKeyOutcome::Handled(Box::new(compose_response(
        session_id, eng, context, window, &expr,
    )))
}

/// 影子缓冲的组词串回包（只有数字/小数点，没有候选）。
fn shadow_response(
    value: &str,
    session_id: u32,
    eng: &RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
) -> IpcResponse {
    context.update(|c| {
        c.suggestion_state = None;
        c.is_composing = true;
        c.composition.preedit = value.to_string();
    });
    window.hide();
    IpcResponse {
        context: Some(Context {
            preedit: Text {
                str: value.to_string(),
            },
            commit: None,
            candidates: CandidateInfo::default(),
        }),
        status: Some(calculator_status(eng)),
        ..IpcResponse::ok(session_id)
    }
}

/// 回放：把吞下的数字先提交，再把当前键注入回输入流（设计验证的核心风险点）。
fn shadow_replay(
    digits: String,
    keysym: i32,
    modifiers: i32,
    session_id: u32,
    eng: &RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
) -> ActiveKeyOutcome {
    cancel_shadow_idle_commit();
    tracing::info!(
        "自动识别：回放 {} 字 + 当前键 keysym=0x{keysym:X} mod={modifiers}",
        digits.chars().count()
    );
    *lock_anchor() = None;
    WINDOW_SHOWN.store(false, Ordering::Relaxed);
    *lock_selected() = 0;
    window.hide();

    let committed = crate::paste::request_commit(&digits);
    let injected = committed && crate::paste::inject_key(keysym, modifiers);
    context.update(|c| {
        c.suggestion_state = None;
        c.is_composing = false;
        c.composition.preedit.clear();
    });

    if !committed {
        // F24 都没注入：只能把数字放在当前回包里上屏；当前键保不住，提示用户重按。
        let _ = crate::paste::take_pending();
        crate::notify::notify("自动识别回放失败：数字已上屏，请重新按键");
        return ActiveKeyOutcome::Handled(Box::new(IpcResponse {
            context: Some(Context {
                preedit: Text { str: String::new() },
                commit: Some(digits),
                candidates: CandidateInfo::default(),
            }),
            status: Some(crate::ipc_server::get_ipc_status(eng)),
            ..IpcResponse::ok(session_id)
        }));
    }
    if !injected {
        // 数字会由 F24 提交；当前键无法重放（少数键/被 UIPI 拦），提示重按。
        crate::notify::notify("自动识别回放失败：该按键无法重放，请重新按键");
    }
    // 当前键吞掉：F24 提交数字，随后注入的那颗键走正常流程。
    ActiveKeyOutcome::Handled(Box::new(IpcResponse {
        context: Some(Context {
            preedit: Text { str: String::new() },
            commit: None,
            candidates: CandidateInfo::default(),
        }),
        status: Some(crate::ipc_server::get_ipc_status(eng)),
        ..IpcResponse::ok(session_id)
    }))
}

/// 计算器自己的状态：组词串不在 rime 里，`Status::composing` 必须由这里补上
/// （宿主 TSF 只看 `status.composing` 决定开/关 composition；漏了它就只出候选、
/// 光标处看不到算式）。
fn calculator_status(eng: &RimeEngine) -> winxime_ipc::Status {
    let mut status = crate::ipc_server::get_ipc_status(eng);
    status.composing = true;
    status
}

/// 组词串/候选栏刷新回包：缓冲变化后重建 preedit 与候选（结果或错误）。
fn compose_response(
    session_id: u32,
    eng: &RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
    expr: &str,
) -> IpcResponse {
    let preedit = format!("{TRIGGER}{}", display_preedit(expr));
    // 候选区实时给两样东西：结果（默认高亮）+ 原式；未成形时只给原式。
    // 点选/空格依赖这份顺序（见 candidate_commit_text），必须和 candidates 同步更新。
    let selected = *lock_selected();
    let horizontal = crate::config::get().style.horizontal;
    let (candidates, commits) = build_candidates(expr, selected, horizontal);
    *lock_committable() = commits;

    context.update(|c| {
        c.suggestion_state = None;
        c.is_composing = true;
        c.composition.preedit = preedit.clone();
    });

    if candidates.candies.is_empty() {
        // 算式没输完：只画组词串，不弹候选栏（候选栏里出现的永远是
        // 能上屏的结果或明确的错误）。
        window.hide();
        WINDOW_SHOWN.store(false, Ordering::Relaxed);
    } else {
        // 位置锚在进入点（见 ANCHOR）；已显示时只刷新内容，不再重复 show。
        let anchor = *lock_anchor();
        let (x, y) = window_origin(anchor, context.read(|c| (c.caret_x, c.caret_y)));
        if !WINDOW_SHOWN.swap(true, Ordering::Relaxed) {
            window.show(x, y);
        }
        window.update(&Context {
            preedit: Text { str: preedit.clone() },
            commit: None,
            candidates: candidates.clone(),
        });
    }

    IpcResponse {
        context: Some(Context {
            preedit: Text { str: preedit },
            commit: None,
            candidates,
        }),
        status: Some(calculator_status(eng)),
        ..IpcResponse::ok(session_id)
    }
}

/// 关闭组词串的回包（无提交、无候选）。
fn close_response(
    session_id: u32,
    eng: &RimeEngine,
    context: &std::sync::Arc<SharedInputContext>,
    window: &std::sync::Arc<CandidateWindow>,
) -> IpcResponse {
    let _ = lock_shadow().take();
    cancel_shadow_idle_commit();
    context.update(|c| {
        c.suggestion_state = None;
        c.is_composing = false;
        c.composition.preedit.clear();
    });
    *lock_anchor() = None;
    WINDOW_SHOWN.store(false, Ordering::Relaxed);
    lock_committable().clear();
    *lock_selected() = 0;
    window.hide();
    IpcResponse {
        context: Some(Context {
            preedit: Text { str: String::new() },
            commit: None,
            candidates: CandidateInfo::default(),
        }),
        status: Some(crate::ipc_server::get_ipc_status(eng)),
        ..IpcResponse::ok(session_id)
    }
}

/// 构造计算器候选：结果（可算时）+ 原式（可切换/点击上屏）。
///
/// - `Value`：`[结果, 原式]`（结果与原式相同则只留一条）；两条都可选，
///   `highlighted` 跟随 `selected`，空格上屏选中的那条。
/// - `Error`：`[错误提示, 原式]`；错误提示不可上屏。
/// - `Incomplete`：只显示原式，不画选中高亮。
///
/// 返回值第二项是与候选一一对应的上屏文本（`None` = 仅展示）。
fn build_candidates(
    expr: &str,
    selected: usize,
    horizontal: bool,
) -> (CandidateInfo, Vec<Option<String>>) {
    let mut candies: Vec<Text> = Vec::new();
    let mut comments: Vec<Text> = Vec::new();
    let mut commits: Vec<Option<String>> = Vec::new();
    let outcome = eval_outcome(expr);
    let mut highlighted = usize::MAX;

    if !expr.is_empty() {
        let disp = display_preedit(expr);
        match &outcome {
            Outcome::Value(value) => {
                let result = format_result(*value);
                candies.push(Text {
                    str: result.clone(),
                });
                comments.push(Text {
                    str: "空格上屏结果".to_string(),
                });
                commits.push(Some(result.clone()));
                if disp != result {
                    candies.push(Text { str: disp.clone() });
                    comments.push(Text {
                        str: "空格上屏原式".to_string(),
                    });
                    commits.push(Some(disp));
                }
            }
            Outcome::Error(message) => {
                candies.push(Text {
                    str: (*message).to_string(),
                });
                comments.push(Text {
                    str: String::new(),
                });
                commits.push(None);
                candies.push(Text { str: disp.clone() });
                comments.push(Text {
                    str: "点击上屏原式".to_string(),
                });
                commits.push(Some(disp));
            }
            Outcome::Incomplete => {
                candies.push(Text { str: disp.clone() });
                comments.push(Text {
                    str: "继续输入 · 点击上屏原式".to_string(),
                });
                commits.push(Some(disp));
            }
        }
    }

    // 选中项只在「至少两条可上屏」时存在；否则沿用旧行为（结果单条高亮 / 无高亮）。
    let selectable: Vec<usize> = commits
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.is_some().then_some(i))
        .collect();
    if selectable.len() >= 2 {
        let pos = selectable.iter().position(|i| *i == selected).unwrap_or(0);
        highlighted = selectable[pos];
        let hint = if horizontal {
            "（←→切换）"
        } else {
            "（↑↓切换）"
        };
        if let Some(first) = selectable.first().and_then(|i| comments.get_mut(*i)) {
            first.str.push_str(hint);
        }
    } else if matches!(outcome, Outcome::Value(_)) {
        highlighted = 0;
    }

    let labels = candies
        .iter()
        .map(|_| Text {
            str: String::new(),
        })
        .collect();
    let info = CandidateInfo {
        current_page: 0,
        total_pages: 1,
        highlighted,
        is_last_page: true,
        candies,
        comments,
        labels,
    };
    (info, commits)
}

/// 按键归类。
enum CalcAction {
    /// 把字符追加进算式。
    Input(char),
    Backspace,
    /// Esc：清空算式并退出。
    Clear,
    /// 空格 / 回车 / `=`：有结果就上屏；空算式退出；未成形则保持打开。
    CommitAttempt,
    /// 方向键在当前布局的轴上切换候选（横向 ←/→，纵向 ↑/↓）。
    SelectPrev,
    SelectNext,
    /// 键不归计算器：退出并把键交给正常流程。
    Delegate,
}

/// keysym → 动作。Ctrl/Alt 组合、key-up 一律不归计算器（红线：移动光标和
/// 快捷键是用户的基本能力；Shift 没问题——移位字符已由 keysym 承载）。
///
/// `horizontal` 决定候选切换用哪根轴：横向候选用 ←/→，纵向候选用 ↑/↓；
/// **另一根轴仍然退出**（把光标移动能力留给用户），这是红线在计算器里的折中。
fn action_for_key(keysym: i32, modifiers: i32, horizontal: bool) -> Option<CalcAction> {
    use librime::key::{K_ALT_MASK, K_CONTROL_MASK, K_RELEASE_MASK};

    if modifiers & (K_RELEASE_MASK as i32) != 0 {
        return None;
    }
    if modifiers & ((K_CONTROL_MASK | K_ALT_MASK) as i32) != 0 {
        return Some(CalcAction::Delegate);
    }

    if horizontal {
        match keysym {
            XK_LEFT_I => return Some(CalcAction::SelectPrev),
            XK_RIGHT_I => return Some(CalcAction::SelectNext),
            _ => {}
        }
    } else {
        match keysym {
            XK_UP_I => return Some(CalcAction::SelectPrev),
            XK_DOWN_I => return Some(CalcAction::SelectNext),
            _ => {}
        }
    }

    let action = match keysym {
        // 数字行 0-9（ASCII 数字 keysym）
        0x30..=0x39 => CalcAction::Input(keysym as u8 as char),
        // `.` `+` `-` `*` `/` `(` `)` `%` `^`（Shift 移位后的字符 keysym 已由 TSF 换算）
        0x2E | 0x2B | 0x2D | 0x2A | 0x2F | 0x28 | 0x29 | 0x25 | 0x5E => {
            CalcAction::Input(keysym as u8 as char)
        }
        // 再次按触发的 `=`：有结果就上屏（见模块注释）；没有结果则退出并放行，
        // 让 rime 的 punctuator 把 `=` 当作普通的 ＝ 提交。
        librime::key::XK_EQUAL => CalcAction::CommitAttempt,
        // 小键盘：数字与四则运算（2026-10-23 起小键盘键码已正确映射 KP_*）
        librime::key::XK_KP_0..=XK_KP_9 => {
            let digit = (keysym - librime::key::XK_KP_0) as u8;
            CalcAction::Input((b'0' + digit) as char)
        }
        librime::key::XK_KP_DECIMAL => CalcAction::Input('.'),
        librime::key::XK_KP_ADD => CalcAction::Input('+'),
        librime::key::XK_KP_SUBTRACT => CalcAction::Input('-'),
        librime::key::XK_KP_MULTIPLY => CalcAction::Input('*'),
        librime::key::XK_KP_DIVIDE => CalcAction::Input('/'),
        // 编辑 / 提交
        XK_BACKSPACE => CalcAction::Backspace,
        XK_ESC => CalcAction::Clear,
        XK_ENTER | XK_SPC => CalcAction::CommitAttempt,
        // 字母、方向键、Tab、功能键、一切没列出的键：退出 + 放行
        _ => CalcAction::Delegate,
    };
    Some(action)
}

/// 求值结果三态。
#[derive(Debug)]
enum Outcome {
    /// 没输完（末尾是运算符/开括号/小数点，或括号未配平）：不显示候选。
    Incomplete,
    /// 明确的错误（不可上屏）。
    Error(&'static str),
    /// 可上屏的数值。
    Value(f64),
}

impl Outcome {
    fn value(&self) -> Option<f64> {
        match self {
            Outcome::Value(v) => Some(*v),
            _ => None,
        }
    }
}

/// 算式当前能不能给出候选。
fn eval_outcome(expr: &str) -> Outcome {
    if expr.is_empty() {
        return Outcome::Incomplete;
    }
    let Some(last) = expr.chars().last() else {
        return Outcome::Incomplete;
    };
    // 末尾是运算符 / 开括号 / 小数点：用户还在输入。
    if matches!(last, '+' | '-' | '*' | '/' | '%' | '^' | '(' | '.') {
        return Outcome::Incomplete;
    }
    // 括号还没配平：用户在输入，不该先报错。
    let opens = expr.chars().filter(|c| *c == '(').count();
    let closes = expr.chars().filter(|c| *c == ')').count();
    if opens > closes {
        return Outcome::Incomplete;
    }
    match evaluate(expr) {
        Ok(value) => Outcome::Value(value),
        Err(CalcError::NotFinite) => Outcome::Error(ERR_DIV_ZERO),
        Err(CalcError::Invalid | CalcError::TooLong) => Outcome::Error(ERR_INVALID),
    }
}

enum CalcError {
    /// 除零 / 溢出 / NaN。
    NotFinite,
    /// 结构错误（解析失败、未知符号）。
    Invalid,
    TooLong,
}

/// 求值：fasteval2（MIT、0 运行期依赖、纯 f64）+ 内置常量与常用函数。
/// 领域语义留在调用侧：没输完不报错（[`eval_outcome`]）、非有限结果按除零报、
/// 显示格式与上屏文本同源。
fn evaluate(expr: &str) -> Result<f64, CalcError> {
    if expr.chars().count() > MAX_EXPR_LEN {
        return Err(CalcError::TooLong);
    }
    // `ez_eval` 一次分配一个 Slab：计算器是逐键求值、算式很短，
    // 复用 Slab 的复杂度不值得（编译模式才是「同一条表达式反复算」的优化）。
    let value = fasteval2::ez_eval(expr, &mut |sym: &str, args: Vec<f64>| {
        eval_symbol(sym, &args)
    })
    .map_err(|_| CalcError::Invalid)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(CalcError::NotFinite)
    }
}

/// fasteval2 的符号回调：常量与一元/二元函数。返回 `None` = 未知符号（报错）。
fn eval_symbol(sym: &str, args: &[f64]) -> Option<f64> {
    let x = || args.first().copied();
    let xy = || {
        if args.len() == 2 {
            Some((args[0], args[1]))
        } else {
            None
        }
    };
    match sym {
        "pi" => Some(std::f64::consts::PI),
        "e" => Some(std::f64::consts::E),
        "sqrt" => x().map(f64::sqrt),
        "abs" => x().map(f64::abs),
        "sin" => x().map(f64::sin),
        "cos" => x().map(f64::cos),
        "tan" => x().map(f64::tan),
        "asin" => x().map(f64::asin),
        "acos" => x().map(f64::acos),
        "atan" => x().map(f64::atan),
        "ln" => x().map(f64::ln),
        "log" => x().map(f64::log10),
        "exp" => x().map(f64::exp),
        "round" => x().map(f64::round),
        "floor" => x().map(f64::floor),
        "ceil" => x().map(f64::ceil),
        "min" => xy().map(|(a, b)| a.min(b)),
        "max" => xy().map(|(a, b)| a.max(b)),
        "pow" => xy().map(|(a, b)| a.powf(b)),
        _ => None,
    }
}

/// 结果显示（= 上屏文本，同源）：最多 10 位小数、去尾零；`-0` 显示 `0`。
fn format_result(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let text = format!("{value:.10}");
    let text = text.trim_end_matches('0');
    let text = text.trim_end_matches('.');
    text.to_string()
}

/// 组词串显示：`*` / `/` 换成 `×` / `÷`（缓冲里保留原字符）。
fn display_preedit(expr: &str) -> String {
    expr.chars()
        .map(|c| match c {
            '*' => '×',
            '/' => '÷',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use librime::key::{
        K_ALT_MASK, K_CONTROL_MASK, K_RELEASE_MASK, K_SHIFT_MASK, XK_EQUAL, XK_KP_0, XK_KP_ADD,
    };

    /// 触碰全局 [`BUFFER`] 的用例用它串行（cargo test 默认多线程）。
    static STATE_LOCK: Mutex<()> = Mutex::new(());

    fn lock_state() -> MutexGuard<'static, ()> {
        match STATE_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    // ---------- 结果格式化 ----------

    #[test]
    fn format_result_trims_and_handles_zero() {
        assert_eq!(format_result(408.0), "408");
        assert_eq!(format_result(0.1 + 0.2), "0.3");
        assert_eq!(format_result(-0.0), "0");
        assert_eq!(format_result(3.5), "3.5");
        assert_eq!(format_result(-7.0 / 2.0), "-3.5");
    }

    // ---------- 求值 ----------

    #[test]
    fn evaluate_arithmetic_precedence_and_functions() {
        assert_eq!(evaluate("1+2*3").unwrap_or(0.0), 7.0);
        assert_eq!(evaluate("(1+2)*3").unwrap_or(0.0), 9.0);
        assert_eq!(evaluate("7/2").unwrap_or(0.0), 3.5);
        assert_eq!(evaluate("7%3").unwrap_or(0.0), 1.0);
        assert_eq!(evaluate("-5+10").unwrap_or(0.0), 5.0);
        assert_eq!(evaluate("-(2+3)*2").unwrap_or(0.0), -10.0);
        assert_eq!(evaluate("2^10").unwrap_or(0.0), 1024.0);
        assert_eq!(evaluate("sqrt(9)").unwrap_or(0.0), 3.0);
        assert_eq!(evaluate("max(3,7)+min(1,2)").unwrap_or(0.0), 8.0);
        let tau = evaluate("pi*2").unwrap_or(0.0);
        assert!((tau - std::f64::consts::TAU).abs() < 1e-12);
    }

    #[test]
    fn evaluate_errors_are_classified() {
        assert!(matches!(evaluate("1/0"), Err(CalcError::NotFinite)));
        assert!(matches!(evaluate("5++"), Err(CalcError::Invalid)));
        assert!(matches!(evaluate("foo(1)"), Err(CalcError::Invalid)));
        assert!(matches!(evaluate(&"1+1".repeat(40)), Err(CalcError::TooLong)));
    }

    // ---------- 候选三态 ----------

    #[test]
    fn eval_outcome_shows_nothing_until_meaningful() {
        assert!(matches!(eval_outcome(""), Outcome::Incomplete));
        assert!(matches!(eval_outcome("5+"), Outcome::Incomplete));
        assert!(matches!(eval_outcome("5*("), Outcome::Incomplete));
        assert!(matches!(eval_outcome("5."), Outcome::Incomplete));
        // 括号没配平：还在输入，不该先弹「算式有误」。
        assert!(matches!(eval_outcome("(1+2"), Outcome::Incomplete));
        assert!(matches!(eval_outcome("1/0"), Outcome::Error(ERR_DIV_ZERO)));
        assert!(matches!(eval_outcome("1+x"), Outcome::Error(ERR_INVALID)));
        // 完整的裸数字也算结果：`=` `5` 空格应上屏 5，而不是把 5 悄悄丢掉。
        match eval_outcome("5") {
            Outcome::Value(v) => assert_eq!(v, 5.0),
            other => panic!("裸数字应为可上屏结果，实际 {other:?}"),
        }
        match eval_outcome("12+34") {
            Outcome::Value(v) => assert_eq!(v, 46.0),
            other => panic!("应为可上屏结果，实际 {other:?}"),
        }
        match eval_outcome("sqrt(2)") {
            Outcome::Value(v) => assert!((v - 2.0f64.sqrt()).abs() < 1e-12),
            other => panic!("函数调用也算可上屏结果，实际 {other:?}"),
        }
    }

    // ---------- 按键映射 ----------

    #[test]
    fn keys_map_to_calculator_actions() {
        let of = |k: i32, m: i32| action_for_key(k, m, true);
        // 算式输入键（含 Shift 移位字符与小键盘）
        assert!(matches!(of(b'5' as i32, 0), Some(CalcAction::Input('5'))));
        assert!(matches!(
            of(b'*' as i32, K_SHIFT_MASK as i32),
            Some(CalcAction::Input('*'))
        ));
        assert!(matches!(
            of(b'(' as i32, K_SHIFT_MASK as i32),
            Some(CalcAction::Input('(')
        )));
        assert!(matches!(of(XK_KP_0 + 3, 0), Some(CalcAction::Input('3'))));
        assert!(matches!(of(XK_KP_ADD, 0), Some(CalcAction::Input('+'))));
        // 编辑 / 提交
        assert!(matches!(of(XK_EQUAL, 0), Some(CalcAction::CommitAttempt)));
        assert!(matches!(of(XK_BACKSPACE, 0), Some(CalcAction::Backspace)));
        assert!(matches!(of(XK_ESC, 0), Some(CalcAction::Clear)));
        assert!(matches!(of(XK_SPC, 0), Some(CalcAction::CommitAttempt)));
        assert!(matches!(of(XK_ENTER, 0), Some(CalcAction::CommitAttempt)));
        // 红线：字母、Ctrl/Alt 组合、key-up 都不归计算器
        assert!(matches!(of(b'a' as i32, 0), Some(CalcAction::Delegate)));
        assert!(matches!(of(b'A' as i32, 0), Some(CalcAction::Delegate)));
        assert!(matches!(
            of(b'a' as i32, K_CONTROL_MASK as i32),
            Some(CalcAction::Delegate)
        ));
        assert!(matches!(
            of(b'a' as i32, K_ALT_MASK as i32),
            Some(CalcAction::Delegate)
        ));
        assert!(of(b'a' as i32, K_RELEASE_MASK as i32).is_none());
        // 横向布局：←/→ 切换候选；↑/↓ 仍退出（把光标移动还给用户）
        assert!(matches!(of(XK_LEFT_I, 0), Some(CalcAction::SelectPrev)));
        assert!(matches!(of(XK_RIGHT_I, 0), Some(CalcAction::SelectNext)));
        assert!(matches!(of(XK_UP_I, 0), Some(CalcAction::Delegate)));
        // 纵向布局：↑/↓ 切换；←/→ 退出
        assert!(matches!(
            action_for_key(XK_UP_I, 0, false),
            Some(CalcAction::SelectPrev)
        ));
        assert!(matches!(
            action_for_key(XK_DOWN_I, 0, false),
            Some(CalcAction::SelectNext)
        ));
        assert!(matches!(
            action_for_key(XK_LEFT_I, 0, false),
            Some(CalcAction::Delegate)
        ));
        // 功能键 / Tab 任何时候都放行
        assert!(matches!(of(0xFF09, 0), Some(CalcAction::Delegate))); // Tab
        assert!(matches!(of(0xFFBE, 0), Some(CalcAction::Delegate))); // F1
    }

    // ---------- 缓冲状态机 ----------

    #[test]
    fn buffer_transitions_cover_enter_backspace_and_exit() {
        let _guard = lock_state();
        *lock_buffer() = Some(String::new());
        assert!(is_active());

        {
            let mut guard = lock_buffer();
            if let Some(expr) = guard.as_mut() {
                expr.push('1');
                expr.push('2');
            }
        }
        assert_eq!(lock_buffer().as_deref(), Some("12"));

        {
            let mut guard = lock_buffer();
            if let Some(expr) = guard.as_mut() {
                expr.pop();
            }
        }
        assert_eq!(lock_buffer().as_deref(), Some("1"));

        // 退格到底后缓冲为空串仍激活（组词串还开着，下一个键必须仍归计算器）
        {
            let mut guard = lock_buffer();
            if let Some(expr) = guard.as_mut() {
                expr.pop();
            }
        }
        assert_eq!(lock_buffer().as_deref(), Some(""));
        assert!(is_active());

        // 外部复位（FocusOut 等）后彻底退出；再复位一次不炸也不刷日志
        reset("FocusOut");
        assert!(!is_active());
        reset("FocusOut");
    }

    // ---------- 触发键让位检测 ----------

    #[test]
    fn builtin_trigger_coexists_with_scheme_calculator() {
        // moqi 形态：自带计算器触发 V + 大写 recognizer + punctuator 的 ＝
        // —— `=` 没被占用，内置计算器与方案计算器可同时可用。
        let moqi_like = "\
calculator:
  trigger: V
recognizer:
  patterns:
    uppercase: \"^[A-Z].*$\"
    punct: \"^/([0-9]+|[A-Za-z]+)?$\"
punctuator:
  half_shape:
    \"=\" : { commit: ＝ }
";
        assert!(!schema_uses_trigger(moqi_like, '='));
    }

    #[test]
    fn builtin_trigger_yields_to_scheme_owner() {
        // recognizer 以 = 开头 → 让位
        assert!(schema_uses_trigger(
            "recognizer:\n  patterns:\n    calc: \"^=[0-9].*\"\n",
            '='
        ));
        // 自带计算器触发就是 = → 让位
        assert!(schema_uses_trigger("calculator:\n  trigger: =\n", '='));
        // 无条件 key_binder 绑定 equal → 让位
        assert!(schema_uses_trigger(
            "key_binder:\n  bindings:\n    - {accept: equal, send: Page_Down}\n",
            '='
        ));
        // 有限定的绑定（组词串为空时不会触发）与普通大写模式 → 不让位
        assert!(!schema_uses_trigger(
            "key_binder:\n  bindings:\n    - {when: has_menu, accept: equal, send: Page_Down}\n",
            '='
        ));
        // speller 把 = 当编码字符 → 让位
        assert!(schema_uses_trigger(
            "speller:\n  alphabet: \"abc=\"\n",
            '='
        ));
        assert!(!schema_uses_trigger(
            "recognizer:\n  patterns:\n    uppercase: \"^[A-Z].*$\"\n",
            '='
        ));
        // 坏 YAML 不炸、也不让位（方案缺文件同语义：内置生效）
        assert!(!schema_uses_trigger("::: not yaml [[[", '='));
    }

    // ---------- 组词串显示 ----------

    #[test]
    fn preedit_prettifies_operators() {
        assert_eq!(display_preedit("12*34/5"), "12×34÷5");
        assert_eq!(display_preedit("1+2-3"), "1+2-3");
    }

    /// 失焦提交只能取走一次：TSF 在旧 context 上提交后，下一个 FocusOut 不能再拿到
    /// 同一串数字（否则会重复上屏）。
    #[test]
    fn shadow_flush_takes_digits_once() {
        let _guard = match STATE_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *lock_shadow() = Some("12".to_string());
        assert_eq!(take_shadow_for_flush().as_deref(), Some("12"));
        assert!(take_shadow_for_flush().is_none());
        assert!(lock_shadow().is_none());
    }

    /// 正式开关：缺省/类型不对都按开启；只有显式 `auto_detect: false` 才关。
    #[test]
    fn auto_detect_config_defaults_on() {
        assert!(auto_detect_from_yaml(""));
        assert!(auto_detect_from_yaml("calculator: {}"));
        assert!(!auto_detect_from_yaml("calculator:\n  auto_detect: false\n"));
        assert!(auto_detect_from_yaml("calculator:\n  auto_detect: true\n"));
        assert!(auto_detect_from_yaml("calculator:\n  auto_detect: yes-please\n"));
    }

    /// B2 影子缓冲的按键分类：数字/小数点累积、运算符接管、其余回放。
    #[test]
    fn shadow_keys_classified() {
        assert_eq!(classify_shadow_key(0x31), ShadowAction::Append('1'));
        assert_eq!(classify_shadow_key(0x2E), ShadowAction::Append('.'));
        assert_eq!(classify_shadow_key(XK_KP_0 + 3), ShadowAction::Append('3'));
        assert_eq!(classify_shadow_key(0x2B), ShadowAction::Operator('+'));
        assert_eq!(classify_shadow_key(XK_KP_ADD), ShadowAction::Operator('+'));
        assert_eq!(classify_shadow_key(XK_EQUAL), ShadowAction::Finish);
        assert_eq!(classify_shadow_key(XK_BACKSPACE), ShadowAction::Backspace);
        assert_eq!(classify_shadow_key(XK_ESC), ShadowAction::Clear);
        // 字母/空格等非算式键走回放
        assert_eq!(classify_shadow_key(0x61), ShadowAction::Replay);
        assert_eq!(classify_shadow_key(0x20), ShadowAction::Replay);
        // 回车直接提交数字；F1–F24 放行给宿主，不能注入。
        assert_eq!(classify_shadow_key(XK_ENTER), ShadowAction::CommitDigits);
        assert_eq!(classify_shadow_key(0xFF8D), ShadowAction::CommitDigits);
        assert_eq!(classify_shadow_key(0xFFBE), ShadowAction::PassThrough);
        assert_eq!(classify_shadow_key(0xFFC2), ShadowAction::PassThrough);

        // 影子起点：主键盘数字与小键盘数字都算；字母不算。
        assert!(is_shadow_starter(0x31));
        assert!(is_shadow_starter(XK_KP_0 + 1));
        assert!(!is_shadow_starter(0x61));
    }

    // ---------- 候选栏位置 / 选中态 ----------

    /// 会话内有锚点就必须用锚点：宿主每次按键报的是组词串末端，跟着它会滑动。
    #[test]
    fn window_origin_prefers_session_anchor() {
        assert_eq!(window_origin(Some((10, 20)), (99, 99)), (10, 20));
        assert_eq!(window_origin(None, (99, 99)), (99, 99));
    }

    /// 结果 + 原式两条候选：默认高亮结果，两条都可上屏。
    #[test]
    fn candidates_expose_result_and_expression() {
        let (info, commits) = build_candidates("12+34", 0, true);
        assert_eq!(info.candies.len(), 2);
        assert_eq!(info.candies[0].str, "46");
        assert_eq!(info.highlighted, 0);
        assert!(info.labels.iter().all(|l| l.str.is_empty()));
        assert_eq!(
            commits,
            vec![Some("46".to_string()), Some("12+34".to_string())]
        );
    }

    /// 选中项跟随 `selected`；切换轴提示按布局写在第一条候选的注释里。
    #[test]
    fn selection_moves_highlight_and_hint_follows_layout() {
        let (info, _) = build_candidates("12+34", 1, true);
        assert_eq!(info.highlighted, 1);
        assert!(info.comments[0].str.contains("←→"));

        let (info, _) = build_candidates("12+34", 1, false);
        assert_eq!(info.highlighted, 1);
        assert!(info.comments[0].str.contains("↑↓"));
    }

    /// 未成形：只显示原式、不画选中高亮；原式仍可点击上屏。
    #[test]
    fn incomplete_candidate_is_expression_only() {
        let (info, commits) = build_candidates("12+", 0, true);
        assert_eq!(info.candies.len(), 1);
        assert_eq!(info.candies[0].str, "12+");
        assert_eq!(info.highlighted, usize::MAX);
        assert_eq!(commits, vec![Some("12+".to_string())]);
    }

    /// 错误提示只展示、不可点击；原式仍可点。
    #[test]
    fn error_candidate_is_not_committable() {
        let (info, commits) = build_candidates("1/0", 0, true);
        assert_eq!(info.candies.len(), 2);
        assert_eq!(info.candies[0].str, ERR_DIV_ZERO);
        assert_eq!(commits[0], None);
        // 原式按**显示形态**上屏（与组词串一致：`/` 已显示为 `÷`）。
        assert_eq!(commits[1], Some("1÷0".to_string()));
        assert_eq!(info.highlighted, usize::MAX);
    }

    /// 裸数字只出现一条：结果与原式相同，不重复画。
    #[test]
    fn bare_number_does_not_duplicate_expression() {
        let (info, commits) = build_candidates("5", 0, true);
        assert_eq!(info.candies.len(), 1);
        assert_eq!(info.candies[0].str, "5");
        assert_eq!(commits, vec![Some("5".to_string())]);
    }
}
