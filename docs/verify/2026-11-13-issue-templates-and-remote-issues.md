# 2026-11-13 远程 issue 模板与远程 issue 本地登记

> 主题：① 给 `ximeiorg/XimeYao` 写 issue 模板（缺陷 / 功能建议 / 提问），供他人反馈；② 把远程已有 issue 逐条登记为本地未决并排期。

## 1. 平台（含版本）

| 项 | 值 |
| --- | --- |
| 系统 | Windows 11 专业版 10.0.26100 |
| 本仓 | XimeYao 0.5.0，`main`（`D:\project\XimeYao`） |
| 上游 | `ximeiorg/XimeYao` `main` = `3473fef`（PR #28 已合并）；本机账号 `tiengming` 对上游无 push 权限，走 PR |
| 本轮提交 | fork `tiengming/XimeYao`：`main` → `fd6747b`（本地文档登记）；`feat/issue-templates` → `d9c7570`（仅 `.github/ISSUE_TEMPLATE/` 四份文件） |
| 上游 PR | `ximeiorg/XimeYao#31`（`tiengming:feat/issue-templates` → `main`），2026-11-13 开、Docs + Build 双绿后**已合并为 `cb111f4`**；模板已在仓库 Issues → New issue 页生效（用户截图确认：功能建议 / 使用问题 / Blank issue / 两个 contact link 均出现） |
| 取数方式 | GitHub REST API（`api.github.com`），经本机代理 `http://127.0.0.1:10809`，脚本 `target/tmp/gh-fetch.mjs`（取数）与 `target/tmp/gh-api.mjs`（带方法/头/体，用于开 PR 与查 CI）（均为一次性产物） |
| 工具链 | node v24.22.1；Python（仅用于 YAML 解析自检） |

## 2. 远程 issue 快照（取数时间 2026-11-13）

远程共 5 个 issue、25 个 PR（PR 全部已关闭，其中 #28 已合并为 `3473fef`）。仓库既有 label：`bug`、`documentation`、`duplicate`、`enhancement`、`good first issue`、`help wanted`、`invalid`、`question`、`wontfix`。

| 编号 | 标题 | 提交人 | 日期 | 评论数 | 原文摘要 | 本地编号 |
| --- | --- | --- | --- | --- | --- | --- |
| #30 | 数字选字问题 | ztff | 2026-11-12 | 0 | 「正常逻辑为字母区的数字选字，数字小键盘区的混合输入，现在本输入法正好相反」 | U-33 |
| #29 | 万象拼音不能切双拼 | ztff | 2026-11-12 | 0 | 「1. 输入 /zrm 后无切换选项；2. 改 wanxiang.custom.yaml 重新部署后无效果」 | U-34 |
| #27 | 0.4 版本的万象无法上屏 | illidanplus | 2026-11-11 | 1 | 「已打出来，点击空格键码后加 1，按回车则是源码上屏，只能鼠标直接点击上屏」（附 2 张截图） | U-35 |
| #12 | 还不能正常用吗 | dadadafafafa | 2026-09-08 | 2 | 「计划什么时候出正式版啊？」（维护者回复「等待ing」「最近都没时间使用 Windows」） | U-36 |
| #11 | 好期待呀，快出一版我们用用吧！ | chenxin0397 | 2026-08-27 | 0 | 「如题」 | U-37 |

## 3. 模板设计（`.github/ISSUE_TEMPLATE/`）

| 文件 | 形态 | 必填字段 |
| --- | --- | --- |
| `bug_report.yml` | issue form | 客户端版本、系统（类型 + 版本号）、所用输入方案、问题模块、预期实现、实际问题、复现路径、复现频率 |
| `feature_request.yml` | issue form | 想解决的问题、期望的使用方式、涉及模块 |
| `question.yml` | issue form | 客户端版本、问题 |
| `config.yml` | 模板选择器 | `blank_issues_enabled: true`；两个 `contact_links` 指向 README 与本仓 `PROGRESS.md` |

设计要点：

