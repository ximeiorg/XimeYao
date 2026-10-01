//! 方案词表（只读）读取 —— 设置页「词典 → 方案词表」。
//!
//! 与「用户词典」（`crate::user_dict`，可编辑、有频率）不同，方案词表就是随方案
//! 分发的码表文件，属于方案本身：这里**只读**，不提供编辑/删除/导入功能。
//!
//! 命名与位置：码表都是 rime 用户目录下的 `<name>.dict.yaml`，主表由方案的
//! `<schema_id>.schema.yaml` 里 `translator.dictionary`（文本里第一个 `dictionary:`
//! 键）指定。真正的"一本方案词表"往往是**一组文件**：主码表 + 它 `import_tables:`
//! 递归引入的子码表 + 方案 `translator.packs` 声明的附加码表；因此下面按 BFS
//! 顺序把这些文件拼成一份词条集合再过滤。
//!
//! 解析规则与 Android 版（Xime `SchemaDictManager`）保持一致：码表文本分元信息段
//! 与正文段（以单独一行 `...` 分隔），只有正文段是词条（`词  码  [权重]`），
//! 权重列没有"频率"语义，回传时 `commits` 恒为 0。所有解析都是纯函数，便于无
//! 文件系统依赖地做单元测试；只有 `read_schema_dict` 碰磁盘。
//!
//! 进程内单条缓存：搜索框每敲一个字都会调用本模块，主码表动辄 1~8MB、几十万条，
//! 反复读盘+解析不可接受。所以缓存一份"文件签名（名字 + 长度 + mtime）+ 解析结果"，
//! 签名不变就直接复用，不碰磁盘；签名对不上（或缓存为空/被毒化）就老老实实重读。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use winxime_ipc::DictEntry;

/// 一次方案词表读取的结果。
pub(crate) struct SchemaDictRead {
    /// 方案主码表名（`<schema_id>.schema.yaml` 里 `dictionary:` 的值；解析不到时退化为 schema_id）。
    pub dict_name: String,
    /// 实际读入的码表名（主表在前，import_tables / translator.packs 按读入顺序）。
    pub tables: Vec<String>,
    /// 读入的词条总数（不受关键词过滤与条数上限影响）。
    pub total: i32,
    /// 关键词命中数（未受条数上限影响）。
    pub matched: i32,
    /// 过滤后回传的词条，最多 `winxime_ipc::MAX_DICT_ENTRIES` 条；方案词表没有频率概念，`commits` 恒为 0。
    pub entries: Vec<DictEntry>,
    /// 方案里声明了但文件不存在的码表名（供 UI 提示，不算错误）。
    pub missing: Vec<String>,
}

/// 码表文本的解析结果。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CodeTable {
    /// 正文段（单独一行 `...` 之后）的词条：`(词, 码)`，第三列权重忽略。
    pub entries: Vec<(String, String)>,
    /// 元信息段声明的子码表名（`import_tables`，保持声明顺序并去重）。
    pub import_tables: Vec<String>,
}

