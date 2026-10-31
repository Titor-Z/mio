//! `TextEncoder` / `TextDecoder` 全局的引擎绑定（WHATWG Encoding，仅 utf-8）。
//!
//! v1 边界：`TextDecoder` 忽略编码标签（一律 utf-8、非 fatal、lossy 替换非法序列）；
//! `TextEncoder` 无状态。utf-8 转换本身是标准库能力，无独立纯数据层。

use boa_engine::object::builtins::{JsArrayBuffer, JsUint8Array};
use boa_engine::{Context, JsArgs, JsNativeError, JsObject, JsResult, JsValue, js_string};

use super::support::{attach_method, register_constructor, set_property};

/// 安装 `TextEncoder` 与 `TextDecoder` 全局。
pub(super) fn install(context: &mut Context) {
    register_constructor(context, "TextEncoder", build_encoder);
    register_constructor(context, "TextDecoder", build_decoder);
}

fn build_encoder(context: &mut Context, _args: &[JsValue]) -> JsResult<JsObject> {
    let object = JsObject::default(context.intrinsics());
    set_property(
        &object,
        js_string!("encoding").into(),
        JsValue::from(js_string!("utf-8")),
        context,
    );
    attach_method(&object, "encode", encode_method, context);
    Ok(object)
}

fn build_decoder(context: &mut Context, args: &[JsValue]) -> JsResult<JsObject> {
    let object = JsObject::default(context.intrinsics());
    // v1 一律 utf-8：标签只如实报告，不影响解码行为。
    let label = args
        .get_or_undefined(0)
        .as_string()
        .map(|string| string.to_std_string_escaped().to_ascii_lowercase())
        .unwrap_or_else(|| "utf-8".to_string());
    set_property(
        &object,
        js_string!("encoding").into(),
        JsValue::from(js_string!(label.as_str())),
        context,
    );
    set_property(
        &object,
        js_string!("fatal").into(),
        JsValue::from(false),
        context,
    );
    set_property(
        &object,
        js_string!("ignoreBOM").into(),
        JsValue::from(false),
        context,
    );
    attach_method(&object, "decode", decode_method, context);
    Ok(object)
}

/// `TextEncoder.prototype.encode(string)`：无参数按空串。
fn encode_method(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = match args.get_or_undefined(0).as_string() {
        Some(string) => string.to_std_string_escaped(),
        None => String::new(),
    };
    let bytes = JsUint8Array::from_iter(text.into_bytes(), context)?;
    Ok(JsValue::from(bytes))
}

/// `TextDecoder.prototype.decode(input)`：非 fatal 语义，非法序列替换 U+FFFD。
fn decode_method(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = decode_input_bytes(args.get_or_undefined(0), context)?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    Ok(JsValue::from(js_string!(text.as_str())))
}

/// `decode` 入参：Uint8Array / ArrayBuffer / null 族，取原始字节。
fn decode_input_bytes(value: &JsValue, context: &mut Context) -> JsResult<Vec<u8>> {
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }
    let Some(object) = value.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("decode expects a typed array or ArrayBuffer")
            .into());
    };
    if let Ok(array) = JsUint8Array::from_object(object.clone()) {
        return array.to_vec(context);
    }
    if let Ok(buffer) = JsArrayBuffer::from_object(object) {
        return Ok(buffer.to_vec().unwrap_or_default());
    }
    Err(JsNativeError::typ()
        .with_message("decode expects a typed array or ArrayBuffer")
        .into())
}