- **缺陷模板收齐复现必需信息**：版本号、系统版本号、方案名、预期/实际、逐步复现路径——对应远程 #27「描述不清晰」的反馈（维护者已在该 issue 下要求补「预期 / 动作 / 实际」）。
- **日志字段指向 `%APPDATA%\Xime\logs\server.log`**（与本仓日志落盘位置一致），并提示先删个人信息。
- **模块下拉项**取自本仓实际模块划分（候选栏 / 设置程序 / 托盘 / 方案市场 / 词典管理 / 语音 / 剪贴板 / 安装升级），便于后续给 issue 打 label 与归类。
- **label 用仓库已有值**（`bug` / `enhancement` / `question`），不新造标签。

## 4. 验证

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 | `python -c "import yaml; [yaml.safe_load(open(f'.github/ISSUE_TEMPLATE/{f}',encoding='utf-8')) for f in ['bug_report.yml','feature_request.yml','question.yml','config.yml']]"` | 四份文件均解析为 dict，无异常 | YAML 语法合法 |
| Windows 11 | `node scripts/doccheck.mjs` | `doccheck: 通过` | 文档结构与链接 |
| Windows 11 | `node scripts/refcheck.mjs` | `refcheck: 通过（N 个文件）` | 章节引用与 D-nnn 引用 |
| Windows 11 | `node target/tmp/gh-fetch.mjs "https://api.github.com/repos/ximeiorg/XimeYao/issues?state=all&per_page=100"` | `200 113568`，解析出 issues=5 / prs=25 | 远程 issue 快照 |
| Windows 11 | `node target/tmp/gh-api.mjs POST "https://api.github.com/repos/ximeiorg/XimeYao/pulls" target/tmp/pr-resp.json target/tmp/pr-body.json` | `201 16284`，PR 编号 31，head `tiengming:feat/issue-templates` @ `d9c7570`，base `main` @ `3473fef`，additions=254 / changed_files=4 | 上游 PR 已开 |
| Windows 11 | `node target/tmp/gh-api.mjs GET ".../commits/d9c7570.../check-runs"` | `200 6111`；`Docs` = completed/success，`Build` = completed/success | PR #31 的 CI 状态 |
| GitHub 网页 | 打开 `ximeiorg/XimeYao` → Issues → New issue | 出现「功能建议（Feature）」「使用问题（Question）」「Blank issue」与两个 contact link（用户截图） | 模板选择器渲染 |

## 5. 缺陷表单缺失：根因与修复（2026-11-13 第二轮）

用户截图（Issues → New issue 页）显示：只有「功能建议（Feature）」「使用问题（Question）」「Blank issue」与两个 contact link，**「缺陷反馈（Bug）」整份表单不出现**，仓库里无任何报错。

### 5.1 根因

`bug_report.yml` 的 `logs` 字段把 `placeholder:` 写在了 `attributes:` **外面**（元素级）：

```yaml
  - type: textarea
    id: logs
    attributes:
      label: 日志片段
      description: |
        可选。`%APPDATA%\Xime\logs\server.log` 中出问题时间附近的内容。
    placeholder: |          # ← 错误位置
      [2026-11-12 10:10:37] ...
```

GitHub 的 issue form 是**整份表单一起校验**——任一 body 元素不合法，整份表单被静默丢弃：New issue 页不显示、仓库也没有任何提示。另外两份表单正常，更像「文件没推上去」，因此排查绕了不少弯路。

### 5.2 已排除的误判方向

- UTF-8 字节无效：raw.githubusercontent 与分支版本均通过严格 UTF-8 解码。
- body 元素数量上限：上游 `golang/go` 的 `11-language-change.yml` 有 17 个元素、`nodejs` 有 9 个，均正常；GitHub 文档无 body 元素数上限。
- 缺 `type: bug` / `title:` 顶层键：schema 允许 `type`；tldraw#8869 那条 `type: Bug` 报错属另一仓库未启用 org issue types。
- JSON SchemaStore 的 `github-issue-forms.json` 无 `maxItems`/`maxLength` 限制（只有 label `maxLength` 39）。
- raw.githubusercontent 抓取内容里的 U+FFFD（`输入���容`）是本机抓取脚本分块解码的假象；改用 `api.github.com/repos/.../contents/...?ref=` 取 base64 blob 复核是干净的。**以后取仓库文件一律走 base64 blob 接口。**
- 未登录访问 `github.com/.../issues/new` 一律返回 "Uh oh! There was an error while loading."（Basic 认证同样），脚本无法当渲染 oracle，只能靠 schema 校验 + 人工核对。

