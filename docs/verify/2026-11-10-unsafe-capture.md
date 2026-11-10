# 语音采集 unsafe 专项（P5 批次 3）

> 对应审计 F7。本批处理 `crates/winxime-server/src/speech/capture.rs` 的 12 处 `unsafe`：
> 全部补 `// SAFETY:` 注释，并给 `parse_mix_format` 写明 `# Safety` 前置条件。不改逻辑。

## 位置与理由

| 行（改前） | 位置 | SAFETY 依赖的事实 |
| --- | --- | --- |
| :61 | `Capture::open` 整块 | 调用线程已初始化 COM；接口指针由 windows crate 智能指针持有；`GetMixFormat` 内存在本块内释放 |
| :118 | `GetNextPacketSize` | `capture` 来自成功的 `GetService`，`&self` 保证存活 |
| :129 | `GetBuffer` | 每轮只取一个包；`data` 有效期到 `ReleaseBuffer` |
| :143 | `from_raw_parts` | `data` 已检查非空；只读 `frames × 声道 × 字节宽` 范围内的字节 |
| :147 | `ReleaseBuffer` | 与本轮 `GetBuffer` 严格配对，`frames` 取自同一缓冲 |
| :159 | `Drop` 里的 `Stop` | `client` 只在 `open` 成功后存在；`Stop` 失败被忽略 |
| :169 | `unsafe fn parse_mix_format` | 新增 `# Safety`：指针非空且指向可读的 `WAVEFORMATEX` |
| :173 | `&*ptr` | 非空已在上一行检查；结构体在调用期间不移动 |
| :276/:288/:291/:298 | 测试内的 `parse_mix_format` 调用 | 指向栈上结构体（或刻意传空指针验证拒绝路径） |

## 不变量

- `GetBuffer` 与 `ReleaseBuffer` 在同一轮 `read_chunk` 内成对出现，`data` 只在两者之间被读取。
- 静音包（`AUDCLNT_BUFFERFLAGS_SILENT`）与空指针都走零样本分支，不解引用。
- 样本字节数由 `SampleFormat::bytes_per_sample` 与设备声道数相乘得到，与 `parse_mix_format` 接受的白名单一致。

## 机检

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 / cargo 1.98.0-nightly | `cargo build --quiet`（`CC=CXX=clang-cl`） | 退出码 0 | 注释与 `# Safety` 不破坏编译 |
| 同上 | `cargo clippy --workspace --all-targets` | 退出码 0；仅 libximecore `aes_gcm` 弃用与 LNK4104 | 无新增 lint |
| 同上 | `cargo test --workspace` | 1 + 158（1 ignored）+ 6 = 165 passed | 混音格式解析与降混未变 |

## 已知限制

- `Capture::open`、`read_chunk`、`Drop` 依赖真实音频设备，没有自动化测试；需实机按住麦克风说一句验证。
- 共享模式混音格式只接受 f32 / i16（与 `parse_mix_format` 白名单一致），遇到 24bit 会明确报错而不是按错误步长解读。
- 采集线程的 COM 初始化在 `speech/mod.rs` 的工作线程里，本文件不负责；若以后改线程模型需同步复核 `# Safety` 前提。
