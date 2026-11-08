//! 内置手册服务：`mio docs [host:port]`。
//!
//! 手册由 VitePress 构建为静态文件（`site/dist/`），编译期整目录嵌入二进制。
//! 运行时不读磁盘、不依赖 Node.js，离线可用。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use include_dir::{Dir, include_dir};

/// 编译期嵌入的手册站点。
static SITE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/site/dist");

/// 默认监听地址：只绑回环，避免默认把手册暴露到局域网。
const DEFAULT_ADDR: &str = "127.0.0.1:4173";

/// 启动手册服务；`addr` 形如 `127.0.0.1:4173`，缺省使用回环地址。
pub fn serve(addr: Option<&str>) -> Result<(), String> {
    let addr = addr.unwrap_or(DEFAULT_ADDR);
    let listener = TcpListener::bind(addr).map_err(|error| format!("无法监听 {addr}：{error}"))?;
    let local = listener
        .local_addr()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| addr.to_string());
    println!("mio 手册：http://{local}/");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                std::thread::spawn(move || {
                    let _ = handle(stream);
                });
            }
            Err(error) => eprintln!("mio docs: 接受连接失败：{error}"),
        }
    }
    Ok(())
}

/// 处理一个连接：读请求行与头，按路径回静态文件。
fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    // 丢弃请求头直到空行；本服务不读请求体。
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    if !matches!(method, "GET" | "HEAD") {
        return write_response(
            &mut stream,
            405,
            "text/plain; charset=utf-8",
            b"method not allowed",
            method == "HEAD",
        );
    }

    match resolve(target) {
        Some((file, status)) => write_response(
            &mut stream,
            status,
            mime_of(file.path().to_string_lossy().as_ref()),
            file.contents(),
            method == "HEAD",
        ),
        None => write_response(
            &mut stream,
            404,
            "text/html; charset=utf-8",
            not_found_page().as_bytes(),
            method == "HEAD",
        ),
    }
}

/// 把请求路径映射到嵌入文件；`/guide` 这类干净链接回退到 `guide.html`。
fn resolve(target: &str) -> Option<(&'static include_dir::File<'static>, u16)> {
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_decode(path);
    let relative = decoded.trim_start_matches('/');
    let candidates = if relative.is_empty() {
        vec![String::from("index.html")]
    } else if relative
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains('.'))
    {
        vec![relative.to_string()]
    } else {
        vec![format!("{relative}.html"), format!("{relative}/index.html")]
    };

    for candidate in candidates {
        if let Some(file) = SITE.get_file(&candidate) {
            return Some((file, 200));
        }
    }
    // SPA/干净链接的兜底：给出内置 404 页面。
    SITE.get_file("404.html").map(|file| (file, 404))
}

/// 极简百分号解码；非法序列原样保留。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(value) = hex.and_then(|text| u8::from_str_radix(text, 16).ok()) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 内置 404 页面：不依赖站点资源，断链时也能给出可用提示。
fn not_found_page() -> &'static str {
    r#"<!doctype html><meta charset="utf-8"><title>404 / MIO</title>
<style>body{margin:0;background:#f1e7d3;color:#111;font:16px/1.6 monospace;display:grid;place-items:center;height:100vh}main{border:3px solid #111;padding:40px 48px;box-shadow:8px 8px 0 #111;background:#f1e7d3}h1{margin:0 0 12px;font-size:64px;letter-spacing:-.06em;color:#d7261e}a{color:#1c4fd8}</style>
<main><h1>404</h1><p>这里没有页面。</p><p><a href="/">返回手册首页</a></p></main>"#
}

/// 按扩展名给出 Content-Type。
fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// 写一个完整的 HTTP 响应并关闭连接。
fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if !head_only {
        stream.write_all(body)?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_clean_urls_to_html() {
        assert_eq!(
            resolve("/").map(|(file, _)| file.path().to_string_lossy().into_owned()),
            Some(String::from("index.html"))
        );
        assert_eq!(
            resolve("/guide").map(|(file, _)| file.path().to_string_lossy().into_owned()),
            Some(String::from("guide.html"))
        );
        assert_eq!(
            resolve("/no-such-page").map(|(_, status)| status),
            Some(404)
        );
    }

    #[test]
    fn percent_decodes_paths() {
        assert_eq!(percent_decode("/a%20b"), "/a b");
        assert_eq!(percent_decode("/%E4%B8%AD"), "/中");
        assert_eq!(percent_decode("/bad%zz"), "/bad%zz");
    }

    #[test]
    fn mime_lookup_covers_site_assets() {
        assert_eq!(mime_of("index.html"), "text/html; charset=utf-8");
        assert_eq!(mime_of("a/b.css"), "text/css; charset=utf-8");
        assert_eq!(mime_of("x.woff2"), "font/woff2");
        assert_eq!(mime_of("unknown.bin"), "application/octet-stream");
    }
}
