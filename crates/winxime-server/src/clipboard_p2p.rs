//! 局域网直连（xime-p2p）：把剪贴板线程的本地变更交给 `Node`，并轮询远端条目。
//!
//! 通道选型在 `clipboard_sync.toml` 的 `channels` 里（`"p2p"` 或插件 id，
//! 与设置程序、Android daemon 共用同一份契约）；本模块只负责节点生命周期和
//! 「文本进 / 事件出」的搬运，去重与写回系统剪贴板仍由剪贴板线程统一处理。

use std::path::{Path, PathBuf};

use tracing::info;
use xime_p2p::{load_or_create_identity, Event, Node, NodeConfig, Profile, TCP_PORT};

/// 输入法侧的 P2P 节点。
///
/// 只做三件事：启动、投递本地文本、取远端事件。跨设备身份与已配对表都在
/// [`p2p_dir_for`] 指向的目录里，与设置页「设备关联」是同一份。
pub struct P2pSync {
    node: Node,
}

impl P2pSync {
    /// 启动节点：监听 `TCP_PORT`、参与局域网公告。
    ///
    /// 身份文件缺失时会新建（与设置页配对时用的是同一份），因此没配过对的
    /// 机器也能正常启动，只是没有对端可连。
    pub fn start(p2p_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(p2p_dir)
            .map_err(|e| format!("P2P 目录创建失败（{}）：{e}", p2p_dir.display()))?;
        let identity = load_or_create_identity(p2p_dir)
            .ok_or("P2P 身份不可用（系统随机源异常，无法生成设备身份）")?;
        let mut config = NodeConfig::new(identity, p2p_dir.join("peers.json"));
        config.listen = true;
        config.discover = true;
        config.port = TCP_PORT;
        let node =
            Node::start(config).map_err(|e| format!("局域网同步节点启动失败：{e}"))?;
        info!(
            "局域网同步节点已启动：did={} 监听端口={}",
            node.did(),
            node.local_port()
        );
        Ok(Self { node })
    }

    /// 投递本地剪贴板文本；节点未运行或没有已配对设备时返回 `false`。
    pub fn push_text(&self, text: &str) -> bool {
        self.node
            .set_clipboard(Profile::from_text(text, Some("xime-windows".to_string())))
    }

    /// 非阻塞取一条事件；当前没有事件时返回 `None`。
    pub fn poll(&self) -> Option<Event> {
        self.node.poll_event()
    }
}

/// 输入法侧的 P2P 目录：与设置页「设备关联」共用 `%APPDATA%\Xime\p2p`。
///
/// 取 rime 用户目录的父级。两端各写各的，就会出现「设置里配对成功、输入法
/// 仍认为没配对」，而界面上看不出异常。
pub fn p2p_dir_for(rime_dir: &Path) -> PathBuf {
    match rime_dir.parent() {
        Some(root) if !root.as_os_str().is_empty() => root.join("p2p"),
        _ => rime_dir.join("p2p"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p2p_dir_sits_next_to_rime_dir() {
        let rime_dir = Path::new("C:\\Users\\me\\AppData\\Roaming\\Xime\\rime");
        let dir = p2p_dir_for(rime_dir);
        assert_eq!(
            dir,
            Path::new("C:\\Users\\me\\AppData\\Roaming\\Xime\\p2p")
        );
    }

    #[test]
    fn rooted_rime_dir_keeps_p2p_inside() {
        assert_eq!(p2p_dir_for(Path::new("rime")), Path::new("rime").join("p2p"));
    }
}
