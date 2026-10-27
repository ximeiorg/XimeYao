use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use tracing::debug;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    System::Registry::{
        RegCloseKey, RegGetValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_READ, RRF_RT_DWORD,
    },
    UI::{
        Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_STATE, NIF_TIP, NIIF_INFO,
            NIM_ADD, NIM_DELETE, NIM_MODIFY, NIS_HIDDEN, NOTIFYICONDATAW, NOTIFY_ICON_STATE,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreateIconFromResource, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
            DeleteMenu, DestroyMenu, DestroyWindow, GetCursorPos, GetMenuItemCount, LoadCursorW,
            LoadIconW, PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
            TrackPopupMenu, CW_USEDEFAULT, HICON, HMENU, IDC_ARROW, IDI_APPLICATION, MF_BYPOSITION,
            MF_CHECKED, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_COMMAND,
            WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_USER,
            WNDCLASSW, WS_POPUP,
        },
    },
};

const ZH_LIGHT_ICO: &[u8] = include_bytes!("../../../resources/trayicon/zh_light.ico");
const ZH_DARK_ICO: &[u8] = include_bytes!("../../../resources/trayicon/zh_dark.ico");
const EN_LIGHT_ICO: &[u8] = include_bytes!("../../../resources/trayicon/en_light.ico");
const EN_DARK_ICO: &[u8] = include_bytes!("../../../resources/trayicon/en_dark.ico");

pub const WM_TRAY_EVENT: u32 = WM_USER + 100;
const WM_TRAY_UPDATE: u32 = WM_USER + 101;
const WM_TRAY_SHOW: u32 = WM_USER + 102;
const WM_TRAY_HIDE: u32 = WM_USER + 103;

const TRAY_ID: u32 = 1;
const MENU_ID_TOGGLE: u32 = 1001;
const MENU_ID_SETTINGS: u32 = 1002;
const MENU_ID_ABOUT: u32 = 1003;
const MENU_ID_FEEDBACK: u32 = 1004;
const MENU_ID_QUIT: u32 = 1005;
const MENU_ID_SYNC: u32 = 1007;
/// 方案 switches 动态菜单项 id 基址（弹出时重建，id = 基址 + 下标）。
const MENU_ID_SWITCH_BASE: u32 = 2000;

pub enum TrayAction {
    ToggleAsciiMode,
    OpenSettings,
    About,
    Feedback,
    Quit,
    /// rime 用户资料同步（对齐 weasel 托盘「用户资料同步」）。
    SyncUserData,
    /// 切换方案开关：布尔开关（name 非空）取反；多选一（options 轮转）切到下一项。
    ToggleSwitch { name: String, options: Vec<String> },
}

/// 托盘菜单里的一个方案开关项（弹出菜单时由 provider 求值）。
#[derive(Clone, Debug)]
pub struct SwitchMenuItem {
    /// 显示文本（当前状态标签）。
    pub label: String,
    /// 是否勾选（布尔开关的「开」态）。
    pub checked: bool,
    /// 布尔开关名；多选一开关为空。
    pub name: String,
    /// 多选一 option 名列表。
    pub options: Vec<String>,
}

/// 方案开关提供者：每次弹出菜单时求值（读当前方案 switches + 实时取值）。
pub type SwitchProvider = Arc<dyn Fn() -> Vec<SwitchMenuItem> + Send + Sync>;

static mut TRAY_HWND: Option<HWND> = None;
/// 与 `TRAY_HWND` 同一个窗口句柄，给**别的线程**读（气泡通知）。
/// `static mut` 跨线程读是 UB，这里单独放一个原子量。
static TRAY_HWND_RAW: AtomicIsize = AtomicIsize::new(0);
static mut TRAY_MENU: Option<HMENU> = None;
static mut TRAY_ON_ACTION: Option<Arc<dyn Fn(TrayAction) + Send + Sync>> = None;
static mut TRAY_IS_ASCII: bool = false;
static mut TRAY_SWITCH_PROVIDER: Option<SwitchProvider> = None;
/// 最近一次弹出时渲染的开关项（id → 数据）。
static TRAY_SWITCH_ITEMS: Mutex<Vec<SwitchMenuItem>> = Mutex::new(Vec::new());