### 5.3 修复与新增机检

- `.github/ISSUE_TEMPLATE/bug_report.yml`：`placeholder:` 移回 `attributes:` 内。
- 新增 `scripts/issueform-check.mjs`（零第三方依赖，按缩进做结构检查）：顶层键白名单、body 元素键白名单、各元素类型 attributes 必填与允许键、validations 白名单、id 合法性与唯一性、label 唯一性、`config.yml` 顶层键。
- `.github/workflows/ci.yml` 的 `docs` job 增加 `issueform-check` 步骤。
- 上游 PR：`ximeiorg/XimeYao#32`（`tiengming:fix/issue-form-bug-template` @ `9b6bcd7` → `main`），2026-11-13 开、CI 双绿后**已合并为 `f1f18e3`**。

### 5.4 验证

| 平台 | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 | `node scripts/issueform-check.mjs` | `issueform-check: 通过（4 个文件）` | 四份表单结构合法 |
| Windows 11 | `node scripts/issueform-check.mjs target/tmp/broken`（内含合并前的旧 `bug_report.yml`） | `issueform-check: 失败（1 项）` + `bug_report.yml: 第 124 行：body 元素下出现非法键 placeholder` | **负向测试**：原缺陷可被机检捕获 |
| Windows 11 | `node scripts/doccheck.mjs` / `node scripts/refcheck.mjs` / `node scripts/issueform-check.mjs` | `doccheck: 通过` / `refcheck: 通过（24 个文件）` / `issueform-check: 通过（4 个文件）` | 三机检全绿 |
| GitHub 网页 | 打开 `ximeiorg/XimeYao` → Issues → New issue | **待用户确认**「缺陷反馈（Bug）」已出现 | 修复合入后的表单渲染 |

## 6. 远程 issue 取证结论（2026-11-13 第三轮）

取证方式：**只读代码与本机配置文件，不运行程序**（按 `AGENTS.md §3` 禁令，实机验证交用户）。读取范围：本仓 `crates/winxime-server/src/`（`calculator.rs`、`ipc_server.rs`、`ui/model.rs`、`schema_switches.rs`、`main.rs`、`tray.rs`）、本仓 `rime-wubi/`（内置方案配置）、上游 librime 源码（`switcher.cc`、`selector.cc`、`key_binder.cc`、`switch_translator.cc`、`switches.cc`、`key.rs`）、本机 `%APPDATA%\Xime\rime` 现场配置。

| 编号 | 远程 issue | 取证结论 | 仍需用户补充 |
| --- | --- | --- | --- |
| U-33 | #30 数字选字 | **候选态下数字选字不由本仓接管**：librime selector 先按 `select_keys` 查表，否则 `XK_0..XK_9` → `index = ((ch - XK_0) + 9) % 10`，小键盘 `XK_KP_0..XK_KP_9` 同式，**主键盘与小键盘数字都能选第 N 个候选**。被抢的是**非组词态**：组词串为空且无建议词时，内置计算器的「数字开头影子缓冲」接管按键（`calculator::is_shadow_starter`），而它**不区分主键盘数字与小键盘数字**（`0x30..=0x39` 与 `XK_KP_0..=XK_KP_9` 同等）。该开关默认开（配置缺键即 true），可在设置页「外观 → 数学计算」关闭 | 哪个应用；当时有没有候选栏（是否在组词中）；按的是主键盘还是小键盘数字，期望选中第几个 |
| U-34 | #29 万象切双拼 | `/zrm` 选方案**不是 rime 内置能力**，依赖方案自带的 lua 脚本；本仓 librime 以 `ENABLE_EXTERNAL_PLUGINS=OFF` 构建（上游 `crates/librime-sys2/build.rs`），外部 lua 插件不加载，这类脚本必然无反应——这解释了「改 wanxiang.custom.yaml 重新部署后无效果」。另：librime **原生支持多选一开关**（schema 里 `options:` 列表 → radio group，会出现在切换面板），本仓托盘菜单 `read_schema_switches` 只解析带 `states` 的项，暴露面比切换面板窄 | 是否需要 rime-lua（体积会变大）；万象是否真在方案清单里；`wanxiang.custom.yaml` 的具体内容 |
| U-35 | #27 万象无法上屏 | **鼠标点选与键盘路径本就不同**：点选走 `eng.select_candidate(index)`（成功即提交并隐藏候选栏）；空格/回车由**方案自带的按键绑定**决定——本仓内置方案 `rime-wubi/default.yaml` 绑定「空格 = 选第一候选」「数字 1–9 = 选候选」「回车 = 提交当前编码（非选词）」，方案若自带 default.yaml 而未绑这些键，空格/回车就落到普通输入路径、把编码原样上屏。另：组词串为空时数字会被内置「数学计算」接管 | 客户端版本（0.4.0 还是 0.5.0）；方案名；原始编码与期望上屏的词；候选栏亮着时按空格候选栏是否立刻消失 |
| U-36 | #12 问正式版 | 非缺陷，是发布节奏提问；对外最新正式版仍是 **v0.4.0（2026-11-10）**，本仓开发版本已到 **0.5.0 但尚未发 release** | 无（等发版节奏确定后回帖） |
| U-37 | #11 期待类 | 期待类反馈，无具体问题；与 U-36 同批答复 | 无 |

