//! WHATWG `fetch` 的纯数据层：请求执行与响应模型。
//!
//! 本文件不出现任何引擎类型（`boa_*` 仅限 `src/engine/`）：HTTP 报文编解码由
//! [`crate::web::net::http`] 承担，socket 搬运由 `reactor`（mio）的非阻塞读驱动，
//! 引擎绑定层把 [`ResponseData`] 翻译为 JS `Response` 对象。
//! 线程模型：每个请求在一个工作线程上完成，结果经 channel 送回宿主事件循环。
//!
//! 当前范围：任意 method + 请求头 + 字节 body；4xx/5xx 是**成功结果**
//! （fetch 语义只在网络/协议层失败时才 reject）。
//! `3xx` 按 WHATWG 默认 `follow` 模式自动跟随（上限 20 跳，每一跳新开一条连接）；
//! `redirect: 'manual' / 'error'` 暂不支持。
//! 仅支持 `http`：本层不含 TLS，`https` 及其它协议一律归一为网络层失败。
//! 取消不在本层：引擎层对已登记的挂起 Promise 立即拒绝，工作线程读完即丢弃结果。
//!
//! 无 socket 级超时：读事件循环阻塞至响应完成或连接关闭，整体时限交由宿主事件循环的
//! `run.timeoutMs` 预算裁决——这样「超时」的拒绝消息始终来自宿主（措辞含 `timed out`），
//! 而非本层先一步以另一种措辞抢先。工作线程对慢连接挂起后，宿主超时即拒绝并丢弃结果。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream as StdTcpStream, ToSocketAddrs};

use reactor::{Events, Interest, Poll, Token, net::TcpStream};
use url::Url;

use crate::web::net::http::{self, Progress};

/// 一次 fetch 的终局：成功携带 [`ResponseData`]，失败携带应包装为 `TypeError` 的消息。
pub type FetchOutcome = Result<ResponseData, String>;

/// 解析后的请求：引擎层把 JS `fetch(url, init)` 拆成此纯数据描述后交给工作线程。
#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// 目标 URL。
    pub url: String,
    /// HTTP 方法（大写），默认 `GET`。
    pub method: String,
    /// 请求头（原样键名，ureq 自身大小写不敏感）。
    pub headers: Vec<(String, String)>,
    /// 请求体字节；`None` 表示无 body。
    pub body: Option<Vec<u8>>,
}

impl FetchRequest {
    /// 构造一个无 init 的 GET 请求。
    #[must_use]
    pub fn new(url: String) -> Self {
        Self {
            url,
            method: String::from("GET"),
            headers: Vec::new(),
            body: None,
        }
    }
}

/// HTTP 响应快照：状态、合并后的头、已完整读入内存的 body。
#[derive(Debug, Clone)]
pub struct ResponseData {
    /// 最终 URL：跟随重定向后的落地地址（无重定向时等于入参），对应 `Response.url`。
    pub url: String,
    /// 响应状态码。
    pub status: u16,
    /// 原因短语，如 `OK`、`Not Found`。
    pub status_text: String,
    /// 响应头：键已小写化，同名多值以 `, ` 合并（即 `Headers.get` 的组合值语义）。
    pub headers: Vec<(String, String)>,
    /// 完整响应体。
    pub body: Vec<u8>,
}

impl ResponseData {
    /// `Response.ok` 语义：2xx。
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// `Headers.get` 语义：大小写不敏感查找合并值，缺失返回 [`None`]。
    pub fn header(&self, name: &str) -> Option<&str> {
        let wanted = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == wanted)
            .map(|(_, value)| value.as_str())
    }

    /// `Headers.has` 语义。
    pub fn has_header(&self, name: &str) -> bool {
        self.header(name).is_some()
    }

    /// 响应体的 UTF-8 文本视图（非法字节按替换字符处理）。
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// 默认重定向上限（与 WHATWG fetch / 浏览器一致的 20 跳）。
const MAX_REDIRECTS: usize = 20;

