//! `@std/log` 的引擎绑定层。
//!
//! 级别是**运行时可变的**：初值来自 `mio.json` 的 `log.level`（或 `MIO_LOG_LEVEL`），
//! 脚本可用 `setLevel()` 当场改。状态放线程局部：同一引擎（含 REPL）内立即生效，
//! 同进程多次运行互不影响；输出统一写 stderr，避免污染脚本的 stdout。
//!
//! 纯 Rust 部分（级别比较、行渲染）在 `crate::stdlib::log`。

use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};

use boa_engine::{Context, JsNativeError, JsResult, JsValue, js_string};

use super::support::{NativeMethod, synthetic_module, to_text};
use crate::stdlib::log as log_core;

thread_local! {
    /// 当前级别阈值。
    static LEVEL: RefCell<log_core::Level> = const { RefCell::new(log_core::Level::Info) };
    /// 是否输出 JSON 行（对齐 `mio.json` 的 `log.json`）。
    static JSON: RefCell<bool> = const { RefCell::new(false) };
}

/// 导出的方法表。
const METHODS: &[(&str, NativeMethod)] = &[
    ("debug", debug),
    ("info", info),
    ("warn", warn),
    ("error", error),
    ("setLevel", set_level),
    ("getLevel", get_level),
];

/// 由命令行在装配引擎时调用，把配置里的日志选项落到本线程状态。
/// 级别文本非法时按 `info` 兜底（配置层已对非法值给过告警）。
pub(crate) fn configure(level: &str, json: bool) {
    let parsed = log_core::Level::parse(level).unwrap_or(log_core::Level::Info);
    LEVEL.with(|cell| *cell.borrow_mut() = parsed);
    JSON.with(|cell| *cell.borrow_mut() = json);
}

/// 装配 `@std/log` 合成模块。
pub(super) fn build_log_module(context: &mut Context) -> JsResult<boa_engine::Module> {
    synthetic_module("log", METHODS, &[], context)
}

fn debug(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    emit(log_core::Level::Debug, args, context)
}

fn info(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    emit(log_core::Level::Info, args, context)
}

fn warn(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    emit(log_core::Level::Warn, args, context)
}

fn error(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    emit(log_core::Level::Error, args, context)
}

/// `setLevel(level)`：改运行时阈值；非法取值抛 `TypeError`。
fn set_level(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let raw = to_text(args.first().unwrap_or(&JsValue::undefined()), context)?;
    let Some(level) = log_core::Level::parse(&raw) else {
        return Err(JsNativeError::typ()
            .with_message(format!(
                "未知日志级别 `{raw}`：可用 silent / error / warn / info / debug"
            ))
            .into());
    };
    LEVEL.with(|cell| *cell.borrow_mut() = level);
    Ok(JsValue::undefined())
}

/// `getLevel()`：当前阈值文本。
fn get_level(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let level = LEVEL.with(|cell| cell.borrow().as_str());
    Ok(JsValue::from(js_string!(level)))
}

/// 按阈值决定是否输出：实参按 `console.log` 的规则用空格连接。
fn emit(level: log_core::Level, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let threshold = LEVEL.with(|cell| *cell.borrow());
    if !threshold.allows(level) {
        return Ok(JsValue::undefined());
    }

    let mut parts: Vec<String> = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(to_text(arg, context)?);
    }
    let message = parts.join(" ");
    let json = JSON.with(|cell| *cell.borrow());
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);

    eprintln!("{}", log_core::render(level, &message, json, timestamp_ms));
    Ok(JsValue::undefined())
}