### 6.1 回复草稿（第三轮存档；第四轮已按用户放行发布）

第三轮只在本档存档草稿，**当时远程 5 个 issue 一条都未回复、也未加任何标签**（用户 2026-11-13 决定：先过目，再解决真实存在的问题）。第四轮用户放行后，这五条回复与标签**已全部发布**（见第 7 节「远程回复与标签发布记录」）。草稿原文如下：

- **#30**：追问出问题的应用、当时有没有候选栏、按的是主键盘数字还是小键盘数字（期望选第几个）；补充已知行为——「设置 → 外观 → 数学计算」开关默认开，组词串为空时主键盘数字与小键盘数字都会被当作算式起点，这一步还没区分两个数字区。
- **#29**：说明 `/zrm` 不是 rime 内置能力，依赖方案自带 lua 脚本，而本仓构建关闭了外部插件，所以 `/zrm` 无反应；追问万象版本、方案列表里能否看到 wanxiang、`wanxiang.custom.yaml` 的具体内容；并问用户是否需要 rime-lua（体积会变大）以便排期。
- **#27**：按「预期 / 动作 / 实际」追问客户端版本（0.4.0 还是 0.5.0）、方案名、原始编码与期望上屏的词、候选栏亮着时按空格是否立刻消失；说明空格/回车行为由方案自带按键绑定决定（内置方案默认空格 = 选第一候选、数字 1–9 = 选候选、回车 = 提交编码；未绑则走普通输入路径把编码原样上屏）；并提示组词串为空时数字会被内置「数学计算」接管。
- **#12**：说明对外最新正式版是 v0.4.0（2026-11-10），仓库开发版本已到 0.5.0（候选栏与面板调整、方案市场安装/卸载修复、词典错误提示文案），正式版打包与发布节奏还没排期，确定后回帖同步。
- **#11**：告知开发版 0.5.0、正式版仍 v0.4.0，节奏未定，进度统一在 #12 同步。

## 7. 第四轮：Lua 默认启用、custom 补丁平铺、关于页版本号（2026-11-13）

第四轮起因是两条新信息：用户 m01137 要求「默认构建需要 lua」并自证「改 `wanxiang.custom.yaml` 后重新部署无效果」是否 bug；远程 #29 反馈者补充「改的是 `<rime>\custom\wanxiang.custom.yaml`，客户端 0.4.0 但关于页显示 1.0」。

### 7.1 两个真实缺陷与两处修复

