# 2026-11-10 仓库地址对齐 ximeiorg/XimeYao

- 背景：项目在 GitHub 上由 `winxime` 改名为 `XimeYao`，旧地址靠 GitHub 重定向仍能访问；但版本库里的 `repository` 与关于页「项目主页」仍指着旧名。
- 改动：`Cargo.toml` 的 `repository`、`crates/winxime-server/src/main.rs` 与 `crates/winxime-setup/src/main.rs` 的 `AppMetadata.homepage` 统一为 `https://github.com/ximeiorg/XimeYao`。`README.md` 没有仓库链接，不改；本地 `git remote upstream` 也指向新地址（只影响本机，不进版本库）。
- 关于页 UI：本仓没有改。页面结构来自 libximecore 上游（`7737db3` 重构为 hero / info / license 三张卡），宿主只注入元数据；「项目主页」按钮读的就是 `homepage`，因此随本次改动指向新地址。

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly | `git ls-remote https://github.com/ximeiorg/XimeYao.git`（走本机代理） | `09ab350a70fb443b299d5fb7d7455976739c83be  refs/heads/main` | 新地址可达 |
| 同上 | `git ls-remote https://github.com/ximeiorg/winxime.git` | 同一 HEAD（GitHub 重定向） | 旧地址仍可用 |
| 同上 | 改完 `cargo build --quiet`（`CC=CXX=clang-cl`） | exit 0，无输出 | Windows x64 |
| 同上 | `node scripts/doccheck.mjs`、`node scripts/refcheck.mjs` | 双绿 | 文档 |

实机验证：设置页「关于」→「项目主页」应打开 `https://github.com/ximeiorg/XimeYao`（需在浏览器里跳到新地址）。回退：三处字符串改回旧地址即可。
