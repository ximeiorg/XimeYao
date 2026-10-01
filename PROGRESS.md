# 曦码·曜 (Xime Yao) 五笔输入法 - 进度跟踪

## 当前状态
- ✅ cargo build 零错误
- ✅ Debug/Release 双版本编译
- ✅ 候选栏 UI 正常显示
- ✅ 输入法可用（已添加到系统）
- ✅ MSI 安装包可用
- ✅ GitHub Actions 自动构建

## 已完成功能 (2026-05-08)

### 核心功能
- [x] librime 引擎集成
- [x] IPC 架构 (TSF DLL + Server)
- [x] 候选栏 Direct2D 渲染
- [x] 配置管理模块
- [x] 方向键导航修复
- [x] 候选栏坐标修复（在 ProcessKeyEvent 前同步获取坐标）
- [x] Shift 键切换中/英文
- [x] 系统托盘图标（嵌入 ICO 文件）
- [x] 托盘显示中/EN 状态图标
- [x] 托盘左键点击切换中/英
- [x] 托盘右键菜单（设置、退出）
- [x] 切换输入法时自动显示/隐藏托盘图标
- [x] 任务栏按钮点击切换中/英文
- [x] 状态同步：输入法启动/切换时正确显示当前中/英状态
- [x] **ITfCompartmentEventSink 实现（监听输入法切换，已打开应用立即生效）**

### 2026-05-09 新增
- [x] **修复候选栏背景截断问题**
  - 问题：候选词少于5个时背景右边被截断，无圆角
  - 原因：窗口大小预留空间固定为25像素，但阴影需要16*scale像素
  - 解决：添加 BLUR_RADIUS 常量，窗口大小改为 `(width + blur_radius * 2) * scale`

- [x] **ITfThreadMgrEventSink 实现（修复已打开应用切换输入法不生效问题）**
  - 问题：从其他输入法切换到当前输入法时，已打开的应用不触发 StartSession
  - 原因：缺少 `ITfThreadMgrEventSink::OnSetFocus` 接口实现
  - 解决：添加 `ITfThreadMgrEventSink` 接口，在文档焦点变化时触发 start_session

- [x] **架构重构：XimeTextService 直接实现 ITfKeyEventSink**
  - 参考 windows-chewing-tsf 项目架构
  - 移除独立的 KeyEventSink 结构
  - 在 Activate 时一次性注册，永不重新注册

- [x] **修复按键双重处理 bug (P0)**
  - 问题：OnTestKeyDown 和 OnKeyDown 都调用 process_key，按键被处理两次
  - 修复：OnTestKeyDown 只做 should_handle_key 检查，不调用 process_key

- [x] **修复 OnSetFocus 焦点处理 (P0)**
  - 问题：两个分支做相同事情，没有区分焦点丢失/获得
  - 修复：pdimfocus.is_null() → focus_out + 清除 composition；非 null → focus_in + start_session

- [x] **移除所有 unwrap() 调用**
  - winxime-tsf 和 winxime-core 已零 unwrap/expect
  - 改用 `lock().unwrap_or_else(|e| e.into_inner())` 容忍 mutex 中毒

## 已验证
- [x] 候选栏第一个字母位置正确

### 2026-08-15 品牌名更新
- [x] 品牌名改为「曦码·曜 (Xime Yao)」
  - [x] 文档标题（README/AGENTS/PROGRESS/DECISIONS）
  - [x] 调用 libximecore 的 metadata（`RimeEngine::new("Xime Yao")`、`resources/xime.yaml`）
  - [x] TSF 注册名 / 语言栏 / DLL 注册名（中文「曦码·曜」）
  - [x] 设置窗口标题、MSI/MSIX 安装包显示名、Release 标题

### 2026-08-15 按键处理对齐 weasel (librime)
- [x] **libximecore `crates/librime/src/key.rs` 键码映射修复**
  - [x] `vk_to_xk` 补充 OEM 标点键映射（VK_OEM_1→XK_SEMICOLON、VK_OEM_7→XK_APOSTROPHE、VK_OEM_4/6→XK_BRACKETLEFT/RIGHT、VK_OEM_MINUS/PLUS→XK_MINUS/EQUAL、VK_OEM_COMMA/PERIOD→XK_COMMA/PERIOD、VK_OEM_2/5/3→XK_SLASH/BACKSLASH/GRAVE、VK_CAPITAL→XK_CAPS_LOCK）
  - [x] 新增常量：K_LOCK_MASK、VK_CAPITAL/SHIFT/CONTROL/MENU、VK_OEM_*、XK_* 标点 keysym
  - [x] `get_key_modifiers(is_key_up: bool)` 补充 Caps Lock 检测（LOCK_MASK）与按键释放（RELEASE_MASK）
  - [x] 单元测试：test_vk_to_xk_oem_punctuation / test_vk_to_xk_letters_lowercase / test_vk_to_xk_misc（11 项全部通过）
- [x] **TSF 层移除硬编码选词/翻页拦截，改由 rime 配置驱动**
  - [x] `handle_key_event` 删除数字 1-9 选词、`;`/`'` 选词、`[`/`]`/`-`/`=`/Tab/Shift+Tab/PgUp/PgDn 翻页的直接 IPC 调用
  - [x] 这些键现在统一走 `process_key(xk, mods)`，由 rime 的 `key_binder`（default.custom.yaml: semicolon→2、bracketleft/right→Page_Up/Down、Tab→Page_Down 等）处理
  - [x] 新增 `handle_key_up_event`：非 Shift/Ctrl 的 KeyUp 转发给 rime（带 RELEASE_MASK），供 ascii_composer 使用
  - [x] `OnTestKeyUp` 同步返回 `should_handle_key` 结果，保证 `OnKeyUp` 能被 TSF 调用
  - [x] `get_key_modifiers` 仅调用一次（不再在按键时刻前后重复取异步状态）
- [x] `cargo check` 零错误；libximecore `cargo test -p librime key` 全部通过

### 2026-08-15 CI 构建修复 (librime-sys2 build.rs)
- [x] **修复 `vswhere failed: os error 123`（CI 源码构建路径）**
  - 问题：`find_vswhere` 用 `Command::new("where")` 输出 `.trim()` 直接作为路径，但 `where vswhere` 可能输出多行（PATH 多个匹配），中间换行符未去除 → `Command::new` 收到非法路径（ERROR_INVALID_NAME）
  - 解决：改为逐行解析 `where` 输出，取第一个 `exists()` 的文件路径；候选路径兜底不变
  - 额外：vswhere 返回的 VS installationPath 校验非空且目录存在，避免写入无效 `vcvars64.bat` 路径
  - 验证：`cargo check -p librime-sys2`（debug/release）零错误（本地因预编译 rime.dll 走跳过分支，CI 源码构建路径由修复逻辑覆盖）

### Server 后台运行
- [x] 单实例检测 + 自动停止旧进程
- [x] `/q` 命令停止
- [x] RegisterApplicationRestart (Windows 自动重启)
- [x] DPI 感知
- [x] Debug/Release 条件编译
- [x] UI 主线程创建（修复消息处理）

### 设置程序
- [x] winxime-setup (GPUI UI)
- [x] 基础设置界面
- [x] 状态管理模块 (Entity<SettingsState>)
- [x] 组件回调支持 (Switch/NumberInput/Button)
- [x] 关于页面 (版本、作者、仓库、许可)
- [x] 菜单图标 (SVG)
- [x] 标题栏左侧与侧边栏颜色一致
- [x] 菜单选中背景色改为主色

### 安装部署 (新增)
- [x] winxime-tsf-register 工具 (TSF 注册)
- [x] MSI 安装包 (WiX v3.14)
- [x] GitHub Actions CI/CD
- [x] SignPath 代码签名配置
- [x] package-release.ps1 打包脚本

## 架构

```
winxime-tsf.dll         → TSF 输入框架 (注册到系统)
winxime-server.exe      → 候选栏 + Rime引擎 (后台运行)
  - Debug: 有控制台窗口 (1.09 MB)
  - Release: 无控制台窗口 (447 KB)
winxime-setup.exe       → 设置界面
winxime-tsf-register.exe → TSF 注册工具 (MSI 安装用)
```

## GitHub Actions

- `.github/workflows/ci.yml` - 构建 MSI
- `.github/workflows/code-signing.yml` - SignPath 签名
- `.github/workflows/release.yml` - 发布流程

## 使用方式

### 开发调试
```powershell
cargo run                     # 启动 Server (有日志)
cargo run -p winxime-server -- /q  # 停止 Server
cargo wix --package winxime-server --bin-path "C:\Program Files (x86)\WiX Toolset v3.14\bin"  # 构建 MSI
```

### 本地安装
```powershell
# 方式1: MSI 安装 (需管理员)
msiexec /i target\wix\winxime-server-0.1.0-x86_64.msi

# 方式2: dist 目录安装
.\dist\install.bat  # 管理员运行
```

### SignPath 签名配置
1. 注册 SignPath.io 组织
2. 创建项目 `winxime`
3. 配置签名策略 `release-signing`
4. 添加 GitHub Secrets:
   - `SIGNPATH_API_TOKEN`
   - `SIGNPATH_ORGANIZATION_ID`

## 设计决策

### winxime-setup 配置交互方案 (2026-05-09)
参考项目分析：
- **weasel (小狼毫)**：`WeaselDeployer.exe` 通过 IPC + librime API 交互
  - `StartMaintenance()` → Server 暂停服务
  - 修改 Rime 配置文件
  - `rime->deploy()` → 重新部署
  - `EndMaintenance()` → 恢复服务
- **windows-chewing-tsf**：注册表 + 自动重载
  - 配置存储在 `HKCU\Software\ChewingTextService`
  - TSF DLL 通过 `reload_if_needed()` 检测变化

**最终方案**：采用 `xime.custom.yaml` 配置文件方式
- 配置路径：`%APPDATA%\Xime\xime.custom.yaml`
- winxime-setup 修改配置文件
- winxime-server 通过 librime API 加载，定期检测变化重载
- 交互方式（待定）：文件监听 或 IPC `ReloadConfig` 命令
- UI 设计要符合 fluent design

## 下一步
 - [x] winxime-setup UI 完善进度
   - [x] 状态管理模块
   - [x] 基础组件回调
   - [x] 关于页面
   - [x] 菜单图标
   - [x] 实现配置持久化 (保存到 xime.custom.yaml)
   - [x] 配置项分组细化
   - [x] 标题栏全局部署按钮
 - [x] 实现 xime.custom.yaml 配置读写
   - [x] librime-sys levers API 绑定
   - [x] RimeConfigManager (UI 配置管理)
   - [x] SchemaManager (输入方案管理)
   - [x] deploy_all() (重新部署功能)
   - [x] 自动创建用户配置文件 (%APPDATA%\Rime)
 - [x] Server 配置加载
   - [x] winxime-server/config.rs 模块
   - [x] config_open("xime") 读取 build/xime.yaml
   - [x] 应用到 CandidateModel (字体、颜色)
 - [x] 部署功能优化
    - [x] 标题栏全局部署按钮
    - [x] 部署结果反馈（标题栏显示消息）
 - [x] Server 配置重载机制
    - [x] IPC ReloadConfig 命令 (winxime-ipc)
    - [x] ipc_server.rs 处理 ReloadConfig → eng.deploy()
    - [x] winxime-setup 部署后调用 IpcClient::reload_config()
- [x] **方案级详细设置 (2026-05-12)**
     - [x] SchemaConfigManager (rime_config.rs)
     - [x] 读取方案配置 (speller/translator/reverse_lookup/tradition)
     - [x] 保存方案配置到 schema.custom.yaml
     - [x] InputSchemaState 添加 schema_config 字段
     - [x] 输入方案页面展示选中方案的详细设置
     - [x] SettingsGroup 组件渲染方案配置分组
  - [x] **日志系统重构 (2026-05-15)**
     - [x] 使用 tracing 替换原来的 log crate
     - [x] winxime-core: init_logging() 支持组件名参数
     - [x] winxime-server: 使用 tracing + init_logging_with_console()
     - [x] winxime-tsf: 使用 tracing::debug!
     - [x] winxime-tsf/language_bar.rs: 使用 tracing
- [x] winxime-server/tray.rs, ui.rs, ipc_server.rs: 使用 tracing
   - [x] **按键绑定实现 (2026-05-16)**
      - [x] key_binder: 分号选词、方括号/Tab翻页
      - [x] ascii_composer: commit_code 行为 (切换时提交编码)
      - [x] switcher: IPC 命令 (GetSchemaList, SelectSchema)
    - - [ ] 下一步
      - [ ] switcher: Ctrl+0 弹出方案选择菜单 (需要 UI)
      - [ ] punctuator 标点符号映射（键码映射修复后已可命中，需验证标点上屏）
      - [ ] recognizer 英文识别模式
      - [ ] menu.page_size 配置读取

### 2026-07-12 修复
- [x] **修复焦点事件风暴导致无法输入中文 (P0)**
   - 问题：三个 TSF sink (`ITfKeyEventSink`、`ITfThreadFocusSink`、`ITfThreadMgrEventSink`) 在同一个焦点转换时分别独立触发 IPC 调用
   - 同步 IPC 在 STA 线程阻塞时引发消息泵送 → 重入的 FocusOut → `abort_composition()` 清除输入状态
   - 解决：
     - 合并 focus 处理到 `ITfThreadMgrEventSink::OnSetFocus`，其他两个 sink 改为 no-op
     - 添加 `processing_focus` 重入保护
     - `show_tray_icon`/`hide_tray_icon` 添加幂等保护（`tray_visible` 标志）
     - 移除 `activate_impl` 中的冗余 `start_session()` 调用

### 2026-06-14 新增
- [x] **引入 librime-octagram / librime-lua / librime-lua-deps 插件**
   - 添加 `plugins/librime-octagram` 和 `plugins/librime-lua` 为 git submodule
   - `build.rs` 构建前自动复制插件到 `librime/plugins/` 并安装 Lua 5.4 第三方依赖
   - CI workflow 同步更新：插件缓存及构建步骤
   - `find_vswhere()` 改为通过 PATH 或候选路径查找，不再硬编码

### 2026-09-11 候选栏菜单面板（参考 macOS 版 XimeYi）
- [x] **候选栏右侧 "⋮" 菜单按钮 + 下方可展开面板（横向布局）**
  - 菜单页：2 列 × 4 行功能卡片（📋剪切板 🚀快捷发送 🧮计算器 😀表情 🔣符号 🎙️语音输入 ⚙️设置）
    + 底部品牌栏「曦码·曜输入法」
  - 子页面 v1 为占位（粗体标题 + 「← 菜单」返回 + 「功能开发中」，与 macOS 版占位页一致）
  - 「设置」项已接通：启动 winxime-setup.exe（main.rs 抽取 `launch_setup()`，托盘菜单共用）
  - 交互：点击 ⋮ 展开/收起；卡片 hover 高亮 + 手型光标（TrackMouseEvent/WM_SETCURSOR）；
    输入新内容（WM_UPDATE_CANDIDATE）或候选栏隐藏时自动收起并复位到菜单页
  - 单元测试 6 项（菜单布局/行槽不压底部栏/返回按钮矩形/命中路由/坐标换算/页面 id 覆盖），全部通过
  - 备注：竖排布局无菜单入口（与 macOS 一致）；表情/符号/剪贴板等子页实际功能为后续功能点

### 2026-09-11 rebuild.ps1 改为「安装效果」测试流程
- [x] **rebuild.ps1 重写：测试路径 = 安装路径（参考 msix-bundle.ps1）**
  - 旧流程的问题：cargo run 从 target\debug 直接启动（Debug 版带控制台黑窗口），
    COM/Profile 注册指向 target\debug 路径，与真实安装（System32 DLL + 包目录）割裂
  - 新流程：release 构建（windows_subsystem=windows，无黑窗口）→ 复用 msix-bundle.ps1
    按安装布局暂存（binaries + rime.dll + data + user-data + resources + AppxManifest）→
    `Add-AppxPackage -Register` 松散文件开发注册（= 安装效果）→ 从注册的包目录
    target\msix-pkg 启动 server（等价于 MSI 的 StartServer 动作）
  - 非管理员运行时自动 UAC 提权重启（-File 重跑自身，新窗口 -NoExit 保留输出），
    重启后 Set-Location 锚定仓库根（提权进程 cwd 是 System32，cargo/msix-bundle 都按相对路径解析）
  - 修复提权后 cwd 分裂：Set-Location 只改 PowerShell 当前位置（cmdlet/外部 exe 用它），
    而 [System.IO.File] 等 .NET API 读进程级 cwd（提权进程默认 system32），导致 manifest
    写到 system32\target\msix-pkg 失败；msix-bundle.ps1 头部现在同时锚定两级目录到脚本根
  - %APPDATA%\Xime 用户数据跨重建保留；日志在 %TEMP%\winxime\*.log
- [x] **msix-bundle.ps1 -Register 分支修复**
  - 原来注册后删除 target\msix-pkg：松散文件注册的包内容就指向该目录，删除等于注册出空壳包
  - 现保留暂存目录（等价于真实安装的 WindowsApps 目录常驻磁盘），并增加注册失败检测（exit 1）
- [x] **winxime-tsf-register 定性（未废弃）**
  - MSI 安装/卸载仍依赖它：main.wxs 自定义动作 RegisterTSF（-copy-and-register，拷 DLL
    到 System32+注册）/ UnregisterTSF（-unregister-and-remove）/ StopServer
  - full-uninstall.ps1 依赖它；MSIX 流程不用它（server 的 register.rs::ensure_registered 自我注册），
    msix-bundle.ps1 打包时只是带着它的 exe 但从不调用（后续可从包清单中移除）

### 2026-09-11 重构：ui.rs 拆分为 ui/ 目录模块
- [x] 原 ui.rs（约 1720 行，职责混杂）按职责拆为 6 个文件，行为不变：
  - `ui/mod.rs`（约 200 行）：`CandidateWindow` 对外 API（show/hide/update/show_root/hide_root）、
    面板状态字段、对外消息常量、共享布局常量
  - `ui/model.rs`（约 160 行）：`CandidateModel`/`RootModel`/`RenderedMetrics` 数据模型
  - `ui/layout.rs`（约 240 行）：DirectWrite 文本测量与布局计算（自由函数，传入工厂而非 self）
  - `ui/paint.rs`（约 580 行）：候选栏/字根提示 D2D 绘制；
    顺带把候选栏与字根提示两处逐字重复的高斯模糊投影块合并为 `draw_drop_shadow()`
  - `ui/view.rs`（约 660 行）：窗口/交换链/合成设备创建 + 全部 `wnd_proc` 消息处理（含面板鼠标交互）
  - `ui/panel.rs`（约 710 行）：菜单面板（上一功能点已建，迁入 ui/ 目录，路径 `crate::ui::panel`）
  - 对外接口不变：`ipc_server.rs` 的 `crate::ui::CandidateWindow` 与 `main.rs` 的 `ui::panel::*` 照常工作
  - `cargo check` 零错误（仅剩拆分前就存在的旧代码 dead_code warning）；6 项单测全部通过
### 2026-09-11 数据目录修复（对齐 Xime 单目录模型）
- [x] **修复「MSIX 安装后方案丢失/数据目录不对/设置里方案列表为空」**
  - 根因 1：`xime_config::get_data_dirs()` 无人调用 `set_rime_paths()`，Windows 回退 Unix
    路径（HOME 未设 → `C:\.config\xime\rime`），设置程序的方案列表/打开数据目录/SchemaManager
    全部扫错目录
  - 根因 2：AppxManifest 缺 `unvirtualizedResources`，MSIX 把 `%APPDATA%\Xime` 虚拟化到包
    LocalCache，包外进程（资源管理器、宿主内 TSF DLL）看不到
  - 根因 3：旧 `ensure_user_config_files` 只要用户目录有任意 .yaml 就永久跳过方案部署
- [x] 修复内容：
  - xime-config `default_rime_paths()` 增加 Windows 分支：单目录模型 shared == user ==
    `%APPDATA%\<config_dir>\rime`（对齐 Xime 的 userDataDir == sharedDataDir）；Unix 分支不变
  - server `get_data_dirs()` release 分支改单目录模型；部署函数重写为 Xime 语义
    （`ensure_rime_data`：rime 目录无 *.schema.yaml 视为首装 → 全量复制安装目录 data/ +
    user-data/；升级 → 仅覆盖内容有变化且文件名不含 "custom" 的文件，保护用户定制）
  - AppxManifest 增加 `<rescap:Capability Name="unvirtualizedResources" />`
  - market_dir 改与真实用户目录同级（修复原先落到 `C:\.config\xime\market` 的错位）
  - xime-config 环境依赖的坏测试改为临时目录 fixture（密封测试）
