//! 用户词典词条的读取与写入（设置页「词典 → 浏览 / 新增 / 删除」）。
//!
//! librime 没有"读词条"的 C 接口，但 levers 的 `export_user_dict` 能把整本
//! 用户词典导出成文本码表（`词⇥码⇥频率`，头部是 `#/@` 元信息行）——设置页的
//! 浏览/搜索就建立在这条通道上：导出到临时文件 → 读回 → 解析/过滤 → 回传。
//!
//! 写入复用同一个通道的反向：`import_user_dict` 是**合并**语义，写一行码表
//! 即可新增一条（或写一行负频率做删除标记）。
//!
//! 与 Android 版（Xime `UserDictIoManager.kt`）的差别：那边经 JNI 遍历
//! `DbSource` 是为了"不落盘"，这里接受一次临时文件；文本格式完全一致。

use std::sync::atomic::{AtomicU64, Ordering};

use winxime_ipc::{DictEntry, MAX_DICT_ENTRIES};

/// 一次读取的结果：词库词条总数（未过滤）+ 本次回传的词条（已过滤、已截断）。
pub(crate) struct UserDictRead {
    /// 词库的词条总数（不受关键词过滤与条数上限影响）。
    pub total: i32,
    /// 命中的条数（**未**受条数上限影响，用来判断回传是否被截断）。
    pub matched: i32,
    /// 本次回传的词条。
    pub entries: Vec<DictEntry>,
}

/// 关键词过滤结果：命中总数 + 回传词条（最多 `MAX_DICT_ENTRIES` 条）。
pub(crate) struct Selection {
    /// 命中总数（未截断）。
    pub matched: usize,
    /// 回传词条（已截断）。
    pub entries: Vec<DictEntry>,
}

/// 临时导出文件名的序号（同一进程内并发读取时避免撞名）。
static EXPORT_SEQ: AtomicU64 = AtomicU64::new(0);

/// 临时导入文件名的序号（同一进程内并发写入时避免撞名）。
static ENTRY_SEQ: AtomicU64 = AtomicU64::new(0);

/// 解析 librime 导出的文本码表。
///
/// - 头部 `# ...` / `#@/...` 元信息行、空行忽略；
/// - 正文为 `词⇥码⇥频率`（码/频率缺失时分别退化为空串与 1）；
/// - 频率 < 0 是已删除词条（tombstone）。librime 的 formatter 本就不输出它们
///   （`table_db.cc` 的 `rime_table_entry_formatter`），这里再挡一次，避免
///   把 tombstone 当成正经词条展示。
pub(crate) fn parse_user_dict_text(text: &str) -> Vec<DictEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let word = fields.next().unwrap_or_default();
        if word.is_empty() {
            continue;
        }
        let code = fields.next().unwrap_or_default();
        let commits = fields
            .next()
            .and_then(|raw| raw.trim().parse::<i32>().ok())
            .unwrap_or(1);
        if commits < 0 {
            continue;
        }
        out.push(DictEntry {
            word: word.to_string(),
            code: code.to_string(),
            commits,
        });
    }
    out
}

/// 关键词是否命中词条（词或码，大小写不敏感）。
fn matches_query(entry: &DictEntry, needle: &str) -> bool {
    entry.word.to_lowercase().contains(needle) || entry.code.to_lowercase().contains(needle)
}

/// 按关键词过滤并截断到 `MAX_DICT_ENTRIES` 条（关键词为空即全部）。
///
/// 同时报出**未截断**的命中总数：命名管道单帧有上限，回传条数被截断时
/// 设置页要靠这个数告诉用户"命中超过 N 条，请补充关键词"。
pub(crate) fn select_entries(entries: &[DictEntry], query: &str) -> Selection {
    let needle = query.trim().to_lowercase();
    let mut matched = 0usize;
    let mut selected = Vec::new();
    for entry in entries {
        if !needle.is_empty() && !matches_query(entry, &needle) {
            continue;
        }
        matched += 1;
        if selected.len() < MAX_DICT_ENTRIES {
            selected.push(entry.clone());
        }
    }
    Selection {
        matched,
        entries: selected,
    }
}

