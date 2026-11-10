# 曦码·曜 代码审计（2026-11-09，静态第一阶段）

## 结论

- 未发现远程可利用的 RCE 或提权链：项目不监听网络端口，外部输入面只有命名管道 IPC、方案/插件/模型压缩包、rime 用户数据。
- 主要风险集中在**本地 IPC 信任边界过宽**（Everyone 可连 + 客户端可传任意路径）与**压缩包解压**（无体积上限、无校验、无回滚）。
- 121 处 `unsafe` 未逐块审查，集中在 TSF 回调、托盘与 Win32 封装，列为后续专项。
- 本阶段只读代码、不改代码；所有结论是静态审查意见，未做动态利用验证。

## 范围与方法

| crate | 文件数 | 行数 | 审查方式 |
| --- | --- | --- | --- |
| winxime-server | 32 | 15,530 | 细读 IPC、插件、语音、用户词典、计算器；其余扫描 |
| winxime-tsf | 5 | 2,261 | 扫描 + 回调/unsafe 抽样 |
| winxime-ipc | 3 | 1,036 | 细读协议、管道、消息结构 |
| winxime-tsf-register | 1 | 738 | 扫描（注册/卸载、LoadLibraryW） |
| winxime-setup | 3 | 338 | 扫描 |
| 合计 | 44 | 19,903 | — |

方法：`cargo` 元数据统计规模；按模式扫描（`unwrap`/`expect`/`panic!`/`unsafe`/`let _ =`/`ZipArchive`/`ureq`/`LoadLibraryW`）；对命中文件逐段细读，确认数据流向与校验点。

未覆盖：TSF/COM 线程模型与消息循环、UI 绘制与高分屏、librime 子模块内部、运行时崩溃转储、动态模糊测试。

## 发现清单

| 编号 | 严重度 | 位置 | 问题 | 建议 |
| --- | --- | --- | --- | --- |
| F1 | 中 | `crates/winxime-server/src/ipc_server.rs` 管道安全描述符 | ACL 为 `D:(A;;GA;;;WD)`，任何本机进程（含其他登录会话）可连接并发送命令 | 收紧为当前用户 SID + SYSTEM，或至少 `OW` 而非 `WD` |
| F2 | 中 | 同文件的 `ExportUserDict` / `ImportUserDict` / `RestoreUserDict` | IPC 传入的路径原样交给 librime，等于任意文件写/读 | 服务端做路径白名单（限制在 rime 用户目录或临时目录） |
| F3 | 中 | `crates/winxime-server/src/plugins.rs` 的 `restore_backup` | 备份 zip 解压无条目数/总体积上限、覆盖用户数据无回滚、`create_dir_all` 错误被 `let _ =` 吞掉 | 限制条目与总大小；先解压到临时目录再整体替换；失败报错 |
| F4 | 中低 | `crates/winxime-server/src/speech/download.rs` 的 `download_and_extract` / `extract_tar_bz2` | 无校验和与体积上限；解压只取文件名铺平，失败残留半成品 | 内置 sha256 与大小上限；解压到临时目录后原子替换 |
| F5 | 低 | `crates/winxime-ipc` 与 `ipc_server.rs` 读循环 | 请求无 id、逐字节读到 NUL、单条上限 1 MB；超限或解析失败即断管道 | 改为长度前缀或 JSON Lines；保留连接级错误计数 |
| F6 | 低 | 全仓 163 处 `let _ =` | 文件删除/目录创建/IPC 发送失败被静默吞掉，排障困难 | 关键路径至少 `tracing::warn!`；非关键路径注明理由 |
| F7 | 信息 | 121 处 `unsafe` | 集中在 TSF 回调、托盘、语音采集、UI 绘制 | 后续按模块做 unsafe 专项，逐块补安全前提注释 |
| F8 | 信息 | 测试代码中的 `unwrap` / `expect` / `panic!` | 均在 `#[cfg(test)]` 内（`ipc_server.rs` 的语音测试等），符合规则 | 无需处理 |

## 明细

### F1 命名管道 ACL 过宽

管道创建时用 `SecurityDescriptor::deserialize(u16cstr!("D:(A;;GA;;;WD)"))`，`WD` 是 Everyone，`GA` 是全部权限。服务端以登录用户身份运行，因此任意本机进程都能发 `ShutdownServer`、`ExportUserDict` 等命令；多用户机器上其他会话也能连到同一管道名。

影响：本地拒绝服务、借输入法进程读写用户可写的任意文件（与 F2 组合）、篡改用户词典。不是远程漏洞，但削弱了「输入法服务只应服务本会话」的边界。

建议：ACL 改为当前用户 SID + `SYSTEM`（必要时加 `Administrators`）；管道名加入用户 SID 或会话 id 作为命名空间。