/// `NOTIFYICONDATAW` 的公共字段（结构大小 / 窗口 / 图标 id）。
/// 每个调用点都要填这三样，抽出来省得六处各写一遍。
fn tray_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
    }
}

/// 写 `szTip`（定长数组，超长截断；传进来的文字需自带结尾 NUL）。
fn set_tip(nid: &mut NOTIFYICONDATAW, text: &str) {
    for (slot, unit) in nid.szTip.iter_mut().zip(text.encode_utf16()) {
        *slot = unit;
    }
}

/// 把文本写进 `NOTIFYICONDATAW` 的定长宽字符缓冲（留结尾 NUL，超长截断）。
fn set_wide(buf: &mut [u16], text: &str) {
    let cap = buf.len().saturating_sub(1);
    for (slot, unit) in buf.iter_mut().zip(text.encode_utf16().take(cap)) {
        *slot = unit;
    }
}

/// 托盘气泡通知（`NIF_INFO`）。Win10/11 会把它显示成通知中心的横幅。
///
/// 为什么需要它：WinRT toast 要求**进程有 MSIX 包身份**，而 MSI 安装到
/// `Program Files` 的进程没有包身份（开发时用 `Start-Process` 直接跑 exe 也一样）。
/// 气泡走的是老 API，只要托盘图标在就能显示，两种安装方式都可用。
///
/// 返回是否被系统接受（托盘图标未创建时返回 `false`）。
pub fn show_balloon(title: &str, body: &str) -> bool {
    let raw = TRAY_HWND_RAW.load(Ordering::Acquire);
    if raw == 0 {
        return false;
    }
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    let mut nid = tray_nid(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    set_wide(&mut nid.szInfoTitle, title);
    set_wide(&mut nid.szInfo, body);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() }
}

pub struct TrayIcon;

impl TrayIcon {
    /// 装上托盘图标：建隐藏窗口 + 弹菜单 + 加图标。
    ///
    /// 托盘图标归 server 独占，但由**当前 owner TIP 实例**上报显隐：
    /// 每个 TIP 实例带自己的 session id，只有 owner 的 Hide 生效，后台实例
    /// 的 Deactivate 不会把前台图标藏掉。TSF 的焦点进出不参与显隐。
    pub fn install(on_action: Arc<dyn Fn(TrayAction) + Send + Sync>, switch_provider: SwitchProvider) {
        let hwnd = Self::create_window();
        let menu = unsafe { CreatePopupMenu().unwrap_or_default() };

        unsafe {
            TRAY_HWND = Some(hwnd);
            TRAY_HWND_RAW.store(hwnd.0 as isize, Ordering::Release);
            TRAY_MENU = Some(menu);
            TRAY_ON_ACTION = Some(on_action);
            TRAY_SWITCH_PROVIDER = Some(switch_provider);

            Self::build_menu(menu);
        }

        Self::add_icon(hwnd);
    }

