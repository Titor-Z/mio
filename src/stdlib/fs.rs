//! `@std/fs` 纯 Rust 核心实现（与引擎隔离）。
//!
//! 设计原则：
//! 1. 只依赖 `std::fs`，不出现任何 `boa_*` 类型
//! 2. 全部返回 `Result<_, String>`，引擎侧负责 Promise / NativeObject 包装
//! 3. Phase 1 使用阻塞 IO（单次操作快）
//!
//! API 分层：
//! - Convenience Layer: `read_file`, `write_file`, `stat`（自动 manage FD）

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

// ============================================================================
// 类型定义
// ============================================================================

/// 文件打开选项（引擎层根据脚本参数构造）。
#[derive(Debug, Clone, Default)]
pub struct FsOpenOptions {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
    pub append: bool,
}

impl FsOpenOptions {
    /// 只读打开（默认）。
    pub fn read_only() -> Self {
        Self {
            read: true,
            ..Self::default()
        }
    }

    /// 写/创建/截断。
    pub fn write_truncate() -> Self {
        Self {
            write: true,
            create: true,
            truncate: true,
            ..Self::default()
        }
    }

    /// 追加写入。
    pub fn append() -> Self {
        Self {
            write: true,
            create: true,
            append: true,
            ..Self::default()
        }
    }

    /// 按脚本侧模式字符串构造：`r` / `w` / `a`，缺省为 `r`。
    pub fn from_mode(mode: &str) -> Self {
        match mode {
            "w" => Self::write_truncate(),
            "a" => Self::append(),
            _ => Self::read_only(),
        }
    }
}

// ============================================================================
// 文件句柄（FD）操作：在线程之间传递 `std::fs::File`
// ============================================================================

/// 按选项打开文件，返回可跨线程持有的 `File` 句柄。
pub fn open_file(path: &str, options: &FsOpenOptions) -> Result<fs::File, String> {
    let mut builder = fs::OpenOptions::new();
    builder
        .read(options.read)
        .write(options.write)
        .create(options.create)
        .truncate(options.truncate)
        .append(options.append);
    builder
        .open(path)
        .map_err(|e| format!("无法打开文件 '{path}': {e}"))
}

/// 从已打开文件读取至多 `len` 字节。
pub fn read_chunk(file: &mut fs::File, len: usize) -> Result<Vec<u8>, String> {
    let mut buffer = vec![0u8; len];
    let read = file
        .read(&mut buffer)
        .map_err(|e| format!("读取失败：{e}"))?;
    buffer.truncate(read);
    Ok(buffer)
}

/// 向已打开文件写入字节，返回写入字节数。
pub fn write_chunk(file: &mut fs::File, data: &[u8]) -> Result<usize, String> {
    file.write(data).map_err(|e| format!("写入失败：{e}"))
}

// ============================================================================
// 核心 API 实现（阻塞 IO）
// ============================================================================

/// 便捷读取整个文件
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let path = Path::new(path);
    fs::read(path).map_err(|e| format!("无法读取文件 '{}': {}", path.display(), e))
}

/// 便捷写入文件
pub fn write_file(path: &str, data: &[u8]) -> Result<(), String> {
    let path = Path::new(path);
    fs::write(path, data).map_err(|e| format!("写入失败 '{}': {}", path.display(), e))
}

/// 读取文本文件
pub fn read_text_file(path: &str) -> Result<String, String> {
    let bytes = read_file(path)?;
    String::from_utf8(bytes).map_err(|e| format!("UTF-8 解码失败：{}", e))
}

/// 写入文本文件
pub fn write_text_file(path: &str, text: &str) -> Result<(), String> {
    write_file(path, text.as_bytes())
}

/// 元数据信息（对齐 Deno 的 FileInfo）
#[derive(Debug)]
pub struct FileInfo {
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<f64>,
    pub accessed: Option<f64>,
    pub created: Option<f64>,
    #[cfg(unix)]
    pub permissions: u32,
}

impl Clone for FileInfo {
    fn clone(&self) -> Self {
        Self {
            is_file: self.is_file,
            is_directory: self.is_directory,
            is_symlink: self.is_symlink,
            size: self.size,
            modified: self.modified,
            accessed: self.accessed,
            created: self.created,
            #[cfg(unix)]
            permissions: self.permissions,
        }
    }
}

