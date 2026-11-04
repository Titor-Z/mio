//! `fetch` 全局对象的引擎绑定层（阶段 A，`boa_*` 只允许出现在本模块）。
//!
//! 线程模型：每次 `fetch` 调用 spawn 一个工作线程执行阻塞 HTTP，结果经
//! unbounded channel 送回。`JsFunction` 句柄不是 `Send`，但挂起表位于引擎线程
//! 的 thread-local，跨线程只传输纯 Rust 的 [`crate::web::fetch::ResponseData`]。
//! 宿主事件循环（`BoaEngine::drive_event_loop`）取出结果、settle Promise 并驱动微任务。
//!
//! 取消语义（v1）：`controller.abort()` 在引擎线程同步拒绝登记的挂起 Promise，
//! 不等工作线程退出：阻塞的 ureq 读完自己的份后结果因挂起表已移除被丢弃。
//! 即「Promise 粒度取消，socket 自然排空」；升级需换成可取消的 IO 层。
//!
//! 多引擎共存于同一线程时以最后 `install` 的通道为准（CLI 场景单引擎，不受影响）。

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{self, Receiver, Sender};

use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::{JsArrayBuffer, JsFunction, JsPromise, JsUint8Array};
use boa_engine::property::{Attribute, PropertyDescriptor, PropertyKey};
use boa_engine::{
    Context, JsArgs, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, js_string,
};

use crate::web::fetch::{self as webfetch, FetchOutcome, ResponseData};

use super::{blob, http as engine_http, url as engine_url};

/// 一次请求的完成消息：请求序号 + HTTP 终局。
pub(super) type FetchResult = (u64, FetchOutcome);

/// 宿主事件循环唯一等待源上的一种事件：fetch 完成，或进来的 HTTP 请求。
///
/// 这是 Deno/Bun「一个循环、一个事件源」形态的落地：定时器不占通道（靠 `poll`/timeout
/// 驱动），fetch 完成与工作线程无关地经此通道回来，serve 进来的请求也走同一条。
pub(super) enum LoopEvent {
    /// 一个 fetch 工作线程的终局。
    Fetch(FetchResult),
    /// 服务端线程收到一条完整请求，等待引擎线程同步处理并回线。
    Server(engine_http::ServerInbound),
}

/// 尚未 settle 的 Promise 两端句柄（仅存活于引擎线程）。
struct PendingFetch {
    id: u64,
    resolve: JsFunction,
    reject: JsFunction,
    /// 关联的 AbortSignal 序号（若 init 携带了 signal）。
    signal: Option<usize>,
}

thread_local! {
    static PENDING: RefCell<Vec<PendingFetch>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    static NEXT_SIGNAL_ID: Cell<usize> = const { Cell::new(1) };
    static SENDER: RefCell<Option<Sender<LoopEvent>>> = const { RefCell::new(None) };
    /// 本引擎创建的 AbortSignal 登记表：序号 → signal 对象（abort 回调只捕获序号）。
    static SIGNALS: RefCell<Vec<(usize, JsObject)>> = const { RefCell::new(Vec::new()) };
}

