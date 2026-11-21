//! `@std/env` 内建模块的纯 Rust 实现（引擎无关）。
//!
//! 口径对齐 Deno 的 `@std/env`：`get` / `set` / `remove` / `has` / `toObject`。
//! （删除操作叫 `remove` 而不是 `delete`：`delete` 是 TypeScript 保留字，声明文件里无法作函数名。）
//!
//! 为什么 `set` / `delete` 不直接调 `std::env::set_var`：Rust 2024 起这两个函数是 `unsafe`，
//! 因为多线程下改动进程环境属于数据竞争。mio 运行时本身有 fetch / fs 工作线程，
//! 因此改用「进程内覆盖表」：读时先查覆盖表，再退回真实环境；写只改覆盖表。
//! 效果是脚本看到的 `@std/env` 视图自洽且可变，且不会 UB。mio 不派生子进程，
//! 因此不存在「子进程看不到改动」的可见性问题。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 进程内环境覆盖表：`Some` 表示被设置为该值，`None` 表示被删除。
fn overlay() -> &'static Mutex<BTreeMap<String, Option<String>>> {
    static OVERLAY: OnceLock<Mutex<BTreeMap<String, Option<String>>>> = OnceLock::new();
    OVERLAY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 读取环境变量：覆盖表优先，其次真实环境。
pub fn get(name: &str) -> Option<String> {
    let overridden = overlay()
        .lock()
        .ok()
        .and_then(|table| table.get(name).cloned());
    match overridden {
        Some(Some(value)) => Some(value),
        Some(None) => None,
        None => std::env::var_os(name).and_then(|value| value.into_string().ok()),
    }
}

/// 判断环境变量是否存在（被删除的视为不存在）。
pub fn has(name: &str) -> bool {
    get(name).is_some()
}

/// 设置环境变量（只影响本进程内 `@std/env` 的视图）。
pub fn set(name: &str, value: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(String::from("环境变量名不能为空"));
    }
    let mut table = overlay()
        .lock()
        .map_err(|_| String::from("环境变量覆盖表已损坏"))?;
    table.insert(name.to_string(), Some(value.to_string()));
    Ok(())
}

/// 删除环境变量，返回它此前是否存在。
pub fn remove(name: &str) -> Result<bool, String> {
    let existed = has(name);
    let mut table = overlay()
        .lock()
        .map_err(|_| String::from("环境变量覆盖表已损坏"))?;
    table.insert(name.to_string(), None);
    Ok(existed)
}

/// 导出当前视图的全部环境变量，按键排序以保证输出稳定。
pub fn to_object() -> Vec<(String, String)> {
    let mut merged: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();

    if let Ok(table) = overlay().lock() {
        for (key, value) in table.iter() {
            match value {
                Some(value) => {
                    merged.insert(key.clone(), value.clone());
                }
                None => {
                    merged.remove(key);
                }
            }
        }
    }

    merged.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_has_delete_round_trip() {
        let name = "MIO_TEST_ENV_ROUND_TRIP";
        assert!(set(name, "one").is_ok());
        assert_eq!(get(name).as_deref(), Some("one"));
        assert!(has(name));

        assert_eq!(remove(name), Ok(true));
        assert_eq!(get(name), None);
        assert!(!has(name));
        // 已删除的再次删除，报告「原本不存在」。
        assert_eq!(remove(name), Ok(false));
    }

    #[test]
    fn empty_name_is_rejected() {
        assert!(set("", "value").is_err());
    }

    #[test]
    fn to_object_reflects_overlay() {
        let name = "MIO_TEST_ENV_VIEW";
        set(name, "visible").unwrap();
        let view = to_object();
        assert!(
            view.iter()
                .any(|(key, value)| key == name && value == "visible")
        );

        remove(name).unwrap();
        let view = to_object();
        assert!(!view.iter().any(|(key, _)| key == name));
    }

    #[test]
    fn reads_real_environment() {
        // PATH 由外部保证存在，用于验证「未覆盖时退回真实环境」。
        assert!(get("PATH").is_some());
    }
}
