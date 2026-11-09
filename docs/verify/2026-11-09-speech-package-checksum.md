# 语音模型包完整性校验（P3b / 审计 F4 后半）

日期：2026-11-09

## 背景

`AsrModelProfile` 的下载地址是 ModelScope 的 `resolve/master/...`：master 是可变分支，
仓库所有者可以用同名文件换内容，URL 一个字节都不变。P3a 只管住了「包里最多能有多大」，
没管住「这个包是不是我们要的那一个」。本批把内置 SHA-256 做成安装闸门。

## 改动

| 位置 | 改动 |
| --- | --- |
| `libximecore/crates/xime-speech/src/registry.rs` | 新增 `ModelChecksum { sha256, size_bytes }`；`AsrModelProfile` 增加 `checksum: Option<ModelChecksum>` 并标记 `#[non_exhaustive]`；两个内置模型填入实测摘要；`profile_or_default` 回退时清空 `checksum` 与 `download_url`（回退结果只用于装载） |
| `libximecore/crates/xime-speech/src/lib.rs` | 导出 `ModelChecksum` |
| `crates/winxime-server/src/hashing.rs`（新增） | 从 `plugins.rs` 提出 `sha256_hex`；新增流式 `Sha256Writer<W>`（边写边算，`finish()` 取内层与摘要） |
| `crates/winxime-server/src/main.rs` | 注册 `mod hashing;` |
| `crates/winxime-server/src/plugins.rs` | 改用 `crate::hashing::sha256_hex`，删掉本地副本 |
| `crates/winxime-server/src/models.rs` | `ensure_model_dir` 改为测试辅助（线上改走暂存目录，不再直接建目标目录） |
| `crates/winxime-server/src/speech/download.rs` | 下载时同步算摘要并返回 `(字节数, 摘要)`；新增 `verify_package`（无摘要拒绝 / 字节不符 / 摘要不符三档文案）；解压改到暂存目录，成功后 `replace_model_dir` 整体替换，失败保留原模型 |

校验顺序固定为「下载 → 校验 → 解压 → 替换」：摘要不符的字节一个都不进解压器，
已有 `models/<id>/` 一个字节都不动。

## 摘要来源（发布事实）

2026-11-09 从 ModelScope 文件 API 取值，并各自完整下载一次实算核对，两者一致：

| 模型 id | 字节数 | SHA-256 |
| --- | --- | --- |
| `sherpa-onnx-x-asr-480ms-streaming-zipformer-transducer-zh-en-punct-int8` | 133,895,136 | `fa5f63d618e5a01526e275a358bb7772e403f84808a4769fba52cffd8160bf74` |
| `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30` | 132,634,597 | `5a2832047ea1f97dd0dc595b816c230c4bafad65cfc0341fa57517cadc50afd0` |

改这两个值等于改发布事实，必须走 `D-nnn`。

## 机检

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 / rustc 1.98.0-nightly | `cargo test -p xime-speech` | 12 passed | 注册表摘要格式、回退清空、既有布局断言 |
| 同上 | `cargo test -p winxime-server` | 158 passed，1 ignored | 含本批 13 条新单测 |
| 同上 | `cargo test --workspace` | 164 passed，1 ignored，0 failed | 全量回归 |
| 同上 | `cargo clippy --workspace --all-targets` | 0 warning（仅第三方 `aes_gcm` 弃用） | 静态检查 |

本批新增单测（13 条）：

- `registry::tests::builtin_profiles_carry_checksum_anchor`
- `registry::tests::fallback_profile_has_no_checksum_or_download_url`
- `hashing::tests::{empty_input_matches_known_digest, known_message_matches_known_digest, streaming_writer_matches_one_shot_digest, streaming_writer_passes_body_through}`
- `download::tests::verify_package_{accepts_matching_digest, rejects_missing_checksum, rejects_size_mismatch, rejects_digest_mismatch_without_claiming_tampering}`
- `download::tests::{stream_to_file_hashes_exactly_what_it_writes, replace_model_dir_swaps_in_place_and_cleans_old_copy, replace_model_dir_moves_staging_when_target_absent}`

## 未做 / 已知限制

- **rev pin 未落地**：`Cargo.toml` 的 8 个 libximecore git 依赖仍无 `rev`。本批改了
  `xime-speech` 的公开结构体，`rev` 必须指向「包含该字段的提交」；该提交目前只存在于
  本机（`bf3b9ae`，本地提交未 push），需先推到 `ximeiorg/libximecore` 才能钉住，否则干净检出会编译失败。发布前必须补上
  （见 `D-005` 遗留）。
- 存量模型（升级前已下载的）没有基线可校验，本批不做 TOFU 清单；首次加载不做完整性检查。
- 只校验归档，不校验解压后的四件套；解压到磁盘后被本地改写的情况不在防护范围。
- 替换成功后删旧目录失败时只记 `warn`，会在 `models/` 下留 `.xime-model-old-*`（不影响加载，属磁盘残留）。
- 真实包端到端下载未跑（交给用户，步骤见下）。

## 实机验证步骤

1. 管理员 PowerShell 跑 `.\rebuild.ps1`。
2. 设置页 → 语音 → 下载默认模型（`x-asr`）：进度到 100%、状态变「已下载」。
3. 再下载另一个模型（`zipformer-zh-int8`），同样成功。
4. 把配置里已选模型切到旧模型再切回，听写一次，确认模型目录未被替换流程弄坏。

失败分支（摘要不符）没有可安全触发的线上手段，靠单测覆盖；若要实机复现，可临时把
`verify_package` 里的 `expected.sha256` 改一个字符，下载应在解压前失败且 `models/<id>/`
保持原样。
