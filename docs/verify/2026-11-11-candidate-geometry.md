# 2026-11-11 候选栏窗口几何（U-9 + U-11 + U-12 + U-13 + U-14 + U-15 + U-16 + 实机三修复 + U-10 + U-28）

- 背景：候选栏位置由两处硬编码 `y + 24` 决定（`show()` 与 `WM_SET_POSITION`），不随 DPI 缩放，光标下方空间不足时不会翻到上方（U-9）；面板展开/切页/翻页只改窗口尺寸（五处 `SetWindowPos` 全带 `SWP_NOMOVE`），窗口原地向下向右生长，底部与右侧顶出屏幕、遮住输入区（U-11）。
- U-12（用户实机反馈「忽上忽下、闪烁」）：纵向布局时每行只有 1 个候选，打第一个键只有 3 个候选＝3 行，打出整词后最多 10 个＝10 行，窗口高度跟着跳；叠加「先 `show` 用旧尺寸摆一次、再 `update` 用新尺寸摆第二次」，一轮两次 `SetWindowPos`，视觉上就是候选栏上下抖。
- U-13（用户实机反馈「点别处后数字跑到点击处」）：数学计算自动识别把数字挂进 TSF 组词串后，若用户在 2.5 秒兜底窗口内点击别处，应用收掉组词串——数字已留在原文档，但影子缓冲的空闲提交线程不知情，醒来后注入 F24 把同一份数字插到**点击处的新光标**上。
- 改法：把「窗口多大」和「窗口摆哪」解耦成两条链路——尺寸仍由 `calculate_client_rect` 算，位置统一交给新的纯函数 `place_window`，所有尺寸变更点一次 `SetWindowPos` 同时落定两者；再给择位加「粘性」（插入点没动且原位放得下就不挪窗口）；组词串被宿主终止时由 TSF 单向通知 server 作废影子空闲提交。

## 改动

### 1. 插入行高度进入 IPC 协议

`crates/winxime-ipc/src/messages.rs`：`Position` 由 `{ x, y }` 扩为 `{ x, y, height }`，`height` 带 `#[serde(default)]`——旧版 TSF 不带该字段时按 0 收下，布局层回落到默认行高，不会解析失败。

`crates/winxime-tsf/src/text_input_processor.rs`：

- 新增 `type CaretAnchor = (i32, i32, i32)` 与 `type PendingCaret = std::sync::Mutex<Option<CaretAnchor>>`：TSF 编辑会话期间握着文档写锁，不能同步等 IPC，量到的锚点先落槽位、会话外再发。
- `update_position(x, y, height)`；`caret_position_in_session` 返回 `(rc.left, rc.bottom, rc.bottom - rc.top)`；`update_caret_position_sync` 用 `rect.bottom - rect.top` 补行高；`send_pending_position` 解三元组。

### 2. 择位函数（可单测）

`crates/winxime-server/src/ui/layout.rs` 新增：

- `struct WorkArea { left, top, right, bottom }`（物理像素，来自 `rcWork`）。
- `fn clamp_axis(value, low, high)`：区间倒挂（窗口比工作区还宽/高）时贴左/上边。标准库 `clamp` 在 `min > max` 时会 panic，而多显示器、缩放切换、窄分辨率下这种倒挂真实存在。
- `fn place_window(anchor, hw_width, hw_height, dpi, work, prefer_above) -> Placement`：`scale = dpi/96`，空隙 `CURSOR_GAP = 4.0` DIP、行高缺失时 `FALLBACK_LINE_HEIGHT = 20.0` DIP；窗口物理尺寸含四周 `BLUR_RADIUS` 的透明模糊边，因此可视内容位置各减一个 `BLUR_RADIUS * scale`。纵向优先贴插入行下方，下方放不下翻到上方，上下都放不下退回下方并夹进工作区。`prefer_above` 是上下粘滞位：上次在上方就继续留上方，不因这次变矮跳回下方。
- `fn place_window_sticky(anchor, w, h, dpi, work, previous, anchor_changed) -> Placement`（U-12 核心）：插入点没变、且上一次的位置放得下新尺寸 → **原样返回上一次的位置**；否则重新 `place_window(..., previous.above)`。
- `fn fits_within(x, y, w, h, work)`：四边含等号的「整块在工作区内」判定。

