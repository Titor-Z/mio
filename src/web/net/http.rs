//! HTTP/1.1 报文的纯数据层：请求序列化与响应增量解析。
//!
//! 不含 socket、不含引擎类型：输入是字节切片，输出是「还要更多字节」或「已完整」。
//! 于是分块传输、无响应体（HEAD/204/304）、`connection: close` 封帧这些容易写错的
//! 分支都能用纯单测覆盖，而不必真起一个服务器。
//!
//! 解析器每前进一步就把已消费的字节从缓冲区前面丢掉，因此任一时刻缓冲区里只剩
//! 未消费的尾部；响应体单独累积，上层按完成度决定何时停止读取。

/// 响应头块结束标记（空行）。
const HEAD_TERMINATOR: &[u8] = b"\r\n\r\n";
/// 容忍只发裸 LF 的服务端。
const HEAD_TERMINATOR_LF: &[u8] = b"\n\n";

/// 解析进度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// 需要更多字节（或需要 EOF 信号）。
    NeedMore,
    /// 响应已完整，上层可以关闭连接。
    Complete,
}

/// 已解析的状态行与响应头。
#[derive(Debug, Clone)]
pub struct ResponseHead {
    /// 状态码。
    pub status: u16,
    /// 原因短语。
    pub status_text: String,
    /// 响应头：键已小写，同名多值以 `, ` 合并（`Headers.get` 的组合值语义）。
    pub headers: Vec<(String, String)>,
}

impl ResponseHead {
    /// 大小写不敏感取头值（键已在解析时小写化）。
    pub fn header(&self, name: &str) -> Option<&str> {
        let wanted = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == wanted)
            .map(|(_, value)| value.as_str())
    }
}

/// 解析出的完整响应：头部 + 已缓冲的体。
#[derive(Debug, Clone)]
pub struct ParsedResponse {
    /// 状态行与头。
    pub head: ResponseHead,
    /// 响应体字节。
    pub body: Vec<u8>,
}

/// 响应体的封帧方式。
#[derive(Debug, Clone, Copy)]
enum State {
    /// 还在收状态行与头。
    Head,
    /// 定长体，还差 `remaining` 字节。
    Length { remaining: usize },
    /// 分块传输。
    Chunked(ChunkedState),
    /// 以连接关闭作为体结束标志。
    UntilEof,
    /// 已完整。
    Complete,
}

#[derive(Debug, Clone, Copy)]
enum ChunkedState {
    /// 待读 chunk 大小行。
    Size,
    /// 正在收 chunk 数据。
    Data { remaining: usize },
    /// chunk 数据后的 CRLF。
    DataCrlf,
    /// 结束块之后的 Trailer 区。
    Trailer,
}

/// 增量响应解析器。
///
/// `body_forbidden` 来自请求方法：HEAD 的响应恒无体，即使声明了 `content-length`
/// ——把 HEAD 当 GET 读是这类实现最常见的错误（旧实现正因此挂掉）。
#[derive(Debug)]
pub struct ResponseParser {
    body_forbidden: bool,
    state: State,
    buffer: Vec<u8>,
    head: Option<ResponseHead>,
    body: Vec<u8>,
}

impl ResponseParser {
    /// 新建解析器；`body_forbidden` 对 HEAD 传 `true`。
    pub fn new(body_forbidden: bool) -> Self {
        Self {
            body_forbidden,
            state: State::Head,
            buffer: Vec::new(),
            head: None,
            body: Vec::new(),
        }
    }

    /// 喂入收到的字节。协议错误以 `Err` 消息表达。
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Progress, String> {
        self.buffer.extend_from_slice(bytes);
        self.consume()
    }

    /// 通知 EOF。只有 `close` 封帧（或已结束块已到）才算正常收尾。
    pub fn end_of_input(&mut self) -> Result<Progress, String> {
        match self.state {
            State::Complete => Ok(Progress::Complete),
            State::UntilEof => {
                self.state = State::Complete;
                Ok(Progress::Complete)
            }
            // 服务端常常在结束块后直接关连接，不再补齐空行。
            State::Chunked(ChunkedState::Trailer) => {
                self.state = State::Complete;
                Ok(Progress::Complete)
            }
            State::Head => Err("连接在收到完整响应头之前关闭".to_owned()),
            State::Length { remaining } => Err(format!(
                "响应体被截断：content-length 还差 {remaining} 字节"
            )),
            State::Chunked(_) => Err("响应体被截断：分块传输未收到结束块".to_owned()),
        }
    }

