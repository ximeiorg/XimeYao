//! 快捷短语（`custom_phrase.txt`）的读取与整表保存（设置页「词典 → 快捷短语」）。
//!
//! 数据形态（对齐安卓版 `PersonalDictManager`，文本格式逐字节一致）：
//! - 文件 `<rime_dir>/<表名>.txt`，表名来自方案配置里 `custom_phrase.user_dict`
//!   （默认 `custom_phrase`），由 `xime_config::custom_phrase_dict_name` 统一解析；
//! - 头部 5 行是 rime 文本码表的标准头（`#@/db_name`、`#@/db_type tabledb`）；
//! - 每行 `词⇥编码[⇥权重]`，权重列可省略；整个文件在保存时**整表重写**；
//! - 方案侧要有一个 `table_translator@custom_phrase` 翻译器才会生效：往
//!   `<schema_id>.custom.yaml` 的 `patch:` 里追加 `engine/translators/+` 一项 +
//!   `custom_phrase:` 配置块（与安卓注入的文本完全一致，librime 的 config 编译器
//!   支持 `/+` 列表追加语法）。**仅当至少有一条短语时才注入**（空表会让 rime
//!   为空表建翻译器而报错）；清空所有短语时也**不**摘除注入——和安卓一致，
//!   摘除需要删 custom.yaml 的 key，rime 的 levers 接口没有这个操作。
//!
//! 保存只写文件与 patch，**不做部署**：patch 首次注入后要重新部署才生效，
//! 由设置页提示用户（或用户点「重新部署」）。

use std::path::{Path, PathBuf};

use winxime_ipc::CustomPhraseEntry;

/// rime 文本码表的标准头（`db_name` 固定写 `custom_phrase`，与安卓版一致；
/// rime 按文件里的 `#@/db_name` 认表，不要求等于文件名）。
const STABLEDB_HEADER: &str = "# Rime table\n\
                               # coding: utf-8\n\
                               #@/db_name\tcustom_phrase\n\
                               #@/db_type\ttabledb\n\
                               #\n";

/// 注入方案 custom.yaml 的翻译器配置块（与安卓版 `applyCustomPhraseTranslator`
/// 写入的文本逐行一致；`engine/translators/+` 是 librime 的列表追加补丁语法）。
const TRANSLATOR_PATCH: &str = concat!(
    "  \"engine/translators/+\":\n",
    "    - table_translator@custom_phrase\n",
    "  \"custom_phrase\":\n",
    "    dictionary: \"\"\n",
    "    user_dict: {dict_name}\n",
    "    db_class: stabledb\n",
    "    enable_completion: false\n",
    "    enable_sentence: false\n",
    "    initial_quality: 99"
);

/// 幂等标记：custom.yaml 里已含该子串就不再注入。
const PATCH_MARKER: &str = "table_translator@custom_phrase";

/// 一次读取的结果。
pub(crate) struct PhraseRead {
    /// 解析出的短语表名（`custom_phrase.user_dict`，通常就是 `custom_phrase`）。
    pub dict_name: String,
    /// 短语表文件名（`<dict_name>.txt`）。
    pub file_name: String,
    /// 短语表文件是否已存在。
    pub file_exists: bool,
    /// 方案 custom.yaml 里是否已注入翻译器。
    pub patch_applied: bool,
    /// 短语列表（文件不存在或还没有条目时为空）。
    pub entries: Vec<CustomPhraseEntry>,
}

/// 一次整表保存的结果。
pub(crate) struct PhraseSaveOutcome {
    pub dict_name: String,
    pub file_name: String,
    pub file_exists: bool,
    pub patch_applied: bool,
    /// 本次保存是否**新**注入了翻译器（是 → 提示需要重新部署）。
    pub patch_added: bool,
    /// 保存后的整表。
    pub entries: Vec<CustomPhraseEntry>,
}

fn phrase_file(rime_dir: &Path, dict_name: &str) -> PathBuf {
    rime_dir.join(format!("{dict_name}.txt"))
}

fn custom_yaml(rime_dir: &Path, schema_id: &str) -> PathBuf {
    rime_dir.join(format!("{schema_id}.custom.yaml"))
}

/// 解析短语表文本（每行 `词⇥编码[⇥权重]`；`#` 头部、空行跳过）。
///
/// 纯函数，便于用字符串样例做单测；分隔符只认制表符（写出去的也是制表符）。
pub(crate) fn parse_phrase_text(text: &str) -> Vec<CustomPhraseEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let Some(word) = fields.next() else { continue };
        let Some(code) = fields.next() else { continue };
        let weight = fields.next().and_then(|w| w.trim().parse::<i32>().ok());
        if word.trim().is_empty() || code.trim().is_empty() {
            continue;
        }
        out.push(CustomPhraseEntry {
            word: word.trim().to_string(),
            code: code.trim().to_string(),
            weight,
        });
    }
    out
}

