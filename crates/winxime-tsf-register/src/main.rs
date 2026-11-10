use std::{env, fs, process};
use windows::{
    core::*,
    Win32::{
        Foundation::{E_FAIL, WIN32_ERROR},
        Globalization::*,
        System::{
            Com::*,
            Console::{AttachConsole, ATTACH_PARENT_PROCESS},
            LibraryLoader::{GetProcAddress, LoadLibraryW},
            Registry::{
                RegDeleteKeyValueW, RegDeleteKeyW, RegOpenKeyW, HKEY, HKEY_CURRENT_USER,
                HKEY_LOCAL_MACHINE,
            },
        },
        UI::{Input::KeyboardAndMouse::HKL, TextServices::*},
    },
};
use winxime_ipc::IpcClient;

windows::core::link!("input.dll" "system" fn InstallLayoutOrTip(psz: *const u16, dwFlags: u32));
const ILOT_INSTALL: u32 = 0x00000000;
const ILOT_UNINSTALL: u32 = 0x00000001;

const XIME_TSF_CLSID: GUID = GUID::from_u128(0x5C1E4D8A_F3B2_4A7E_9CD1_2A3B4C5D6E7F);
const XIME_PROFILE_GUID: GUID = GUID::from_u128(0x5C1E4D8A_F3B2_4A7E_9CD1_2A3B4C5D6E7F);
const XIME_TIP_DESC: PCWSTR =
    w!("0x0804:{5C1E4D8A-F3B2-4A7E-9CD1-2A3B4C5D6E7F}{5C1E4D8A-F3B2-4A7E-9CD1-2A3B4C5D6E7F}");

const CATEGORIES: [GUID; 7] = [
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    GUID_TFCAT_TIPCAP_COMLESS,
];

fn get_system_dir() -> std::path::PathBuf {
    let system_root = env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    std::path::PathBuf::from(system_root).join("System32")
}

fn copy_dll_to_system(source: &std::path::Path) -> Result<std::path::PathBuf> {
    let system_dir = get_system_dir();
    let dest = system_dir.join("winxime_tsf.dll");

    if dest.exists() {
        fs::remove_file(&dest).ok();
    }

    fs::copy(source, &dest).map_err(|_| Error::from(E_FAIL))?;
    println!("  Copied {} -> {}", source.display(), dest.display());

    Ok(dest)
}

fn remove_dll_from_system() -> Result<()> {
    let system_dir = get_system_dir();
    let dll_path = system_dir.join("winxime_tsf.dll");

    if dll_path.exists() {
        fs::remove_file(&dll_path).map_err(|_| Error::from(E_FAIL))?;
        println!("  Removed {}", dll_path.display());
    }

    Ok(())
}

fn get_user_data_dir() -> std::path::PathBuf {
    env::var("APPDATA")
        .map(|p| std::path::PathBuf::from(p).join("Xime"))
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn remove_user_data_dir() -> Result<()> {
    let user_data_dir = get_user_data_dir();

    if user_data_dir.exists() {
        fs::remove_dir_all(&user_data_dir).map_err(|_| Error::from(E_FAIL))?;
        println!("  Removed {}", user_data_dir.display());
    }

    Ok(())
}

fn get_install_dir() -> std::path::PathBuf {
    let exe_path = std::env::current_exe().ok();
    exe_path
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("C:\\Program Files\\Xime"))
}

fn remove_install_dir() -> Result<()> {
    let install_dir = get_install_dir();

    if !install_dir.exists() {
        println!("  Install directory does not exist");
        return Ok(());
    }

    if install_dir.to_string_lossy().contains("Program Files")
        || install_dir
            .to_string_lossy()
            .contains("Program Files (x86)")
    {
        let cmd = std::process::Command::new("cmd")
            .args([
                "/c",
                "ping",
                "127.0.0.1",
                "-n",
                "2",
                ">nul",
                "&",
                "rmdir",
                "/s",
                "/q",
                install_dir.to_string_lossy().as_ref(),
            ])
            .spawn();

        match cmd {
            Ok(_) => println!(
                "  Scheduled removal of {} (will execute after this process exits)",
                install_dir.display()
            ),
            Err(e) => println!("  Failed to schedule removal: {:?}", e),
        }
    } else {
        println!(
            "  Skipping {} (not in Program Files, appears to be dev directory)",
            install_dir.display()
        );
    }

    Ok(())
}

