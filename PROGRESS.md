# 进度（PROGRESS）

> 写法与上限见 `WORKFLOW.md §5`；本文件合计 ≤150 行。历史原文见 [completed-legacy-progress.md](docs/verify/completed-legacy-progress.md)。

## 接手指南

- 当前阶段：规则体系接入已收口（规则文件 + 机检 + CI），功能代码未改。
- 下一个功能点：待用户指定；开工前按 `WORKFLOW.md §2` 写 S0。
- 阻断项：无。
- 30 秒读法：本文件 → `DECISIONS.md` 最近 3 条 → `WORKFLOW.md §1`。

## 当前状态

- 版本：0.3.2（`Cargo.toml` 的 `workspace.package.version`）
- 分支：`main`，领先 `origin/main` 7 个提交（含 upstream 0.3.2 合并），未 push
- 环境：Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly / node v24.22.1
- 基线：build / clippy / test 三项 2026-11-09 实测通过（138 passed、1 ignored、0 failed）
- 焦点：等用户决定提交与下一个功能点

## 进行中

- 无。上一轮「规则体系接入」已收口，证据见验证记录。

## 已完成

| 日期 | 事项 | 证据 |
| --- | --- | --- |
| 2026-11-09 | 规则体系接入：五份规则文件、PROGRESS 重写、D-nnn 启用、docs/verify、doccheck/refcheck、CI docs job | [2026-11-09-rules-adoption.md](docs/verify/2026-11-09-rules-adoption.md) |
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
| U-1 | 本地 main 领先 origin 7 个提交未 push | 待用户决定 | 登记 | 用户确认后 push |
| U-2 | CI 只有构建，test/clippy 未进流水线 | 已登记 | 评估构建时长后决定 | 先让 docs job 跑通 |
| U-3 | 历史验证证据只在旧 PROGRESS 与提交信息里 | 已登记 | 不追溯回填 | 新功能点按 `WORKFLOW.md §3` 落档 |
| U-4 | DECISIONS 旧条目无 D-nnn | 已裁定 | 不溯改（`D-001`） | 引用旧条目用日期加标题 |

## 验证记录

| 日期 | 主题 | 文件 | 结论 |
| --- | --- | --- | --- |
| 2026-11-09 | 规则体系接入 | [2026-11-09-rules-adoption.md](docs/verify/2026-11-09-rules-adoption.md) | 通过 |

## 下一步

- 用户确认后提交规则接入改动（不 push）。
- 观察 `docs` job 在 CI 的实际耗时，再决定是否把 test/clippy 加进流水线。
- 下一个功能点开工前写 S0，并按 `WORKFLOW.md §2` 过门。
