# 2026-11-09 规则体系接入验证

> 范围：规则文件、PROGRESS/DECISIONS 改造、docs/verify、doccheck/refcheck、CI docs job。本轮不改功能代码。

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 26100 / rustc 1.98.0-nightly | `cargo build --quiet` | `build exit=0`（日志 `target/tmp/build.log`） | workspace 编译 |
| 同上 | `cargo clippy --workspace --all-targets` | `Finished dev profile ... clippy exit=0`；仅 1 行构建脚本提示（librime-sys2 复制 rime.dll），无 lint 警告 | 0 warning |
| 同上 | `cargo test --workspace` | `1 passed` / `131 passed; 1 ignored` / `6 passed`；合计 138 passed、1 ignored、0 failed，退出码 0（日志 `target/tmp/test-baseline.log`） | 单元测试 |
| Node v24.22.1 | `node scripts/doccheck.mjs` | `doccheck: 通过`，exit 0；负面测试：临时注入行号引用后 exit 1，还原后通过 | 文档结构 |
| Node v24.22.1 | `node scripts/refcheck.mjs` | `refcheck: 通过（9 个文件）`，exit 0 | 引用锚点 |
| GitHub Actions | `docs` job（ubuntu-latest / node 20） | push `763df7a` 触发，`Docs: success`；`Build` job 未跟进（用户指示暂不处理） | CI 文档门禁 |

- 未验证：MSI/MSIX 打包、实机安装与输入法行为未在本轮复跑。
- 肉眼复核：本轮无 UI 改动，不适用。
