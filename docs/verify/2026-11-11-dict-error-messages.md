# U-29 词典操作失败文案修复（2026-11-11）

## 背景

用户实机走词典三操作（m04348「已测试词典导入导出，模型下载，方案下载」）时，导出失败提示为「导出emoji失败：导出用户词典失败：ExportUserDict」——librime 错误枚举的 Debug 格式直出给用户，违反本仓「用户可见文案说清发生了什么/为什么/下一步」的硬规则。日志核查发现三个叠加问题：

1. `handle_dict_command` 三处 `dict_error(..., format!("…失败：{e:?}"))`（Restore/Export/Import）Debug 直出。
2. `BackupUserDict` 失败时只回 `success=false`，连 `dict_response.error` 字段都没填，客户端只能显示兜底文案。
3. 引擎锁 `try_lock` 失败（`WouldBlock`）时所有命令一律裸 `IpcResponse::fail`——词典命令正好撞上打字高峰（日志：16:57:36 ExportUserDict 紧跟 `Engine lock would block`），设置页看到的失败其实与词典无关。

## 改动

全部在 `crates/winxime-server/src/ipc_server.rs`：

- 新增 `fn dict_op_reason(op: &str, e: librime::error::Error) -> String`：按枚举分支转人话——引擎未初始化 / 版本未提供该功能 / 词库可能被占用或文件无法写入 / 词典名含无法处理的字符 / 兜底「引擎返回失败」；每条都说清「发生了什么 + 请停止输入后重试/重启输入法」的下一步，末尾括注 Display 原文（英文）供排查。
- Restore/Export/Import 三处 `{e:?}` 改用 `dict_op_reason`。
- `BackupUserDict` 分支重写：失败统一走 `dict_error`（带可读原因），成功只填 `count`（不再手工铺 DictResponse 全字段）。
- 引擎锁 `WouldBlock` 分支：词典七命令（ListUserDicts/ListDictEntries/ImportDictEntry/Backup/Restore/Export/Import）改回 `dict_error`，文案「输入法正在处理其它任务（如同步词库或部署方案），暂时无法执行词典操作。请稍等几秒后重试。」；其余命令行为不变（仍裸 fail）。

客户端链路无需改动：`winxime-ipc/src/pipe.rs` 的 `dict_error_message` 本就优先取 `dict_response.error`，此前是服务端没给。

## 机检

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly | `cargo build --quiet`（`CC=CXX=clang-cl`，`PATH` 前置 LLVM） | exit 0，无 warning | 全工作区编译 |
| 同上 | `cargo clippy --workspace --all-targets` | exit 0，0 条 warning | 全工作区 lint |
| 同上 | `cargo test --workspace` | 192 passed、1 ignored、0 failed（未新增单测：文案改动由客户端 `dict_error_message` 既有链路承接，错误分支依赖 librime 实际返回，实机覆盖） | 全工作区测试 |
| 同上 | `node scripts/doccheck.mjs`、`node scripts/refcheck.mjs` | 双绿 | 文档 |

## 实机验证步骤

**自动验证（2026-11-11 已完成）**：临时探测脚本（存档于 `target/tmp/u29-probe/u29_probe.rs`，复用 `winxime-ipc` 客户端，只发失败路径不改词库数据）对本仓 debug 构建 server 实测六条命令，全部拿到预期响应：

| 探测 | 响应 error 内容 | 覆盖 |
| --- | --- | --- |
| 导出不存在的词典 | 「导出用户词典失败：引擎执行词库读写失败，词库可能正被占用或文件无法写入。请停止输入后重试…（Failed to export user dict）」 | `dict_op_reason` 人话 + Display 原文 |
| 备份不存在的词典 | 「备份用户词典失败：…（Failed to backup user dict）」 | 备份失败**带** error（修复点；修复前无此字段） |
| 恢复白名单外路径 | 「路径不在用户数据目录或临时目录内，已拒绝：…」 | dict_paths 拒绝回归 |
| 恢复不存在的文件 | 「文件不存在或无权访问：系统找不到指定的文件。(os error 2)」 | dict_paths 拒绝回归 |
| 导入白名单外路径 | 「路径不在用户数据目录或临时目录内，已拒绝：…」 | dict_paths 拒绝回归 |
| 列出用户词典 | `success=true`，无 error | 成功路径通信正常 |

引擎锁占用分支（词典命令 WouldBlock →「输入法正在处理其它任务…」）无法在脚本里稳定复现（需引擎长时间持锁），由代码路径审查覆盖，留待日常使用中出现时确认。

自动验证结论：**通过**。验证过程中确认一个环境事实：安装态 server（2026-11-10 构建）对 Rust `interprocess` 客户端连接返回 PermissionDenied（os error 5），原生 CreateFileW 同参数正常——起因待查，与 U-29 无关（本轮测试以本仓 debug server 替代完成）。

仍建议用户日常使用中各走一次导出/导入/恢复补 U-5b 证据：

1. `.\rebuild.ps1`（管理员）重装并启动 server。
2. **引擎占用提示**：在记事本里连续打字，立刻切到设置页点「导出」——预期提示「输入法正在处理其它任务…请稍等几秒后重试」，不再是「导出用户词典失败：ExportUserDict」。
3. **空闲导出**：停止打字等几秒后再导出——预期成功并弹出导出文件路径（此步同时补 U-5b 的导出实机证据）。
4. **导入/恢复**：各走一次（此前从未实测，U-5b 收尾）。
5. 观察失败场景文案是否为中文人话 + 可执行的下一步（可临时断开引擎状态复现，或跳过）。

## 回退

单文件改动，`git revert` 对应提交即可；客户端协议字段（`DictResponse.error`）未变。