配套 14 条单测：贴下方、翻上方、上下都紧、右侧越界、缺行高、窗口比屏幕大、高 DPI 缩放、底边不越界；粘滞四条（上方放不下退回下方、已在上方不翻回、边界含等号、插入点稳定时高度变化不挪窗、插入点变了要重摆、涨到放不下才翻）。

### 3. 尺寸与位置合并成一次 SetWindowPos

`crates/winxime-server/src/ui/view.rs`：

- `fn work_area_at(point) -> Option<WorkArea>`：`MonitorFromPoint` + `GetMonitorInfoW` 取 `rcWork`。
- `unsafe fn set_size_and_place(this, hwnd, w, h, anchor_changed)`：有锚点时 `place_window_sticky` 算出位置再 `SetWindowPos`（无 `SWP_NOMOVE`）；没有锚点时退回原来的「只改尺寸」。
- `fn window_matches(hwnd, x, y, w, h)`：尺寸与位置都没变就直接返回，不调 `SetWindowPos`——每次按键都让 DWM 重新合成一遍是「闪」的另一个来源。
- 四处尺寸变更全部改调它：`WM_SHOW_CANDIDATE`、`WM_SHOW_ROOT`、`WM_HIDE_ROOT`、`relayout_and_repaint`（面板展开/收起/切页/翻页/hover 共 16 个调用点都走这里）。
- `WM_SHOW_CANDIDATE` 改为接收 `ShowRequest { anchor, context }`：在同一条消息里建模型、算尺寸、择位、绘制，最后才 `ShowWindow`。删除 `WM_UPDATE_CANDIDATE` 分支——它与 `show` 拆开正是「一轮摆两次」的根源。

`crates/winxime-server/src/ui/mod.rs`：`CandidateWindow` 新增 `anchor: Cell<Option<(i32, i32, i32)>>` 与 `placement: Cell<Option<Placement>>`；`show(anchor, ctx)` 把锚点与上下文一起装箱投递（不再做偏移），删除 `update()` 与 `WM_UPDATE_CANDIDATE`（5 处调用点全是「show 紧跟 update」，无独立 update）；删除 `WM_SET_POSITION`（全仓无发送方，是死分支）。

`crates/winxime-server/src/context.rs` / `ipc_server.rs`：`InputContext` 新增 `caret_height` 与 `caret_anchor()`，`handle_update_position` 落行高，四处展示点合并成一次 `window.show(anchor, ctx)`。

`crates/winxime-server/src/calculator.rs`：会话锚点 `ANCHOR` 升级为三元组，`window_origin` 同改；刷新点合并成一次 `show`，随之删除已无用的 `WINDOW_SHOWN` 开关（它原本存在的理由就是「避免重复 show」，而重复 show 现在不会再触发额外 `SetWindowPos`）。

`crates/winxime-server/src/ui/model.rs`：`From<&Context> for CandidateModel` 里 `max_items` 原被定义两遍（`clamp(1, 10)` 给 comments、`clamp(1, 32)` 给 items），合并为一处 `clamp(1, 10)`——选字键只有 1–9 + 0，超过 10 个也只能用方向键选。

### 4. 组词串终止通知（U-13）

- `crates/winxime-ipc/src/messages.rs`：`IpcCommand` 新增 `CompositionTerminated`（单向命令，`is_one_way()` 覆盖它，测试同步）。
- `crates/winxime-tsf/src/text_input_processor.rs`：`CompositionSink` 增加 `ipc: IpcClientHandle` 字段；`OnCompositionTerminated` 在清掉组词串槽位后调 `notify_composition_terminated()`（单向 `send_no_reply`，连接断了静默放弃）。
- `crates/winxime-server/src/ipc_server.rs`：锁外预分发与主 match 各加一支 `CompositionTerminated`，落 `handle_composition_terminated`——`take_shadow_for_flush()`（数字已随组词串留在原地）+ `calculator::reset`（作废空闲提交代号），2.5 秒兜底线程醒来发现代号新一代即退出，不再注入 F24。

### 5. 面板方向（U-14）

