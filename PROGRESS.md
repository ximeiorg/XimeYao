# 进度（PROGRESS）

> 写法与上限见 `WORKFLOW.md §5`；本文件合计 ≤150 行。历史原文见 [completed-legacy-progress.md](docs/verify/completed-legacy-progress.md)。

## 接手指南

- 当前阶段：按修复计划推进审计整改；P1a（IPC 管道 ACL）、P2（备份恢复加固）、P3a（语音包限额）、P3b（语音包摘要校验）已落地并本地提交（`2b57fac`、`8284a4b`、`eac49f0`、`6aeed27`，未 push）。
- 下一个功能点：P3b 的 rev pin（等 libximecore `bf3b9ae` 推到 `ximeiorg` 后钉住）与 P1b（词典路径白名单，待批准）；开工前按 `WORKFLOW.md §2` 写 S0。
- 阻断项：本机 cargo 直接编译 C 依赖会报 `D8050`，构建时需 `CC=CXX=C:\Program Files\LLVM\bin\clang-cl.exe` 且 `PATH` 前置 LLVM（见审计文档附录）；CI 不受影响。
- 30 秒读法：本文件 → `DECISIONS.md` 最近 3 条 → `WORKFLOW.md §1`。

## 当前状态

- 版本：0.3.2（`Cargo.toml` 的 `workspace.package.version`）
- 分支：`main`，本地领先 `origin/main`（`763df7a`）两个提交：`1bf9913`、`b43a7da`；其后 P1a/P2/P3a/P3b 与本轮文档共 4 个本地提交（`2b57fac`→`6aeed27`），未 push
- 环境：Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly / node v24.22.1
- 基线：build / clippy / test 三项 2026-11-09 实测通过（164 passed、1 ignored、0 failed；含 P1a 4 条、P2 7 条、P3a 5 条、P3b 13 条）
- 依赖：libximecore 已拉至 `25aa1c8`（含 P2P 剪贴板与设置页）并加了 `xime-speech` 摘要字段（本地提交 `bf3b9ae`，未 push）；librime 子模块仍 `d71168e`
- 焦点：P1a、P2、P3a、P3b 与 libximecore 摘要字段均已本地提交，全部待实机验证；P1b 需用户批准后才动 `libximecore` 的 `xime-setup`；P3b 的 rev pin 卡在 `bf3b9ae` 尚未推送到 `ximeiorg/libximecore`。

## 进行中

- P1a（U-5 前半）：管道 ACL 收紧为「当前用户 SID + SYSTEM」，单测与三项机检已过；等用户实机验证。
- P2（U-6）：`restore_backup` 改为「三重限额 + 暂存目录 + 失败回滚」，顺带修 `_xime_backup/` 跳过判定在 Windows 上失效的 bug；功能尚未接线，待发布前端到端验证。
- P3a（U-7 前半）：模型包下载 512 MiB 硬上限、解压单文件 512 MiB / 合计 1 GiB，失败清理半成品；已机检，待发布前用真实包验证一次。
- P1b（U-5 后半）：词典导出/导入/恢复的路径白名单。会动 `winxime-ipc` 协议与 `libximecore` 的 `xime-setup`（导出路径改由服务端生成），按规则需用户同意；用户 2026-11-09 表示暂不做。
- P3b（U-7 后半）：注册表加 `ModelChecksum`，下载流式算摘要、先校验后解压、暂存后整体替换；专家评审已过、用户已批准范围，机检通过。剩 rev pin（等 core 合入）与真实包端到端。
- P5（F7）批次 1：`text_input_processor.rs` 27 处 `unsafe` 补 26 块 SAFETY 注释（仅注释，不改行为）。批次 2：`tray.rs` 21 处补注释，并把三个跨线程入口的句柄与中英状态改为原子量（`D-006`）。余下语音采集、UI 绘制、tsf-register 按批次 3–4 继续。

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
| 2026-11-09 | 本批改动本地提交（未 push）：`2b57fac` IPC、`8284a4b` 插件、`eac49f0` 语音、`6aeed27` 文档；libximecore 摘要字段 `bf3b9ae` | `git 2b57fac..6aeed27`、`core bf3b9ae` |
| 2026-11-09 | P5 批次 1：TSF 回调 27 处 `unsafe` 补 26 块 SAFETY 注释（仅注释） | [2026-11-09-unsafe-tsf.md](docs/verify/2026-11-09-unsafe-tsf.md) |
| 2026-11-10 | P5 批次 2：托盘 21 处 `unsafe` 补注释；跨线程句柄与中英状态改原子量（`D-006`） | [2026-11-10-unsafe-tray.md](docs/verify/2026-11-10-unsafe-tray.md) |
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
| U-5b | 导出/导入/恢复词典接受客户端任意路径（审计 F2） | 待批准 | 服务端生成导出路径；导入/恢复只收白名单内真实文件 | 需同意后改 `winxime-ipc` 与 `libximecore` 的 `xime-setup` |
| U-6 | 插件备份恢复解压无体积上限、无回滚、目录错误被吞（审计 F3） | 已修（未接线；`8284a4b`） | 三重限额 + 暂存目录 + 失败回滚（`D-003`） | 接线后拿真实备份包做端到端恢复 |
| U-7a | 语音模型包无体积上限、失败留半成品（审计 F4） | 已修（`eac49f0`） | 下载 512 MiB、解压单文件 512 MiB / 合计 1 GiB、失败清理 | 发布前用真实模型包下载一次 |
| U-7b | 语音模型包无 sha256 校验（审计 F4） | 已修（`eac49f0` + core `bf3b9ae`；rev pin 待推送） | 注册表加 `ModelChecksum`，下载流式校验、暂存后替换（`D-005`） | 合入 `ximeiorg/libximecore` 后钉 `rev`，再用真实包下载一次 |

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

## 下一步

- P3b 收尾：把 libximecore 的 `bf3b9ae` 推到 `ximeiorg/libximecore`，再给 8 个依赖钉 `rev`；之后用真实模型包走一次下载。
- P1b 等用户点头（要动 `libximecore` 的 `xime-setup` 与 IPC 协议）。
- P5 批次 3：语音采集（`speech/capture.rs` 12 处），再 UI 绘制与 `tsf-register`。
- P1a / P2 / P3a 待实机：P1a 按档案 4 步；P2 接线后拿真实备份包；P3a、P3b 用真实模型包下载一次。
- 本机构建先设 `CC=CXX=clang-cl` 与 LLVM `PATH`，再跑 `cargo build --quiet`。
