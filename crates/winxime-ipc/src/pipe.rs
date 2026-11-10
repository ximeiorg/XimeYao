use interprocess::os::windows::named_pipe::{pipe_mode::Bytes, DuplexPipeStream};
use interprocess::ConnectWaitMode;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

/// 建连上限。服务端是"接受一个连接、再补一个新实例"的循环，客户端可能正好赶上
/// 那个间隙；给它一点余量，但不能是无限（无限就是 TSF 宿主线程卡死的来源）。
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1000);
/// 探测「服务在不在」的建连上限。只用在启动/退出这类冷路径上，给宽一点，
/// 避免服务端恰好忙一下就被误判成「没在运行」。
const PROBE_TIMEOUT: Duration = Duration::from_millis(2000);
/// 快命令（按键、状态查询、toast）一条响应的上限。超过这个时间，
/// 说明对端已经不健康了——宁可丢这一次响应，也不能继续占着调用线程。
const QUICK_TIMEOUT: Duration = Duration::from_millis(1000);
/// 慢命令（词典导入导出、方案词表、同步、redeploy）的上限：这些本来就秒级。
const SLOW_TIMEOUT: Duration = Duration::from_secs(60);
/// 读响应时的自旋窗口。快命令的响应通常在 1ms 内到；自旋能避开 Windows
/// 定时器 1ms~15.6ms 的睡眠粒度，否则每次按键都被硬加十几毫秒。
const SPIN_WINDOW: Duration = Duration::from_millis(2);
/// 单次 read 的块大小（配合 PeekNamedPipe 的可用字节数）。
const READ_CHUNK: usize = 8 * 1024;

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
            IpcError::Timeout => write!(f, "Timeout"),
        }
    }
}

pub fn check_server_running() -> bool {
    connect_pipe(PROBE_TIMEOUT).is_ok()
}

fn connect_pipe(timeout: Duration) -> Result<DuplexPipeStream<Bytes>, IpcError> {
    let pipe_path = crate::messages::get_pipe_path();
    // 不用 `connect_by_path`（等实例是无上限的）：服务端没起、或者实例被占满时，
    // 调用线程会一直挂在 CreateFile 里。
    match DuplexPipeStream::<Bytes>::connect_by_path_with_wait_mode(
        pipe_path,
        ConnectWaitMode::Timeout(timeout),
    ) {
        Ok(pipe) => Ok(pipe),
        Err(e) => Err(IpcError::ConnectionFailed(format!("{:?}", e))),
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
    /// `None` = 这条连接已经不可再用。
    ///
    /// 读超时或管道出错之后，对端可能还是会把响应写进来；而协议里没有请求 id，
    /// 下一条请求就会把这条迟到的响应当成自己的（静默错位：候选词串到别的按键上）。
    /// 所以一出错就把管道丢掉，让调用方重新 connect。
    pipe: Option<DuplexPipeStream<Bytes>>,
}

/// 问管道「现在有多少字节可读」。函数本身不阻塞。
fn peek_available_bytes(pipe: &DuplexPipeStream<Bytes>) -> std::io::Result<u32> {
    let mut available = 0u32;
    // SAFETY: 句柄归 `pipe` 所有，调用期间一直有效；出参都是本栈上的可写变量。
    let ok = unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle() as _,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(available)
    }
}

/// 对端已经关掉管道（不是「暂时没数据」）。
fn is_pipe_gone(error: &std::io::Error) -> bool {
    match error.raw_os_error() {
        Some(code) => code == ERROR_BROKEN_PIPE as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32,
        None => false,
    }
}

/// 从词典命令的失败响应里取可读原因；服务端没给就退回调用方给的默认文案。
fn dict_error_message(response: Option<crate::DictResponse>, fallback: &str) -> String {
    response
        .and_then(|d| d.error)
        .unwrap_or_else(|| fallback.to_string())
}

impl IpcClient {
    pub fn connect() -> Result<Self, IpcError> {
        connect_pipe(CONNECT_TIMEOUT).map(|pipe| Self { pipe: Some(pipe) })
    }

    /// 连接还能不能用（出过一次错就作废，见结构体里的 `pipe` 字段说明）。
    pub fn is_usable(&self) -> bool {
        self.pipe.is_some()
    }

    /// 发请求并读一条响应（快命令口径）。按键这类热路径用这个。
    pub fn send_request(
        &mut self,
        request: &crate::IpcRequest,
    ) -> Result<crate::IpcResponse, IpcError> {
        self.send_request_with_timeout(request, QUICK_TIMEOUT)
    }

