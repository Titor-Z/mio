//! 全局 `serve` 的引擎绑定层（`boa_*` 只允许出现在本模块）。
//!
//! 线程模型（对齐 Deno/Bun「一个循环、一个事件源」）：`serve()` 另起一个服务端线程跑
//! [`webhttp::serve_loop`]；每收到一条完整请求，就把 [`ServerInbound`]（请求纯数据 + 一条
//! 回线 channel）塞进引擎的 [`LoopEvent::Server`] 通道，服务端线程**阻塞等待**结果。引擎线程
//! 调用 JS handler：同步返回 `Response` 立即回线；返回 Promise 则挂进「待决服务器请求表」，
//! 事件循环每轮排空微任务后扫描，落定再回线——于是 handler 里的 `await fetch(...)` 能正常完成。
//!
//! v1 边界：`Request` 暂不含标准 `body`/`text()`/`json()`（异步请求体的读取另议）；服务端
//! 每连接处理一个请求即关（`connection: close`），keep-alive 与并发后续再开。

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::{JsFunction, JsPromise, JsUint8Array};
use boa_engine::property::Attribute;
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, js_string,
};

use crate::web::net::http::ParsedRequest;
use crate::web::net::server::{self as webhttp, ResponseSpec};

use super::fetch::{self as engine_fetch, LoopEvent};
use super::response;
use super::support::{attach_method, global_static_function, set_property, to_text};

/// 一条进来的请求：处理器序号 + 请求纯数据 + 回线发送端。
pub(super) struct ServerInbound {
    /// `serve(handler)` 注册的处理器序号（支持多次 serve）。
    handler_id: usize,
    /// 已解析的请求。
    request: ParsedRequest,
    /// 把响应（或错误消息）写回服务端线程。
    reply: mpsc::Sender<Result<ResponseSpec, String>>,
}

/// 一个挂起的异步 handler：回线端 + 其返回的 Promise（仅存活于引擎线程）。
struct PendingServer {
    reply: mpsc::Sender<Result<ResponseSpec, String>>,
    promise: JsPromise,
}

/// 当前在跑的服务器数量：>0 时事件循环保持存活（即便无在途 fetch/定时器）。
static LISTENING: AtomicUsize = AtomicUsize::new(0);

/// 一个已登记、待 handler 异步读取的请求体。
struct RequestBody {
    bytes: Vec<u8>,
}

thread_local! {
    /// `serve()` 注册的处理器表，按序号索引。
    static HANDLERS: RefCell<Vec<JsFunction>> = const { RefCell::new(Vec::new()) };
    /// 异步 handler 挂起表：Promise 落定后据此回线。
    static PENDING_SERVER: RefCell<Vec<PendingServer>> = const { RefCell::new(Vec::new()) };
    /// 请求体登记表：序号 → 字节 + content-type（`_requestId` 关联，供 `req.json()` 等读）。
    static BODIES: RefCell<Vec<(usize, RequestBody)>> = const { RefCell::new(Vec::new()) };
    static NEXT_BODY_ID: std::cell::Cell<usize> = const { std::cell::Cell::new(1) };
}

/// 是否仍有在监听的服务器（事件循环据此决定存活）。
pub(super) fn is_listening() -> bool {
    LISTENING.load(Ordering::SeqCst) > 0
}

/// 是否仍有挂起的异步 handler（其响应尚未回线）。
pub(super) fn has_pending() -> bool {
    PENDING_SERVER.with(|list| !list.borrow().is_empty())
}

/// 安装全局 `serve`。
pub(super) fn install(context: &mut Context) {
    let serve_fn = NativeFunction::from_copy_closure(|_this, args, context| {
        let Some(handler) = args.get_or_undefined(0).as_function() else {
            return Err(JsNativeError::typ()
                .with_message("serve expects a function handler")
                .into());
        };
        let handler_id = HANDLERS.with(|list| {
            let mut list = list.borrow_mut();
            list.push(handler.clone());
            list.len() - 1
        });

        let addr = read_addr(args.get_or_undefined(1), context)?;
        let sender = engine_fetch::sender()
            .ok_or_else(|| JsNativeError::typ().with_message("serve: event channel unavailable"))?;

        // 先增计数再 spawn：循环据此保持存活，直到首个请求到达也来得及。
        LISTENING.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let _ = webhttp::serve_loop(addr, move |request: &ParsedRequest| {
                let (reply_tx, reply_rx) = mpsc::channel();
                let inbound = ServerInbound {
                    handler_id,
                    request: request.clone(),
                    reply: reply_tx,
                };
                if sender.send(LoopEvent::Server(inbound)).is_err() {
                    return ResponseSpec::text(500, "engine unavailable");
                }
                // 阻塞等引擎回线（同步 handler 即时；异步 handler 待 Promise 落定）。
                match reply_rx.recv() {
                    Ok(Ok(spec)) => spec,
                    Ok(Err(message)) => ResponseSpec::text(500, &message),
                    Err(_) => ResponseSpec::text(500, "handler produced no response"),
                }
            });
        });

        Ok(JsValue::undefined())
    });

    let function = FunctionObjectBuilder::new(context.realm(), serve_fn)
        .name(js_string!("serve"))
        .build();
    context
        .register_global_property(js_string!("serve"), function, Attribute::all())
        .expect("安装 serve 全局失败");
}

