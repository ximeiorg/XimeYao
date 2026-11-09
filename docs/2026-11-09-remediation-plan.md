# 审计发现修复计划（2026-11-09）

依据 [代码审计](2026-11-09-code-audit.md) 的 F1–F7，按「先边界、后完整、再协议」排序。

## 原则

- 一次一个功能点，按 `WORKFLOW.md §2` 过门，每项落 `docs/verify/`。
- `libximecore` 内的改动必须先由团队专家商定必要性，形成结论后报用户，同意后再改。
- XimeYao 内能独立完成的先做，不因 P3b 阻塞 P1/P2/P3a。
- 每批独立提交，可单独回退；仍不 push。

## 批次总览

| 批次 | 对应 | 内容 | 预计 | 前置 |
| --- | --- | --- | --- | --- |
| P1 | U-5 / F1+F2 | IPC 管道 ACL 收紧 + 客户端路径白名单 | 0.5–1 天 | 无 |
| P2 | U-6 / F3 | 插件备份恢复：体积上限、临时目录替换、错误上报 | 0.5 天 | 无 |
| P3a | U-7 前半 | 语音包下载/解压体积上限 | 0.5 天 | 无 |
| P3b | U-7 后半 | 模型 sha256/大小校验（改 `libximecore`） | 0.5 天 | 专家结论 + 用户同意 |
| P4 | F5 | IPC 协议长度前缀/请求 id | 1–2 天 | 与功能点合并，不单独排 |
| P5 | F7 | unsafe 专项（分批） | 2–3 天 | 无 |

## P1 IPC 边界（U-5）

范围：`crates/winxime-server/src/ipc_server.rs`、`crates/winxime-server/src/user_dict.rs`；若设置页参与路径选择，联动 `crates/winxime-setup` 与 `crates/winxime-ipc/src/messages.rs`。

做法：

1. 管道 ACL 从 `D:(A;;GA;;;WD)` 改为当前用户 SID + `SYSTEM`，管道名保持不变。
2. `ExportUserDict` 的路径由服务端在系统临时目录生成，客户端只读该路径，不再由客户端指定。
3. `ImportUserDict` / `RestoreUserDict` 的路径必须是用户数据目录或系统临时目录下的真实文件，否则拒绝，并在响应里返回可读原因。

验收：单测覆盖路径白名单与拒绝分支；设置页导出/导入/恢复各一次；ACL 用 `icacls` 之外的 SDDL 单测断言不再含 `WD`。

风险：若存在跨会话连接场景，收紧 ACL 会断开连接；本机单会话使用无影响。回退：ACL 与路径校验分开提交。

## P2 插件备份恢复边界（U-6）

范围：`crates/winxime-server/src/plugins.rs` 的 `restore_backup`。

做法：解压前统计条目数与声明总大小并设上限；解压到 `rime_dir` 下的临时目录，全部成功后再整体替换；`create_dir_all`、写入、替换失败都要上报，失败保留原数据。

验收：单测构造超限包与损坏包；失败注入后用户目录不变；现有备份/恢复测试回归。

风险：替换阶段文件被占用时恢复失败；失败路径必须清掉临时目录并保留原目录。

## P3a 语音包体积上限（U-7 前半）

范围：`crates/winxime-server/src/speech/download.rs`。

做法：下载过程累计字节数，超过硬上限即中止并删除归档；解压阶段限制单文件与总解压量，超限即停。

验收：单测用受限读取模拟超限；正常模型包下载与解压成功。

## P3b 模型校验和（U-7 后半，需审批）

范围：`libximecore` 的 `crates/xime-speech/src/registry.rs`（`AsrModelProfile` 增加 `sha256` 与 `size` 字段，为内置四个 profile 填值），XimeYao 侧在下载完成后校验。

流程：

1. 队长组织 2–3 名专家（安全、架构、发布）评审：是否确有必要、字段是否兼容旧版本、是否需要发版。
2. 结论与改动方案报用户。
3. 用户同意后再改 `libximecore`，并按该仓规则提交。
4. XimeYao 侧接线并补验证。

不做也可以：P3a 已限制体积，完整性缺失可暂时接受；由用户决定是否值得动 `libximecore`。

## P4 IPC 协议（F5）

请求帧无 id、逐字节读到 NUL。改动涉及 `winxime-ipc` 与 server/TSF/setup 三端，建议随下一次 IPC 相关功能点一起做，不单独排期。

## P5 unsafe 专项（F7）

按模块分批：TSF 回调 → 托盘与线程 → 语音采集 → UI 绘制。每块补「指针来源、生命周期、失败路径」注释，必要时加防御；不改行为，不做顺手重构。每批一个提交，附验证说明。

## 汇报点

- 每批完成：`cargo build --quiet`、`cargo clippy --workspace --all-targets`、`cargo test --workspace` 结果 + `docs/verify/` 档案 + PROGRESS 更新。
- P3b 开工前：专家结论 + 用户同意，两者缺一不动 `libximecore`。
- 全程本地提交，不 push。