    /// 取出解析结果；未完成时返回错误而不是半成品。
    pub fn into_response(self) -> Result<ParsedResponse, String> {
        if !matches!(self.state, State::Complete) {
            return Err("响应尚未解析完成".to_owned());
        }
        let head = self.head.ok_or_else(|| "响应尚未解析完成".to_owned())?;
        Ok(ParsedResponse {
            head,
            body: self.body,
        })
    }

    /// 推进状态机，直到需要更多字节、完成或出错。每个分支要么消费字节，要么返回。
    fn consume(&mut self) -> Result<Progress, String> {
        loop {
            match self.state {
                State::Head => match self.take_head()? {
                    Progress::NeedMore => return Ok(Progress::NeedMore),
                    Progress::Complete => continue,
                },
                State::Length { remaining } => {
                    let take = remaining.min(self.buffer.len());
                    self.body.extend_from_slice(&self.buffer[..take]);
                    self.buffer.drain(..take);
                    if remaining == take {
                        self.state = State::Complete;
                        return Ok(Progress::Complete);
                    }
                    self.state = State::Length {
                        remaining: remaining - take,
                    };
                    return Ok(Progress::NeedMore);
                }
                State::UntilEof => {
                    self.body.append(&mut self.buffer);
                    return Ok(Progress::NeedMore);
                }
                State::Chunked(chunked) => match self.take_chunk(chunked)? {
                    Progress::NeedMore => return Ok(Progress::NeedMore),
                    Progress::Complete => continue,
                },
                State::Complete => return Ok(Progress::Complete),
            }
        }
    }

    /// 切出一个响应头块。信息性响应（1xx）被跳过，继续等真正的响应。
    fn take_head(&mut self) -> Result<Progress, String> {
        let Some((end, terminator)) = find_head_end(&self.buffer) else {
            return Ok(Progress::NeedMore);
        };
        let block = self.buffer[..end + terminator].to_vec();
        self.buffer.drain(..end + terminator);

        let head = parse_head_block(&block)?;
        // RFC 9112：1xx 是不带体的独立消息，其后才是真正的响应。
        if (100..200).contains(&head.status) {
            self.state = State::Head;
            return Ok(Progress::Complete);
        }
        self.state = decide_framing(self.body_forbidden, &head)?;
        self.head = Some(head);
        Ok(Progress::Complete)
    }

    /// 分块传输推进一步。
    fn take_chunk(&mut self, state: ChunkedState) -> Result<Progress, String> {
        match state {
            ChunkedState::Size => {
                let Some(line) = take_line(&mut self.buffer) else {
                    return Ok(Progress::NeedMore);
                };
                let size_text = line.split(';').next().unwrap_or("").trim();
                let size = usize::from_str_radix(size_text, 16)
                    .map_err(|_| format!("分块大小非法：{line:?}"))?;
                self.state = State::Chunked(if size == 0 {
                    ChunkedState::Trailer
                } else {
                    ChunkedState::Data { remaining: size }
                });
                Ok(Progress::Complete)
            }
            ChunkedState::Data { remaining } => {
                let take = remaining.min(self.buffer.len());
                self.body.extend_from_slice(&self.buffer[..take]);
                self.buffer.drain(..take);
                if remaining == take {
                    self.state = State::Chunked(ChunkedState::DataCrlf);
                    return Ok(Progress::Complete);
                }
                self.state = State::Chunked(ChunkedState::Data {
                    remaining: remaining - take,
                });
                Ok(Progress::NeedMore)
            }
            ChunkedState::DataCrlf => {
                // chunk 数据后必须跟 CRLF（同样容忍裸 LF）。
                match self.buffer.first() {
                    Some(b'\r') if self.buffer.len() >= 2 => {
                        self.buffer.drain(..2);
                    }
                    Some(b'\n') => {
                        self.buffer.drain(..1);
                    }
                    _ => return Ok(Progress::NeedMore),
                }
                self.state = State::Chunked(ChunkedState::Size);
                Ok(Progress::Complete)
            }
            ChunkedState::Trailer => {
                // Trailer 字段本版本忽略（不并入 Headers），只需走到空行。
                loop {
                    let Some(line) = take_line(&mut self.buffer) else {
                        return Ok(Progress::NeedMore);
                    };
                    if line.is_empty() {
                        self.state = State::Complete;
                        return Ok(Progress::Complete);
                    }
                }
            }
        }
    }
}

