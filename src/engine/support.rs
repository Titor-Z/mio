//! Web 全局绑定层的共享工具：属性挂载、字符串强制转换、数组迭代器桥接。
//!
//! 这些是 Boa 宿主侧的胶水，无规范语义，供 `url` / `blob` / `fetch` 等绑定复用。

use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::JsFunction;
use boa_engine::property::{Attribute, PropertyDescriptor, PropertyKey};
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};

/// 挂载一个数据属性（可写、可枚举、可配置——duck-typed 全局对象的统一形态）。
pub(super) fn set_property(
    object: &JsObject,
    key: PropertyKey,
    value: JsValue,
    context: &mut Context,
) {
    let _ = object.create_data_property(key, value, context);
}

/// 以函数指针注册一个方法属性。
pub(super) fn attach_method(
    object: &JsObject,
    name: &str,
    method: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
    context: &mut Context,
) {
    let function = FunctionObjectBuilder::new(
        context.realm(),
        boa_engine::NativeFunction::from_fn_ptr(method),
    )
    .name(js_string!(name))
    .build();
    let descriptor = PropertyDescriptor::builder()
        .value(function)
        .writable(true)
        .enumerable(false)
        .configurable(true)
        .build();
    object.insert_property(js_string!(name), descriptor);
}

/// ToString 强制转换（走规范语义，尊重 toString / Symbol.toPrimitive）。
pub(super) fn to_text(value: &JsValue, context: &mut Context) -> JsResult<String> {
    let string = value.to_string(context)?;
    Ok(string.to_std_string_escaped())
}

/// 取全局静态方法，如 `Object.keys` / `JSON.parse`。
pub(super) fn global_static_function(
    context: &mut Context,
    holder: &str,
    name: &str,
) -> JsResult<JsFunction> {
    let object = context.global_object().get(js_string!(holder), context)?;
    Ok(object
        .as_object()
        .and_then(|object| object.get(js_string!(name), context).ok())
        .and_then(|value| value.as_function())
        .ok_or_else(|| JsNativeError::typ().with_message(format!("{holder}.{name} unavailable")))?)
}

/// 用 `Array.prototype[Symbol.iterator]` 把 JS 数组包装成正宗迭代器对象，
/// 供 `entries()` / `keys()` / `values()` 与 `[Symbol.iterator]` 复用——
/// 免去手写 next() 游标的状态管理。
pub(super) fn array_iterator(array: &JsValue, context: &mut Context) -> JsResult<JsValue> {
    let symbol = boa_engine::JsSymbol::iterator();
    let array_constructor = context.global_object().get(js_string!("Array"), context)?;
    let prototype = array_constructor
        .as_object()
        .and_then(|object| object.get(js_string!("prototype"), context).ok())
        .and_then(|value| value.as_object())
        .ok_or_else(|| JsNativeError::typ().with_message("Array.prototype unavailable"))?;
    let method = prototype
        .get(symbol, context)
        .ok()
        .and_then(|value| value.as_function())
        .ok_or_else(|| JsNativeError::typ().with_message("Array iterator unavailable"))?;
    let iterator = method.call(array, &[], context)?;
    Ok(iterator)
}

/// 注册全局构造器：`build` 负责铸造实例对象（Boa 原生构造器需自造实例并返回）。
pub(super) fn register_constructor(
    context: &mut Context,
    name: &'static str,
    build: fn(&mut Context, &[JsValue]) -> JsResult<JsObject>,
) {
    let constructor = boa_engine::NativeFunction::from_copy_closure(move |_this, args, context| {
        Ok(JsValue::from(build(context, args)?))
    });
    let function = FunctionObjectBuilder::new(context.realm(), constructor)
        .name(js_string!(name))
        .constructor(true)
        .build();
    context
        .register_global_property(js_string!(name), function, Attribute::all())
        .expect("安装全局构造器失败");
}
