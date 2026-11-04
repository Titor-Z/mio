//! 网络 IO 层：与 `web` 的「全局对象」分工不同，这里是被 `fetch` 消费的下层设施。
//!
//! [`http`] 是 HTTP/1.1 报文的纯数据层（请求序列化 + 响应增量解析），不含 socket、
//! 不含引擎类型，因此分块传输、无响应体、`connection: close` 封帧等易错分支可用纯
//! 单测覆盖，而不必真起一个服务器。socket 的搬运（连接、读写、事件循环）在
//! `web::fetch` 里用 `reactor`（mio）驱动，喂给 [`http`] 的解析器。
//!
//! [`server`] 是服务端的对称面：接受连接、用 [`http::RequestParser`] 解析请求、
//! 用 [`http::serialize_response`] 写出响应，处理器以闭包注入，同样不含引擎类型。

pub mod http;
pub mod server;