- [x] 验证：xime-config 5/5、xime-plugin 27/27、winxime-server 6/6，release 构建零错误

### 2026-09-11 插件系统（plugins-core 宿主接入 + 云备份/剪贴板同步）
- [x] **背景**：libximecore 已有平台无关的 `xime-plugin` crate（mlua Lua54 沙箱运行时、
  manifest/capabilities 解析、PluginManager 安装/启停、host.http/crypto/json/config 等 host API），
  与 Xime（Android）的 plugin-core Lua 插件契约逐字对齐
- [x] **libximecore 扩展**：
  - `PluginRuntime` 补 backup 契约封装：`backup_push/pull/list/delete`（二进制备份包经
    Lua string 往返，与 Android LuaBackupPluginAdapter 一致），新增二进制往返单测
  - `PluginManager` 补 `install_from_dir`（安装随宿主分发的解压态内置插件，保留启用状态）
- [x] **winxime-server 宿主接线**（新模块 `plugins.rs` + `clipboard.rs`）：
  - 启动时安装 `resources/plugins/` 内置插件 → `%APPDATA%\Xime\plugins`，加载全部已启用插件
  - 内置插件：`webdav-backup`（云备份）与 `webdav-clipboard-sync`（剪贴板同步），源码取自
    Xime 仓库 plugins/ 目录，随 resources 打包进 MSIX/安装目录
  - 云备份：宿主打包 zip（rime 目录全部文件、跳过 build/，条目前缀 `rime/`，与 Xime 布局
    一致）→ 插件 WebDAV PUT；恢复：按条目写回 rime 目录（enclosed_name 防穿越，跳过
    `_xime_backup/` 元数据）；托盘菜单新增「立即云备份」
  - 剪贴板同步：消息窗口 `WM_CLIPBOARDUPDATE` 监听本地变化 + 30s 定时拉取；三通道去重
    （当前 hash / 上次推送 / 远端写回），写回走系统剪贴板从而进入输入法剪贴板历史；
    Profile JSON 与 ximed 同构（snake_case），阻塞 HTTP 全部派发到工作线程
- [x] 配置方式（v1，设置 UI 为后续功能点）：手工创建
  `%APPDATA%\Xime\plugins\config\<plugin-id>.yaml`，webdav-backup 键：url/username/
  password/remote_path；webdav-clipboard-sync 键：davUrl/remotePath/username/password
- [ ] 后续功能点：设置程序插件中心页（启停/配置表单 getSettingsSchema/备份列表）；
  IPC 插件命令；`_xime_backup/` 设置与插件配置恢复；候选栏剪贴板/备份入口卡片接线