fn remove_registry_keys() -> Result<()> {
    // SAFETY: 根键是系统预定义句柄；键名由 `w!` 生成，是 NUL 结尾的静态宽串；只做删除，
    // 键不存在时按返回码 2 走正常分支。
    unsafe {
        let hklm = HKEY_LOCAL_MACHINE;

        let xime_key = w!("SOFTWARE\\Xime");
        let result = RegDeleteKeyW(hklm, xime_key);
        if result == WIN32_ERROR(0) || result == WIN32_ERROR(2) {
            println!("  Removed HKLM\\SOFTWARE\\Xime");
        }

        let run_key = w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run");
        let xime_server = w!("XimeServer");
        let _ = RegDeleteKeyValueW(hklm, run_key, xime_server);
        println!("  Removed Run startup entry");

        let hkcu = HKEY_CURRENT_USER;
        let xime_cu_key = w!("SOFTWARE\\Xime");
        let result = RegDeleteKeyW(hkcu, xime_cu_key);
        if result == WIN32_ERROR(0) || result == WIN32_ERROR(2) {
            println!("  Removed HKCU\\SOFTWARE\\Xime");
        }
    }

    Ok(())
}

fn register_dll(dll_path: &str) -> Result<()> {
    // SAFETY: 路径转成 NUL 结尾的 UTF-16 并在本块内存活；DLL 由 LoadLibraryW 保持加载；
    // GetProcAddress 取的是本项目自己的 TSF DLL，`DllRegisterServer` 的签名由 COM 约定，
    // transmute 只把 FARPROC 转成同一 ABI 的函数指针。
    unsafe {
        let dll_path_wide: Vec<u16> = dll_path.encode_utf16().chain(std::iter::once(0)).collect();
        let hmodule = LoadLibraryW(PCWSTR::from_raw(dll_path_wide.as_ptr()))?;

        let Some(proc) = GetProcAddress(hmodule, s!("DllRegisterServer")) else {
            return Err(Error::from(E_FAIL));
        };

        let func: extern "system" fn() -> HRESULT = std::mem::transmute(proc);
        let result = func();
        if result.is_err() {
            return Err(Error::from(result));
        }
    }
    Ok(())
}

fn unregister_dll(dll_path: &str) -> Result<()> {
    // SAFETY: 同 register_dll；导出符号 `DllUnregisterServer` 由同一 DLL 提供，签名按 COM 约定。
    unsafe {
        let dll_path_wide: Vec<u16> = dll_path.encode_utf16().chain(std::iter::once(0)).collect();
        let hmodule = LoadLibraryW(PCWSTR::from_raw(dll_path_wide.as_ptr()))?;

        let Some(proc) = GetProcAddress(hmodule, s!("DllUnregisterServer")) else {
            return Err(Error::from(E_FAIL));
        };

        let func: extern "system" fn() -> HRESULT = std::mem::transmute(proc);
        let result = func();
        if result.is_err() {
            return Err(Error::from(result));
        }
    }
    Ok(())
}

fn register(icon_path: String) -> Result<()> {
    // SAFETY: COM 已在 main 里按 STA 初始化；CLSID/GUID 与注册表键均为常量；profile 名、
    // 图标路径的宽串都在本块内存活到调用返回。
    unsafe {
        let input_processor_profile_mgr: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;

        let pw_icon_path = icon_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();

        let mut lcid = LocaleNameToLCID(w!("zh-CN"), 0);
        if matches!(lcid, 0 | 0x0C00 | 0x1000) {
            lcid = 0x804;
        }

        input_processor_profile_mgr.RegisterProfile(
            &XIME_TSF_CLSID,
            lcid as u16,
            &XIME_PROFILE_GUID,
            w!("曦码·曜输入法").as_wide(),
            &pw_icon_path,
            0,
            HKL::default(),
            0,
            false,
            0,
        )?;

        let category_manager: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        for tfcat in &CATEGORIES {
            category_manager.RegisterCategory(&XIME_TSF_CLSID, tfcat, &XIME_TSF_CLSID)?;
        }
    }

    Ok(())
}