/// 把整表写成 rime 文本码表（标准头 + 每行 `词⇥编码[⇥权重]`，文件以换行结尾）。
///
/// 纯函数，便于单测钉住字节格式（与安卓版 `buildStableDbText` 一致）。
pub(crate) fn build_phrase_text(entries: &[CustomPhraseEntry]) -> String {
    let mut text = String::from(STABLEDB_HEADER);
    for entry in entries {
        text.push_str(&entry.word);
        text.push('\t');
        text.push_str(&entry.code);
        if let Some(weight) = entry.weight {
            text.push('\t');
            text.push_str(&weight.to_string());
        }
        text.push('\n');
    }
    text
}

/// 读取某方案的快捷短语表。
pub(crate) fn read_custom_phrases(
    rime_dir: &Path,
    schema_id: &str,
) -> Result<PhraseRead, String> {
    let dict_name = xime_config::schema_manifest::custom_phrase_dict_name(rime_dir, schema_id);
    let file_name = format!("{dict_name}.txt");
    let file = phrase_file(rime_dir, &dict_name);
    let (file_exists, entries) = match std::fs::read_to_string(&file) {
        Ok(text) => (true, parse_phrase_text(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (false, Vec::new()),
        Err(e) => return Err(format!("读取 {file_name} 失败：{e}")),
    };
    let patch_applied = patch_is_applied(rime_dir, schema_id);
    Ok(PhraseRead {
        dict_name,
        file_name,
        file_exists,
        patch_applied,
        entries,
    })
}

/// custom.yaml 里是否已有翻译器注入（幂等标记）。
fn patch_is_applied(rime_dir: &Path, schema_id: &str) -> bool {
    match std::fs::read_to_string(custom_yaml(rime_dir, schema_id)) {
        Ok(text) => text.contains(PATCH_MARKER),
        Err(_) => false,
    }
}

/// 校验整表输入（逐条修剪并拒绝坏行），返回修剪后的整表。
///
/// 规则与用户词条一致：词/编码必填、不含制表符换行；权重为正整数（省略 = None）。
pub(crate) fn validate_phrases(
    entries: &[CustomPhraseEntry],
) -> Result<Vec<CustomPhraseEntry>, String> {
    let mut cleaned = Vec::with_capacity(entries.len());
    for entry in entries {
        let word = entry.word.trim();
        let code = entry.code.trim();
        if word.is_empty() || code.is_empty() {
            return Err("词和编码都要填".to_string());
        }
        for (label, value) in [("词", word), ("编码", code)] {
            if value.contains('\t') || value.contains('\n') || value.contains('\r') {
                return Err(format!("{label}不能含制表符或换行"));
            }
        }
        if let Some(weight) = entry.weight {
            if weight <= 0 {
                return Err("权重要是正整数（留空即默认）".to_string());
            }
        }
        cleaned.push(CustomPhraseEntry {
            word: word.to_string(),
            code: code.to_string(),
            weight: entry.weight,
        });
    }
    Ok(cleaned)
}

/// 整表保存某方案的快捷短语：写文件 + 视需要注入方案 patch。
///
/// **不做部署**——`patch_added` 为 true 时调用方要提示用户重新部署。
pub(crate) fn save_custom_phrases(
    rime_dir: &Path,
    schema_id: &str,
    entries: &[CustomPhraseEntry],
) -> Result<PhraseSaveOutcome, String> {
    let entries = validate_phrases(entries)?;
    let dict_name = xime_config::schema_manifest::custom_phrase_dict_name(rime_dir, schema_id);
    let file_name = format!("{dict_name}.txt");

    // 1) 写短语表（覆盖式整表重写）。
    let file = phrase_file(rime_dir, &dict_name);
    std::fs::write(&file, build_phrase_text(&entries).as_bytes())
        .map_err(|e| format!("写入 {file_name} 失败：{e}"))?;

    // 2) 注入翻译器：仅当至少有一条短语（空表会让 rime 为空表建翻译器而报错）。
    //    已注入过就不再动（幂等，与安卓版一致）。
    let patch_added = !entries.is_empty()
        && !patch_is_applied(rime_dir, schema_id)
        && apply_translator_patch(rime_dir, schema_id, &dict_name)?;
    let patch_applied = patch_is_applied(rime_dir, schema_id);

    Ok(PhraseSaveOutcome {
        dict_name,
        file_name,
        file_exists: true,
        patch_applied,
        patch_added,
        entries,
    })
}

/// 往方案的 custom.yaml 注入 `table_translator@custom_phrase` 翻译器（文本级合并）。
///
/// - 已有 `patch:` 块 → 插到 `patch:` 行的下一行（其余内容原样保留）；
/// - 没有 → 先去掉结尾的 `...` 文档结束标记，再追加 `patch:\n<块>`；
/// - 文件不存在 → 新建一个只含 patch 块的文件。
///
/// 与安卓版 `insertUnderPatch` 的文本手术一致：不解析 YAML、不重排用户已有的
/// 配置。幂等性由调用方（`patch_is_applied`）保证。
fn apply_translator_patch(rime_dir: &Path, schema_id: &str, dict_name: &str) -> Result<bool, String> {
    let yaml_path = custom_yaml(rime_dir, schema_id);
    let existing = std::fs::read_to_string(&yaml_path).unwrap_or_default();
    let block = TRANSLATOR_PATCH.replace("{dict_name}", dict_name);

    let patched = if let Some(at) = existing
        .lines()
        .position(|line| line.trim_start().starts_with("patch:"))
    {
        // 已有 patch: 块 → 插到它后面（块内部自带换行，join 时原样保留）。
        let mut lines: Vec<&str> = existing.lines().collect();
        lines.insert(at + 1, block.as_str());
        let mut text = lines.join("\n");
        text.push('\n');
        text
    } else {
        // 没有 patch: 块：剥掉结尾的 ... 标记再追加（rime 的 custom.yaml 惯例是
        // 文档结束标记之后才不会被当作配置体）。
        let mut cleaned = existing
            .trim_end_matches(['\n', '\r', ' '])
            .trim_end_matches("...")
            .trim_end()
            .to_string();
        if !cleaned.is_empty() {
            cleaned.push_str("\n\n");
        }
        cleaned.push_str("patch:\n");
        cleaned.push_str(&block);
        cleaned.push('\n');
        cleaned
    };

    std::fs::write(&yaml_path, patched.as_bytes())
        .map_err(|e| format!("写入 {}.custom.yaml 失败：{e}", schema_id))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(word: &str, code: &str, weight: Option<i32>) -> CustomPhraseEntry {
        CustomPhraseEntry {
            word: word.to_string(),
            code: code.to_string(),
            weight,
        }
    }

    #[test]
    fn parses_header_and_entries() {
        let text = "# Rime table\n\
                    # coding: utf-8\n\
                    #@/db_name\tcustom_phrase\n\
                    #@/db_type\ttabledb\n\
                    #\n\
                    你好\tnh\n\
                    早上好\tzsh\t3\n\
                    \n\
                    半角行 no-tab\n";
        let entries = parse_phrase_text(text);
        assert_eq!(
            entries,
            vec![
                entry("你好", "nh", None),
                entry("早上好", "zsh", Some(3)),
            ]
        );
    }

    #[test]
    fn roundtrips_build_and_parse() {
        let entries = vec![
            entry("你好", "nh", None),
            entry("早上好", "zsh", Some(3)),
            entry("邮箱", "yx", None),
        ];
        let text = build_phrase_text(&entries);
        assert!(text.starts_with("# Rime table\n"));
        assert!(text.contains("#@/db_type\ttabledb\n"));
        assert!(text.contains("你好\tnh\n"));
        assert!(text.contains("早上好\tzsh\t3\n"));
        assert!(text.ends_with('\n'), "文件以换行结尾（安卓版同样）");
        assert_eq!(parse_phrase_text(&text), entries, "写出去再读回来不变");
    }

    #[test]
    fn validation_rejects_bad_rows() {
        assert!(validate_phrases(&[entry("你好", "nh", None)]).is_ok());
        assert!(validate_phrases(&[entry("你好", "nh", None), entry("  ", "x", None)]).is_err());
        assert!(validate_phrases(&[entry("你\t好", "nh", None)]).is_err());
        assert!(validate_phrases(&[entry("你好", "n\nh", None)]).is_err());
        assert!(validate_phrases(&[entry("你好", "nh", Some(0))]).is_err(), "权重 0");
        assert!(validate_phrases(&[entry("你好", "nh", Some(-2))]).is_err(), "负权重");
        // 修剪：首尾空白去掉。
        let cleaned = validate_phrases(&[entry("  你好 ", " nh ", Some(3))]);
        assert_eq!(
            cleaned,
            Ok(vec![entry("你好", "nh", Some(3))]),
            "{cleaned:?}"
        );
    }

    #[test]
    fn translator_patch_block_shape() {
        let block = TRANSLATOR_PATCH.replace("{dict_name}", "custom_phrase");
        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(lines[0], "  \"engine/translators/+\":");
        assert_eq!(lines[1], "    - table_translator@custom_phrase");
        assert_eq!(lines[2], "  \"custom_phrase\":");
        assert!(block.contains("user_dict: custom_phrase\n"));
        assert!(block.contains("db_class: stabledb"));
        assert!(block.contains("initial_quality: 99"));
        assert!(block.contains("enable_completion: false"));
    }

    #[test]
    fn patch_idempotent_marker_is_unique_enough() {
        // 幂等标记必须出现在注入块里。
        assert!(TRANSLATOR_PATCH.contains(PATCH_MARKER));
    }

    #[test]
    fn empty_entries_write_header_only_file() {
        let text = build_phrase_text(&[]);
        assert_eq!(text, STABLEDB_HEADER);
    }
}
