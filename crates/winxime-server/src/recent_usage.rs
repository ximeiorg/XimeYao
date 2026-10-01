//! 面板「最近使用」记录（表情 / 符号各自一份）。
//!
//! 语义对齐安卓版 `RecentUsageStore`：
//!
//! - **LRU，不是频次**：点一次就置顶去重、最久没用的排末尾、超过上限截断。
//!   安卓那边的注释说得对——按点击次数排序会让早期高频项固化霸榜，
//!   而「最近使用」的字面语义就是时间序。
//! - **上限 32**：正好是网格页一页的容量（8 列 × 4 行），所以「最近」标签只有一页。
//! - **表情与符号各存一份**：键名照抄安卓的 `recent_emojis` / `recent_symbols`。
//! - **落在 `%APPDATA%\Xime\recent_usage.json`**：安卓把 JSON 数组放进
//!   SharedPreferences，这里等价。不塞进 clipboard.db——这份数据只属于面板，
//!   和剪贴板历史没有关系，也不该让设置程序去管。
//! - **读写失败一律不打扰用户**：文件读坏 / 写不进去只记一行日志、当成空表。
//!   面板不是关键路径，更不能因为一个记录文件打不开就让「点表情上屏」失败。
//!
//! 「在最近使用页里点按不重排」这条规则**不在这里**：那是 UI 的事
//! （见 `ui::view` 的 `GlyphCell` 分支），本模块只提供纯粹的 LRU 与读写。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{json, Value};

/// 最近使用上限（对齐安卓 `RecentUsageStore.MAX_COUNT`）。
pub(crate) const MAX_COUNT: usize = 32;

/// 记录种类：表情与符号各一条（安卓用两个 key 区分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecentKind {
    Emoji,
    Symbol,
}

impl RecentKind {
    /// JSON 里的键名（与安卓的 SharedPreferences key 同名，便于对照）。
    fn key(self) -> &'static str {
        match self {
            Self::Emoji => "recent_emojis",
            Self::Symbol => "recent_symbols",
        }
    }

    /// 日志里给人看的名字。
    fn label(self) -> &'static str {
        match self {
            Self::Emoji => "表情",
            Self::Symbol => "符号",
        }
    }
}

/// 记录文件路径（`main` 启动时按数据根注册）。
static STORE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 注册记录文件路径（`%APPDATA%\Xime\recent_usage.json`，与 clipboard.db 同目录）。
pub(crate) fn set_store_path(path: PathBuf) {
    let _ = STORE_PATH.set(path);
}

/// 记录文件路径；未注册时按 xime-config 的剪贴板库默认路径取同目录
/// （与 `ui::panel` 里剪贴板库路径的回退口径一致：默认数据根只有一个来源）。
fn store_path() -> PathBuf {
    STORE_PATH.get().cloned().unwrap_or_else(|| {
        xime_config::clipboard_store::default_db_path()
            .parent()
            .map_or_else(
                || PathBuf::from("recent_usage.json"),
                |dir| dir.join("recent_usage.json"),
            )
    })
}

/// LRU 核心（纯函数，便于对着安卓那条语义写单测）：`value` 置顶去重后截断到 `max`。
/// 纯时间序——不数点击次数。
pub(crate) fn record(list: &[String], value: &str, max: usize) -> Vec<String> {
    let mut next: Vec<String> = Vec::with_capacity(list.len().min(max) + 1);
    next.push(value.to_string());
    next.extend(list.iter().filter(|item| item.as_str() != value).cloned());
    next.truncate(max);
    next
}

/// 从 JSON 文本里取某一类的记录。
/// 不是 JSON 对象 / 键不存在 / 元素不是字符串，都当空表（容错优先，记录不值得报错）。
fn parse_kind(raw: &str, kind: RecentKind) -> Vec<String> {
    let doc: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    doc.get(kind.key())
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .take(MAX_COUNT)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 把某一类的记录写回 JSON 文本；**另一类原样保留**（两类共用一个文件）。
fn with_kind(raw: &str, kind: RecentKind, list: &[String]) -> String {
    let mut doc: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    if !doc.is_object() {
        doc = json!({});
    }
    if let Some(map) = doc.as_object_mut() {
        map.insert(kind.key().to_string(), json!(list));
    }
    // 缩进输出：这个文件是给人看一眼就懂的（排查「为什么没有最近使用」时直接打开）。
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

/// 读取某一类的最近使用（文件不存在 / 读坏都当空表）。
pub(crate) fn load(kind: RecentKind) -> Vec<String> {
    load_from(&store_path(), kind)
}

/// 记录一次使用并落盘，返回更新后的列表（面板据此刷新内存里的那份）。
pub(crate) fn record_use(kind: RecentKind, value: &str) -> Vec<String> {
    record_use_at(&store_path(), kind, value)
}

/// 按路径读取（路径可注入：单测用临时文件，不碰真实数据根）。
fn load_from(path: &Path, kind: RecentKind) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => parse_kind(&raw, kind),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::warn!("读取{}最近使用失败（{}）: {e}", kind.label(), path.display());
            Vec::new()
        }
    }
}

