//! `fetch` 全局对象的引擎绑定层（阶段 A，`boa_*` 只允许出现在本模块）。
//!
//! 线程模型：每次 `fetch` 调用 spawn 一个工作线程执行阻塞 HTTP，结果经
//! unbounded channel 送回。`JsFunction` 句柄不是 `Send`，但挂起表位于引擎线程
//! 的 thread-local，跨线程只传输纯 Rust 的 [`crate::web::fetch::ResponseData`]。
//! 宿主事件循环（`BoaEngine::drive_event_loop`）取出结果、settle Promise 并驱动微任务。
//!
//! 多引擎共存于同一线程时以最后 `install` 的通道为准（CLI 场景单引擎，不受影响）。

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{self, Receiver, Sender};

use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::{JsFunction, JsPromise};
use boa_engine::property::{Attribute, PropertyDescriptor, PropertyKey};
use boa_engine::{
    Context, JsArgs, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, js_string,
};

use crate::web::fetch::{self as webfetch, FetchOutcome, ResponseData};

/// 一次请求的完成消息：请求序号 + HTTP 终局。
pub(super) type FetchResult = (u64, FetchOutcome);

/// 尚未 settle 的 Promise 两端句柄（仅存活于引擎线程）。
struct PendingFetch {
    id: u64,
    resolve: JsFunction,
    reject: JsFunction,
}

thread_local! {
    static PENDING: RefCell<Vec<PendingFetch>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    static SENDER: RefCell<Option<Sender<FetchResult>>> = const { RefCell::new(None) };
}

/// 安装全局 `fetch`，返回宿主事件循环用来收取完成结果的接收端。
pub(super) fn install(context: &mut Context) -> Receiver<FetchResult> {
    let (sender, receiver) = mpsc::channel();
    SENDER.with(|slot| *slot.borrow_mut() = Some(sender.clone()));

    let fetch_fn = NativeFunction::from_copy_closure(|_this, args, context| {
        let url = match args.get_or_undefined(0).as_string() {
            Some(string) => string.to_std_string_escaped(),
            None => {
                return Err(JsNativeError::typ()
                    .with_message("fetch() expects a string URL")
                    .into());
            }
        };

        let (promise, resolvers) = JsPromise::new_pending(context);
        let id = NEXT_ID.with(|cell| {
            let next = cell.get();
            cell.set(next + 1);
            next
        });
        PENDING.with(|list| {
            list.borrow_mut().push(PendingFetch {
                id,
                resolve: resolvers.resolve,
                reject: resolvers.reject,
            })
        });

        let sender = SENDER
            .with(|slot| slot.borrow().clone())
            .ok_or_else(|| JsNativeError::typ().with_message("fetch channel unavailable"))?;
        std::thread::spawn(move || {
            let outcome = webfetch::fetch_get(&url);
            let _ = sender.send((id, outcome));
        });

        Ok(promise.into())
    });

    let function = FunctionObjectBuilder::new(context.realm(), fetch_fn)
        .name(js_string!("fetch"))
        .build();
    context
        .register_global_property(js_string!("fetch"), function, Attribute::all())
        .expect("安装 fetch 全局失败");

    receiver
}

/// 是否仍有在途请求。
pub(super) fn has_pending() -> bool {
    PENDING.with(|list| !list.borrow().is_empty())
}

/// settle 一批完成结果：按序号找回挂起 Promise，resolve 为 `Response` 或 reject 为 `TypeError`。
pub(super) fn settle(results: &[FetchResult], context: &mut Context) {
    for (id, outcome) in results {
        let Some(pending) = take_pending(*id) else {
            continue;
        };
        let call = match outcome {
            Ok(data) => {
                let response = build_response(data, context);
                pending
                    .resolve
                    .call(&JsValue::undefined(), &[response], context)
            }
            Err(message) => {
                let error = JsNativeError::typ()
                    .with_message(message.clone())
                    .into_opaque(context);
                pending
                    .reject
                    .call(&JsValue::undefined(), &[error.into()], context)
            }
        };
        // settle 自身抛出属于宿主内部错误，直接显式暴露而不是变成未处理拒绝。
        if let Err(error) = call {
            eprintln!("mio: failed to settle fetch promise: {error}");
        }
    }
}