/// 解析码表文本（纯函数）。
///
/// 元信息段（`...` 之前）只取 `import_tables`，其余全部忽略；正文段跳过空行与
/// `#` 注释行，按空白串切列，至少要两列（词 / 码）才算一条词条。
pub(crate) fn parse_code_table_text(text: &str) -> CodeTable {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut in_data = false;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if !in_data {
            if line.trim() == "..." {
                in_data = true;
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // 等价于 Android 版「先按 \t 切、再按两空格切、再按单空格切并丢掉空串」。
        let mut columns = trimmed
            .split(|c: char| c == ' ' || c == '\t')
            .filter(|column| !column.is_empty());
        let Some(word) = columns.next() else { continue };
        let Some(code) = columns.next() else { continue };
        entries.push((word.to_string(), code.to_string()));
    }
    CodeTable {
        entries,
        // 元信息段只关心 import_tables（只扫到第一个 `...`，代价可忽略）。
        import_tables: parse_import_tables(text),
    }
}

/// 解析 `import_tables` 声明（纯函数）：只扫描第一个单独一行 `...` 之前的部分。
pub(crate) fn parse_import_tables(text: &str) -> Vec<String> {
    import_tables_from_lines(text.lines().take_while(|line| line.trim() != "..."))
}

/// `import_tables` 行解析：`import_tables: [a, b]` 行内形式与 `- name` 块状形式。
fn import_tables_from_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // 是否已读到 `import_tables:` 且其值在后续行（块状列表）。
    let mut in_block = false;
    for line in lines {
        let trimmed = line.trim();
        if in_block {
            // 块状列表项：`- name`；遇到第一个不是列表项的行就结束声明。
            let Some(item) = trimmed.strip_prefix('-') else {
                break;
            };
            push_unique(&mut out, strip_quotes(item));
            continue;
        }
        let Some(rest) = key_value(trimmed, "import_tables:") else {
            continue;
        };
        if let (Some(open), Some(close)) = (rest.find('['), rest.rfind(']')) {
            if close > open {
                for item in rest[open + 1..close].split(',') {
                    push_unique(&mut out, strip_quotes(item));
                }
            }
            // 行内形式已经写完，后面不会再跟块状项。
            continue;
        }
        in_block = true;
    }
    out
}

/// 解析方案 `translator.packs`（纯函数）：YAML 列表里的标量名，顺序保留。
///
/// 用 `serde_yaml::Value` 导航而不是正则，`translator`/`packs` 缺失、类型不对或
/// 整份 YAML 解析失败都退化为空列表（不报错）。
pub(crate) fn parse_packs(schema_text: &str) -> Vec<String> {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(schema_text) else {
        return Vec::new();
    };
    value
        .get("translator")
        .and_then(|translator| translator.get("packs"))
        .and_then(|packs| packs.as_sequence())
        .map(|list| list.iter().filter_map(yaml_scalar).collect())
        .unwrap_or_default()
}

/// 解析方案主码表名（纯函数）：文本里**第一个** `dictionary:` 键的值。
///
/// 值允许单/双引号包裹，字符集限定 `[A-Za-z0-9_-]`（码表名只有这些字符，行尾的
/// `# 注释` 与 `enable_xxx:` 之类同后缀字段因此都不会误伤）。解析不到就用 `schema_id`。
pub(crate) fn resolve_dict_name(schema_text: &str, schema_id: &str) -> String {
    for line in schema_text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(rest) = key_value(trimmed, "dictionary:") else {
            continue;
        };
        let name = dict_name_charset(strip_quotes(rest));
        if !name.is_empty() {
            return name;
        }
    }
    schema_id.to_string()
}

/// 在（已 trim 的）一行里找独立的 `key`，返回其后的原始文本。
fn key_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut from = 0usize;
    while let Some(offset) = line[from..].find(key) {
        let start = from + offset;
        let prev_is_identifier = line[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !prev_is_identifier {
            return Some(&line[start + key.len()..]);
        }
        from = start + key.len();
    }
    None
}

/// 剥掉成对的单/双引号（并 trim）。
fn strip_quotes(text: &str) -> &str {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return text[1..text.len() - 1].trim();
        }
    }
    text
}

