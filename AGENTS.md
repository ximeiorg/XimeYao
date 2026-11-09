# 曦码·曜 (Xime Yao) 五笔输入法 - Windows 版

## §1 项目简介

基于 rime 框架的 Windows 五笔输入法：Rust + TSF，workspace 含 5 个 crate，librime 为子模块。许可 GPL-3.0-or-later。

## §2 文件地图

| 文件 | 用途 | 上限（行） |
| --- | --- | --- |
| AGENTS.md | 入口：底线、红线、地图 | 90 |
| WORKFLOW.md | 打卡、质量门、证据、三档、PROGRESS/DECISIONS 写法 | 130 |
| RULES.md | 规则写法、引用、机检 | 80 |
| VISUAL.md | 候选栏/托盘/设置页视觉纪律 | 50 |
| TEAM.md | 多人协作与复核 | 90 |
| PROGRESS.md | 当前状态/已完成/未决/验证记录/下一步 | 150 |
| DECISIONS.md | D-nnn 决策记录 | 单条 50 |
| docs/verify/ | 验证证据与历史归档 | 不限 |

## §3 硬性规则（违反即失败）

- 所有命令用 PowerShell。
- 改完跑 `cargo build --quiet`；收尾跑 `cargo clippy --workspace --all-targets`（0 warning）与 `cargo test --workspace`。
- 代码禁 `unwrap()` / `expect()`；错误必须显式处理。
- 禁止自己运行程序、禁止自己安装；实机验证交给用户，我只给步骤与预期。
- `libximecore/librime` 只读；唯一例外是构建输入（插件目录联接、build/dist 产物）。
- 未经用户明确指示，不 commit / push。
- 文档改动跑 `node scripts/doccheck.mjs` 与 `node scripts/refcheck.mjs`，双绿才提交。

## §4 工作规则

- 一次只做一个功能点；端到端验证通过再开下一个。
- 不做功能 A 时顺手重构 B；要重构先写 `D-nnn`。
- 需要时补单元测试；修 bug 必须带回归用例或可复现步骤。
- 用户可见文案说清「发生了什么 / 为什么 / 下一步能做什么」，禁止只写「操作失败」。
- 事实优先级（高→低）：产品事实（代码/实测/用户确认）> `D-nnn` > 一手实测记录 > 上游规范 > 对标项目实测 > 第三方文档 > 模型记忆；记忆不得单独作依据，不确定就写「未验证」。

## §5 打卡

- 上班：读 `PROGRESS.md` 与 `DECISIONS.md` 最近 3 条 → `git status --short` → `cargo build --quiet` → 从「下一步」继续；先写一句 S0（目标 + 验收）。
- 下班：更新 PROGRESS 与 `docs/verify/` → build（必要时 clippy/test）→ 双机检 → 汇报 → 用户同意后提交。细则见 `WORKFLOW.md §1`、`WORKFLOW.md §7`。

## §6 常用命令

- 开发构建：`.\rebuild.ps1`（管理员；构建 + 按安装布局暂存 + MSIX 开发注册 + 启动 server）
- 打包 MSI：`.\msi-build.ps1` → `target\wix\ximeyao-{version}-x86_64.msi`
- 安装测试：`msiexec /i target\wix\ximeyao-{version}-x86_64.msi`
- 完全卸载：`.\full-uninstall.ps1`
- 版本号：`Cargo.toml` 的 `workspace.package.version`，打包脚本自动读取

## §7 临时产物

- 一次性脚本、日志、截图放 `target/tmp/`，用完清理；仓库根目录不留临时文件。
- 机检与打包产生的中间文件不得加入版本库。
