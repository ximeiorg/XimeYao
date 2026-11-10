# 候选窗与 TSF 注册工具 unsafe 专项（P5 批次 4）

> 对应审计 F7 的收尾批次。覆盖两个文件、24 处 `unsafe`（`ui/view.rs` 16 处、
> `winxime-tsf-register/src/main.rs` 8 处）。只加注释与 `# Safety` 文档，不改逻辑。

## 候选窗 `ui/view.rs`

| 位置 | 形式 | 依赖的不变量 |
| --- | --- | --- |
| `RenderedView::new` | `// SAFETY` | `user_data` 指向 `Box<CandidateWindow>` 内对象，生命周期覆盖窗口；UI 线程创建 |
| `create_swapchain_bitmap` | `# Safety` | UI 线程；`swapchain` 与 `target` 同属一个视图（D2D 工厂单线程）；调用前无未完成绘制 |
| `wnd_proc` | `# Safety` | 只由系统回调；`hwnd` 是已注册窗口；各消息的指针所有权见分支注释 |
| `WM_NCCREATE` 三个块 | `// SAFETY` | `CREATESTRUCTW` 回调期间有效且已判空；`GWLP_USERDATA` 只由 UI 线程写 |
| `WM_SHOW_ROOT` 三个块 | `// SAFETY` | `RootModel` 由 `Box::into_raw` 转移，消息投递即移交；`this_ptr` 来自窗口用户数据，销毁前有效 |
| `get_dpi_for_window` / `get_dpi_for_point` / `clamp_point_to_monitor` | `// SAFETY` | 参数按值传入，输出缓冲在栈上，API 失败走默认值 |
| `enter_voice_page` / `leave_voice_page` / `toggle_voice` / `relayout_and_repaint` | `# Safety` | 只能在 UI 线程调用，`hwnd` 必须是本候选栏窗口 |

`wnd_proc` 的函数体本身是 `unsafe fn`，其中的裸指针解引用（如 `WM_UPDATE_CANDIDATE`
里的 `Box::from_raw(ctx_ptr)`、`(*this_ptr).model.replace(…)`）没有独立 `unsafe` 块；
这些调用的前提下移到窗口过程的 `# Safety` 与所在分支的注释里，不再逐句重复。

## TSF 注册工具 `winxime-tsf-register/src/main.rs`

| 位置 | 依赖的不变量 |
| --- | --- |
| `remove_registry_keys` | 根键为系统预定义句柄；键名是 `w!` 生成的 NUL 结尾静态宽串 |
| `register_dll` / `unregister_dll` | 路径转 NUL 结尾 UTF-16 且在本块内存活；`LoadLibraryW` 保持 DLL 加载；导出符号签名由 COM 约定，`transmute` 只换 ABI 相同的函数指针 |
| `register` / `unregister` | COM 已在 `main` 里按 STA 初始化；CLSID/GUID 为常量；名称与图标宽串存活到调用返回 |
| `enable` / `disable` | `XIME_TIP_DESC` 是 NUL 结尾静态宽串常量 |
| `main` | 先挂父控制台、再初始化 COM；失败只影响 COM 路径，参数分发不受影响 |

## 机检

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 / cargo 1.98.0-nightly | `cargo build --quiet`（`CC=CXX=clang-cl`） | 退出码 0 | 注释与 `# Safety` 不破坏编译 |
| 同上 | `cargo clippy --workspace --all-targets` | 退出码 0；仅 libximecore `aes_gcm` 弃用与 LNK4104 | 无新增 lint |
| 同上 | `cargo test --workspace` | 1 + 158（1 ignored）+ 6 = 165 passed | 逻辑未变 |

## 已知限制

- 候选窗与 TSF 注册都是 GUI / 安装路径，没有自动化测试；候选栏显隐、面板、语音页与
  `-install` / `-uninstall` 仍需实机走一遍。
- `wnd_proc` 里未加独立 `unsafe` 块的裸指针调用，安全性靠窗口过程的调用约定与
  消息发送方约定维持；若以后新增消息类型，需要按同一格式补分支注释。
- `XIME_TIP_DESC` 等宽串常量由 `w!` 宏生成，若改成运行时拼接，`enabled`/`disable`
  前的常量前提要重新确认。