/// 把词典名缩成可安全拼进文件名的标签（词典名来自 userdb 目录名）。
fn file_label(dict: &str) -> String {
    dict.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 读取一本用户词典的词条（经 librime 导出通道）。
///
/// 失败时返回给用户看的原因（不 panic）：服务未初始化、词库打不开/不是 userdb、
/// 临时文件读写失败等。
pub(crate) fn read_user_dict(dict: &str, query: &str) -> Result<UserDictRead, String> {
    let path = std::env::temp_dir().join(format!(
        "xime_dict_{}_{}_{}.txt",
        file_label(dict),
        std::process::id(),
        EXPORT_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let path_str = path.to_string_lossy().into_owned();

    let exported = librime::export_user_dict(dict, &path_str)
        .map_err(|e| format!("导出词库失败（词库可能正被输入法占用）：{e:?}"))?;

    let read = std::fs::read(&path).map_err(|e| format!("读取导出文件失败：{e}"));
    // 临时文件即用即删（失败也不影响结果）。
    let _ = std::fs::remove_file(&path);
    let bytes = read?;

    let text = String::from_utf8_lossy(&bytes);
    let entries = parse_user_dict_text(&text);
    let total = entries.len() as i32;
    if exported >= 0 && exported != total {
        tracing::warn!("用户词典 {dict}：librime 报 {exported} 条、解析出 {total} 条（已忽略无法解析的行）");
    }
    let selection = select_entries(&entries, query);
    Ok(UserDictRead {
        total,
        matched: selection.matched as i32,
        entries: selection.entries,
    })
}

/// 校验一条用户词条的输入（对齐安卓版 `checkEntryInput` 的规则）。
///
/// 词和编码去掉首尾空白后都必须有内容，且不能含制表符/换行（会破坏码表
/// 格式）；频率非零——> 0 是新增、< 0 是删除标记，0 两个语义都不成立。
/// 返回修剪后的 (词, 编码)。
pub(crate) fn validate_entry(
    word: &str,
    code: &str,
    commits: i32,
) -> Result<(String, String), String> {
    let word = word.trim();
    let code = code.trim();
    if word.is_empty() || code.is_empty() {
        return Err("词和编码都要填".to_string());
    }
    for (label, value) in [("词", word), ("编码", code)] {
        if value.contains('\t') || value.contains('\n') || value.contains('\r') {
            return Err(format!("{label}不能含制表符或换行"));
        }
    }
    if commits == 0 {
        return Err("频率要填正整数（新增），删除走删除按钮".to_string());
    }
    Ok((word.to_string(), code.to_string()))
}

/// 一行码表文本（与安卓版 `codeTableText` 逐字节一致：`词⇥码⇥频率⇥换行`）。
fn entry_line(word: &str, code: &str, commits: i32) -> String {
    format!("{word}\t{code}\t{commits}\n")
}

/// 写入一条用户词条（频率 > 0 新增；< 0 是删除标记/tombstone）。
///
/// librime 的 `import_user_dict` 是**合并**语义（`UserDictImporter`）：同词条取
/// 较大频率、负频率视为删除标记。所以：
/// - 删除不是物理删除——被删的词之后再次被输入并选中会"复活"；
/// - 频率调不低（要调低得先删再写）；
/// - 之前被删过的词再写一遍，频率恢复为正（= 复活）。
/// 返回导入条数（正常应当为 1）。
///
/// **调用方必须保证此刻用户词典处于关闭状态**（`user_dict_manager.h` 的 CAVEAT）：
/// 在本进程里即先销毁 rime 会话（`RimeEngine::with_user_dict_closed`）。
pub(crate) fn import_entry(
    dict: &str,
    word: &str,
    code: &str,
    commits: i32,
) -> Result<i32, String> {
    let (word, code) = validate_entry(word, code, commits)?;
    let label = file_label(dict);
    let path = std::env::temp_dir().join(format!(
        "xime_dict_entry_{label}_{}_{}.txt",
        std::process::id(),
        ENTRY_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(e) = std::fs::write(&path, entry_line(&word, &code, commits).as_bytes()) {
        return Err(format!("写临时文件失败：{e}"));
    }
    let imported = librime::import_user_dict(dict, &path.to_string_lossy());
    // 临时文件即用即删（失败也不影响结果）。
    let _ = std::fs::remove_file(&path);
    let imported = imported.map_err(|e| format!("导入用户词典失败：{e:?}"))?;
    if imported <= 0 {
        return Err("导入用户词典失败（librime 写入 0 条，词库可能正被占用）".to_string());
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实导出文件的样子（取自本机 rime_ice 的导出结果）。
    const SAMPLE: &str = "# Rime user dictionary export\n\
                          #@/db_name\trime_ice\n\
                          #@/db_type\tuserdb\n\
                          #@/rime_version\t1.16.1\n\
                          #@/tick\t4\n\
                          #@/user_id\t3d4f1bf2-d753-481f-8dea-867f441a39b9\n\
                          的\tde\t1\n\
                          购买\tgou mai\t1\n\
                          请\tqing\t1\n\
                          威望\twei wang\t1\n";

    #[test]
    fn parses_export_skipping_header_lines() {
        let entries = parse_user_dict_text(SAMPLE);
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries[0],
            DictEntry {
                word: "的".to_string(),
                code: "de".to_string(),
                commits: 1,
            }
        );
        assert_eq!(entries[3].word, "威望");
        assert_eq!(entries[3].code, "wei wang");
    }

    #[test]
    fn missing_columns_degrade_gracefully() {
        let entries = parse_user_dict_text("词\t\n独词\n\n\t\t\n");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].code, "");
        assert_eq!(entries[0].commits, 1);
        assert_eq!(entries[1].word, "独词");
        assert_eq!(entries[1].commits, 1);
    }

    #[test]
    fn deleted_entries_are_not_shown() {
        // tombstone：频率 -1，librime 不会再把它导出来，这里同样不展示。
        let entries = parse_user_dict_text("删过的\tde\t-1\n留下的\tde\t2\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].word, "留下的");
        assert_eq!(entries[0].commits, 2);
    }

    #[test]
    fn filters_by_word_or_code_case_insensitively() {
        let entries = parse_user_dict_text(SAMPLE);
        let by_word = select_entries(&entries, "购买");
        assert_eq!(by_word.matched, 1);
        assert_eq!(by_word.entries.len(), 1);
        assert_eq!(by_word.entries[0].code, "gou mai");

        let by_code = select_entries(&entries, "GOU");
        assert_eq!(by_code.matched, 1);
        assert_eq!(by_code.entries[0].word, "购买");

        let none = select_entries(&entries, "不存在的词");
        assert_eq!(none.matched, 0);
        assert!(none.entries.is_empty());

        let trimmed = select_entries(&entries, "  de  ");
        assert_eq!(trimmed.matched, 1);

        let all = select_entries(&entries, "");
        assert_eq!(all.matched, 4);
        assert_eq!(all.entries.len(), 4);
    }

    #[test]
    fn caps_returned_entries_but_reports_full_hit_count() {
        let mut text = String::from("# Rime user dictionary export\n");
        for i in 0..(MAX_DICT_ENTRIES + 40) {
            text.push_str(&format!("词{i}\tcode{i}\t1\n"));
        }
        let entries = parse_user_dict_text(&text);
        assert_eq!(entries.len(), MAX_DICT_ENTRIES + 40);

        let selected = select_entries(&entries, "");
        assert_eq!(selected.entries.len(), MAX_DICT_ENTRIES);
        assert_eq!(selected.matched, MAX_DICT_ENTRIES + 40, "命中总数不受截断影响");

        // 有关键词时同样：回传被截断，但命中总数如实报。
        let matched = select_entries(&entries, "词");
        assert_eq!(matched.entries.len(), MAX_DICT_ENTRIES);
        assert_eq!(matched.matched, MAX_DICT_ENTRIES + 40);
    }

    #[test]
    fn file_label_strips_path_characters() {
        assert_eq!(file_label("wubi86"), "wubi86");
        assert_eq!(file_label("../rime_ice"), "___rime_ice");
        assert_eq!(file_label("a/b\\c:d"), "a_b_c_d");
    }

    #[test]
    fn entry_validation_trims_and_rejects_bad_input() {
        assert_eq!(
            validate_entry("  词 ", " code ", 1),
            Ok(("词".to_string(), "code".to_string()))
        );
        // 删除标记（负频率）是合法输入。
        assert!(validate_entry("词", "code", -1).is_ok());

        assert!(validate_entry("", "code", 1).is_err(), "空词");
        assert!(validate_entry("词", "", 1).is_err(), "空编码");
        assert!(validate_entry("词\t带制表", "code", 1).is_err(), "词里的制表符");
        assert!(validate_entry("词", "co\nde", 1).is_err(), "编码里的换行");
        assert!(validate_entry("词", "code", 0).is_err(), "频率 0 两头不是");
        // 首尾空白被修剪，所以只有"内部"的制表符/换行才要拒。
        assert!(validate_entry("\t词\n", "code", 1).is_ok(), "首尾空白修掉即可");
    }

    #[test]
    fn entry_line_matches_android_code_table_text() {
        assert_eq!(entry_line("你好", "nh", 1), "你好\tnh\t1\n");
        assert_eq!(entry_line("删", "shan", -1), "删\tshan\t-1\n");
    }
}