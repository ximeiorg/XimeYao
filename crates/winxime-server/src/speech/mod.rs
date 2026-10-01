//! 本地语音识别（sherpa-onnx 流式 zipformer）：麦克风 → 识别 → 上屏。
//!
//! 线程与所有权：
//! - 识别器由**一个专用工作线程**独占（`StreamingRecognizer` 不是 Sync 的，
//!   也绝不能让它去碰 rime 引擎锁）——UI 线程只发命令、只读状态快照；
//! - 采集按会话开关（见 [`capture`]），空闲时不占麦克风；识别器会话间复用，
//!   避免每次说话都重新装载 148MB 模型（首次装载约 3s，UI 显示「正在加载」）；
//! - 上屏复用面板那条 F24 自注入通道（[`crate::paste::request_commit`]），
//!   因此语音文本走的是和打字/选词完全相同的 TSF 编辑会话，不需要扩 IPC 协议。
//!
//! 会话结束有两种触发：说话停顿到端点（sherpa 端点检测，免手按）或显式
//! `Stop`（面板「停止」）。

mod capture;
/// 模型目录与下载（设置页的「本地模型」区驱动）。
pub mod download;
/// 语音设置持久化（选中模型）。
pub mod settings;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use xime_speech::{AsrModelProfile, AsrModelRegistry, SpeechConfig, StreamingRecognizer};
#[cfg(feature = "speech-cuda")]
use xime_speech::SpeechProvider;

/// 识别器工作线程数：int8 流式 zipformer 在 CPU 上 2 线程即可实时，
/// 再多只会和前台应用抢核。
const NUM_THREADS: i32 = 2;

/// 无音频时的轮询间隔：采集是共享模式轮询，10ms 足够跟得上（缓冲 200ms）。
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 会话状态（UI 只读快照的一部分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpeechState {
    /// 空闲（未在采集）。
    #[default]
    Idle,
    /// 正在装载模型/打开麦克风。
    Loading,
    /// 正在采集识别。
    Listening,
}

/// UI 线程读的状态快照。
///
/// 实现 `PartialEq`：面板 🎙️ 页的定时器靠它判断「这一拍有没有变化」，
/// 没变化就不重绘。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeechSink {
    /// 当前状态。
    pub state: SpeechState,
    /// 实时识别文本（聆听中持续更新；结束时保留最后结果）。
    pub partial: String,
    /// 最近一次错误（模型缺失、麦克风打不开等）；成功开始会话时清空。
    pub error: Option<String>,
    /// 最近一次成功上屏的文本（UI 用来显示「已上屏」反馈）。
    pub last_commit: Option<String>,
    /// 输入电平 0~100（面板 🎙️ 页的电平条）。
    ///
    /// 用 `u8` 而不是 `f32`：既保住 `Eq`（快照要靠它判断有没有变化），
    /// 也天然把重绘压到「肉眼能分辨的档位」——电平条本来也只有几十像素宽。
    pub level: u8,
}

/// 输入电平包络（面板 🎙️ 页的电平条）。
///
/// 做法：每块 PCM 算 RMS → 归一化到 0~100（`-50dB` 以下当静音、`-10dB` 当满格，
/// 人说话的正常音量正好落在这段）→ 快起慢落（涨立刻跟上、落按 0.6 衰减）。
/// 「快起慢落」是电平表的通用观感：不衰减会闪成噪声，衰减太快又会一直贴地。
#[derive(Debug, Default)]
struct LevelEnvelope {
    /// 上一块的电平（0~1）。
    last: f32,
}

impl LevelEnvelope {
    /// 静音门限（-50dBFS）：低于它一律当 0，避免底噪把电平条顶起来。
    const FLOOR: f32 = 0.003_16;
    /// 满格门限（-10dBFS）：正常说话音量到这里就是满格。
    const CEIL: f32 = 0.316;
    /// 落下系数（每块乘一次，约 0.4s 落到底）。
    const DECAY: f32 = 0.6;

    /// 吃一块 PCM（单声道 f32，范围 -1~1），返回 0~100 的电平。
    fn next(&mut self, chunk: &[f32]) -> u8 {
        let raw = rms_to_unit(chunk);
        let value = if raw > self.last {
            raw
        } else {
            self.last * Self::DECAY
        };
        self.last = value;
        (value * 100.0).round().clamp(0.0, 100.0) as u8
    }

    /// 会话结束：清掉包络（下次会话从 0 起）。
    fn reset(&mut self) {
        self.last = 0.0;
    }
}