用户期望：横向候选栏（⋮ 在左）展开面板在候选栏**上方**；纵向候选栏（⋯ 在顶）展开面板在候选栏**右侧**。原实现面板一律长在候选栏下方（窗口向下生长）。

- `crates/winxime-server/src/ui/panel.rs`：新增 `enum PanelSide { Above, Right }` 与 `panel_side(horizontal)`（横→Above、纵→Right）与 `panel_origin(side, bar_width)`（Above→`(BLUR_RADIUS, BLUR_RADIUS)`；Right→`(BLUR_RADIUS + bar_width + PANEL_GAP, BLUR_RADIUS)`）。`window_to_panel` 改为 `(bar_width, panel_width, panel_height, side, x, y)`，按 `panel_origin` 映射；`draw_panel` 同步改参，面板内容全部 panel-local 绘制不需要动；删除已无语义的 `panel_extra_height`。命中测试重写为双向用例。
- `crates/winxime-server/src/ui/model.rs`：`RenderedMetrics` 新增 `panel_side: Option<PanelSide>`（None＝收起）与 `panel_content_width: f32`（面板内容宽，收起为 0）。
- `crates/winxime-server/src/ui/layout.rs`：`calculate_client_rect` 按 side 算增量——Above：`height = bar_height + PANEL_GAP + panel_height`、宽度撑到 `PANEL_MIN_WIDTH`（候选条带被压到窗口底部）；Right：`width = bar_width + PANEL_GAP + panel_width`、高度取 `bar_height` 与面板列高的大者。`candidate_item_at` 与 menu_button 矩形带 Above 偏移（`bar_top = panel_height + PANEL_GAP`）。
- `crates/winxime-server/src/ui/paint.rs`：候选绘制起点按 `panel_side` 加 `bar_top` 偏移（面板在上方时候选条带画在下半部分）；`draw_panel` 调用改传 `panel_content_width`、`bar_width`、`panel_side`。
- `crates/winxime-server/src/ui/view.rs`：`panel_metrics` 返回五元组（bar_width、bar_height、panel_width、panel_height、side），三处 `window_to_panel` 调用点改新签名。择位链路 `place_window_sticky` 不变——面板方向只改窗口形状，整窗仍按 hw 尺寸择位。

### 6. 面板展开/收起动画（U-15）

面板沿 U-14 的新方向做展开/收起动画：约 130ms（8 帧 × 16ms）三次方 ease-out，展开时从候选栏「长出来」（上方从下滑入、右侧从左滑入）并淡入，收起反向播放、播完才真正收窗。

- `crates/winxime-server/src/ui/mod.rs`：新增 `PanelAnim { expanding, frame, total_frames }` 与 `linear()`/`progress()`（ease-out），`CandidateWindow` 挂 `panel_anim` 与 `panel_anim_timer` 两个状态位；`panel_progress()` 给绘制链算当前可见度（无动画恒 1.0）。
- `crates/winxime-server/src/ui/panel.rs`：`draw_panel` 增加 `progress` 参数——所有笔刷 alpha 乘可见度、面板原点按 `PANEL_ANIM_SLIDE_DIP`（12 DIP）做滑入偏移；面板内部绘制全部以偏移后的原点平移，逐项内容自动跟随。
- `crates/winxime-server/src/ui/paint.rs`：`on_paint`/`on_paint_with_metrics` 透传 `panel_progress`。
- `crates/winxime-server/src/ui/view.rs`：`PANEL_ANIM_TIMER_ID=2`（与语音轮询定时器并存）；menu_button 点击启动/防抖（动画中再点直接落终态）；`WM_TIMER` 逐帧推进，收起动画播完才 `collapse_panel` 收窗；候选栏隐藏、子页跳转、条目上屏等路径都先 `cancel_panel_anim` 打断动画（不打断直接收，无动画）。
- 单测：ease-out 进度单调有界、首末帧 0/1、`total_frames=0` 不除零（共 2 条）。

### 7. 面板快捷键（U-16）

打字窗口里按 Ctrl+` 直接展开/收起面板，不必用鼠标点候选栏上的按钮；不注册系统全局热键（不抢占其它应用的 Ctrl+`），只在输入法激活期间由 TSF 以「保留键」分发。

