# 进度（PROGRESS）

> 写法与上限见 `WORKFLOW.md §5`；本文件合计 ≤150 行。历史原文见 [completed-legacy-progress.md](docs/verify/completed-legacy-progress.md)。

## 接手指南

- 当前阶段：按修复计划推进审计整改；P1a（IPC 管道 ACL）、P1b（词典路径白名单）、P2（备份恢复加固）、P3a（语音包限额）、P3b（语音包摘要校验）已落地并本地提交（未 push）；剪贴板局域网直连已接进宿主（`D-009`），两台设备实机互传已通过；仓库地址与关于页入口已统一到 `ximeiorg/XimeYao`。
- 下一个功能点：P1b/P1a 等实机验证；P3b 的 rev pin（等 libximecore `b222de2` 推到 `ximeiorg` 后钉住）；之后按 P4/F5 或新计划排。开工前按 `WORKFLOW.md §2` 写 S0。
- 阻断项：本机 cargo 直接编译 C 依赖会报 `D8050`，构建时需 `CC=CXX=C:\Program Files\LLVM\bin\clang-cl.exe` 且 `PATH` 前置 LLVM（见审计文档附录）；CI 不受影响。
- 30 秒读法：本文件 → `DECISIONS.md` 最近 3 条 → `WORKFLOW.md §1`。

## 当前状态

- 版本：0.3.2（`Cargo.toml` 的 `workspace.package.version`）
- 分支：`main` 与 `origin/main`（fork `tiengming/XimeYao`）同步于 `f0fa784`；本轮 17 个提交（`1bf9913` 起）已推送。上游 `ximeiorg/XimeYao`（旧名 `ximeiorg/winxime`，同一 HEAD `09ab350`）需走 PR：`tiengming` 对 `ximeiorg/winxime` 无 push 权限（`git push --dry-run upstream main` 返回 403）
- 环境：Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly / node v24.22.1
- 基线：build / clippy / test 三项 2026-11-10 实测通过（169 passed、1 ignored、0 failed + tsf 6 passed = 175；含 P1a 4 条、P1b 7 条、P2 7 条、P3a 5 条、P3b 13 条、P2P 接线 4 条）
- 依赖：libximecore 已同步到上游 `b4714f6`（P2P 拆仓、设置页 UI 重构、剪贴板存储统一），本地两提交变基为 `b222de2`（`xime-speech` 摘要字段）与 `6affcfe`（词典回调 Result），未 push；新仓 libximep2p 克隆在 `D:\project\libximep2p`（HEAD `0865b35`，core 钉 `b19ecde`）；`winxime-setup` 已开 `pair-page`（`D-008`），本机依赖图里已有 `xime-p2p`；librime 子模块仍 `d71168e`
- 焦点：P1a、P2、P3a、P3b、P5 已本地提交，全部待实机验证；P1b 已按批准范围落地（动了两仓），待实机走一遍设置页；P3b 的 rev pin 卡在 `b222de2` 尚未推送到 `ximeiorg/libximecore`。

## 进行中

- P1a（U-5 前半）：管道 ACL 收紧为「当前用户 SID + SYSTEM」，单测与三项机检已过；等用户实机验证。
- P2（U-6）：`restore_backup` 改为「三重限额 + 暂存目录 + 失败回滚」，顺带修 `_xime_backup/` 跳过判定在 Windows 上失效的 bug；功能尚未接线，待发布前端到端验证。
- P3a（U-7 前半）：模型包下载 512 MiB 硬上限、解压单文件 512 MiB / 合计 1 GiB，失败清理半成品；已机检，待发布前用真实包验证一次。
- P1b（U-5 后半）：词典导出路径改由服务端在临时目录生成、客户端只读；导入/恢复只收「用户数据目录或临时目录」下的真实普通文件，拒绝时回可读原因（`D-007`）。协议、server、setup 宿主与 `xime-setup` 四处已改，机检通过，待实机走设置页三种操作。
- P3b（U-7 后半）：注册表加 `ModelChecksum`，下载流式算摘要、先校验后解压、暂存后整体替换；专家评审已过、用户已批准范围，机检通过。剩 rev pin（等 core 合入）与真实包端到端。
- P5（F7）批次 1：`text_input_processor.rs` 27 处 `unsafe` 补 26 块 SAFETY 注释（仅注释，不改行为）。批次 2：`tray.rs` 21 处补注释，并把三个跨线程入口的句柄与中英状态改为原子量（`D-006`）。批次 3：`speech/capture.rs` 12 处全部补注释，`parse_mix_format` 写明 `# Safety`。批次 4：`ui/view.rs` 16 处与 `winxime-tsf-register` 8 处全部补注释（`unsafe fn` 用 `# Safety` 写明线程/所有权前提）。P5 四批已全部落地，仅剩实机验证。
- 剪贴板局域网直连（`D-009`）：`ClipboardSyncSelection` 读 `channels`，工作线程按选型起 `xime-p2p` 节点；远端文本走宿主原有三态去重后写回剪贴板并记历史。节点起不来只记 warn，插件通道与历史不受影响。机检通过，待两台设备实机互传。