/// 一块 PCM 的 RMS → 0~1（门限内线性映射；空块/静音为 0）。
fn rms_to_unit(chunk: &[f32]) -> f32 {
    if chunk.is_empty() {
        return 0.0;
    }
    let sum: f32 = chunk.iter().map(|s| s * s).sum();
    let rms = (sum / chunk.len() as f32).sqrt();
    if rms <= LevelEnvelope::FLOOR {
        return 0.0;
    }
    ((rms - LevelEnvelope::FLOOR) / (LevelEnvelope::CEIL - LevelEnvelope::FLOOR)).clamp(0.0, 1.0)
}

/// 工作线程命令（`Warmup` / `Start` / `Stop` / `Cancel` 由面板 🎙️ 页发出，
/// 模型管理三条由设置页经 IPC 发出）。
enum Command {
    /// 预装载识别器（进入语音页时调，摊掉 3s 装载延迟；不开麦克风）。
    Warmup,
    /// 开始一次会话（打开麦克风 + 识别）。
    Start,
    /// 试听会话（设置页）：识别结果只显示、不上屏。
    StartPreview,
    /// 结束会话并上屏。
    Stop,
    /// 放弃本次会话（不上屏）。
    Cancel,
    /// 切换选中模型（持久化在引擎侧已完成；这里丢掉旧识别器）。
    SelectModel(String),
    /// 删除模型目录（丢识别器 → 删目录 → 必要时清选中）。
    DeleteModel(String),
    /// 线程退出。
    Shutdown,
}

/// 语音引擎句柄（进程级单例）。
///
/// 只留「发命令 + 读快照 + 数据根」三样：**选中模型不缓存在这里**，每次都按
/// `speech.toml` 现读（设置程序换了模型，server 不必重启也不必同步）。
pub struct SpeechEngine {
    tx: Sender<Command>,
    /// 状态快照（UI 线程读，见 [`SpeechEngine::snapshot`]）。
    sink: Arc<Mutex<SpeechSink>>,
    /// 数据根（`%APPDATA%\Xime`）：`speech.toml` 与 `models/` 都在它下面。
    data_root: PathBuf,
}

static ENGINE: OnceLock<SpeechEngine> = OnceLock::new();

/// 初始化失败的兜底引擎（不接命令，只给 UI 一句错误）。
static FALLBACK: OnceLock<SpeechEngine> = OnceLock::new();

/// 初始化语音引擎（进程内一次；重复调用保留首次的实例）。
///
/// `rime_user_dir` 是 rime 用户目录（`%APPDATA%\Xime\rime`），模型目录按
/// 与安卓一致的约定取它的同级 `models/<id>/`（见 [`crate::models`]）。
pub fn init(rime_user_dir: &Path) -> &'static SpeechEngine {
    let _ = ENGINE.set(SpeechEngine::spawn(rime_user_dir));
    // OnceLock 刚 set 过；取不到只可能是并发 set 失败——退回一个不接命令的空
    // 引擎，让 UI 显示「未初始化」而不是 panic。
    ENGINE
        .get()
        .unwrap_or_else(|| FALLBACK.get_or_init(SpeechEngine::detached))
}

/// 进程级引擎（未初始化时返回 None）。
fn engine() -> Option<&'static SpeechEngine> {
    ENGINE.get()
}

/// 引擎实现（命令与状态访问器：面板 🎙️ 页 [`crate::ui::panel::VoiceView`] 消费）。
impl SpeechEngine {
    fn spawn(rime_user_dir: &Path) -> Self {
        let profile = AsrModelRegistry::default_profile();
        let model_dir = crate::models::model_dir(rime_user_dir, &profile.id);
        // 数据根 = rime 用户目录的上级（与 plugins.rs 读 clipboard_sync.toml 同一约定）。
        let data_root = rime_user_dir
            .parent()
            .unwrap_or(rime_user_dir)
            .to_path_buf();
        let sink = Arc::new(Mutex::new(SpeechSink::default()));
        let (tx, rx) = std::sync::mpsc::channel();

        let worker_sink = Arc::clone(&sink);
        let worker_model_dir = model_dir.clone();
        let worker_profile = profile.clone();
        let worker_data_root = data_root.clone();
        let spawned = std::thread::Builder::new()
            .name("xime-speech".into())
            .spawn(move || {
                let mut worker = Worker {
                    data_root: worker_data_root,
                    model_dir: worker_model_dir,
                    profile: worker_profile,
                    recognizer: None,
                    sink: worker_sink,
                    shutdown: false,
                    deferred: Vec::new(),
                    preview: false,
                    level_envelope: LevelEnvelope::default(),
                };
                worker.run(&rx);
            });
        if let Err(e) = spawned {
            // 线程起不来时退化：命令发不出去，UI 显示错误。
            tracing::error!("语音工作线程启动失败: {e}");
            let mut guard = sink.lock().unwrap_or_else(|p| p.into_inner());
            guard.error = Some(format!("语音工作线程启动失败: {e}"));
        }

        Self {
            tx,
            sink,
            data_root,
        }
    }

