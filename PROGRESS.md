# 进度（PROGRESS）

> 写法与上限见 `WORKFLOW.md §5`；本文件合计 ≤150 行。历史原文见 [completed-legacy-progress.md](docs/verify/completed-legacy-progress.md)。

## 接手指南

- 当前阶段：规则体系已收口；完成静态代码审计第一阶段（只读，未改代码）。
- 下一个功能点：待用户指定；开工前按 `WORKFLOW.md §2` 写 S0。
- 阻断项：本机 cargo 直接编译 C 依赖会报 `D8050`，构建时需 `CC=CXX=C:\Program Files\LLVM\bin\clang-cl.exe` 且 `PATH` 前置 LLVM（见审计文档附录）；CI 不受影响。
- 30 秒读法：本文件 → `DECISIONS.md` 最近 3 条 → `WORKFLOW.md §1`。

## 当前状态

- 版本：0.3.2（`Cargo.toml` 的 `workspace.package.version`）
- 分支：`main`，与 `origin/main` 一致（`763df7a` 已推送）
- 环境：Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly / node v24.22.1
- 基线：build / clippy / test 三项 2026-11-09 实测通过（138 passed、1 ignored、0 failed）
- 依赖：libximecore 已拉至 `25aa1c8`（含 P2P 剪贴板与设置页），librime 子模块仍 `d71168e`
- 焦点：等用户决定审计发现的排期与提交

## 进行中

- 无。上一轮「规则体系接入」已收口，证据见验证记录。

## 已完成

| 日期 | 事项 | 证据 |
| --- | --- | --- |
| 2026-11-09 | 规则体系接入：五份规则文件、PROGRESS 重写、D-nnn 启用、docs/verify、doccheck/refcheck、CI docs job | [2026-11-09-rules-adoption.md](docs/verify/2026-11-09-rules-adoption.md) |
| 2026-11-09 | 规则接入提交并推送 `origin/main`（`763df7a`），CI docs job 通过 | `git 763df7a` |
| 2026-11-09 | 清理过期构建与历史垃圾：`target` 24.8 GB → 33 MB | [2026-11-09-workspace-cleanup.md](docs/verify/2026-11-09-workspace-cleanup.md) |
| 2026-11-09 | libximecore 拉至 `25aa1c8`（P2P 剪贴板、设置页）；Cargo.lock 解开 sha2/toml 版本冲突 | `git 25aa1c8` |
| 2026-11-09 | 静态代码审计第一阶段：IPC 边界、压缩包解压、unsafe 分布 | [2026-11-09-code-audit.md](docs/2026-11-09-code-audit.md) |
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
| U-5 | IPC 管道 ACL 为 Everyone，且导出/导入/恢复词典接受客户端任意路径（审计 F1/F2） | 已登记 | 同一处边界，一起改 | 等用户排期 |
| U-6 | 插件备份恢复解压无体积上限、无回滚、目录错误被吞（审计 F3） | 已登记 | 解压到临时目录后整体替换 | 等用户排期 |
| U-7 | 语音模型包无校验和与体积上限（审计 F4） | 已登记 | 内置 sha256 与大小 | 等用户排期 |

## 验证记录

| 日期 | 主题 | 文件 | 结论 |
| --- | --- | --- | --- |
| 2026-11-09 | 规则体系接入 | [2026-11-09-rules-adoption.md](docs/verify/2026-11-09-rules-adoption.md) | 通过 |
| 2026-11-09 | 工作区清理 | [2026-11-09-workspace-cleanup.md](docs/verify/2026-11-09-workspace-cleanup.md) | 通过（1 个占用文件遗留） |
| 2026-11-09 | 代码审计与构建基线 | [2026-11-09-audit.md](docs/verify/2026-11-09-audit.md) | 通过 |

## 下一步

- 按 [修复计划](docs/2026-11-09-remediation-plan.md) 执行 U-5 → U-6 → U-7（P3b 涉及 `libximecore`，需专家商定与用户同意）；开工前写 S0。
- 本机构建先设 `CC=CXX=clang-cl` 与 LLVM `PATH`，再跑 `cargo build --quiet`。
