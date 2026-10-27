use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Text {
    pub str: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CandidateInfo {
    pub current_page: u32,
    pub total_pages: u32,
    pub highlighted: usize,
    pub is_last_page: bool,
    pub candies: Vec<Text>,
    pub comments: Vec<Text>,
    pub labels: Vec<Text>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Context {
    pub preedit: Text,
    pub commit: Option<String>,
    pub candidates: CandidateInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Status {
    pub schema_name: String,
    pub schema_id: String,
    pub ascii_mode: bool,
    pub composing: bool,
    /// 语音子状态：**只有语音相关命令才填**（其余命令留 None——它是按键热路径，
    /// 不能每次都去查模型目录）。这样新加字段也不用改 `IpcResponse` 那几十处
    /// 逐字段字面量。
    #[serde(default)]
    pub speech: Option<SpeechStatus>,
}

/// 语音状态快照（设置页「语音转文本」区的一屏数据）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpeechStatus {
    /// 引擎状态：`idle` / `loading` / `listening`。
    #[serde(default)]
    pub state: String,
    /// 当前选中的模型 id。
    #[serde(default)]
    pub model_id: String,
    /// 当前选中的模型展示名。
    #[serde(default)]
    pub model_name: String,
    /// 选中模型是否已下载完整。
    #[serde(default)]
    pub model_ready: bool,
    /// 推理后端说明（如 `CPU` / `CUDA（GPU）`）。
    #[serde(default)]
    pub provider: String,
    /// 实时识别文本（试听时用）。
    #[serde(default)]
    pub text: String,
    /// 最近一次错误（会话失败 / 模型操作失败）。
    #[serde(default)]
    pub error: Option<String>,
    /// 正在下载的模型与进度（None = 没有下载在进行）。
    #[serde(default)]
    pub download: Option<SpeechDownload>,
    /// 模型集合变化计数：与上一次不同就重查 `models`。
    #[serde(default)]
    pub models_rev: u64,
    /// 可管理的模型列表。
    #[serde(default)]
    pub models: Vec<SpeechModel>,
}

/// 下载中的模型与进度。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpeechDownload {
    /// 目标模型 id。
    #[serde(default)]
    pub model_id: String,
    /// 进度 0.0~1.0。
    #[serde(default)]
    pub progress: f32,
}

/// 可管理的一个语音模型。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpeechModel {
    /// 模型 id。
    #[serde(default)]
    pub id: String,
    /// 展示名。
    #[serde(default)]
    pub name: String,
    /// 一句话描述。
    #[serde(default)]
    pub description: String,
    /// 下载包大小（展示用）。
    #[serde(default)]
    pub size: String,
    /// 四件套是否已下载完整。
    #[serde(default)]
    pub downloaded: bool,
    /// 是否为当前选中。
    #[serde(default)]
    pub selected: bool,
    /// 是否为推荐模型（设置页挂「推荐」标记）。
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcCommand {
    Echo,
    /// 只读状态（当前方案 / 中英态 / 组词状态），不动引擎、不开会话。
    /// 设置页用它把「当前方案」标到真正在用的那一行。
    GetStatus,
    StartSession,
    EndSession,
    ProcessKeyEvent,
    UpdateInputPosition,
    FocusIn,
    FocusOut,
    SelectCandidate,
    ChangePage,
    CommitComposition,
    ClearComposition,
    ShutdownServer,
    ToggleAsciiMode,
    /// 兼容旧版 TSF：服务端已不再根据它隐藏托盘，只做 no-op。
    ShowTrayIcon,
    /// 兼容旧版 TSF：服务端已不再根据它隐藏托盘，只做 no-op。
    HideTrayIcon,
    HideCandidates,
    ReloadConfig,
    ReloadPlugins,
    /// rime 用户资料同步（导出/合并用户词典快照，对齐 weasel「用户资料同步」）。
    SyncUserData,
    /// 词典管理：列出用户词典（对齐 weasel DictManagementDialog）。
    ListUserDicts,
    /// 词典管理：备份用户词典快照到同步目录（data = UserDict）。
    BackupUserDict,
    /// 词典管理：从快照文件恢复（data = UserDictPath）。
    RestoreUserDict,
    /// 词典管理：导出用户词典为文本（data = UserDictFile(dict, path)）。
    ExportUserDict,
    /// 词典管理：从文本导入用户词典（data = UserDictFile(dict, path)）。
    ImportUserDict,
    /// 词典管理：读取用户词典词条用于浏览/搜索（data = UserDictQuery(dict, query)）。
    ListDictEntries,
    /// 词典管理：写入一条用户词条（data = UserDictEntry；频率 > 0 新增，< 0 标记删除）。
    ImportDictEntry,
    /// 方案词表：只读读取方案码表词条（data = SchemaQuery(schema_id, query)）。
    ListSchemaEntries,
    /// 快捷短语：读取某方案的短语表（data = SchemaName(schema_id)）。
    ListCustomPhrases,
    /// 快捷短语：整表保存（data = CustomPhraseTable(schema_id, entries)）。
    SaveCustomPhrases,
    /// 系统通知（toast）：由有 MSIX 包身份的 server 进程弹出
    /// （设置进程直跑无包身份，toast 无从归属）。
    ShowToast,
    GetSchemaList,
    SelectSchema,
    ShowRoot,
    HideRoot,
    // 语音（本地离线模型）：设置页的「语音转文本」区驱动 server 侧的语音引擎。
    /// 读语音状态：当前模型 / 可管理模型列表 / 下载进度 / 引擎状态。
    GetSpeechStatus,
    /// 开始下载模型（data = SpeechModel(id)）；立即返回，进度靠 GetSpeechStatus 轮询。
    SpeechDownload,
    /// 删除模型目录（data = SpeechModel(id)）。
    SpeechDelete,
    /// 切换当前使用的模型（data = SpeechModel(id)）。
    SpeechSelect,
    /// 试听：让 server 开一次识别会话（麦克风归 server 所有，面板与设置页共用）。
    SpeechTestStart,
    /// 试听结束：停止并**不上屏**（设置页试听不该往用户光标处塞文本）。
    SpeechTestStop,
}

impl IpcCommand {
    /// 是不是「单向」命令：客户端只写、服务端**不回包**，两边必须同时遵守。
    ///
    /// 协议是「NUL 结尾的 JSON，一问一答」，请求和应答靠**顺序**配对、没有请求 id。
    /// 所以回包命令一旦被当成单向（服务端多写了没人读，或者客户端不读而服务端写了），
    /// 那条应答就会留在管道里，被下一条请求读成自己的应答——候选词/上屏文本串位，
    /// 而且完全不报错。新增单向命令必须同时改这里、服务端 `handle_connection` 的
    /// 跳过分支和客户端 [`crate::IpcClient::send_no_reply`] 的调用点。
    ///
    /// 目前只有位置上报：它每次按键都发，坐标是「尽力而为」，等应答纯属浪费一个
    /// 完整往返。
    pub fn is_one_way(&self) -> bool {
        matches!(self, Self::UpdateInputPosition)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub command: IpcCommand,
    pub session_id: u32,
    pub data: IpcRequestData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum IpcRequestData {
    None,
    KeyEvent(KeyEventData),
    Position(Position),
    SelectIndex(usize),
    ChangePage(bool),
    SelectSchema(String),
    ShowRoot(char),
    /// 词典管理：用户词典名（BackupUserDict）。
    UserDict(String),
    /// 词典管理：快照文件路径（RestoreUserDict）。
    UserDictPath(String),
    /// 词典管理：用户词典名 + 文本文件路径（ExportUserDict / ImportUserDict）。
    UserDictFile(String, String),
    /// 词典管理：用户词典名 + 过滤关键词（ListDictEntries，空串即不过滤）。
    UserDictQuery(String, String),
    /// 词典管理：用户词典名 + 词 + 编码 + 频率（ImportDictEntry；频率 < 0 即标记删除）。
    UserDictEntry(String, String, String, i32),
    /// 方案词表：方案 id + 过滤关键词（ListSchemaEntries，空串即不过滤）。
    SchemaQuery(String, String),
    /// 快捷短语：方案 id（ListCustomPhrases）。
    SchemaName(String),
    /// 快捷短语：方案 id + 整张短语表（SaveCustomPhrases，覆盖式写入）。
    CustomPhraseTable(String, Vec<CustomPhraseEntry>),
    /// 系统通知内容（ShowToast）。
    Toast(ToastMessage),
    /// 语音模型操作的目标模型 id（SpeechDownload / SpeechDelete / SpeechSelect）。
    SpeechModel(String),
}

/// 系统通知标题 + 正文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToastMessage {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub schema_id: String,
    pub schema_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEventData {
    pub keycode: i32,
    pub modifiers: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IpcResponse {
    pub success: bool,
    pub session_id: u32,
    #[serde(default)]
    pub context: Option<Context>,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub schema_list: Option<Vec<SchemaInfo>>,
    #[serde(default)]
    pub dict_response: Option<DictResponse>,
    /// 方案词表读取响应（ListSchemaEntries）。
    #[serde(default)]
    pub schema_dict_response: Option<SchemaDictResponse>,
    /// 快捷短语响应（ListCustomPhrases / SaveCustomPhrases）。
    #[serde(default)]
    pub phrase_response: Option<CustomPhraseResponse>,
}

impl IpcResponse {
    /// 成功响应骨架：只填 `success`/`session_id`，其余字段留空。
    ///
    /// 处理分支里通常只关心一两个字段（`status` / `dict_response` / …），
    /// 用 `IpcResponse { status: Some(..), ..IpcResponse::ok(id) }` 补即可，
    /// 不必每次重复写全 8 个字段。
    pub fn ok(session_id: u32) -> Self {
        Self {
            success: true,
            session_id,
            ..Default::default()
        }
    }

    /// 失败响应骨架（字段布局同 [`Self::ok`]）。
    pub fn fail(session_id: u32) -> Self {
        Self {
            success: false,
            session_id,
            ..Default::default()
        }
    }
}

/// 词典管理响应（对齐 weasel DictManagementDialog 的操作结果）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DictResponse {
    /// 用户词典名列表（ListUserDicts 时填充）。
    #[serde(default)]
    pub dicts: Vec<String>,
    /// 导出/导入的记录条数；ListDictEntries 时为**命中条数**（未受回传上限影响）。
    #[serde(default)]
    pub count: i32,
    /// 快照目录（ListUserDicts 时填充）。
    #[serde(default)]
    pub sync_dir: String,
    /// 词条列表（ListDictEntries 时填充，最多 `MAX_DICT_ENTRIES` 条）。
    #[serde(default)]
    pub entries: Vec<DictEntry>,
    /// 词条总数（ListDictEntries 时填充，未受关键词过滤与条数上限影响）。
    #[serde(default)]
    pub total: i32,
}

/// 用户词典中的一条词条（词 / 编码 / 频率）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DictEntry {
    /// 词条文本。
    pub word: String,
    /// 编码（五笔码、拼音码等；可能为空）。
    pub code: String,
    /// 频率（librime 的 commits，越大越优先）。
    pub commits: i32,
}

/// 单次 `ListDictEntries` 最多返回的词条数。
///
/// 命名管道单帧上限 1MB（`winxime-server::ipc_server::MAX_BUFFER_SIZE`），
/// 大词库全量回传会超限；超出部分由设置页提示"用搜索框查找"。
pub const MAX_DICT_ENTRIES: usize = 500;

/// 方案词表读取响应（ListSchemaEntries）。
///
/// 方案词表是**只读**的：码表随方案文件发布，改它属于改方案本身；
/// 要加自己的词，走用户词典（ImportDictEntry）或快捷短语（SaveCustomPhrases）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchemaDictResponse {
    /// 方案主码表名（`<schema_id>.schema.yaml` 里 `dictionary:` 的值）。
    #[serde(default)]
    pub dict_name: String,
    /// 实际读入的码表名（主表在前，import_tables / translator.packs 按读入顺序）。
    #[serde(default)]
    pub tables: Vec<String>,
    /// 方案里声明了但文件不存在的码表名（提示用，不算错误）。
    #[serde(default)]
    pub missing: Vec<String>,
    /// 词条列表（最多 `MAX_DICT_ENTRIES` 条；方案词表没有频率概念，`commits` 恒为 0）。
    #[serde(default)]
    pub entries: Vec<DictEntry>,
    /// 读入的词条总数（不受关键词过滤与条数上限影响）。
    #[serde(default)]
    pub total: i32,
    /// 命中条数（未受回传上限影响）。
    #[serde(default)]
    pub matched: i32,
}

/// 快捷短语的一条（词 / 编码 / 可选权重）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CustomPhraseEntry {
    /// 短语文本。
    pub word: String,
    /// 触发编码。
    pub code: String,
    /// 权重（越大越优先）；`None` = 文件里没写这一列（rime 视为默认权重）。
    #[serde(default)]
    pub weight: Option<i32>,
}