- `crates/winxime-ipc/src/messages.rs`：`IpcCommand` 新增 `TogglePanel`（单向命令，`is_one_way()` 覆盖，测试同步）。
- `crates/winxime-tsf/src/text_input_processor.rs`：本地定义 `TF_MOD_CONTROL`（0x0002，windows crate 该 feature 未启用）与 `PANEL_HOTKEY_VK`（VK_OEM_3）、`PANEL_HOTKEY_GUID`（自定义 GUID）；`activate_impl` 在 `AdviseKeyEventSink` 后 `PreserveKey` 注册 Ctrl+`（成功才置 `panel_hotkey_advised`），`deactivate` 对称 `UnpreserveKey`；`OnPreservedKey` 命中该 GUID 即 `notify_toggle_panel()` 并返回已处理；`IpcClientHandle` 新增 `notify_toggle_panel`（单向 `send_no_reply`，连接断了静默放弃）。
- `crates/winxime-server/src/ipc_server.rs`：锁外预分发与主 match 各加一支 `TogglePanel` → `handle_toggle_panel`（不碰引擎，直接投窗口消息）。
- `crates/winxime-server/src/ui/mod.rs`：`WM_TOGGLE_PANEL = WM_USER + 7`；`CandidateWindow::post_toggle_panel()`。
- `crates/winxime-server/src/ui/view.rs`：原 menu_button 点击的「防抖 + toggle + 动画」逻辑抽成 `toggle_panel(this, hwnd)`；`WM_TOGGLE_PANEL` 分支仅在候选栏可见时响应（避免凭空弹面板），复用同一逻辑（含 U-15 动画）。
- 设置页改快捷键（U-16b）属上游设置页范围：`xime-config` 加 `panel.hotkey` 字段 + 外观页控件，走上游 PR；本期默认键 Ctrl+` 硬编码。

### 8. 实机反馈三修复（纵向空白/按钮重叠、横向收起后浮空、品牌栏）

首轮实机（用户 2026-11-11）暴露三个问题：

- **纵向展开面板后下方大片空白**：`crates/winxime-server/src/ui/layout.rs` `calculate_client_rect` Right 分支把候选栏高度重复叠加——`height = (bar_height + PANEL_GAP + ph).max(bar_height)`，而纵向窗口本来就以「候选条带与面板列并排」绘制，窗口高只需要两者较大者。改为 `let height = ph.max(bar_height);`。
- **纵向汉堡按钮与面板重叠**：同文件 menu_button 矩形的 x 原按面板展开后的总宽 `width` 居中，按钮被水平挪进面板列；改按候选条带自身宽度 `bar_width` 居中。
- **横向收起面板后候选栏浮空**：`crates/winxime-server/src/ui/view.rs` `relayout_and_repaint` 恒传 `anchor_changed=false`，粘滞择位认为「原位放得下」就沿用面板展开时（窗口较高时）的位置，收起后候选栏停在半空。改为比较新旧 metrics 的 `panel_side`：`had_panel && 新为 None`（面板收起）时传 `panel_collapsed=true` 当作插入点变化强制重摆，贴回光标；展开方向仍原位长大不挪窗。
- **品牌栏布局**：`crates/winxime-server/src/ui/panel.rs` 菜单页底部品牌栏由整行居中一段文字改为两端分布——左侧「曦码·曜输入法」（左对齐、加粗、前景色）+ 右侧 `github.com/ximeiorg/XimeYao`（trailing 对齐、次级色、字号再小一号），左右各留 `PANEL_H_INSET` 内边距。

### 9. 品牌栏移除 + 版本推进 0.5.0

- 用户复测三修复通过（「测试正常」），但要求候选面板不显示版权标识：菜单页底部品牌栏（「曦码·曜输入法」+ 项目地址）整体删除——`crates/winxime-server/src/ui/panel.rs` 菜单页分支的两段 DrawText 与 `panel_footer_y()` 的 `PANEL_ITEM_HEIGHT` 分量（内容区下界改为 `PANEL_HEIGHT - PANEL_BOTTOM_MARGIN`）；`PANEL_HEIGHT` 由 162 收紧到 130（顶部 10 + 3 行卡片 104 + 间距 8 + 底边距 8），菜单页不再留出无内容的底部条；相关注释与单测 `menu_rows_fit_above_footer` 同步。
- 版权/版本/主页信息集中在关于页展示：`AppMetadata.version` 本就取 `env!("CARGO_PKG_VERSION")`、`homepage` 为 `https://github.com/ximeiorg/XimeYao`，随版本号自动跟随。
- 版本推进：`Cargo.toml` workspace `version` 0.4.0 → **0.5.0**（本批功能量大：几何三项 + 面板四项 + 三修复 + 品牌栏移除）；`msi-build.ps1` 自动读取该值，MSI/安装后元数据同步为 0.5.0。

