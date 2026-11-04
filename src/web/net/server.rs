//! HTTP/1.1 服务端的纯数据/IO 层：接收连接、解析请求、写出响应。
//!
//! 不含引擎类型（`boa_*` 只在 `src/engine/`）：本层只认 [`crate::web::net::http`] 的
//! [`http::ParsedRequest`] 与 [`ResponseSpec`]，处理器以闭包注入。引擎绑定层把 JS 的
//! `serve()` handler 适配成这个闭包，并把跨线程往返桥接过来。
//!
//! M1 范围：串行处理连接（一次一个），每连接读完一个请求即应答并关闭（`connection: close`）。
//! 连接复用（keep-alive）与并发是后续项——阻塞式单线程循环先把协议正确性跑通。

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener};

use super::http::{self, Progress, RequestParser};

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

/// 在一条双向连接上读一个请求、交给 `handler`、写出响应。
///
/// 泛型于 `S: Read + Write`：真实 [`std::net::TcpStream`] 与内存流都能喂。读用阻塞
/// `read`（服务端线程上，语义与 `web::fetch` 的阻塞连接一致）；解析逻辑全落在
/// [`RequestParser`]，本函数只负责「何时再给一批字节、何时写出」。协议错误转成
/// [`io::ErrorKind::InvalidData`]。
pub fn handle_connection<S>(
    mut stream: S,
    handler: &mut dyn FnMut(&http::ParsedRequest) -> ResponseSpec,
) -> io::Result<()>
where
    S: Read + Write,
{
    let mut parser = RequestParser::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                // 客户端提前关闭：请求若不完整，解析器按截断报错；完整则正常收尾。
                parser.end_of_input().map_err(invalid)?;
                break;
            }
            Ok(read) => match parser.feed(&buffer[..read]).map_err(invalid)? {
                Progress::Complete => break,
                Progress::NeedMore => {}
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }

    let request = parser.into_request().map_err(invalid)?;
    let response = handler(&request);
    let wire = http::serialize_response(
        response.status,
        &response.status_text,
        &response.headers,
        Some(&response.body),
    );
    stream.write_all(&wire)?;
    stream.flush()?;
    Ok(())
}

/// 绑定地址并进入串行 accept 循环，逐连接交给 `handler`。
///
/// 返回监听端口（供调用方/测试得知实际端口，绑定 0 时尤其有用）。循环阻塞至
/// `listener.incoming()` 结束；进程退出即回收。
pub fn serve_loop(
    addr: SocketAddr,
    mut handler: impl FnMut(&http::ParsedRequest) -> ResponseSpec,
) -> io::Result<u16> {
    let listener = TcpListener::bind(addr)?;
    let port = listener.local_addr()?.port();
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // 单连接出错不影响后续 accept：吞掉继续。
        let _ = handle_connection(stream, &mut handler);
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
            .write_all(b"GET /health?q=1 HTTP/1.1\r\nhost: x\r\n\r\n")
            .unwrap();

        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("content-length: 15\r\n"), "{response}");
        assert!(response.ends_with("\r\n\r\nGET /health?q=1"), "{response}");
    }
}