## 已完成

| 日期 | 事项 | 证据 |
| --- | --- | --- |
| 2026-11-09 | 规则体系接入：五份规则文件、PROGRESS 重写、D-nnn 启用、docs/verify、doccheck/refcheck、CI docs job | [2026-11-09-rules-adoption.md](docs/verify/2026-11-09-rules-adoption.md) |
| 2026-11-09 | 规则接入提交并推送 `origin/main`（`763df7a`），CI docs job 通过 | `git 763df7a` |
| 2026-11-09 | 清理过期构建与历史垃圾：`target` 24.8 GB → 33 MB | [2026-11-09-workspace-cleanup.md](docs/verify/2026-11-09-workspace-cleanup.md) |
| 2026-11-09 | libximecore 拉至 `25aa1c8`（P2P 剪贴板、设置页）；Cargo.lock 解开 sha2/toml 版本冲突 | `git 25aa1c8` |
| 2026-11-09 | 静态代码审计第一阶段：IPC 边界、压缩包解压、unsafe 分布 | [2026-11-09-code-audit.md](docs/2026-11-09-code-audit.md) |
| 2026-11-09 | 修复计划落档；收尾本地提交 `1bf9913`、`b43a7da`（未 push） | [2026-11-09-remediation-plan.md](docs/2026-11-09-remediation-plan.md) |
| 2026-11-09 | P1a：IPC 管道 ACL 由 Everyone 收紧为「当前用户 SID + SYSTEM」，含 4 条单测（`2b57fac`） | [2026-11-09-ipc-acl.md](docs/verify/2026-11-09-ipc-acl.md) |
| 2026-11-09 | P2：备份恢复三重限额（20k 条目 / 512 MiB / 128 MiB）、暂存后替换、失败回滚，含 7 条单测（`8284a4b`） | [2026-11-09-backup-restore.md](docs/verify/2026-11-09-backup-restore.md) |
| 2026-11-09 | P3a：模型包下载 512 MiB 硬上限、解压单文件 512 MiB / 合计 1 GiB、失败清理半成品，含 5 条单测（`eac49f0`） | [2026-11-09-speech-package-limits.md](docs/verify/2026-11-09-speech-package-limits.md) |
| 2026-11-09 | P3b：`ModelChecksum` 内置摘要、下载边写边算、先校验后解压、暂存后整体替换，含 13 条单测（`eac49f0`，rev pin 未做） | [2026-11-09-speech-package-checksum.md](docs/verify/2026-11-09-speech-package-checksum.md) |
| 2026-11-09 | 本批改动本地提交（未 push）：`2b57fac` IPC、`8284a4b` 插件、`eac49f0` 语音、`6aeed27` 文档；libximecore 摘要字段（变基后 `b222de2`） | `git 2b57fac..6aeed27`、`core b222de2` |
| 2026-11-09 | P5 批次 1：TSF 回调 27 处 `unsafe` 补 26 块 SAFETY 注释（仅注释） | [2026-11-09-unsafe-tsf.md](docs/verify/2026-11-09-unsafe-tsf.md) |
| 2026-11-10 | P5 批次 2：托盘 21 处 `unsafe` 补注释；跨线程句柄与中英状态改原子量（`D-006`） | [2026-11-10-unsafe-tray.md](docs/verify/2026-11-10-unsafe-tray.md) |
| 2026-11-10 | P5 批次 3：语音采集 12 处 `unsafe` 补注释（仅注释） | [2026-11-10-unsafe-capture.md](docs/verify/2026-11-10-unsafe-capture.md) |
| 2026-11-10 | P5 批次 4：候选窗 16 处、TSF 注册工具 8 处 `unsafe` 补注释（仅注释） | [2026-11-10-unsafe-ui-register.md](docs/verify/2026-11-10-unsafe-ui-register.md) |
| 2026-11-10 | P1b：导出路径服务端生成、导入/恢复白名单校验，失败原因透传到设置页（`D-007`） | [2026-11-10-dict-path-boundary.md](docs/verify/2026-11-10-dict-path-boundary.md) |
| 2026-11-10 | 同步上游：libximecore 变基本地两提交到 `b4714f6`，克隆新仓 libximep2p 并验证「XimeYao → core → p2p」链路；补 `AppMetadata` 新增四字段；启用设置页「设备关联」（`D-008`） | [2026-11-10-libximep2p-chain.md](docs/verify/2026-11-10-libximep2p-chain.md) |
| 2026-11-10 | 剪贴板局域网直连接入宿主剪贴板线程：`channels` 决定走插件还是 xime-p2p（`D-009`）；两台设备实机互传通过 | [2026-11-10-clipboard-p2p-host.md](docs/verify/2026-11-10-clipboard-p2p-host.md) |
| 2026-11-10 | 仓库地址统一到 `ximeiorg/XimeYao`（`Cargo.toml` 与两份 `AppMetadata.homepage`）；关于页 UI 未改，入口随元数据指向新地址 | [2026-11-10-repo-address.md](docs/verify/2026-11-10-repo-address.md) |
| 2026-10-29 | 版本 0.3.2；合并 upstream（删除 builtin-extra，不再分发 librime 默认方案） | `git 09ab350` |
| 2026-10-27 | 托盘 owner 常驻、F1–F23 交还宿主、数字后回车提交 | `git eb9582a` |
| 2026-10-27 | 内置方案 seed 管理、导入方案、删除下载包 | `git 7976083` |
| 2026-10-26 | 候选栏菜单纵横定稿、滚轮翻页 | `git 5593ccd` |
| 2026-10-26 | 内置计算器 B1/B2、候选点选与 hover | `git 9e71da9` |
| 2026-10-23 | CI MSI/MSIX 打包修复 | `git de88157` |

