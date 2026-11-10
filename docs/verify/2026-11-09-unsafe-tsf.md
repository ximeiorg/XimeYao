# TSF 回调 unsafe 注释专项（P5 批次 1）

> 对应审计 F7：全仓约 121 处 `unsafe` 集中在 TSF 回调、托盘、语音采集、UI 绘制。
> 本批只处理 `crates/winxime-tsf/src/text_input_processor.rs`（27 处），只补
> 「指针来源 / 生命周期 / 失败路径」注释，不改任何调用与返回值。

## 改动

| 位置 | 改动 | 说明 |
| --- | --- | --- |
| `text_input_processor.rs` | 26 个 `// SAFETY:` 注释块覆盖 27 处 `unsafe` | 相邻的 `clsid` / `guidprofile` 两个 `as_ref` 共用一块；其余一处一块 |

按职责分四组：

| 组 | 处数 | 主要前提 |
| --- | --- | --- |
| 组词与编辑会话（`DoEditSession`、`start/update/end_composition`、`RequestEditSession`） | 10 | 接口来自 `context.cast` 或 `self.composition`，调用期间本地持有；`ec` 是 TSF 传入的会话 cookie；out-param 都先检查 `Result` |
| 语言栏与事件槽（`Advise*/Unadvise*`、`AddItem/RemoveItem`、`OnUpdate`） | 13 | 接口在作用域内存活、TSF 会 `AddRef`；cookie 记录在 `self`，解绑后立刻置 `TF_INVALID_COOKIE`，不会重复解绑 |
| 焦点与裸指针入参（`OnActivated` 的 `clsid`/`guidprofile`、`OnSetFocus` 的 `GetTop`） | 3 | 指针由 TSF 在回调栈上提供，只用 `as_ref` 做本次调用内的借用，不保存 |
| 模块路径日志（`GetModuleFileNameW`） | 1 | 栈上 `[u16; 512]`，返回长度用 `min(buf.len())` 夹紧后才切片 |

## 机检

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 / cargo 1.98.0-nightly | `cargo build --quiet`（`CC=CXX=clang-cl`） | 退出码 0 | 注释不改变编译结果 |
| 同上 | `cargo clippy --workspace --all-targets` | 退出码 0；仅 libximecore `aes_gcm` 弃用告警与链接器 LNK4104 | 无新增 lint |
| 同上 | `cargo test --workspace` | 1 + 158（1 ignored）+ 6 = 165 passed | 行为未变 |

## 依赖的外部约定

- `ITfContext::GetSelection` 失败时不写 out 参数，因此失败分支不需要 `ManuallyDrop::into_inner`。
  若上游违约写了 range，会泄漏一个 `ITfRange` 引用（只泄漏、不悬垂）。要兜住这点就得在失败分支
  也消费一次数组，属于行为微调，留到后续批次决定。
- `AdviseSink` / `AdviseKeyEventSink` 成功后 TSF 自己 `AddRef`；本地接口变量在作用域末尾释放的是
  我们自己那一份引用，不影响 TSF 持有的引用。

## 未做

- 其余模块：托盘（`tray.rs` 20 处）、语音采集（`speech/capture.rs` 11 处）、UI 绘制（`ui/view.rs` 10 处）、
  `tsf-register/main.rs` 8 处，按批次 2–4 处理。
- 没有给 `unsafe fn` 或 `unsafe impl` 增加额外防御；本批不改变任何运行行为。
