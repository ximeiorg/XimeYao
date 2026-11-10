# 剪贴板局域网直连接入宿主（2026-11-10）

## 背景

设置页「设备关联」（`D-008`）已经能用 xime-p2p 配对、把对端写进
`%APPDATA%\Xime\p2p\peers.json`，剪贴板配置也会把 `channels` 写成
`["p2p"]`；但 Windows 端的剪贴板工作线程只实现了「JS 插件」一条通道，
`channels` 字段根本没读。结果就是：配对成功、设置页显示「设备互传」，
复制的内容却不会跨机，日志里连一条 P2P 记录都没有。

本次把 `xime-p2p` 的 `Node` 接进 `crates/winxime-server/src/plugins.rs`
的剪贴板工作线程，并让 `clipboard_sync.toml` 的 `channels` 真正决定走哪条路。

## 改动

| 文件 | 改动 |
| --- | --- |
| `Cargo.toml` | 新增 workspace 依赖 `xime-p2p`，`rev = b19ecdec…`（与 core 的 pair-page 同一提交） |
| `crates/winxime-server/Cargo.toml` | 引入 `xime-p2p` |
| `crates/winxime-server/src/clipboard_p2p.rs` | 新增：`P2pSync::start/push_text/poll`，`p2p_dir_for(rime_dir)` |
| `crates/winxime-server/src/main.rs` | 注册 `mod clipboard_p2p;` |
| `crates/winxime-server/src/plugins.rs` | `ClipboardSyncSelection` 读 `channels`；按通道起插件/节点；工作线程内投递与轮询；新增 `handle_p2p_event`、`worker_key` |

## 行为

- 选型：`enabled = true` 且 `channels` 含 `"p2p"` 才启动节点；含非 `"p2p"`
  的值则按原逻辑启动同步插件。旧配置没有 `channels` 时沿用「有插件就同步」。
- 去重：沿用工作线程原有的三态（当前内容 / 上次推送 / 自写回显）。远端条目
  写回系统剪贴板会触发一次本地回声，被 `self_written` 抑制。
- 生命周期：节点和插件运行时都归工作线程独占；选型键（插件 id + 是否 P2P）
  变化时停旧线程、起新线程。
- 失败不致命：节点起不来（端口争用、身份文件损坏）只记 warn，剪贴板历史与
  插件通道照常工作。

## 验证

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 | `cargo build --quiet`（CC/CXX=clang-cl） | exit 0，仅第三方 `aes_gcm` 弃用与 LNK4104 既有告警 | 编译 |
| Windows 11 | `cargo clippy --workspace --all-targets` | exit 0，本仓 0 warning | 静态检查 |
| Windows 11 | `cargo test --workspace` | 169 passed / 1 ignored + 6 passed = 175 | 单测 |
| Windows 11 | `cargo test -p winxime-server -- clipboard_selection worker_key p2p_dir` | 4 passed | 本次新增用例 |

新增单测：

- `clipboard_selection_channels_decide_transport`：p2p / 自动 / 插件 / 旧配置 / 关闭五种选型。
- `worker_key_changes_when_transport_changes`：通道变化会触发线程重建。
- `clipboard_p2p::tests::p2p_dir_sits_next_to_rime_dir`、`rooted_rime_dir_keeps_p2p_inside`：P2P 目录与 rime 目录同级。

## 已知限制

- 只同步文本；对端发来的图片/附件条目会记一条日志后跳过（与插件通道一致）。
- 「自动」模式目前是两条通道都发，不判断 P2P 是否真的连上；重复内容由 hash 去重。
- 节点监听 `47821`，端口被占时由 xime-p2p 自动顺延；跨设备要能互通仍需同一
  局域网且防火墙放行（本机已临时关闭三档防火墙做验证）。
- 设置页切换通道后，宿主在下一个轮询节拍内重建线程，不需要重启输入法。

## 待实机

1. `.\rebuild.ps1`（管理员）。
2. 两台机器同一 Wi-Fi：设置页 →「设备关联」→ 双向各配一次。
3. 剪贴板页同步方式选「设备互传」（`channels = ["p2p"]`）。
4. 在 A 机复制一段文字，B 机粘贴；看 `%APPDATA%\Xime\logs\server.log` 是否出现
   `局域网同步节点已启动`、`局域网直连已投递`、`局域网直连收到远端剪贴板`。