| 缺陷 | 根因 | 修复位置 | 证据 |
| --- | --- | --- | --- |
| 默认构建不带 lua | 上游 `crates/librime-sys2/build.rs` 只在 unix 传 `-DENABLE_EXTERNAL_PLUGINS=OFF`，Windows 走 `build.bat` 完全不传插件参数；librime 的 `plugins/` 目录**不在 git 跟踪内**（`git ls-files plugins` 只有 `CMakeLists.txt`/`plugin.cc`/`plugins_module.cc`），rime-lua 是独立仓 `hchunhui/librime-lua`，其 lua 源码只在 `thirdparty` 分支 | 上游 libximecore `crates/librime-sys2/build.rs` 新增 `ensure_lua_plugin()`：构建前探测 `<librime>/plugins/lua/CMakeLists.txt` 与 `<librime>/plugins/lua/thirdparty/lua5.4/lua.h`，缺失则 `git clone --depth 1` 拉 rime-lua、再 `git clone --depth 1 --branch thirdparty` 拉 lua 源码；失败时删掉半个插件目录（否则 CMake `FATAL_ERROR "Lua not found"` 会打断整个构建） | 上游 PR `ximeiorg/libximecore#10`；本仓内置方案 `rime-wubi/wubi86.schema.yaml`、`wubi98.schema.yaml`、`pinyin_simp.schema.yaml`、`wubi86_pinyin.schema.yaml` 均引用 `lua_translator@*uuid` / `lua_translator@*date_translator`，此前在默认构建里**静默失效** |
| 改 `custom/` 子目录里的 custom 文件不生效 | librime 只在**方案同目录**找 `<schema>.custom.yaml`：`AutoPatchConfigPlugin::ReviewCompileOutput` 生成 `PatchReference{remove_suffix(id,".schema")+".custom", "patch", optional}`，资源 id 由 `ToResourceId` 按所在目录拼出。方案包（万象等）把补丁模板放在 `custom/` 子目录里，用户改完那份再「重新部署」自然没有效果——正是远程 #29 的现象 | 上游 libximecore `crates/xime-setup/src/state.rs` 新增 `flatten_schema_custom_templates()`：安装释放后把**本包自带方案**的 `custom/<schema>.custom.yaml` 平铺到 rime 根目录；根目录已有同名文件则**不覆盖**（保护用户已改内容）；`custom/` 下副本保留（万象的 lua 把它当模板来源） | 上游 PR `ximeiorg/libximecore#10`；回归测试 `flatten_custom_templates_puts_schema_patches_at_rime_root`、`flatten_custom_templates_keeps_user_edited_root_file`（`cargo +1.95 test -p xime-setup-lib` → 50 passed） |

### 7.2 关于页版本号显示 1.0（远程 #29 反馈）

- 根因在上游 `xime-config`：`app_metadata()` 用 `OnceLock<AppMetadata>` + `get_or_init(AppMetadata::default)`，**一旦被读过一次就不可覆盖**；而设置页过去的顺序是「先 `init_logging_with_console("setup")`、后 `set_app_metadata(...)`」，日志初始化会经 `build_file_writer → get_log_dir → get_data_dirs → default_rime_paths → app_metadata().config_dir_name` **读**一次元数据，于是 OnceLock 被默认值（`version: "1.0"`）占住，之后 `set_app_metadata` 返回 `Err("app metadata already set")`，关于页读到的仍是默认值。
- `git show v0.4.0:crates/winxime-setup/src/main.rs` 确认 0.4.0 顺序相同 → **该缺陷自 0.4.0 起就存在**。`crates/winxime-server/src/main.rs` 顺序正确（`set_app_metadata` 在 `init_logging_with_console` 之前），server 侧不受影响；TSF DLL 从不注入元数据，影响面仅设置页。
- 本仓修复：`crates/winxime-setup/src/main.rs` 把 `set_app_metadata(...)` 移到 `init_logging_with_console("setup")` **之前**，并加注释说明根因。
- 上游加固：libximecore PR `ximeiorg/libximecore#11` 把元数据持有者从 `OnceLock` 换成 `MetadataCell`（`Mutex<&'static AppMetadata>` + `Mutex<bool>`）——**默认值可被一次显式注入覆盖**，第二次及以后仍报 `Err("app metadata already set")`；新增回归测试 `injected_metadata_overrides_default_after_first_read`、`second_injection_is_rejected`（`cargo +1.95 test -p xime-config metadata` → 4 passed）。

### 7.3 远程回复与标签发布记录（第四轮，按用户放行）