/// 执行一次请求：解析 URL、建连、发送 [`http::serialize_request`] 的报文、
/// 用 `reactor` 的非阻塞读驱动 [`http::ResponseParser`] 直到响应完整；
/// 若响应是需跟随的重定向（301/302/303/307/308 + `Location`），以最终 URL 为准
/// 重新发起，直至终态响应或达到 [`MAX_REDIRECTS`]。
///
/// 不抛出：一切失败（非法 URL、非 http、DNS、连接、协议、截断、无效 Location、
/// 重定向成环）都归一为 [`FetchOutcome`] 的 `Err` 消息，由引擎层包装成 `TypeError` 拒绝 Promise。
pub fn fetch_execute(request: &FetchRequest) -> FetchOutcome {
    let mut url = request.url.clone();
    let mut method = request.method.clone();
    let mut headers = request.headers.clone();
    let mut body = request.body.clone();

    // 首个请求不计入跳数：循环至多 MAX_REDIRECTS 次重定向（共 MAX_REDIRECTS+1 个请求）。
    for _ in 0..=MAX_REDIRECTS {
        let (addr, authority, path) = split_target(&url)?;
        let wire = http::serialize_request(&method, &path, &authority, &headers, body.as_deref());
        let parsed = roundtrip(addr, &wire, method == "HEAD")?;
        let head = parsed.head;

        match redirect_plan(&url, &method, head.status, &head.headers) {
            Redirect::None => {
                return Ok(ResponseData {
                    url,
                    status: head.status,
                    status_text: head.status_text,
                    headers: head.headers,
                    body: parsed.body,
                });
            }
            Redirect::Follow { target, to_get } => {
                if to_get {
                    // 301/302/303 把非 GET/HEAD 降级为 GET：清空 body，丢弃与请求体绑定的头，
                    // 避免陈旧的 content-type/length 跟随到新请求。
                    method = String::from("GET");
                    body = None;
                    strip_entity_headers(&mut headers);
                }
                url = target;
            }
            Redirect::Error(message) => return Err(message),
        }
    }

    Err(format!(
        "fetch failed: maximum redirect count ({MAX_REDIRECTS}) exceeded"
    ))
}

/// 一条重定向的判定结果。
enum Redirect {
    /// 不是需要跟随的重定向（非 3xx、或 3xx 无 `Location`）：原样返回该响应。
    None,
    /// 跟随到 `target`；`to_get` 表示需把方法降级为 GET 并丢弃 body。
    Follow { target: String, to_get: bool },
    /// 不可跟随（`Location` 无法解析）：作为网络层失败 reject。
    Error(String),
}

/// 按 WHATWG fetch 判定一个响应是否应跟随，以及跟随后的方法变化。
///
/// 仅 `301/302/303/307/308` 且带 `Location` 才跟随；缺 `Location` 视为终态响应
/// （返回 [`Redirect::None`]）。`Location` 以当前 URL 为基准解析（允许相对）；
/// 解析失败为网络错误。`301/302/303` 把非 GET/HEAD 降级为 GET；`307/308` 原样保留方法与 body。
fn redirect_plan(
    current: &str,
    method: &str,
    status: u16,
    headers: &[(String, String)],
) -> Redirect {
    if !matches!(status, 301..=303 | 307 | 308) {
        return Redirect::None;
    }
    let Some((_, location)) = headers.iter().find(|(name, _)| name == "location") else {
        return Redirect::None;
    };
    let Ok(base) = Url::parse(current) else {
        // current 在发起前已由 split_target 校验为合法 URL；此处仅为防御，回落终态。
        return Redirect::None;
    };
    let mut target = match base.join(location) {
        Ok(url) => url,
        Err(_) => {
            return Redirect::Error(format!(
                "fetch failed: invalid redirect location: {location}"
            ));
        }
    };
    // WHATWG：`Location` 未显式带片段时，继承当前 URL 的片段。
    if !location.contains('#') {
        target.set_fragment(base.fragment());
    }

    let to_get = matches!(status, 301..=303) && method != "GET" && method != "HEAD";
    Redirect::Follow {
        target: target.into(),
        to_get,
    }
}

/// 方法降级为 GET 时，丢弃与请求体绑定的头。
fn strip_entity_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(name, _)| {
        !matches!(
            name.to_ascii_lowercase().as_str(),
            "content-length" | "content-type" | "transfer-encoding"
        )
    });
}

