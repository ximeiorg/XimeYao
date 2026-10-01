//! 语音设置持久化（当前选中的本地模型）。
//!
//! 落点与其它跨进程配置一致：`<数据根>/speech.toml`（数据根 = rime 用户目录的
//! 上级 = `%APPDATA%\Xime`，与 `clipboard_sync.toml` 同级）。设置程序写、
//! server 读——**server 侧按「用到才读」**（进语音页查一次、每次开会话再读一次），
//! 所以用户在设置里换了模型，不必重启 server 就能生效。
//!
//! 文件缺失 / 解析失败 / 模型 id 未知都退回默认模型（[`AsrModelRegistry`]），
//! 不让一个坏配置把语音功能整个卡死。

use std::path::{Path, PathBuf};

use xime_speech::{AsrModelProfile, AsrModelRegistry};

/// 配置文件名（与 `clipboard_sync.toml` 同目录）。
pub const SETTINGS_FILE: &str = "speech.toml";

/// 语音设置（目前只有「选中模型」；后续字段一律 `#[serde(default)]` 以便向前兼容）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpeechSettings {
    /// 选中的模型 id；空串 = 用默认模型（[`AsrModelRegistry::default_profile`]）。
    #[serde(default)]
    pub model_id: String,
}

/// 设置文件路径：`<数据根>/speech.toml`。
pub fn settings_path(data_root: &Path) -> PathBuf {
    data_root.join(SETTINGS_FILE)
}

/// 读设置；文件缺失或解析失败一律当默认值（空 model_id）。
pub fn load(data_root: &Path) -> SpeechSettings {
    std::fs::read_to_string(settings_path(data_root))
        .ok()
        .and_then(|text| toml::from_str::<SpeechSettings>(&text).ok())
        .unwrap_or_default()
}

/// 写设置（原子性够用：先写临时文件再改名，避免设置程序读到半截文件）。
pub fn save(data_root: &Path, settings: &SpeechSettings) -> Result<(), String> {
    let path = settings_path(data_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let text = toml::to_string(settings).map_err(|e| format!("序列化语音设置失败: {e}"))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写入语音设置失败: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换语音设置失败: {e}"))
}

/// 当前选中的模型：设置里的 id 查注册表；空 / 未知 id 退回默认模型
/// （未知 id 用 [`AsrModelRegistry::profile_or_default`] 保留原目录名，
/// 这样按通用命名发布的新模型仍能装载）。
pub fn selected_profile(data_root: &Path) -> AsrModelProfile {
    let settings = load(data_root);
    let id = settings.model_id.trim();
    if id.is_empty() {
        AsrModelRegistry::default_profile()
    } else {
        AsrModelRegistry::profile_or_default(id)
    }
}

/// 选中模型的文件是否齐（判定与 [`crate::models::is_model_downloaded`] 同一口径：
/// 四个角色文件都存在且非空）。
pub fn selected_model_ready(data_root: &Path) -> bool {
    let profile = selected_profile(data_root);
    crate::models::is_model_downloaded(
        &data_root.join("rime"),
        &profile.id,
        &profile_files(&profile),
    )
}

/// 模型四个角色文件名（借用切片给 `is_model_downloaded` 用）。
pub fn profile_files(profile: &AsrModelProfile) -> Vec<&str> {
    vec![
        profile.encoder_file.as_str(),
        profile.decoder_file.as_str(),
        profile.joiner_file.as_str(),
        profile.tokens_file.as_str(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用数据根：仓库 `target/test-tmp/`（本环境 Rust 测试进程不能在
    /// `%TEMP%` 建目录，见 PROGRESS 环境坑 3），`rime/` 子目录模拟 rime 用户目录。
    fn temp_root(label: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("speech_settings_{label}"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::create_dir_all(root.join("rime"));
        root
    }

    #[test]
    fn missing_file_falls_back_to_default_model() {
        let root = temp_root("missing");
        assert_eq!(load(&root), SpeechSettings::default());
        assert_eq!(
            selected_profile(&root).id,
            AsrModelRegistry::default_profile().id
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_then_load_roundtrips_and_selects() {
        let root = temp_root("roundtrip");
        let settings = SpeechSettings {
            model_id: "zipformer-zh-int8".to_string(),
        };
        save(&root, &settings).expect("保存语音设置");
        assert_eq!(load(&root), settings);
        assert_eq!(selected_profile(&root).id, "zipformer-zh-int8");
        // 临时文件不残留（rename 走的是 tmp → 正式名）。
        assert!(!settings_path(&root).with_extension("toml.tmp").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_id_keeps_its_directory_name() {
        let root = temp_root("unknown");
        save(
            &root,
            &SpeechSettings {
                model_id: "future-model".to_string(),
            },
        )
        .expect("保存语音设置");
        let profile = selected_profile(&root);
        // 未知 id：目录名保留，文件布局退回默认（与注册表口径一致）。
        assert_eq!(profile.id, "future-model");
        assert_eq!(
            profile.encoder_file,
            AsrModelRegistry::default_profile().encoder_file
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn blank_id_uses_default_and_readiness_tracks_files() {
        let root = temp_root("ready");
        // 空 id = 默认模型；文件没下载 → 未就绪。
        assert!(!selected_model_ready(&root));
        let profile = AsrModelRegistry::default_profile();
        let dir = crate::models::ensure_model_dir(&root.join("rime"), &profile.id).expect("建模型目录");
        for (index, name) in profile_files(&profile).iter().enumerate() {
            std::fs::write(dir.join(name), format!("bytes{index}")).expect("写模型文件");
        }
        assert!(selected_model_ready(&root));
        // 切到另一个没下载的模型 → 又未就绪（就绪判定跟着选中模型走）。
        save(
            &root,
            &SpeechSettings {
                model_id: "zipformer-zh-int8".to_string(),
            },
        )
        .expect("保存语音设置");
        assert!(!selected_model_ready(&root));
        let _ = std::fs::remove_dir_all(&root);
    }
}