    /// 不接命令的空引擎（仅用于极端初始化失败路径）。
    fn detached() -> Self {
        let (tx, _rx) = std::sync::mpsc::channel();
        Self {
            tx,
            sink: Arc::new(Mutex::new(SpeechSink {
                error: Some("语音引擎未初始化".to_string()),
                ..Default::default()
            })),
            data_root: PathBuf::new(),
        }
    }

    fn send(&self, command: Command) {
        if self.tx.send(command).is_err() {
            tracing::warn!("语音命令发送失败（工作线程已退出）");
        }
    }

    /// 命令：预装载模型（不开麦克风）。
    pub fn warmup(&self) {
        self.send(Command::Warmup);
    }

    /// 命令：开始识别。
    pub fn start(&self) {
        self.send(Command::Start);
    }

    /// 命令：结束并上屏。
    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    /// 命令：开始试听（设置页用；识别结果不上屏）。
    pub fn start_preview(&self) {
        self.send(Command::StartPreview);
    }

    /// 命令：放弃本次会话（不上屏）。
    pub fn cancel(&self) {
        self.send(Command::Cancel);
    }

    /// 命令：退出工作线程（进程收尾用）。
    pub fn shutdown(&self) {
        self.send(Command::Shutdown);
    }

    /// 当前状态快照。
    pub fn snapshot(&self) -> SpeechSink {
        match self.sink.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 选中模型的文件是否齐（UI 进入语音页时问一次）。
    ///
    /// **不缓存**：用户在设置程序里换了模型 / 下了新模型，下次进页面就该看到
    /// 新状态，不必重启 server（读一次 toml + 四次文件属性，代价可忽略）。
    pub fn model_ready(&self) -> bool {
        settings::selected_model_ready(&self.data_root)
    }

    /// 当前选中的模型（按设置文件实时解析）。
    pub fn selected_profile(&self) -> AsrModelProfile {
        settings::selected_profile(&self.data_root)
    }

    /// 模型列表（设置页用）。
    pub fn catalog(&self) -> Vec<download::ModelInfo> {
        download::catalog(&self.data_root)
    }

    /// 命令：切换选中模型（同步持久化 + 通知工作线程丢弃旧识别器）。
    ///
    /// 只接受注册表里有的 id：设置页给的选项本来就来自注册表，未知 id 多半是
    /// 手改配置或跨端同步来的脏数据，直接拒绝比默默回退好排查。
    pub fn select_model(&self, model_id: &str) -> Result<(), String> {
        let profile = AsrModelRegistry::find_by_id(model_id)
            .ok_or_else(|| format!("未知模型：{model_id}"))?;
        settings::save(
            &self.data_root,
            &settings::SpeechSettings {
                model_id: profile.id.clone(),
            },
        )?;
        self.send(Command::SelectModel(profile.id));
        Ok(())
    }

    /// 命令：删除模型目录（异步；结果看列表刷新与错误行）。
    pub fn delete_model(&self, model_id: &str) -> Result<(), String> {
        AsrModelRegistry::find_by_id(model_id)
            .ok_or_else(|| format!("未知模型：{model_id}"))?;
        self.send(Command::DeleteModel(model_id.to_string()));
        Ok(())
    }

    /// 命令：开始下载模型（异步线程；同一时刻一个）。
    ///
    /// 会话进行中拒绝下载：模型文件可能正被识别器占着，Windows 下写同名文件会失败。
    pub fn start_download(&self, model_id: &str) -> Result<(), String> {
        if self.snapshot().state != SpeechState::Idle {
            return Err("正在识别，结束后再下载模型".to_string());
        }
        download::start_download(self.data_root.clone(), model_id.to_string())
    }

    /// 引擎状态快照（进程级；未初始化返回默认值）。
    pub fn global_snapshot() -> SpeechSink {
        engine().map(SpeechEngine::snapshot).unwrap_or_default()
    }

    /// 模型是否就绪（进程级）。
    pub fn global_model_ready() -> bool {
        engine().map(SpeechEngine::model_ready).unwrap_or(false)
    }

    /// 预装载（进程级）。
    pub fn global_warmup() {
        if let Some(engine) = engine() {
            engine.warmup();
        }
    }

    /// 开始识别（进程级）。
    pub fn global_start() {
        if let Some(engine) = engine() {
            engine.start();
        }
    }

    /// 开始试听（进程级；不上屏）。
    pub fn global_start_preview() {
        if let Some(engine) = engine() {
            engine.start_preview();
        }
    }

    /// 结束并上屏（进程级）。
    pub fn global_stop() {
        if let Some(engine) = engine() {
            engine.stop();
        }
    }

    /// 放弃本次会话（进程级）。
    pub fn global_cancel() {
        if let Some(engine) = engine() {
            engine.cancel();
        }
    }

    /// 退出工作线程（进程级）。
    pub fn global_shutdown() {
        if let Some(engine) = engine() {
            engine.shutdown();
        }
    }

    /// 模型列表（进程级；未初始化返回空列表）。
    pub fn global_catalog() -> Vec<download::ModelInfo> {
        engine().map(SpeechEngine::catalog).unwrap_or_default()
    }

    /// 当前选中的模型（进程级；未初始化按默认模型回答——设置页仍能显示名字）。
    pub fn global_selected_profile() -> AsrModelProfile {
        engine()
            .map(SpeechEngine::selected_profile)
            .unwrap_or_else(AsrModelRegistry::default_profile)
    }

    /// 切换选中模型（进程级）。
    pub fn global_select_model(model_id: &str) -> Result<(), String> {
        match engine() {
            Some(engine) => engine.select_model(model_id),
            None => Err("语音引擎未初始化".to_string()),
        }
    }

    /// 删除模型（进程级）。
    pub fn global_delete_model(model_id: &str) -> Result<(), String> {
        match engine() {
            Some(engine) => engine.delete_model(model_id),
            None => Err("语音引擎未初始化".to_string()),
        }
    }

    /// 下载模型（进程级）。
    pub fn global_start_download(model_id: &str) -> Result<(), String> {
        match engine() {
            Some(engine) => engine.start_download(model_id),
            None => Err("语音引擎未初始化".to_string()),
        }
    }
}

/// 工作线程：独占识别器，串行处理命令。
struct Worker {
    /// 数据根（`%APPDATA%\Xime`）：每次开会话前按设置解析选中模型。
    data_root: PathBuf,
    model_dir: PathBuf,
    profile: AsrModelProfile,
    recognizer: Option<StreamingRecognizer>,
    sink: Arc<Mutex<SpeechSink>>,
    /// 会话中途收到 Shutdown：会话结束后线程也要退出。
    shutdown: bool,
    /// 会话中收到、但必须等会话结束才能执行的命令（切模型 / 删模型）。
    /// 会话循环会把它们收进来，`run` 在会话收尾后补做——不能直接丢，
    /// 丢了用户的「删除」就变成静默无效。
    deferred: Vec<Command>,
    /// 本次会话是否为试听（设置页）：结束时不长屏、不弹通知。
    preview: bool,
    /// 输入电平包络（跨块保留，面板 🎙️ 页的电平条用它）。
    level_envelope: LevelEnvelope,
}

/// CUDA 分包要的 CUDA 运行库是否就绪。
///
/// **硬依赖**（`dumpbin /dependents onnxruntime_providers_cuda.dll` 实测）：
/// `cublasLt64_13.dll` + `cublas64_13.dll`（CUDA 13 运行库的 cuBLAS）——
/// 缺了 `onnxruntime.dll` 建 CUDA 会话时**直接 abort 进程**
/// （C++ 异常穿过 FFI 边界，实测 `STATUS_STACK_BUFFER_OVERRUN`，Rust 兜不住）。
///
/// **cuDNN 9 不是 imports 里的依赖**（该 DLL 既无静态导入也无 delay-load；
/// 只有我们不用的 TensorRT EP 才导入 `cudnn64_9.dll`），但 sherpa-onnx 官方
/// CUDA 包按「CUDA 13 + cuDNN 9」发布，部分算子可能动态找 cuDNN——所以
/// 把它当**保守条件**：缺了不冒险建 GPU 会话，回退 CPU。
#[cfg(feature = "speech-cuda")]
fn cuda_runtime_ready() -> bool {
    const HARD: [&str; 2] = ["cublasLt64_13.dll", "cublas64_13.dll"];
    let missing_hard: Vec<&str> = HARD.iter().copied().filter(|n| !dll_available(n)).collect();
    if !missing_hard.is_empty() {
        tracing::warn!(
            "未找到 CUDA 运行库（cuBLAS）：{} —— 语音识别用 CPU（缺这个会让进程直接崩，不冒险）",
            missing_hard.join(", ")
        );
        return false;
    }
    if !dll_available("cudnn64_9.dll") {
        tracing::warn!("未找到 cuDNN 9（保守条件，非硬依赖）——语音识别用 CPU");
        return false;
    }
    true
}

/// 系统加载器能否找到该 DLL（exe 目录 → System32 → PATH）；找到即释放句柄。
#[cfg(feature = "speech-cuda")]
fn dll_available(name: &str) -> bool {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::LoadLibraryW;
    let wide = windows_core::HSTRING::from(name);
    match unsafe { LoadLibraryW(&wide) } {
        Ok(handle) => {
            unsafe {
                let _ = FreeLibrary(handle);
            }
            true
        }
        Err(_) => false,
    }
}

impl Worker {
    /// 命令循环（返回即线程结束）。
    fn run(&mut self, rx: &Receiver<Command>) {
        // 采集走 WASAPI，需要 COM；工作线程固定 MTA，避免和 UI 的 STA 纠缠。
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        loop {
            let command = match rx.recv() {
                Ok(command) => command,
                Err(_) => break,
            };
            match command {
                Command::Warmup => self.warmup(),
                Command::Start => {
                    self.preview = false;
                    self.run_session(rx);
                    if self.shutdown {
                        break;
                    }
                }
                Command::StartPreview => {
                    self.preview = true;
                    self.run_session(rx);
                    if self.shutdown {
                        break;
                    }
                }
                Command::Stop | Command::Cancel => {
                    // 会话外的 Stop/Cancel：没有会话可结束，忽略。
                }
                Command::SelectModel(model_id) => self.select_model(&model_id),
                Command::DeleteModel(model_id) => self.delete_model(&model_id),
                Command::Shutdown => break,
            }
            // 会话里攒下的模型管理命令，会话结束后补做。
            while let Some(command) = self.deferred.pop() {
                match command {
                    Command::SelectModel(model_id) => self.select_model(&model_id),
                    Command::DeleteModel(model_id) => self.delete_model(&model_id),
                    _ => {}
                }
            }
        }
        tracing::info!("语音工作线程退出");
    }