    /// 同上，显式指定整条响应的上限（慢命令传 `SLOW_TIMEOUT`）。
    pub fn send_request_with_timeout(
        &mut self,
        request: &crate::IpcRequest,
        timeout: Duration,
    ) -> Result<crate::IpcResponse, IpcError> {
        let mut pipe = self.take_pipe()?;
        let result = Self::request_response(&mut pipe, request, timeout);
        if result.is_ok() {
            self.pipe = Some(pipe);
        }
        result
    }

    fn take_pipe(&mut self) -> Result<DuplexPipeStream<Bytes>, IpcError> {
        self.pipe.take().ok_or_else(|| {
            IpcError::ConnectionFailed("connection dropped after an earlier error".to_string())
        })
    }

    fn request_response(
        pipe: &mut DuplexPipeStream<Bytes>,
        request: &crate::IpcRequest,
        timeout: Duration,
    ) -> Result<crate::IpcResponse, IpcError> {
        Self::write_request(pipe, request)?;
        let response = Self::read_response(pipe, timeout)?;
        serde_json::from_slice(&response)
            .map_err(|e| IpcError::DeserializeFailed(format!("{:?}", e)))
    }

    fn write_request(
        pipe: &mut DuplexPipeStream<Bytes>,
        request: &crate::IpcRequest,
    ) -> Result<(), IpcError> {
        let json = serde_json::to_vec(request)
            .map_err(|e| IpcError::SerializeFailed(format!("{:?}", e)))?;
        pipe.write_all(&json)
            .map_err(|e| IpcError::WriteFailed(format!("write_all json: {:?}", e)))?;
        pipe.write_all(&[0])
            .map_err(|e| IpcError::WriteFailed(format!("write_all terminator: {:?}", e)))?;
        pipe.flush()
            .map_err(|e| IpcError::WriteFailed(format!("flush: {:?}", e)))?;
        Ok(())
    }

    /// 读一条以 NUL 结尾的响应，`timeout` 是整条响应的上限。
    ///
    /// 原来的写法是「直接 `read` 一个字节，前后比 `Instant`」，可 `read` 自己不带超时：
    /// 对端一声不响时它会永远阻塞，那个 `Instant` 检查也就永远轮不到——调用方是
    /// TSF 里的宿主 UI 线程，卡在这里就是整桌面假死。
    ///
    /// 现在先 `PeekNamedPipe` 问「到了多少字节」，只读**已经到达**的部分，所以每次
    /// `read` 都是短读；没数据就自旋（快命令通常 1ms 内到，避开 Windows 定时器
    /// 1ms~15.6ms 的睡眠粒度）或 1ms 睡眠等，到期返回 [`IpcError::Timeout`]。
    ///
    /// 顺带把「逐字节 read」改成「按可用字节整块 read」：一次候选栏响应原来要几百
    /// 上千次系统调用。
    fn read_response(
        pipe: &mut DuplexPipeStream<Bytes>,
        timeout: Duration,
    ) -> Result<Vec<u8>, IpcError> {
        let start = Instant::now();
        let deadline = start + timeout;
        let spin_until = start + SPIN_WINDOW;
        let mut out: Vec<u8> = Vec::new();
        let mut chunk = [0u8; READ_CHUNK];

        loop {
            let available = match peek_available_bytes(pipe) {
                Ok(available) => available,
                // 对端关闭：和 `read` 返回 0 一样按 EOF 处理，交给下面的统一判空。
                Err(e) if is_pipe_gone(&e) => break,
                Err(e) => return Err(IpcError::ReadFailed(format!("PeekNamedPipe: {e}"))),
            };

            if available == 0 {
                let now = Instant::now();
                if now >= deadline {
                    return Err(IpcError::Timeout);
                }
                if now < spin_until {
                    std::hint::spin_loop();
                } else {
                    std::thread::sleep(Duration::from_millis(1));
                }
                continue;
            }

            let want = (available as usize).min(chunk.len());
            let read = pipe
                .read(&mut chunk[..want])
                .map_err(|e| IpcError::ReadFailed(format!("{:?}", e)))?;
            if read == 0 {
                break;
            }
            if let Some(end) = chunk[..read].iter().position(|&b| b == 0) {
                out.extend_from_slice(&chunk[..end]);
                break;
            }
            out.extend_from_slice(&chunk[..read]);
            if out.len() > MAX_RESPONSE_SIZE {
                return Err(IpcError::ResponseTooLarge);
            }
        }

        if out.is_empty() {
            return Err(IpcError::EmptyResponse);
        }
        Ok(out)
    }

