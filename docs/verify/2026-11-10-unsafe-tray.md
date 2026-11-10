# 托盘 unsafe 专项（P5 批次 2）

> 对应审计 F7。本批处理 `crates/winxime-server/src/tray.rs` 的 21 处 `unsafe`：
> 20 处补注释，另 3 处（`show_icon` / `hide_icon` / `update_tray_icon`）改为读原子句柄，
> 顺手消掉一处真实的跨线程 `static mut` 读取。

## 改动

| 位置 | 改动 | 说明 |
| --- | --- | --- |
| `tray.rs` 全部 21 处 `unsafe` | 逐一补 `// SAFETY:` | 写明句柄来源、存活期、失败路径 |
| `TRAY_IS_ASCII` | `static mut bool` → `AtomicBool` | IPC 线程写、窗口线程读；原实现是数据竞争 |
| `show_icon` / `hide_icon` / `update_tray_icon` | 句柄改从 `TRAY_HWND_RAW` 读 | 三者由 IPC 连接线程调用，原实现直接读 `static mut TRAY_HWND`，属跨线程 aliasing |
| `do_show_icon` / `do_hide_icon` / `do_update_icon` | 保持读 `TRAY_HWND` | 只在窗口线程（消息循环）执行，与 install / WM_DESTROY / cleanup 同线程 |

行为不变：消息 id、投递时机、图标状态位都没动；`PostMessageW` 失败仍被忽略（窗口已销毁时本来就会失败）。

## 线程模型

| 线程 | 做的事 | 访问的状态 |
| --- | --- | --- |
| 主线程（消息循环） | `install`、`window_proc`、菜单跟踪、`cleanup` | `TRAY_HWND` / `TRAY_MENU` / `TRAY_ON_ACTION` / `TRAY_SWITCH_PROVIDER` 的 `static mut` |
| IPC 连接线程 | `show_icon` / `hide_icon` / `update_tray_icon` / `show_balloon` | 只读 `TRAY_HWND_RAW`，只写 `TRAY_IS_ASCII`（原子量） |

`TRAY_SWITCH_ITEMS` 一直是 `Mutex`，不在本批范围。

## 机检

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 / cargo 1.98.0-nightly | `cargo build --quiet`（`CC=CXX=clang-cl`） | 退出码 0 | 原子量替换与注释不破坏编译 |
| 同上 | `cargo clippy --workspace --all-targets` | 退出码 0；仅 libximecore `aes_gcm` 弃用与 LNK4104 | 无新增 lint |
| 同上 | `cargo test --workspace` | 1 + 158（1 ignored）+ 6 = 165 passed | 行为未变 |

## 已知限制

- 托盘是 GUI 路径，没有自动化测试；显隐、右键菜单、主题切换、气泡仍需实机点一遍。
- `WM_DESTROY` 分支只清 `TRAY_HWND`，`TRAY_HWND_RAW` 留到 `cleanup` 才清零；窗口销毁后
  其他线程的 `PostMessageW` 会失败并被忽略，属预期路径。
- `TRAY_ON_ACTION` 等仍是 `static mut`，历史上只为窗口线程使用；若以后有别的线程调用
  `handle_menu_command`，需要一并改成同步原语。