    /// 按设置文件重新解析选中模型（设置程序可能刚改过；开会话前调一次）。
    fn reload_selection(&mut self) {
        let profile = settings::selected_profile(&self.data_root);
        if profile != self.profile {
            tracing::info!(
                "语音模型切换：{} → {}",
                self.profile.id,
                profile.id
            );
            // 换模型就作废旧识别器（下次开会话按新模型装载）。
            self.recognizer = None;
        }
        self.model_dir = crate::models::model_dir(&self.data_root.join("rime"), &profile.id);
        self.profile = profile;
    }

    /// 切换选中模型：设置已由引擎侧持久化，这里只让识别器跟着换。
    fn select_model(&mut self, _model_id: &str) {
        self.recognizer = None;
        self.reload_selection();
    }

    /// 删除模型目录；删掉的正好是选中模型时清空选择（回默认模型），
    /// 避免语音卡在「模型未下载」而用户看不出该怎么办。
    fn delete_model(&mut self, model_id: &str) {
        if model_id == self.profile.id {
            self.recognizer = None;
        }
        match crate::models::delete_model(&self.data_root.join("rime"), model_id) {
            Ok(()) => {
                tracing::info!("语音模型已删除：{model_id}");
                if settings::load(&self.data_root).model_id == model_id {
                    if let Err(e) =
                        settings::save(&self.data_root, &settings::SpeechSettings::default())
                    {
                        tracing::warn!("清空选中模型失败：{e}");
                    }
                }
                self.reload_selection();
                download::bump_rev();
            }
            Err(e) => {
                tracing::warn!("删除语音模型失败（{model_id}）：{e}");
                download::set_last_error(e);
            }
        }
    }