| issue | 回复评论 | 标签 | 回复要点 |
| --- | --- | --- | --- |
| #29 万象切双拼 | [issuecomment-6849973422](https://github.com/ximeiorg/XimeYao/issues/29#issuecomment-6849973422) | `bug` | 说明两个原因（`/zrm` 靠万象自带 `lua_translator@*wanxiang.set_schema`，依赖 librime-lua 而默认构建没编进去；rime 只在方案同目录找 custom 文件）+ 已做的两处修复 + 上游 PR #10 + 追问改的是哪个路径的文件与万象版本 |
| #30 数字选字 | [issuecomment-6849978916](https://github.com/ximeiorg/XimeYao/issues/30#issuecomment-6849978916) | `question` | 追问三个问题（哪个应用 / 是否有候选栏 / 主键盘还是小键盘数字）；说明候选态下 librime 原生支持数字选字，被抢的是无组词串态的「数学计算」影子缓冲 |
| #27 万象无法上屏 | [issuecomment-6849984251](https://github.com/ximeiorg/XimeYao/issues/27#issuecomment-6849984251) | `question` | 按「预期 / 动作 / 实际」追问版本、方案名、原始编码与期望上屏词；说明点选与键盘路径不同、空格/回车由方案按键绑定决定；提到 Lua 修复随 0.5.0 发布 |
| #12 问正式版 | [issuecomment-6849989663](https://github.com/ximeiorg/XimeYao/issues/12#issuecomment-6849989663) | `question` | 对外正式版 v0.4.0（2026-11-10），开发版 0.5.0，正式版节奏待排期，确定后在本 issue 同步 |
| #11 期待类 | [issuecomment-6849994840](https://github.com/ximeiorg/XimeYao/issues/11#issuecomment-6849994840) | `question` | 开发版 0.5.0、正式版 v0.4.0，节奏未定，进度统一在 #12 同步 |

### 7.4 本仓依赖升级

`Cargo.toml` 的 8 个 libximecore 依赖 rev 由 `04d6ca9` 升到上游 main `64580b580e453a288c7fec053e0849ed05c3282e`（含 PR #7 方案包 id 归一化、#8 aes-gcm 弃用、#9 clipboard-page 门控修复、**#10 lua 默认启用与 custom 补丁平铺**、**#11 元数据允许后注入**）。`xime-p2p` 一并由 `b19ecde` 升到 `ca07a02`（2026-11-16，`D-016`）：升 libximecore rev 后锁文件里会出现两份 xime-p2p（core 内部已是 `ca07a02`），本仓统一到同一份实例；`ca07a02` 新增 `Event::Message { topic, payload }`（协议 v2 通用 topic 消息，帧类型 `MsgType::Message = 0x20`），`handle_p2p_event` 的 `match` 因此报 `E0004`，Windows 端只用剪贴板通道（`ClipboardSet 0x08`）、未订阅 topic，故新增分支只记 `trace!` 日志、行为不变。本仓三项机检在该 rev 上全绿：

| 平台（含版本） | 命令或步骤 | 实际输出 | 覆盖范围 |
| --- | --- | --- | --- |
| Windows 11 / rustc 1.98.0-nightly | `cargo +nightly-2026-07-03 build --quiet` | exit 0 | 全 workspace 编译（含新 rev 的 8 个 core crate） |
| Windows 11 / rustc 1.98.0-nightly | `cargo +nightly-2026-07-03 clippy --workspace --all-targets` | 0 warning（仅 build script 提示 rime.dll 输出路径） | 全 workspace lint |
| Windows 11 / rustc 1.98.0-nightly | `cargo +nightly-2026-07-03 test --workspace` | 192 passed / 0 failed / 1 ignored | 全 workspace 单测 |
| Windows 11 / rustc 1.95 | `cargo +1.95 test -p xime-setup-lib`（上游） | 50 passed / 0 failed / 1 ignored | 含两个 custom 平铺回归用例 |
| Windows 11 / rustc 1.95 | `cargo +1.95 test -p xime-config metadata`（上游） | 4 passed / 0 failed | 元数据注入两个新用例 |

### 7.5 `Cargo.lock` 可复现性（2026-11-16 补记）

本地 `.cargo/config.toml` 用 `[patch."https://github.com/ximeiorg/libximecore.git"]` 把 8 个 core crate 指向本地克隆 `D:\project\libximecore`。**只要带着这个 patch 跑 cargo，锁文件里的 git source 行就会被抹掉**（`librime` / `librime-sys2` / `xime-config` 等条目 `source` 消失，workspace crate 版本还会出现 0.4.0→0.5.0 的噪声 diff），这样的锁文件提交上去 CI 无法复现。

