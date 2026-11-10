//! 词典导入 / 恢复的输入路径边界（审计 F2）。
//!
//! 设置页把用户选的文件路径交给 IPC，服务端不能直接信任它——同机的任意进程
//! 都能发同样的消息，原样交给 librime 等于「任意文件读」。这里的规则是：
//! 只接受「用户数据目录」或「系统临时目录」之下的真实普通文件，其余一律拒绝
//! 并给出可读原因；设置页宿主会先把用户选的文件复制进临时目录再发请求。
//!
//! 导出方向反过来：路径由服务端在临时目录生成，随响应回传，客户端只读。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// 导出临时文件的序号（同一进程内并发调用也不撞名）。
static EXPORT_SEQ: AtomicU32 = AtomicU32::new(0);

/// 生成导出用的临时文件路径（服务端唯一的写入点）。
pub(crate) fn export_path(dict: &str) -> PathBuf {
    let label: String = dict
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    std::env::temp_dir().join(format!(
        "xime_dict_export_{label}_{}_{}.txt",
        std::process::id(),
        EXPORT_SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// 校验客户端送来的导入 / 恢复路径。
///
/// 通过条件（按顺序）：非空 → 能 canonicalize（存在且可访问）→ 是普通文件 →
/// 位于用户数据目录或系统临时目录之下。失败时返回给用户看的原因。
pub(crate) fn validate_input_path(raw: &str, user_data_dir: &Path) -> Result<PathBuf, String> {
    validate_against(&allowed_roots(user_data_dir), raw)
}

/// 白名单校验的实现：根目录由调用方给出，便于在测试里注入临时目录。
fn validate_against(roots: &[PathBuf], raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("没有选择文件".to_string());
    }
    // canonicalize 同时解决 `..`、符号链接与大小写，后续比较只看真实路径。
    let path = std::fs::canonicalize(raw).map_err(|e| format!("文件不存在或无权访问：{e}"))?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("读取文件信息失败：{e}"))?;
    if !meta.is_file() {
        return Err("选中的不是一个普通文件".to_string());
    }
    if roots.iter().any(|root| path.starts_with(root)) {
        return Ok(path);
    }
    Err(format!(
        "路径不在用户数据目录或临时目录内，已拒绝：{}",
        path.display()
    ))
}

/// 允许的根目录（都取 canonicalize 后的形式）。
fn allowed_roots(user_data_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::fs::canonicalize(user_data_dir) {
        roots.push(dir);
    }
    if let Ok(dir) = std::fs::canonicalize(std::env::temp_dir()) {
        roots.push(dir);
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试目录放 `target/tmp`（AGENTS §7），取 canonicalize 后的真实路径当根。
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("dict-paths-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建测试目录");
        std::fs::canonicalize(dir).expect("取测试目录真实路径")
    }

    #[test]
    fn accepts_file_inside_allowed_root() {
        let root = scratch("inside");
        let file = root.join("words.txt");
        std::fs::write(&file, b"word\tcode\t1\n").expect("写测试文件");
        let got = validate_against(std::slice::from_ref(&root), &file.to_string_lossy());
        assert!(got.is_ok(), "根目录内的文件应被接受：{got:?}");
        assert_eq!(got.ok(), Some(std::fs::canonicalize(&file).expect("真实路径")));
    }

    #[test]
    fn rejects_file_outside_allowed_roots() {
        let root = scratch("outside-root");
        let elsewhere = scratch("outside-file");
        let file = elsewhere.join("secret.txt");
        std::fs::write(&file, b"secret").expect("写测试文件");
        let err = validate_against(&[root], &file.to_string_lossy())
            .expect_err("目录外的文件必须被拒绝");
        assert!(err.contains("已拒绝"), "错误应说明被拒绝：{err}");
    }

    #[test]
    fn rejects_missing_file() {
        let root = scratch("missing");
        let missing = root.join("definitely-missing.txt");
        let err = validate_against(&[root], &missing.to_string_lossy())
            .expect_err("不存在的文件必须被拒绝");
        assert!(err.contains("不存在"), "错误应说明文件不存在：{err}");
    }

    #[test]
    fn rejects_directory_instead_of_file() {
        let root = scratch("dir");
        let dir = root.join("a-directory");
        std::fs::create_dir_all(&dir).expect("建子目录");
        let err = validate_against(&[root], &dir.to_string_lossy())
            .expect_err("目录必须被拒绝");
        assert!(err.contains("普通文件"), "错误应说明不是普通文件：{err}");
    }

    #[test]
    fn rejects_empty_path() {
        let err = validate_against(&[], "   ").expect_err("空路径必须被拒绝");
        assert!(err.contains("没有选择文件"), "错误应说明没选文件：{err}");
    }

    #[test]
    fn allowed_roots_cover_user_data_and_temp() {
        let user_data = scratch("roots");
        let roots = allowed_roots(&user_data);
        assert!(roots.contains(&user_data), "应包含用户数据目录");
        let temp = std::fs::canonicalize(std::env::temp_dir()).expect("临时目录真实路径");
        assert!(roots.contains(&temp), "应包含系统临时目录");
    }

    #[test]
    fn export_paths_are_unique_and_end_with_txt() {
        let first = export_path("luna_pinyin");
        let second = export_path("luna_pinyin");
        assert_ne!(first, second, "连续两次导出不能撞名");
        assert_eq!(first.extension().and_then(|e| e.to_str()), Some("txt"));
        assert!(first.starts_with(std::env::temp_dir()));
    }
}
