# libximep2p 拆仓与三仓依赖链验证

- 日期：2026-11-10
- 范围：`libximecore` 同步上游、新仓 `libximep2p`、`XimeYao` 跟随上游后的编译与测试
- 结论：链路上的编译与单测全部通过；P2P 运行时握手未测（按规则不能自行运行程序，留给用户）

## 背景

上游 `libximecore` 在 `ea8e300` 把 `xime-sync-domain`、`xime-p2p`、`xime-p2p-c` 三个 crate
迁到独立仓 [libximep2p](https://github.com/ximeiorg/libximep2p)，并在 `Cargo.toml` 里改为
git 依赖，钉在 `rev = b19ecdec2c0336669897cafe909541906024dc29`。

本机原来的 `libximecore` 停在 `25aa1c8`，且带着两个未推送的本地提交。这次把上游
`b4714f6` 拉下来变基，再验证「XimeYao → libximecore → libximep2p」这条链是否还能编译、
测试是否还绿。

## 三仓状态

| 仓库 | 路径 | HEAD | 说明 |
| --- | --- | --- | --- |
| `libximecore` | `D:\project\libximecore` | `6affcfe` | 变基到上游 `b4714f6` 之上，本地两提交重写为 `b222de2`（ASR 摘要字段）、`6affcfe`（词典回调 Result）；未 push |
| `libximep2p` | `D:\project\libximep2p` | `0865b35` | 新克隆；工作区干净；`libximecore` 钉的是更早的 `b19ecde` |
| `XimeYao` | `D:\project\XimeYao` | 本地领先 `origin/main` 14 个提交 | 本次只改两个 `main.rs` 的 `AppMetadata` 初始化 |

上游这次进来的四个提交：`7737db3` 设置页 UI 重构并支持多产品元数据、`ea8e300` P2P 拆仓、
`5694db5` 剪贴板同步通道重构与自动模式、`b4714f6` 剪贴板存储统一到 `xime-config`。

## 改了什么

| 位置 | 改动 |
| --- | --- |
| `crates/winxime-server/src/main.rs` | `set_app_metadata` 补上游新增的 `settings_name` / `subtitle` / `tagline` / `homepage` 四个字段 |
| `crates/winxime-setup/src/main.rs` | 同上，设置页进程也补四个字段 |
| `crates/winxime-setup/Cargo.toml` | 给 `xime-setup-lib` 加 `pair-page` feature（与 `clipboard-page` 同开），启用「设备关联」页并接入 `libximep2p` |
| `libximecore`（本地） | 两个未推送提交变基到 `b4714f6`；无冲突 |

## 机检

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 / rustc 1.98.0-nightly | `cargo test`（`D:\project\libximep2p`，HEAD `0865b35`） | 75 + 4 + 4 + 22 = 105 passed，0 failed | 新仓全部 crate 与协议向量 |
| Windows 11 / rustc 1.98.0-nightly | `git checkout b19ecde` 后 `cargo test` | 74 + 4 + 4 + 22 = 104 passed，0 failed | `libximecore` 实际钉住的那个 rev |
| Windows 11 | `cargo +nightly tree -p xime-setup-lib --features "clipboard-page pair-page"` | `xime-p2p v0.1.0 (https://github.com/ximeiorg/libximep2p.git?rev=b19ecdec…#b19ecdec)` | 依赖边确实指向新仓 |
| Windows 11 / nightly | `cargo +nightly check -p xime-setup-lib --features "clipboard-page pair-page"` | exit 0 | 上游设置页 + P2P 一起编译 |
| Windows 11 / rustc 1.98.0-nightly | `cargo build --quiet`（`CC=CXX=clang-cl`） | exit 0，仅既有 aes_gcm 弃用与 LNK4104 告警 | XimeYao 全部 crate，含 `pair-page` |
| Windows 11 | `cargo tree -p winxime-setup` | `xime-p2p v0.1.0 (https://github.com/ximeiorg/libximep2p.git?rev=b19ecdec…#b19ecdec)` | 宿主依赖图确实含新仓 |
| Windows 11 | `cargo test --workspace` | 1 + 165（1 ignored）+ 6 = 172 passed，0 failed | 全部测试，与升级前一致 |
| Windows 11 | `cargo clippy --workspace --all-targets` | exit 0，本仓 0 warning | 全部 crate |

## 依赖链细节

- `cargo` 拉 `libximep2p` 走的是 git-over-https，本机需要 `HTTPS_PROXY=http://127.0.0.1:10809`；
  `.cargo/config.toml` 里 `git-fetch-with-cli = true` 会把这个代理传给 `git`。
- `XimeYao` 从本批起**已开** `pair-page`：`winxime-setup` 依赖 `xime-setup-lib` 同时开
  `clipboard-page` / `backup-page` / `voice-page` / `pair-page`，`Cargo.lock` 因此多出
  `libximep2p` 的 git source（钉在 `b19ecde`）。此后构建这台机器需要能拉 GitHub，
  或预热 cargo 的 git 缓存。

## 已知问题

- `xime-setup-lib` 单独开 `pair-page` 编不过：`Message::SyncModeChanged` 变体与
  `SettingsState::sync_plugin` 字段由 `clipboard-page` 门控，但 `app.rs:802` 的匹配臂
  没加 `cfg`。上游 `b4714f6` 上同样存在（本次没碰 `app.rs`），属于 feature 组合缺口；
  本批两个 feature 同开，已验证可编译。若要给上游提，建议在 `Cargo.toml` 写
  `pair-page = ["dep:xime-p2p", "clipboard-page"]`，或在匹配臂上补 `#[cfg]`。
- 运行时 P2P（配对、剪贴板互传）没有测：仓库规则禁止自行运行程序，留给用户按实机步骤验证。
- `libximecore` 钉的 `b19ecde` 落后 `libximep2p` 四个提交（含协议 v2 `82ad8d8`、`0865b35`），
  是否升级要上游决定；本机不擅自改 `rev`。

## 实机验证步骤（需两台设备）

1. 以管理员运行 `.\rebuild.ps1`。构建会用到 `libximep2p` 的 git 依赖，第一次需要能访问
   GitHub（本机走 `HTTPS_PROXY=http://127.0.0.1:10809`）。
2. 打开设置页，侧栏应出现「设备关联」；进入后先确认「本机 → 设备名」可保存、「设备标识」
   显示 32 位短标识（首次进入会生成 `p2p/device.json`）。
3. 两台机器连同一个 Wi-Fi。A 机选「显示配对码」→「生成配对码」，拿到 6 位数字；
   B 机选「输入配对码」填进去 →「配对」。正常应在 60 秒内出「配对成功」，
   「已配对设备」里出现对方。
4. 同上反过来再做一次（A 输入、B 显示），确认两个方向都能配对，且配对表里两台都在。
5. 打开剪贴板页，同步模式选局域网直连，应能看到已配对设备；在 A 机复制一段文本，
   看 B 机剪贴板是否收到。
6. 异常场景：拔掉 Wi-Fi 再点「配对」，应提示查找超时/网络不可达而不是卡死；在
   「已配对设备」里移除一台，确认要二次确认，移除后不再出现在列表。
7. 手动地址兜底：在 B 机展开「手动地址」，填 A 机的 `IP:端口`，确认可配对（用于 AP 隔离）。