更早记录见 [completed-legacy-progress.md](docs/verify/completed-legacy-progress.md)。

## 未决

| 编号 | 问题 | 状态 | 处置 | 下一步 |
| --- | --- | --- | --- | --- |
| U-2 | CI 只有构建，test/clippy 未进流水线 | 已登记 | 评估构建时长后决定 | 先让 docs job 跑通 |
| U-3 | 历史验证证据只在旧 PROGRESS 与提交信息里 | 已登记 | 不追溯回填 | 新功能点按 `WORKFLOW.md §3` 落档 |
| U-4 | DECISIONS 旧条目无 D-nnn | 已裁定 | 不溯改（`D-001`） | 引用旧条目用日期加标题 |
| U-5a | IPC 管道 ACL 为 Everyone（审计 F1） | 已修（待实机；`2b57fac`） | 当前用户 SID + SYSTEM | 用户跑 `rebuild.ps1` 后按档案 4 步验证 |
| U-5b | 导出/导入/恢复词典接受客户端任意路径（审计 F2） | 已修（待实机） | 服务端生成导出路径；导入/恢复只收白名单内真实文件（`D-007`） | 设置页导出/导入/恢复各走一次 |
| U-6 | 插件备份恢复解压无体积上限、无回滚、目录错误被吞（审计 F3） | 已修（未接线；`8284a4b`） | 三重限额 + 暂存目录 + 失败回滚（`D-003`） | 接线后拿真实备份包做端到端恢复 |
| U-7a | 语音模型包无体积上限、失败留半成品（审计 F4） | 已修（`eac49f0`） | 下载 512 MiB、解压单文件 512 MiB / 合计 1 GiB、失败清理 | 发布前用真实模型包下载一次 |
| U-7b | 语音模型包无 sha256 校验（审计 F4） | 已修（`eac49f0` + core `b222de2`；rev pin 待推送） | 注册表加 `ModelChecksum`，下载流式校验、暂存后替换（`D-005`） | 合入 `ximeiorg/libximecore` 后钉 `rev`，再用真实包下载一次 |

