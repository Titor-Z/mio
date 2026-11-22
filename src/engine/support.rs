//! Web 全局绑定层的共享工具：属性挂载、字符串强制转换、数组迭代器桥接。
//!
//! 这些是 Boa 宿主侧的胶水，无规范语义，供 `url` / `blob` / `fetch` 等绑定复用。
//!
//! W3C 对齐：本文件还负责「带原型的全局构造器」注册。带原型的类使
//! `x instanceof X` 与 `Object.prototype.toString.call(x)` 成立，方法挂在原型上
//! 而非逐实例复制；旧的零原型 [`register_constructor`] 保留给尚未迁移的类。

use std::cell::RefCell;

use boa_engine::builtins::object::OrdinaryObject;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::JsFunction;
use boa_engine::property::{Attribute, PropertyDescriptor, PropertyKey};
use boa_engine::{
    Context, JsNativeError, JsObject, JsResult, JsString, JsSymbol, JsValue, js_string,
};

thread_local! {
    /// 已注册原型的类：类名 → 原型对象。工厂铸造实例时据此取 `[[Prototype]]`。
    static PROTOTYPES: RefCell<rustc_hash::FxHashMap<&'static str, JsObject>> =
        RefCell::new(rustc_hash::FxHashMap::default());
}

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

/// 注册一个**带原型**的全局构造器（W3C 对齐形态）。
///
/// 与 [`register_constructor`] 的差别：实例由 [`instantiate`] 用同一个原型铸造，
/// 因此 `x instanceof X` 成立、`Object.prototype.toString.call(x)` 得到 `[object X]`；
/// 方法挂在原型上，不再逐实例复制。
///
/// `members` 为「成员名 → 方法」，按不可枚举、可写、可配置的属性挂到原型上。
pub(super) fn register_class(
    context: &mut Context,
    name: &'static str,
    build: fn(&mut Context, &[JsValue]) -> JsResult<JsObject>,
    members: &[(&'static str, NativeMethod)],
) {
    let prototype = JsObject::with_object_proto(context.intrinsics());

    for (member, method) in members {
        let function = FunctionObjectBuilder::new(
            context.realm(),
            boa_engine::NativeFunction::from_fn_ptr(*method),
        )
        .name(js_string!(*member))
        .build();
        let _ = prototype.insert_property(js_string!(*member), member_descriptor(function.into()));
    }
    let _ = prototype.insert_property(
        JsSymbol::to_string_tag(),
        member_descriptor(JsValue::from(js_string!(name))),
    );

    // 构造器本身：返回工厂铸造的对象（原生构造器返回对象时即以该对象为结果）。
    let constructor = boa_engine::NativeFunction::from_copy_closure(move |_this, args, context| {
        Ok(JsValue::from(build(context, args)?))
    });
    let function = FunctionObjectBuilder::new(context.realm(), constructor)
        .name(js_string!(name))
        .constructor(true)
        .build();

    // 原型与构造器互指，并把函数的 `prototype` 钉成不可改写的规范形态。
    let _ = prototype.insert_property(
        js_string!("constructor"),
        member_descriptor(JsValue::from(function.clone())),
    );
    let function_object: JsObject = function.clone().into();
    let _ = function_object.insert_property(
        js_string!("prototype"),
        PropertyDescriptor::builder()
            .value(prototype.clone())
            .writable(false)
            .enumerable(false)
            .configurable(false)
            .build(),
    );

    PROTOTYPES.with(|table| {
        table.borrow_mut().insert(name, prototype);
    });

    context
        .register_global_property(js_string!(name), function, Attribute::all())
        .expect("安装全局构造器失败");
}

/// 用已注册原型的类铸造一个普通实例（`ObjectCreate(X.prototype)`）。
///
/// 调用方随后自行挂载内部字段（如 `_headersId`）。
pub(super) fn instantiate(name: &'static str, context: &mut Context) -> JsResult<JsObject> {
    let prototype = PROTOTYPES.with(|table| table.borrow().get(name).cloned());
    let Some(prototype) = prototype else {
        return Err(JsNativeError::typ()
            .with_message(format!("类 `{name}` 尚未注册原型"))
            .into());
    };
    let _ = context;
    Ok(JsObject::from_proto_and_data(
        Some(prototype),
        OrdinaryObject,
    ))
}

/// 取已注册类的原型对象（已在安装阶段挂好成员）。
pub(super) fn prototype_of(name: &'static str) -> Option<JsObject> {
    PROTOTYPES.with(|table| table.borrow().get(name).cloned())
}

/// 把原型上已有的命名方法再挂到一个符号键上（如 `[Symbol.iterator]` 复用 `entries`）。
pub(super) fn alias_symbol_member(
    prototype: &JsObject,
    symbol: JsSymbol,
    member: &str,
    context: &mut Context,
) {
    if let Ok(value) = prototype.get(js_string!(member), context) {
        let _ = prototype.insert_property(symbol, member_descriptor(value));
    }
}

/// 原型成员的标准属性描述符：不可枚举、可写、可配置。
fn member_descriptor(value: JsValue) -> PropertyDescriptor {
    PropertyDescriptor::builder()
        .value(value)
        .writable(true)
        .enumerable(false)
        .configurable(true)
        .build()
}

/// 一个导出方法的签名，与 `NativeFunction::from_fn_ptr` 一致。
pub(super) type NativeMethod = fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>;

/// 按「导出名 → 方法」与「常量名 → 文本」表装配一个 `@std/*` 合成模块。
///
/// 初始化闭包只捕获两个 `&'static` 切片（均为 `Copy`），故可用 `from_copy_closure`；
/// 真正的 `NativeFunction` 在求值时现场构造。需要返回对象或二进制、或自定义
/// `undefined` 语义时，各自在 `engine/<模块>.rs` 里手写构建器。
pub(super) fn synthetic_module(
    module_name: &'static str,
    methods: &'static [(&'static str, NativeMethod)],
    constants: &'static [(&'static str, &'static str)],
    context: &mut Context,
) -> JsResult<boa_engine::Module> {
    use boa_engine::Module;
    use boa_engine::module::SyntheticModuleInitializer;

    let export_names: Vec<JsString> = methods
        .iter()
        .map(|(name, _)| js_string!(*name))
        .chain(constants.iter().map(|(name, _)| js_string!(*name)))
        .collect();

    let initializer = SyntheticModuleInitializer::from_copy_closure(move |synthetic, ctx| {
        for (name, method) in methods {
            let function = FunctionObjectBuilder::new(
                ctx.realm(),
                boa_engine::NativeFunction::from_fn_ptr(*method),
            )
            .name(js_string!(*name))
            .build();
            synthetic.set_export(&js_string!(*name), function.into())?;
        }
        for (name, value) in constants {
            synthetic.set_export(&js_string!(*name), JsValue::from(js_string!(*value)))?;
        }
        Ok(())
    });

    Ok(Module::synthetic(
        &export_names,
        initializer,
        Some(std::path::PathBuf::from(format!("@std/{module_name}"))),
        None,
        context,
    ))
}