/// 截取码表名字符集 `[A-Za-z0-9_-]` 的前缀（用于挡掉行尾注释与多余空白）。
fn dict_name_charset(text: &str) -> String {
    text.trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

/// YAML 标量转字符串（数字/布尔也接受，与 `schema_switches` 的取法一致）。
fn yaml_scalar(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(text) => Some(text.clone()),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        serde_yaml::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// 保序去重地追加一个码表名（空名忽略）。
fn push_unique(out: &mut Vec<String>, name: &str) {
    let name = name.trim();
    if name.is_empty() || out.iter().any(|existing| existing == name) {
        return;
    }
    out.push(name.to_string());
}

/// 文件签名：`None` 表示文件不存在，`Some((长度, mtime 纳秒))` 表示存在。
type FileStamp = Option<(u64, i64)>;

/// BFS 遍历的结果（缓存需要文件签名，因此一并返回访问过的所有名字）。
struct Traversal {
    /// 成功读入的码表名（访问顺序）。
    tables: Vec<String>,
    /// 声明了但文件不存在（或读不出内容）的码表名（访问顺序）。
    missing: Vec<String>,
    /// 拼接后的词条（访问顺序，未过滤未截断）。
    entries: Vec<(String, String)>,
}

/// 按 BFS 顺序读入主码表 + 其 `import_tables`（递归）+ `packs` 的词条。
///
/// 用 `read` 闭包取文件名对应的码表文本（`None` = 文件不存在），这样遍历逻辑
/// 可以脱离文件系统测试；`visited` 集合同时负责去重与**环保护**（A 引 B、B 引 A
/// 时会终止）。`FnMut` 是为了让真实调用方在闭包里顺手记录文件签名。
fn traverse<F>(dict_name: &str, packs: &[String], mut read: F) -> Traversal
where
    F: FnMut(&str) -> Option<String>,
{
    let mut queue: Vec<String> = Vec::new();
    push_unique(&mut queue, dict_name);
    for pack in packs {
        push_unique(&mut queue, pack);
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut tables: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut entries: Vec<(String, String)> = Vec::new();

    let mut index = 0usize;
    while index < queue.len() {
        let name = queue[index].clone();
        index += 1;
        if !visited.insert(name.clone()) {
            continue;
        }
        match read(&name) {
            Some(text) => {
                let table = parse_code_table_text(&text);
                entries.extend(table.entries);
                tables.push(name);
                for import in table.import_tables {
                    if !visited.contains(&import) {
                        push_unique(&mut queue, &import);
                    }
                }
            }
            None => missing.push(name),
        }
    }

    Traversal {
        tables,
        missing,
        entries,
    }
}

/// 词条行 → `DictEntry`（方案词表没有频率概念，`commits` 恒为 0）。
fn entries_to_dict_entries(rows: &[(String, String)]) -> Vec<DictEntry> {
    rows.iter()
        .map(|(word, code)| DictEntry {
            word: word.clone(),
            code: code.clone(),
            commits: 0,
        })
        .collect()
}

/// 过滤 + 截断（关键词语义与用户词典浏览完全一致，复用 `select_entries`）。
fn finish_read(
    dict_name: &str,
    tables: &[String],
    missing: &[String],
    total: i32,
    entries: &[DictEntry],
    query: &str,
) -> SchemaDictRead {
    let selection = crate::user_dict::select_entries(entries, query);
    SchemaDictRead {
        dict_name: dict_name.to_string(),
        tables: tables.to_vec(),
        total,
        matched: selection.matched as i32,
        entries: selection.entries,
        missing: missing.to_vec(),
    }
}

/// 取文件签名（读不到元信息当作"不存在"，mtime 取不到记 0）。
fn stamp_of(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_nanos() as i64)
        .unwrap_or(0);
    Some((meta.len(), mtime))
}

/// 进程内单条缓存：一次已解析的方案词表 + 读过的文件签名。
struct SchemaDictCache {
    /// 主码表名（签名的一部分：换方案直接失效）。
    dict_name: String,
    /// 方案 packs（签名的一部分）。
    packs: Vec<String>,
    /// 读入的码表名（含顺序，回传给 UI）。
    tables: Vec<String>,
    /// 缺失的码表名。
    missing: Vec<String>,
    /// 词条总数。
    total: i32,
    /// 词条（`commits` 恒为 0，未过滤）。
    entries: Vec<DictEntry>,
    /// `tables ∪ missing` 里每个名字的签名：全部一致才算命中。
    stamps: Vec<(String, FileStamp)>,
}

impl SchemaDictCache {
    /// 签名是否仍然成立（逐个 stat；缺失的名字也必须仍然缺失）。
    ///
    /// 缓存里的 `missing` 也要一起签名：某个此前缺失的码表文件后来出现了，
    /// 签名就从 `None` 变成 `Some(...)`，缓存自然失效。
    fn matches(&self, rime_dir: &Path) -> bool {
        self.stamps.iter().all(|(name, stamp)| {
            stamp_of(&rime_dir.join(format!("{name}.dict.yaml"))) == *stamp
        })
    }
}

/// 方案词表缓存（同一时刻只保留"最近读的那一本"）。
static CACHE: Mutex<Option<SchemaDictCache>> = Mutex::new(None);

/// 读取某方案的词表词条（主码表 + import_tables 递归 + translator.packs），按关键词过滤并截断。
///
/// 只读、不修改任何文件；主码表文件缺失/读不出才是 `Err`（`import_tables`/`packs`
/// 里缺的文件只记进 `missing`，由 UI 提示）。锁被毒化、缓存签名对不上等情况都退化
/// 为一次完整重读，不会变成错误。
pub(crate) fn read_schema_dict(
    rime_dir: &Path,
    schema_id: &str,
    query: &str,
) -> Result<SchemaDictRead, String> {
    let schema_id = schema_id.trim();
    if schema_id.is_empty() {
        return Err("方案 id 为空，无法读取方案词表".to_string());
    }

    // 方案 yaml 很小（KB 级），每次调用都重新读：主码表名与 packs 都来自它。
    // 读不到不算错误——主码表名退化为 schema_id，packs 为空。
    let schema_text = std::fs::read_to_string(rime_dir.join(format!("{schema_id}.schema.yaml")))
        .unwrap_or_default();
    let dict_name = resolve_dict_name(&schema_text, schema_id);
    let packs = parse_packs(&schema_text);

    // 先查缓存：签名一致就完全不碰磁盘上的大码表（搜索框每敲一个字都会走到这里）。
    {
        let cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cache.as_ref() {
            if cached.dict_name == dict_name && cached.packs == packs && cached.matches(rime_dir) {
                return Ok(finish_read(
                    &cached.dict_name,
                    &cached.tables,
                    &cached.missing,
                    cached.total,
                    &cached.entries,
                    query,
                ));
            }
        }
    }

    // 冷读：顺手按访问顺序记录每个文件的签名（含缺失的，用 None 表示）。
    let mut stamps: Vec<(String, FileStamp)> = Vec::new();
    let traversal = traverse(&dict_name, &packs, |name| {
        let path = rime_dir.join(format!("{name}.dict.yaml"));
        let stamp = stamp_of(&path);
        let text = std::fs::read_to_string(&path).ok();
        if text.is_none() && stamp.is_some() {
            tracing::warn!("方案码表无法作为 UTF-8 文本读取：{}", path.display());
        }
        stamps.push((name.to_string(), stamp));
        text
    });

    if traversal.missing.iter().any(|name| name == &dict_name) {
        return Err(format!(
            "方案「{schema_id}」的主码表 {dict_name}.dict.yaml 不存在或无法读取"
        ));
    }

    let entries = entries_to_dict_entries(&traversal.entries);
    let total = entries.len() as i32;
    let read = finish_read(
        &dict_name,
        &traversal.tables,
        &traversal.missing,
        total,
        &entries,
        query,
    );

    *CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SchemaDictCache {
        dict_name,
        packs,
        tables: traversal.tables,
        missing: traversal.missing,
        total,
        entries,
        stamps,
    });

    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use winxime_ipc::MAX_DICT_ENTRIES;

    /// 真实码表的样子（元信息段 + `...` + 正文段），本机 wubi86.dict.yaml 的缩影。
    const SAMPLE: &str = "---\n\
                          name: wubi86\n\
                          version: \"4.3\"\n\
                          sort: by_weight\n\
                          import_tables:\n\
                          \x20 - wubi86_extra\n\
                          encoder:\n\
                          \x20 rules:\n\
                          \x20   - length_equal: 2\n\
                          ...\n\
                          工\ta\n\
                          式\taa\n";

    /// 用内存文件表跑一遍遍历。
    fn traverse_files(
        dict_name: &str,
        packs: &[String],
        files: &HashMap<&str, &str>,
    ) -> Traversal {
        traverse(dict_name, packs, |name| {
            files.get(name).map(|text| (*text).to_string())
        })
    }

    #[test]
    fn metadata_section_is_ignored_and_data_section_parsed() {
        let table = parse_code_table_text(SAMPLE);
        assert_eq!(
            table.entries,
            vec![
                ("工".to_string(), "a".to_string()),
                ("式".to_string(), "aa".to_string()),
            ]
        );
        assert_eq!(table.import_tables, vec!["wubi86_extra".to_string()]);
        // 元信息段里的 `sort: by_weight` 之类不会被当成词条。
        assert!(!table.entries.iter().any(|(word, _)| word == "name"));
    }

    #[test]
    fn all_separator_styles_parse_the_same() {
        let tab = parse_code_table_text("...\n词\ta\n");
        let two_spaces = parse_code_table_text("...\n词  a\n");
        let one_space = parse_code_table_text("...\n词 a\n");
        let expected = vec![("词".to_string(), "a".to_string())];
        assert_eq!(tab.entries, expected);
        assert_eq!(two_spaces.entries, expected);
        assert_eq!(one_space.entries, expected);
        // 第三列（权重）忽略。
        assert_eq!(parse_code_table_text("...\n词\ta\t123\n").entries, expected);
    }

    #[test]
    fn blank_comment_and_single_column_lines_are_skipped() {
        let text = "...\n\
                    \n\
                    # 注释行\n\
                    \x20\x20# 缩进的注释行\n\
                    只有一列\n\
                    ## 分组名\n\
                    词\ta\n\
                    \u{3000}\n";
        assert_eq!(
            parse_code_table_text(text).entries,
            vec![("词".to_string(), "a".to_string())]
        );
    }

    #[test]
    fn import_tables_block_and_inline_forms() {
        let block = parse_import_tables(
            "---\nname: x\nimport_tables:\n  - a\n  - 'b'\n  - \"c\"\n  - a\nencoder:\n  rules: []\n...\n字\tzi\n",
        );
        assert_eq!(block, vec!["a", "b", "c"]);

        let inline = parse_import_tables("import_tables: [a, \"b\", 'c', a]\n...\n");
        assert_eq!(inline, vec!["a", "b", "c"]);

        assert!(parse_import_tables("import_tables: []\n...\n").is_empty());
        // 正文段里的同名行不算声明（只扫 `...` 之前）。
        assert!(parse_import_tables("...\nimport_tables: [z]\n").is_empty());
        // 没有 `...` 时整份文本都算元信息段。
        assert_eq!(parse_import_tables("import_tables: [a]\n"), vec!["a"]);
    }

    #[test]
    fn resolve_dict_name_variants() {
        assert_eq!(
            resolve_dict_name("translator:\n  dictionary: wubi86\n", "fallback"),
            "wubi86"
        );
        assert_eq!(
            resolve_dict_name("translator:\n  dictionary: \"wubi86\"\n", "fallback"),
            "wubi86"
        );
        assert_eq!(
            resolve_dict_name("translator:\n  dictionary:  'wubi86'\n", "fallback"),
            "wubi86"
        );
        // 键出现在其它键之后照样能取到；行尾注释被字符集截断。
        assert_eq!(
            resolve_dict_name(
                "schema:\n  schema_id: wubi86_trad\nname: n\ntranslator:\n  dictionary: wubi86  # 翻译器调用的字典\n  enable_user_dict: false\n",
                "fallback"
            ),
            "wubi86"
        );
        // 取第一个 `dictionary:`（注释行不算，后面的 reverse_lookup 不算）。
        assert_eq!(
            resolve_dict_name(
                "# dictionary: nope\ndictionary: first\nreverse_lookup:\n  dictionary: second\n",
                "fallback"
            ),
            "first"
        );
        // 没有该键 / 空文本 → 退化为 schema_id。
        assert_eq!(
            resolve_dict_name("schema:\n  schema_id: x\n", "my_schema"),
            "my_schema"
        );
        assert_eq!(resolve_dict_name("", "my_schema"), "my_schema");
    }

    #[test]
    fn parse_packs_block_and_inline_forms() {
        assert_eq!(
            parse_packs("translator:\n  dictionary: pinyin_simp\n  packs:\n    - user_simp\n    - extra\n"),
            vec!["user_simp", "extra"]
        );
        assert_eq!(
            parse_packs("translator:\n  packs: [user_simp, \"extra\"]\n"),
            vec!["user_simp", "extra"]
        );
        // 没有 packs / 没有 translator / YAML 不合法 → 空列表（不报错）。
        assert!(parse_packs("translator:\n  dictionary: wubi86\n").is_empty());
        assert!(parse_packs("schema:\n  schema_id: x\n").is_empty());
        assert!(parse_packs("not: [valid\n").is_empty());
    }

    #[test]
    fn traverse_terminates_on_cycle_without_duplicating_entries() {
        let files: HashMap<&str, &str> = [
            ("a", "import_tables:\n  - b\n...\n甲\tjia\n"),
            ("b", "import_tables:\n  - a\n...\n乙\tyi\n"),
        ]
        .into_iter()
        .collect();

        let traversal = traverse_files("a", &[], &files);
        assert_eq!(traversal.tables, vec!["a", "b"], "每个码表只访问一次");
        assert_eq!(
            traversal.entries,
            vec![
                ("甲".to_string(), "jia".to_string()),
                ("乙".to_string(), "yi".to_string()),
            ],
            "自环/互引不会把词条重复拼进来"
        );
        assert!(traversal.missing.is_empty());

        // 自引用（A 引 A）同样终止。
        let self_ref: HashMap<&str, &str> =
            [("a", "import_tables: [a]\n...\n甲\tjia\n")].into_iter().collect();
        let traversal = traverse_files("a", &[], &self_ref);
        assert_eq!(traversal.tables, vec!["a"]);
        assert_eq!(traversal.entries.len(), 1);
    }

    #[test]
    fn missing_import_is_skipped_and_reported() {
        // A 声明了 b，但 b 的文件不存在：跳过 b，继续别的。
        let files: HashMap<&str, &str> = [(
            "a",
            "import_tables: [b, c]\n...\n甲\tjia\n",
        ), ("c", "...\n丙\tbing\n")]
            .into_iter()
            .collect();
        let traversal = traverse_files("a", &[], &files);
        assert_eq!(traversal.tables, vec!["a", "c"]);
        assert_eq!(traversal.missing, vec!["b"]);
        assert_eq!(traversal.entries.len(), 2);

        // packs 里的缺失码表同样只记 missing，不影响其它文件。
        let no_imports: HashMap<&str, &str> = [("a", "...\n甲\tjia\n")].into_iter().collect();
        let traversal = traverse_files("a", &["ghost".to_string()], &no_imports);
        assert_eq!(traversal.tables, vec!["a"]);
        assert_eq!(traversal.missing, vec!["ghost"]);
    }

    #[test]
    fn filtered_result_caps_entries_and_zeroes_commits() {
        let mut text = String::from("---\n...\n");
        for i in 0..(MAX_DICT_ENTRIES + 10) {
            text.push_str(&format!("字{i}\tcode{i:05}\n"));
        }
        let rows = parse_code_table_text(&text).entries;
        assert_eq!(rows.len(), MAX_DICT_ENTRIES + 10);
        let entries = entries_to_dict_entries(&rows);

        let all = finish_read("big", &["big".to_string()], &[], entries.len() as i32, &entries, "");
        assert_eq!(all.total, MAX_DICT_ENTRIES as i32 + 10);
        assert_eq!(
            all.matched,
            MAX_DICT_ENTRIES as i32 + 10,
            "命中总数不受截断影响"
        );
        assert_eq!(all.entries.len(), MAX_DICT_ENTRIES, "回传截断到上限");
        assert!(all.entries.iter().all(|entry| entry.commits == 0));
        assert_eq!(all.dict_name, "big");
        assert_eq!(all.tables, vec!["big".to_string()]);
        assert!(all.missing.is_empty());

        // 关键词命中（大小写不敏感），commits 依旧为 0。
        let one = finish_read(
            "big",
            &[],
            &[],
            entries.len() as i32,
            &entries,
            "CODE00007",
        );
        assert_eq!(one.matched, 1);
        assert_eq!(one.entries.len(), 1);
        assert_eq!(one.entries[0].word, "字7");
        assert_eq!(one.entries[0].commits, 0);
    }
}