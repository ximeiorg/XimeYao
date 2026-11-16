# 2026-11-12 方案包安装/卸载失败与通知-UI 不统一修复

> 主题：市场方案包「装不上 / 卸不掉」与「系统通知和设置页文案不一致」两个问题的根因定位与修复。
> 改动主体在上游 `libximecore`（`xime-config` + `xime-setup`），本仓通过 `.cargo/config.toml` 的 `[patch]` 引入。

## 1. 平台（含版本）

| 项 | 值 |
| --- | --- |
| 系统 | Windows 11 专业版 10.0.26100 |
| 本仓 | XimeYao 0.5.0，`main`（`D:\project\XimeYao`） |
| 上游 | libximecore `main` @ `cda992c`（`D:\project\libximecore`）；本次修复已拆成 3 个 PR 提交到 `ximeiorg/libximecore`（#7 / #8 / #9），本仓通过 `[patch]` 构建 |
| 工具链 | `cargo +nightly-2026-07-03`（stable 1.89 过旧，`ordered-float@5.5.0` 需 1.90、`rfd@0.18.0` 需 1.92） |
| C 编译器 | `CC=CXX=C:\Program Files\LLVM\bin\clang-cl.exe`，LLVM 置于 `PATH` 最前 |

## 2. 日志证据（问题现场）

日志目录 `%APPDATA%\xime\logs\`（`server.log` 45 MB / `setup.log` 207 KB / `tsf.log` 117 MB）2026-11-12 关键行：

```text
00:24:33 通知: 方案部署失败 — rime-wanxiang：文件冲突：README.md（已被 本地方案 使用）。
                 本机同一时刻只保留一个方案包，请先在「已安装」里卸载占用方（或直接用安装确认弹窗卸载后安装）