    fn update_sink(&self, update: impl FnOnce(&mut SpeechSink)) {
        match self.sink.lock() {
            Ok(mut guard) => update(&mut guard),
            Err(poisoned) => update(&mut poisoned.into_inner()),
        }
    }

    fn set_state(&self, state: SpeechState) {
        self.update_sink(|sink| sink.state = state);
    }

    fn fail(&self, message: &str) {
        tracing::warn!("语音会话失败: {message}");
        self.update_sink(|sink| {
            sink.error = Some(message.to_string());
            sink.state = SpeechState::Idle;
        });
    }

    /// 预装载：只建识别器，不碰麦克风。
    fn warmup(&mut self) {
        // 进语音页预装载：先按设置解析选中模型（可能刚在设置里换过）。
        self.reload_selection();
        if self.recognizer.is_some() {
            return;
        }
        if !self.files_exist() {
            self.update_sink(|sink| sink.error = Some(MODEL_MISSING.to_string()));
            return;
        }
        self.set_state(SpeechState::Loading);
        match self.build_recognizer() {
            Ok(recognizer) => {
                tracing::info!("语音模型已预装载: {}", self.model_dir.display());
                self.recognizer = Some(recognizer);
                self.update_sink(|sink| sink.error = None);
            }
            Err(e) => self.fail(&e),
        }
        self.set_state(SpeechState::Idle);
    }