/// 找出头块结束位置，返回（终止符起点, 终止符长度）；取先出现的那个。
fn find_head_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let crlf = find_sub(buffer, HEAD_TERMINATOR);
    let lf = find_sub(buffer, HEAD_TERMINATOR_LF);
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(if a <= b { (a, 4) } else { (b, 2) }),
        (Some(a), None) => Some((a, 4)),
        (None, Some(b)) => Some((b, 2)),
        (None, None) => None,
    }
}

/// 朴素子串查找：头块体量小，不值得引入搜索算法依赖。
fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 取出以 CRLF/LF 结尾的一行（不含结尾）；行还不全时返回 [`None`]。
fn take_line(buffer: &mut Vec<u8>) -> Option<String> {
    if let Some(index) = find_sub(buffer, b"\r\n") {
        let line = String::from_utf8_lossy(&buffer[..index]).into_owned();
        buffer.drain(..index + 2);
        return Some(line);
    }
    let index = find_sub(buffer, b"\n")?;
    let line = String::from_utf8_lossy(&buffer[..index]).into_owned();
    buffer.drain(..index + 1);
    Some(line)
}

/// 解析一个完整头块（含结尾空行）。
fn parse_head_block(block: &[u8]) -> Result<ResponseHead, String> {
    let text = String::from_utf8_lossy(block).into_owned();
    let mut lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| !line.is_empty());

    let status_line = lines.next().ok_or_else(|| "响应缺少状态行".to_owned())?;
    if !status_line.starts_with("HTTP/1.") {
        return Err(format!("响应不是 HTTP/1.x：{status_line:?}"));
    }
    let mut parts = status_line.splitn(3, ' ');
    let _ = parts.next();
    let status: u16 = parts
        .next()
        .and_then(|raw| raw.parse().ok())
        .ok_or_else(|| format!("状态码非法：{status_line:?}"))?;
    let status_text = parts.next().unwrap_or("").to_owned();

    // 同名头合并成组合值，顺序按首次出现；obs-fold（空白开头的续行）并入上一条。
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, value)) = headers.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!("响应头行非法：{line:?}"));
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        match headers.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, existing)) => {
                existing.push_str(", ");
                existing.push_str(&value);
            }
            None => headers.push((name, value)),
        }
    }

    Ok(ResponseHead {
        status,
        status_text,
        headers,
    })
}

/// 依据请求方法与响应状态码定下体的封帧方式。
fn decide_framing(body_forbidden: bool, head: &ResponseHead) -> Result<State, String> {
    // RFC 9112 §6.3：204/304 恒无体；HEAD 同理，即便声明了 content-length。
    if body_forbidden || head.status == 204 || head.status == 304 {
        return Ok(State::Complete);
    }
    let transfer_encoding = head
        .header("transfer-encoding")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if transfer_encoding
        .split(',')
        .any(|token| token.trim() == "chunked")
    {
        return Ok(State::Chunked(ChunkedState::Size));
    }
    let Some(value) = head.header("content-length") else {
        return Ok(State::UntilEof);
    };
    // 多个 content-length 声明本版按第一个取值；不一致属于协议违例，留待 M3 严格化。
    let first = value.split(',').next().unwrap_or(value).trim();
    let length: usize = first
        .parse()
        .map_err(|_| format!("content-length 非法：{value:?}"))?;
    if length == 0 {
        return Ok(State::Complete);
    }
    Ok(State::Length { remaining: length })
}