/// 安装全局 `fetch` 与 `AbortController`，返回宿主事件循环用来收取完成结果的接收端。
pub(super) fn install(context: &mut Context) -> Receiver<LoopEvent> {
    let (sender, receiver) = mpsc::channel();
    SENDER.with(|slot| *slot.borrow_mut() = Some(sender.clone()));
    install_abort_controller(context);

    let fetch_fn = NativeFunction::from_copy_closure(|_this, args, context| {
        let parsed = parse_request(args, context)?;

        // 规范：向已 abort 的 signal 发请求，立刻得到拒绝的 Promise，不发请求。
        // 需携带 name=AbortError，故手动 settle（JsPromise::reject 只接受 JsError 形态）。
        if parsed.aborted {
            let (promise, resolvers) = JsPromise::new_pending(context);
            let error = abort_error(context);
            let _ = resolvers
                .reject
                .call(&JsValue::undefined(), &[error], context);
            return Ok(promise.into());
        }

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
                signal: parsed.signal_id,
            })
        });

        let sender = SENDER
            .with(|slot| slot.borrow().clone())
            .ok_or_else(|| JsNativeError::typ().with_message("fetch channel unavailable"))?;
        std::thread::spawn(move || {
            let outcome = webfetch::fetch_execute(&parsed.request);
            let _ = sender.send(LoopEvent::Fetch((id, outcome)));
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

/// 取当前事件循环的完成通道发送端（供 `serve` 服务端线程克隆）。
pub(super) fn sender() -> Option<Sender<LoopEvent>> {
    SENDER.with(|slot| slot.borrow().clone())
}

/// `parse_request` 的产出：纯数据请求 + 取消关联信息。
struct ParsedRequest {
    request: webfetch::FetchRequest,
    /// init.signal 的序号（已确认可用且未 abort）。
    signal_id: Option<usize>,
    /// 携带了已 abort 的 signal：调用方需短路为拒绝 Promise。
    aborted: bool,
}

/// 解析 `fetch(url, init?)`：把 init 拆成纯数据请求，并提取 signal 序号。
///
/// init 字段范围：`method`（字符串）/ `headers`（普通对象，键名小写化）/
/// `body`（字符串或 Uint8Array）/ `signal`（本实现创建的 AbortSignal）。
/// 不支持数组对形式的 headers 与 ReadableStream body（留待后续）。
fn parse_request(args: &[JsValue], context: &mut Context) -> JsResult<ParsedRequest> {
    // 入参接受字符串或本实现铸造的 URL 实例（读 href 快照）。
    let url = match args.get_or_undefined(0).as_string() {
        Some(string) => string.to_std_string_escaped(),
        None => match engine_url::url_href(args.get_or_undefined(0), context)? {
            Some(href) => href,
            None => {
                return Err(JsNativeError::typ()
                    .with_message("fetch() expects a string URL or URL instance")
                    .into());
            }
        },
    };
    let mut request = webfetch::FetchRequest::new(url);
    let mut signal_id = None;
    let mut aborted = false;

    let init = args.get_or_undefined(1);
    if init.is_undefined() || init.is_null() {
        return Ok(ParsedRequest {
            request,
            signal_id,
            aborted,
        });
    }
    let Some(init) = init.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("fetch init must be an object")
            .into());
    };

    let method = init.get(js_string!("method"), context)?;
    if !method.is_undefined() {
        let Some(method) = method.as_string() else {
            return Err(JsNativeError::typ()
                .with_message("method must be a string")
                .into());
        };
        request.method = method.to_std_string_escaped().to_ascii_uppercase();
    }

    let headers = init.get(js_string!("headers"), context)?;
    if !headers.is_undefined() && !headers.is_null() {
        let Some(headers) = headers.as_object() else {
            return Err(JsNativeError::typ()
                .with_message("headers must be an object")
                .into());
        };
        // Boa 的 PropertyMap 无稳定枚举 API，走全局 Object.keys（同 JSON.parse 桥接模式）。
        let keys = global_static_function(context, "Object", "keys")?;
        let names = keys.call(&JsValue::undefined(), &[headers.clone().into()], context)?;
        let names = names
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("Object.keys returned a non-array"))?;
        let length = names
            .get(js_string!("length"), context)?
            .as_number()
            .unwrap_or(0.0) as u32;
        for index in 0..length {
            let key = names.get(index, context)?;
            let Some(name) = key.as_string() else {
                continue;
            };
            let name = name.to_std_string_escaped();
            // 属性查找用原键（JS 对象大小写敏感），上线前才按 Headers 规范小写化。
            let value = headers.get(js_string!(name.as_str()), context)?;
            if value.is_undefined() {
                continue;
            }
            let text = value.to_string(context)?;
            request
                .headers
                .push((name.to_ascii_lowercase(), text.to_std_string_escaped()));
        }
    }

    let body = init.get(js_string!("body"), context)?;
    if !body.is_undefined() && !body.is_null() {
        if request.method == "GET" || request.method == "HEAD" {
            return Err(JsNativeError::typ()
                .with_message(format!("{} request cannot have a body", request.method))
                .into());
        }
        let (bytes, auto_content_type) = if let Some(string) = body.as_string() {
            (string.to_std_string_escaped().into_bytes(), None)
        } else if let Some((bytes, content_type)) = blob::body_from_registry(&body, context)? {
            // FormData → multipart、Blob → 原始字节（规范各自的 content-type 兜底）。
            (bytes, content_type)
        } else if let Some(object) = body.as_object() {
            if let Ok(array) = JsUint8Array::from_object(object.clone()) {
                (array.to_vec(context)?, None)
            } else if let Ok(buffer) = JsArrayBuffer::from_object(object.clone()) {
                (buffer.to_vec().unwrap_or_default(), None)
            } else {
                return Err(JsNativeError::typ()
                    .with_message("body must be a string, Blob, FormData or buffer-like object")
                    .into());
            }
        } else {
            return Err(JsNativeError::typ()
                .with_message("body must be a string, Blob, FormData or buffer-like object")
                .into());
        };
        request.body = Some(bytes);
        if let Some(content_type) = auto_content_type
            && !request
                .headers
                .iter()
                .any(|(name, _)| name == "content-type")
        {
            request
                .headers
                .push(("content-type".to_string(), content_type));
        }
    }

    let signal = init.get(js_string!("signal"), context)?;
    if !signal.is_undefined() && !signal.is_null() {
        let Some(signal) = signal.as_object() else {
            return Err(JsNativeError::typ()
                .with_message("signal must be an AbortSignal")
                .into());
        };
        // 只认本实现铸造的 signal（带 _signalId 内部标记），避免伪造对象静默失效。
        let id = signal.get(js_string!("_signalId"), context)?;
        let Some(id) = id.as_number() else {
            return Err(JsNativeError::typ()
                .with_message("signal must be an AbortSignal from AbortController")
                .into());
        };
        signal_id = Some(id as usize);
        aborted = signal.get(js_string!("aborted"), context)?.to_boolean();
    }

    Ok(ParsedRequest {
        request,
        signal_id,
        aborted,
    })
}