    fn create_window() -> HWND {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();

            let class_name: Vec<u16> = "XimeTrayWindow\0".encode_utf16().collect();

            let wnd_class = WNDCLASSW {
                lpfnWndProc: Some(Self::window_proc),
                hInstance: HINSTANCE(hinst.0),
                lpszClassName: windows_core::PCWSTR(class_name.as_ptr()),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };

            let _ = RegisterClassW(&wnd_class);

            CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE::default(),
                windows_core::PCWSTR(class_name.as_ptr()),
                windows_core::PCWSTR::null(),
                WS_POPUP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(HINSTANCE(hinst.0)),
                None,
            )
            .unwrap_or_default()
        }
    }

    fn add_icon(hwnd: HWND) {
        unsafe {
            let hicon = load_icon_from_ico(get_icon_for_mode(false));
            let mut nid = tray_nid(hwnd);
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY_EVENT;
            nid.hIcon = hicon;
            set_tip(&mut nid, "中\0");
            let result = Shell_NotifyIconW(NIM_ADD, &nid);
            debug!("Shell_NotifyIconW NIM_ADD result: {:?}", result);
        }
    }

    fn remove_icon(hwnd: HWND) {
        unsafe {
            let nid = tray_nid(hwnd);
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TRAY_EVENT => {
                Self::handle_tray_event(lparam.0 as u32);
                LRESULT(0)
            }
            WM_TRAY_UPDATE => {
                do_update_icon(wparam.0 != 0);
                LRESULT(0)
            }
            WM_TRAY_SHOW => {
                do_show_icon();
                LRESULT(0)
            }
            WM_TRAY_HIDE => {
                do_hide_icon();
                LRESULT(0)
            }
            WM_COMMAND => {
                Self::handle_menu_command(wparam.0 as u32);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                Self::refresh_icon_for_theme_change();
                LRESULT(0)
            }
            WM_DESTROY => {
                Self::remove_icon(hwnd);
                TRAY_HWND = None;
                if let Some(menu) = TRAY_MENU {
                    let _ = DestroyMenu(menu);
                    TRAY_MENU = None;
                }
                TRAY_ON_ACTION = None;
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn refresh_icon_for_theme_change() {
        unsafe {
            if let Some(hwnd) = TRAY_HWND {
                let is_ascii = TRAY_IS_ASCII;
                let hicon = load_icon_from_ico(get_icon_for_mode(is_ascii));
                let tip_str = if is_ascii { "EN\0" } else { "中\0" };
                let mut nid = tray_nid(hwnd);
                nid.uFlags = NIF_ICON | NIF_TIP;
                nid.hIcon = hicon;
                set_tip(&mut nid, tip_str);
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
        }
    }

    fn handle_tray_event(event: u32) {
        match event {
            WM_LBUTTONUP => {
                Self::handle_menu_command(MENU_ID_TOGGLE);
            }
            WM_RBUTTONUP => {
                Self::show_context_menu();
            }
            _ => {}
        }
    }

    fn show_context_menu() {
        unsafe {
            if let (Some(hwnd), Some(menu)) = (TRAY_HWND, TRAY_MENU) {
                Self::build_menu(menu);
                let _ = SetForegroundWindow(hwnd);
                let mut pt = POINT { x: 0, y: 0 };
                let _ = GetCursorPos(&mut pt);
                let _ = TrackPopupMenu(
                    menu,
                    TPM_BOTTOMALIGN | TPM_LEFTALIGN,
                    pt.x,
                    pt.y,
                    None,
                    hwnd,
                    None,
                );
                // KB135788（对齐 weasel SystemTraySDK）：不补 WM_NULL 时菜单
                // 跟踪未正确结束，下一次点击会被当作取消而吞掉（表现为
                // 「第一下无效、第二下才生效」）。
                let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// 调 provider 求值当前方案开关（provider 未设置时为空）。
    fn provider_switch_items() -> Vec<SwitchMenuItem> {
        let Some(provider) = (unsafe {
            #[allow(static_mut_refs)]
            TRAY_SWITCH_PROVIDER.as_ref()
        }) else {
            return Vec::new();
        };
        provider()
    }

    /// 重建整份托盘菜单：固定项 + 当前方案 switches 动态分组（每次弹出
    /// 菜单时求值，方案切换 / 开关状态变化即时反映）。
    fn build_menu(menu: HMENU) {
        unsafe {
            // 清空旧项。
            while GetMenuItemCount(Some(menu)) > 0 {
                if DeleteMenu(menu, 0, MF_BYPOSITION).is_err() {
                    break;
                }
            }
            let toggle_text: Vec<u16> = "切换中/英\0".encode_utf16().collect();
            let settings_text: Vec<u16> = "输入法设置\0".encode_utf16().collect();
            let sync_text: Vec<u16> = "用户资料同步\0".encode_utf16().collect();
            let about_text: Vec<u16> = "关于\0".encode_utf16().collect();
            let feedback_text: Vec<u16> = "反馈\0".encode_utf16().collect();
            let quit_text: Vec<u16> = "退出\0".encode_utf16().collect();

            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_TOGGLE as usize,
                windows_core::PCWSTR(toggle_text.as_ptr()),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows_core::PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_SETTINGS as usize,
                windows_core::PCWSTR(settings_text.as_ptr()),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows_core::PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_SYNC as usize,
                windows_core::PCWSTR(sync_text.as_ptr()),
            );

            // 方案 switches 分组（对齐 Android menubar）：跳过 ascii_mode
            // （顶部已有「切换中/英」），布尔开关显示当前态 + 勾选，多选一
            // 显示当前激活标签（点击轮转）。
            let items = Self::provider_switch_items();
            for (i, item) in items.iter().enumerate() {
                let label: Vec<u16> = format!("{}\0", item.label).encode_utf16().collect();
                let flags = if item.checked {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                let _ = AppendMenuW(
                    menu,
                    flags,
                    (MENU_ID_SWITCH_BASE + i as u32) as usize,
                    windows_core::PCWSTR(label.as_ptr()),
                );
            }
            *TRAY_SWITCH_ITEMS.lock().unwrap_or_else(|e| e.into_inner()) = items;

            if !TRAY_SWITCH_ITEMS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
            {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows_core::PCWSTR::null());
            }
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_ABOUT as usize,
                windows_core::PCWSTR(about_text.as_ptr()),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_FEEDBACK as usize,
                windows_core::PCWSTR(feedback_text.as_ptr()),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows_core::PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_QUIT as usize,
                windows_core::PCWSTR(quit_text.as_ptr()),
            );
        }
    }

    fn handle_menu_command(cmd: u32) {
        unsafe {
            if let Some(ref on_action) = TRAY_ON_ACTION {
                if cmd >= MENU_ID_SWITCH_BASE {
                    // 方案开关项：id = 基址 + 下标，数据取上次渲染的列表。
                    let index = (cmd - MENU_ID_SWITCH_BASE) as usize;
                    let item = {
                        let items = TRAY_SWITCH_ITEMS.lock().unwrap_or_else(|e| e.into_inner());
                        items.get(index).cloned()
                    };
                    if let Some(item) = item {
                        on_action(TrayAction::ToggleSwitch {
                            name: item.name,
                            options: item.options,
                        });
                    }
                    return;
                }
                match cmd {
                    MENU_ID_TOGGLE => on_action(TrayAction::ToggleAsciiMode),
                    MENU_ID_SETTINGS => on_action(TrayAction::OpenSettings),
                    MENU_ID_SYNC => on_action(TrayAction::SyncUserData),
                    MENU_ID_ABOUT => on_action(TrayAction::About),
                    MENU_ID_FEEDBACK => on_action(TrayAction::Feedback),
                    MENU_ID_QUIT => on_action(TrayAction::Quit),
                    _ => {}
                }
            }
        }
    }
}

/// 更新托盘图标状态（中/英）
pub fn update_tray_icon(is_ascii: bool) {
    unsafe {
        TRAY_IS_ASCII = is_ascii;
        if let Some(hwnd) = TRAY_HWND {
            let _ = PostMessageW(
                Some(hwnd),
                WM_TRAY_UPDATE,
                WPARAM(if is_ascii { 1 } else { 0 }),
                LPARAM(0),
            );
        }
    }
}

fn do_update_icon(is_ascii: bool) {
    unsafe {
        if let Some(hwnd) = TRAY_HWND {
            let hicon = load_icon_from_ico(get_icon_for_mode(is_ascii));
            let tip_str = if is_ascii { "EN\0" } else { "中\0" };
            let mut nid = tray_nid(hwnd);
            nid.uFlags = NIF_ICON | NIF_TIP;
            nid.hIcon = hicon;
            set_tip(&mut nid, tip_str);
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

/// 显示托盘图标（当前 owner TIP 激活时）。
pub fn show_icon() {
    unsafe {
        if let Some(hwnd) = TRAY_HWND {
            let _ = PostMessageW(Some(hwnd), WM_TRAY_SHOW, WPARAM(0), LPARAM(0));
        }
    }
}

fn do_show_icon() {
    unsafe {
        if let Some(hwnd) = TRAY_HWND {
            let mut nid = tray_nid(hwnd);
            nid.uFlags = NIF_STATE;
            nid.dwState = NOTIFY_ICON_STATE(0);
            nid.dwStateMask = NIS_HIDDEN;
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

/// 隐藏托盘图标（owner TIP 确认切到其他输入法产品时）。
pub fn hide_icon() {
    unsafe {
        if let Some(hwnd) = TRAY_HWND {
            let _ = PostMessageW(Some(hwnd), WM_TRAY_HIDE, WPARAM(0), LPARAM(0));
        }
    }
}

fn do_hide_icon() {
    unsafe {
        if let Some(hwnd) = TRAY_HWND {
            let mut nid = tray_nid(hwnd);
            nid.uFlags = NIF_STATE;
            nid.dwState = NIS_HIDDEN;
            nid.dwStateMask = NIS_HIDDEN;
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

/// 清理托盘图标和窗口（消息循环退出后调用）
pub fn cleanup() {    unsafe {
        if let Some(hwnd) = TRAY_HWND {
            let nid = tray_nid(hwnd);
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);

            let _ = DestroyWindow(hwnd);
            TRAY_HWND = None;
            TRAY_HWND_RAW.store(0, Ordering::Release);
        }
        if let Some(menu) = TRAY_MENU {
            let _ = DestroyMenu(menu);
            TRAY_MENU = None;
        }
        TRAY_ON_ACTION = None;
    }
}

fn load_icon_from_ico(ico_data: &[u8]) -> HICON {
    unsafe {
        if ico_data.len() < 22 {
            return LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        }

        let image_size =
            u32::from_le_bytes([ico_data[14], ico_data[15], ico_data[16], ico_data[17]]) as usize;

        let image_offset =
            u32::from_le_bytes([ico_data[18], ico_data[19], ico_data[20], ico_data[21]]) as usize;

        if ico_data.len() < image_offset + image_size {
            return LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        }

        let icon_data = &ico_data[image_offset..image_offset + image_size];

        CreateIconFromResource(icon_data, true, 0x00030000)
            .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
    }
}

fn is_system_light_theme() -> bool {
    unsafe {
        let subkey: Vec<u16> =
            "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
                .encode_utf16()
                .collect();
        let value: Vec<u16> = "SystemUsesLightTheme\0".encode_utf16().collect();
        let mut hkey = windows::Win32::System::Registry::HKEY::default();

        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows_core::PCWSTR(subkey.as_ptr()),
            Some(0),
            KEY_READ,
            &mut hkey,
        )
        .is_ok()
        {
            let mut data: u32 = 1;
            let mut data_size = std::mem::size_of::<u32>() as u32;

            let result = RegGetValueW(
                hkey,
                windows_core::PCWSTR::null(),
                windows_core::PCWSTR(value.as_ptr()),
                RRF_RT_DWORD,
                None,
                Some(&mut data as *mut _ as *mut _),
                Some(&mut data_size),
            );

            let _ = RegCloseKey(hkey);

            if result.is_ok() {
                return data != 0;
            }
        }
        true
    }
}

fn get_icon_for_mode(is_ascii: bool) -> &'static [u8] {
    let is_light = is_system_light_theme();
    match (is_ascii, is_light) {
        (false, true) => ZH_LIGHT_ICO,
        (false, false) => ZH_DARK_ICO,
        (true, true) => EN_LIGHT_ICO,
        (true, false) => EN_DARK_ICO,
    }
}