/// 序列化请求报文：请求行 + 头 + 空行 + 体。
///
/// `path` 为 origin-form（以 `/` 开头的路径加查询串），`authority` 用于 Host 头。
/// 调用方头里若已含 `host` / `content-length` / `connection`，以用户值为准。
#[must_use]
pub fn serialize_request(
    method: &str,
    path: &str,
    authority: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> Vec<u8> {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let mut wire = format!("{method} {path} HTTP/1.1\r\n");
    let mut gave_host = false;
    let mut gave_length = false;
    let mut gave_connection = false;
    for (name, value) in headers {
        // 所有头都原样写出；标志仅用于决定要不要补默认值（缺失时）。
        match name.to_ascii_lowercase().as_str() {
            "host" => gave_host = true,
            "content-length" => gave_length = true,
            "connection" => gave_connection = true,
            _ => {}
        }
        wire.push_str(&format!("{name}: {value}\r\n"));
    }
    if !gave_host {
        wire.push_str(&format!("host: {authority}\r\n"));
    }
    if let Some(bytes) = body
        && !gave_length
    {
        wire.push_str(&format!("content-length: {}\r\n", bytes.len()));
    }
    if !gave_connection {
        // M1 不做连接复用：读完即关，封帧歧义最少。
        wire.push_str("connection: close\r\n");
    }
    wire.push_str("\r\n");
    let mut message = wire.into_bytes();
    if let Some(bytes) = body {
        message.extend_from_slice(bytes);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_block(status: &str, headers: &[(&str, &str)]) -> String {
        let mut text = format!("HTTP/1.1 {status}\r\n");
        for (name, value) in headers {
            text.push_str(&format!("{name}: {value}\r\n"));
        }
        text.push_str("\r\n");
        text
    }

    #[test]
    fn parses_content_length_body_split_across_reads() {
        let mut parser = ResponseParser::new(false);
        let head = head_block("200 OK", &[("content-length", "5")]);
        assert_eq!(parser.feed(head.as_bytes()).unwrap(), Progress::NeedMore);
        assert_eq!(parser.feed(b"he").unwrap(), Progress::NeedMore);
        assert_eq!(parser.feed(b"llo").unwrap(), Progress::Complete);
        let response = parser.into_response().unwrap();
        assert_eq!(response.head.status, 200);
        assert_eq!(response.body, b"hello");
    }

    #[test]
    fn empty_read_does_not_spin_the_state_machine() {
        let mut parser = ResponseParser::new(false);
        let head = head_block("200 OK", &[("content-length", "5")]);
        parser.feed(head.as_bytes()).unwrap();
        parser.feed(b"he").unwrap();
        for _ in 0..3 {
            assert_eq!(parser.feed(b"").unwrap(), Progress::NeedMore);
        }
    }

    #[test]
    fn head_request_never_reads_a_body_even_with_content_length() {
        let mut parser = ResponseParser::new(true);
        let head = head_block("200 OK", &[("content-length", "123")]);
        assert_eq!(
            parser.feed(head.as_bytes()).unwrap(),
            Progress::Complete,
            "HEAD 声明了长度却没有体，按长度等会一直读到连接关闭才报错"
        );
        assert!(parser.into_response().unwrap().body.is_empty());
    }

    #[test]
    fn no_content_and_not_modified_have_no_body() {
        for status in ["204 No Content", "304 Not Modified"] {
            let mut parser = ResponseParser::new(false);
            let head = head_block(status, &[("content-length", "10")]);
            assert_eq!(parser.feed(head.as_bytes()).unwrap(), Progress::Complete);
            assert!(
                parser.into_response().unwrap().body.is_empty(),
                "{status} 应无体"
            );
        }
    }

    #[test]
    fn decodes_chunked_with_extensions_and_trailers() {
        let mut parser = ResponseParser::new(false);
        let head = head_block("200 OK", &[("transfer-encoding", "chunked")]);
        assert_eq!(parser.feed(head.as_bytes()).unwrap(), Progress::NeedMore);
        assert_eq!(
            parser.feed(b"4;ext=1\r\nwiki\r\n").unwrap(),
            Progress::NeedMore
        );
        assert_eq!(parser.feed(b"5\r\npedia").unwrap(), Progress::NeedMore);
        assert_eq!(parser.feed(b"\r\n").unwrap(), Progress::NeedMore);
        assert_eq!(
            parser.feed(b"0\r\nx-trace: abc\r\n\r\n").unwrap(),
            Progress::Complete
        );
        assert_eq!(parser.into_response().unwrap().body, b"wikipedia");
    }

    #[test]
    fn chunked_body_split_mid_chunk_and_final_block_without_trailers() {
        let mut parser = ResponseParser::new(false);
        let head = head_block("200 OK", &[("transfer-encoding", "chunked")]);
        parser.feed(head.as_bytes()).unwrap();
        assert_eq!(parser.feed(b"6\r\nab").unwrap(), Progress::NeedMore);
        assert_eq!(parser.feed(b"cdef").unwrap(), Progress::NeedMore);
        assert_eq!(parser.feed(b"\r\n0\r\n\r\n").unwrap(), Progress::Complete);
        assert_eq!(parser.into_response().unwrap().body, b"abcdef");
    }

    #[test]
    fn chunked_stream_ending_right_after_the_last_chunk_is_not_truncated() {
        let mut parser = ResponseParser::new(false);
        let head = head_block("200 OK", &[("transfer-encoding", "chunked")]);
        parser.feed(head.as_bytes()).unwrap();
        parser.feed(b"2\r\nhi\r\n0\r\n").unwrap();
        assert_eq!(
            parser.end_of_input().unwrap(),
            Progress::Complete,
            "服务端常在结束块后直接关连接"
        );
    }

    #[test]
    fn combines_duplicate_headers_and_reads_case_insensitively() {
        let mut parser = ResponseParser::new(false);
        let head = head_block(
            "200 OK",
            &[("X-MIO", "ok"), ("x-mio", "again"), ("content-length", "0")],
        );
        parser.feed(head.as_bytes()).unwrap();
        let response = parser.into_response().unwrap();
        assert_eq!(response.head.header("X-Mio").unwrap(), "ok, again");
        assert_eq!(response.head.status_text, "OK");
    }

    #[test]
    fn skips_informational_response_before_the_real_one() {
        let mut parser = ResponseParser::new(false);
        let mut wire = head_block("100 Continue", &[]);
        wire.push_str(&head_block("200 OK", &[("content-length", "2")]));
        assert_eq!(parser.feed(wire.as_bytes()).unwrap(), Progress::NeedMore);
        assert_eq!(parser.feed(b"hi").unwrap(), Progress::Complete);
        let response = parser.into_response().unwrap();
        assert_eq!(response.head.status, 200);
        assert_eq!(response.body, b"hi");
    }

    #[test]
    fn missing_length_falls_back_to_close_delimited_body() {
        let mut parser = ResponseParser::new(false);
        let head = head_block("200 OK", &[("content-type", "text/plain")]);
        assert_eq!(parser.feed(head.as_bytes()).unwrap(), Progress::NeedMore);
        assert_eq!(parser.feed(b"tail").unwrap(), Progress::NeedMore);
        assert_eq!(parser.end_of_input().unwrap(), Progress::Complete);
        assert_eq!(parser.into_response().unwrap().body, b"tail");
    }

    #[test]
    fn truncated_declared_body_reports_how_much_was_missing() {
        let mut parser = ResponseParser::new(false);
        let head = head_block("200 OK", &[("content-length", "10")]);
        parser.feed(head.as_bytes()).unwrap();
        parser.feed(b"123").unwrap();
        let error = parser.end_of_input().unwrap_err();
        assert!(error.contains("截断"), "{error}");
        assert!(error.contains('7'), "应说明还差几字节：{error}");
    }

    #[test]
    fn rejects_foreign_version_and_bad_status() {
        let mut parser = ResponseParser::new(false);
        assert!(
            parser
                .feed(b"RTSP/1.0 200 OK\r\ncontent-length: 0\r\n\r\n")
                .is_err()
        );
        let mut another = ResponseParser::new(false);
        assert!(
            another
                .feed(b"HTTP/1.1 abc OK\r\ncontent-length: 0\r\n\r\n")
                .is_err()
        );
    }

    #[test]
    fn accepts_bare_lf_line_endings() {
        let mut parser = ResponseParser::new(false);
        let block = "HTTP/1.1 200 OK\ncontent-length: 2\n\nhi";
        assert_eq!(parser.feed(block.as_bytes()).unwrap(), Progress::Complete);
        assert_eq!(parser.into_response().unwrap().body, b"hi");
    }

    #[test]
    fn serializes_request_line_and_managed_headers() {
        let wire = String::from_utf8(serialize_request(
            "POST",
            "/upload?q=1",
            "127.0.0.1:8080",
            &[
                (String::from("content-type"), String::from("text/plain")),
                (String::from("connection"), String::from("keep-alive")),
            ],
            Some(b"body"),
        ))
        .unwrap();
        assert!(wire.starts_with("POST /upload?q=1 HTTP/1.1\r\n"), "{wire}");
        assert!(wire.contains("content-type: text/plain\r\n"));
        assert!(wire.contains("host: 127.0.0.1:8080\r\n"));
        assert!(wire.contains("content-length: 4\r\n"));
        // 用户给出的 connection 优先，本层不再追加。
        assert!(wire.contains("connection: keep-alive\r\n"));
        assert!(!wire.contains("connection: close"));
        assert!(wire.ends_with("\r\nbody"));
    }

    #[test]
    fn defaults_to_connection_close_and_leading_slash() {
        let wire =
            String::from_utf8(serialize_request("GET", "hello", "example.com", &[], None)).unwrap();
        assert!(
            wire.starts_with("GET /hello HTTP/1.1\r\n"),
            "origin-form 必须以 / 开头：{wire}"
        );
        assert!(wire.contains("connection: close\r\n"));
        assert!(!wire.contains("content-length"), "无体请求不声明长度");
    }
}
