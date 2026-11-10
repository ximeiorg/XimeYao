//! 模型数据目录（对齐安卓 Xime `ModelStorage`：`files/models/{modelId}/`）。
//!
//! 目录约定（数据根 = `%APPDATA%\Xime`，与 rime/、plugins/、market/ 同级）：
//! - `<root>/models/<modelId>/`  模型文件目录，文件名如实命名（如 encoder.int8.onnx、
//!   tokens.txt），由模型清单（models/index.yaml）的 files[].name 决定
//! - 模型独立于插件管理（安卓侧注释明确二者分立），版本记录属后续模型中心功能点
//! - 目录「用到才建」（对齐安卓 ModelDownloader.downloadModel 内 mkdirs），不在启动时创建
//!
//! 模型下载本身属后续功能点（ASR/联想后端尚未接入 Windows 端），本模块先行固化目录约定。

use std::path::{Path, PathBuf};

/// 模型根目录：`<数据根>/models`（数据根 = rime 用户目录的上级）。
pub fn models_root(user_data_dir: &Path) -> PathBuf {
    user_data_dir
        .parent()
        .unwrap_or(user_data_dir)
        .join("models")
}

/// 指定模型的存储目录：`<数据根>/models/<modelId>/`。
pub fn model_dir(user_data_dir: &Path, model_id: &str) -> PathBuf {
    models_root(user_data_dir).join(model_id)
}

/// 创建（如不存在）并返回模型目录；对齐安卓「用到才建」。
///
/// 线上下载走「暂存目录 → 整体替换」，不再直接建目标目录；这里留给测试造
/// 已下载模型的现场。
#[cfg(test)]
pub fn ensure_model_dir(user_data_dir: &Path, model_id: &str) -> Result<PathBuf, String> {
    let dir = model_dir(user_data_dir, model_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建模型目录失败: {}", e))?;
    Ok(dir)
}

/// 校验模型是否已下载完成：清单声明的每个文件都存在且非空
/// （对齐安卓 `ModelManager.isModelDownloaded`）。清单为空视为未下载。
pub fn is_model_downloaded(user_data_dir: &Path, model_id: &str, expected_files: &[&str]) -> bool {
    if expected_files.is_empty() {
        return false;
    }
    let dir = model_dir(user_data_dir, model_id);
    expected_files.iter().all(|name| {
        std::fs::metadata(dir.join(name))
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
    })
}

/// 删除模型目录（对齐安卓 `ModelManager.deleteModel`，版本记录清理属模型中心功能点）。
pub fn delete_model(user_data_dir: &Path, model_id: &str) -> Result<(), String> {
    let dir = model_dir(user_data_dir, model_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("删除模型目录失败: {}", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        crate::test_support::test_root(&format!("models_{label}"))
    }

    #[test]
    fn model_dirs_are_siblings_of_rime() {
        let root = temp_root("layout");
        let rime = root.join("rime");
        // 与 rime/ 同级：models/<id>/
        assert_eq!(models_root(&rime), root.join("models"));
        assert_eq!(model_dir(&rime, "zipformer-zh-int8"), root.join("models").join("zipformer-zh-int8"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ensure_creates_and_download_check_verifies_files() {
        let root = temp_root("check");
        let rime = root.join("rime");
        assert!(!models_root(&rime).exists(), "启动时不应预创建（用到才建）");

        let dir = ensure_model_dir(&rime, "zipformer-zh-int8").unwrap();
        assert!(dir.exists());

        // 文件缺失 → 未下载
        assert!(!is_model_downloaded(&rime, "zipformer-zh-int8", &["encoder.int8.onnx", "tokens.txt"]));

        // 空文件 → 未下载（对齐安卓：文件存在且非空）
        std::fs::write(dir.join("encoder.int8.onnx"), b"").unwrap();
        std::fs::write(dir.join("tokens.txt"), b"abc").unwrap();
        assert!(!is_model_downloaded(&rime, "zipformer-zh-int8", &["encoder.int8.onnx", "tokens.txt"]));

        // 全部存在且非空 → 已下载
        std::fs::write(dir.join("encoder.int8.onnx"), b"weights").unwrap();
        assert!(is_model_downloaded(&rime, "zipformer-zh-int8", &["encoder.int8.onnx", "tokens.txt"]));

        // 清单为空 → 视为未下载
        assert!(!is_model_downloaded(&rime, "zipformer-zh-int8", &[]));

        delete_model(&rime, "zipformer-zh-int8").unwrap();
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
