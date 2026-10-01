#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use winxime_ipc::IpcClient;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use xime_setup_lib::{set_app_metadata, AppMetadata};

mod toast;

fn main() {
    let process_start = std::time::Instant::now();
    // 设置进程日志（%APPDATA%\xime\logs\setup.log；GUI 进程无控制台可看）。
    xime_config::init_logging_with_console("setup");
    // 冷启动计时锚点：与 server.log 的「启动设置程序」时间戳对齐，两者之差
    // 就是「进程创建 + DLL 装载 + 日志初始化」这段看不到的开销。
    tracing::info!(
        "setup 进程冷启动：日志就绪 +{}ms（对齐 server.log 的启动设置程序时间戳）",
        process_start.elapsed().as_millis()
    );

    let _ = set_app_metadata(AppMetadata {
        display_name: "曦码·曜",
        config_dir_name: "xime",
        config_file_base: "xime",
        distribution_name: "Xime Yao",
        distribution_code_name: "Xime Yao",
        app_name: "rime.xime.setup",
        version: env!("CARGO_PKG_VERSION"),
    });
    const MUTEX_NAME: &str = "XimeSetupSingleInstanceMutex";
    const WINDOW_CLASS: &str = "GPUI Window";
    const WINDOW_TITLE: &str = "曦码·曜 设置";

    let mutex_name_wide: Vec<u16> = MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let already_running = unsafe {
        let handle = CreateMutexW(None, false, PCWSTR(mutex_name_wide.as_ptr()));
        if handle.is_ok() {
            let last_error = GetLastError();
            if last_error == ERROR_ALREADY_EXISTS {
                let class_wide: Vec<u16> = WINDOW_CLASS
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let title_wide: Vec<u16> = WINDOW_TITLE
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let hwnd = FindWindowW(PCWSTR(class_wide.as_ptr()), PCWSTR(title_wide.as_ptr()));
                if hwnd.is_ok() {
                    let hwnd = hwnd.unwrap();
                    if !hwnd.0.is_null() {
                        if IsIconic(hwnd).as_bool() {
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                        }
                        let _ = SetForegroundWindow(hwnd);
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        }
    };

    if already_running {
        return;
    }

    xime_setup_lib::set_notify_select_schema(|schema_id| {
        IpcClient::select_schema(schema_id)
    });
    xime_setup_lib::set_notify_deploy(|| {
        let _ = IpcClient::reload_config();
    });
    // 剪贴板同步插件启停/选择后通知 server 重载插件运行时。
    xime_setup_lib::set_notify_reload_plugins(|| {
        let _ = IpcClient::reload_plugins();
    });
    // 用户资料同步：IPC SyncUserData（阻塞等待 rime 维护线程完成）。
    xime_setup_lib::set_notify_sync_user_data(IpcClient::sync_user_data);
    // 词典管理：rime 用户词典操作（经 IPC 在 server 进程执行）。
    xime_setup_lib::set_notify_dict_list(|| {
        IpcClient::list_user_dicts().map(|d| xime_setup_lib::state::DictListResult {
            dicts: d.dicts,
            sync_dir: d.sync_dir,
        })
    });
    xime_setup_lib::set_notify_dict_backup(IpcClient::backup_user_dict);
    xime_setup_lib::set_notify_dict_restore(IpcClient::restore_user_dict);
    xime_setup_lib::set_notify_dict_export(IpcClient::export_user_dict);
    xime_setup_lib::set_notify_dict_import(IpcClient::import_user_dict);
    // 词条浏览：IPC ListDictEntries（server 侧走 librime 导出通道再解析）。
    xime_setup_lib::set_notify_dict_entries(|dict, query| {
        IpcClient::list_dict_entries(dict, query).map(|d| {
            xime_setup_lib::state::DictEntriesResult {
                total: d.total,
                matched: d.count,
                entries: d
                    .entries
                    .into_iter()
                    .map(|e| xime_setup_lib::state::DictEntryRow {
                        word: e.word,
                        code: e.code,
                        commits: e.commits,
                    })
                    .collect(),
            }
        })
    });
    // 写词条（新增/删除标记）：IPC ImportDictEntry（server 销毁会话→导入→重建）。
    xime_setup_lib::set_notify_dict_entry_write(|dict, word, code, commits| {
        IpcClient::import_dict_entry(dict, word, code, commits)
    });
    // 方案词表只读浏览：IPC ListSchemaEntries（server 解析 .dict.yaml 码表）。
    xime_setup_lib::set_notify_schema_entries(|schema_id, query| {
        IpcClient::list_schema_entries(schema_id, query).map(|d| {
            xime_setup_lib::state::SchemaEntriesResult {
                dict_name: d.dict_name,
                tables: d.tables,
                missing: d.missing,
                total: d.total,
                matched: d.matched,
                entries: d
                    .entries
                    .into_iter()
                    .map(|e| xime_setup_lib::state::DictEntryRow {
                        word: e.word,
                        code: e.code,
                        commits: e.commits,
                    })
                    .collect(),
            }
        })
    });
    // 快捷短语：读表 IPC ListCustomPhrases。
    xime_setup_lib::set_notify_phrase_list(|schema_id| {
        IpcClient::list_custom_phrases(schema_id).map(|d| {
            xime_setup_lib::state::PhraseListResult {
                dict_name: d.dict_name,
                file_name: d.file_name,
                file_exists: d.file_exists,
                patch_applied: d.patch_applied,
                entries: d
                    .entries
                    .into_iter()
                    .map(|e| xime_setup_lib::state::CustomPhraseRow {
                        word: e.word,
                        code: e.code,
                        weight: e.weight,
                    })
                    .collect(),
            }
        })
    });
    // 快捷短语：整表覆盖保存 IPC SaveCustomPhrases（server 视需要注入方案 patch）。
    xime_setup_lib::set_notify_phrase_save(|schema_id, entries| {
        let ipc_entries: Vec<winxime_ipc::CustomPhraseEntry> = entries
            .iter()
            .map(|e| winxime_ipc::CustomPhraseEntry {
                word: e.word.clone(),
                code: e.code.clone(),
                weight: e.weight,
            })
            .collect();
        IpcClient::save_custom_phrases(schema_id, &ipc_entries).map(|d| {
            xime_setup_lib::state::PhraseSaveResult {
                dict_name: d.dict_name,
                file_name: d.file_name,
                file_exists: d.file_exists,
                patch_applied: d.patch_applied,
                patch_added: d.patch_added,
                entries: d
                    .entries
                    .into_iter()
                    .map(|e| xime_setup_lib::state::CustomPhraseRow {
                        word: e.word,
                        code: e.code,
                        weight: e.weight,
                    })
                    .collect(),
            }
        })
    });
    // 部署结果系统通知（WinRT toast；非打包环境静默跳过）。
    xime_setup_lib::set_notify_deploy_toast(toast::show_toast);

    tracing::info!(
        "回调注册完成 +{}ms，进入 iced",
        process_start.elapsed().as_millis()
    );
    let _ = xime_setup_lib::run();
}