/// 取全局静态方法，如 `Object.keys` / `JSON.parse`。
fn global_static_function(context: &mut Context, holder: &str, name: &str) -> JsResult<JsFunction> {
    let object = context.global_object().get(js_string!(holder), context)?;
    Ok(object
        .as_object()
        .and_then(|object| object.get(js_string!(name), context).ok())
        .and_then(|value| value.as_function())
        .ok_or_else(|| JsNativeError::typ().with_message(format!("{holder}.{name} unavailable")))?)
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

/// 安装全局 `AbortController`：`new` 出 controller，`controller.abort()` 同步拒绝
/// 该 signal 名下的全部在途 fetch。signal 以 `_signalId` 内部标记关联序号。
///
/// Boa 原生构造器语义：闭包收到的 `this` 是 new.target（函数自身）而非实例，
/// 返回非对象时 Boa 会另建空对象丢弃副作用——因此必须自造实例对象并返回。
fn install_abort_controller(context: &mut Context) {
    let constructor = NativeFunction::from_copy_closure(|_this, _args, context| {
        let controller = JsObject::default(context.intrinsics());
        let id = NEXT_SIGNAL_ID.with(|cell| {
            let next = cell.get();
            cell.set(next + 1);
            next
        });

        let signal = JsObject::default(context.intrinsics());
        set_property(
            &signal,
            js_string!("aborted"),
            JsValue::from(false),
            context,
        );
        set_property(
            &signal,
            js_string!("_signalId"),
            JsValue::from(id as u32),
            context,
        );
        SIGNALS.with(|list| list.borrow_mut().push((id, signal.clone())));

        // JsObject 不是 Copy，closure 只捕获序号，abort 时回登记表取 signal 句柄。
        let abort = NativeFunction::from_copy_closure(move |_this, _args, context| {
            let signal = SIGNALS.with(|list| {
                list.borrow()
                    .iter()
                    .find(|(existing, _)| *existing == id)
                    .map(|(_, object)| object.clone())
            });
            let Some(signal) = signal else {
                return Ok(JsValue::undefined());
            };
            if signal.get(js_string!("aborted"), context)?.to_boolean() {
                return Ok(JsValue::undefined());
            }
            set_property(&signal, js_string!("aborted"), JsValue::from(true), context);
            reject_signal_pending(id, context);
            Ok(JsValue::undefined())
        });
        let abort_fn = FunctionObjectBuilder::new(context.realm(), abort)
            .name(js_string!("abort"))
            .build();
        set_property(&controller, js_string!("abort"), abort_fn.into(), context);
        set_property(&controller, js_string!("signal"), signal.into(), context);
        Ok(controller.into())
    });

    let function = FunctionObjectBuilder::new(context.realm(), constructor)
        .name(js_string!("AbortController"))
        .constructor(true)
        .build();
    context
        .register_global_property(js_string!("AbortController"), function, Attribute::all())
        .expect("安装 AbortController 全局失败");
}

/// WHATWG 取消错误：`Error` 实例但 `name === 'AbortError'`（v1 用 Error 代 DOMException）。
fn abort_error(context: &mut Context) -> JsValue {
    let error = JsNativeError::error()
        .with_message("This operation was aborted")
        .into_opaque(context);
    set_property(
        &error,
        js_string!("name"),
        JsValue::from(js_string!("AbortError")),
        context,
    );
    error.into()
}

/// abort 收尾：把指定 signal 名下的在途请求从挂起表移除并拒绝。
/// 工作线程的结果稍后因序号不存在被 settle 静默丢弃。
fn reject_signal_pending(id: usize, context: &mut Context) {
    let all = PENDING.with(|list| std::mem::take(&mut *list.borrow_mut()));
    let (hit, keep): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|pending| pending.signal == Some(id));
    PENDING.with(|list| *list.borrow_mut() = keep);
    for pending in hit {
        let error = abort_error(context);
        let _ = pending
            .reject
            .call(&JsValue::undefined(), &[error], context);
    }
}