00:24:46 通知: 方案卸载失败 — Moqi Wan Flypymo：方案包 Moqi Wan Flypymo 未安装
00:24:58 通知: 方案部署失败 — rime-wanxiang：文件冲突：README.md（已被 本地方案 使用）…
00:24:32 setup.log: 卸载方案包 Moqi Wan Flypymo：删除 53 个文件、保留 0 个共享文件、清理 14 个衍生产物
```

同时 `%APPDATA%\xime\market\` 下存在三种历史目录命名（`Moqi Wan Flypymo`、`Moqi Wan Flypymo 1.2.0`、`moqi_wan_flypymo`），
且 `.registry.yaml` 只登记了 `local`（`README.md`、`essay.txt`）——说明卸载其实删了文件，但界面侧的方案包 id 与注册表 key 对不上。

## 3. 根因（代码定位）

| 编号 | 根因 | 位置（上游） |
| --- | --- | --- |
| 1 | 缓存目录按索引 id 命名，但 `scan_downloaded_schema_ids` 把**目录名**当方案 id 返回，界面 id ≠ 索引 id ≠ 注册表 key | `xime-setup/src/state.rs` |
| 2 | 卸载按目录名查注册表 ⇒ 报「方案包未安装」（实际文件已删） | `xime-setup/src/state.rs` + `xime-config/src/schema_manifest.rs` |
| 3 | `local`（用户自放的兜底记录）被当作冲突占用方 ⇒ 市场方案被 README.md 挡下 | `xime-config/src/schema_manifest.rs: detect_conflicts` |
| 4 | `schema_install_conflict` 把注册表里**所有**其它非 local 包都算冲突 ⇒ 装第二个市场包必被要求卸载第一个 | `xime-setup/src/state.rs` |
| 5 | 成功只弹 WinRT toast，失败是 4 秒后自动消失的红字 ⇒ 通知与 UI 两套文案、两套生命周期 | `xime-setup/src/state.rs: expire_install_messages` |
| 6 | 下载临时文件名拼成 `x.zip.part.part.<pid>`，进程被杀即残留半截文件 | `xime-setup/src/state.rs: download_file_with_stats` |
| 7 | 商店页「已安装」只有纯文本，没有卸载入口 | `xime-setup/src/pages/store.rs: schema_action` |

## 4. 修复内容（上游改动）

- **id 归一化**（`xime-config/src/schema_manifest.rs`）：新增 `normalize_package_id`、`same_package_id`、`resolve_package_key`；
  `is_installed` / `uninstall_package` 改用别名查找，`detect_conflicts` 跳过 `local`。
- **规范 id 解析层**（`xime-setup/src/state.rs`）：新增索引缓存（`set_market_schema_index` / `market_schema_display_name`）、
  `canonical_schema_id`（索引 id 归一）、`package_cache_dir`（按归一 id 找真正的缓存目录，兼容三种历史目录名）；
  `scan_downloaded_schema_ids` 返回规范 id；`do_install` / `validate_package_archive` / `delete_market_package` 走该解析。
- **冲突预检**（`schema_install_conflict`）：排除内置方案包、本地方案与自身，只列真正占用的第三方包。
- **通知与 UI 同源**：新增 `MarketNotice { title, body }`，安装/卸载/内置还原/清理的文案在同一处构造，
  `poll_market_task` 同时写 toast 与页面 `install_message`；失败文案按「发生了什么 / 为什么 / 下一步」组织。
- **结果不自动消失**：`expire_install_messages` 只回收模型/插件消息，方案结果常驻页面。
- **下载临时文件**：`x.zip.part.<pid>`（去双重 part），并在索引刷新时清理遗留 `.part` 残留。
- **界面**：商店页已下载/已安装卡片改为「安装 / 卸载」按钮；「已下载」页卡片显示人类可读包名（含 id 兜底）。

## 5. 命令与实际输出

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| 上游 libximecore | `cargo +nightly-2026-07-03 test --quiet -p xime-setup-lib --all-features` | `64 passed; 0 failed; 1 ignored` | id 归一化、冲突过滤、临时文件命名等新回归用例 + 既有设置页用例 |
| 上游 libximecore | `cargo +nightly-2026-07-03 test --quiet -p xime-config --all-features` | `44 passed; 2 failed` | 见第 8 节「预存失败」 |
| 上游 libximecore | `git stash push -- crates/xime-config/src/schema_manifest.rs crates/xime-config/src/lib.rs` 后复测 `cargo +nightly-2026-07-03 test --quiet -p xime-config --all-features` | `39 passed; 1 failed` | 证明 `refresh_builtin_tracks_scheme_files_and_skips_user_data` 失败为预存问题 |
| 本仓 XimeYao | `cargo build --quiet` | 退出码 0，无输出 | 本仓 + patched 上游整体编译 |
| 本仓 XimeYao | `cargo clippy --workspace --all-targets --quiet` | 无 warning 输出 | 0 warning |
| 本仓 XimeYao | `cargo test --workspace --quiet` | `1 passed` / `185 passed; 0 failed; 1 ignored` / `6 passed` 等，全绿 | 本仓全部用例 |
| 上游 libximecore | `cargo +nightly-2026-07-03 clippy --quiet --all-targets --all-features -p xime-config -p xime-setup-lib` | 本次新增代码 0 warning；余下为预存（librime-sys2/build.rs:325、xime-plugin/cipher.rs:108/139、state.rs:393/395/397/3957、theme.rs:55） | 自有改动无 lint 债 |
| 上游 libximecore | `cargo +nightly-2026-07-03 build --quiet -p xime-setup-lib`（**不带任何 feature**） | 退出码 0（此前 E0433/E0599/E0609/E0004 ⇒ PR #9） | 无 feature 编译路径可用 |
| 上游 libximecore | `cargo +nightly-2026-07-03 build --quiet -p xime-setup-lib --features clipboard-page,pair-page,dict-page` | 退出码 0 | 带 feature 编译路径不回归 |
| 上游 libximecore | `cargo +nightly-2026-07-03 test --quiet -p xime-setup-lib --all-features`（合并 PR #8/#9 后复跑） | `62 passed; 0 failed; 1 ignored` | PR 合并后用例仍全绿 |
| 本仓 XimeYao | `cargo build --quiet` / `cargo clippy --workspace --all-targets --quiet` / `cargo test --workspace --quiet`（合并三个上游修复后复跑） | build 退出码 0；clippy 0 warning；test `1 passed` / `185 passed; 0 failed; 1 ignored` / `6 passed` 全绿 | 本仓 + patched 上游整体 |
| GitHub API | `POST /repos/ximeiorg/libximecore/pulls` ×3 + `GET .../pulls/7/files` | PR #7/#8/#9 创建成功；#7 文件清单 5 个文件、`docs/` 计数 = 0 | 上游 PR 内容与「不带 docs」约束 |

## 6. 新增回归用例（上游）

| 用例 | 断言 |
| --- | --- |
| `schema_manifest::tests::normalize_package_id_ignores_case_separator_and_version` | `moqi_wan_flypymo` / `Moqi Wan Flypymo` / `Moqi Wan Flypymo 1.2.0` 归一为同一 id |
| `schema_manifest::tests::resolve_package_key_finds_legacy_ids` | 历史写法能查到注册表里的真实 key |
| `schema_manifest::tests::local_package_does_not_block_market_install` | `local` 不再阻塞市场方案安装 |
| `schema_manifest::tests::uninstall_resolves_legacy_package_id` | 用规范 id 卸载能删掉文件并移除注册表项 |
| `state::tests::downloaded_schema_ids_map_legacy_dir_names_to_index_id` | 目录 `Moqi Wan Flypymo` 与 `Moqi Wan Flypymo 1.2.0` ⇒ id 均为 `moqi_wan_flypymo`（两种历史命名合在一个用例里，避免并行改写全局索引造成竞态） |
| `state::tests::install_conflict_ignores_builtin_and_local_packages` | 内置包 / 本地方案 / 自身都不算冲突方 |

## 7. 脚本测试（自动化探针，非单元测试）

探针脚本：`target/tmp/schema-probe/schema_probe.rs`（一次性产物，跑完删除；上游临时以
`crates/xime-setup/examples/schema_probe.rs` 复用已编译产物，验证后已移除）。
脚本把数据目录重定向到临时目录（`set_rime_paths` 只生效一次），不触碰真实 `%APPDATA%`，
也不调用 `deploy_all()`（避免拉起真实 librime 维护进程）。

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| 上游 libximecore | `cargo +nightly-2026-07-03 run --quiet -p xime-setup-lib --features clipboard-page --example schema_probe` | `[RESULT] 全部通过`（18 项断言全 PASS，退出码 0） | 见下方逐项清单 |

逐项断言（`[PASS]` 行）：

| # | 断言 | 输出要点 |
| --- | --- | --- |
| 1 | 已下载列表归一到索引 id | `ids = ["moqi_wan_flypymo", "rime-wanxiang"]` |
| 2 | 索引 id 能定位遗留缓存目录 | `package_cache_dir = ...\market\Moqi Wan Flypymo` |
| 3 | 索引 id 直连缓存目录 | `package_cache_dir = ...\market\rime-wanxiang` |
| 4 | 遗留目录名归一到索引 id | `Moqi Wan Flypymo -> moqi_wan_flypymo`；`Moqi Wan Flypymo 1.2.0 -> moqi_wan_flypymo` |
| 5 | 未知 id 原样返回 | `unknown-local-bundle` |
| 6 | 显示名取索引 name | `moqi_wan_flypymo -> 墨水万飞拼音`；无索引时退回 id |
| 7 | 清理下载残片且保留正式包 | `part 存在=false，正式包存在=true` |
| 8 | 本地方案登记成功 | `Ok(())` |
| 9 | 本地方案不阻塞市场方案安装 | `conflicts = []` |
| 10 | 历史写法包登记成功 / 索引 id 也能查到已安装 | `Ok(())`；`installed = true` |
| 11 | 用索引 id 卸载历史写法包成功 | `result = Some((1, 0, 0))`，schema 文件已删除 |
| 12 | 未安装包给出明确原因 | `Err("方案包 not-installed-anywhere 未安装")` |
| 13 | 冲突预检排除内置与本地方案 | `packages = ["other-third-party"]` |
| 14 | 页面提示与系统通知同一份文案 | page 与 toast 均为「方案「moqi_wan_flypymo」已启用，可直接输入使用」 |
| 15 | 方案结果不自动消失 | `poll_background` 后 `install_message` 仍在 |
| 16 | 失败文案包含原因与下一步 | 「…卸载失败：方案包未安装。下一步：请刷新「输入方案」页后重试」 |

> 脚本首轮有 1 项 FAIL（「用索引 id 卸载历史写法包成功」）：原因是探针先写了 `moqi.schema.yaml`
> 再登记 `local`，该文件被 `local` 声明为共享 ⇒ 卸载按设计保留共享文件（`kept_shared`）。
> 调整探针顺序（先登记 `local`，第三方文件后写入）后复跑全绿，属探针写法问题而非产品缺陷。

## 8. 预存失败（与本次改动无关，未擅自修）

| 用例 | 现象 | 判定 |
| --- | --- | --- |
| `xime-config::calculator::tests::parses_explicit_false_only` | panic at `crates/xime-config/src/calculator.rs:60:55`「类型不对应回退默认: Error("auto_detect: invalid type: string \"nope\", expected a boolean", line: 1, column: 14)」 | `calculator.rs` 是上游未跟踪的新文件（先前遗留改动），不在本次范围 |
| `xime-config::schema_manifest::tests::refresh_builtin_tracks_scheme_files_and_skips_user_data` | assertion `left == right` failed，left: 3 right: 0（`schema_manifest.rs:961`） | 移除本次两个文件的改动后仍失败 ⇒ 预存问题（`seed_builtin_from` 非幂等） |

## 9. 肉眼复核（待用户实机执行）

1. 重启设置页 → 「扩展商店 → 方案」：已下载卡片应显示**人类可读包名**（如「墨奇万飞（moqi_wan_flypymo）」），并出现「安装 / 卸载」按钮（不再只有「已安装」纯文本）。
2. 「输入方案 → 已下载」页对「墨奇万飞」点「卸载」：应**不再**报「方案包 Moqi Wan Flypymo 未安装」，应提示「方案「…」已移除并重新部署」。
3. 安装 `rime-wanxiang`：不应再报「文件冲突：README.md（已被 本地方案 使用）」；若确有第三方包占用，冲突弹窗只列真正占用的第三方包。
4. 装第二个市场方案包：不应再被要求先卸载第一个（除非文件真的重叠）。
5. 失败提示（如断网下载）：页面红字应**常驻不消失**，且与系统通知文案完全一致（含「下一步：…」）。
6. 下载中断后重进商店：`market/` 下不应再出现 `*.part.part.*` 半截文件。

## 10. 上游 PR（已提交）

| PR | 分支 | 提交 | 内容 |
| --- | --- | --- | --- |
| [#7](https://github.com/ximeiorg/libximecore/pull/7) | `feat/schema-package-id-and-notify` | `c813f89` fix(config): 方案包 id 归一化，注册表按别名查找 | `normalize_package_id` / `same_package_id` / `resolve_package_key`；`is_installed`、`uninstall_package` 改用别名查找；`detect_conflicts` 跳过 `local`；4 个新单测 |
| | | `40c4327` fix(setup): 方案包 id 归一化到索引 id，通知与页面文案同源 | 索引 id 解析层、`MarketNotice` 文案同源、冲突预检过滤、`.part` 临时名与清理、商店页「安装/卸载」按钮 |
| [#8](https://github.com/ximeiorg/libximecore/pull/8) | `fix/aes-gcm-nonce-deprecation` | `d343088` fix(plugin): aes-gcm 0.11 弃用 `Nonce::from_slice`，改用 `copy_from_slice` | 顺带修掉的弃用告警（clippy `unnecessary_mut_passed` 同源） |
| [#9](https://github.com/ximeiorg/libximecore/pull/9) | `fix/clipboard-page-cfg-gate` | `85e9f0a` fix(setup): 给剪贴板同步消息补上 clipboard-page 门控 | 修复**不带 features 编译 `xime-setup-lib` 失败**（E0433/E0599/E0609，补 cfg 后再补 E0004 的变体门控） |

> 三个 PR 的文件清单里 **docs/ 计数为 0**（按用户要求，证据文件只留在本仓 `docs/verify/`，不推上游远程仓库）。

## 11. 未验证 / 遗留

- 三个 PR 待上游仓库（`ximeiorg/libximecore`）合并；本机账号对该仓库无 push 权限。
- 实机验证由用户执行（AGENTS.md 禁止代理自行运行/安装程序），步骤见第 9 节。
- 上游工作区仍有**先前遗留、未提交**的改动：外观页两项开关（候选栏横/纵排列、数学计算开关）与新增的 `crates/xime-config/src/calculator.rs`；不在本轮范围，未进任何 PR。
- `xime-config::calculator::tests::parses_explicit_false_only` 与 `schema_manifest::tests::refresh_builtin_tracks_scheme_files_and_skips_user_data` 两项预存失败仍在（见第 8 节），本次未擅自修。