### 2026-09-12 下载数据目录对齐安卓 Xime（插件/方案市场/模型）
- [x] **目录映射总表**（安卓 filesDir ↔ Windows %APPDATA%\Xime，见 DECISIONS.md）：
  - 方案市场包：`files/market/{id}/` ↔ `%APPDATA%\Xime\market\<id>\`（P0 已对齐）
  - 插件：`files/plugins/{id}/` ↔ `%APPDATA%\Xime\plugins\<id>\`（已对齐，注册表格式为
    Rust 平台实现 registry.yaml，目录布局一致）
  - 模型：`files/models/{modelId}/` ↔ `%APPDATA%\Xime\models\<modelId>\`（新增 `models.rs`
    固化约定：models_root/model_dir/ensure_model_dir/is_model_downloaded/delete_model，
    对齐安卓 ModelStorage/ModelManager 语义——文件如实命名、存在且非空才算已下载、
    用到才建目录、模型独立于插件管理；模型下载功能本身待 ASR/联想后端接入）
  - 市场注册表：安卓在数据根（files/.registry.json），Windows 从 market/.registry.yaml
    移到 `%APPDATA%\Xime\.registry.yaml`（market/ 只存下载包）
  - 下载临时文件：安卓约定 cache/xime_plugin_{id}_{fileName} 即用即删，Windows 对齐为
    `%TEMP%\xime_plugin_{id}_{fileName}`（plugins.rs `plugin_download_temp_path`，
    供后续插件市场下载使用）
- [x] 验证：winxime-server 测试 8/8（含 models 目录 2 项），release 构建零错误

### 2026-09-12 盘根残留目录清理（C:\.config\xime）
- [x] 旧 bug 残留的 `C:\.config\xime`（HOME 未设时 Unix 回退路径拼到盘根产生）已清理：
  - 其中 `models/ochwpro`（6.8MB 手写模型，模型中心经旧路径下载的真实数据）已迁移到
    `%APPDATA%\Xime\models\ochwpro\`，与联想模型 predictive-text-small 并列
  - 其余（rime/build 部署产物、installation.yaml、空 market）为可再生垃圾，随目录删除
- [x] 代码层面确认：全部路径经 `xime_config::get_data_dirs()`（Windows 分支 → %APPDATA%\xime），
  Unix 回退已 cfg(not(windows)) 隔离，setup 的模型/市场/插件目录不会再写盘根；
  设置程序（xime-setup-lib）cargo check 通过

### 2026-09-12 修复 MSIX 开发注册同版本重复注册失败（0x80073CFB）
- [x] 现象：第二次 `rebuild.ps1` 起必现「提供的程序包已安装，且禁止重新安装」
  （Add-AppxPackage -Register 拒绝同 Identity+Version 的重复注册）
- [x] 修复：`msix-bundle.ps1 -Register` 注册前按 manifest 的 Identity.Name 移除旧的开发注册
  （Get-AppxPackage → Remove-AppxPackage）再重新注册；独立调用时先停包内进程
  （winxime-server/winxime-setup）避免移除被文件占用阻塞

- [x] **修复：剪贴板历史不记录（未启用同步插件时）**——历史逻辑原先在
  剪贴板同步工作线程里，而该线程只在同步插件启用时启动。重构为
  **worker 常驻**：历史始终记录（SQLite），同步插件运行时按选型可选加载
  （worker 持 `Option<PluginRuntime>`，推送/拉取按需执行）；选型变更仍
  停旧起新（`worker_started` AtomicBool + 插件 id 比对）。补回归测试
  clipboard_worker_records_history_without_sync_plugin（无插件 LocalChanged
  仍落库）+ 恢复误删的 clipboard_selection_follows_clipboard_sync_toml；
  14/14

### 2026-09-29 插件配置值加密（对齐 Android SecureValueCipher）
- [x] **背景**：安卓端插件配置全值加密（Keystore AES-GCM，`enc:` 前缀 +
  base64(iv+密文+tag)，认证失败视为无效，明文兼容回退）；Windows 端
  host.config 值为明文 YAML，WebDAV 密码同机任意程序可读（%APPDATA% 按
  用户划界不按应用划界，已核对本机 ACL）
- [x] **libximecore 新增 `xime-plugin/src/cipher`**（同算法同密文格式）：
  - AES-256-GCM；密钥 32 字节随机生成，经 **DPAPI(CryptProtectData)**
    加密存于数据目录 `secret.key`（DPAPI = Windows 对应 Keystore 的角色，
    按用户绑定、文件离机不可解）；密钥文件损坏不覆盖（避免误清密文）
  - `encrypt_with_key_path / decrypt_with_key_path`；密钥路径由配置文件
    路径推导（plugins/config/<id>.yaml → 数据目录/secret.key）
  - 非 Windows：恒等实现（行为与旧版一致，Linux daemon 不受影响）
  - 单测 3 项（往返+明文兼容、篡改密文认证失败、密钥文件非裸密钥）
- [x] **两端接线**：xime-plugin runtime 的 load_config/save_config（host.config
  读写层）与 xime-setup 的 read/write_plugin_config + start_schema_load
  值读取全部走加解密；**存量明文配置在首次保存时随全量写入自动升级密文**
  （读取侧明文兼容，无迁移动作也不会丢数据）
- [x] 验证：cipher 3/3、winxime-server 14/14、debug/release 零错误

### 2026-09-30 安装对齐 Android：真实方案 id 发现 + 已安装列表即时刷新
- [x] **包 id ≠ 方案 id**：此前把包 id（如 rime-ice）直接塞进 schema_list，
  rime 不认识 → 方案装了却不生效。对齐 Android `installPackageFromMarketDir`：
  从释放的顶层 `*.schema.yaml` 提取真实方案 id（优先取与包 id 规范化后
  同名者），校验前置（无 .schema.yaml 拒装），安装即切换（启用列表替换为
  新方案，对齐 Android switchEnabled 语义）
- [x] **已安装列表不刷新**：`available_schemas` 只在启动时 load 一次且
  `schemas_loaded` 幂等挡板拦住重载；InstallDone/UninstallDone 只更新商店
  installed_ids。新增 `reload_schemas()`（无挡板强制重载），安装/卸载
  完成即刷新列表
- [x] 验证：构建零错误、libximecore 测试全绿

### 2026-09-30 停止分发 librime minimal 示例数据（用户 rime 目录污染源）
- [x] **根因**（用户对比 rime-wubi 发现多余文件、删除重装仍在）：
  `msix-bundle.ps1` / `msi-build.ps1` 把 `libximecore/librime/data/minimal`
  （librime 自带示例：cangjie5 / luna_pinyin / essay.txt / default.yaml /
  symbols.yaml）当"rime base data"打进安装包，`ensure_rime_data` 首装
  全量拷入用户 rime 目录；rime 目录里另有 market 安装的 rime-ice 全套
  （build/、cn_dicts/ 等属其正常内容，但 default.yaml 覆盖与 build/ 释放
  已由安装过滤器修复）
- [x] 修复：两个打包脚本移除 minimal 拷贝并清空暂存 data/（防历史残留）；
  rime-wubi（user-data/）自包含 default.yaml / symbols.yaml，无功能依赖；
  msi-build 顺带移除孤儿 `Find-LibrimeRoot`
- [x] 另：do_install 增加 `is_protected_release_path` 过滤——市场包不得
  释放 default.yaml 等宿主/引擎自有文件、`build/` 部署产物、`*.userdb/`
- [x] 验证：构建零错误、libximecore 测试全绿；PS1 语法校验通过
- [x] 用户操作：`.\rebuild.ps1` 重打包后删一次 `%APPDATA%\xime\rime`
  再启动，目录即只剩 rime-wubi 内容 + 运行产物

### 2026-09-30 系统通知改为 server 代弹（IPC ShowToast）
- [x] **setup 进程无包身份**：MSIX 清单只声明 `winxime-server.exe` 一个
  应用入口；设置程序直跑（非 server 派生）时 `GetCurrentPackageFullName`
  拿不到身份 → toast 无从归属被系统拒绝，且 setup.log 无失败日志（静默）
- [x] 方案（用户直觉验证成立：要走 IPC）：新增 `ShowToast` IPC 命令
  （ToastMessage{title,body}）+ `IpcClient::show_toast`；server 持有
  toast.rs（WinRT 实现，有包身份）代为弹出，失败记 server.log
- [x] setup 的 `toast::show_toast` 改为 **IPC 优先**，失败回退本进程直弹
  （server 派生启动时有身份的场景）；server windows crate 增
  Data_Xml_Dom / UI_Notifications / Win32_Storage_Packaging_Appx /
  Foundation features
- [x] 验证：构建零错误、winxime-server 18/18

### 2026-09-30 修复「部署失败：deploy returned 0」——deploy 语义误读
- [x] 根因（librime 源码 rime_api_impl.h 确认）：`api->deploy` 是
  `RimeStartMaintenanceOnWorkspaceChange`——`installation_update` /
  `detect_modifications` 判定**无变化时返回 0，是"无需维护"不是失败**；
  levers 的 `deploy_all_with_config` 把 0 当错误抛出
- [x] 修复：`xime_config::deploy_all` 改为显式全量维护（对齐 weasel
  「重新部署」）：`start_maintenance(full)` + `join_maintenance_thread`，
  之后补跑 `deploy_config_file(xime.yaml)`（幂等）；不再使用 OnWorkspaceChange
  语义的 api->deploy 做成败判定
- [x] 日志佐证：setup.log 无部署条目（错误在 UI 层产生），server.log 的
  启动 "deployment failed" 是另一处 DeployResult 通知未捕获的存量问题，
  不影响功能，后续单独处理
- [x] 验证：构建零错误、libximecore 测试全绿

### 2026-09-30 修复「部署方案」完全没有反馈
- [x] **toast 从未弹出的真凶**：`package_aumid()` 把 Win32 两段式调用的
  第一段（空缓冲取长度，正常返回 `ERROR_INSUFFICIENT_BUFFER`）误判为
  「非打包环境」直接返回 None——toast 永远静默跳过。修正：仅
  `APPMODEL_ERROR_NO_PACKAGE` 视为非打包
- [x] **页内消息从未显示**：`show_message` 只发宿主回调，而 winxime-setup
  从未注册 `set_notify_message`——「正在部署…」「部署成功」等全部落空。
  修复：show_message 写入 `ui_message`（Instant 时间戳），app 视图顶部
  渲染全局消息条（主色底、5 秒经 BackgroundPoll 过期）
- [x] 设置进程接日志：`init_logging_with_console("setup")` →
  `logs\setup.log`（toast 失败等此前 eprintln 进黑洞的诊断信息可见）
- [x] 验证：构建零错误、winxime-server 18/18、libximecore 全绿

### 2026-09-30 选中方案记忆 + 启动不再覆盖用户弃用的 builtin 方案
- [x] **打字时自动切回第一个方案**：engine 的会话选中完全没持久化——
  redeploy/deploy 重建会话、server 重启都回落 schema_list 第一个。修复：
  - `RimeEngine` 记住 `selected_schema`，`redeploy()`/`deploy()` 重建会话后
    自动重新选择
  - server：SelectSchema 成功后写数据根 `selected_schema.txt`；启动时
    deploy 完读回并恢复（重启也不丢）
- [x] **启动覆盖用户方案**：`ensure_rime_data` 升级路径此前强更所有非
  custom 文件。修复：读用户启用列表（default.custom.yaml 的 `- schema:`
  行），**未启用的 builtin 方案文件**（`<id>.schema.yaml/.dict.yaml`，
  id 属于安装目录 builtin 集合）不再强更——不覆盖用户自己的方案；
  共享资产（essay/symbols/lua/default.yaml）照常更新；首装全量不变
- [x] **卸载压扁启用列表**：`do_uninstall` 此前把启用列表写成只剩第一个
  剩余方案。修复：保留全部剩余启用方案（get_schema_list_ids 过滤），
  全空才回退首个现存方案
- [x] 验证：构建零错误、winxime-server 18/18、libximecore 全绿

### 2026-09-30 方案安装隔离（对齐 Android installPackageFromMarketDir）+ 部署按钮 toast
- [x] **部署按钮无通知**：「部署方案」（DeploySchemas）此前只更新页面底部
  消息；现成败两路接 `notify_deploy_toast`（与安装/卸载一致）
- [x] **安装隔离（此前致命缺口：只拷 .schema.yaml，词典/lua 全丢，多方案
  文件混在 rime 根目录、卸载删不掉）**。`do_install` 重写为对齐 Android：
  1. **全量释放**——归档内容全部进 rime 目录（保留相对路径，含词典/lua），
     损坏包解压失败即弃（对齐 validateArchive）
  2. **冲突检测**——目标文件已被其他包占用 → 拒绝安装并报冲突来源
     （同包重装允许覆盖；对齐 detectConflicts）
  3. **安装清单**——按包写数据根 `.registry.yaml`（`<pkg>: files: [...]`，
     与 server SchemaManager 同格式）；卸载侧（上一轮已接数据根）据此
     精确删除本包文件，跨包不再互相污染
- [x] 注意：修复前已混装的旧方案没有清单，仍卸载不干净（历史数据无法
  追溯归属），重新安装一次即可获得清单
- [x] 验证：构建零错误、libximecore 测试全绿

### 2026-09-30 修复设置程序三处 UI 冻结（启动 / 部署按钮）
- [x] **启动卡死**：`SettingsState::new()` → `load_schemas()` →
  `SchemaManager::new()` → `init_rime_deployer()` 内置
  `start_maintenance(true)+join`（全量部署，rime-ice 数秒）跑在 UI 线程。
  修复：初始化只做 setup+initialize+create_session（毫秒级），部署一律走
  显式 `deploy_all()`（调用方已后台线程）；server 启动时本就维护 build/
- [x] **部署按钮卡死**（输入方案「部署方案」/ 快捷键「重新部署」同一条
  `DeploySchemas` 路径）：`poll_deploy` 里的 `notify_daemon_reload()` 是
  同步 IPC，server `eng.redeploy()` 数秒期间 UI 冻结。修复：daemon 重载
  挪进 `start_deploy` 的后台线程（部署→重载→组合文案一并返回），
  `poll_deploy` 只展示结果字符串（DEPLOY_RESULT 类型改为 Result<String,String>）
- [x] 验证：构建零错误、libximecore 测试全绿

### 2026-09-30 日志目录收敛到数据根（%APPDATA%\<name>\logs）
- [x] `get_log_dir()` Windows 分支原为 `%TEMP%\<name>\`（TEMP 清理会丢日志、
  排障时也想不到去那找），改为数据根 `logs\` 子目录（与用户数据同处）；
  TEMP 兜底保留；Unix 分支不动
- [x] 说明：`clipboard_sync.toml` 是同步选型文件（开关写入、server 30s
  轮询读取），非垃圾——删除等于关掉剪贴板同步
- [x] 数据根全景盘点确认已聚合：*.toml/*.db/*.key 平铺 + market/models/
  plugins/rime/logs 子目录；唯一约定性例外是 %TEMP% 下载缓存（即用即删，
  对齐 Android cache/，DECISIONS 已记录）
- [x] 验证：构建零错误

### 2026-09-30 方案部署结果系统通知（WinRT toast）
- [x] **架构结论**：部署发生在 setup 进程（`init_rime_deployer` 在调用进程
  初始化 librime），结果就在 setup 手里；server 热载的成败也在 IPC 应答
  现场——**不需要新增 IPC**。toast 是 Windows 专属，不能进 libximecore
  （跨平台库），落在 winxime-setup 宿主
- [x] libximecore：`set_notify_deploy_toast(f: fn(&str, &str))` 平台无关
  钩子；安装/卸载线程的成败两路触发（含失败原因）
- [x] winxime-setup：`toast.rs`——WinRT ToastNotification（ToastGeneric 模板，
  XML 转义），AUMID 动态取 `GetCurrentPackageFullName()!XimeServer`；
  非打包环境（开发直跑）静默跳过；后台线程弹（先 CoInitializeEx MTA）
- [x] windows crate 增 features：Data_Xml_Dom / UI_Notifications /
  Win32_Storage_Packaging_Appx / Win32_System_Com
- [x] 验证：构建零错误、libximecore 测试全绿

### 2026-09-30 修复输入方案「已下载」列表为空
- [x] 根因：`input_schema.rs::scan_market_dir()` 是第三套路径——release 扫
  **exe 同级目录** `market`（MSIX 安装目录，不存在）、debug 扫仓库
  `target\debug\market`，而商店下载落在数据根 `market\`（上一条修复后）
- [x] 修复：改为复用 `state::market_dir()`（开为 pub(crate)），目录常量
  至此唯一；实机验证数据根下已有 `market\rime-ice` 包
- [x] 验证：构建零错误、libximecore 测试全绿

### 2026-09-30 修复方案市场路径/注册表与 DECISIONS 声明的漂移
- [x] setup 侧 `markets_dir()`（复数 `markets\`）改为 `market_dir()`
  （单数 `market\`），与 server SchemaManager 及 DECISIONS「下载数据目录
  映射」收敛为同一目录；5 处调用点（下载包目录/已装列表/安装/缓存清理）
  一并生效；过时注释（`~/.config/xime/markets/`）修正
- [x] setup 卸载的注册表从 `markets\.registry.yaml`（无人写入的孤儿文件）
  改为数据根 `.registry.yaml`（server 安装时写入的位置）——修复卸载
  找不到已装文件清单、根注册表条目残留的问题
- [x] 实机无历史数据（两条路径均未安装过），无迁移成本
- [x] 验证：构建零错误、libximecore 全部套件通过

### 2026-09-30 词典管理（对齐 weasel DictManagementDialog）
- [x] **librime 封装**：用户词典函数在 **levers API**（非主 API），levers.rs
  新增 list_user_dicts / backup_user_dict / restore_user_dict /
  export_user_dict / import_user_dict；lib.rs 补 get_user_data_sync_dir
- [x] **IPC**：ListUserDicts / BackupUserDict / RestoreUserDict /
  ExportUserDict / ImportUserDict 五命令 + DictResponse（dicts/count/
  sync_dir）挂 IpcResponse.dict_response；server handler 调 librime
- [x] **设置词典页**（原占位页重写）：用户词典列表（每项 备份/导出/导入）+
  恢复快照（rfd 原生文件对话框，对齐 weasel 恢复流程）+ 快照目录展示 +
  刷新；操作结果（含导出/导入条数）经后台线程 + BackgroundPoll 回显；
  回调注册 set_notify_dict_*（host 包 IpcClient）
- [x] 依赖：libximecore workspace 加 rfd = "0.15"（Windows 原生文件对话框）
- [x] 验证：构建零错误、winxime-server 18/18

### 2026-09-30 修复托盘菜单「第一下无效」
- [x] 根因：TrackPopupMenu 后未补 `PostMessage(WM_NULL)`（KB135788），菜单
  跟踪未正确结束，下一次点击被当作取消吞掉；对齐 weasel SystemTraySDK
  的三件套（SetForegroundWindow → TrackPopupMenu → WM_NULL）
- [x] 验证：构建零错误、winxime-server 18/18

### 2026-09-30 托盘菜单渲染当前方案 switches（对齐 Android menubar）
- [x] **解析**：`schema_switches.rs` 读 `<rime>/<当前方案>.schema.yaml` 的
  switches 块（根目录优先、build/ 产物兜底），结构对齐 Android
  SchemaSwitch——布尔开关（name + states 两态标签）/ 多选一开关（options
  轮转 + states）；字符串简写条目跳过（与 Android 一致）；4 个单测
- [x] **托盘**：菜单改为弹出时全量重建（`build_menu`），「用户资料同步」与
  「关于」之间插入 switches 分组——布尔开关显示当前态标签 + 勾选，多选一
  显示激活标签（点击轮转）；ascii_mode 跳过（与顶部「切换中/英」重复）；
  无 switches 时不渲染该组
- [x] **切换**：`TrayAction::ToggleSwitch{name, options}` → engine 取反 /
  options 循环 setOption（对齐 Android toggleSchemaSwitch；未做 user.yaml
  持久化，后续可接 librime levers）
- [x] 验证：构建零错误（新代码无告警）、winxime-server 18/18

### 2026-09-30 托盘移除「立即云备份」
- [x] 托盘菜单删「立即云备份」项（TrayAction::BackupNow / MENU_ID_BACKUP /
  main.rs 分支一并移除）；备份功能保留在设置 → 同步与备份页，
  `PluginHost::backup_now` 公共 API 与单测不动（后续 IPC/插件中心可用）
- [x] 验证：构建零错误、winxime-server 14/14

### 2026-09-30 语音转文本设置页（v1：Windows WinRT 听写）
- [x] **架构对齐 Android xime speech 模块**：`RecognitionState` 状态机
  （Idle/Listening/Processing/Error）+ 后台 worker 独占引擎 + 共享结果槽
  （Android 回调 → Rust UI 250ms 轮询 `SpeechSink`）；页面在「智能」组
- [x] **v1 后端选型**：sherpa-rs 仅离线封装无流式识别器，故 v1 接 Windows
  自带 `SpeechRecognizer` 连续听写（零新模型/依赖，麦克风系统托管）；
  后端抽象保留，后续接与 Android 同款的本地 zipformer（sherpa-onnx sys）
- [x] 实现：`speech.rs`（worker 线程 MTA + 听写约束 Dictation + 约束编译
  一次复用 + ResultGenerated 逐短语追加 + 轮询等待异步，windows-future 0.3
  无阻塞 get）+ `SpeechState`（toggle/poll/clear/copy_text）+ 3 消息
  （SpeechToggle/SpeechClear/SpeechCopy）+ `pages/voice.rs`（状态行/结果面板/
  复制到剪贴板 arboard/清空）+ mic.svg 图标
- [x] 接线：`voice-page` feature（winxime-setup 启用）；MSIX 清单加
  `microphone` DeviceCapability；VoiceHandle Drop 时停会话收尾
- [x] 验证：构建零错误（新代码无告警）；libximecore 全部套件通过
- [ ] 后续：本地离线模型后端（sherpa-onnx zipformer，与 Android 同模型源）；
  IME 面板语音按钮直通

### 2026-09-30 rime 用户资料同步（对齐 weasel「用户资料同步」）
- [x] **定位重整**：原「云备份」是整包快照（tar.gz 覆盖式恢复），rime
  `sync_user_data` 是词库快照导出+多端合并（sync/<installation_id>/），
  两者正交。设置导航「云备份」→「同步与备份」，页首新增用户资料同步卡片
- [x] IPC：`SyncUserData` 命令（winxime-ipc）→ server 调
  `librime::sync_user_data()` + `join_maintenance_thread()`（对齐 weasel
  Configurator::SyncUserData；server 同进程免维护模式切换）
- [x] 托盘菜单「用户资料同步」（MENU_ID_SYNC，走 IPC 回环与设置同路径）
- [x] 设置页：用户资料同步卡片（本机标识 / 快照目录 / 上次同步相对时间 /
  立即同步）；`RimeSyncState` 解析 installation.yaml + sync 目录设备列表
- [x] 回调：`set_notify_sync_user_data` 注册到 `IpcClient::sync_user_data`
- [x] 验证：构建零错误；libximecore 全部套件通过；winxime-server 14/14
- [ ] 后续：sync/ 目录上云（WebDAV 插件承载，与 Android 同目录约定）

### 2026-09-29 快捷发送卡片对齐历史卡 + 两列表翻页
- [x] **样式统一**：提取 `card_button` 共用组件（点击选中 / 主色边框 / hover），
  快捷发送卡与历史卡完全同款——删除按钮仅选中时出现（此前常驻右上）
- [x] **翻页**：两列表每页 8 条（2 列 × 4 行），页脚分页条（上一页 / 第 x / y 页 /
  下一页，首末页置灰禁用；单页不显示）；state 加 `page` + total_pages/clamp/
  prev/next（列表缩减后自动夹回），4 个翻页消息；单测 `list_pagination_pages_and_clamps`
- [x] 验证：构建零错误、xime-setup 12/12

### 2026-09-29 剪贴板同步「配了但不推送」修复
- [x] **根因**：`clipboard_sync.toml` 不存在——「启用剪贴板同步」开关从未打开
  （填插件配置表单只写 plugins/config/<id>.yaml，不写选型文件）；服务端日志
  全程无 ReloadPlugins、worker 显示「同步插件=未启用」，推送静默跳过
- [x] 服务端自愈：`clipboard_poll_remote`（30s Tick）先 `sync_clipboard_worker`
  对齐选型，开关打开后无需重启 IME 即生效（与 IPC ReloadPlugins 互为兜底）
- [x] 推送失败补告警日志（此前插件返回 false 静默）
- [x] 设置页：同步未启用时保存配置即提示「请先打开启用开关」
- [x] 验证：构建零错误、winxime-server 14/14

### 2026-09-29 操作按钮并入 Tab 栏（space-between 页头）
- [x] 历史页（刷新/清空历史）与快捷发送页（添加）的操作按钮从列表前的
  「操作」行上移到**页头 Tab 栏右侧**：`clipboard_header` = Tab 栏 +
  `Space(Fill)` + 操作行，同一行 space-between 布局（同步页无操作按钮）
- [x] 顺带更新空态文案（「刷新」「添加」位置改为右上角）
- [x] 验证：构建零错误

### 2026-09-29 快捷发送卡片样式与历史卡统一（选中交互）
- [x] 快捷发送卡片改为与剪贴板历史同款：**点击选中**（主色 1.5px 边框 +
  背景加深 + hover 反馈），「删除」按钮仅在选中时出现（此前常驻卡片右上）；
  常显的删除钮移除后卡片内容为标题（文本前缀）+ 内容摘要（含编码标记）
- [x] 结构：`QuickSendState.selected: Option<i64>` + `select`；
  `QuickSendSelected(i64)` 消息 + 分发
- [x] **操作工具栏移至表头**：历史（刷新/清空历史）与快捷发送（添加）的
  操作按钮从列表尾部的「操作」行改为列表上方的工具栏行；顺带修正快捷发送
  分组描述（存储已是 clipboard.db，不再是 quick_send.yaml）
- [x] 验证：构建零错误、14/14

### 2026-09-29 剪贴板历史卡片：选中交互 + 操作按钮
- [x] **交互**：点击历史卡片选中（主色边框高亮 + 背景加深），再点取消；
  选中时卡内出现「添加到快捷发送」「删除」两个按钮
- [x] **实现**：
  - store：`ClipboardHistoryItem` 增加 `id` 列（list 查询带 id），新增
    `remove_history_item`（按 id 删单条）
  - state：`ClipboardHistoryState.selected: Option<i64>` + `select`（点击
    切换）/`remove`（删库 + 刷新 + 清选中）；`QuickSendState.add_from_text`
    （历史文本直接加为快捷发送，无触发编码）
  - 消息 3 个：ClipboardHistorySelected / ClipboardHistoryRemove /
    QuickSendFromHistory；卡片改为 button（点击 + hover 反馈），选中态
    主色 1.5px 边框
- [x] 验证：构建零错误、14/14

### 2026-09-29 修复（二次）：OpenClipboard 传监听窗口句柄而非 NULL
- [x] **日志实锤的新矛盾**：`OpenClipboard(None)` 返回成功（无重试耗尽警告），
  但紧随的 `GetClipboardData` 报 `ERROR_CLIPBOARD_NOT_OPEN`（1418「线程没有
  打开的剪贴板」）且 `EnumClipboardFormats` 为空——打开状态在两调用之间
  无效，指向 `OpenClipboard(NULL)` 在窗口消息循环线程上的关联不可靠
- [x] **修复**：新增 `LISTENER_HWND` 静态句柄（start_listener 创建监听窗口
  后记录），`open_clipboard_with_retry` 改传 `OpenClipboard(Some(监听窗口))`
  ——**传真实窗口句柄是剪贴板管理器的常规做法**（NULL 关联在消息循环
  上下文中的行为 quirk 规避）；重试逻辑保留
- [x] 验证：构建零错误、14/14；效果待 rebuild 后复制确认（诊断日志仍保留：
  若仍有问题，warn 会给出错误码与实际格式枚举）

### 2026-09-29 修复：剪贴板历史仍为空（链路断点=OpenClipboard 竞态）
- [x] **诊断**（链路足迹日志实锤）：复制时「剪贴板事件触发: Changed」有日志、
  「剪贴板变化进入宿主」无——断在 `read_text()`：WM_CLIPBOARDUPDATE 到达时
  来源应用可能仍持有剪贴板锁，`OpenClipboard` 一次失败即放弃 → 事件静默丢弃
- [x] **修复**：`read_text`/`write_text` 的 `OpenClipboard` 加 5 次 × 10ms 重试
  （`open_clipboard_with_retry`，Windows 剪贴板读取的常规做法），重试耗尽打
  warn 日志
- [x] 顺带发现：worker 日志中 db 路径为小写 `xime`（与实际目录 `Xime` 大小写
  不一致；NTFS 不区分大小写，功能无影响，属命名不一致待统一）
- [x] 验证：构建零错误、14/14；效果待 rebuild 后复制确认（链路日志全量
  足迹：事件触发 → 进入宿主 → worker 收到 → 历史已记录）

### 2026-09-29 剪贴板存储迁移 JSON/YAML → SQLite（对齐 Android clipboard.db）
- [x] **背景**：安卓端剪贴板历史与快捷发送共用 SQLite（Room clipboard.db
  v4，表 clipboard_entries，快捷发送即 isQuickSend=1 子集，含触发编码
  code 列）；Windows 端此前用 clipboard_history.json / quick_send.yaml，
  与安卓 schema 不通
- [x] **libximecore 新增 `xime-config/src/clipboard_store`**（rusqlite
  bundled，工作区已声明 0.32）：
  - 建表语句与 Android Room 实体逐列对齐（id/text/code/timestamp/
    isPinned/isQuickShare/isQuickSend/consumed/type/imagePath/imageHash/
    mimeType/sizeBytes/width/height + text/imageHash 索引），库名同为
    clipboard.db（%APPDATA%\xime\），WAL 多进程安全
  - API：append_history（同文本去重移前 + 容量裁剪，快捷发送条目不受
    历史裁剪波及）/ list_history / clear_history / list_quick_send /
    add_quick_send / remove_quick_send / migrate_legacy
  - **旧 JSON/YAML 自动迁移**：server 与设置程序任一首次打开时幂等迁移
    （导入后改名 *.migrated）
  - 单测 3 项（去重移前+容量裁剪、快捷发送增删+清空保留、旧文件迁移）
- [x] **两端接线**：server worker 历史写入改走 store（截断 2000 字符）；
  设置程序历史/快捷发送状态全部改走 store（删除 JSON/YAML 读写）；
  **对话框从「名称/内容」改为「内容/触发编码」**（对齐安卓 QuickSendItem
  {id,text,code,timestamp,isPinned}——无独立名称列，列表标题取文本前缀，
  触发编码是安卓的真实功能：输入编码前缀条目进入候选栏）
- [x] 验证：xime-config 8/8、winxime-server 12/12、debug/release 零错误

### 2026-09-29 剪贴板页改版：Tabs 结构（历史 / 快捷发送 / 同步）
- [x] 页面重构为三个 Tab（样式对齐扩展商店页 tab_bar）：
  「剪贴板历史」（默认）、「快捷发送」、「剪贴板同步」；页标题改「剪贴板」
  （与侧栏菜单项一致）
- [x] 结构：view() 拆为 tab 栏 + 三个内容源（history_groups /
  quick_send_groups / sync_groups），`SettingsState.clipboard_tab` 记忆当前
  Tab，`Message::ClipboardTab(usize)` 切换
- [x] **添加快捷发送改为弹窗**（iced 0.14 无内置 Modal，widgets.rs 新增
  `modal_dialog` 通用组件：stack + 半透明遮罩 + 居中卡片，项目内可复用）：
  列表页只留「添加」按钮 → `QuickSendOpen` 弹出对话框（名称/内容 +
  取消/添加），确认成功自动关闭（内容为空保持打开），取消清空草稿
- [x] 验证：debug/release 构建零错误、14/14
- [x] **历史/快捷发送列表改 2 列卡片网格**（iced 0.14 Grid：columns(2) +
  height(Shrink)）：新增 `list_card` 通用单元样式（浅前景底色圆角卡）；
  历史格 = 截断文本；快捷发送格 = 名称行（semibold + 撑开 + 删除按钮）
  + 内容摘要；「刷新/清空/添加」操作项保持全宽

### 2026-09-29 设置页：剪贴板历史 + 快捷发送展示与编辑
- [x] **剪贴板历史**（文件契约，零 IPC 改动，与 clipboard_sync.toml 同模式）：
  - server 剪贴板工作线程每次本地复制/远端写回时持久化
    `%APPDATA%\Xime\clipboard_history.json`（读改写：内容去重移到最前
    （对齐 Windows 历史语义）、容量 50 条、单条截断 2000 字符；文件为
    唯一事实源，设置页清空 = 写空文件，server 下次追加自然接续）
  - 设置页「剪贴板历史」分组：最近 8 条（截断 60 字符展示）+ 刷新/清空按钮
    （接通上游预留的 `Message::ClearClipboardHistory`）
- [x] **快捷发送**：`%APPDATA%\Xime\quick_send.yaml`
  （`items: [{name, content}]`）——设置页「快捷发送」分组：列表（名称+内容
  摘要+删除）+ 新增草稿（名称留空取内容前 12 字符）；后续输入法候选栏
  「快捷发送」面板消费同一文件（host.quickSend 上游暂为占位）
- [x] 结构：xime-setup 新增 `ClipboardHistoryState/QuickSendState`
  （cfg clipboard-page 门控）+ 5 个 Message 变体 + update 分发；
  server 新增 `record_clipboard_history`（worker 线程内调用）
- [x] 测试 14/14（新增 clipboard_history_persists_and_dedups：追加去重
  移前 + 清空接续）；debug/release 构建零错误

### 2026-09-29 设置程序 ASCII 符号乱码修复（用户截图实锤定位）
- [x] **现象**：剪贴板页所有含 ASCII 的文字渲染成错误符号
  （"Web"→"Ⓐ−▼"、"Android"→"A■_↓X"、"30"→"←¯"），中文全部正常；
  字符数一一对应（非缺字形豆腐块），per-char 映射基本确定
- [x] **根因**：设置程序未指定具体字体，text 控件用 iced 通用族
  （Sans Serif）交给 fontdb 在系统字体中解析；该机器上通用族解析命中
  **图标字体**（Segoe Fluent Icons 类，也被归类为 sans-serif）——图标
  字体把 ASCII 码位映射成符号字形；CJK 不被图标字体覆盖、回退雅黑，
  所以只有拉丁/数字乱码
- [x] **修复**（libximecore xime-setup）：`components/widgets.rs` 新增
  `UI_FONT = Font::with_name("Microsoft YaHei UI")`（Windows 全版本自带、
  拉丁+中文覆盖完整），medium()/semibold() 改为基于 UI_FONT；
  app.rs `run()` 加 `.default_font(UI_FONT)`——所有未显式设字体的
  text 控件（含 pick_list/输入框/button）统一走雅黑 UI，通用族解析
  彻底不再参与
- [x] 验证：构建零错误、13/13；效果需 rebuild.ps1 后打开设置确认

### 2026-09-29 候选栏菜单面板留白修复
- [x] **问题**：菜单面板上下留白特别多——菜单页顶部预留了 36px 标题栏 + 8px
  间距但菜单页不画标题（「← 菜单」是子页面才有）→ 顶部空 44px；底部品牌栏
  32px 只有一行小字。内容卡片仅占 232px 面板中的 140px（60% 是留白）
- [x] **修复**（ui/panel.rs）：
  - 菜单页行槽改 `panel_menu_row_y`：从 `PANEL_MENU_TOP(10)` 起，不再预留标题栏
  - 面板高度 232 → 198（10 + 4 行卡片 140 + 间距 8 + 品牌栏 32 + 底边距 8），
    窗口高度经 `panel_extra_height` 自动跟随
  - 子页面占位文本改在「标题栏底 ↔ 品牌栏顶」间居中（原先引用菜单行几何）
  - 测试同步（行槽断言改新函数 + 新增首行紧贴顶部断言）；顺手清掉
    libximecore clipboard.rs 在 Windows 下的两个 unused import 警告
  - 验证：构建零错误、winxime-server 13/13

### 2026-09-29 修复：中文态 Shift+符号键无法上屏（如打「问题」后 Shift+/ 出不来 ？）
- [x] **根因**：`vk_to_xk(vk)` 无 shift 概念，Shift+/ 发给 rime 的是
  `XK_SLASH + SHIFT`；而 X11/weasel 语义是上报**移位后的字符 keysym**
  （`XK_question`），librime 的 punctuator/key_binder 按 '?'、'(' 等字符
  keysym 登记 → 永远匹配不上 → 按键被吞、无输出。数字行同理（Shift+9 的
  （、Shift+1 的！等全部失效）
- [x] **修复**（libximecore crates/librime/src/key.rs）：
  - `vk_to_xk(vk, shift: bool)`：shift=true 时数字行（0-9）与 OEM 键
    （;=,-./[\]' 共 11 键）返回移位字符 keysym（新增 21 个 XK_* 常量，
    ASCII 可见字符 keysym == ASCII 码）；无移位字符的键忽略 shift
  - 字母键不变：基键保持小写，大小写语义由 SHIFT 修饰位承载（X11 语义）
  - 新增单测 `test_vk_to_xk_shifted_printables`（22 项断言）；librime key
    测试 12/12 通过
- [x] **TSF 调用点**（text_input_processor.rs）：`handle_key_event` /
  `handle_key_up_event` 传入 `mods & K_SHIFT_MASK as i32 != 0`，
  key-up 与 key-down 同规则（配对一致）
- [x] **修复：Shift+符号键误触发中英切换**：OnKeyUp 对 VK_SHIFT 无条件
  toggle_ascii_mode，Shift+/ 松开 Shift 即切换。加 `shift_solo` 单按判定：
  Shift 按下置位，期间任何其他键按下/抬起（含 VK_CONTROL 早退路径之前）即
  清除；Shift 抬起时仅在仍置位时才切换（对齐 weasel「空按 Shift」语义）
- [x] **修复：中文态 Ctrl/Alt 组合键失效**（如 Ctrl+A/C/V/F5，英文态正常）：
  `should_handle_key` 不看修饰键，中文态对字母/数字/符号键一概认领 →
  应用跳过自身加速器路径 → rime 不处理（无 Ctrl/Alt 绑定）→ 按键丢失。
  修复：Ctrl 或 Alt 按住时不认领任何非修饰键（修饰键本身豁免——组合中
  Ctrl 按下仍需进入 OnKeyDown 触发字根提示 show_root）
- [x] **修复：中文态回车失效**（server 日志实锤：不组词时 rime 对回车
  `handled:false`）：同一类病——`should_handle_key` 不组词也认领回车/
  退格/Esc/Tab/空格/数字/翻页/方向键，但这些键只在组词中被 rime 消费
  （上屏原始码/删码/选候选/翻页/移光标）。修复：这些键不组词时不再认领
  （交应用原生处理）；字母（起始组词）与标点键（punctuator 上全角）保持
  认领。**认领原则沉淀：只在 rime 会处理的键上认领，认领 = 承诺消费**
- [x] **按键层重构：对齐 weasel KeyHandler 架构，根除认领启发式**
  （对比 weasel-0.17.4 WeaselTSF/KeyEventSink.cpp）：
  - weasel 无任何认领猜测——OnTestKeyDown 即完成整个按键处理（IPC 询问
    rime），rime 说吃才吃；OnKeyDown 只重放结果；`_fTestKeyDownPending/
    _fTestKeyUpPending` 应对怪异应用（多次 TestKeyDown / 只调 KeyDown，
    如 QQ、Word）
  - 本层同构实现：`process_key_event(context, vk, is_up)` 统一处理
    （合并原 handle_key_event/handle_key_up_event），四个 On* 入口全部
    改为 pending 重放模式；**删除整个 should_handle_key 启发式**——
    此前回车/Ctrl+A 两类丢键 bug 的根源（认领 = 猜测，猜错即丢键）从
    结构上消除，rime 拒绝的键天然交还应用
  - 保留的本地决策：修饰键（Shift/Ctrl/Alt 单按）不经 rime——Shift 中英
    切换走本层 shift_solo、Ctrl 走 show_root；英文态本地短路不询问 rime
    （对齐 weasel keyboard-open 检查）；shift 移位 keysym 转换与 weasel
    ToUnicodeEx 语义一致（移位后字符作 keysym）
  - 与 weasel 的已知差异（后续对齐项）：Caps Lock 事件不转发 rime
    （weasel 转 Caps_Lock 给 ascii_composer，含双按还原 SendInput 逻辑）；
    小键盘 VK_NUMPAD 映射 ASCII 数字而非 KP_*；VK→字符转换用静态美式
    布局表而非 ToUnicodeEx（非美式键盘布局移位字符可能不准）
- [x] **修复 CI 构建**：CI 用 git 依赖 libximecore a06f864（.cargo/config.toml
  本地 patch 不入库），其 vk_to_xk 是单参——移位映射从上游 key.rs 挪回
  winxime-tsf 本地（vk_to_xk_shifted/vk_to_xk_with_shift 包装 +
  VK_OEM_* 常量），libximecore 本地 key.rs 改动已回退（上游推送移位支持
  前本地/CI 编译路径一致）；代码只依赖上游已发布 API
- [x] 教训：严禁用 PowerShell Get-Content/Set-Content 改 UTF-8 源码
  （PS5.1 按 GBK 读写，中文注释全部乱码且可能吞换行）；改源码一律用
  Edit/Write 工具

### 2026-09-29 适配 libximecore：插件运行时 mlua(Lua) → quickjs-rusty(JS)
- [x] **libximecore 拉取**（a2853e6 → a06f864），关键变化：
  - 插件运行时迁移 QuickJS，契约对齐 xime 3.0 Android `JsScriptRuntime`
    （manifest.json 优先兼容 yaml、入口 main.js、`globalThis.plugin` 分组命名空间、
    契约调用硬超时 + 超时熔断、网络门禁 fail-closed）
  - backup 契约改名并类型化：`backup_push/list/pull/delete` →
    `push_backup → BackupUploadResult{ok,id,message}`、`list_backups → Vec<RemoteBackupEntry>`、
    `pull_backup → Option<Vec<u8>>`、`delete_backup → bool`；
    clipboard `push/pull` 返回值由 `Option<..>` 扁平化
  - **PluginRuntime 不再是 Send**（QuickJS 裸指针）：运行时必须活在创建线程内
  - host.http 底层 ureq → reqwest blocking（支持 PROPFIND/MKCOL，非 2xx 也返回响应对象）
- [x] **winxime-server 宿主线程模型重构**（plugins.rs）：
  - backup 类操作改为「一操作一实例」：调用线程内 `PluginRuntime::load` + `call_on_load`
    + 执行 + 即弃（对齐 libximecore setup 侧 `load_plugin_runtime` 模式）
  - 剪贴板同步改为**专用工作线程**独占持有运行时，宿主经 mpsc 投递
    `LocalChanged/PollTick` 命令；三通道去重状态（当前/上次推送/自写回显）归线程所有，
    不再加锁；远端拉取命中后由工作线程直接写回系统剪贴板
  - main.rs 剪贴板回调从「每事件 spawn 线程」简化为「非阻塞投递命令」
- [x] **内置插件迁移 Lua → JS**（源码取自 Xime 仓库 plugins/ 的 xime 3.0 版，
  libs 内联为单文件）：
  - `resources/plugins/webdav-backup`：manifest.json（id 不变，配置无缝延续；
    platforms + windows）+ main.js（backup.test/push/pull/list/remove + settings.schema）
  - `resources/plugins/webdav-clipboard-sync`：manifest.json + main.js
    （clipboardSync.push/pull/test + ETag 条件拉取 + 503 限流退避 + 附件 blobs 契约
    （桌面端休眠））；旧 main.lua/manifest.yaml 已删除（force 安装会清空旧目录完成迁移）
- [x] **构建环境**：quickjs 的 `libquickjs-ng-sys` 需要 bindgen(libclang) + clang 编译 C 源码，
  本机原先无 LLVM → scoop 用户级安装 llvm 23.1.2；`.cargo/config.toml`（未跟踪）
  增加 `[env] LIBCLANG_PATH / TARGET_CC` 指向 scoop LLVM
- [x] 测试：winxime-server 12/12（新增 4 项：backup_now 走真实 QuickJS 运行时跑通
  契约调用、无插件时报错、剪贴板线程启动门禁、clipboard_sync.toml 选型）；
  libximecore xime-plugin 43/43（1 忽略）
- [x] **剪贴板功能接入设置程序（对齐上游 7d22192「选中即启用并通知 daemon」）**：
  - winxime-setup 启用 xime-setup-lib 的 `clipboard-page` + `backup-page` feature
    （此前未启用，设置程序里看不到剪贴板/云备份页——「怎么没有剪切板功能」的根因）
  - winxime-ipc 新增 `ReloadPlugins` 命令 + `IpcClient::reload_plugins()`
  - winxime-server ipc_server 处理 ReloadPlugins → `PluginHost::reload()`
    （重扫启用清单 + 按 `clipboard_sync.toml` 选型对齐剪贴板工作线程：
    应启未启→启动、选型变更→停旧起新、应停→Shutdown）
  - 宿主遵守 `clipboard_sync.toml`（setup 写入，与 Android daemon 契约共享）：
    enabled + plugin_id 精确选中同步插件；无 toml 时不启用（明确 opt-in）
  - main.rs 插件宿主创建提前到 IPC 线程启动之前（IPC 需引用）
- [x] libximecore 小改：剪贴板页「同步服务器（xime-sync-server）」分组
  cfg(target_os="linux") 门控（Windows 端不分发该服务，隐藏以免误导；
  server_groups 抽成函数解决非 Linux 下闭包类型推断失败）
- [x] **「奇怪符号」真正根因：候选栏 ⋮ 菜单面板的 emoji 图标渲染成方框**
  （用户口中的"剪切板页面"即面板第一张卡片"📋 剪切板"）：panel.rs 用用户候选
  字体渲染 emoji 字符，中文字体无 emoji 字形 → 方框。修复：图标改用系统
  "Segoe UI Emoji" 字体 + `D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT`
  （Win10+ 彩色字形），标签文字仍用候选字体
- [x] 次要修复：剪贴板设置页 helpText 中的 `→`（U+2192）改为纯中文表述
  （iced 字体回退同样可能缺字形；运行时 dump schema 全字段排查，其余字符串
  均干净；manifest 的 ☁️ icon 不被设置 UI 渲染。iced 无法渲染彩色 emoji，
  第三方插件 schema 文案含 emoji 会显示方框——上游待办）
- [x] **msix-bundle.ps1 加固：被占用旧包目录改名让位**——rebuild 时旧
  winxime_tsf.dll 常被仍开着输入法的宿主进程映射（不允许删除/覆盖，
  允许改名）：Remove-Item 失败时整目录改名 `msix-pkg.old-<时间戳>` 让位
  再重建，历史让位目录下次构建自动清理；避免半删除半复制的脏暂存状态
- [ ] 后续功能点：设置程序插件中心页对接新契约（settings.schema/启停通知）；
  插件市场下载（install_from_zip + plugin_download_temp_path 已就绪）；
  候选栏剪贴板/备份入口卡片接线

### 2026-09-30 输入方案安装隔离闭环（builtin 方案包 + 冲突确认 + 精确卸载 + 备份还原）
- [x] **问题**：设置程序「输入方案 → 已下载 → 安装」把第三方方案包全量释放进 rime 根目录，
  第三方方案文件与内置方案（rime-wubi：wubi86*/pinyin_simp/symbols/lua…）混装；内置方案文件在
  注册表里无人认领 → 第三方包可**静默覆盖**它们（本机历史数据里 rime-ice 就覆盖了
  `custom_phrase.txt`、`lua/date_translator.lua`），卸载时又把这些内置文件一并删掉
- [x] **安卓参照**（Xime `SchemaManifestManager` + `SchemaLocalViewModel`）：untracked 文件归
  `builtin` 包 + `market/builtin/` 备份（`ensureBuiltinBackup`/`refreshBuiltinManifest`）、
  `detectConflicts`（claimedBy + sha256，含 builtin）、`uninstallWithManifest`（claimedBy 共享保护）、
  安装前「rime 目录已有其他方案包」→ 弹确认「需要先卸载冲突方案」→ `confirmInstallWithUninstall`
- [x] **libximecore `xime-config::schema_manifest`（新模块，唯一权威实现）**：
  - 数据根 `.registry.yaml`：`<pkg>: {files: [...], sha256: {rel: hex}}`（旧 `files:` 列表向后兼容）
  - `refresh_builtin_package`：无主文件 → builtin 包 + 备份 `market/builtin/`（含 sha256；
    已消失文件撤声明，避免幽灵条目）
  - `detect_conflicts`：异内容冲突 / 同内容视为共享依赖放行 / 同包重装升级放行
  - `uninstall_package`：claimedBy 保护（其他包仍声明的共享文件保留）+ 衍生产物清理
    （`<id>.custom.yaml`、`<id>_merged.dict.yaml`、`build/<id>.*`）+ 空目录剪枝
  - `restore_builtin_package`：从 `market/builtin/` 备份还原内置方案包
  - 路径规则：用户数据（`*.custom.yaml`/`*.userdb/`/`build/`/`themes/`/`sync/`/`installation.yaml`/
    `custom_phrase.txt`）与宿主自有配置（`default.yaml`/`xime.yaml`/`user.yaml`/`squirrel*`/`weasel*`/
    `.registry*`）永不入清单、卸载不删
  - 单测 7 项：登记与跳过用户数据、冲突/共享/同包重装、卸载共享保护与衍生清理、旧注册表兼容、
    备份还原、归属优先第三方包、路径规则
- [x] **xime-setup（设置程序）**：
  - `do_install` 全走清单：refresh builtin → 全量释放（过滤受保护路径）→ sha256 冲突检测 →
    释放 → 写包清单；`do_uninstall(pkg, deploy)` 同样走清单，启用列表按「包名下全部方案 id」移除
    （此前只按包 id 过滤，rime-ice ↔ rime_ice 这类不同名根本删不掉）
  - `install_market_schema` 冲突预检 → 弹窗（`ConfirmSchemaInstall` / `CancelSchemaInstall`）→
    `confirm_schema_install` 先逐个精确卸载冲突包（`deploy=false`，不重复数秒级部署）再安装
  - 「已安装」列表**按方案包分组**（内置方案包在前，标注 内置/第三方 + 每行来源），
    内置方案包被卸载后出现「还原内置方案」卡片（`RestoreBuiltinSchema`）
  - 已安装包列表改以**注册表**为准（此前用磁盘上全部 `*.schema.yaml`，package id ≠ schema id 时判定恒错）
  - 冲突弹窗在 app.rs 全局渲染：输入方案页与扩展商店两个安装入口共用同一隔离流程
- [x] **winxime-server**：启动部署内置数据后 `register_builtin_schema_package()`
  （`ensureBuiltinBackup` + `refreshBuiltinManifest` 的 Windows 对应物），内置方案文件启动即登记+备份，
  安装/卸载都在同一注册表事实上工作
- [x] 验证：`cargo build --quiet` 零错误；xime-config 15/15、xime-setup-lib 13/13
- [ ] 遗留：①server 侧旧 `schema_manager.rs`（IPC `InstallSchema`/`UninstallSchema`）仍是
  「只拷 .schema.yaml」且用 `packages:` 包裹的旧注册表格式，与设置程序实现重复且格式不兼容——
  当前 UI 无调用入口（死路径），待统一到 `xime_config::schema_manifest` 或删除；
  ②历史混装数据无法追溯归属（修复前已被第三方包覆盖的内置文件丢失原内容），
  重新/修复安装后 builtin 备份才完整

### 2026-09-30 选中方案改用 rime 自己的记录（删除数据根 selected_schema.txt）
- [x] **事实**：选中方案本就由 librime 自己持久化——`RimeSelectSchema` → `Engine::ApplySchema`
  → `Switcher::SetActiveSchema` 把 `var/previously_selected_schema`（+ `schema_access_time`）
  写进**用户目录 `rime/user.yaml`**，`Switcher::CreateSchema` 建会话时读回。
  数据根 `selected_schema.txt` 是重复的第二份记录（两处必然不同步）
- [x] **改动**：server 删除 `persist_selected_schema`（不再写 txt）；启动恢复改为
  `load_rime_selected_schema()` 读 `rime/user.yaml` 的 `var/previously_selected_schema`
  （纯函数 `parse_rime_selected_schema`，2 项单测：真实 user.yaml 形态 / 无记录与非法内容返回 None）
- [x] 显式恢复保留的原因：rime 建会话只在 **schema_list 之内**按该字段恢复，而设置程序允许
  选中未启用（不在 schema_list）的方案（`RimeSelectSchema` 按 id 直选不受列表限制）；
  待「选中即写入 schema_list」后这个显式恢复即可一起删掉
- [x] 已清理本机历史残留 `%APPDATA%\Xime\selected_schema.txt`（数据根现只剩
  `.registry.yaml` + 剪贴板/配对/密钥等真实数据文件）
- [x] 验证：`cargo build --quiet` 零错误；winxime-server 新增 2 项单测通过
- 注：本机 DSH 沙箱下 `%TEMP%` 建目录被拒（OS error 5），models/schema_switches/plugins
  共 7 项既有测试失败；已用 `git stash` 对照确认与本次改动无关（改动前同样 7 项失败）

### 2026-09-30 方案来源互斥（rime 目录同一时刻只允许一个来源）
- [x] **要求**：rime 目录里不允许出现多个方案来源（内置方案包 + 第三方包）混装——
  与安卓一致：装第三方方案时先把内置方案包卸掉，而不是两者共存
- [x] **安装路径**（已有）：`install_market_schema` 预检 → 冲突弹窗 → 确认后逐个精确卸载
  其余方案包（`deploy=false`）再安装目标包（只部署一次）= 装完只剩一个来源
- [x] **还原路径**（本次补）：`restore_builtin_schema` 同样走冲突预检
  （新增 `schema_restore_conflict()`：注册表里除 builtin 外的全部方案包），
  确认后先卸载第三方包再还原内置包 → 不允许还原成混装；
  弹窗文案按目标区分（`SchemaInstallConflict::is_restore_builtin()`）
- [x] **历史混装**：**不做界面提示**（对齐安卓——安卓只在安装时提示一次，没有常驻的
  混装告警）。历史混装靠动作自然收敛：装包（预检 → 确认 → 卸掉其余来源再装）、
  还原内置（同样先卸第三方）、卸载（卸掉市场包时 `refresh_builtin_package` 会把无主
  方案文件登记回内置方案包 → 目录里只剩内置一个来源）。
  曾实现「混装告警卡片 + 一键只保留某来源」（`schema_sources` / `KeepOnlySchemaSource` /
  `mixed_sources_card` / `keep_only_schema_source`），按要求**已删除**
- [x] **用户数据安全**：~~不删未登记的 `<id>.custom.yaml`~~ → **改为对齐安卓**：
  `uninstall_package` 按 `uninstallWithManifest`（安卓 333-361 行）删除该方案的
  `<id>.custom.yaml`、`<id>_merged.dict.yaml`、**方案短语表**（`custom_phrase.txt`，
  或 `custom_phrase.user_dict` 声明的 `<名>.txt`）、`build/<id>.*`；
  保留 `*.userdb/` 输入记录与受保护文件（`default.yaml`/`xime.yaml`/`themes/`）。
  短语表名必须在删文件**之前**解析（安卓 305-308 行注释点明：删掉
  `<id>.schema.yaml`/`<id>.custom.yaml` 后就解析不出 `user_dict` 了）——
  第一版顺序写反，被单测抓出
- [x] 新增回归单测 `mixed_sources_converge_to_single_source`：复现本机真实状态
  （内置 wubi86 系列 + 第三方包声明共享 `symbols.yaml`）→ 卸载内置后
  方案文件与 `<id>.custom.yaml` 删除、共享文件/受保护文件/输入记录保留、
  注册表只剩一个来源；再卸载第三方 → 从 `market/builtin/` 还原内置，仍是单来源
- [x] 新增单测 `custom_phrase_dict_name_follows_user_dict_declaration`：块式/行内式/
  补丁式（`custom_phrase/user_dict:`，安卓不认、我们多认）三种 `user_dict` 写法
- [x] 验证：`cargo build --quiet` 零错误；xime-config schema_manifest 9/9 通过
  （xime-setup-lib 3 项 `%TEMP%` 权限类失败是本机沙箱既有问题，与本次改动无关）

### 2026-09-30 打包产物改名 xime → ximeyao（对齐项目名曦码·曜）
- [x] 问题：项目叫 XimeYao（曦码·曜），但打包产物文件名还是 `xime-*`
- [x] 统一命名：`ximeyao-{version}-x86_64.msi` / `ximeyao-{version}-x86_64.msix`
  （MSI 顺手补上缺失的 `-x86_64`，修复 AGENTS.md 文档与实际产物名的漂移）
- [x] `msix-bundle.ps1`：输出路径 + 构建横幅（Building XimeYao (曦码·曜) MSIX）
- [x] `msi-build.ps1`：light -out 路径 + 结果检查路径 + 构建横幅
- [x] `install-msi.ps1`：去掉硬编码 `xime-0.1.0.msi`，改为自动从 Cargo.toml 读版本
  （与其他脚本一致，否则改名后必坏）
- [x] `ci.yml`：light -out、MakeAppx /p、两个 upload-artifact 的 path 通配
- [x] `code-signing.yml`：签名产物改名 `winxime.msi` → `ximeyao.msi`，
  mv 源从硬编码 `winxime-server-0.1.0-x86_64.msi` 改为 `ximeyao-*.msi` 通配
  （原硬编码名与实际产物从来对不上，全靠 `|| true` 掩盖）
- [x] 文档同步：AGENTS.md「MSI 构建」、README 安装命令与输出路径
- [x] 不改的部分（有意）：二进制名 winxime-server.exe 等（涉及 crate/IPC 管道/TSF
  注册，牵一发动全身）；安装目录 `Program Files\Xime`、注册表 `Software\Xime`、
  数据目录 `%APPDATA%\Xime`（改动会孤立既有安装与用户数据）；WiX 产品名本就是
  「曦码·曜」无需动
- [x] 验证：`cargo build --quiet` 零错误；三个 ps1 通过 PowerShell 语法解析检查；
  全仓 grep 无旧产物名残留（release.yml/AppPackageAutoUpdate.yml 用 `*.msi/*.msix`
  通配，不受影响）

### 2026-09-30 修复「恢复默认方案」无路可走（用户实机踩中）
- [x] 问题：用户实机处于半卸载状态——注册表里有个只剩 4 个共享词典、没有任何
  `.schema.yaml` 的**残缺 builtin 条目**；已下载列表按设计不显示 builtin；
  还原卡片条件「注册表里没有 builtin」又被这个残条目挡死 → 无任何恢复默认入口
- [x] **根因 1（还原入口被挡）**：`reload_schemas` 的 `builtin_restorable` 判定放宽——
  「备份存在 && builtin 条目里没有任何 `.schema.yaml`」即可还原（覆盖已卸载与
  残缺两种状态），残条目不再挡路
- [x] **根因 2（卸载最后一个第三方包后不还原内置）**：旧设计只靠
  `refresh_builtin_package` 把**还活着**的无主文件登记回 builtin，被删掉的方案
  文件永远回不来 → `do_uninstall` 补齐：用户主动卸载（`deploy=true`）且启用列表
  清空、注册表无其他市场包时，自动从 `market/builtin/` 备份还原内置包——
  「卸载第三方方案」= 恢复默认，一步到位（`deploy=false` 的冲突预卸载路径不触发）
- [x] **根因 3（还原后仍无法输入）**：还原流程不修启用列表，用户的
  `default.custom.yaml` 还指着已删除的 `rime_ice` → 还原成功后调
  `apply_restored_builtin_schema_list` 把启用列表指向还原出的默认方案
  （`restored_default_schema_id`：wubi86 优先，否则字典序最靠前的顶层方案）
- [x] UI：还原卡片文案改用户视角——「内置方案已卸载或不完整」+ 按钮
  「恢复默认方案」；还原完成消息改为「默认方案已启用」
- [x] 新增单测 `restored_default_schema_prefers_wubi86`（wubi86 优先 /
  无 wubi86 取顶层字典序首个 / 子目录与非方案文件不算）
- [x] 验证：libximecore `cargo build --quiet` 零错误；新单测通过；
  xime-config 17/17；XimeYao `cargo build --quiet` 零错误
  （libximecore 侧改动未提交，XimeYao 经 .cargo/config.toml patch 到本地路径，
  rebuild.ps1 即可验证）

### 2026-09-30 托盘打开设置「慢」定位：大头是图形初始化，不是业务冻结
- [x] **实测方法**（不改代码、不跑程序）：读用户实机
  `%APPDATA%\Xime\logs\setup.log`（12.9 MB / 22.5 万行）最后一次会话的时间戳，
  按毫秒差还原冷启动时间线（日志时间戳是 UTC）
- [x] **实测结果**（17:59:46 那次，本地时间）：
  - 日志就绪 46.202 → 窗口属性 46.205（**3ms**）→ 市场/模型/插件网络线程启动
    46.207（**2ms**）→ 首个渲染（图集分配 + 首帧）47.324 = **+1.12 秒**
  - 其中：wgpu 建 Vulkan 实例 146ms、DX12 +2ms、**GL +85ms**（三套实例 232ms）
    → 枚举适配器 214ms → 选定适配器 212ms → surface 配置 84ms + 367ms
    → 首帧字体图集/文字 60ms
  - **`SettingsState::new()`（librime setup+initialize+create_session、方案列表、
    配色/剪贴板/同步状态、市场线程启动）只占 2~5ms**
- [x] **结论**：之前那类「UI 冻结」（全量部署、daemon 重载跑在 UI 线程）已经不在
  启动路径上了；托盘→设置慢 = 进程创建 + iced/wgpu 图形后端探测 + 窗口/字体初始化，
  与业务数据加载无关。另外 12.9 MB 日志本身是负担：tracing 的**格式化发生在
  写日志的线程**（设置程序里就是 UI 线程），且三方库每帧都在写
- [x] **日志降噪**：`init_logging` / `init_logging_with_console` 默认过滤从
  `debug` 改为 `DEFAULT_LOG_FILTER`——三方库 `info`，本项目 crate 保持 `debug`
  （xime_config / xime_setup_lib / xime_plugin / xime_rime / xime_ipc /
  winxime_server / winxime_setup / winxime_tsf / winxime_ipc）；
  `RUST_LOG=debug` 仍可恢复全量；新增单测校验过滤串可被 EnvFilter 解析且
  本项目 crate 的 debug 指令没漏
- [x] **冷启动分段计时埋点**（INFO，跨进程可对齐）：
  server.log「启动设置程序」（T0）→ setup.log「进程冷启动：日志就绪 +Xms」（T1，
  T0→T1 = 进程创建/装载）→「回调注册完成 +Xms」→「run(): iced 初始化开始」→
  「SettingsState::new() 完成 +Xms」→「首帧构建完成：run() 之后 +Yms」
  （run→首帧 = 图形后端 + 窗口 + 字体，即上面那 1.1 秒）
- [x] **慢帧/慢轮询告警**：`view` 构建或 `BackgroundPoll` 超过 50ms 记 warn
  （250ms 一跳的后台轮询超标就是 UI 卡顿的直接证据）
- [x] 验证：XimeYao `cargo build --quiet` 零错误；libximecore
  `cargo test -q -p xime-config` **18/18**（含新日志过滤单测与 9 项方案清单单测）；
  下一步待用户重启设置程序后读日志确认分段数字（并据此决定是否收敛 wgpu
  后端 / 进程常驻）

### 2026-09-30 埋点复核 + UI 冻结面整体审计（xime-setup / winxime-server / winxime-tsf）
- [x] **埋点复核**（用户重启后的真实 setup.log，18:27:02 本地时间）：
  「进程冷启动：日志就绪 +1ms」→「回调注册完成 +2ms」→「run(): iced 初始化开始」
  →「SettingsState::new() 完成 **+50ms**」→「首帧构建完成：run() 之后 **+1119ms**
  （本帧 view 133.3µs）」→ 结论不变：**打开慢 = 进程创建 + wgpu 后端探测/适配器/
  surface/字体图集 ≈1.07s；业务加载 50ms；view 构建 133µs**
- [x] 抓到一次真实 UI 线程卡顿证据：「后台轮询偏慢：**61ms**（250ms 一跳，超标即
  UI 卡顿）」（18:27:40）——量级是几十毫秒，与下面 P0 的「秒级」冻结不同源
- [x] **审计结论（按严重度）**：UI 冻结的真正来源集中在「点击/按键触发的 handler
  链路里同步做重活」，不在启动路径、也不在页面 view（页面层只有一处每帧磁盘 I/O）

**P0 点击后秒级冻结（UI 线程同步重活，4 处）**
- [ ] `xime-setup/src/state.rs:719` `save_schema()` 回退分支直接
  `rime_deploy::deploy_all_schemas()`（`start_maintenance(1)` + `join`，秒级）。
  触发：`Message::SelectSchema`（点方案行）时服务未运行，**或服务在忙**
  （`ipc_server.rs:174-188` 引擎锁 try_lock 失败 → `success:false` →
  `notify_select_schema` 返回 false）。修：照 `start_deploy()`（state.rs:805）
  的模式丢线程 + 结果槽；`deploy_all` 的注释本就写明「调用方已在后台线程」
- [ ] `xime-setup/src/app.rs:426-436` `Message::RimeSyncNow` →
  `notify_sync_user_data()` 同步 IPC（服务端 `ipc_server.rs:648-657`
  `sync_user_data` + `join_maintenance_thread`，秒级）。附带缺陷：客户端读超时
  只有 100ms（`winxime-ipc/src/pipe.rs:6`），同步慢时表现为「卡几秒还报同步失败」
- [ ] `xime-setup/src/state.rs:3560` / `3576` 安装/还原的冲突预检里
  `refresh_builtin_package()`（`schema_manifest.rs:298 collect_files` 递归 +
  `303 sha256_file` 全量读盘 + `308` 拷贝备份 + `326` 写注册表）**没有短路**，
  而 `reload_schemas`（state.rs:610）有「注册表已有 builtin 就跳过」。修：预检
  同样短路（或整体移入后台线程）
- [ ] `xime-setup/src/state.rs:905/909/916` `poll_market_task()` →
  `reload_schemas()`：`get_schema_list()`（逐 `*.schema.yaml` read+yaml 解析）、
  `load_registry()`、`builtin_backup_files()`（递归扫 market/builtin/）都在
  250ms 轮询回调（UI 线程）里。修：整体后台线程 + 结果槽

**P1 打字链路 / 宿主卡顿（7 处）**
- [ ] `winxime-server/src/ipc_server.rs:174`：引擎锁在 `handle_request` 开头取，
  **整个 match 都持锁**（函数内无 `drop(eng)`）。`SyncUserData`(648)、
  `ReloadConfig`(619)、`FetchSchemaIndex`(1035，网络)、`download_schema`(1077)、
  `plugin_host.reload()`(636) 全在锁内 → 期间 TSF 每次按键 `try_lock` 失败拿到
  `success:false`（按键被丢，tsf 侧还断连重连），托盘 `try_lock` 路径静默无操作。
  修：锁只包引擎调用，网络/解压/维护移出锁外
- [ ] `winxime-server/src/ui/view.rs:258`（每次候选刷新 = 每次按键）→
  `ui/model.rs:96` `XimeConfig::load()` **无缓存**：每次按键重读 + 重解析
  `xime.yaml`（内嵌 + 系统 + 用户三份 + 2 次 merge）。修：进程级配置缓存，
  `ReloadConfig` 时失效
- [ ] `view.rs:252-298` / `ui/paint.rs:242-246`：按键路径上有 13 条
  `info!/debug!`（含候选列表 `{:?}`、metrics、DPI），每次按键都格式化 + 写盘
  （server.log 已 21.2 MB）。修：降为 debug 或删除
- [ ] `ui/paint.rs:249-260`：每次绘制**无条件** `ResizeBuffers` + 重建交换链
  位图；`ui/view.rs:115` 用 `D3D_DRIVER_TYPE_WARP`（软件光栅）；每帧还重建
  `CreateTextFormat`/画刷（paint.rs:262-272、panel.rs:376-406 等）。修：尺寸未变
  跳过重建、资源按参数缓存、驱动改 HARDWARE 失败再回退 WARP
- [ ] `winxime-server/src/main.rs:501-544`：托盘右键菜单 `switch_provider` 在
  **持有引擎锁**期间读并解析 `<id>.schema.yaml`（`schema_switches.rs` 整文件
  `read_to_string` + `serde_yaml::from_str`，无缓存）→ 菜单弹出期间 IPC 拿不到锁
- [ ] `winxime-ipc/src/pipe.rs:58-65` `IpcClient::connect()` 走
  `interprocess` 的 `connect_by_path` = `ConnectWaitMode::Unbounded`
  （`interprocess-2.4.3/src/os/windows/named_pipe/stream/impl/ctor.rs:84`）→
  命名管道实例全忙时 `WaitNamedPipeW(FOREVER)`（`c_wrappers.rs:201` +
  `wait_timeout.rs` FOREVER=0xFFFFFFFF），**连接无超时**。TSF 每次按键都新建
  连接。修：`connect_by_path_with_wait_mode(.., ConnectWaitMode::Timeout(..))`
- [ ] **更正我自己之前的判断**：`READ_TIMEOUT_MS=100`（pipe.rs:6）**不是超时**，
  只是两次 `read` 之间的检查（pipe.rs:92/136）；底层读是
  `ReadFileEx` + `SleepEx(duration_to_timeout(None)=u32::MAX=INFINITE)`
  （`interprocess-2.4.3/src/os/windows/c_wrappers.rs:99-104`、`114-122`、`157-166`），
  **单次系统调用永不超时**——服务端接了连接却不回包时，调用线程（TSF 宿主 UI 线程
  或设置 UI 线程）可以永久挂住。`flush()` = `FlushFileBuffers`（c_wrappers.rs:152-154）
  阻塞到对端读完，写路径同样无界。修：非阻塞 + 总截止时间轮询；超时只丢本次响应、
  **不要**把连接置空（现在 text_input_processor.rs:169-171 会置空 → 下次按键再赌一次
  无界连接）
- [ ] `winxime-server/src/main.rs:492-494`：托盘「退出」在消息线程同步 IPC
  （`send_oneway` 名不副实，也等应答，pipe.rs:128-149）；且服务端
  `ShutdownServer` 分支在 `try_lock` 之后，锁忙时退出彻底失效
- [ ] `winxime-tsf/src/text_input_processor.rs:940` 每次按键（down）都先
  `update_caret_position_sync()`（777）：先 `RequestEditSession`（841，
  `TF_ES_ASYNCDONTCARE|TF_ES_READ`）**同步读宿主文档**（GetSelection + GetTextExt）
  并立刻读回 `session.rect()`（848，隐含假设同步执行；若被异步化则位置是默认值），
  再用 `ipc.update_position()`（855 → `send_oneway`，227）**再等一次应答**。
  加上随后的 `process_key`（956），**一次按键 = 最多 2~4 次同步 IPC 往返**
  （重连时还要 connect + start_session）：`update_position` 的应答没人用，应改成
  真正单向（不等应答）或把坐标与按键事件合并成一次请求
- [ ] **更严重的一处：IPC 跑在 TSF 编辑会话里（持宿主文档写锁）**。
  `text_input_processor.rs:669`（在 `start_composition` 的 `DoEditSession` 内，
  该会话用 `TF_ES_READWRITE` 申请：886-887）→ `update_caret_position_in_session`
  （564-595）→ 587 `ipc.update_position(...)`（写 + FlushFileBuffers + 等应答）。
  服务端慢/挂 → 宿主文档锁一直不放 → **整个宿主程序无响应**（不是候选栏慢，
  是应用卡死）。修：编辑会话内零 IPC（坐标改为会话外采样 + 后台发送 + 去重）
- [ ] `text_input_processor.rs:1329` `activate_impl` 每次都调
  `xime_config::init_logging("tsf")`：`create_dir_all` + 建日志文件（盘 IO），并且
  `*g = Some(guard)`（`xime-config/src/lib.rs:314-316`）会**丢掉旧 guard** ——
  `tracing_appender` 的 `WorkerGuard::drop`（`non_blocking.rs:282-293`）先
  `send_timeout(Shutdown, 100ms)` 再 `send_timeout((), 1000ms)`，
  **最坏约 1.1 秒**阻塞在调用线程（= 输入法激活的宿主 UI 线程）。修：日志只初始化
  一次（Once / 不覆盖 LOG_GUARD），并移出 Activate

**P2 中低（选摘）**
- [ ] `pages/input_schema.rs:372` 「已下载」tab 每帧 `scan_market_dir()`
  （两层 `read_dir`）——页面层唯一每帧磁盘 I/O，缓存进 state 即可
- [ ] `state.rs:2610/2620`（剪贴板同步插件）与 `3019-3021`（备份插件）：配置
  文本框**每敲一个字符**读 + YAML 解析 + 写盘
- [ ] `state.rs:1825-1832` `stop_server()` 的 `child.wait()`；`state.rs:546/548`
  剪贴板/快捷发送 SQLite `migrate_legacy` 被调用两次且逐行独立事务
- [ ] `pages/store.rs:240-248` 商店页每帧 O(n²) 标签去重 + 每卡片 `truncate`
  分配；`state.rs:2275-2286` 语音 poll 每 250ms 无条件 clone 全文
- [ ] `state.rs:857-887/969-998/1118-1147` 索引下载完成那一 tick 在 UI 线程整份
  `serde_yaml::from_str` + 目录扫描（解析应放在 `start_load_*` 线程内）
- [ ] `pages/mod.rs:144/179`、`pages/about.rs:24` 每帧重建 `svg::Handle`：
  **已核实不会重复解析**（`iced_core-0.14.0/src/svg.rs:93-101` 用内容哈希做 id，
  `iced_wgpu/src/image/vector.rs` 按 id 命中缓存）；但 debug 构建下 `rust-embed`
  每次 `Assets::get` 会读盘（11 次/帧）→ 建议 `OnceLock<svg::Handle>` 缓存，
  release 无此问题
- [ ] `state.rs:670-687` 保存外观 2× load+save；`state.rs:1937`
  `PluginManager::get()` 内部 `list()` 造成 O(n²) 目录扫描；
  `tray.rs:419-433` 每次图标更新重建 HICON + `Shell_NotifyIconW`
- [ ] 设计如此、无需改：`rfd` 原生模态文件对话框（app.rs:464-498）、
  `xime-sync-server` 的 spawn、`SystemTheme::detect()`（Linux 分支才起子进程）

**已核实安全**：`start_deploy`/下载/安装/卸载/词典/备份/同步插件/语音重活全部
在线程内；`ipc_server` 每条连接一线程；消息线程三处引擎锁全用 `try_lock`
（不阻塞、但会静默失败）；`pages/` 全域只有 `input_schema.rs` 触盘；
`backup.rs`/`webdav.rs`/`speech.rs` 架构正确
- [ ] 下一步（待用户选优先级）：P0 四处（`save_schema` 全量部署、`RimeSyncNow`
  同步 IPC、冲突预检 `refresh_builtin_package`、轮询里的 `reload_schemas`）都是
  「低改动量、确定收益」，建议按此顺序修

### 2026-09-30 输入方案页：安装/卸载/还原的进行中（loading）状态
- [x] **问题核实**（用户提出「已下载里点安装没有 loading，看着不合理」）：
  `pages/input_schema.rs` 全文**没有**读 `installing`/`downloading`/`download_progress`，
  卡片只有静态的「安装/卸载」按钮 + 静态状态文案；而扩展商店页状态是齐的
  （`store.rs:412-419` 的 `安装中…`/`下载中 XX%`）。state 层其实一直在设
  `market_schema.installing`（`state.rs:1458` 安装 / `1493` 卸载 / `1335` 确认冲突后 /
  `1425` 还原），**纯属页面没接线**。另外 `install_message` 原来只在「已安装」tab
  渲染，从「已下载」安装失败时错误出现在另一个 tab 上，看不到
- [x] **改动**：
  - `components/widgets.rs`：把 store.rs 的私有 `disabled_button` 提为公共
    `button_disabled`（进行中标签占位：无 `on_press`、次要按钮尺寸、弱化文字色）；
    store.rs 删掉私有实现并改用公共的（4 处调用同步补 `&colors`）
  - `pages/input_schema.rs`：新增纯函数
    `action_button(idle_label, busy_label, danger, busy_self, any_busy) -> ActionButton`
    统一决定按钮文案/可用性/颜色；「已下载」卡片本包在忙 → `安装中…`/`卸载中…`
    （禁用）并把状态文案换成「正在安装：解压方案包并全量部署…」/「正在卸载：删除该
    方案包的文件与部署缓存…」，别的任务在忙 → 保留原标签但禁用（避免连点）；
    「已安装」的还原卡片 → `还原中…`（禁用）+「正在从 market/builtin/ 备份还原…」；
    `install_message` 现在两个 tab 都显示
- [x] **验证**：`cargo build -q -p xime-setup-lib` 零错误；`cargo test -q -p
  xime-setup-lib` **16/16**（新增 3 项：空闲 / 本卡在忙 / 其它任务在忙 三种按钮态）；
  XimeYao `cargo build --quiet` 零错误；未运行程序（按仓库规则由用户 `rebuild.ps1` 目视）
- [ ] **已知边界（本次只做 UI 状态，用户选择的 B）**：
  `install_market_schema` 第一步仍是**同步**的
  `schema_install_conflict → refresh_builtin_package()`（审计 P0 #3），所以点下去的头
  几百毫秒~数秒依旧是无反馈的卡顿，`安装中…` 只能在预检跑完之后才出现。
  修 A（预检短路：注册表已有 builtin 就不重扫）待后续单独做——做完这个 loading 才是
  「点下去立刻出现」

### 2026-09-30 剪贴板拉取内容不入历史（修复）；图片同步现状盘点
- [x] **日志实锤**：`远端剪贴板已写回本地 (9 字符)` —— 拉取链路本身是通的，
  断点在历史：写回触发 WM_CLIPBOARDUPDATE → LocalChanged → hash 命中
  `self_written`（自写回声抑制）→ 被去重跳过 → 拉取内容永远不进历史
- [x] 修复：`PollTick` 拉取成功后**显式记历史**（新增 `record_history`，与
  LocalChanged 共用，超长截断 + 容量裁剪同规则）——对齐 Android 语义（拉到的
  内容出现在剪贴板面板）；回声抑制只管推送去重，不该挡历史
- [x] 顺带：远端图片附件 profile（`has_data=true`）此前无声 `continue`，现留
  显式日志「桌面端暂未支持图片同步，跳过」
- [x] **图片同步现状盘点**（对照 Android ClipboardSyncBridge）：
  - 已就绪：插件 JS（webdav_clipboard_sync 1.1.0 推/拉都支持 `has_data`/`data_name`
    blob 上传下载，MIME 表齐全）、DB schema（clipboard_entries v4 的
    `type/imagePath/imageHash/mimeType/sizeBytes/width/height` 列已建）、
    桥接层（`clipboard_pull` 能把 JS 侧 `data` 字节带回宿主；注释明言
    「附件字节桌面宿主尚未启用，仅文本路径」）
  - 缺失（Windows 宿主，待做）：本地图片采集（CF_DIB/CF_DIBV5 → PNG 落盘
    内容寻址 `<sha256>.png`，对齐 Android ClipboardImageStore）、历史 API 图片
    字段落库、推送 profile 带 `data` 字节、拉取图片 hash 校验后落盘 + 入历史 +
    写回系统剪贴板（PNG → DIB）、设置页缩略图渲染
  - 按「一次一个功能点」规矩：待本修复端到端验证后再做图片链路
- [x] 测试甄别：`cargo test -p winxime-server` 20 项中 7 项失败**全部为既有失败**
  ——3 项 plugins（stash 回 HEAD 验证同样挂，来自并行会话提交 8bf872a：
  `clipboard_selection_follows_clipboard_sync_toml` /
  `clipboard_worker_records_history_without_sync_plugin` /
  `backup_now_uses_typed_plugin_runtime`，断言级失败）；
  4 项 models/schema_switches 为沙箱环境 `create_dir_all` PermissionDenied
  （这些文件处于 HEAD 未改动状态）。本次改动不引入新失败
- [x] 验证：`cargo build --quiet` 零错误

### 2026-09-30 切到未启用方案后无法打中文（修复死会话）
- [x] **日志链路**：12:14:02 `SelectSchema wubi86_pinyin -> schema selected
  successfully`（librime 居然返回成功）→ 12:14:04 ReloadConfig `redeploy result:
  true`（重建会话后重选了同一方案）→ 此后所有按键 `handled: false, input: Some(""),
  composing: false`（F4 也不响应；该配置的切换菜单热键是 Ctrl+0，F4 本就不绑定）
- [x] **根因**：`wubi86_pinyin.schema.yaml` 在 rime 目录但不在 schema_list、
  `build/` 无部署产物（没有编译词典）。librime 的 `select_schema` 对这种
  「文件在但未部署」的方案照样成功——Schema 配置能加载，但翻译器无词典 →
  **死会话：所有按键不组词**。设置页方案列表又是扫 rime 目录全部
  `.schema.yaml`（不止已启用的），用户点到未启用方案即触发；
  engine 的 `redeploy()/deploy()` 重建会话后还会把死方案重选回去，固化故障
- [x] **修复（xime-rime/engine.rs）**：新增 `schema_deployed()`（build/ 有
  `<id>.schema.yaml` 产物才算已部署）——
  ① `select_schema` 前置守卫：未部署直接返回 false，宁拒不选（死会话从源头杜绝）；
  ② `redeploy()`/`deploy()` 重建会话后的重选同样守卫，未部署则清掉选中记录
  （会话自然回落 schema_list 第一个，必是已部署的）
- [x] **服务端**（winxime-server/ipc_server.rs）：SelectSchema 失败日志写明
  原因（未部署或不存在）；返回 success=false 后，设置端 `save_schema` 自然
  落到既有「选中方案置顶进 schema_list + save + deploy_all」持久化路径——
  用户「切到未启用方案」的意图由正路满足（代价：该路径是同步部署，设置页
  会卡数秒，与并行会话标注的已知边界同源）
- [x] **用户当前状态的自愈**：重启 server 后启动恢复读 rime 的
  `previously_selected_schema`（= wubi86_pinyin）→ 守卫拒绝 → 停在 wubi86，
  打字立刻恢复；再到设置页点五笔拼音会走「启用+部署」后正常切换
- [x] 验证：libximecore `cargo build --quiet` 零错误；xime-setup-lib 16/16；
  XimeYao `cargo build --quiet` 零错误（xime-rime 无测试覆盖，守卫逻辑待实机验证）

### 2026-09-30 方案包启用列表整包化 + 切换方案「先落盘→通知→未部署则后台补部署再选中」
- [x] **接上一条（上一节修的是「死会话」）**：守卫让「未部署方案」从死会话变成
  明确失败后，暴露出另外两个问题——用户「切了方案但打字还是旧的」就是它们
- [x] **根因 1：启用列表被写窄成单项**。写 `default.custom.yaml` 的四处代码里三处
  写的是**单项**：安装步骤 5（注释原文「启用新方案的第一个」）、
  `apply_restored_builtin_schema_list`、`do_uninstall` 卸载最后一个来源时的自动还原
  （只 `push` 默认方案一个 id）。librime 只编译 `schema_list` 里的方案 +
  `dependencies`，所以随包释放但没进列表的方案没有 `build/` 产物 → 守卫判「未部署」。
  证据：内置包登记 9 个 `.schema.yaml`（`.registry.yaml`），`build/` 只有 3 个
  （= `default.yaml` 的 3 项，pinyin_simp 还是 wubi86_pinyin 的 `dependencies`）；
  20:33:58 的部署只产出 wubi86（`wubi86.table.bin` 时间戳），
  `build/wubi86_pinyin.schema.yaml` 直到 20:35:42 才被重建
- [x] **根因 2：兜底部署没通知守护进程**。`save_schema` 的失败分支只写文件 + 在设置
  进程里 `deploy_all_schemas()`，没有 `notify_daemon_reload()` → 「文件层面切好了，
  正在打字的引擎还是旧的」。日志：20:35:42 那次 SelectSchema 失败之后再无
  `ReloadConfig`，直到用户手点「部署方案」（20:42:06 `redeploy result: true`）
- [x] **修正时间线的关键一点**（我先前判断有误）：20:14:02/20:14:21 那两次
  `selecting schema: wubi86_pinyin → schema selected successfully` **不能**证明它当时
  已部署——旧版服务端没有守卫，librime 对「文件在但没编译词典」的方案照样返回成功，
  那正是上一节的死会话。所以启用列表在 19:50（卸载第三方包 → 自动还原内置 → 单项写入）
  就已经变窄，产物当时就没了
- [x] **改动**（`xime-setup`，与上一节的 `xime-rime` 改动互补，无重叠）：
  - 新增纯函数 `package_schema_ids(files, default_id)`：取某包**全部**顶层方案 id
    （默认方案置顶、字典序、去重、忽略子目录），三处单项写入全部收敛到它 →
    **整包启用**
  - 新增 `deployed_schema_ids[_in](build/)`：按引擎同一判据
    （`build/<id>.schema.yaml` 存在）列出「已部署方案」
  - `save_schema` 顺序固定为：**先落盘启用列表 → 再通知宿主 SelectSchema**（保住
    「已部署方案毫秒级切换、零部署」快路径）→ 失败则走新增的
    `start_deploy_then_select`：后台 `deploy_all()` → `notify_daemon_reload()` →
    **再补一次** `notify_select_schema()`（宿主 redeploy 会优先恢复它记住的旧方案，
    少了这步就是「部署了但没切过去」）→ 结果写进 `deploy_result` 由 `poll_deploy` 提示。
    返回值改为提示语（`app.rs` 显示「已切换到 X」/「正在切换 X：正在后台部署…」）
  - 顺带：启用列表自愈（丢掉指向已删除方案的死项，如卸载掉的第三方方案）；
    兜底部署不再走 UI 线程（= 审计 P0 #1 里「点方案行冻设置窗口」那一处）
  - UI：`InputSchemaState.deployed_schema_ids`（`reload_schemas` 扫一次 `build/` 填充，
    页面不触盘）；`schema_row` 对没有产物的方案显示「未启用」badge，仍可点
    （点它 = 启用并部署）
- [x] **验证**：`cargo build -q -p xime-setup-lib` 0 错 0 警告；
  `cargo test -q -p xime-setup-lib` **20/20**（新增 4 项：整包启用 / 默认方案缺失回退
  字典序 / 嵌套忽略+去重 / build 目录不存在）；XimeYao `cargo build --quiet` 0 错。
  未运行程序（按规则由用户 `rebuild.ps1` 目视）
- [ ] **待用户目视验证**：① 输入方案页里没编译的方案（五笔98/繁体五笔/繁体五笔拼音/
  T9/numbers/handwriting）显示「未启用」，点它 → 「正在切换…正在后台部署…」→ 完成后
  「已切换到…」，server.log 应出现 `Running rime deployment` + `ReloadConfig` +
  `selecting schema: <id>`；② 安装/还原方案包后包内方案应全部进 `schema_list`
  （内置包 = 9 项）且 `build/` 有对应产物，包内互相切换毫秒级完成

### 2026-10-01 候选栏面板「剪切板」子页（历史列表 + 点击即上屏）
- [x] **背景**：候选栏 ⋮ 菜单的 7 个子页自 2026-09-11 起都是「功能开发中」占位；
  本次接第一个真实子页（📋 剪切板），其余子页占位不变。
- [x] **数据源**：`clipboard.db`（与设置程序、剪贴板同步工作线程同一个文件）——
  `xime_config::clipboard_store::list_history`，上限 100 条、最新在前。
  server 启动时用 `ui::panel::set_clipboard_history_db()` 注入路径
  （`user_data_dir.parent()/clipboard.db`，即 `%APPDATA%\Xime\clipboard.db`）；
  未注入时回退 xime-config 默认路径（测试/未接线场景）。
- [x] **只在「进页」时读一次库**：点菜单卡片 → `reload_clipboard_page()` 一次 SQLite
  查询 → 存进 `CandidateWindow.panel_clipboard`；绘制只读内存快照
  （保持「绘制不触盘」约定，UI 线程没有逐帧 IO）。
- [x] **布局：面板高度改为按页面** —— 菜单页 198 不变，剪切板页 304
  （标题栏 36 + 6 条 × (32+4) + 翻页条 32 + 边距）。`panel_height(page)` 是唯一几何
  来源，`calculate_client_rect` / `window_to_panel` / `panel_hit` / `draw_panel`
  全部走它（画的和点的共用一套坐标）。条目单行显示
  （`DWRITE_WORD_WRAPPING_NO_WRAP`）、控制字符折成空格、超 40 字截断加省略号
  ——复制用的始终是**全文**。
- [x] **交互**：条目行 hover 高亮（与菜单卡片同一套 hover 状态）；
  右下角「上一页/下一页」（首页/末页禁用，禁用时**不可命中**，见 panel_hit）；
  空库显示「暂无剪贴板记录」。
- [x] **点击即上屏（第二轮修正，第一版只写回剪贴板被用户否掉）**：点条目 →
  `clipboard::write_text()` 放回系统剪贴板（兜底）+ `paste::request_commit()` 直接上屏
  → 收起面板 → 系统通知「已上屏（N 字）」。
  上屏通道 `src/paste.rs`：server 自己 `SendInput` 一个 **VK_F24**（实体键盘上不存在），
  宿主 TSF 照常把这个键上报给 server，server 在 `ProcessKeyEvent` 里认出它
  （`handle_paste_trigger`）→ `clear_composition()` 清掉半成品编码串 →
  把待上屏文本当 `commit` 回包 → **宿主用提交文本替换 composition 并结束它**
  （`XimeEditSession::DoEditSession` 既有逻辑），文本于是落到光标处。
  - 不需要改 IPC 协议、**不需要改宿主**（复用的是「按键 → commit 回包」这条现有通道），
    也就没有「模拟 Ctrl+V」那种「粘进半成品编码串里」和完整性级别（UIPI）的坑
  - 待上屏文本带 1.5 s 有效期：注入失败（前台应用完整性更高、英文态、宿主没在跑）时
    不会被很久以后的触发键取走；`SendInput` 返回 0 时回退提示
    「已复制到剪贴板，按 Ctrl+V 粘贴」
  - 语义取舍：点击 = 「这条现在就上屏」，**当前未上屏的编码串不保留**（否则它会跟着
    文本一起留在文档里）
- [x] **顺手修掉一个必 panic 的写法**：`if this.panel_clipboard.borrow_mut().next_page() {…}`
  ——`if` 条件的临时值会延续到整个 `if` 体，体内重绘再 `borrow()` 即 RefCell panic；
  改为先 `let turned = …borrow_mut()…;` 再判断（注释已留在代码里）。
- [x] **验证**：`cargo build --quiet` 0 错、无新增警告（仅既有 `vk_to_xk` /
  `font_family` / `comment_width` 等旧警告）；`cargo test -q -p winxime-server ui::panel`
  **10/10**（新增 4 项：面板高度与翻页条三段不重叠、行/翻页命中路由（含禁用态不命中）、
  翻页夹回与空列表、展示文本折行截断）；`cargo test -q -p winxime-server paste`
  **2/2**（待上屏文本取一次/过期作废、触发键 keycode 与宿主 `vk_to_xk` 换算一致）；
  整包 `cargo test -q -p winxime-server` **19 通过 / 7 失败**——7 项与改动前基线
  （13/7）完全同源，都是既有失败（3 项 plugins 断言 + 4 项沙箱 PermissionDenied）。
  未运行程序（按规则由用户 `rebuild.ps1` 目视）。
- [ ] **待用户目视验证**：候选栏 ⋮ → 📋 剪切板 → 应看到最近 6 条历史（含刚复制过的
  内容）且 hover 高亮；**点一条 → 该条直接出现在光标处**（面板同时收起、有「已上屏」
  通知）；条目多于 6 条时右下角可翻页（首页「上一页」灰）；空库显示「暂无剪贴板记录」。
  若前台应用以管理员身份运行（server 未提权时会被 UIPI 拦），应看到
  「已复制到剪贴板，按 Ctrl+V 粘贴」这条兜底提示，server.log 里是
  `上屏触发键注入失败`。
- [ ] **后续（未做）**：面板打开期间剪贴板变化不自动刷新（需重进该页）；条目删除/
  置顶/搜索。

### 2026-10-01 候选栏面板「快捷发送」子页（同一张表的 isQuickSend=1 子集）
- [x] **背景**：⋮ 菜单第二张卡片 🚀 快捷发送 原是「功能开发中」占位；本条把它接成
  真实子页——数据与剪切板历史**同一张表**（`clipboard.db`，`isQuickSend=1` 子集，
  含触发编码 `code` 列），顺序由 store 决定（`isPinned DESC, timestamp DESC, id DESC`，
  即置顶优先、新在前）。
- [x] **列表版式收敛为一套**：原来只有「剪切板」一个列表子页，`PanelClipboard` /
  `clipboard_row_*` / `clipboard_page_*` 这套名字在接第二个列表页时就名不副实了。
  本次把列表子页的模型与几何收敛成 `PanelList`（条目 = `PanelListItem { text, code }`）
  + `list_row_*` / `list_footer_y` / `list_page_button_rect` / `list_page_label_rect` /
  `list_count_rect`：
  - `PanelList.source` 记录这份数据属于哪个子页；`panel_hit` / `draw_panel` 只在
    `source == page` 时画/点条目（进页即 reload，这里是兜底——数据与页面不一致时
    按空态处理，绝不把上一个子页的条目画出来还点得动，有单测钉住）
  - 「剪切板」行为完全不变（条目 code 恒为空串 → 不占编码列），12 项面板单测全绿
- [x] **进页读一次库**：`CandidateWindow::reload_panel_list(page)` 按页面选数据源
  （剪切板 → `list_history`，快捷发送 → `list_quick_send`），一次 SQLite 查询后绘制
  只读内存快照；库路径同一个 `set_clipboard_db_path()`（顺手把
  `set_clipboard_history_db`/`clipboard_history_db` 改成现在的名字）。
- [x] **显示带触发编码**：快捷发送条目在行首留出编码列（64px，次要色，字号同条目），
  正文右移避让；**只有本列表确实有条目带编码时才占这一列**，且按整份列表判断而不是
  按页判断（翻页时正文不会左右跳）。编码与正文都走同一套「控制字符折空格 + 40 字截断」
  显示规则（上屏用的是全文）。
- [x] **空态指路**：无条目时主文案「暂无快捷发送内容」+ 一行小字
  「在设置程序的「剪贴板 → 快捷发送」里添加短语」（剪切板页保持原来的单行空态）。
- [x] **交互与剪切板一致**：行 hover 高亮、右下角「上一页/下一页」（首/末页禁用且
  不可命中）、「共 N 条」/「第 x/y 页」；**点击条目直接上屏**（复用上一功能点的
  `paste::request_commit` 触发键通道 → 宿主 commit 回包）。
  唯一差别：**快捷发送不主动污染系统剪贴板**——只有在「上屏注入失败」时才退化成
  「复制到剪贴板 + Ctrl+V 粘贴」的兜底提示；剪切板条目仍先放回剪贴板（它本来就是
  剪贴板内容，顺带留兜底路径）。
- [x] **验证**：`cargo build --quiet` 0 错、无新增警告（仅既有 `comment_width` /
  `font_family` 等旧警告）；`cargo test -q -p winxime-server ui::panel` **12/12**
  （原 8 项列表相关单测改名后全绿 + 新增 2 项：跨页数据不命中、编码列与正文不重叠）；
  `cargo test -q -p winxime-server paste` **2/2**；整包 **21 通过 / 7 失败**——7 项与
  基线完全同源（3 项 plugins 断言 + 4 项沙箱 PermissionDenied）。未运行程序
  （按规则由用户 `rebuild.ps1` 目视）。
- [ ] **待用户目视验证**：候选栏 ⋮ → 🚀 快捷发送 → 应看到设置程序里录入的条目
  （有编码的条目左列显示编码、右侧显示内容）；点一条 → 内容直接出现在光标处 +
  「已上屏（N 字）」通知；条目多于 6 条可翻页；没录入过时显示
  「暂无快捷发送内容」+ 一行添加指引。
- [ ] **后续（未做）**：**输入触发编码前缀让条目进候选栏**（安卓端的真实功能，需要
  接 librime 的自定义候选源，是独立功能点）；置顶（`isPinned` 有列有排序但两端都还
  没有设置入口）；面板内增删/管理（管理入口仍在设置程序的「剪贴板 → 快捷发送」）。

### 2026-10-01 候选栏菜单「计算器」= 模式入口（在输入框直接敲算式，候选栏显示实时结果）
- [x] **背景 / 返工**：⋮ 菜单第三张卡片 🧮 计算器。第一版按「面板内 4×5 数字键盘子页」做，
  用户当场否掉：**「计算器模式，应该是直接在调出来后，直接在输入框输入在候选栏显示计算结果」**，
  **「这里的菜单的计算器只是一个触发入口而已」**。本版按这个语义重做，键盘子页整段删除。
- [x] **形态：模式，不是子页**。点 🧮 → 打开计算器模式 + 面板收起；之后**直接在输入框敲算式**
  （算式就是输入法自己的 composition/组词串，带着下划线画在光标处），**候选栏实时显示计算结果**；
  空格 / 回车 / `=` / **点候选栏里的结果** 都能上屏（上屏后**自动退出模式**，算完即用完）；
  `Esc` 清空算式，**算式已空时再按 `Esc` 退出模式**（再点一次菜单里的 🧮 也退出——卡片文案
  变成「退出计算器」，一眼看得出再点一下是退出）。**一开始打中文（敲字母）也会自动退出并把键
  交给 rime**——这条是当天实机事故后补的硬规则，见下面那条「模式把输入法堵死」。
  为什么「能在输入框里敲」：Windows 没有安卓那个数字软键盘，而面板窗口是 `WS_EX_NOACTIVATE`
  （点了不抢焦点）——算式只能借输入法本来就有的组词串通道：宿主看到 `status.composing = true`
  + `preedit` 就会在光标处画出来，候选栏由 `candidates` 承载结果。两条都是现成的，
  **没有改宿主协议、没有新 IPC 命令**。
- [x] **新增 `crates/winxime-server/src/calculator.rs`**（模式状态 + 按键映射 + 求值，纯逻辑）：
  `AtomicBool` 开关 + `Mutex<String>` 算式（与 `paste.rs`/托盘同款进程级状态，锁中毒取回内部值）；
  `action_for_key(keysym, modifiers)` 把 **X11 keysym** 映射成 Input/Backspace/Clear/Commit/
  Passthrough/ExitAndDelegate（主键盘与小键盘的数字、`.`、`+ - * /`、退格、Esc、回车、空格、`=`；
  **Ctrl/Alt 组合键与 Tab/方向/翻页/功能键一律放行**——模式不能把人困住，连移动光标都不行；
  **其余键（字母/标点/没列出的键）一律 ExitAndDelegate**）；
  `idle_action()`：**算式为空时只留「算式的输入键」与 Esc**，上屏/退格升级成 ExitAndDelegate。
  12 项单测（求值/格式化/按键映射/待命态/输入编辑/长度上限/模式开关清算式），其中碰全局状态的
  用例用 `state_lock()` 串行锁（cargo test 默认多线程，否则随机互踩）。
- [x] **求值仍用 `fasteval2 = "2.1.1"`**（crate 局部依赖，不升 workspace）：MIT、**0 运行期依赖**、
  纯 f64（`7/2=3.5`）。否掉的：`evalexpr`（AGPL + **整数除法 `7/2=3`**，语义就是错的）、
  `meval`（2018 停更、拖 nom 1.x）、`fasteval` 0.2.4（API 别扭）、`exmex`（regex+smallvec 偏重）、
  **自己手写解析器**（优先级/一元负号/`12.`/`.5`/`1/0` 一堆边界，还得自己补一套测试）。
  领域语义留在自己手里：算式没输完（末尾是运算符 / 只剩一个数 / 只有一元负号）**不给结果也不报错**
  （否则刚敲 `1` 就跳出 `=1`，看着像算完了）；至少一个二元运算符；除零/非有限 →「⚠ 不能除以 0」；
  解析失败 →「⚠ 算式有误」；显示 10 位小数去尾零（`0.1+0.2` → `0.3`），**`-0` 也显示成 `0`**；
  48 字符上限（键盘敲得出的算式都在这个量级）。
- [x] **IPC 接线**（`ipc_server.rs`）：`ProcessKeyEvent` 里**先认上屏触发键、再认计算器模式**，
  然后才轮到英文态/联想/rime。`handle_calculator_key` 只管三件事：改算式、组 context、显示或隐藏。
  算式非空 → `composing = true` + `preedit = 算式`（`*` `/` 显示成 `×` `÷`）+ 候选栏一条候选
  （结果，注释「空格上屏」；**label 给空串**，否则结果前面会画出「1.」，让人以为按 1 能选中它）；
  算式没输完 → 不显示候选栏（候选栏里出现的永远是**能上屏的结果**或**明确的错误**）；
  上屏 → `commit = 结果` + `composing = false`（宿主自己收尾 composition）;
  进入模式时顺手清掉 rime 里的半截编码（先打了 `nihao` 再点 🧮，退出后不会又接着组词）。
  **返回值是 `Option<IpcResponse>`**：`None` = 这个键不归计算器管、模式已退出，调用方要**继续往下走**
  让 rime 处理（字母因此永远能打中文）。
- [x] **点候选栏结果上屏**（`ui/view.rs::commit_calculator_result` + 新增
  `ui/layout.rs::candidate_item_rect`）：与 ⋮ 按钮同一套坐标约定（`pt - BLUR_RADIUS`），
  矩形算法与 `paint.rs` 的绘制递推逐项对齐（有单测锁定：首项位置/列间距/换行/越界给 None/
  每行候选数为 0 不除零），否则会出现「画得出来却点不到」。上屏走列表子页同一条
  `paste::request_commit` 触发键通道，失败退化成复制到剪贴板 + 通知。
  **只对计算器模式生效**——通用「点候选上屏」（`IpcCommand::SelectCandidate`）是独立功能点，没混做。
- [x] **菜单页随之收敛**：`PanelPage` 里的 `Calculator` / `Settings` 变体删除——
  它们**不是页面而是动作入口**（`MenuAction::ToggleCalculator` / `OpenSettings`），
  `from_id` 不再解析这两个 id（解析成功会开出一个空子页）；补测试
  `page_from_id_covers_all_menu_defs`（这两条 id 必须解析失败）+ `menu_cards_expose_action_ids`
  （7 张卡片的 id 顺序）。
- [x] **切英文态收起候选栏**（`main.rs` 托盘「切换中/英」补 `window.hide()`，与 IPC 侧
  `ToggleAsciiMode` 已有的行为一致）：英文态 = 输入法透明（宿主把按键短路在本地，连上屏通道
  都退回剪贴板），候选栏留在屏幕上没有意义；这也顺带保证「英文态下 ⋮ 菜单打不开，
  不会进到一个敲了没反应的死状态」。
- [x] **验证**：`cargo build --quiet` **0 错、无新增警告**（仅既有 `font_family` /
  `comment_width` / dead_code 旧警告）；`cargo test -q -p winxime-server` **36 通过 / 7 失败**
  ——7 项与基线**完全同源**（3 项 plugins 断言 + 4 项沙箱 PermissionDenied）。
  按规则**没有运行程序**，界面由用户 `.\rebuild.ps1` 目视。
- [ ] **待用户目视验证**：候选栏 ⋮ → 🧮 计算器 → 通知条 + 卡片变「退出计算器」；直接敲 `12+34`
  → 输入框里看到下划线算式、候选栏显示 `408`（注释「空格上屏」）；空格 → `408` 落到光标处、
  候选栏收起**且模式自动退出**；再打开敲 `1/0` → 候选栏「⚠ 不能除以 0」且空格/回车不动作；
  **敲字母 → 立刻回到中文输入**；`Esc` 清空、再 `Esc` 退出模式（或再点一次 🧮）；
  `12*34` 的算式显示成 `12×34`。
- [ ] **后续（未做）**：通用「点候选栏上屏」（`SelectCandidate`，适用所有模式）；括号/幂/百分号/
  函数；算式历史；`%` 与单位换算；英文态下也能用计算器（要动宿主英文态短路，见 DECISIONS）。
- [!] **过程记录（工具红线）**：本次收尾时用 PowerShell 批量改测试调用，误把 `ui/panel.rs`
  压成了一行（`Set-Content -NoNewline` 传**数组**会丢掉所有换行），随后 `git checkout --`
  只回到「最后一次提交」的旧版（本仓库从不提交），两次都靠 DSH 的编辑前镜像
  （`%TEMP%\dsh-workspace-changes-*`）完整还原，并用**去空白逐字符比对**核对过。
  教训见 `DECISIONS.md`「工具操作红线」：**禁止把数组喂给 `-NoNewline`**、
  **禁止对未提交的工作区用 `git checkout --` 回滚**。

### 2026-10-01 实机事故：计算器模式把输入法堵死（「连中文都输入不了了」）
- [!] **现象（用户报）**：「现在连中文都输入不了了」——中文态下敲任何键都没反应
  （或者只往文档里落英文字母）。
- [x] **定位（靠日志，不靠猜）**：`%APPDATA%\Xime\logs\server.log`（本地时间 = UTC+8）
  - `16:02:12.76Z` 之前每个按键都走 rime：`Key event, ascii_mode=false` → `Key: 115` →
    `candies: [要,木,是,说,上]`，中文正常；
  - `16:02:14.07Z` `on_paint: height=239.78`（候选栏 37.78 + 面板 202）= **用户点开了 ⋮ 菜单**；
  - 之后只剩 `Received request: ProcessKeyEvent` + `WM_HIDE_CANDIDATE`，**再没有任何 `Key:` 行**
    → 每个按键都被计算器分支拦下了 —— **是模式开着没关，不是崩溃、不是 IPC 断、不是 rime 坏**。
  - 用户中途按了两次 Shift（`ToggleAsciiMode` current=false→true→false），说明他也在怀疑中/英态。
- [!] **两个设计错误（都要认）**：
  1. **吞键/放行键都等于让输入法失灵**：旧版算式非空时把字母/标点 `Swallow`（不落文档、也不给
     rime），算式为空时把字母 `Passthrough` 交回前台应用（于是文档里只出现英文字母）——两条路
     都绕过了 rime，用户看到的就是「中文打不出来」。
  2. **模式开着时屏幕上没有常驻提示**：候选栏只在「有可上屏结果」时才显示，待命态什么都没有；
     通知条几秒就消失，⋮ 卡片文案得点开菜单才看得到 → 用户完全不知道自己在计算器模式里。
     一个会接管键盘的模式，**不能是隐形的**。
- [x] **修法（把「堵死」这个可能从设计上删掉）**：
  - `CalcAction::Swallow` 删除，新增 `ExitAndDelegate`：**字母、标点、以及任何没列出来的键
    一律「退出模式 + 交给 rime」**——一开始打中文就自动离开计算器，永远回得来；
  - 待命态（算式为空）里上屏键/退格也升级成 `ExitAndDelegate`（空格就该打空格）；
  - **上屏成功即退出模式**（算完即用完；点候选栏上屏同样退出）——模式的生命周期被压到最短；
  - `handle_calculator_key` 返回值改成 `Option<IpcResponse>`：`None` 表示「这个键不归我、
    模式已退出」，调用方**继续往下走**让 rime 处理（这是「永不堵死」的实现基础）；
  - **进出模式都写日志**（`计算器模式：进入/退出（原因）`）+ 进入时的通知条把退出方式写在里面；
  - 回归测试直接钉住这条规则：`keys_map_to_calculator_actions` 里断言字母/逗号/Insert/未知功能键
    都是 `ExitAndDelegate`，注释写明「别改回吞掉或原样交给前台应用」。
- [x] **验证**：`cargo build --quiet` 0 错、无新增警告；`cargo test -q -p winxime-server`
  **36 通过 / 7 失败**（7 项与基线同源：3 plugins 断言 + 4 沙箱 PermissionDenied）。
  **自动化验证只有构建 + 单测**，运行时行为仍需用户实机确认。
- [ ] **待用户目视验证**：进入计算器模式后**敲字母应立即回到中文输入**（这是本次修复的核心）；
  上屏一个结果后模式应已退出（卡片文案变回「计算器」）；其余同上一条。
- [ ] **后续（未做）**：模式活着时能否有常驻可见提示（候选栏放一条不可上屏的提示项，或托盘
  图标加标记）——本期先用「自动退出 + 日志」把风险压掉，可见性方案单独做。

### 2026-10-01 计算器功能按用户决定**暂时下线**（代码整段移除，不留半死状态）
- [!] **决定（用户原话）**：「要不暂时去除计算器功能吧，先把相关代码去除」。事故复盘之后，
  计算器这条路要重新设计（模式的可见性、接管键盘的边界），**重新设计之前不留任何代码**——
  不做「注释掉」「加个开关关掉」这种半死状态。
- [x] **删掉的东西**：
  - `crates/winxime-server/src/calculator.rs` **整文件删除**（模式状态机 + 求值 + 12 条单测）；
  - `ipc_server.rs`：`ProcessKeyEvent` 里的计算器分支，以及 `handle_calculator_key` /
    `exit_calculator` / `calculator_candidates` / `calculator_status` /
    `empty_ipc_context` / `not_handled_response`（后两个只有计算器在用）；
  - `ui/view.rs`：候选栏点击上屏 `commit_calculator_result` 及其调用点、菜单 🧮 的
    `ToggleCalculator` 分发；
  - `ui/panel.rs`：`MenuAction::ToggleCalculator`、菜单卡片随状态变文案（「退出计算器」）；
  - `ui/layout.rs`：为「点候选上屏」加的 `candidate_item_rect` 与
    `RenderedMetrics::cand_per_row`（`CandidateModel::cand_per_row` 是原有的，保留）；
  - `ui/model.rs`：让候选 `labels` 生效的 `selkeys_from`（那是给计算器候选去掉「1.」用的）
    —— 选字键回到**固定的 1..5** 这个原有行为；
  - `Cargo.toml`：`fasteval2` 依赖（`Cargo.lock` 随之收敛）。
  - **保留**：⋮ 菜单里 🧮 那一格还在（和其它未接入功能一样落回「功能开发中」占位页），
    但**点了给一句明确反馈**（「「计算器」功能暂未开放」+ 收起面板）而不是**静默无反应**——
    卡片既没有子页也没有动作时不许做「点了没反应」的死卡片；反馈文案取
    `panel::menu_item_label`（与卡片 id 同源，新增单测钉住 7 张卡片的 id/文案顺序）。
- [x] **特意保留的东西**：事故复盘（上一条）+ `DECISIONS.md`「会让输入法接管键盘的模式」
  红线**原样保留**——那条规则是给**以后任何**接管键盘的模式看的，不是计算器的实现细节；
  计算器那节加了状态说明，写明文中提到的文件/函数在代码里已经不存在。
- [x] **验证**：`cargo build --quiet` 0 错、无新增警告；`cargo test -q -p winxime-server`
  **22 通过 / 7 失败**（36 − 14：计算器那 14 条单测随代码一起消失；7 失败与基线同源：
  3 plugins 断言 + 4 沙箱 PermissionDenied）。

### 2026-10-01 候选栏面板「表情」子页（分类标签 + 网格，点击即上屏）
> 注（2026-10-01 续）：本节写的 `EMOJI_PANEL_HEIGHT = 244`、「每个分类必须一页放得下」、
> `emoji_category` 与 `EmojiTab` / `EmojiCell` 已被下面「共用网格版式 + 分类内翻页」那节取代：
> 表情页现在与符号页共用翻页条（高度 282），分类超过一页会自动翻页，表情数据表也归到
> `ui::glyph` 的统一模型下。
- [x] **形态先问后做**（计算器那次返工的教训）：数据用**内置表情表**（代码内常量、离线、
  零配置，不依赖 rime 方案带不带 emoji 词典）；版式用**分类标签 + 网格**（每行 8 个 × 4 行，
  点标签切分类），不套列表页那套「文本行 + 翻页条」。
- [x] **数据**（`ui/emoji.rs`，新文件）：6 类（常用/人物/手势/自然/食物/符号）各 32 个，
  正好一页；`EmojiCategory { label, emojis }` + `category_count` / `category` / `count_in` /
  `emoji_at` 四个**越界安全**的查询函数。取值只用 Win10 1809+ 的 Segoe UI Emoji 稳定有
  字形的常见表情，不用 ZWJ 组合序列（👨‍👩‍👧 之类在格子里挤成一团、宽度不稳）。
  6 条数据单测钉住：每类非空且 ≤ 一页容量、标签唯一、第一个分类是「常用」、类内不重复、
  每个条目 1~2 个 char、条目不含空白。
- [x] **几何**（`ui/panel.rs`）：`EMOJI_PANEL_HEIGHT = 标题栏 36 + 间距 8 + 标签栏 28 + 间距 8
  + 4 行 ×(36+4) − 4 + 底边距 8 = 244`；标签栏在可用宽度内等宽均分；格子**行高固定 36**、
  **列宽按面板宽度算出来并夹在 26~44**（面板宽度 = max(候选行宽, 320)，可能远宽于 320，
  所以网格整体居中，不写死列宽）。3 条面板单测守：高度恰好占满（标签栏/网格/底边距不重叠）、
  **每个分类的每个格子都在面板内**（320 与 520 两种宽度逐格算）、标签栏等宽且不越界。
- [x] **命中与绘制同源**：`PanelHit::EmojiTab(i)` / `EmojiCell(i)` 由 `panel_hit` 用同一组
  `emoji_tab_rect` / `emoji_cell_rect` 判定；空槽（分类没填满时剩下的格）不响应 hover/点击；
  分类下标越界一律夹回第一个分类。单测覆盖标签/格子/返回按钮三条命中路径。
- [x] **交互**（`ui/view.rs`）：
  - 点标签 → 换 `emoji_category`（`Cell<usize>`，挂在 `CandidateWindow` 上）+ 清 hover + 重排重绘；
  - 点表情 → **与剪切板条目同一条上屏通道**（`paste::request_commit` 自注入触发键 → 宿主
    回包当 commit；失败退化成写剪贴板 + 通知条），随后收起面板；
  - hover：表情页加入可 hover 页面（格子高亮），`WM_SETCURSOR` 走同一命中（手指光标）；
  - 表情数据是编译期常量 → 进页面**不需要** `reload_panel_list`（表情页不是列表页）。
- [x] **顺带的小改动**：面板绘制状态 `Option<(PanelPage, Option<usize>)>` 收成
  `PanelPaintState { page, hovered_item, emoji_category }`（三样是同一次绘制的同一份状态，
  表情页又要多一个下标，再往元组里塞就散了）。
- [x] **验证**：`cargo build --quiet` 0 错、**无新增警告**（新代码没有 dead_code / 未用导入）；
  `cargo test -q -p winxime-server` **31 通过 / 7 失败**（新增 9 条：emoji 6 + panel 3；
  7 失败与基线同源）。**自动化验证只有构建 + 单测**。
- [ ] **待用户目视验证**：⋮ → 😀 表情 → 标签栏 + 32 格网格；点标签切分类；点表情应上屏到
  前台应用光标处（字形大小 / 格子手感 / 上屏落点这些只有实机能看）。英文态下候选栏整体隐藏，
  面板按既有设计进不去。
- [ ] **后续（未做）**：点一个表情即收起（与剪切板条目一致，且上屏通道本身会让候选栏收起），
  「连点几个表情」要单独设计上屏后保持可见的方案；「最近使用」分类、在设置程序里自定义
  表情包、以及表情的输入编码联想（rime 侧）都还没做。
### 2026-10-01（续）表情「最近使用」+ 新增「符号」子页（共用网格版式 + 分类内翻页）
- [x] **先定形态**（延续「形态先问后做」）：表情页与符号页**共用一套版式**（分类标签栏 +
  8 列网格 + 底部翻页条），只在三处分叉——数据来源（`emoji` / `symbol` 两张表）、字形字体、
  标签行数。于是把版式与数据解析抽成新模块 `ui/glyph.rs`，表情/符号各自只留一张常量表。
  这避免了两页各写一套几何/命中/绘制（那种重复必然导致「一边改一边忘」，见本项目
  「画得出来必须点得到」的规矩）。
- [x] **平台差异：桌面用翻页，不照搬安卓的滚动**。安卓键盘是 `LazyColumn` 滚动；桌面候选栏
  窗口**永远不获得焦点**（TSF 候选窗靠 `SWP_NOACTIVATE` 显示），`WM_MOUSEWHEEL` 到不了它手里
  ——做列表子页时已经踩过这条。所以分类内条目超过一页（32 格）时用**与剪切板 / 快捷发送
  同一套翻页条**：既复用已被验证的几何，也保证面板高度稳定。一页 = 4 行 × 8 列 = 32 格，
  正好等于最近使用上限（`glyph::PER_PAGE == recent_usage::MAX_COUNT`），所以「最近使用」页
  永远不需要翻页。
- [x] **最近使用（表情 + 符号都要）**：新增 `recent_usage.rs`——
  - 落盘 `%APPDATA%\Xime\recent_usage.json`（与 `clipboard.db` 同目录，但**不进数据库**：
    它是面板私有的 UI 记录，设置程序没有理由去管，也不该跟着「清空剪贴板历史」被清掉）；
  - 键 `recent_emojis` / `recent_symbols`（两页各记各的，由 `RecentKind` 枚举给出），
    写盘时**保留另一个键与未知键**（前向兼容：以后加别的记录不会互相抹掉）；
  - **LRU 32 条、按时间倒序**（不是按频次——安卓那边的注释写得很清楚：频次排序会让早期
    高频项长期霸榜），重复项移到队首、超长截断；
  - 读取**全程容错**：文件不存在 / 不是 JSON / 不是对象 / 值不是字符串数组 / 元素不是字符串，
    都退化成「空历史 + 继续可用」，绝不 panic（这是每次开面板都会读的文件）；
  - 单测 9 条覆盖：置顶、重复移动、时间序而非频次、截断到 32、空历史、各种垃圾形状、
    另一个键的保留、真实文件往返（含嵌套目录 + 坏文件恢复）。写文件路径可注入
    （`record_use_at`），单测用 `%TEMP%` 下的唯一目录，且**建不出目录就跳过**——不往
    用户数据目录里写测试垃圾。
- [x] **「最近使用」是第一个标签页，且在那一页点按不重排**（照安卓 `RecentUsageStore` /
  `EmojiKeyboardLayout` 的语义）：
  - 标签 0 = 最近（文案「最近」，桌面标签位窄放不下「最近使用」），空态在网格区居中显示
    「暂无最近使用」；
  - 在**最近使用页**点字形 → 只上屏、**不写记录**：位置稳定，同一个格可以连点同一个字形；
    在**分类页**点 → 记录 + 置顶（下次进面板就在最前面）；
  - 进页面（点菜单卡片）时读一次 `recent_usage.json`（`reload_panel_grid`），绘制只读内存
    ——与列表子页 `reload_panel_list` 同一节奏。这条「点按不重排」的规则放在 UI 层
    （`ui::view` 的点击分支），数据层只负责「记一条」。
- [x] **「符号」页数据**（`ui/symbol.rs`，新文件）：移植安卓 `SymbolData.kt` 的 17 个分类，
  顺序与标签照抄（中 英 数 ⓵ Xⁿ ◓ ⇌ ¥ 𝒲 δ 🆎 ㎠ ぁ ㅞ ɠ ♈ ㄎ）。移植时做了两处**有意的
  删减**并在文件头写明：类内按**码点**去重（安卓表里有 33 个重复项；⚾/⚽、ˆ/＾ 这种看起来
  像但码点不同的**不算**重复）、丢掉 1 个纯空格占位；最终 **17 类 1987 个**（2021 − 33 − 1）。
  ⚠️ 移植过程踩到的坑记在 `DECISIONS.md`：PowerShell 的 `-contains` 是**文化相关**比较，
  会把 ⚾/⚽、ˆ/＾ 判成同一个字符，用它去重会**悄悄丢真数据**（第一版少了 10 个）。改用
  「码点序列」当键，再做独立交叉核对（`保留 + 丢弃 == 2021`）才对上。
- [x] **表情表变成纯数据**（`ui/emoji.rs` 重写）：分类/取值/翻页全部归 `ui/glyph.rs`，
  这里只剩 6 张常量表（常用/人物/手势/自然/食物/符号，每行 8 个源格式）。旧的
  「每个分类必须 ≤ 32 一页放下」这条不变量**取消**（现在超过一页会自动翻页），单测改为守
  「字形非空、类内不重复、无空白、标签唯一且 ≤ 2 字」。
- [x] **几何**（`ui/panel.rs`）：`GRID_TAB_HEIGHT 26` / `GRID_CELL_HEIGHT 36` / 列宽夹在
  26~44 且网格整体居中；标签栏 1 行（表情 1+6 = 7 个标签铺满）或 2 行（符号 1+17 = 18 个，
  每行固定 9 个）；面板高度 = 标题栏 36 + 8 + 标签栏 + 8 + 网格 156 + 8 + 翻页条 32 + 8
  → **表情 282 / 符号 312**（列表页 304、菜单页 198 作对照）。**翻页条始终占位**（一页时只画
  「共 N 个」），面板高度不随分类跳动。列表子页那段内联的翻页条绘制抽成 `draw_footer` 闭包，
  两页共用（外观一致，只有 y 与文案不同）。
- [x] **命中与绘制同源依旧**：`PanelHit::GlyphTab(i)` / `GlyphCell(i)` / `PrevPage` /
  `NextPage`（后两个取代原来的 `ListPrevPage` / `ListNextPage`，两个子页共用底部那一条）。
  空槽（当前页没有内容的格子）既不画也点不到；数据与页面对不上时（`PanelGrid::is_live`）
  只保留标签可点，格子一律不响应。
- [x] **验证**：`cargo build --quiet` 0 错、**无新增警告**（顺手把两个只给单测用的几何包装
  标了 `#[cfg(test)]`）；`cargo test -p winxime-server` **49 通过 / 7 失败**（新增 18 条：
  `recent_usage` 9 + `ui::glyph` 9 + `ui::symbol` 2 + panel 网格 4，表情旧 6 条变 3 条；
  7 失败与基线同源：3 条 plugins 断言 + 4 条 `%TEMP%` 权限，与本次改动无关）。
  **自动化验证仍然只有构建 + 单测。**
- [ ] **待用户目视验证**（需 `.\rebuild.ps1`）：⋮ → 😀 表情 / 🔣 符号 → 标签栏（符号页两行）
  + 32 格网格；在分类页点几个字 → 关掉面板再进 → 「最近」里应看到刚点过的（最近的排最前）；
  在「最近」页连点同一个格 → 位置不跳；符号分类里翻到第 2、3 页；空「最近」显示
  「暂无最近使用」。字形大小 / 格子手感 / 上屏落点这些只有实机能看。
  符号页里 `𝒲` / `🆎` / `ㅞ` / `ɠ` 这几类罕见字形要专门看一眼会不会画成方框（候选中文字体可能没有，
  现在靠 DirectWrite 的系统字体回退兜着）；另外顺手确认「点分类页字形 → 收起 → 重开 → 最近里有它」。
  字形大小 / 格子手感 / 上屏落点这些只有实机能看。
- [ ] **后续（未做）**：点一下就收起面板（上屏通道自带，与剪切板条目一致），「连点几个」
  要单独设计「上屏后保持面板可见」的方案；最近使用记录的管理界面（清空 / 置顶）没做；
  符号分类名只有一个代表字（安卓同款），hover 显示全名没做。

### 2026-10-02 候选栏菜单去掉「计算器」+ 网格页标签栏移到底部、网格铺满宽度

- [x] **菜单去掉 🧮 计算器**（`ui/panel.rs`）：`MENU_DEFS` 7 → **6 张卡片**（📋 剪切板 /
  🚀 快捷发送 / 😀 表情 / 🔣 符号 / 🎙️ 语音输入 / ⚙️ 设置），2 列 × 3 行；菜单页高度
  198 → **162**。菜单里现在每张卡片都能落地（5 张开子页 + 1 张设置）；`ui::view` 里
  「既没有子页也没有动作就弹一句反馈」的分支暂时不可达，**保留**作以后新增卡片的兜底。
- [x] **网格页标签栏移到底部**（表情 / 符号共用版式，两页一起改）：自上而下
  标题栏 → 网格 →（翻页条）→ 标签栏，标签栏贴面板底边。
- [x] **表情页不再留翻页条**：新增 `GlyphKind::needs_paging()`——表情每类恰好 32 个（正好
  一页）、「最近使用」上限也是一页，所以表情页任何标签都不可能多页；符号页「中」类 83 个
  （3 页）仍需要它。翻页条占不占位是**页级**属性，切分类时面板不会忽高忽低。
  面板高度：表情 282 → **242**、符号 **312** 不变。
- [x] **网格铺满可用宽度**（去掉列宽 44 的上限与整体居中）：用户说的「内容两边 padding
  过大」根因就在这里——面板宽度跟着候选栏走，候选行一宽（> 380）网格就居中、两边各空
  一大块。现在列宽按可用宽度均分，两边只留 10（与标签栏、菜单卡片对齐）。
- [x] **「打开面板给最小宽度、关掉恢复随候选词」**：这本来就是现有行为（`ui/layout.rs`：
  展开时 `max(候选行宽, PANEL_MIN_WIDTH = 320)`、收起时就是候选行宽），这次没有改它；
  网格铺满之后它不再是「防网格被挤扁」的手段，只是菜单卡片 / 标签栏的下限。
- [x] **单测**：`ui::panel` 网格版式 4 条 + 菜单 4 条改到新几何（标签栏在下、网格铺满、
  菜单 6 张卡片），`ui::glyph` 新增 1 条 `only_the_symbol_page_needs_a_paging_bar`
  （同时钉住 `recent_usage::MAX_COUNT <= PER_PAGE`）。
- [x] **验证**：`cargo build --quiet` 0 错（无新增警告）；`cargo test -p winxime-server ui::`
  **32 通过 / 0 失败**；全量 `cargo test -p winxime-server` **50 通过 / 7 失败**，7 条与基线
  同源（3 条 plugins 断言 + 4 条 `%TEMP%` 权限，与本次改动无关）。
- [ ] **待用户目视验证**（需 `.\rebuild.ps1`）：⋮ → 菜单只剩 6 张卡片（没有计算器）、面板更矮；
  😀 表情 → 网格在上、分类标签在最底部一行，左右不应再有大片空白；点标签切分类正常；
  🔣 符号 → 翻页条仍在（网格与标签栏之间），两行标签在最底部。
- [ ] **后续（未做）**：用滚动显示全部分类（用户这次又问过）——候选栏窗口始终不获得焦点、
  `WM_MOUSEWHEEL` 送不到它手里，要单独设计（见 DECISIONS.md 2026-10-02 那条）。

### 2026-10-02 设置「词典」页：用户词库词条浏览/搜索（只读）+ 启动自动拉词典列表

- [x] **形态先问后做（先对比、再让用户选）**：把安卓版 Xime 的「词库管理」hub 拆成 4 块，
  与本仓库现状逐块对照后给用户选，用户选「P0 地基 + P1 浏览」：
  - **已有等价物、不照搬**：整本词典 列表/备份/恢复快照/导出/导入（对齐 weasel
    DictManagementDialog）；多设备词库同步 = 「同步与备份」页 + server 侧 rime 原生
    `sync_user_data`（快照目录与桌面 rime 同构）——安卓那套 SAF / 下载目录 / 云备份插件
    外壳在 Windows 上不需要；
  - **本次做**：用户词库**浏览 + 搜索**（只读）+ 词条所在词典的入口；
  - **留到后续功能点**：词条新增/删除（安卓用 `Import` 写一行码表实现，删除是频率 −1 的
    tombstone，写路径要先解决"输入法运行时把 userdb 交出来"）、方案词表只读浏览（纯文件解析，
    最省事）、快捷短语 `custom_phrase`（要动方案分发：注入 `table_translator@custom_phrase`
    + 重新部署）。
- [x] **数据通道：librime 没有"读词条"的 C 接口，走 levers 的导出通道**。新文件
  `crates/winxime-server/src/user_dict.rs`：`librime::export_user_dict` 导出到临时文件
  （`%TEMP%\xime_dict_<dict>_<pid>_<seq>.txt`，读完即删，序列号防同进程并发撞名）→
  解析文本码表 → 关键词过滤（词/码，大小写不敏感）→ 截断 500 条回传。解析只认
  `词⇥码⇥频率`：头部 `#` / `#@` 元信息行与空行跳过、缺列退化（码为空、频率 1）、
  频率 < 0 的 tombstone 不展示（librime 的 formatter 本就不导出，这里再挡一次）。
  词典名先过一遍文件名清洗（只留字母数字与 `_-`），不让名字里的路径字符跑进临时文件名。
- [x] **IPC**（`crates/winxime-ipc`）：`IpcCommand::ListDictEntries` +
  `IpcRequestData::UserDictQuery(dict, query)`、`DictEntry { word, code, commits }`、
  `DictResponse` 增 `entries` / `total`（`count` 在 ListDictEntries 时表示**命中条数**，
  未受回传上限影响；都带 `#[serde(default)]`，新旧进程通讯不炸）、
  `MAX_DICT_ENTRIES = 500`（命名管道单帧上限 1MB，大词库全量回传会超限）、
  客户端 `IpcClient::list_dict_entries(dict, query)`。
- [x] **单测（server 6 条）**：用**真实导出样本**（本机 `rime_ice` 那份带 4 条词条的导出）钉住
  解析跳过头部、缺列退化、tombstone 不展示、词/码过滤与去空白、500 条截断、
  文件名清洗。`cargo test -p winxime-server user_dict` **6 通过**。
- [x] **UI**（libximecore `xime-setup-lib`，本地 patch 路径）：词典页保持**单页**，
  用**页内子视图**切换（本仓库第一个"列表↔详情"，`browse: Option<DictBrowseState>`，
  `None` = 词典列表；返回按钮照剪贴板页 header 的写法）。子视图 = 返回词典列表 / 重新读取 +
  搜索框（`text_input`）+ 状态行（`共 N 条 / 匹配 M 条`，命中被截断时提示补关键词）+
  词条行（词、编码、频率）+ 分页（每页 50 条）。
- [x] **读取是"单飞 + 防抖 + 结果回灌"**：关键词变化只记 `pending`，`BackgroundPoll`
  （已有 250ms 节拍）在**防抖 300ms 到点且没有在途读取**时才发线程；结果经
  `DICT_TASK_OUTCOME` 回灌。避免每敲一个字打一次 IPC（每次都是一次全库扫描），
  也避免多线程结果互相覆盖；关闭子视图后在途结果直接丢弃。
- [x] **顺手修掉"必须先点一次刷新"**：`DictManageState` 改为手写 `Default`（启动即
  `start_refresh()`，与剪贴板页 `SyncPluginUiState::default` 同款）——词典列表为空时页面上
  连「浏览词条」按钮都没有，功能不该藏在"先点一次刷新"后面。
- [x] **单测（UI 7 条）**：分页切片（整页/末页/越界空切片）、页数边界、截断标志、
  关键词变化重置页码并触发防抖（同关键词不重复排队）、翻页夹取、状态文案四态、
  命中被截断的提示文案。libximecore `cargo test -p xime-setup-lib dict_browse` **7 通过**。
- [x] **验证**：`cargo build --quiet` **0 错**（无新增警告；`winxime-tsf` / `ui::paint` 里那几条
  unused 是基线）；`cargo test -p winxime-server` **56 通过 / 7 失败**（50 → 56 为本次新增 6 条；
  7 条失败与基线同源：3 条 plugins 断言 + 2 条 models + 2 条 schema_switches，都是 `%TEMP%`
  权限，与本次改动无关）。**自动化验证仍然只有构建 + 单测。**
- [ ] **待用户目视验证**（需 `.\rebuild.ps1`）：设置 → 词典 → 每本词典多一个「浏览词条」→
  子视图里应看到"词 / 编码 / 频率"列表与搜索框；搜索（词或码）、翻页、返回词典列表、
  重新读取都正常；一本还没造过词的词库应有"这本词典还没有词条"提示。
- [ ] **读路径这次**没有**做"操作前关闭用户词典"**：librime `user_dict_manager.h` 有 CAVEAT
  （Backup/Restore/Export/Import 前 user dict 应处于关闭状态），安卓用 `withUserDictClosed`
  （销毁会话 → 操作 → 重建会话）绕开。**用户实测在输入法运行时导出是成功的**（导出文件里
  4 条真词条），所以读路径先按"直接导出 + 失败给一句人话"（"词库可能正被输入法占用"）处理。
- [ ] **写路径不能这么将就（下一个功能点 P2 的前置）**：增删词条=导入一行码表/一行频率 −1 的
  tombstone，变化可能被运行中的会话覆盖，必须先在 server 侧做"关会话 / 重建会话"
  （销毁会话会丢掉正在输入的 composition，策略要单独定：只在没有活动 composition 时做，
  或临时拦住输入并明确提示）。

### 2026-10-02 词典页三件套：P2 词条增删 + P3 方案词表浏览 + P4 快捷短语

用户要求三块一起做完再验证（「后面三块一起做，做完了我才验证」）。数据链路全部走
server IPC（设置进程不碰 librime、不猜 rime 目录）；UI 侧沿用 P1 的"单飞 + 防抖 +
结果信箱"骨架。

- [x] **P2 用户词条新增 / 删除（写路径）**：
  - **librime 会话装卸**（libximecore `xime-rime/src/engine.rs`）：抽出私有
    `recreate_session()`（建会话 + 恢复 selected_schema，`deploy`/`redeploy` 共用）；
    新增 `with_user_dict_closed(op)`——关会话 → 执行操作 → 重建会话，两级兜底
    （`recreate_session` 失败再试 `redeploy`，都失败报"输入法服务需要重启"）。
    对齐安卓 `withUserDictClosed`；**代价如实告知**：正在输入的句子会丢
    （server 写完后照 FocusOut 清 composition / 候选窗 / suggestion）。
  - **写入 = 导入一行 TSV**（`user_dict.rs`）：`import_entry` 走
    `librime::import_user_dict`（临时文件 `xime_dict_entry_<dict>_<pid>_<seq>.txt`，
    用完即删）；`validate_entry`（trim、非空、拒绝制表/换行、频率非 0）。
    **删除是频率 −1 的 tombstone 不是物理删除**：被删的词再次被输入并选中会复活、
    频率只能调高不能调低（librime 导入合并语义）——按钮与提示文案如实说明。
  - **IPC**：`ImportDictEntry` + `UserDictEntry(dict, word, code, commits)`，结果走
    `dict_response.count`。server handler 在**引擎锁内**执行（要动会话）。
  - **UI**（词典页浏览子视图）：词条行加「删除」（两步确认，取消/确认删除）；
    工具行加「新增词条」→ 模态对话框（词 / 编码 / 频率，`modal_dialog`）；本地先校验
    （频率空 = 1、正整数），写入在途全部写按钮禁用；成功后 notice 提示 + 防抖 300ms
    后自动重读词库。写结果走独立信箱 `DICT_WRITE_OUTCOME`（与读取信箱分开，避免
    在途读取 + 写入互相覆盖）。
- [x] **P3 方案词表只读浏览（输入方案页第 3 个 tab「方案词表」）**：
  - **server 解析**（新 `schema_dict.rs`，子代理完成）：`read_schema_dict(rime_dir,
    schema_id, query)`——主码表 `dictionary:` → `import_tables`（块 + 行内）递归 +
    `translator.packs` 额外根，BFS 去重防环；数据段（`...` 之后）按空白切列，跳空行/
    注释/单列；缺失码表记入 `missing` 继续跑。**签名缓存**（dict_name + packs +
    每个文件的 name@len@mtime，含缺失文件）——首次解析 pinyin_simp 39 万条要几百
    毫秒，缓存后毫秒级。实测 wubi86 → 91397 条（wubi86 + wubi86_extra）。
  - **IPC**：`ListSchemaEntries` + `SchemaQuery(schema_id, query)` +
    `SchemaDictResponse`（dict_name/tables/missing/entries/total/matched）。
    **在引擎锁之外执行**（`process_request` 开头预分发）：纯文件操作不需要会话，
    首次解析大码表持锁会挡住正在打字的键事件（try_lock 失败的键直接失败）。
  - **UI**：输入方案页 tab 栏 3 个标签（已安装 / 已下载 / 方案词表）；码表信息卡
    （方案名 / 主码表 / 读入码表 / 缺失警告）+ 搜索框（防抖 300ms）+ 只读词条行
    （词居左编码居右）+ 分页 50/页。进 tab 或换选中方案自动读一次
    （`schema_dict_ensure`；同一方案读过不重读）。结果信箱 `SCHEMA_DICT_OUTCOME`
    带 schema_id，在途期间换方案的旧结果直接丢弃。
- [x] **P4 快捷短语（词典页子视图，对齐安卓 Xime 的 custom_phrase）**：
  - **格式与安卓逐字对齐**（`custom_phrase.rs`）：文件头 5 行 rime 表头
    （`#@/db_name custom_phrase`、`#@/db_type tabledb`）；条目 `词⇥码[⇥权重]`；
    **整表覆盖写**（表式词典无单条追加 API）。表名解析 `custom_phrase.user_dict`
    （custom.yaml 优先、schema.yaml 兜底、默认 `custom_phrase`），
    `custom_phrase_dict_name` 开为 pub（server 复用，两端同名）。
  - **方案 patch 注入**（`apply_translator_patch`）：首次保存**非空表**时往
    `<id>.custom.yaml` 的 `patch:` 下插入 `engine/translators/+: table_translator@
    custom_phrase` + `custom_phrase` 翻译器块（db_class stabledb、initial_quality 99）；
    幂等（marker 检测）、**注入后永不摘除**（摘了会让已保存的短语失效——安卓同款
    wart，注释里写明）。**不自动部署**：patch 与表内容改动都要重新部署方案才生效。
  - **IPC**：`ListCustomPhrases` / `SaveCustomPhrases` + `CustomPhraseEntry/
    CustomPhraseResponse`（含 file_exists / patch_applied / patch_added）。
    同样在引擎锁之外执行（纯文件操作）。
  - **UI**（词典页「快捷短语」行 → 子视图）：方案 `pick_list`（换目标即重读）+ 表文件
    信息行 + 短语列表（编辑 / 删除两步确认）+ 新增/编辑模态对话框（词 / 编码 / 权重，
    权重空 = 省略列）+ 底部「部署方案」按钮（复用 `Message::DeploySchemas`，不自动
    部署的显式入口）。每次增删改 = 本地改整表 → 整表保存（echo 回读），结果信箱
    `PHRASE_OUTCOME` 带 schema_id 防串台。
- [x] **顺手修的基建**：`PluginHost::rime_dir()`（handler 不得自行重推 rime 目录——
  debug 构建是 `target/debug/user-data`，重推会指向 %APPDATA%，历史 bug）；
  `pipe.rs` `send_request` 超时从"整响应 100ms 总限"改为**逐字节空闲限**（大词表导出
  慢但有效的响应此前会被误判超时）。
- [x] **验证**：`cargo build --quiet` 两仓库 **0 错**；libximecore `cargo test` 全绿
  （xime-setup-lib 34 通过，含新增 `dict_edit_tests` 8 条：频率/权重解析、新增对话框
  校验、两步确认、方案词表分页/截断）；`cargo test -p winxime-server`
  **73 通过 / 7 失败**（56 → 73 新增 17 条：user_dict +2、custom_phrase 6、
  schema_dict 9；7 条失败与基线同源：%TEMP% 权限 + plugins 断言，与本次无关）。
  **自动化验证仍然只有构建 + 单测。**
- [ ] **待用户目视验证**（需 `.\rebuild.ps1`，server 必须重建——新 IPC 命令在 server）：
  - P2：设置 → 词典 → 浏览词条 → 「新增词条」填词/编码 → 添加 → 顶部 notice +
    列表自动重读出现新词；删除 → 确认 → 重新读取后词条消失（导出不含 tombstone）；
    写入期间（打字状态下）正在输入的句子会被打断（提示文案已说明）。
  - P3：设置 → 输入方案 → 「方案词表」→ 显示 wubi86 主码表与词条总数（约 9 万）、
    搜索、翻页；切到拼音方案看大表（约 39 万条，首次解析稍慢）；码表缺失警告形态。
  - P4：设置 → 词典 → 「快捷短语」→ 新增（词/编码/权重）→ 保存后提示重新部署 →
    点「部署方案」→ 打字输入该编码短语上屏；编辑/删除；换方案下拉读不同表。
- [ ] **已知边界（如实告知，不算 bug）**：删除词条是 tombstone（复活语义）；快捷短语
  patch 注入后不摘除（清空短语表只清文件内容）；两条写路径都在用户打字时各有代价
  （P2 丢当前句、P4 要手动部署）。
