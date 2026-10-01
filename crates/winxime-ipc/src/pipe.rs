use interprocess::os::windows::named_pipe::{pipe_mode::Bytes, DuplexPipeStream};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;
const READ_TIMEOUT_MS: u64 = 100;

#[derive(Debug)]
pub enum IpcError {
    ConnectionFailed(String),
    SerializeFailed(String),
    DeserializeFailed(String),
    WriteFailed(String),
    ReadFailed(String),
    EmptyResponse,
    ResponseTooLarge,
    Timeout,
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcError::ConnectionFailed(s) => write!(f, "ConnectionFailed: {}", s),
            IpcError::SerializeFailed(s) => write!(f, "SerializeFailed: {}", s),
            IpcError::DeserializeFailed(s) => write!(f, "DeserializeFailed: {}", s),
            IpcError::WriteFailed(s) => write!(f, "WriteFailed: {}", s),
            IpcError::ReadFailed(s) => write!(f, "ReadFailed: {}", s),
            IpcError::EmptyResponse => write!(f, "EmptyResponse"),
            IpcError::ResponseTooLarge => {
                write!(f, "ResponseTooLarge (max {} bytes)", MAX_RESPONSE_SIZE)
            }
            IpcError::Timeout => write!(f, "Timeout ({}ms)", READ_TIMEOUT_MS),
        }
    }
}

pub fn check_server_running() -> bool {
    let pipe_path = crate::messages::get_pipe_path();
    match DuplexPipeStream::<Bytes>::connect_by_path(pipe_path) {
        Ok(_) => true,
        Err(_) => false,
    }
}

pub fn stop_server() -> bool {
    if check_server_running() {
        IpcClient::shutdown_server()
    } else {
        false
    }
}

pub struct IpcClient {
    pipe: DuplexPipeStream<Bytes>,
}

impl IpcClient {
    pub fn connect() -> Result<Self, IpcError> {
        let pipe_path = crate::messages::get_pipe_path();

        match DuplexPipeStream::connect_by_path(pipe_path) {
            Ok(pipe) => Ok(Self { pipe }),
            Err(e) => Err(IpcError::ConnectionFailed(format!("{:?}", e))),
        }
    }