/// 按路径记录一次使用（路径可注入，见 [`load_from`]）。
fn record_use_at(path: &Path, kind: RecentKind, value: &str) -> Vec<String> {
    // 读不到（不存在 / 被拒）就当空表继续：用户刚点了东西，这一次记录不该丢。
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let updated = record(&parse_kind(&raw, kind), value, MAX_COUNT);
    let doc = with_kind(&raw, kind, &updated);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, doc) {
        tracing::warn!("写入{}最近使用失败（{}）: {e}", kind.label(), path.display());
    }
    updated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    /// 每次调用都拿到一个全新的临时目录（名字带自增序号：不能用 pid 当唯一后缀，
    /// 环境里已经堆着同名旧目录，撞上别人的残留目录会被 ACL 拒掉）。
    fn temp_dir(label: &str) -> Option<PathBuf> {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "xime_recent_{}_{}_{}",
            label,
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir_all(&dir) {
            Ok(()) => Some(dir),
            // 临时目录不可写就跳过：记录文件不是关键路径，测试不该把环境问题
            // 算成代码失败（本仓库已有几条这样的基线失败，不再添新的）。
            Err(e) => {
                eprintln!("跳过 {label}：临时目录不可写（{e}）");
                None
            }
        }
    }

    #[test]
    fn record_prepends_the_new_value() {
        assert_eq!(
            record(&strings(&["a", "b"]), "c", 32),
            strings(&["c", "a", "b"])
        );
    }

    #[test]
    fn record_moves_an_existing_value_to_the_front_without_duplicating() {
        assert_eq!(
            record(&strings(&["a", "b", "c"]), "b", 32),
            strings(&["b", "a", "c"])
        );
    }

    #[test]
    fn record_is_time_ordered_not_frequency_ranked() {
        // 安卓那条语义：连点同一个值不会「累积权重」把它钉在榜首——
        // 别的值后来点过就该排在它前面。
        let mut list = Vec::new();
        for _ in 0..5 {
            list = record(&list, "😀", 32);
        }
        list = record(&list, "🎉", 32);
        assert_eq!(list, strings(&["🎉", "😀"]));
    }

    #[test]
    fn record_truncates_to_the_cap() {
        let list: Vec<String> = (0..40).map(|i| format!("e{i}")).collect();
        let updated = record(&list, "new", 32);
        assert_eq!(updated.len(), 32);
        assert_eq!(updated[0], "new");
        // 末尾那 8 个（最久没用的）被截掉。
        assert!(!updated.contains(&"e39".to_string()));
    }

    #[test]
    fn record_accepts_an_empty_history() {
        assert_eq!(record(&[], "😀", 32), strings(&["😀"]));
    }

    #[test]
    fn parse_ignores_garbage_and_foreign_shapes() {
        assert!(parse_kind("", RecentKind::Emoji).is_empty());
        assert!(parse_kind("{ not json", RecentKind::Emoji).is_empty());
        assert!(parse_kind("[]", RecentKind::Emoji).is_empty());
        assert!(parse_kind(r#"{"recent_symbols":["①"]}"#, RecentKind::Emoji).is_empty());
        // 元素不是字符串就跳过，不整体报废。
        assert_eq!(
            parse_kind(r#"{"recent_emojis":["😀",3,null,"🎉"]}"#, RecentKind::Emoji),
            strings(&["😀", "🎉"])
        );
    }

    #[test]
    fn writing_one_kind_keeps_the_other() {
        let raw = r#"{"recent_emojis":["😀"],"recent_symbols":["①"],"unrelated":1}"#;
        let doc = with_kind(raw, RecentKind::Emoji, &strings(&["🎉", "😀"]));
        let value: Value = serde_json::from_str(&doc).unwrap_or(Value::Null);
        assert_eq!(
            value.get("recent_symbols").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
        assert_eq!(value.get("unrelated").and_then(Value::as_i64), Some(1));
        assert_eq!(parse_kind(&doc, RecentKind::Emoji), strings(&["🎉", "😀"]));
        // 空文档也能直接写。
        let doc = with_kind("", RecentKind::Symbol, &strings(&["⇒"]));
        assert_eq!(parse_kind(&doc, RecentKind::Symbol), strings(&["⇒"]));
    }

    #[test]
    fn record_use_round_trips_through_the_file_and_keeps_kinds_apart() {
        let Some(dir) = temp_dir("roundtrip") else {
            return;
        };
        let path = dir.join("recent_usage.json");
        // 文件还不存在：读出来是空表。
        assert!(load_from(&path, RecentKind::Emoji).is_empty());

        record_use_at(&path, RecentKind::Emoji, "😀");
        record_use_at(&path, RecentKind::Emoji, "🎉");
        record_use_at(&path, RecentKind::Symbol, "①");
        assert_eq!(load_from(&path, RecentKind::Emoji), strings(&["🎉", "😀"]));
        assert_eq!(load_from(&path, RecentKind::Symbol), strings(&["①"]));

        // 再点一次旧的：置顶，且符号那份不被抹掉。
        record_use_at(&path, RecentKind::Emoji, "😀");
        assert_eq!(load_from(&path, RecentKind::Emoji), strings(&["😀", "🎉"]));
        assert_eq!(load_from(&path, RecentKind::Symbol), strings(&["①"]));

        // 目录不存在时自己建（首次安装时 %APPDATA%\Xime 可能还没有这个文件）。
        let nested = dir.join("nested").join("recent_usage.json");
        record_use_at(&nested, RecentKind::Symbol, "⇒");
        assert_eq!(load_from(&nested, RecentKind::Symbol), strings(&["⇒"]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_does_not_block_recording() {
        let Some(dir) = temp_dir("corrupt") else {
            return;
        };
        let path = dir.join("recent_usage.json");
        assert!(std::fs::write(&path, "{ this is not json").is_ok());
        assert!(load_from(&path, RecentKind::Symbol).is_empty());
        record_use_at(&path, RecentKind::Symbol, "⇒");
        assert_eq!(load_from(&path, RecentKind::Symbol), strings(&["⇒"]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}