//! WHATWG `fetch` 的纯数据层：请求执行与响应模型。
//!
//! 本文件不出现任何引擎类型（`boa_*` 仅限 `src/engine/`）：HTTP 报文编解码由
//! [`crate::web::net::http`] 承担，socket 搬运由 `reactor`（mio）的非阻塞读驱动，
//! 引擎绑定层把 [`ResponseData`] 翻译为 JS `Response` 对象。
//! 线程模型：每个请求在一个工作线程上完成，结果经 channel 送回宿主事件循环。
//!
//! 当前范围：任意 method + 请求头 + 字节 body；4xx/5xx 是**成功结果**
//! （fetch 语义只在网络/协议层失败时才 reject）。
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
    /// 发起请求的 URL（重定向后语义暂不区分，等于入参）。
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

/// 执行一次请求：解析 URL、建连、发送 [`http::serialize_request`] 的报文、
/// 用 `reactor` 的非阻塞读驱动 [`http::ResponseParser`] 直到响应完整。
///
/// 不抛出：一切失败（非法 URL、非 http、DNS、连接、协议、截断）都归一为
/// [`FetchOutcome`] 的 `Err` 消息，由引擎层包装成 `TypeError` 拒绝 Promise。
pub fn fetch_execute(request: &FetchRequest) -> FetchOutcome {
    let (addr, authority, path) = split_target(&request.url)?;
    let wire = http::serialize_request(
        &request.method,
        &path,
        &authority,
        &request.headers,
        request.body.as_deref(),
    );
    let body_forbidden = request.method == "HEAD";

    let parsed = roundtrip(addr, &wire, body_forbidden)?;
    let head = parsed.head;
    Ok(ResponseData {
        url: request.url.clone(),
        status: head.status,
        status_text: head.status_text,
        headers: head.headers,
        body: parsed.body,
    })
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
