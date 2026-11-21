//! `@std/env` 的引擎绑定层。
//!
//! 与 `@std/os` 的差别：`get` 要能返回 `undefined`、`toObject` 要返回普通对象，
//! 「字符串进、字符串出」的通用装配表达不了，故手写构建器。

use boa_engine::{Context, JsNativeError, JsResult, JsValue, js_string};

use super::support::{NativeMethod, synthetic_module, to_text};
use crate::stdlib::env as env_core;

/// 导出的方法表。
const METHODS: &[(&str, NativeMethod)] = &[
    ("get", get),
    ("set", set),
    ("remove", remove),
    ("has", has),
    ("toObject", to_object),
];

/// 装配 `@std/env` 合成模块。
pub(super) fn build_env_module(context: &mut Context) -> JsResult<boa_engine::Module> {
    synthetic_module("env", METHODS, &[], context)
}

/// `get(name)`：存在返回值，不存在返回 `undefined`。
fn get(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = required_name(args, context)?;
    Ok(match env_core::get(&name) {
        Some(value) => JsValue::from(js_string!(value.as_str())),
        None => JsValue::undefined(),
    })
}

/// `set(name, value)`：只改本进程内的 `@std/env` 视图，返回 `undefined`。
fn set(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = required_name(args, context)?;
    let value = to_text(args.get(1).unwrap_or(&JsValue::undefined()), context)?;
    env_core::set(&name, &value).map_err(|message| JsNativeError::typ().with_message(message))?;
    Ok(JsValue::undefined())
}

/// `remove(name)`：返回该变量此前是否存在。
fn remove(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = required_name(args, context)?;
    let existed =
        env_core::remove(&name).map_err(|message| JsNativeError::typ().with_message(message))?;
    Ok(JsValue::from(existed))
}

/// `has(name)`：是否存在。
fn has(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = required_name(args, context)?;
    Ok(JsValue::from(env_core::has(&name)))
}

/// `toObject()`：把当前视图导出为普通对象（键排序，便于比对）。
fn to_object(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let object = boa_engine::JsObject::default(context.intrinsics());
    for (key, value) in env_core::to_object() {
        object.create_data_property(
            boa_engine::property::PropertyKey::from(js_string!(key.as_str())),
            JsValue::from(js_string!(value.as_str())),
            context,
        )?;
    }
    Ok(object.into())
}

/// 取第一个实参作为环境变量名；非字符串或空串直接抛 `TypeError`。
fn required_name(args: &[JsValue], context: &mut Context) -> JsResult<String> {
    let name = match args.first() {
        Some(value) if !value.is_undefined() => to_text(value, context)?,
        _ => String::new(),
    };
    if name.is_empty() {
        return Err(JsNativeError::typ()
            .with_message("环境变量名必须是非空字符串")
            .into());
    }
    Ok(name)
}