    pub fn send_request(
        &mut self,
        request: &crate::IpcRequest,
    ) -> Result<crate::IpcResponse, IpcError> {
        let json = serde_json::to_vec(request)
            .map_err(|e| IpcError::SerializeFailed(format!("{:?}", e)))?;

        self.pipe
            .write_all(&json)
            .map_err(|e| IpcError::WriteFailed(format!("write_all json: {:?}", e)))?;
        self.pipe
            .write_all(&[0])
            .map_err(|e| IpcError::WriteFailed(format!("write_all terminator: {:?}", e)))?;
        self.pipe
            .flush()
            .map_err(|e| IpcError::WriteFailed(format!("flush: {:?}", e)))?;

        let mut response_buf = Vec::new();
        let mut byte = [0u8; 1];
        // 超时口径是「两条字节之间的静默间隔」，不是「整个响应的总时长」：
        // 服务端的词典导出 / 同步等操作可能几百毫秒才吐出第一个字节（数据完整、
        // 只是慢），按总时长判会把慢而正确的响应误判成超时。
        let mut last_byte_at = Instant::now();

        loop {
            if response_buf.len() > MAX_RESPONSE_SIZE {
                return Err(IpcError::ResponseTooLarge);
            }
            if last_byte_at.elapsed() > Duration::from_millis(READ_TIMEOUT_MS) {
                return Err(IpcError::Timeout);
            }
            match self.pipe.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    if byte[0] == 0 {
                        break;
                    }
                    response_buf.push(byte[0]);
                    last_byte_at = Instant::now();
                }
                Err(e) => return Err(IpcError::ReadFailed(format!("{:?}", e))),
            }
        }

        if response_buf.is_empty() {
            return Err(IpcError::EmptyResponse);
        }

        serde_json::from_slice(&response_buf)
            .map_err(|e| IpcError::DeserializeFailed(format!("{:?}", e)))
    }

    pub fn send_oneway(&mut self, request: &crate::IpcRequest) -> Result<(), IpcError> {
        let json = serde_json::to_vec(request)
            .map_err(|e| IpcError::SerializeFailed(format!("{:?}", e)))?;
        self.pipe
            .write_all(&json)
            .map_err(|e| IpcError::WriteFailed(format!("{:?}", e)))?;
        self.pipe
            .write_all(&[0])
            .map_err(|e| IpcError::WriteFailed(format!("{:?}", e)))?;
        self.pipe
            .flush()
            .map_err(|e| IpcError::WriteFailed(format!("{:?}", e)))?;

        let mut response_buf = Vec::new();
        let mut byte = [0u8; 1];
        let start_time = Instant::now();

        loop {
            if response_buf.len() > MAX_RESPONSE_SIZE {
                return Err(IpcError::ResponseTooLarge);
            }
            if start_time.elapsed() > Duration::from_millis(READ_TIMEOUT_MS) {
                return Err(IpcError::Timeout);
            }
            match self.pipe.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    if byte[0] == 0 {
                        break;
                    }
                    response_buf.push(byte[0]);
                }
                Err(e) => return Err(IpcError::ReadFailed(format!("{:?}", e))),
            }
        }
        Ok(())
    }

    pub fn shutdown_server() -> bool {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ShutdownServer,
                session_id: 0,
                data: crate::IpcRequestData::None,
            };
            client.send_oneway(&request).is_ok()
        } else {
            false
        }
    }

    pub fn reload_config() -> bool {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ReloadConfig,
                session_id: 0,
                data: crate::IpcRequestData::None,
            };
            match client.send_request(&request) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    pub fn reload_plugins() -> bool {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ReloadPlugins,
                session_id: 0,
                data: crate::IpcRequestData::None,
            };
            match client.send_request(&request) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// rime 用户资料同步（同步执行，词典大时可能需要数秒）。
    pub fn sync_user_data() -> bool {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::SyncUserData,
                session_id: 0,
                data: crate::IpcRequestData::None,
            };
            match client.send_request(&request) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// 列出用户词典（含快照目录）；服务未运行返回 None。
    pub fn list_user_dicts() -> Option<crate::DictResponse> {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ListUserDicts,
                session_id: 0,
                data: crate::IpcRequestData::None,
            };
            match client.send_request(&request) {
                Ok(response) => response.dict_response,
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// 备份用户词典快照到同步目录。
    pub fn backup_user_dict(dict: &str) -> bool {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::BackupUserDict,
                session_id: 0,
                data: crate::IpcRequestData::UserDict(dict.to_string()),
            };
            match client.send_request(&request) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// 从快照文件恢复用户词典。
    pub fn restore_user_dict(snapshot_path: &str) -> bool {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::RestoreUserDict,
                session_id: 0,
                data: crate::IpcRequestData::UserDictPath(snapshot_path.to_string()),
            };
            match client.send_request(&request) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// 导出用户词典为文本，返回记录条数。
    pub fn export_user_dict(dict: &str, path: &str) -> Option<i32> {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ExportUserDict,
                session_id: 0,
                data: crate::IpcRequestData::UserDictFile(dict.to_string(), path.to_string()),
            };
            match client.send_request(&request) {
                Ok(response) => response.dict_response.map(|d| d.count),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// 从文本导入用户词典，返回记录条数。
    pub fn import_user_dict(dict: &str, path: &str) -> Option<i32> {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ImportUserDict,
                session_id: 0,
                data: crate::IpcRequestData::UserDictFile(dict.to_string(), path.to_string()),
            };
            match client.send_request(&request) {
                Ok(response) => response.dict_response.map(|d| d.count),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// 读取用户词典词条（query 为空即不过滤），返回词条列表与总数。
    ///
    /// 走 librime levers 的导出通道（`export_user_dict`）落临时文本再解析，
    /// 因此每次调用都是一次全库扫描：词库很大时应当只在打开页面/改关键词时调。
    pub fn list_dict_entries(dict: &str, query: &str) -> Option<crate::DictResponse> {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ListDictEntries,
                session_id: 0,
                data: crate::IpcRequestData::UserDictQuery(dict.to_string(), query.to_string()),
            };
            match client.send_request(&request) {
                Ok(response) => response.dict_response,
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// 写入一条用户词条（频率 > 0 新增；< 0 标记删除/tombstone），返回导入条数。
    ///
    /// 服务端会先销毁当前 rime 会话再导入（librime 要求 user dict 关闭后再写），
    /// 正在输入的句子会被打断——调用方要向用户说明。
    pub fn import_dict_entry(dict: &str, word: &str, code: &str, commits: i32) -> Option<i32> {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ImportDictEntry,
                session_id: 0,
                data: crate::IpcRequestData::UserDictEntry(
                    dict.to_string(),
                    word.to_string(),
                    code.to_string(),
                    commits,
                ),
            };
            match client.send_request(&request) {
                Ok(response) => response.dict_response.map(|d| d.count),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// 只读读取方案词表词条（query 为空即不过滤）。
    pub fn list_schema_entries(
        schema_id: &str,
        query: &str,
    ) -> Option<crate::SchemaDictResponse> {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ListSchemaEntries,
                session_id: 0,
                data: crate::IpcRequestData::SchemaQuery(
                    schema_id.to_string(),
                    query.to_string(),
                ),
            };
            match client.send_request(&request) {
                Ok(response) => response.schema_dict_response,
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// 读取某方案的快捷短语表（词 / 编码 / 权重 + 文件与 patch 状态）。
    pub fn list_custom_phrases(schema_id: &str) -> Option<crate::CustomPhraseResponse> {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ListCustomPhrases,
                session_id: 0,
                data: crate::IpcRequestData::SchemaName(schema_id.to_string()),
            };
            match client.send_request(&request) {
                Ok(response) => response.phrase_response,
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// 整表保存某方案的快捷短语（服务端写文件 + 视需要注入方案 patch），覆盖式。
    pub fn save_custom_phrases(
        schema_id: &str,
        entries: &[crate::CustomPhraseEntry],
    ) -> Option<crate::CustomPhraseResponse> {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::SaveCustomPhrases,
                session_id: 0,
                data: crate::IpcRequestData::CustomPhraseTable(
                    schema_id.to_string(),
                    entries.to_vec(),
                ),
            };
            match client.send_request(&request) {
                Ok(response) => response.phrase_response,
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// 请求 server 弹出系统通知（server 有 MSIX 包身份，toast 可归属）。
    pub fn show_toast(title: &str, body: &str) -> bool {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::ShowToast,
                session_id: 0,
                data: crate::IpcRequestData::Toast(crate::ToastMessage {
                    title: title.to_string(),
                    body: body.to_string(),
                }),
            };
            match client.send_request(&request) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    pub fn select_schema(schema_id: &str) -> bool {
        if let Ok(mut client) = Self::connect() {
            let request = crate::IpcRequest {
                command: crate::IpcCommand::SelectSchema,
                session_id: 0,
                data: crate::IpcRequestData::SelectSchema(schema_id.to_string()),
            };
            match client.send_request(&request) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }
}