/// 超时收尾：以 `TypeError` 拒绝所有在途请求。
pub(super) fn reject_all_pending(message: &str, context: &mut Context) {
    let message = message.to_owned();
    for pending in PENDING.with(|list| std::mem::take(&mut *list.borrow_mut())) {
        let error = JsNativeError::typ()
            .with_message(message.clone())
            .into_opaque(context);
        let _ = pending
            .reject
            .call(&JsValue::undefined(), &[error.into()], context);
    }
}

/// 从 thread-local 挂起表移除并返回指定序号的条目。
fn take_pending(id: u64) -> Option<PendingFetch> {
    PENDING.with(|list| {
        let index = list.borrow().iter().position(|pending| pending.id == id)?;
        Some(list.borrow_mut().remove(index))
    })
}

/// 把 [`ResponseData`] 翻译为 JS `Response` 对象（duck-typed，v1 不设原型链）。
///
/// 体数据以 `_text` 字符串属性承载；`text()` / `json()` 通过 `this` 读取自身属性，
/// 因此全部是无需捕获的函数指针。
fn build_response(data: &ResponseData, context: &mut Context) -> JsValue {
    let response = JsObject::default(context.intrinsics());
    set_property(
        &response,
        js_string!("url"),
        JsValue::from(js_string!(data.url.as_str())),
        context,
    );
    set_property(
        &response,
        js_string!("status"),
        JsValue::from(u32::from(data.status)),
        context,
    );
    set_property(
        &response,
        js_string!("statusText"),
        JsValue::from(js_string!(data.status_text.as_str())),
        context,
    );
    set_property(
        &response,
        js_string!("ok"),
        JsValue::from(data.is_ok()),
        context,
    );
    set_property(
        &response,
        js_string!("_text"),
        JsValue::from(js_string!(data.body_text().as_str())),
        context,
    );

    let headers = build_headers(data, context);
    set_property(&response, js_string!("headers"), headers, context);

    attach_method(&response, "text", text_method, context);
    attach_method(&response, "json", json_method, context);

    response.into()
}

/// 构造 `Headers` 视图：小写键直接挂在对象上，`get` / `has` 走 `this` 查找。
fn build_headers(data: &ResponseData, context: &mut Context) -> JsValue {
    let headers = JsObject::default(context.intrinsics());
    for (name, value) in &data.headers {
        set_property(
            &headers,
            js_string!(name.as_str()),
            JsValue::from(js_string!(value.as_str())),
            context,
        );
    }
    attach_method(&headers, "get", header_get, context);
    attach_method(&headers, "has", header_has, context);
    headers.into()
}

fn set_property<K>(object: &JsObject, key: K, value: JsValue, context: &mut Context)
where
    K: Into<PropertyKey>,
{
    let _ = object.create_data_property(key, value, context);
}

/// 以函数指针注册一个不可枚举的方法属性。
fn attach_method(
    object: &JsObject,
    name: &str,
    method: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
    context: &mut Context,
) {
    let function = FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(method))
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

/// `Response.text()`：body 已在内存，直接返回已完成的 Promise。
fn text_method(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let body = read_internal_text(this, context)?;
    Ok(JsPromise::resolve(body, context)?.into())
}

/// `Response.json()`：用宿主 `JSON.parse` 解析内存 body。
fn json_method(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let body = read_internal_text(this, context)?;
    let json = context.global_object().get(js_string!("JSON"), context)?;
    let parse = json
        .as_object()
        .and_then(|object| object.get(js_string!("parse"), context).ok())
        .and_then(|value| value.as_function())
        .ok_or_else(|| JsNativeError::typ().with_message("JSON.parse unavailable"))?;
    let parsed = parse.call(&json, &[body], context)?;
    Ok(JsPromise::resolve(parsed, context)?.into())
}

fn read_internal_text(this: &JsValue, context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on Response")
            .into());
    };
    object.get(js_string!("_text"), context)
}

/// `Headers.get(name)`：缺失返回 `null`（WHATWG 语义）。
fn header_get(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(name) = args.get_or_undefined(0).as_string() else {
        return Ok(JsValue::null());
    };
    let key = name.to_std_string_escaped().to_ascii_lowercase();
    let Some(object) = this.as_object() else {
        return Ok(JsValue::null());
    };
    let value = object.get(js_string!(key.as_str()), context)?;
    Ok(if value.is_undefined() {
        JsValue::null()
    } else {
        value
    })
}

/// `Headers.has(name)`。
fn header_has(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let value = header_get(this, args, context)?;
    Ok(JsValue::from(!value.is_null()))
}