/// 引擎线程处理一条进来的请求：调用 handler，同步 `Response` 直接回线，Promise 挂起待决。
///
/// 任何 handler 抛错或返回非 Response/Promise，都转成服务端线程的 500，绝不中断事件循环。
pub(super) fn dispatch_server(inbound: ServerInbound, context: &mut Context) {
    let Some(handler) = HANDLERS.with(|list| list.borrow().get(inbound.handler_id).cloned()) else {
        let _ = inbound
            .reply
            .send(Err("serve handler unavailable".to_owned()));
        return;
    };
    let request_value = match build_request(&inbound.request, context) {
        Ok(value) => value,
        Err(error) => {
            let _ = inbound.reply.send(Err(error.to_string()));
            return;
        }
    };
    let value = match handler.call(&JsValue::undefined(), &[request_value], context) {
        Ok(value) => value,
        Err(error) => {
            let _ = inbound.reply.send(Err(format!("handler threw: {error}")));
            return;
        }
    };
    // 同步 Response：立即回线。
    if let Some(spec) = response::response_to_spec(&value, context) {
        let _ = inbound.reply.send(Ok(spec));
        return;
    }
    // 异步：返回 Promise → 挂起表，落定后由 poll_pending_server 回线。
    if let Some(object) = value.as_object()
        && let Ok(promise) = JsPromise::from_object(object.clone())
    {
        PENDING_SERVER.with(|list| {
            list.borrow_mut().push(PendingServer {
                reply: inbound.reply,
                promise,
            })
        });
        return;
    }
    let _ = inbound.reply.send(Err(
        "serve handler must return a Response (or a Promise resolving to one)".to_owned(),
    ));
}

/// 扫描挂起表：已落定的异步 handler 据其结果回线，未落定者留待下一轮。
///
/// 应在每轮 `run_jobs` 之后调用，使 `await fetch`/`await setTimeout` 完成的 Promise 能被兑现。
pub(super) fn poll_pending_server(context: &mut Context) {
    PENDING_SERVER.with(|list| {
        let mut list = list.borrow_mut();
        let mut index = 0;
        while index < list.len() {
            match list[index].promise.state() {
                boa_engine::builtins::promise::PromiseState::Pending => index += 1,
                boa_engine::builtins::promise::PromiseState::Fulfilled(value) => {
                    let outcome = match response::response_to_spec(&value, context) {
                        Some(spec) => Ok(spec),
                        None => Err("handler resolved to a non-Response".to_owned()),
                    };
                    let _ = list[index].reply.send(outcome);
                    list.remove(index);
                }
                boa_engine::builtins::promise::PromiseState::Rejected(reason) => {
                    let message = JsError::from_opaque(reason).to_string();
                    let _ = list[index]
                        .reply
                        .send(Err(format!("handler rejected: {message}")));
                    list.remove(index);
                }
            }
        }
    });
}

/// 预算耗尽：给所有挂起的异步 handler 回一个 500，令服务端线程关掉连接而非无限阻塞。
pub(super) fn abandon_pending() {
    for entry in PENDING_SERVER.with(|list| std::mem::take(&mut *list.borrow_mut())) {
        let _ = entry.reply.send(Err("serve handler timed out".to_owned()));
    }
}