/// 把 URL 拆成（对端地址, Host 头值, origin-form 请求路径）。
///
/// 非 `http` 方案在此拒绝；DNS 解析在本工作线程上同步进行（已在引擎线程之外，
/// 阻塞不影响宿主事件循环），仅取第一个解析结果。
fn split_target(raw: &str) -> Result<(SocketAddr, String, String), String> {
    let parsed = Url::parse(raw).map_err(|error| format!("fetch failed: invalid URL: {error}"))?;
    if parsed.scheme() != "http" {
        // 本层无 TLS：https 与其它协议一律视为网络层失败（fetch reject）。
        return Err(format!(
            "fetch failed: unsupported scheme {:?} (only http is available)",
            parsed.scheme()
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "fetch failed: URL has no host".to_owned())?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| "fetch failed: URL has no port".to_owned())?;

    // Host 头仅在显式端口（非默认 80）时携带端口，符合 RFC 9110 authority 形式。
    let authority = match parsed.port() {
        Some(explicit) if explicit != 80 => format!("{host}:{explicit}"),
        _ => host.to_owned(),
    };

    let mut path = parsed.path().to_owned();
    if let Some(query) = parsed.query() {
        path.push('?');
        path.push_str(query);
    }
    if path.is_empty() {
        path.push('/');
    }

    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("fetch failed: could not resolve {host}: {error}"))?
        .next()
        .ok_or_else(|| format!("fetch failed: could not resolve {host}"))?;

    Ok((addr, authority, path))
}