### 10. 二级菜单定位 + 构建警告清零

- **二级菜单切换定位**（用户 2026-11-11 反馈）：面板展开期间窗口尺寸随页面变化（菜单 130、列表子页更高、网格/语音页各不同），旧逻辑只在「面板收起」时强制重择位，切页走粘滞择位「原位放得下就不挪」——窗口向下/向右原地长大，会盖住正在输入的文字或把候选条带推离光标。改为 `crates/winxime-server/src/ui/view.rs` `relayout_and_repaint`：**面板展开状态下一律按插入点重新择位**（`panel_open = metrics.panel_side.is_some()`，与 `panel_collapsed` 一起传入 `anchor_changed`）。重择位是确定性的：同一锚点+尺寸+上下偏好得到同一位置，hover/翻页等尺寸不变的 relayout 不会引起窗口挪动；上方布局窗口底边钉在光标上沿、面板向上长，下方布局窗口顶边钉在光标下沿。
- **上游 aes-gcm deprecated ×2**：`xime-plugin/src/cipher.rs`（Windows 与 Linux 两个 imp 模块的 encrypt/decrypt）`Nonce::from_slice(&iv)` 弃用，改为 `let mut nonce = Nonce::default(); nonce.copy_from_slice(&iv);` 后传 `&nonce`——密文格式不变（nonce 仍是原 12 字节），与 Android 端互换不受影响。
- **winxime_tsf.dll LNK4104 ×4**：链接器对 cdylib 导出的 COM 入口（`DllGetClassObject`/`DllCanUnloadNow`/`DllRegisterServer`/`DllUnregisterServer`，`dll.rs` 的 `#[no_mangle]`）报「导出符号应为 PRIVATE」——TSF DLL 本来就必须公开导出这四个入口，属正常形态。在 `crates/winxime-tsf/src/lib.rs` 加 crate 级 `#![allow(linker_messages)]`（注释说明缘由），不再把链接器 stdout 当 warning 透传。
- 构建警告清零：`cargo build --quiet` 与 `cargo clippy --workspace --all-targets` 均 0 条 warning（含上游）。

### 11. U-10 日志治理 + U-28 上屏通知修复

背景（U-10 量化结论）：主链路性能不慢（按键→响应 p50 0.94ms），真问题是日志洪水——TSF 每键 27~47 行 debug（含完整 `IpcResponse` Debug 转储，单行可达数 KB）、server 按键热路径全是 info（含候选列表 Debug 转储），tsf.log 半天 110MB、server.log 43MB。根因：上游 `xime-config` 的默认过滤器对本项目各 crate 开 debug。

