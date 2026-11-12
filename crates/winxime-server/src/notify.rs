//! 面向用户的系统通知（统一入口）。
//!
//! 为什么不直接到处调 `toast::show_toast`：
//!
//! - 同一件事可能被多条路径触发（设置页自己弹 + 服务端也弹），重复弹窗很烦；
//! - 批量重活时（部署 + 同步 + 下载）可能一次刷出好几条，把通知中心淹掉；
//! - 排查"到底弹没弹"时，日志里得有一条对得上的记录。
//!
//! 规则很简单：**同一内容 10 秒内只弹一次；10 秒内最多 3 条**。
//! 被挡掉的通知一律留日志（去重 debug、限流 warn），不会静默吞掉。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 所有通知的统一标题（用户在通知中心一眼能认出是谁）。
pub const TITLE: &str = "曦码·曜";

/// 同一内容的去重窗口。
const DEDUP_WINDOW: Duration = Duration::from_secs(10);
/// 限流窗口与窗口内允许的条数。
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_MAX: usize = 3;

/// 判定结果（同时作为日志依据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Send,
    Duplicate,
    RateLimited,
}

/// 去重 + 限流状态。抽成独立结构体，用注入的 `now` 就能单测。
struct Throttle {
    last: Option<(String, Instant)>,
    sent: VecDeque<Instant>,
}

impl Throttle {
    const fn new() -> Self {
        Self {
            last: None,
            sent: VecDeque::new(),
        }
    }

    fn allow(&mut self, body: &str, now: Instant) -> Verdict {
        if let Some((last_body, at)) = &self.last {
            if last_body == body && now.duration_since(*at) < DEDUP_WINDOW {
                return Verdict::Duplicate;
            }
        }

        while let Some(front) = self.sent.front() {
            if now.duration_since(*front) >= RATE_WINDOW {
                self.sent.pop_front();
            } else {
                break;
            }
        }
        if self.sent.len() >= RATE_MAX {
            return Verdict::RateLimited;
        }

        self.sent.push_back(now);
        self.last = Some((body.to_string(), now));
        Verdict::Send
    }
}

static THROTTLE: Mutex<Throttle> = Mutex::new(Throttle::new());

/// 弹一条通知（统一标题）。`body` 面向普通用户，一句话说清"发生了什么"。
pub fn notify(body: impl Into<String>) {
    notify_titled(TITLE, body.into());
}

/// 带自定义标题的通知（设置页转发的 toast 走这里，事件来自别处时也共用限流）。
pub fn notify_titled(title: &str, body: String) {
    let verdict = match THROTTLE.lock() {
        Ok(mut throttle) => throttle.allow(&body, Instant::now()),
        // 锁中毒也不该把用户通知吞掉：放行，下一次再恢复状态。
        Err(_) => Verdict::Send,
    };

    match verdict {
        Verdict::Send => {
            tracing::debug!("通知: {title} — {body}");
            crate::toast::show_toast(title, &body);
        }
        Verdict::Duplicate => {
            tracing::debug!("通知去重（{DEDUP_WINDOW:?} 内已弹过同样内容）: {body}");
        }
        Verdict::RateLimited => {
            tracing::warn!(
                "通知限流（{RATE_WINDOW:?} 内已弹 {RATE_MAX} 条），本次不弹: {body}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同一内容在窗口内只弹一次，窗口过去后可以再弹。
    #[test]
    fn duplicate_body_is_suppressed_within_window() {
        let mut throttle = Throttle::new();
        let t0 = Instant::now();
        assert_eq!(throttle.allow("已部署", t0), Verdict::Send);
        assert_eq!(
            throttle.allow("已部署", t0 + Duration::from_secs(3)),
            Verdict::Duplicate
        );
        assert_eq!(
            throttle.allow("已部署", t0 + DEDUP_WINDOW + Duration::from_secs(1)),
            Verdict::Send
        );
    }

    /// 不同内容不会互相去重，但会被限流挡在第 4 条。
    #[test]
    fn rate_limit_caps_notifications_per_window() {
        let mut throttle = Throttle::new();
        let t0 = Instant::now();
        assert_eq!(throttle.allow("a", t0), Verdict::Send);
        assert_eq!(throttle.allow("b", t0), Verdict::Send);
        assert_eq!(throttle.allow("c", t0), Verdict::Send);
        assert_eq!(throttle.allow("d", t0), Verdict::RateLimited);

        // 窗口滑过去之后恢复
        assert_eq!(
            throttle.allow("d", t0 + RATE_WINDOW + Duration::from_secs(1)),
            Verdict::Send
        );
    }
}
