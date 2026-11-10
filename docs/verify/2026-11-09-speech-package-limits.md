# 2026-11-09 语音模型包下载/解压限额（审计 F4 / 计划 P3a）

改动文件：`crates/winxime-server/src/speech/download.rs`。

- 下载上限 `MAX_MODEL_DOWNLOAD_BYTES` = 512 MiB：
  - `Content-Length` 声明超限 → 一个字节都不下（`ensure_declared_size`）；
  - 实际写入字节超限 → `stream_to_file` 中途中止；半截包由 `download_and_extract` 统一删除（原有逻辑）。
- 解压上限（`ExtractLimits`）：单文件 512 MiB、合计 1 GiB。tar 条目先按头部声明查一遍（不建空壳文件），
  写入后按实际字节再查一遍（声明值可以撒谎）。
- 解压中途失败会删掉本次已写出的文件，避免「半套模型」被 `catalog` 当成已下载。
- `extract_tar_bz2` 拆成「开 bz2 解码器」+ `extract_tar<R: Read>`，后者与压缩格式无关，单测可直接喂未压缩 tar 流。
- 说明：注册表来自编译内置（非远程可控），但下载源在网络另一端，包大小仍按不可信输入处理。

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly | 设 `CC=CXX=C:\Program Files\LLVM\bin\clang-cl.exe`、`PATH` 前置 LLVM，跑 `cargo build --quiet` | exit 0 | Windows x64 |
| 同上 | `cargo test --workspace` | exit 0；1 + 147（1 ignored）+ 6 = 154 passed、1 ignored、0 failed | Windows x64 |
| 同上 | `cargo test -p winxime-server -- extract_ declared_size stream_to_file` | 5 passed | 新增单测 |
| 同上 | `cargo clippy --workspace --all-targets` | exit 0；本仓代码 0 warning（仅第三方 `aes_gcm` 弃用提示） | Windows x64 |

新增单测（`speech/download.rs` 的 `mod tests`）：

- `declared_size_over_cap_is_rejected`：声明超限直接拒绝；无 `Content-Length` 不拦。
- `stream_to_file_aborts_when_bytes_exceed_cap`：实际字节超限中途中止。
- `extract_rejects_oversized_entry_declaration`：单文件声明超限，目标文件都不建。
- `extract_rejects_total_over_limit_and_cleans_partial_files`：合计超限，先写出的文件被删干净。
- `extract_flattens_nested_paths`：只取文件名铺平，不复现包内目录。

未做：sha256 校验需要注册表带摘要（`libximecore` 的 `xime-speech/src/registry.rs`），属计划 P3b，需专家评审 + 用户同意后才动。
