//! `@std/os` 内建模块的纯 Rust 实现（引擎无关）。
//!
//! 口径对齐 Deno 的 `@std/os`：只提供「运行环境自身」的事实，不模仿 Node 的 `os` 模块
//! （没有 `cpus()` 等重量级信息），全部同步、无 Promise。
//! 目标平台为类 Unix 与 Windows，取值尽量走 `std::env::consts` 与环境变量，
//! 不引入 libc：拿不到时退回安全默认值（`hostname` → `"localhost"`）。

/// 操作系统名（Deno 口径的短名）：`darwin` / `linux` / `windows` / `freebsd` 等。
pub fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

/// CPU 架构名（Rust 口径：`x86_64` / `aarch64` / `x86` 等）。
pub fn arch() -> &'static str {
    std::env::consts::ARCH
}

/// 主机名：优先环境变量，其次 `/etc/hostname`，都拿不到时退回 `localhost`。
pub fn hostname() -> String {
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Some(value) = std::env::var_os(key).and_then(|value| value.into_string().ok())
            && !value.is_empty()
        {
            return value;
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| String::from("localhost"))
}

/// 当前用户主目录：`HOME`（类 Unix）/ `USERPROFILE`（Windows）。
pub fn homedir() -> Option<String> {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key).and_then(|value| value.into_string().ok())
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    None
}

/// 临时目录：`TMPDIR` / `TEMP` / `TMP`，都缺失时退回 `/tmp`。
pub fn tmpdir() -> String {
    for key in ["TMPDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key).and_then(|value| value.into_string().ok())
            && !value.is_empty()
        {
            return value;
        }
    }
    String::from("/tmp")
}

/// 当前工作目录；取不到时返回错误文本（通常是目录已被删除）。
pub fn cwd() -> Result<String, String> {
    std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| format!("无法获取当前工作目录：{error}"))
}

/// 当前进程号。
pub fn pid() -> u32 {
    std::process::id()
}

/// 平台换行符：Windows 为 `\r\n`，其余为 `\n`。
pub const EOL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_is_non_empty_and_not_macos() {
        assert!(!platform().is_empty());
        // Deno 口径：macOS 报 darwin，不报 macos。
        assert_ne!(platform(), "macos");
    }

    #[test]
    fn pid_is_current_process() {
        assert_eq!(pid(), std::process::id());
    }

    #[test]
    fn homedir_and_tmpdir_are_non_empty() {
        assert!(tmpdir().starts_with('/') || tmpdir().contains(':'));
        if let Some(home) = homedir() {
            assert!(!home.is_empty());
        }
    }

    #[test]
    fn cwd_matches_std() {
        let expected = std::env::current_dir().unwrap();
        assert_eq!(cwd().unwrap(), expected.to_string_lossy());
    }
}
