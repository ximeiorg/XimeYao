# 词典导出 / 导入 / 恢复的路径边界（P1b，审计 F2）

- 日期：2026-11-10
- 范围：`winxime-ipc` 协议、`winxime-server` 的词典命令、`winxime-setup` 宿主回调、`xime-setup` 的结果类型
- 结论：机检通过；设置页三种操作待实机按下方步骤验证

## 改了什么

| 位置 | 改动 |
| --- | --- |
| `crates/winxime-ipc/src/messages.rs` | `ExportUserDict` 的 `data` 由 `UserDictFile(dict, path)` 改为 `UserDict(dict)`；`DictResponse` 新增 `exported_path` 与 `error`（都带 `#[serde(default)]`，旧客户端不受影响） |
| `crates/winxime-server/src/dict_paths.rs`（新） | `export_path()` 在系统临时目录生成唯一文件名；`validate_input_path()` 只放行「用户数据目录」或「临时目录」下的真实普通文件 |
| `crates/winxime-server/src/ipc_server.rs` | 三个命令接入白名单：Restore / Import 先校验再调 librime；Export 用服务端生成的路径，成功回传 `exported_path`、失败删临时文件；失败原因放进 `DictResponse.error` |
| `crates/winxime-ipc/src/pipe.rs` | `restore_user_dict` / `export_user_dict` / `import_user_dict` 改为 `Result`，把服务端的可读原因带出来 |
| `crates/winxime-setup/src/main.rs` | 宿主先把用户选的文件复制进临时目录再发 IPC（`stage_dict_file`）；导出则读服务端临时文件、复制到用户选的目标位置 |
| `libximecore` `crates/xime-setup/src/state.rs` | 三个词典回调改为返回 `Result`，失败时把原因拼进提示文案 |

## 机检

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 / rustc 1.98.0-nightly | `cargo build --quiet`（`CC=CXX=clang-cl`） | exit 0，仅既有 aes_gcm 弃用与 LNK4104 告警 | 两仓联合编译 |
| Windows 11 | `cargo clippy --workspace --all-targets` | exit 0，本仓 0 warning | 全部 crate |
| Windows 11 | `cargo test --workspace` | 1 + 165（1 ignored）+ 6 = 172 passed，0 failed | 全部测试 |
| Windows 11 | `cargo test -p winxime-server -- dict_paths` | 7 passed | 路径白名单 |
| Windows 11 / node v24.22.1 | `node scripts/doccheck.mjs`、`node scripts/refcheck.mjs` | 双绿 | 文档规则与引用 |

## 白名单单测

- `accepts_file_inside_allowed_root`：根目录内的普通文件被接受，返回 canonicalize 后的路径。
- `rejects_file_outside_allowed_roots`：根目录外的文件被拒，原因含「已拒绝」。
- `rejects_missing_file`：不存在的路径被拒，原因含「不存在」。
- `rejects_directory_instead_of_file`：目录被拒，原因含「普通文件」。
- `rejects_empty_path`：空路径被拒，原因含「没有选择文件」。
- `allowed_roots_cover_user_data_and_temp`：白名单根同时覆盖用户数据目录与系统临时目录。
- `export_paths_are_unique_and_end_with_txt`：连续两次导出不撞名、扩展名为 `.txt`。

## 已知限制

- 白名单按「canonicalize 后的前缀」判断，不防同一目录内的符号链接指向别处（指向的外层路径仍会被 canonicalize 拦下）。
- 导出临时文件在客户端复制成功后删除；客户端异常退出时会在 `%TEMP%` 留一份，系统清理临时目录时会一并删除。
- 服务端仍是「一问一答、无请求 id」的协议（审计 F5），本批不拆，随功能点一起改。

## 实机验证步骤

1. 以管理员运行 `.\rebuild.ps1`，确认 server 与设置页都能起来。
2. 设置页 → 词典 → 选一个词典点「导出」，保存到桌面；用记事本打开，应有 `词<TAB>编码` 行。
3. 把导出文件改一行后「导入」，应提示「已导入 …（N 条）」；再故意选一个不存在的路径不可行，改选一个无权限目录里的文件，应提示「文件不存在或无权访问」或「路径不在用户数据目录或临时目录内，已拒绝」。
4. 先用「备份」生成快照，再「从快照恢复」，应提示「已从快照恢复用户词典」。