    /// 发一条命令并**把应答读掉、不解析**（`FocusIn`/`EndSession` 这类「只关心成没成」的）。
    ///
    /// 名字容易误会：协议是严格一问一答，这里**必须**把应答读走，否则它会留在管道里，
    /// 被下一条请求读成自己的应答（无请求 id，静默串位）。真单向命令请用
    /// [`IpcClient::send_no_reply`]。
    pub fn send_oneway(&mut self, request: &crate::IpcRequest) -> Result<(), IpcError> {
        let mut pipe = self.take_pipe()?;
        let outcome = Self::write_request(&mut pipe, request).and_then(|()| {
            // 只把响应读掉、不解析；退出流程里对端可能直接关管道，空响应也算成功。
            match Self::read_response(&mut pipe, QUICK_TIMEOUT) {
                Ok(_) | Err(IpcError::EmptyResponse) => Ok(()),
                Err(e) => Err(e),
            }
        });
        if outcome.is_ok() {
            self.pipe = Some(pipe);
        }
        outcome
    }

    /// 发一条**服务端不回包**的命令（`IpcCommand::is_one_way`），写完就走。
    ///
    /// 与 [`IpcClient::send_oneway`] 的区别是这里**不读**：省掉一个完整往返（位置上报
    /// 每次按键都发）。前提是服务端对同一条命令也不写（见 `handle_connection` 里的
    /// 跳过分支）——两边不一致就会出现应答积压、下一条请求读错。
    pub fn send_no_reply(&mut self, request: &crate::IpcRequest) -> Result<(), IpcError> {
        debug_assert!(
            request.command.is_one_way(),
            "send_no_reply 只能用于单向命令；会回包的命令请用 send_request / send_oneway"
        );
        let mut pipe = self.take_pipe()?;
        let outcome = Self::write_request(&mut pipe, request);
        if outcome.is_ok() {
            self.pipe = Some(pipe);
        }
        outcome
    }