    fn files_exist(&self) -> bool {
        let names = [
            &self.profile.encoder_file,
            &self.profile.decoder_file,
            &self.profile.joiner_file,
            &self.profile.tokens_file,
        ];
        names.iter().all(|name| {
            std::fs::metadata(self.model_dir.join(name))
                .map(|meta| meta.is_file() && meta.len() > 0)
                .unwrap_or(false)
        })
    }

    /// 建识别器：CUDA 分包优先 GPU，创建失败回退 CPU。
    fn build_recognizer(&self) -> Result<StreamingRecognizer, String> {
        if let Some(recognizer) = self.try_cuda() {
            return Ok(recognizer);
        }
        let config = SpeechConfig {
            num_threads: NUM_THREADS,
            ..SpeechConfig::default()
        };
        StreamingRecognizer::open(&self.profile, &self.model_dir, &config)
            .map_err(|e| format!("语音模型装载失败: {e}"))
    }

    /// CUDA 分包（`speech-cuda` feature）下尝试 GPU；CPU 分包恒返回 None。
    ///
    /// 先探 CUDA 运行库再建会话：onnxruntime 的 CUDA EP 缺 DLL 时**直接 abort
    /// 进程**（C++ 异常穿过 FFI 边界，Rust 兜不住，实测 exit 0xc0000409），
    /// 所以「创建失败回退 CPU」在这里救不了场——必须提前探到。
    #[cfg(feature = "speech-cuda")]
    fn try_cuda(&self) -> Option<StreamingRecognizer> {
        if !cuda_runtime_ready() {
            // 缺哪个库由 cuda_runtime_ready 自己 warn（含硬依赖 / 保守条件之分）。
            return None;
        }
        let config = SpeechConfig {
            num_threads: NUM_THREADS,
            provider: SpeechProvider::Cuda,
        };
        match StreamingRecognizer::open(&self.profile, &self.model_dir, &config) {
            Ok(recognizer) => {
                tracing::info!("语音识别使用 CUDA（GPU）");
                Some(recognizer)
            }
            Err(e) => {
                tracing::warn!("CUDA 会话创建失败，回退 CPU：{e}");
                None
            }
        }
    }

    #[cfg(not(feature = "speech-cuda"))]
    fn try_cuda(&self) -> Option<StreamingRecognizer> {
        None
    }