- **TSF 侧**（`crates/winxime-tsf/src/text_input_processor.rs`）：全文件 `debug!`→`trace!`；删除完整 `IpcResponse` Debug 转储（洪水最大单点）；`process_key_event` 收敛为返回后单条 `trace!("process_key_event: vk=… up=… handled=… composing=…")`；`update_caret_position_sync` 4 条并 1（含坐标）；`OnTestKeyDown`/`OnTestKeyUp`/`OnKeyDown`/`OnKeyUp` 各收敛为 1 条或仅在吃键时打；错误路径（FAILED、无连接）升 `debug!` 保留。`OnTestKeyDown`/`OnTestKeyUp` 是 TSF 对每个按键都调用的「预询问」回调，此前无论是否吃键都打日志。
- **server 侧**（`crates/winxime-server/src/`）：`ipc_server.rs` 按键热路径 `info!`→`trace!`、删候选列表 Debug 转储，每键只留 1 条汇总 trace；`SelectCandidate`/`ChangePage`/`ShowRoot`/会话与焦点事件降 debug；`>>> COMMIT_TO_SCREEN` 保留 info（低频、上屏关键事件）；词典管理/管道初始化等低频 info 不动。`ui/view.rs` 删 `model.items` 与 `item_widths` 转储、窗口消息入口降 trace。`plugins.rs` 剪贴板热路径降 trace。`calculator.rs` 18 条 info 降 debug。`notify.rs` 通知落日志降 debug。
- **U-28 通知修复**（`crates/winxime-server/src/ui/view.rs`）：实机日志实锤「多余符号/表情上屏通知」＝面板字形格/剪贴板条目上屏成功后重复弹系统通知（server.log：「通知: 曦码·曜 — 已上屏：🍔」）。改为：上屏成功不再弹通知（内容刚被用户点选、已可见）；仅在失败时提示「已复制到剪贴板，按 Ctrl+V 粘贴」或「上屏失败：剪贴板被其他程序占用，请重试」。计算器回放失败、语音会话、部署同步等需用户行动的通知保留。
- 说明：日志级别门槛仍由上游默认过滤器控制（trace 不会进文件），本次治理是「把每键噪音降到 trace 级 + 删大对象转储」，调试时用 `RUST_LOG` 环境变量即可临时打开，无需改码。


| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 专业版 10.0.26100 / rustc 1.98.0-nightly | `cargo build --quiet`（`CC=CXX=clang-cl`，`PATH` 前置 LLVM） | exit 0，**0 条 warning**（含上游；aes-gcm deprecated 与 LNK4104 已清，见第 10 节） | 全工作区编译 |
| 同上 | `cargo clippy --workspace --all-targets` | exit 0，0 条 warning | 全工作区 lint |
| 同上 | `cargo test --workspace` | 183 passed、1 ignored、0 failed；tsf 6 passed；合计 192（几何基线 176 + 择位 14 + 动画 2；U-16/三修复/品牌栏移除/二级菜单定位/U-10/U-28 未新增单测——保留键行为依赖 TSF 宿主实机覆盖，其余为择位参数修正与日志级别调整，由既有择位与布局单测守住回归） | 全工作区测试 |
| 同上 | `node scripts/doccheck.mjs`、`node scripts/refcheck.mjs` | 双绿 | 文档 |

实机验证（需用户执行）：

