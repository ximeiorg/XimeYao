# 2026-11-09 备份恢复加固（审计 F3 / 计划 P2）

改动文件：`crates/winxime-server/src/plugins.rs`。

- `restore_backup` 拆成 `restore_archive`（流程）/ `extract_to_staging`（解压+限额）/ `apply_staged`（替换+回滚），
  另有 `copy_capped`（双重字节上限）、`undo_apply`（回滚）、`side_dir`（临时目录命名）。
- 硬上限：条目 20,000；解压后合计 512 MiB；单文件 128 MiB。预扫描看 zip 中心目录**声明**的大小，
  解压时再按**实际写入字节**复核（声明值可以撒谎）。
- 流程：解压到 rime 目录同级的 `.xime-restore-<pid>-<n>` 暂存目录 → 全部成功后才逐个 `rename` 覆盖目标 →
  被覆盖的旧文件先挪进 `.xime-restore-old-…` 回滚目录 → 任一步失败 `undo_apply` 倒回并清暂存。
  超限包/坏包在碰用户数据之前就被拒绝。
- 顺带修一个真 bug：`_xime_backup/` 元数据的跳过判定原按 `"_xime_backup/"` 字符串前缀比较，
  而 Windows 上 `enclosed_name()` 返回的是 `_xime_backup\meta.json`，判定永远不中、元数据会被写进 rime 目录；
  改为按首个路径组件比较。
- `create_dir_all` 不再 `let _ =` 吞错，失败带路径上报。

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly | 设 `CC=CXX=C:\Program Files\LLVM\bin\clang-cl.exe`、`PATH` 前置 LLVM，跑 `cargo build --quiet` | exit 0，无输出 | Windows x64 |
| 同上 | `cargo test --workspace --quiet` | exit 0；1 + 142（1 ignored）+ 6 = 149 passed、1 ignored、0 failed | Windows x64 |
| 同上 | `cargo test -p winxime-server restore_` | 7 passed | 新增单测 |
| 同上 | `cargo clippy --workspace --all-targets` | exit 0；本仓代码 0 warning（仅第三方 `aes_gcm` 弃用提示） | Windows x64 |

新增单测（`plugins.rs` 的 `mod tests`）：

- `restore_archive_writes_files_and_skips_metadata`：正常落位、元数据跳过、无关文件保留、临时目录清干净。
- `restore_rejects_archive_over_entry_limit` / `_over_total_bytes` / `_single_file_over_limit`：三种超限各自拒绝且不落盘。
- `restore_rejects_archive_without_files`：只有元数据的包报「没有可恢复的文件」。
- `restore_skips_path_traversal_entries`：`../evil.txt` 不越界落盘。
- `restore_failure_rolls_back_replaced_files`：替换阶段失败时已换上的文件回滚成旧内容。

未验证：云备份功能目前**尚未接 IPC 命令与设置页入口**（`plugins.rs` 里仍是 `#[allow(dead_code)]`），
本机无法点出真实恢复流程；接线发布前需用真实备份包做一次端到端恢复。