    /// 一次会话：打开麦克风 → 识别 → 端点或 Stop 结束 → 上屏。
    fn run_session(&mut self, rx: &Receiver<Command>) {
        // 开会话前按设置重新解析模型：设置程序里刚换的模型立刻生效。
        self.reload_selection();
        if !self.files_exist() {
            self.fail(MODEL_MISSING);
            return;
        }
        self.set_state(SpeechState::Loading);
        if self.recognizer.is_none() {
            match self.build_recognizer() {
                Ok(recognizer) => self.recognizer = Some(recognizer),
                Err(e) => {
                    self.fail(&e);
                    return;
                }
            }
        }
        if let Some(recognizer) = self.recognizer.as_mut() {
            // 清掉上一会话的残留（finalize 后流已 EOF，reset 重建流）。
            recognizer.reset();
        }

        let capture = match capture::Capture::open() {
            Ok(capture) => capture,
            Err(e) => {
                self.fail(&e);
                return;
            }
        };
        let sample_rate = capture.sample_rate();
        tracing::info!("语音会话开始（采集 {} Hz）", sample_rate);
        self.update_sink(|sink| {
            sink.state = SpeechState::Listening;
            sink.partial.clear();
            sink.error = None;
        });

        loop {
            match rx.try_recv() {
                Ok(Command::Stop) => {
                    self.finish_session();
                    break;
                }
                Ok(Command::Cancel) => {
                    tracing::info!("语音会话取消（不上屏）");
                    self.update_sink(|sink| sink.partial.clear());
                    break;
                }
                Ok(Command::Shutdown) => {
                    tracing::info!("语音会话中收到退出命令");
                    self.shutdown = true;
                    break;
                }
                Ok(Command::Warmup | Command::Start | Command::StartPreview) => {}
                // 模型管理命令不能现在做（正在用模型 / 正在占麦克风）：
                // 收进 deferred，会话收尾后由 run 补做。
                Ok(command @ (Command::SelectModel(_) | Command::DeleteModel(_))) => {
                    self.deferred.push(command);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }

            match capture.read_chunk() {
                Ok(chunk) if chunk.is_empty() => std::thread::sleep(POLL_INTERVAL),
                Ok(chunk) => {
                    let (partial, endpoint) = match self.recognizer.as_mut() {
                        Some(recognizer) => {
                            recognizer.accept_pcm32f(sample_rate, &chunk);
                            (recognizer.partial_text(), recognizer.is_endpoint())
                        }
                        None => break,
                    };
                    // 输入电平：UI 要看得出「它在听我说话」。包络（快起慢落）在
                    // 这里做，而不是让面板自己带平滑状态——面板逐帧只读内存。
                    let level = self.level_envelope.next(&chunk);
                    self.update_sink(|sink| {
                        sink.partial = partial;
                        sink.level = level;
                    });
                    if endpoint {
                        tracing::info!("语音端点检测：停顿结束会话");
                        self.finish_session();
                        break;
                    }
                }
                Err(e) => {
                    self.fail(&e);
                    break;
                }
            }
        }
        // 会话收尾：电平条归零（否则面板会停在最后一格音量上，看着像还在听）。
        self.level_envelope.reset();
        self.update_sink(|sink| sink.level = 0);
        self.set_state(SpeechState::Idle);
    }

    /// 结束会话：取最终文本 → 上屏（失败退化为复制）→ 通知提示。
    ///
    /// 试听会话（`preview`）到此为止：只把最终文本留在快照里给设置页显示，
    /// **不上屏也不弹通知**——用户在设置页试听，不该往他光标处塞字。
    fn finish_session(&mut self) {
        let text = self
            .recognizer
            .as_mut()
            .map(StreamingRecognizer::finalize)
            .unwrap_or_default();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            tracing::info!("语音会话结束：无识别结果");
            self.update_sink(|sink| sink.partial.clear());
            return;
        }
        if self.preview {
            tracing::info!("语音试听结束：{} 字（不上屏）", trimmed.chars().count());
            let text = trimmed.to_string();
            self.update_sink(|sink| sink.partial = text);
            return;
        }
        let committed = crate::paste::request_commit(trimmed);
        let copied = !committed && crate::clipboard::write_text(trimmed);
        let chars = trimmed.chars().count();
        let body = if committed {
            format!("已上屏（{} 字）", chars)
        } else if copied {
            "上屏失败，已复制到剪贴板，按 Ctrl+V 粘贴".to_string()
        } else {
            "上屏失败：剪贴板被其他程序占用，请重试".to_string()
        };
        tracing::info!("语音会话结束：{} 字，上屏={}", chars, committed);
        crate::toast::show_toast("曦码·曜输入法", &body);
        let text = trimmed.to_string();
        self.update_sink(|sink| {
            sink.partial = text.clone();
            sink.last_commit = Some(text);
        });
    }
}

/// 模型未下载时的统一提示（设置程序里下载，见 P3）。
const MODEL_MISSING: &str = "语音模型未下载";

/// 推理后端说明（设置页展示：本包到底能不能用 GPU）。
///
/// 只说明**分包能力**：CUDA 分包在运行库缺失时会自动回退 CPU（日志里有 warn），
/// 实际用的是哪个，看日志比看这里准——不在设置页假装成运行时事实。
pub fn provider_label() -> String {
    let version = xime_speech::sherpa_version();
    #[cfg(feature = "speech-cuda")]
    {
        format!("GPU 优先（CUDA 分包）· sherpa-onnx {version}；缺 CUDA 运行库时回退 CPU")
    }
    #[cfg(not(feature = "speech-cuda"))]
    {
        format!("CPU 推理（CPU 分包）· sherpa-onnx {version}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 输入电平：静音贴地、说话满格、块内能量越高越大。
    #[test]
    fn level_maps_silence_to_zero_and_speech_to_full() {
        let mut envelope = LevelEnvelope::default();
        // 静音（全 0）与底噪（-60dB 级）都必须是 0，不能把电平条顶起来。
        assert_eq!(envelope.next(&[0.0; 480]), 0);
        assert_eq!(envelope.next(&[0.001; 480]), 0);
        // 正常说话音量（±0.3 满幅正弦的 RMS≈0.21）落在门限内 → 明显非零。
        let speech: Vec<f32> = (0..480)
            .map(|i| 0.3 * (i as f32 * 0.1).sin())
            .collect();
        let loud = envelope.next(&speech);
        assert!((60..=100).contains(&loud), "说话音量应到 6 成以上: {loud}");
        // 满幅 → 满格（封顶 100）。
        assert_eq!(envelope.next(&[1.0; 480]), 100);
        // 空块不能 panic、不能 NaN。
        envelope.reset();
        assert_eq!(envelope.next(&[]), 0);
    }

    /// 快起慢落：静音块只衰减一格，不会瞬间贴地。
    #[test]
    fn level_envelope_decays_gradually_and_resets() {
        let mut envelope = LevelEnvelope::default();
        let full = envelope.next(&[1.0; 480]);
        assert_eq!(full, 100);
        let decayed = envelope.next(&[0.0; 480]);
        assert!(decayed > 0 && decayed < full, "静音应衰减但不归零: {decayed}");
        // 连续静音最终落到底。
        let mut last = decayed;
        for _ in 0..20 {
            last = envelope.next(&[0.0; 480]);
        }
        assert_eq!(last, 0);
        // 会话结束 reset：下一会话从 0 起，不继承上次的尾巴。
        let _ = envelope.next(&[1.0; 480]);
        envelope.reset();
        assert_eq!(envelope.next(&[0.0; 480]), 0);
    }

    /// 快照读写在 UI 线程/工作线程之间共享，锁中毒也不能 panic。
    #[test]
    fn sink_snapshot_survives_poisoned_lock() {
        let sink = Arc::new(Mutex::new(SpeechSink::default()));
        let clone = Arc::clone(&sink);
        let _ = std::thread::spawn(move || {
            let _guard = clone.lock().unwrap();
            panic!("故意中毒");
        })
        .join();

        let engine = SpeechEngine {
            tx: std::sync::mpsc::channel().0,
            sink: Arc::clone(&sink),
            data_root: PathBuf::new(),
        };
        assert_eq!(engine.snapshot().state, SpeechState::Idle);
        assert!(!engine.model_ready(), "空模型目录不应被视为已就绪");
    }

    /// 模型就绪判定：**选中模型**的四个文件齐且非空。
    ///
    /// 测试目录放在仓库 target/ 下（`%TEMP%` 在某些受限环境里拒绝创建目录，
    /// 那是 baseline 里 models/schema_switches 测试失败的原因，新测试不跟着踩）。
    #[test]
    fn model_ready_requires_all_four_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join("speech_model_ready");
        let _ = std::fs::remove_dir_all(&root);
        let rime_user_dir = root.join("rime");
        assert!(
            std::fs::create_dir_all(&rime_user_dir).is_ok(),
            "测试目录应可创建：{}",
            rime_user_dir.display()
        );
        let profile = AsrModelRegistry::default_profile();
        let engine = SpeechEngine {
            tx: std::sync::mpsc::channel().0,
            sink: Arc::new(Mutex::new(SpeechSink::default())),
            data_root: root.clone(),
        };
        assert!(!engine.model_ready());

        let model_dir =
            crate::models::ensure_model_dir(&rime_user_dir, &profile.id).expect("建模型目录");
        for name in [
            &profile.encoder_file,
            &profile.decoder_file,
            &profile.joiner_file,
        ] {
            assert!(
                std::fs::write(model_dir.join(name), b"x").is_ok(),
                "应能写入 {name}"
            );
        }
        assert!(!engine.model_ready(), "缺 tokens.txt 不应就绪");

        assert!(
            std::fs::write(model_dir.join(&profile.tokens_file), b"").is_ok(),
            "应能写入空词表"
        );
        assert!(!engine.model_ready(), "空文件不应就绪");

        assert!(
            std::fs::write(model_dir.join(&profile.tokens_file), b"tokens").is_ok(),
            "应能写入词表"
        );
        assert!(engine.model_ready());

        let _ = std::fs::remove_dir_all(&root);
    }
}