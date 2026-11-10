//! SHA-256 工具：插件包与语音模型包共用一份实现。
//!
//! 只负责「把字节变成小写十六进制摘要」。要跟哪个摘要比、不符之后删不删文件，
//! 由调用方决定——插件安装与模型下载的失败处理并不相同，混在一起会写出
//! 谁都不满意的半抽象。

use std::io::{self, Write};

use sha2::{Digest, Sha256};

/// 对一段内存字节算 SHA-256（小写十六进制）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// 边写边算的写入器：写给内层的每个字节同时喂给 hasher。
///
/// 模型包动辄上百 MB，先落盘再读回来算摘要要多一倍磁盘 I/O；下载时顺手算完，
/// [`Sha256Writer::finish`] 一次取出内层写入器与摘要。
pub struct Sha256Writer<W: Write> {
    inner: W,
    hasher: Option<Sha256>,
}

impl<W: Write> Sha256Writer<W> {
    /// 包住目标写入器。
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Some(Sha256::new()),
        }
    }

    /// 冲刷并取出内层写入器与摘要；重复调用返回错误（摘要已经交出去了）。
    pub fn finish(mut self) -> io::Result<(W, String)> {
        self.inner.flush()?;
        let hasher = self
            .hasher
            .take()
            .ok_or_else(|| io::Error::other("SHA-256 摘要已被取出"))?;
        Ok((self.inner, hex::encode(hasher.finalize())))
    }
}

impl<W: Write> Write for Sha256Writer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.hasher.as_mut() {
            Some(hasher) => {
                hasher.update(buf);
                self.inner.write(buf)
            }
            None => Err(io::Error::other("SHA-256 摘要已被取出")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空串的 SHA-256（`sha256sum /dev/null` 的公开值）。
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn empty_input_matches_known_digest() {
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
    }

    #[test]
    fn known_message_matches_known_digest() {
        // "abc" 的 SHA-256（NIST 测试向量）。
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn streaming_writer_matches_one_shot_digest() {
        let mut writer = Sha256Writer::new(Vec::new());
        writer.write_all(b"ab").expect("写入前半段");
        writer.write_all(b"c").expect("写入后半段");
        let (buffer, digest) = writer.finish().expect("取摘要");
        assert_eq!(buffer, b"abc");
        assert_eq!(digest, sha256_hex(b"abc"));
    }

    #[test]
    fn streaming_writer_passes_body_through() {
        let mut writer = Sha256Writer::new(Vec::new());
        writer.write_all(&[0u8; 1024]).expect("写入 1 KiB");
        let (buffer, _) = writer.finish().expect("取摘要");
        assert_eq!(buffer.len(), 1024);
    }
}