### F2 IPC 命令接受任意路径

`ExportUserDict`、`ImportUserDict`、`RestoreUserDict` 把 `UserDictFile`/`UserDictPath` 里的路径直接传给 `librime::export_user_dict` / `import_user_dict` / `restore_user_dict`。命令本身由设置页使用，路径来自设置页选择的文件。

影响：与 F1 组合后，任意本地进程可让服务端把用户词典写到任意路径（覆盖文件），或从任意路径导入内容到用户词典。

建议：服务端校验路径必须位于用户数据目录或系统临时目录下；导出路径由服务端生成而非客户端指定。

### F3 插件备份恢复缺少边界

`restore_backup` 遍历 zip 条目，经 `enclosed_name` 防目录穿越后写入 rime 目录，跳过 `_xime_backup/`、剥掉 `rime/` 前缀。但没有条目数/解压体积上限，覆盖现有文件前不备份，失败时已写入的文件不回滚；`create_dir_all` 的错误被 `let _ =` 丢弃。

影响：恶意或损坏的备份包可写满磁盘、破坏用户方案数据；失败后状态不可预期。

建议：解压前统计条目数与声明大小并设上限；解压到临时目录，校验通过后整目录替换；错误全程上报。

### F4 语音模型包无完整性校验

下载地址来自编译内置的 `AsrModelRegistry`（非远程可控），但下载后直接 `tar.bz2` 解压，不校验 sha256 与大小；`extract_tar_bz2` 只取每个条目的文件名铺平，重名互相覆盖，失败只删除归档、留下半解压文件。

影响：网络劫持或源站投毒时无检测手段；解压炸弹可耗尽磁盘。

建议：注册表内置 sha256 与字节数；解压到临时目录，成功后替换；限制单文件与总解压体积。

### F5 协议与读循环

请求为 NUL 结尾的 JSON，无请求 id；服务端逐字节读、超过 `MAX_BUFFER_SIZE`（1 MB）即断开。客户端超时后丢弃整条管道，所有并发调用一起失败重连。

影响：慢客户端或半包会占住读线程；错误无法与请求对应，排障成本高。

建议：长度前缀或每行一条 JSON；加入请求 id；把「超时即丢管道」限制在协议错位时。

### F6 错误吞掉

`let _ =` 共 163 处，多数是清理动作。抽查发现 `plugins.rs` 的目录创建、`user_dict.rs` 的临时文件删除等关键步骤也在其中。

建议：删除临时文件可以保留；目录创建、写入、IPC 发送失败必须记日志。

### F7 `unsafe` 分布

`text_input_processor.rs` 27 处、`tray.rs` 20 处、`speech/capture.rs` 11 处、`ui/view.rs` 10 处、`tsf-register/main.rs` 8 处，其余零散。多数是 Win32 结构体与 COM 接口封装，属 TSF 开发的常规形态，但缺少「安全前提」注释，无法快速判断生命周期与指针有效性。

建议：按模块做 unsafe 专项，每块补「指针来源、生命周期、失败路径」注释，优先 TSF 回调与托盘线程。

### F8 测试内 panic 类调用

`models.rs`、`schema_switches.rs`、`speech/*`、`calculator.rs`、`plugins.rs` 的 `unwrap/expect/panic!` 均在测试模块内，符合项目规则（生产代码禁用）。

## 已确认的防护

- 词典名进临时文件名前经 `file_label` 过滤为 ASCII 字母数字/`_`/`-`，临时路径含 pid 与自增序号。
- zip 条目用 `enclosed_name` 校验，拒绝绝对路径与 `..`；语音解压用 `file_name` 只取文件名，拒绝空名与 `.`/`..`。
- 计算器表达式有 `MAX_EXPR_LEN = 48` 上限。
- IPC 单条消息上限 1 MB，超限断开而不是继续分配。
- 生产代码未发现 `unwrap()`/`expect()` 直接用于外部输入。

## 建议处置

- 登记到 PROGRESS 未决：F1–F4，等用户排期；F1/F2 是同一处边界，建议一起改。
- 后续专项：F7 的 unsafe 审计。
- 暂不处理：F5、F6 的非关键路径，随相关功能点顺手改善。

## 附：审计时的构建环境问题

本机 MSVC 14.44 在 cargo 环境里编译 C 依赖时，`cl` 报 `D8050 无法执行 c1.dll：未能将命令行放入调试记录中`（ring、zstd-sys、libsqlite3-sys、lzma-sys 均中招）；手工以相同参数在 cmd/vcvars 下编译同一文件成功。属本机工具链/进程环境问题，与仓库代码无关，未改动任何受版本控制的构建配置。CI 在干净 windows-2022 上构建，不受影响。