impl FileInfo {
    pub fn from_metadata(metadata: std::fs::Metadata, is_symlink: bool) -> Self {
        use std::time::UNIX_EPOCH;

        let mod_time = metadata.modified().ok().and_then(|t| {
            t.duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as f64)
        });

        let acc_time = metadata.accessed().ok().and_then(|t| {
            t.duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as f64)
        });

        let cre_time = metadata.created().ok().and_then(|t| {
            t.duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as f64)
        });

        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(unix)]
        let perms = metadata.mode() & 0o777;

        Self {
            is_file: metadata.is_file(),
            is_directory: metadata.is_dir(),
            is_symlink,
            size: metadata.len(),
            modified: mod_time,
            accessed: acc_time,
            created: cre_time,
            #[cfg(unix)]
            permissions: perms,
        }
    }
}

/// 查询文件信息
pub fn stat(path: &str) -> Result<FileInfo, String> {
    let path = Path::new(path);
    let is_symlink = path
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);

    let metadata =
        fs::metadata(path).map_err(|e| format!("stat 失败 '{}': {}", path.display(), e))?;

    Ok(FileInfo::from_metadata(metadata, is_symlink))
}

/// 检查文件是否存在
pub fn exists(path: &str) -> Result<bool, String> {
    let path = Path::new(path);
    Ok(fs::exists(path).unwrap_or(false))
}

/// 目录操作：创建目录
pub fn mkdir(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    fs::create_dir_all(path).map_err(|e| format!("创建目录失败 '{}': {}", path.display(), e))
}

/// 目录操作：列出目录内容
pub fn readdir(path: &str) -> Result<Vec<String>, String> {
    let path = Path::new(path);
    let entries =
        fs::read_dir(path).map_err(|e| format!("读取目录失败 '{}': {}", path.display(), e))?;

    Ok(entries
        .filter_map(|entry| entry.ok())
        .filter_map(|e| e.path().to_str().map(String::from))
        .collect())
}

/// 删除文件或空目录
pub fn remove(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    let is_dir = path.metadata().map(|m| m.is_dir()).unwrap_or(false);

    if is_dir {
        fs::remove_dir_all(path).map_err(|e| format!("删除目录失败 '{}': {}", path.display(), e))
    } else {
        fs::remove_file(path).map_err(|e| format!("删除文件失败 '{}': {}", path.display(), e))
    }
}

/// 复制文件
pub fn copy_file(src: &str, dest: &str) -> Result<(), String> {
    fs::copy(src, dest)
        .map(|_| ())
        .map_err(|e| format!("复制文件 '{}' → '{}' 失败：{}", src, dest, e))
}

/// 重命名文件/目录
pub fn rename(old: &str, new_: &str) -> Result<(), String> {
    fs::rename(old, new_).map_err(|e| format!("重命名失败 '{old}' → '{new_}': {}", e))
}

// ============================================================================
// 对外暴露的函数名（供文档与类型声明对齐；引擎装配见 engine/fs.rs 的 EXPORTS）
// ============================================================================

#[allow(dead_code)]
pub const FUNCTION_NAMES: &[&str] = &[
    "readFile",
    "readTextFile",
    "writeFile",
    "writeTextFile",
    "stat",
    "exists",
    "mkdir",
    "readdir",
    "remove",
    "copyFile",
    "rename",
];

// open/close/read/write (FD version) handled in engine/fs.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_write_basic() {
        let temp_dir = std::env::temp_dir().join("mio_fs_test_rw");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("创建临时目录失败");

        let test_file = temp_dir.join("test.txt");
        let test_content = "Hello, Mio!";

        write_file(test_file.to_str().unwrap(), test_content.as_bytes()).expect("write_file 失败");

        let content = read_file(test_file.to_str().unwrap()).expect("read_file 失败");

        assert_eq!(String::from_utf8_lossy(&content), test_content);
        remove(test_file.to_str().unwrap()).expect("remove 失败");
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_stat() {
        let temp_dir = std::env::temp_dir().join("mio_fs_test_stat");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("创建临时目录失败");

        let metadata = stat(temp_dir.to_str().unwrap()).expect("stat 失败");

        assert!(metadata.is_directory);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