fn unregister() -> Result<()> {
    // SAFETY: 同 register；删除分类与语言配置只依赖常量 CLSID/GUID，宽串常量由 `w!` 生成。
    unsafe {
        let input_processor_profile_mgr: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;

        let category_manager: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        for tfcat in &CATEGORIES {
            if let Err(error) =
                category_manager.UnregisterCategory(&XIME_TSF_CLSID, tfcat, &XIME_TSF_CLSID)
            {
                println!("Failed to unregister category {tfcat:?}: {error}");
            }
        }

        let mut lcid = LocaleNameToLCID(w!("zh-CN"), 0);
        if matches!(lcid, 0 | 0x0C00 | 0x1000) {
            lcid = 0x804;
        }
        input_processor_profile_mgr.UnregisterProfile(
            &XIME_TSF_CLSID,
            lcid as u16,
            &XIME_PROFILE_GUID,
            0,
        )?;

        let hklm = HKEY_LOCAL_MACHINE;
        let clsid_key = w!("CLSID\\{5C1E4D8A-F3B2-4A7E-9CD1-2A3B4C5D6E7F}");
        let _ = RegDeleteKeyW(hklm, clsid_key).ok();

        let tip_key = w!("SOFTWARE\\Microsoft\\CTF\\TIP\\{5C1E4D8A-F3B2-4A7E-9CD1-2A3B4C5D6E7F}");
        let _ = RegDeleteKeyW(hklm, tip_key).ok();
    }

    Ok(())
}

fn enable() {
    // SAFETY: XIME_TIP_DESC 是 NUL 结尾的静态宽串常量，生命周期覆盖调用。
    unsafe {
        InstallLayoutOrTip(XIME_TIP_DESC.as_ptr(), ILOT_INSTALL);
    }
}

fn disable() {
    // SAFETY: 同 enable；只做注销，字符串仍是静态常量。
    unsafe {
        InstallLayoutOrTip(XIME_TIP_DESC.as_ptr(), ILOT_UNINSTALL);
    }
}

fn stop() {
    if IpcClient::shutdown_server() {
        println!("Server stopped via IPC");
        return;
    }

    println!("IPC shutdown failed, trying taskkill...");
    let result = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "winxime-server.exe"])
        .output();

    match result {
        Ok(output) => {
            if output.status.success() {
                println!("Server killed via taskkill");
            } else {
                println!("Server may not be running");
            }
        }
        Err(_) => {
            println!("Server may not be running");
        }
    }
}

/// 取第 `n` 个命令行参数（去掉外层引号）；缺失时打印用途并退出。
///
/// 这个程序是 MSI/脚本调用的控制台工具，参数缺失属于用法错误，
/// 直接给出提示 + 非零退出码，比 panic 打印栈对安装日志更友好。
fn arg_at(n: usize, what: &str) -> String {
    match env::args().nth(n) {
        Some(value) => value.trim_matches('"').to_string(),
        None => {
            println!("缺少{what}参数");
            process::exit(1);
        }
    }
}