/// 把 [`ResponseData`] 翻译为 JS `Response` 对象（duck-typed，v1 不设原型链）。
///
/// 体数据以 `_text` 字符串与 `_bytes`（Uint8Array）双形式承载；`text()` / `json()` /
/// `arrayBuffer()` 通过 `this` 读取自身属性，因此全部是无需捕获的函数指针。
/// 注：`arrayBuffer()` 返回共享底层缓冲的同一个视图（规范应每次副本，v1 取舍）。
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
    // typed array 构造仅可能在内存耗尽时失败：降级为不设 _bytes（arrayBuffer 得 undefined）。
    if let Ok(bytes) = JsUint8Array::from_iter(data.body.clone(), context) {
        set_property(&response, js_string!("_bytes"), bytes.into(), context);
    }

    let headers = build_headers(data, context);
    set_property(&response, js_string!("headers"), headers, context);

    attach_method(&response, "text", text_method, context);
    attach_method(&response, "json", json_method, context);
    attach_method(&response, "arrayBuffer", array_buffer_method, context);

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
    let parse = global_static_function(context, "JSON", "parse")?;
    let json = context.global_object().get(js_string!("JSON"), context)?;
    let parsed = parse.call(&json, &[body], context)?;
    Ok(JsPromise::resolve(parsed, context)?.into())
}

/// `Response.arrayBuffer()`：返回承载 body 的 `Uint8Array`（约定 1：二进制 → Uint8Array）。
fn array_buffer_method(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on Response")
            .into());
    };
    let bytes = object.get(js_string!("_bytes"), context)?;
    Ok(JsPromise::resolve(bytes, context)?.into())
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