/// 从解析出的请求构造标准形态的 `Request`（method / url / headers / 异步体访问器）。
///
/// `url` 按 WHATWG 为绝对式；体已解析入内存，按序号登记，`text()`/`json()`/`arrayBuffer()`
/// 经 `this._requestId` 回查（同 `Blob` 模式，Boa 0.22 无 `host_data` 可用）。
fn build_request(request: &ParsedRequest, context: &mut Context) -> JsResult<JsValue> {
    let head = &request.head;
    let host = head.header("host").unwrap_or("localhost");
    let target = if head.target.starts_with('/') {
        head.target.clone()
    } else {
        format!("/{}", head.target)
    };
    let url = format!("http://{host}{target}");

    // 登记请求体，拿序号回指。
    let id = NEXT_BODY_ID.with(|cell| {
        let next = cell.get();
        cell.set(next + 1);
        next
    });
    BODIES.with(|table| {
        table.borrow_mut().push((
            id,
            RequestBody {
                bytes: request.body.clone(),
            },
        ))
    });

    let object = JsObject::default(context.intrinsics());
    set_property(
        &object,
        js_string!("_requestId").into(),
        JsValue::from(id as u32),
        context,
    );
    set_property(
        &object,
        js_string!("method").into(),
        JsValue::from(js_string!(head.method.as_str())),
        context,
    );
    set_property(
        &object,
        js_string!("url").into(),
        JsValue::from(js_string!(url.as_str())),
        context,
    );
    let headers = response::new_headers(head.headers.clone(), context)?;
    set_property(
        &object,
        js_string!("headers").into(),
        headers.into(),
        context,
    );
    set_property(&object, js_string!("body").into(), JsValue::null(), context);

    for name in ["text", "json", "arrayBuffer"] {
        attach_method(&object, name, request_method(name), context);
    }

    Ok(object.into())
}

/// 取本实现 `Request` 的请求体字节；非本实现（无 `_requestId`）报错。
fn request_body(this: &JsValue, context: &mut Context) -> JsResult<RequestBody> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on Request")
            .into());
    };
    let id = object.get(js_string!("_requestId"), context)?;
    let id = id
        .as_number()
        .ok_or_else(|| JsNativeError::typ().with_message("method must be called on Request"))?
        as usize;
    BODIES.with(|table| {
        table
            .borrow()
            .iter()
            .find(|(existing, _)| *existing == id)
            .map(|(_, body)| RequestBody {
                bytes: body.bytes.clone(),
            })
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("request body unavailable")
                    .into()
            })
    })
}

fn request_method(name: &str) -> fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue> {
    match name {
        "text" => request_text,
        "json" => request_json,
        "arrayBuffer" => request_array_buffer,
        _ => unreachable!("未知的 request 方法"),
    }
}

/// `Request.text()`：已缓冲字节 → UTF-8 文本的已完成 Promise。
fn request_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let body = request_body(this, context)?;
    let text = String::from_utf8_lossy(&body.bytes).into_owned();
    Ok(JsPromise::resolve(JsValue::from(js_string!(text.as_str())), context)?.into())
}

/// `Request.json()`：已缓冲字节 → `JSON.parse` 的已完成 Promise。
fn request_json(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let body = request_body(this, context)?;
    let text = String::from_utf8_lossy(&body.bytes).into_owned();
    let parse = global_static_function(context, "JSON", "parse")?;
    let json = context.global_object().get(js_string!("JSON"), context)?;
    let parsed = parse.call(&json, &[JsValue::from(js_string!(text.as_str()))], context)?;
    Ok(JsPromise::resolve(parsed, context)?.into())
}

/// `Request.arrayBuffer()`：已缓冲字节 → Uint8Array 的已完成 Promise。
fn request_array_buffer(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let body = request_body(this, context)?;
    let view = JsUint8Array::from_iter(body.bytes, context)?;
    Ok(JsPromise::resolve(JsValue::from(view), context)?.into())
}

/// 从 `serve(handler, { port, hostname })` 读监听地址；缺省绑 `127.0.0.1:0`（临时端口）。
fn read_addr(options: &JsValue, context: &mut Context) -> JsResult<SocketAddr> {
    let mut port: u16 = 0;
    let mut host: IpAddr = Ipv4Addr::LOCALHOST.into();

    if let Some(object) = options.as_object() {
        let port_value = object.get(js_string!("port"), context)?;
        if let Some(number) = port_value.as_number() {
            port = number as u16;
        }
        let hostname = object.get(js_string!("hostname"), context)?;
        if !hostname.is_undefined() && !hostname.is_null() {
            let text = to_text(&hostname, context)?;
            host = text.parse::<IpAddr>().map_err(|_| {
                JsNativeError::range().with_message(format!(
                    "invalid hostname {text:?}: serve 只接受 IP 字面量（如 127.0.0.1），域名解析属于客户端侧"
                ))
            })?;
        }
    }
    Ok(SocketAddr::new(host, port))
}
