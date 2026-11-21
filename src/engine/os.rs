//! `@std/os` 的引擎绑定层。
//!
//! 所有读取都是同步且无副作用的，因此直接用「方法表 → 合成模块」的通用装配，
//! 算法本身在 `crate::stdlib::os`（纯 Rust，可独立单测）。

use boa_engine::{Context, JsNativeError, JsResult, JsValue, js_string};

use super::support::{NativeMethod, synthetic_module};
use crate::stdlib::os as os_core;

/// 导出的方法表（`pid` 返回数值，其余返回字符串）。
const METHODS: &[(&str, NativeMethod)] = &[
    ("platform", platform),
    ("arch", arch),
    ("hostname", hostname),
    ("homedir", homedir),
    ("tmpdir", tmpdir),
    ("cwd", cwd),
    ("pid", pid),
];

/// 导出的字符串常量：平台换行符。
const CONSTANTS: &[(&str, &str)] = &[("eol", os_core::EOL)];

/// 装配 `@std/os` 合成模块。
pub(super) fn build_os_module(context: &mut Context) -> JsResult<boa_engine::Module> {
    synthetic_module("os", METHODS, CONSTANTS, context)
}

/// `platform()`：`darwin` / `linux` / `windows` 等。
fn platform(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(os_core::platform())))
}

/// `arch()`：`x86_64` / `aarch64` 等。
fn arch(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(os_core::arch())))
}

/// `hostname()`：主机名。
fn hostname(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let name = os_core::hostname();
    Ok(JsValue::from(js_string!(name.as_str())))
}

/// `homedir()`：主目录，拿不到时返回 `undefined`。
fn homedir(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(match os_core::homedir() {
        Some(home) => JsValue::from(js_string!(home.as_str())),
        None => JsValue::undefined(),
    })
}

/// `tmpdir()`：临时目录。
fn tmpdir(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let dir = os_core::tmpdir();
    Ok(JsValue::from(js_string!(dir.as_str())))
}

/// `cwd()`：当前工作目录，取不到时抛 `TypeError`。
fn cwd(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    match os_core::cwd() {
        Ok(dir) => Ok(JsValue::from(js_string!(dir.as_str()))),
        Err(message) => Err(JsNativeError::typ().with_message(message).into()),
    }
}

/// `pid()`：当前进程号（约定 2 之外的特例，整数语义明确，故用数值）。
fn pid(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(f64::from(os_core::pid())))
}
