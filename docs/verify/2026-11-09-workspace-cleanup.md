# 2026-11-09 工作区清理验证

> 范围：过期构建缓存、CI 调试残留、已合并分支。清理前 `cargo build --quiet` exit 0。

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 26100 / cargo 1.98.0-nightly | `cargo clean --profile dev` | `Removed 31861 files, 20.3GiB total` | target/debug |
| 同上 | `cargo clean --release` | `Removed 7211 files, 3.8GiB total` | target/release |
| 同上 | 删除 target 下 ci 调试目录、doc、lzmatest、msix-pkg.old-*、test-tmp、tmp、ci-*.zip/txt | 逐一删除成功；`msix-pkg` 保留 | 打包与 CI 残留 |
| 同上 | `git branch -d feat/schema-management-import fix/input-calculator-tray-fkeys` | `Deleted branch ... (was 27aef4f)` / `(was 1cb6145)` | 已合并分支 |
| 同上 | `git fetch --prune mine`、`git fetch --prune origin`、`git gc --prune=now` | 通过，gc exit 0 | 远端引用与对象库 |
| 同上 | 清理前后测量 | `target` 24,843.8 MB → 33.2 MB；仓库 61.9 MB | 磁盘占用 |

| 同上 | 管理员进程删除 `target/msix-pkg` | 提权 `powershell.exe -Verb RunAs` 退出码 0；目录删除后 `target` 29.7 MB | 高完整性残留 |

- 已解决：`target/msix-pkg` 两个文件并非进程占用，而是 `msix-bundle.ps1` 把 exe 标为 High 完整性，Medium 会话无法删除；提权后一次删除成功。
- 未验证：清理后未重新构建；下次 `cargo build --quiet` 需完整重建（预计数分钟）。
