//! HTTP/1.1 服务端的纯数据/IO 层：接收连接、解析请求、写出响应。
//!
//! 不含引擎类型（`boa_*` 只在 `src/engine/`）：本层只认 [`crate::web::net::http`] 的
//! [`http::ParsedRequest`] 与 [`ResponseSpec`]，处理器以闭包注入。引擎绑定层把 JS 的
//! `serve()` handler 适配成这个闭包，并把跨线程往返桥接过来。
//!
//! M1 范围：每连接一个线程并发处理（accept 后立即移交），并支持 keep-alive（同一
//! 连接顺序处理多个请求，直到对端关闭 / 空闲超时 / 某响应带 `connection: close`）。
//! 流式 body（ReadableStream）与分块回应仍是后续项。

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

use super::http::{self, Progress, RequestParser};

/// keep-alive 空闲读超时：一条连接处理完一个请求后等下一个，至多这么久，
/// 到点即静默关闭，不无限占用连接线程（慢客户端 / 半开连接的兜底）。
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// 服务端要写出的响应：状态、原因短语、头、体。引擎层从 JS `Response` 拆成此纯数据。
#[derive(Debug, Clone)]
pub struct ResponseSpec {
    /// 状态码。
    pub status: u16,
    /// 原因短语，空则由 [`http::serialize_response`] 留空。
    pub status_text: String,
    /// 响应头（原样键名，写出时大小写保持）。
    pub headers: Vec<(String, String)>,
    /// 响应体。
    pub body: Vec<u8>,
}

impl ResponseSpec {
    /// 构造一个 `text/plain` 文本响应，便于内部使用与测试。
    #[must_use]
    pub fn text(status: u16, text: &str) -> Self {
        Self {
            status,
            status_text: String::new(),
            headers: vec![(
                String::from("content-type"),
                String::from("text/plain; charset=utf-8"),
            )],
            body: text.as_bytes().to_vec(),
        }
    }
}

/// 在一条双向连接上循环处理请求：读一个、交给 `handler`、写出响应，直至对端关闭、
/// 超时、或某个响应声明 `connection: close`。
///
/// 泛型于 `S: Read + Write`：真实 [`std::net::TcpStream`] 与内存流都能喂。读超时（由
/// [`serve_loop`] 给 TcpStream 设置）到期时返回 [`io::ErrorKind::WouldBlock`] /
/// [`io::ErrorKind::TimedOut`]，本函数据此静默收尾。keep-alive 的封帧完全由请求头
/// `Connection` 与版本决定（见 [`request_keep_alive`]）。协议错误转成 [`io::ErrorKind::InvalidData`]。
pub fn handle_connection<S>(
    mut stream: S,
    handler: &mut dyn FnMut(&http::ParsedRequest) -> ResponseSpec,
) -> io::Result<()>
where
    S: Read + Write,
{
    let mut buffer = [0u8; 8 * 1024];
    loop {
        // 每个请求一个全新的解析器：上一请求的字节已全部消费（解析器边收边丢）。
        let mut parser = RequestParser::new();
        let mut fed_any = false;
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => {
                    // 请求间隙对端关闭 → 正常收尾；半路 EOF → 截断错误。
                    if !fed_any {
                        return Ok(());
                    }
                    parser.end_of_input().map_err(invalid)?;
                    return Ok(());
                }
                Ok(read) => match parser.feed(&buffer[..read]).map_err(invalid)? {
                    Progress::Complete => break,
                    Progress::NeedMore => fed_any = true,
                },
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    // 空闲/慢客户端超时：静默关闭该连接。
                    return Ok(());
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }

        let request = parser.into_request().map_err(invalid)?;
        let keep_alive = request_keep_alive(&request);
        let response = handler(&request);
        let wire = http::serialize_response(
            response.status,
            &response.status_text,
            &response.headers,
            Some(&response.body),
            !keep_alive,
        );
        stream.write_all(&wire)?;
        stream.flush()?;
        if !keep_alive {
            return Ok(());
        }
    }
}

/// 按请求判定本连接是否应保持：`Connection` 头优先，否则 HTTP/1.1 默认持久、1.0 默认关。
fn request_keep_alive(request: &http::ParsedRequest) -> bool {
    let connection = request
        .head
        .header("connection")
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut tokens = connection.split(',').map(|token| token.trim());
    if tokens.any(|token| token == "close") {
        return false;
    }
    if request
        .head
        .header("connection")
        .unwrap_or("")
        .to_ascii_lowercase()
        .split(',')
        .any(|token| token.trim() == "keep-alive")
    {
        return true;
    }
    // 无显式指令：仅 HTTP/1.1 默认持久。
    request.head.version == "HTTP/1.1"
}

/// 绑定地址并进入 accept 循环，每连接开一个线程交给 `handler`。
///
/// 返回监听端口（供调用方/测试得知实际端口，绑定 0 时尤其有用）。accept 后立即
/// 移交连接线程、主循环回到 accept，故多个连接并发在途；连接线程上设空闲读超时。
/// `handler` 需 `Clone + Send + 'static` 以便逐连接各拥一份。
pub fn serve_loop<F>(addr: SocketAddr, handler: F) -> io::Result<u16>
where
    F: Fn(&http::ParsedRequest) -> ResponseSpec + Send + Clone + 'static,
{
    let listener = TcpListener::bind(addr)?;
    let port = listener.local_addr()?.port();
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // 空闲超时只在真实 socket 上设；handle_connection 对 WouldBlock/TimedOut 静默收尾。
        let _ = stream.set_read_timeout(Some(IDLE_TIMEOUT));
        let mut handler = handler.clone();
        std::thread::spawn(move || {
            let _ = handle_connection(stream, &mut handler);
        });
    }
    Ok(port)
}

/// 把解析层的字符串错误包装成 IO 错误。
fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpStream;

    /// 起一个回显服务，客户端发一个请求，验证解析 + 响应写出全链路（仅回环地址）。
    ///
    /// 服务端 `connection: close` 写完后关闭连接，客户端 `read_to_string` 拿到完整响应。
    #[test]
    fn echoes_method_and_path_over_real_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = handle_connection(stream, &mut |request| {
                    ResponseSpec::text(
                        200,
                        &format!("{} {}", request.head.method, request.head.target),
                    )
                });
            }
        });

        let mut client = TcpStream::connect(("127.0.0.1", port)).expect("连接");
        client
            .write_all(b"GET /health?q=1 HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
            .unwrap();

        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("content-length: 15\r\n"), "{response}");
        assert!(response.ends_with("\r\n\r\nGET /health?q=1"), "{response}");
    }
}