    /// 读一次状态（当前方案 / 中英态 / 组词状态）。服务未运行或引擎忙 → None。
    ///
    /// 只读：不建会话、不改状态，设置页启动时安全调用。
    pub fn get_status() -> Option<crate::Status> {
        let mut client = Self::connect().ok()?;
        let request = crate::IpcRequest {
            command: crate::IpcCommand::GetStatus,
            session_id: 0,
            data: crate::IpcRequestData::None,
        };
        client
            .send_request_with_timeout(&request, QUICK_TIMEOUT)
            .ok()?
            .status
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// 从快照文件恢复用户词典。
    ///
    /// 路径必须落在用户数据目录或系统临时目录内，否则服务端拒绝并回可读原因
    /// （审计 F2 的路径白名单）；设置页宿主会先把用户选的文件复制进临时目录。
    pub fn restore_user_dict(snapshot_path: &str) -> Result<(), String> {
        let mut client = Self::connect().map_err(|_| "输入法服务未运行".to_string())?;
        let request = crate::IpcRequest {
            command: crate::IpcCommand::RestoreUserDict,
            session_id: 0,
            data: crate::IpcRequestData::UserDictPath(snapshot_path.to_string()),
        };
        let response = client
            .send_request_with_timeout(&request, SLOW_TIMEOUT)
            .map_err(|e| format!("与输入法服务通信失败：{e}"))?;
        if response.success {
            Ok(())
        } else {
            Err(dict_error_message(response.dict_response, "恢复用户词典失败"))
        }
    }

    /// 导出用户词典为文本，返回（记录条数, 服务端生成的文件路径）。
    ///
    /// 文件由服务端写在系统临时目录，调用方读走/复制后应自行删除；服务端不再
    /// 接受调用方指定的写入路径（审计 F2）。
    pub fn export_user_dict(dict: &str) -> Result<(i32, String), String> {
        let mut client = Self::connect().map_err(|_| "输入法服务未运行".to_string())?;
        let request = crate::IpcRequest {
            command: crate::IpcCommand::ExportUserDict,
            session_id: 0,
            data: crate::IpcRequestData::UserDict(dict.to_string()),
        };
        let response = client
            .send_request_with_timeout(&request, SLOW_TIMEOUT)
            .map_err(|e| format!("与输入法服务通信失败：{e}"))?;
        if !response.success {
            return Err(dict_error_message(response.dict_response, "导出用户词典失败"));
        }
        let dict_response = response
            .dict_response
            .ok_or_else(|| "服务端没有返回导出结果".to_string())?;
        let path = dict_response
            .exported_path
            .ok_or_else(|| "服务端没有返回导出文件路径".to_string())?;
        Ok((dict_response.count, path))
    }

    /// 从文本导入用户词典，返回记录条数。
    ///
    /// 路径限制同 [`Self::restore_user_dict`]：只接受用户数据目录或临时目录内的真实文件。
    pub fn import_user_dict(dict: &str, path: &str) -> Result<i32, String> {
        let mut client = Self::connect().map_err(|_| "输入法服务未运行".to_string())?;
        let request = crate::IpcRequest {
            command: crate::IpcCommand::ImportUserDict,
            session_id: 0,
            data: crate::IpcRequestData::UserDictFile(dict.to_string(), path.to_string()),
        };
        let response = client
            .send_request_with_timeout(&request, SLOW_TIMEOUT)
            .map_err(|e| format!("与输入法服务通信失败：{e}"))?;
        if !response.success {
            return Err(dict_error_message(response.dict_response, "导入用户词典失败"));
        }
        response
            .dict_response
            .map(|d| d.count)
            .ok_or_else(|| "服务端没有返回导入结果".to_string())
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
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
            match client.send_request_with_timeout(&request, SLOW_TIMEOUT) {
                Ok(response) => response.success,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// 语音状态（设置页「语音转文本」区每 250ms 拉一次）。
    ///
    /// server 没起来 / 管道不通都返回 None，调用方据此显示「服务未运行」，
    /// 不要把它当成「没有模型」。
    pub fn speech_status() -> Option<crate::SpeechStatus> {
        let mut client = Self::connect().ok()?;
        let request = crate::IpcRequest {
            command: crate::IpcCommand::GetSpeechStatus,
            session_id: 0,
            data: crate::IpcRequestData::None,
        };
        match client.send_request(&request) {
            Ok(response) => response.status.and_then(|status| status.speech),
            Err(_) => None,
        }
    }

    /// 语音模型的写操作（下载 / 删除 / 切换 / 试听起停）。
    ///
    /// 返回 `Some(status)` = 命令已被 server 受理（status 里带最新快照，可能含
    /// 本次操作的错误）；`None` = 管道不通 / 没起来。
    fn speech_action(
        command: crate::IpcCommand,
        data: crate::IpcRequestData,
    ) -> Option<crate::SpeechStatus> {
        let mut client = Self::connect().ok()?;
        let request = crate::IpcRequest {
            command,
            session_id: 0,
            data,
        };
        match client.send_request(&request) {
            Ok(response) => response.status.and_then(|status| status.speech),
            Err(_) => None,
        }
    }

    /// 下载语音模型（异步：进度看 [`IpcClient::speech_status`] 的 `download`）。
    pub fn speech_download(model_id: &str) -> Option<crate::SpeechStatus> {
        Self::speech_action(
            crate::IpcCommand::SpeechDownload,
            crate::IpcRequestData::SpeechModel(model_id.to_string()),
        )
    }

    /// 删除语音模型目录。
    pub fn speech_delete(model_id: &str) -> Option<crate::SpeechStatus> {
        Self::speech_action(
            crate::IpcCommand::SpeechDelete,
            crate::IpcRequestData::SpeechModel(model_id.to_string()),
        )
    }

    /// 切换当前使用的语音模型。
    pub fn speech_select(model_id: &str) -> Option<crate::SpeechStatus> {
        Self::speech_action(
            crate::IpcCommand::SpeechSelect,
            crate::IpcRequestData::SpeechModel(model_id.to_string()),
        )
    }

    /// 开始试听（server 侧开麦克风；结果只回显，不上屏）。
    pub fn speech_test_start() -> Option<crate::SpeechStatus> {
        Self::speech_action(crate::IpcCommand::SpeechTestStart, crate::IpcRequestData::None)
    }

    /// 结束试听。
    pub fn speech_test_stop() -> Option<crate::SpeechStatus> {
        Self::speech_action(crate::IpcCommand::SpeechTestStop, crate::IpcRequestData::None)
    }
}