/// 单连接的请求-响应往返。
///
/// 连接与写出用阻塞的 `std::net::TcpStream`（本机/局域网瞬时完成，远端 SYN 挂起也由
/// 宿主预算兜底），随后切到非阻塞并交给 `reactor::Poll` 驱动读：每次可读字节喂进解析器，
/// 直到 [`Progress::Complete`] 或 EOF。这样报文封帧逻辑全部落在 [`http::ResponseParser`]，
/// socket 层只负责「何时再给一批字节」。
fn roundtrip(
    addr: SocketAddr,
    wire: &[u8],
    body_forbidden: bool,
) -> Result<http::ParsedResponse, String> {
    let mut std_stream = StdTcpStream::connect(addr)
        .map_err(|error| format!("fetch failed: could not connect to {addr}: {error}"))?;
    std_stream
        .write_all(wire)
        .map_err(|error| format!("fetch failed: could not send request: {error}"))?;

    // 转非阻塞后交给 mio 包装：读以 Poll 驱动。
    std_stream
        .set_nonblocking(true)
        .map_err(|error| format!("fetch failed: could not set nonblocking: {error}"))?;
    let mut stream = TcpStream::from_std(std_stream);

    let mut poll =
        Poll::new().map_err(|error| format!("fetch failed: could not init poll: {error}"))?;
    poll.registry()
        .register(&mut stream, Token(0), Interest::READABLE)
        .map_err(|error| format!("fetch failed: could not register socket: {error}"))?;

    let mut parser = http::ResponseParser::new(body_forbidden);
    let mut events = Events::with_capacity(1);
    let mut buffer = [0u8; 8 * 1024];

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                // EOF：只有 close 封帧（或结束块已到）才正常收尾，否则解析器报截断。
                parser.end_of_input()?;
                break;
            }
            Ok(read) => match parser.feed(&buffer[..read])? {
                Progress::Complete => break,
                Progress::NeedMore => {}
            },
            Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                // 内核暂无数据：阻塞至下一次可读（无超时，整体时限交给宿主预算）。
                poll.poll(&mut events, None)
                    .map_err(|error| format!("fetch failed: poll error: {error}"))?;
            }
            Err(error) => {
                return Err(format!("fetch failed: read error: {error}"));
            }
        }
    }

    parser.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ResponseData {
        ResponseData {
            url: String::from("http://example.com/"),
            status: 200,
            status_text: String::from("OK"),
            headers: vec![
                (String::from("content-type"), String::from("text/plain")),
                (String::from("x-mio"), String::from("ok, again")),
            ],
            body: b"hello mio".to_vec(),
        }
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let data = sample();
        assert_eq!(data.header("X-MIO"), Some("ok, again"));
        assert_eq!(data.header("content-TYPE"), Some("text/plain"));
        assert_eq!(data.header("missing"), None);
        assert!(data.has_header("x-mio"));
    }

    #[test]
    fn ok_covers_only_two_x() {
        let mut data = sample();
        assert!(data.is_ok());
        data.status = 301;
        assert!(!data.is_ok());
        data.status = 404;
        assert!(!data.is_ok());
    }

    #[test]
    fn body_text_is_lossy_utf8() {
        let mut data = sample();
        data.body = vec![0xFF, b'a'];
        assert_eq!(data.body_text(), "\u{FFFD}a");
    }

    #[test]
    fn invalid_url_produces_err_without_network() {
        let outcome = fetch_execute(&FetchRequest::new(String::from("not-a-url")));
        assert!(outcome.is_err(), "非法 URL 必须走失败路径");
    }

    #[test]
    fn get_request_defaults() {
        let request = FetchRequest::new(String::from("http://127.0.0.1:1/"));
        assert_eq!(request.method, "GET");
        assert!(request.headers.is_empty());
        assert!(request.body.is_none());
    }

    fn loc(headers: &[(&str, &str)]) -> Vec<(String, String)> {
        headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn redirect_resolves_relative_location() {
        let plan = redirect_plan(
            "http://h:8080/a/b?x=1",
            "GET",
            302,
            &loc(&[("location", "/final")]),
        );
        match plan {
            Redirect::Follow { target, to_get } => {
                assert_eq!(target, "http://h:8080/final");
                assert!(!to_get, "GET 无需降级");
            }
            _ => panic!("302 带 Location 应跟随"),
        }
    }

    #[test]
    fn redirect_303_downgrades_post_to_get() {
        let plan = redirect_plan(
            "http://h/submit",
            "POST",
            303,
            &loc(&[("location", "/result")]),
        );
        match plan {
            Redirect::Follow { target, to_get } => {
                assert!(to_get, "303 把 POST 降为 GET");
                assert_eq!(target, "http://h/result");
            }
            _ => panic!("应跟随"),
        }
    }

    #[test]
    fn redirect_308_preserves_method_and_body() {
        let plan = redirect_plan("http://h/a", "POST", 308, &loc(&[("location", "/b")]));
        match plan {
            Redirect::Follow { to_get, .. } => assert!(!to_get, "308 保留方法与 body"),
            _ => panic!("应跟随"),
        }
    }

    #[test]
    fn redirect_without_location_is_terminal() {
        // 缺 Location 的 301 是终态响应，不跟随也不报错。
        assert!(matches!(
            redirect_plan("http://h/a", "GET", 301, &loc(&[])),
            Redirect::None
        ));
    }

    #[test]
    fn non_redirect_status_is_not_followed() {
        assert!(matches!(
            redirect_plan("http://h/a", "GET", 200, &loc(&[("location", "/b")])),
            Redirect::None
        ));
    }

    #[test]
    fn unparseable_location_is_error() {
        // `http://` 无主机 → join 失败 → 网络层错误。
        assert!(matches!(
            redirect_plan("http://h/a", "GET", 302, &loc(&[("location", "http://")])),
            Redirect::Error(_)
        ));
    }

    #[test]
    fn redirect_inherits_current_fragment_when_location_has_none() {
        let plan = redirect_plan(
            "http://h/a#section",
            "GET",
            302,
            &loc(&[("location", "/b")]),
        );
        match plan {
            Redirect::Follow { target, .. } => assert_eq!(target, "http://h/b#section"),
            _ => panic!("应跟随"),
        }
    }

    #[test]
    fn strip_entity_headers_drops_body_bound_headers() {
        let mut headers = loc(&[
            ("content-type", "application/json"),
            ("content-length", "12"),
            ("x-test", "keep"),
        ]);
        strip_entity_headers(&mut headers);
        let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, vec!["x-test"]);
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[test]
    fn request_defaults_to_get_without_body() {
        let request = FetchRequest::new("http://127.0.0.1/x".to_string());
        assert_eq!(request.method, "GET");
        assert!(request.headers.is_empty());
        assert!(request.body.is_none());
    }
}