fn main() -> Result<()> {
    // SAFETY: 命令行入口先挂父控制台再按 STA 初始化 COM；两者失败都不影响参数分发，
    // 后续 CoCreateInstance 失败会按 Result 返回错误。
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();

        let exe_path = match env::current_exe() {
            Ok(path) => path,
            Err(e) => {
                println!("无法获取 exe 路径: {e}");
                process::exit(1);
            }
        };
        let Some(exe_dir) = exe_path.parent() else {
            println!("无法获取 exe 目录");
            process::exit(1);
        };

        if env::args().len() == 1 {
            println!("Usage:");
            println!("  winxime-tsf-register -install         完整安装(复制DLL到System32+注册)");
            println!("  winxime-tsf-register -uninstall       完整卸载");
            println!("  winxime-tsf-register -register <DllPath> <IconPath>  HKLM注册(不复制DLL,需要管理员权限)");
            println!(
                "  winxime-tsf-register -unregister <DllPath>           HKLM卸载(需要管理员权限)"
            );
            println!(
                "  winxime-tsf-register -register-user <DllPath> <IconPath>  用户级注册(不复制DLL)"
            );
            println!("  winxime-tsf-register -unregister-user <DllPath>           用户级卸载");
            println!("  winxime-tsf-register -register-only <IconPath>            仅注册Profile(DLL已在System32)");
            println!("  winxime-tsf-register -unregister-only                    仅注销Profile");
            println!(
                "  winxime-tsf-register -copy-to-system <DllPath>           仅复制DLL到System32"
            );
            println!("  winxime-tsf-register -copy-and-register <DllPath> <IconPath>  复制DLL到System32+注册(MSI安装用)");
            println!(
                "  winxime-tsf-register -remove-from-system                仅删除System32中的DLL"
            );
            println!(
                "  winxime-tsf-register -unregister-and-remove             MSI卸载(保留用户数据)"
            );
            println!("  winxime-tsf-register -full-uninstall                    完整卸载(删除用户数据,脚本用)");
            println!("  winxime-tsf-register -r <IconPath>    注册TSF Profile");
            println!("  winxime-tsf-register -dll             注册DLL");
            println!("  winxime-tsf-register -i               启用输入法");
            println!("  winxime-tsf-register -d               停用输入法");
            println!("  winxime-tsf-register -s               停止 winxime-server");
            process::exit(1);
        }

        let arg1 = arg_at(1, "缺少参数");

        match arg1.as_str() {
            "-register" => {
                let dll_path = arg_at(2, "缺少 DLL 路径参数");
                let icon_path = arg_at(3, "缺少 Icon 路径参数");

                println!("Step 1: Registering DLL to HKLM...");
                println!("  DLL path: {}", dll_path);
                match register_dll(&dll_path) {
                    Ok(_) => println!("  DLL registered"),
                    Err(e) => {
                        println!("  DLL registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                println!("Step 2: Registering TSF Profile...");
                println!("  Icon path: {}", icon_path);
                match register(icon_path) {
                    Ok(_) => println!("  Profile registered"),
                    Err(e) => {
                        println!("  Profile registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                println!("Step 3: Enabling input method...");
                enable();
                println!("  Enabled");

                println!("HKLM registration complete!");
            }
            "-copy-to-system" => {
                let src_dll = arg_at(2, "缺少源 DLL 路径参数");

                println!("Copying DLL to System32...");
                println!("  Source: {}", src_dll);
                let src_path = std::path::Path::new(&src_dll);
                match copy_dll_to_system(src_path) {
                    Ok(dest) => println!("  Copied to: {}", dest.display()),
                    Err(e) => {
                        println!("  Copy FAILED: {:?}", e);
                        println!("  Note: This requires administrator privileges");
                        process::exit(1);
                    }
                }
                println!("Copy complete!");
            }
            "-copy-and-register" => {
                let src_dll = arg_at(2, "缺少源 DLL 路径参数");
                let icon_path = arg_at(3, "缺少 Icon 路径参数");

                println!("Step 1: Copying DLL to System32...");
                println!("  Source: {}", src_dll);
                let src_path = std::path::Path::new(&src_dll);
                let system_dll = match copy_dll_to_system(src_path) {
                    Ok(path) => {
                        println!("  Copied to: {}", path.display());
                        path
                    }
                    Err(e) => {
                        println!("  Copy FAILED: {:?}", e);
                        println!("  Note: This requires administrator privileges");
                        process::exit(1);
                    }
                };

                println!("Step 2: Registering DLL...");
                match register_dll(&system_dll.to_string_lossy()) {
                    Ok(_) => println!("  DLL registered"),
                    Err(e) => {
                        println!("  DLL registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                println!("Step 3: Registering TSF Profile...");
                println!("  Icon path: {}", icon_path);
                match register(icon_path) {
                    Ok(_) => println!("  Profile registered"),
                    Err(e) => {
                        println!("  Profile registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                println!("Step 4: Enabling input method...");
                enable();
                println!("  Enabled");

                println!("Copy-and-register complete!");
            }
            "-unregister" => {
                let dll_path = arg_at(2, "缺少 DLL 路径参数");

                println!("Step 1: Stopping server...");
                stop();

                println!("Step 2: Unregistering TSF Profile...");
                match unregister() {
                    Ok(_) => println!("  Profile unregistered"),
                    Err(e) => println!("  Profile unregister failed: {:?}", e),
                }

                println!("Step 3: Unregistering DLL...");
                println!("  DLL path: {}", dll_path);
                match unregister_dll(&dll_path) {
                    Ok(_) => println!("  DLL unregistered"),
                    Err(e) => println!("  DLL unregister failed: {:?}", e),
                }

                println!("HKLM unregistration complete!");
            }
            "-remove-from-system" => {
                println!("Removing DLL from System32...");
                match remove_dll_from_system() {
                    Ok(_) => println!("  DLL removed"),
                    Err(e) => {
                        println!("  Remove failed: {:?}", e);
                        process::exit(1);
                    }
                }
                println!("Remove complete!");
            }
            "-unregister-and-remove" => {
                println!("Step 1: Stopping server...");
                stop();

                println!("Step 2: Unregistering TSF Profile...");
                match unregister() {
                    Ok(_) => println!("  Profile unregistered"),
                    Err(e) => println!("  Profile unregister failed: {:?}", e),
                }

                let system_dir = get_system_dir();
                let system_dll = system_dir.join("winxime_tsf.dll");
                println!("Step 3: Unregistering DLL from System32...");
                match unregister_dll(&system_dll.to_string_lossy()) {
                    Ok(_) => println!("  DLL unregistered"),
                    Err(e) => println!("  DLL unregister failed: {:?}", e),
                }

                println!("Step 4: Removing DLL from System32...");
                match remove_dll_from_system() {
                    Ok(_) => println!("  DLL removed"),
                    Err(e) => println!("  Remove failed: {:?}", e),
                }

                println!("Step 5: Removing registry keys...");
                match remove_registry_keys() {
                    Ok(_) => println!("  Registry keys removed"),
                    Err(e) => println!("  Registry cleanup failed: {:?}", e),
                }

                println!("MSI uninstall complete (user data preserved)!");
            }
            "-full-uninstall" => {
                println!("=== Full Uninstall ===");

                println!("Step 1: Stopping server...");
                stop();
                std::thread::sleep(std::time::Duration::from_secs(2));

                println!("Step 2: Unregistering TSF Profile...");
                match unregister() {
                    Ok(_) => println!("  Profile unregistered"),
                    Err(e) => println!("  Profile unregister failed: {:?}", e),
                }

                let system_dir = get_system_dir();
                let system_dll = system_dir.join("winxime_tsf.dll");
                println!("Step 3: Unregistering DLL from System32...");
                match unregister_dll(&system_dll.to_string_lossy()) {
                    Ok(_) => println!("  DLL unregistered"),
                    Err(e) => println!("  DLL unregister failed: {:?}", e),
                }

                println!("Step 4: Removing DLL from System32...");
                match remove_dll_from_system() {
                    Ok(_) => println!("  DLL removed"),
                    Err(e) => println!("  Remove failed: {:?}", e),
                }

                println!("Step 5: Removing registry keys...");
                match remove_registry_keys() {
                    Ok(_) => println!("  Registry keys removed"),
                    Err(e) => println!("  Registry cleanup failed: {:?}", e),
                }

                println!("Step 6: Removing user data directory...");
                let user_data_dir = get_user_data_dir();
                println!("  User data dir: {}", user_data_dir.display());
                match remove_user_data_dir() {
                    Ok(_) => println!("  User data removed"),
                    Err(e) => println!("  User data removal failed: {:?}", e),
                }

                println!("Step 7: Removing install directory...");
                let install_dir = get_install_dir();
                println!("  Install dir: {}", install_dir.display());
                match remove_install_dir() {
                    Ok(_) => println!("  Install dir removed"),
                    Err(e) => println!("  Install dir removal failed: {:?}", e),
                }

                println!("Step 8: Verifying uninstall...");
                let mut errors = Vec::new();

                if system_dll.exists() {
                    errors.push("System32 DLL still exists");
                }
                if user_data_dir.exists() {
                    errors.push("User data directory still exists");
                }
                if install_dir.exists() && install_dir.to_string_lossy().contains("Program Files") {
                    errors.push("Install directory still exists");
                }

                let hklm = HKEY_LOCAL_MACHINE;
                let clsid_key = w!("CLSID\\{5C1E4D8A-F3B2-4A7E-9CD1-2A3B4C5D6E7F}");
                let mut result_key: HKEY = HKEY::default();
                if RegOpenKeyW(hklm, clsid_key, &mut result_key).is_ok() {
                    errors.push("CLSID registry key still exists");
                }

                let tip_key =
                    w!("SOFTWARE\\Microsoft\\CTF\\TIP\\{5C1E4D8A-F3B2-4A7E-9CD1-2A3B4C5D6E7F}");
                let mut result_key: HKEY = HKEY::default();
                if RegOpenKeyW(hklm, tip_key, &mut result_key).is_ok() {
                    errors.push("TIP registry key still exists");
                }

                if errors.is_empty() {
                    println!("=== Uninstall Complete ===");
                    println!("All components removed successfully.");
                } else {
                    println!("=== WARNING: Uninstall incomplete! ===");
                    println!("Remaining items:");
                    for err in &errors {
                        println!("  - {}", err);
                    }
                    println!("Please restart Windows to complete cleanup.");
                    println!("After restart, run this command again to verify.");
                }
            }
            "-register-user" => {
                let dll_path = arg_at(2, "缺少 DLL 路径参数");
                let icon_path = arg_at(3, "缺少 Icon 路径参数");

                println!("Step 1: Registering DLL from user directory...");
                println!("  DLL path: {}", dll_path);
                match register_dll(&dll_path) {
                    Ok(_) => println!("  DLL registered"),
                    Err(e) => {
                        println!("  DLL registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                println!("Step 2: Registering TSF Profile...");
                println!("  Icon path: {}", icon_path);
                match register(icon_path) {
                    Ok(_) => println!("  Profile registered"),
                    Err(e) => {
                        println!("  Profile registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                println!("Step 3: Enabling input method...");
                enable();
                println!("  Enabled");

                println!("User-level registration complete!");
            }
            "-unregister-user" => {
                let dll_path = arg_at(2, "缺少 DLL 路径参数");

                println!("Step 1: Stopping server...");
                stop();

                println!("Step 2: Unregistering TSF Profile...");
                match unregister() {
                    Ok(_) => println!("  Profile unregistered"),
                    Err(e) => println!("  Profile unregister failed: {:?}", e),
                }

                println!("Step 3: Unregistering DLL...");
                println!("  DLL path: {}", dll_path);
                match unregister_dll(&dll_path) {
                    Ok(_) => println!("  DLL unregistered"),
                    Err(e) => println!("  DLL unregister failed: {:?}", e),
                }

                println!("User-level unregistration complete!");
            }
            "-install" => {
                println!("Step 1: Copying DLL to system directory...");
                let source_dll = exe_dir.join("winxime_tsf.dll");
                let system_dll = match copy_dll_to_system(&source_dll) {
                    Ok(path) => path,
                    Err(e) => {
                        println!("  Copy FAILED: {:?}", e);
                        println!("  Note: This requires administrator privileges");
                        process::exit(1);
                    }
                };

                println!("Step 2: Registering DLL...");
                match register_dll(&system_dll.to_string_lossy()) {
                    Ok(_) => println!("  DLL registered"),
                    Err(e) => {
                        println!("  DLL registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                let icon_path = exe_dir.join("icon.ico").to_string_lossy().to_string();
                println!("Step 3: Registering TSF Profile...");
                match register(icon_path) {
                    Ok(_) => println!("  Profile registered"),
                    Err(e) => {
                        println!("  Profile registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                println!("Step 4: Enabling input method...");
                enable();
                println!("  Enabled");

                println!("Installation complete!");
            }
            "-uninstall" => {
                println!("Step 1: Stopping server...");
                stop();

                println!("Step 2: Unregistering TSF Profile...");
                match unregister() {
                    Ok(_) => println!("  Profile unregistered"),
                    Err(e) => println!("  Profile unregister failed: {:?}", e),
                }

                let system_dir = get_system_dir();
                let system_dll = system_dir.join("winxime_tsf.dll");
                println!("Step 3: Unregistering DLL...");
                match unregister_dll(&system_dll.to_string_lossy()) {
                    Ok(_) => println!("  DLL unregistered"),
                    Err(e) => println!("  DLL unregister failed: {:?}", e),
                }

                println!("Step 4: Removing DLL from system directory...");
                match remove_dll_from_system() {
                    Ok(_) => println!("  DLL removed"),
                    Err(e) => println!("  Remove failed: {:?}", e),
                }

                println!("Uninstallation complete!");
            }
            "-register-only" => {
                // For WiX: DLL is already copied to System32 by MSI
                // Icon path is passed as argument, or use exe directory
                let system_dir = get_system_dir();
                let system_dll = system_dir.join("winxime_tsf.dll");

                println!("Step 1: Registering DLL...");
                match register_dll(&system_dll.to_string_lossy()) {
                    Ok(_) => println!("  DLL registered"),
                    Err(e) => {
                        println!("  DLL registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                // Use passed icon path, or fallback to exe directory
                let icon_path = env::args()
                    .nth(2)
                    .map(|p| p.trim_matches('"').to_string())
                    .unwrap_or_else(|| exe_dir.join("icon.ico").to_string_lossy().to_string());
                println!("Step 2: Registering TSF Profile with icon: {}", icon_path);
                match register(icon_path) {
                    Ok(_) => println!("  Profile registered"),
                    Err(e) => {
                        println!("  Profile registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }

                println!("Step 3: Enabling input method...");
                enable();
                println!("  Enabled");

                println!("Registration complete!");
            }
            "-unregister-only" => {
                // For WiX: only unregister, don't delete DLL (MSI handles that)
                println!("Step 1: Unregistering TSF Profile...");
                match unregister() {
                    Ok(_) => println!("  Profile unregistered"),
                    Err(e) => println!("  Profile unregister failed: {:?}", e),
                }

                let system_dir = get_system_dir();
                let system_dll = system_dir.join("winxime_tsf.dll");
                println!("Step 2: Unregistering DLL...");
                match unregister_dll(&system_dll.to_string_lossy()) {
                    Ok(_) => println!("  DLL unregistered"),
                    Err(e) => println!("  DLL unregister failed: {:?}", e),
                }

                println!("Unregistration complete!");
            }
            "-r" => {
                let icon_path = arg_at(2, "缺少 IconPath");
                println!("Registering profile with icon: {}", icon_path);
                match register(icon_path) {
                    Ok(_) => println!("Registration successful"),
                    Err(e) => {
                        println!("Registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }
            }
            "-dll" => {
                let dll_path = exe_dir.join("winxime_tsf.dll");
                let dll_path_str = dll_path.to_string_lossy().to_string();
                println!("Registering DLL: {}", dll_path_str);
                match register_dll(&dll_path_str) {
                    Ok(_) => println!("DLL registered successfully"),
                    Err(e) => {
                        println!("DLL registration FAILED: {:?}", e);
                        process::exit(1);
                    }
                }
            }
            "-i" => {
                println!("Enabling input method...");
                enable();
                println!("Enable command sent");
            }
            "-d" => {
                println!("Disabling input method...");
                disable();
                println!("Disable command sent");
            }
            "-s" | "-stop-server" => {
                println!("Stopping server...");
                stop();
            }
            "-u" => {
                println!("Unregistering profile...");
                match unregister() {
                    Ok(_) => println!("Unregistration successful"),
                    Err(err) => {
                        println!("Unregister FAILED: {:?}", err);
                        process::exit(1);
                    }
                }
            }
            _ => {
                println!("Unknown parameter: {}", arg1);
                process::exit(1);
            }
        }
    }

    Ok(())
}
