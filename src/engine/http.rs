//! 全局 `serve` 的引擎绑定层（`boa_*` 只允许出现在本模块）。
//!
//! 线程模型（对齐 Deno/Bun「一个循环、一个事件源」）：`serve()` 另起一个服务端线程跑
//! [`webhttp::serve_loop`]；每收到一条完整请求，就把 [`ServerInbound`]（含请求纯数据 +
//! 一条回线 channel）塞进引擎的 [`LoopEvent::Server`] 通道，服务端线程**阻塞等待**引擎把
//! 结果送回来。引擎线程在 [`dispatch_server`] 里**同步**调用 JS handler，取出 `Response`
//! 拆成 [`webhttp::ResponseSpec`] 回线。全程只有一个等待点（事件通道 + 定时器超时），
//! JS 线程不被任何 socket 阻塞。
//!
//! v1 边界：handler 必须**同步返回 `Response`**——异步 handler（返回 Promise，内部可
//! `await fetch`）需要「挂起的服务器请求表」与 fetch 完成通道的深度协作，列为后续项；
//! 检测到返回 thenable 时回 500 并给出明确原因，而非静默丢包。

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::{JsFunction, JsUint8Array};
use boa_engine::property::Attribute;
use boa_engine::{
    Context, JsArgs, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, js_string,
};

use crate::web::net::http::ParsedRequest;
use crate::web::net::server::{self as webhttp, ResponseSpec};

use super::fetch::{self as engine_fetch, LoopEvent};
use super::response;
use super::support::{set_property, to_text};

/// 一条进来的请求：处理器序号 + 请求纯数据 + 回线发送端。
pub(super) struct ServerInbound {
    /// `serve(handler)` 注册的处理器序号（支持多次 serve）。
    handler_id: usize,
    /// 已解析的请求。
    request: ParsedRequest,
    /// 把响应（或错误消息）写回服务端线程。
    reply: mpsc::Sender<Result<ResponseSpec, String>>,
}

/// 当前在跑的服务器数量：>0 时事件循环保持存活（即便无在途 fetch/定时器）。
static LISTENING: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// `serve()` 注册的处理器表，按序号索引。
    static HANDLERS: RefCell<Vec<JsFunction>> = const { RefCell::new(Vec::new()) };
}

/// 是否仍有在监听的服务器（事件循环据此决定存活）。
pub(super) fn is_listening() -> bool {
    LISTENING.load(Ordering::SeqCst) > 0
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
                // 阻塞等引擎同步处理完这一条。
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

/// 引擎线程处理一条进来的请求：构造 `Request`、同步调用 handler、把 `Response` 回线。
///
/// 任何 handler 抛错或返回非 Response，都转成服务端线程的 500，绝不中断事件循环。
pub(super) fn dispatch_server(inbound: ServerInbound, context: &mut Context) {
    let outcome = run_handler(inbound.handler_id, &inbound.request, context);
    let _ = inbound.reply.send(outcome);
}

/// 调用注册的第 `handler_id` 个处理器处理 `request`，产出 [`ResponseSpec`] 或错误消息。
fn run_handler(
    handler_id: usize,
    request: &ParsedRequest,
    context: &mut Context,
) -> Result<ResponseSpec, String> {
    let handler = HANDLERS.with(|list| list.borrow().get(handler_id).cloned());
    let Some(handler) = handler else {
        return Err("serve handler unavailable".to_owned());
    };

    let request_value = match build_request(request, context) {
        Ok(value) => value,
        Err(error) => return Err(error.to_string()),
    };

    let result = handler
        .call(&JsValue::undefined(), &[request_value], context)
        .map_err(|error| format!("handler threw: {error}"))?;

    interpret_return(&result, context)
}

/// 把 handler 返回值解释为响应：本实现 `Response` → 抽出 spec；thenable → 异步未支持；其余 → 报错。
fn interpret_return(value: &JsValue, context: &mut Context) -> Result<ResponseSpec, String> {
    if let Some(spec) = response::response_to_spec(value, context) {
        return Ok(spec);
    }
    if is_thenable(value, context) {
        return Err(
            "await  inside serve handler is not yet supported; return a Response synchronously"
                .to_owned(),
        );
    }
    Err("serve handler must return a Response".to_owned())
}

/// 粗略判定 thenable：对象上 `then` 是函数即视为 Promise。
fn is_thenable(value: &JsValue, context: &mut Context) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object
        .get(js_string!("then"), context)
        .map(|then| then.as_function().is_some())
        .unwrap_or(false)
}

/// 从解析出的请求构造标准形态的 `Request`（v1：method / url / headers）。
///
/// 体不进 `Request`（规范要求经异步 `body`/`text()` 读取，异步 handler 支持后才提供）；
/// 同步 handler 用 `method`/`url`/`headers` 即可应答。
fn build_request(request: &ParsedRequest, context: &mut Context) -> JsResult<JsValue> {
    let head = &request.head;
    let host = head.header("host").unwrap_or("localhost");
    let target = if head.target.starts_with('/') {
        head.target.clone()
    } else {
        format!("/{}", head.target)
    };
    let url = format!("http://{host}{target}");

    let object = JsObject::default(context.intrinsics());
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

    // 请求体以同步可读的 Uint8Array 附于 `body`（v1 取舍：标准 body 是流，异步支持后再换）。
    let body = JsUint8Array::from_iter(request.body.clone(), context)?;
    set_property(&object, js_string!("body").into(), body.into(), context);

    Ok(object.into())
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