/// 快捷短语响应（ListCustomPhrases / SaveCustomPhrases）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomPhraseResponse {
    /// 解析出的短语表名（`custom_phrase.user_dict`，通常就是 `custom_phrase`）。
    #[serde(default)]
    pub dict_name: String,
    /// 短语表文件名（`<dict_name>.txt`）。
    #[serde(default)]
    pub file_name: String,
    /// 短语表文件是否已存在。
    #[serde(default)]
    pub file_exists: bool,
    /// 方案 custom.yaml 里是否已注入 custom_phrase 翻译器。
    #[serde(default)]
    pub patch_applied: bool,
    /// 本次保存是否**新**注入了翻译器（是 → 需要重新部署才生效）。
    #[serde(default)]
    pub patch_added: bool,
    /// 短语列表（保存成功后回填保存后的整表）。
    #[serde(default)]
    pub entries: Vec<CustomPhraseEntry>,
}

pub const IPC_PIPE_NAME: &str = "WinximeNamedPipe";

pub fn get_pipe_path() -> String {
    format!("\\\\.\\pipe\\{}", IPC_PIPE_NAME)
}

#[cfg(test)]
mod tests {
    use super::IpcCommand;

    /// 单向命令（服务端不回包）只允许是位置上报。
    ///
    /// 这个测试的主要作用是**反向约束**：回包命令一旦被登记成单向，服务端就不写应答，
    /// 客户端却还在等；反过来（登记漏了）则应答堆在管道里被下一条请求读走。
    /// 两种都是静默串位，编译期看不出来，只能靠这条守住。
    #[test]
    fn only_position_update_is_one_way() {
        assert!(IpcCommand::UpdateInputPosition.is_one_way());
        // 典型的回包命令：按键、会话、开关、退出、配置、语音状态。
        for command in [
            IpcCommand::Echo,
            IpcCommand::GetStatus,
            IpcCommand::StartSession,
            IpcCommand::EndSession,
            IpcCommand::ProcessKeyEvent,
            IpcCommand::FocusIn,
            IpcCommand::FocusOut,
            IpcCommand::ToggleAsciiMode,
            IpcCommand::ShutdownServer,
            IpcCommand::ReloadConfig,
            IpcCommand::SyncUserData,
            IpcCommand::ShowToast,
            IpcCommand::GetSpeechStatus,
        ] {
            assert!(
                !command.is_one_way(),
                "{command:?} 会回包，不能登记成单向命令"
            );
        }
    }
}