可复现的做法：移走 `.cargo/config.toml` → `git checkout upstream/main -- Cargo.lock` → `cargo update --precise <libximecore-sha> -p xime-config` → 移回 config。核对口径：`Select-String -Path Cargo.lock -Pattern 'libximecore.git'` 计数应为 7（librime / librime-sys2 / xime-config / xime-plugin / xime-rime / xime-setup-lib / xime-speech；`xime-ipc` 未出现在锁文件里——本仓用的是自己的 `crates/winxime-ipc`），`path+file` 应为 0，`libximep2p.git?rev=b19ecdec` 应为 0。

**踩过的坑**：`cargo update -p xime-config --precise <rev>` 对 git 依赖报 `package ID specification did not match any packages`，写成 `cargo update --precise <sha> -p xime-config` 即可；单独再更新 xime-p2p 时会因旧条目已被移除而报同样的错，属正常——核对锁文件里只剩 `ca07a02` 一份即可。

## 8. 未验证项

- 修复合入上游后，**「缺陷反馈（Bug）」是否真的出现在 New issue 页仍需用户在网页上确认**（机检只能证明结构合法，不能证明 GitHub 渲染）。
- 模板在 GitHub 网页上的渲染已由用户截图确认（模板选择器可用）；**三个表单各自的必填项校验与下拉提交**尚未逐个走查。
- 远程 issue 的行为根因（U-33 ~ U-35）本轮为**代码与配置文件层面的取证**，尚未本机复现：U-33 的「哪个应用 / 是否组词中 / 哪个数字区」仍待用户回答；U-34 的 rime-lua 需求与万象是否在方案清单内待用户回答；U-35 待用户补版本与「预期/动作/实际」后再复现（0.5.0 与 0.4.0 行为可能已不同）。
- #27 附带的两张截图（`github.com/user-attachments`）本轮未下载细看。
- lua 默认启用与 custom 平铺两处修复**已在上游 libximecore PR #10 合并**（2026-11-16），本仓 8 个 core 依赖随之升到上游 main `64580b5`，**0.5.0 打包出来的 rime.dll 会带 lua**；但**本机 `D:\project\libximecore` 克隆的 `plugins/lua` 目录需要在构建时联网拉取**（CI 与发布流水线同样需要能访问 github）。
- 关于页版本号修复已在本仓落地并通过编译，上游加固 PR #11 **已合并**（2026-11-16，本仓 rev 已升到 `64580b5`）；**关于页显示真实版本号尚未实机确认**（需用户打开设置页「关于」查看）。
- 远程 5 个 issue 的回复与标签已发布（第 7.3 节），**回复后的用户反馈尚未收到**。
- 升级 rev 后 `Cargo.lock` 里出现**两份 xime-p2p**（本仓直钉 `b19ecde` + libximecore 内部用 `ca07a02`）——2026-11-16 已把本仓统一到 `ca07a02`（`D-016`），进程内只剩一份实例；但**剪贴板 P2P 与设置页配对是否共用同一份身份与端口，仍需两台机器实机验证**（步骤见 `docs/verify/2026-11-10-clipboard-p2p-host.md` 末节）。协议 `PROTOCOL_VERSION` 仍为 1，与旧版本客户端的互通也未验证。

## 9. 关联

- 决策：`D-012`（模板与本地登记）、`D-013`（坏表单静默消失的根因与新增机检）、`D-014`（远程 issue 先本地取证、回复与标签由用户放行）、`D-015`（默认构建启用 lua、方案包 custom 补丁平铺、关于页版本号）、`D-016`（xime-p2p 升到协议 v2，与 libximecore 保持同一份实例）。
- 未决映射见 `PROGRESS.md` 未决表 U-33 ~ U-37。
- 上游 PR：`ximeiorg/libximecore#10`（lua 插件 + custom 平铺，**已合并**）、`ximeiorg/libximecore#11`（元数据允许在读到默认值后再注入，**已合并**）、`ximeiorg/XimeYao#34`（本仓：关于页版本号修复 + rev 升 `64580b5` + xime-p2p `ca07a02` + docs 首次随代码上上游，**已创建待合并**）。
