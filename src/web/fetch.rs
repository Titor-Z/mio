//! WHATWG `fetch` 的纯数据层：请求执行与响应模型。
//!
//! 本文件不出现任何引擎类型（`boa_*` 仅限 `src/engine/`）：HTTP 由 `ureq` 阻塞执行，
//! 引擎绑定层把 [`ResponseData`] 翻译为 JS `Response` 对象。
//! 线程模型：每个请求在一个工作线程上阻塞完成，结果经 channel 送回宿主事件循环。
//!
//! 当前范围：仅 GET；4xx/5xx 是**成功结果**（fetch 语义只在网络/协议层失败时才 reject）。

use std::io::Read;

/// 一次 fetch 的终局：成功携带 [`ResponseData`]，失败携带应包装为 `TypeError` 的消息。
pub type FetchOutcome = Result<ResponseData, String>;

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

/// 执行一次阻塞 GET。不抛出：一切失败都归一为 [`FetchOutcome`] 的 `Err` 消息。
pub fn fetch_get(url: &str) -> FetchOutcome {
    match ureq::get(url).call() {
        Ok(response) => read_response(url, response),
        Err(error) => {
            let message = error.to_string();
            match error.into_response() {
                // Status 错误同样携带完整响应：按 fetch 语义视为成功。
                Some(response) => read_response(url, response),
                None => Err(format!("fetch failed: {message}")),
            }
        }
    }
}

/// 从 ureq 响应提取快照。头名小写化并按 `Headers` 的组合值语义合并。
fn read_response(url: &str, response: ureq::Response) -> FetchOutcome {
    let status = response.status();
    let status_text = response.status_text().to_string();

    let mut headers: Vec<(String, String)> = Vec::new();
    for name in response.headers_names() {
        let values = response.all(&name);
        headers.push((name.to_ascii_lowercase(), values.join(", ")));
    }

    let mut body = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body)
        .map_err(|error| format!("fetch failed reading body: {error}"))?;

    Ok(ResponseData {
        url: url.to_string(),
        status,
        status_text,
        headers,
        body,
    })
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
        let outcome = fetch_get("not-a-url");
        assert!(outcome.is_err(), "非法 URL 必须走失败路径");
    }
}