1. `.\rebuild.ps1`（管理员）；
2. 记事本里打一长串词组，让候选栏出现 → 候选栏应紧贴光标下方约 4 DIP，不再浮空 24 像素；
3. **先打一个键、再继续打成整词**（这一条专门验 U-12）→ 候选从 3 个涨到 10 个时，窗口应只向下长高，上边界不动、不上下跳；
4. 把记事本窗口拖到屏幕最底部，光标停在最后一行打字 → 候选栏应翻到光标上方，且整体可见；
5. 横向候选栏与纵向候选栏各点一次 ⋮ 展开菜单、切「剪切板」「表情」「符号」子页、翻页 → 面板完整可见，底部不超出任务栏、右侧不超出屏幕；
6. 反复按同一个键看候选变化（快速输入一段）→ 不应再有闪烁或抖动；
7. 拖到第二块显示器（若有，缩放不同更好）重复第 2~6 步 → 偏移量随 DPI 变化，位置仍正确；
8. 打开计算器模式（输入 `=` 前缀）→ 候选栏仍贴在进入计算器时的位置；
9. **U-13 复现路径**：确认设置里数学计算开启 → 输入 `123`（不按 Enter、不按 `=`）→ 立刻用鼠标点文档里其它位置 → 数字应留在原输入位置、不重复、不出现在点击处。
10. **U-14 面板方向**：横向候选栏点 ⋮ 展开菜单 → 面板应出现在候选栏**上方**；纵向候选栏点 ⋯ 展开菜单 → 面板应出现在候选栏**右侧**；两个方向各切「剪切板」「表情」「符号」子页、翻页、hover 菜单项 → 面板位置正确、点击与悬停命中不偏移、窗口不超出屏幕。
11. **U-15 动画**：点 ⋮ 展开 → 面板淡入 + 从候选栏方向滑入（约 0.13s，先快后缓）；再点收起 → 反向淡出滑出、播完窗口才缩回；动画播放中快速连点 → 不卡死、不跳变，直接落到目标态；打字上屏时面板直接消失（不打动画）。
12. **U-16 快捷键**：记事本打字窗口中按 **Ctrl+`** → 候选栏可见时面板应展开（带动画）；再按一次 → 面板收起；候选栏未显示时按键 → 不应有任何窗口凭空弹出；确认该按键不打字、不残留反引号字符。
13. **三修复复测**：①纵向候选栏点 ⋯ 展开菜单 → 面板右下方不再有大片空白，汉堡按钮（⋯）仍居中在候选条带上、不与面板重叠；②横向候选栏展开面板后再收起 → 候选栏应贴回光标下方原位，不再停在面板展开时的窗口顶部位置；③任意方向展开菜单页 → 底部品牌栏左侧「曦码·曜输入法」加粗、右侧项目地址右对齐，两端对齐、不居中重叠。（2026-11-11 用户实机复测：**通过**；随后按用户要求移除品牌栏，第 ③ 项改为「菜单页底部不再有任何品牌/版权文字，面板高度收紧」。）
14. **二级菜单定位**（本轮新增）：横向候选栏展开菜单 → 面板在候选栏上方、候选条带仍贴住光标 → 切「剪切板」（窗口变高）→ 候选条带应仍钉在光标处、面板向上生长，不盖住正在输入的文字；切「表情」「符号」与返回菜单同理；纵向候选栏切各子页 → 候选条带贴住光标、面板向右生长，不超出屏幕右缘；光标贴近屏幕底部/顶部时切子页 → 面板整体仍完整可见（放不下时整体翻到另一侧）。
15. **构建警告清零**（开发侧，无需用户操作）：`cargo build --quiet` 与 `cargo clippy` 输出无任何 warning（原上游 aes-gcm deprecated ×2 与 TSF 链接 LNK4104 ×4 已消除，见第 10 节）。
16. **U-10 日志治理**：`.\rebuild.ps1` 重启后正常打字一段（含候选上屏、翻页、数字计算）→ 到 `%APPDATA%\xime\logs\` 看 `tsf.log` 与 `server.log` 尾部增量：连续输入 100 键后 tsf.log 增量应 <1MB（原每键 27~47 行，现目标 <5 行/键）、server.log 增量应 <100KB；日志里不再出现完整候选列表转储。
17. **U-28 通知**：横向候选栏点 ⋮ → 表情页点一条 emoji 上屏 → 内容上屏但**不应弹出**「已上屏：🍔」系统通知；剪贴板页点一条历史条目同理；把剪贴板占用（如按住 Ctrl+C 的程序）模拟失败较难，可忽略失败分支。

回退：`git checkout -- crates/winxime-ipc crates/winxime-server crates/winxime-tsf` 三个目录一起回退（IPC 字段与两端使用方必须同版本，单独回退一侧会编译失败）；二级菜单定位只动 `crates/winxime-server/src/ui/view.rs` 的 `relayout_and_repaint`，可单独回退；构建警告修复涉及上游 `D:\project\libximecore\crates\xime-plugin\src\cipher.rs` 与本仓 `crates/winxime-tsf/src/lib.rs`，随各自仓库回退。

## 已知遗留

- U-10：日志治理已编码（第 11 节），待实机确认日志增量；绘制重建与按键→响应 506ms 离群值随后单独一轮。`DECISIONS.md` 2026-09-30 条里「每键重读三份 yaml」的说法已过时（`config::get()` 有 mtime 缓存）。
- 窗口的透明模糊边（四周各 8 DIP）仍会吃掉鼠标点击，点到边缘外框不会穿透到下层应用；本轮未处理。
- 设置页外观两项（候选栏横纵开关、数学计算开关）在 `D:\project\libximecore` 本地落地（`CalculatorConfig.auto_detect` 默认开启 + 外观页两个 switch），本仓经本地 `[patch]` 编译；上游 PR 合入前 CI 不会包含这两个控件。
- 设置页修改面板快捷键（U-16b）：`xime-config` 加 `panel.hotkey` 字段 + 外观页控件，走上游 PR；本期默认 Ctrl+` 硬编码在 TSF 侧。