## 验证记录

| 日期 | 主题 | 文件 | 结论 |
| --- | --- | --- | --- |
| 2026-11-09 | 规则体系接入 | [2026-11-09-rules-adoption.md](docs/verify/2026-11-09-rules-adoption.md) | 通过 |
| 2026-11-09 | 工作区清理 | [2026-11-09-workspace-cleanup.md](docs/verify/2026-11-09-workspace-cleanup.md) | 通过（1 个占用文件遗留） |
| 2026-11-09 | 代码审计与构建基线 | [2026-11-09-audit.md](docs/verify/2026-11-09-audit.md) | 通过 |
| 2026-11-09 | IPC 管道 ACL 收紧 | [2026-11-09-ipc-acl.md](docs/verify/2026-11-09-ipc-acl.md) | 机检通过，待实机连通性 |
| 2026-11-09 | 备份恢复加固（F3） | [2026-11-09-backup-restore.md](docs/verify/2026-11-09-backup-restore.md) | 机检通过，待接线后实机 |
| 2026-11-09 | 语音模型包限额（F4 前半） | [2026-11-09-speech-package-limits.md](docs/verify/2026-11-09-speech-package-limits.md) | 机检通过 |
| 2026-11-09 | 语音模型包摘要校验（F4 后半） | [2026-11-09-speech-package-checksum.md](docs/verify/2026-11-09-speech-package-checksum.md) | 机检通过；rev pin 与实机待做 |
| 2026-11-10 | 词典路径边界（F2） | [2026-11-10-dict-path-boundary.md](docs/verify/2026-11-10-dict-path-boundary.md) | 机检通过；设置页待实机 |
| 2026-11-10 | libximep2p 拆仓与三仓链路 | [2026-11-10-libximep2p-chain.md](docs/verify/2026-11-10-libximep2p-chain.md) | 编译与单测通过；运行时 P2P 待实机 |
| 2026-11-10 | 剪贴板局域网直连接线 | [2026-11-10-clipboard-p2p-host.md](docs/verify/2026-11-10-clipboard-p2p-host.md) | 机检通过；两台设备互传实机通过 |
| 2026-11-10 | 仓库地址对齐 | [2026-11-10-repo-address.md](docs/verify/2026-11-10-repo-address.md) | 机检通过；关于页入口待实机点一次 |

## 下一步

- 已开 PR：`ximeiorg/XimeYao#26`（草稿，19 个提交，`tiengming:main` → `main`）与 `ximeiorg/libximecore#6`（`b222de2` 摘要字段 + `6affcfe` 词典回调 Result）。#26 的构建作业依赖 #6 先合入，再在 `Cargo.toml` 把 8 个 core 依赖钉到对应 `rev` 并重跑 CI。
- 关于页实机点一次「项目主页」，确认打开的是 `https://github.com/ximeiorg/XimeYao`。
- 上游同步：`origin` 是 fork，本轮已推送；要把改动送进 `ximeiorg/XimeYao` 得开 PR（本机账号无 push 权限，403）。
- P3b 收尾（暂缓：按要求不推远程）：把 libximecore 的 `b222de2` 推到 `ximeiorg/libximecore` 后再给 8 个依赖钉 `rev`；推送恢复后用真实模型包走一次下载。
- P1b 待实机：`.\rebuild.ps1` → 设置页 → 词典 → 导出/导入/从快照恢复各一次，按档案 4 步看提示。
- P5 收尾：四批注释已落地，待实机走候选栏、语音页与 `winxime-tsf-register -install/-uninstall`。
- P1a / P2 / P3a 待实机：P1a 按档案 4 步；P2 接线后拿真实备份包；P3a、P3b 用真实模型包下载一次。
- 本机构建先设 `CC=CXX=clang-cl` 与 LLVM `PATH`，再跑 `cargo build --quiet`。
- P2P 实机（`D-008`/`D-009`）：两台机器同一 Wi-Fi，设置页 →「设备关联」双向各配一次，剪贴板页选「设备互传」，在 A 机复制、B 机粘贴；日志看 `局域网同步节点已启动`、`局域网直连已投递`、`局域网直连收到远端剪贴板`。步骤见 [2026-11-10-clipboard-p2p-host.md](docs/verify/2026-11-10-clipboard-p2p-host.md) 末节。
