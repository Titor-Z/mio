//! `Headers` 与 `Response` 全局的引擎绑定（WHATWG Fetch 标准面）。
//!
//! 与 `blob`/`url` 同构：实例经内部序号（`_headersId` / `_responseId`）关联线程局部登记表，
//! 不含跨引擎可移植状态。两个类均为**带原型的构造器**（`instanceof` 成立、方法挂原型），
//! 且 `fetch` 产出的 Response 也由本模块同一入口铸造——全局只有一套 Response。
//!
//! `Response.headers` 是**共享同一个头表**的 live 视图：`res.headers.set(...)` 会真的影响
//! `serve` 写线的头（早前是快照拷贝，改了看不到）。
//!
//! v1 边界：`Headers` 不含 `getSetCookie` 特例；`Response` 无流式 body、无 `body` getter、
//! `redirect`/`error` 静态方法未实现；body 仅接受字符串与缓冲类/Blob（无 ReadableStream）。

use std::cell::{Cell, RefCell};

use boa_engine::object::builtins::{JsArray, JsArrayBuffer, JsPromise, JsUint8Array};
use boa_engine::{
    Context, JsArgs, JsNativeError, JsObject, JsResult, JsSymbol, JsValue, js_string,
};

use crate::web::net::server::ResponseSpec;

use super::support::{
    NativeMethod, alias_symbol_member, array_iterator, global_static_function, instantiate,
    prototype_of, register_class, set_property, to_text,
};

/// 登记表中的 Header 集合：保留插入顺序，同名多值合并为 `, ` 组合值。
type HeaderEntries = Vec<(String, String)>;

thread_local! {
    static HEADERS: RefCell<Vec<(usize, HeaderEntries)>> = const { RefCell::new(Vec::new()) };
    static RESPONSES: RefCell<Vec<(usize, StoredResponse)>> = const { RefCell::new(Vec::new()) };
    static NEXT_HEADERS_ID: Cell<usize> = const { Cell::new(1) };
    static NEXT_RESPONSE_ID: Cell<usize> = const { Cell::new(1) };
}

/// 登记的一个 Response：状态 + 原因短语 + 头表序号 + 已缓冲的体。
struct StoredResponse {
    status: u16,
    status_text: String,
    headers_id: usize,
    body: Vec<u8>,
    /// 最终 URL（`fetch` 写入落地地址，`new Response` 为空串）。
    url: String,
    /// 响应类型（`default` / `basic`）。
    response_type: String,
}

/// `Headers.prototype` 上的方法表。
const HEADERS_MEMBERS: &[(&str, NativeMethod)] = &[
    ("get", headers_get),
    ("set", headers_set),
    ("append", headers_append),
    ("has", headers_has),
    ("delete", headers_delete),
    ("forEach", headers_for_each),
    ("entries", headers_entries),
    ("keys", headers_keys),
    ("values", headers_values),
];

/// `Response.prototype` 上的方法表。
const RESPONSE_MEMBERS: &[(&str, NativeMethod)] = &[
    ("text", response_text),
    ("json", response_json),
    ("arrayBuffer", response_array_buffer),
    ("clone", response_clone),
];

/// 安装 `Headers` 与 `Response` 全局构造器（带原型，`instanceof` 成立）。
pub(super) fn install(context: &mut Context) {
    register_class(context, "Headers", build_headers, HEADERS_MEMBERS);
    register_class(context, "Response", build_response, RESPONSE_MEMBERS);
    // `Headers` 的迭代协议：`[Symbol.iterator]` 与 `entries()` 同一实现。
    if let Some(prototype) = prototype_of("Headers") {
        alias_symbol_member(&prototype, JsSymbol::iterator(), "entries", context);
    }
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

/// 从登记表取头表副本。
fn read_headers(id: usize) -> HeaderEntries {
    HEADERS.with(|table| {
        table
            .borrow()
            .iter()
            .find(|(existing, _)| *existing == id)
            .map(|(_, entries)| entries.clone())
            .unwrap_or_default()
    })
}

fn write_headers(id: usize, entries: HeaderEntries) {
    HEADERS.with(|table| {
        let mut table = table.borrow_mut();
        match table.iter_mut().find(|(existing, _)| *existing == id) {
            Some(entry) => entry.1 = entries,
            None => table.push((id, entries)),
        }
    });
}

/// 大小写不敏感取组合值（同名以 `, ` 连接）。
fn lookup_header(entries: &HeaderEntries, name: &str) -> Option<String> {
    let wanted = name.to_ascii_lowercase();
    let found: Vec<&str> = entries
        .iter()
        .filter(|(key, _)| key == &wanted)
        .map(|(_, value)| value.as_str())
        .collect();
    if found.is_empty() {
        return None;
    }
    Some(found.join(", "))
}

/// 追加一条头（同名保留为多条，读取时组合）。
fn append_entry(entries: &mut HeaderEntries, name: &str, value: &str) {
    entries.push((name.to_ascii_lowercase(), value.to_owned()));
}

/// 设置一条头（先删同名，再写入单值）。
fn set_entry(entries: &mut HeaderEntries, name: &str, value: &str) {
    let key = name.to_ascii_lowercase();
    entries.retain(|(existing, _)| existing != &key);
    entries.push((key, value.to_owned()));
}

/// 删除一条头。
fn delete_entry(entries: &mut HeaderEntries, name: &str) {
    let key = name.to_ascii_lowercase();
    entries.retain(|(existing, _)| existing != &key);
}

fn new_headers_object(entries: HeaderEntries, context: &mut Context) -> JsResult<JsObject> {
    let id = NEXT_HEADERS_ID.with(|cell| {
        let next = cell.get();
        cell.set(next + 1);
        next
    });
    write_headers(id, entries);
    headers_view(id, context)
}

/// 以**已有**头表序号造一个 `Headers` 视图：与响应共享同一张表，因此为 live。
fn headers_view(id: usize, context: &mut Context) -> JsResult<JsObject> {
    let object = instantiate("Headers", context)?;
    set_property(
        &object,
        js_string!("_headersId").into(),
        JsValue::from(id as u32),
        context,
    );
    Ok(object)
}

/// 解析 `HeadersInit`（本实现 Headers / `[name, value]` 数组序列 / 普通对象）为头表。
pub(super) fn header_entries_from_init(
    init: &JsValue,
    context: &mut Context,
) -> JsResult<HeaderEntries> {
    let mut entries: HeaderEntries = Vec::new();
    if init.is_undefined() || init.is_null() {
        return Ok(entries);
    }
    // 已是本实现 Headers：拷贝其表。
    if let Some(object) = init.as_object()
        && let Some(id) = object.get(js_string!("_headersId"), context)?.as_number()
    {
        return Ok(read_headers(id as usize));
    }
    // 数组序列：每项是 [name, value]。
    if let Some(object) = init.as_object()
        && let Ok(array) = JsArray::from_object(object.clone())
    {
        let length = array.length(context)?;
        for index in 0..length {
            let item = array.at(index as i64, context)?;
            let pair = item
                .as_object()
                .and_then(|object| JsArray::from_object(object.clone()).ok())
                .ok_or_else(|| {
                    JsNativeError::typ()
                        .with_message("Headers init array items must be [name, value]")
                })?;
            if pair.length(context)? < 2 {
                return Err(JsNativeError::typ()
                    .with_message("Headers init array items must be [name, value]")
                    .into());
            }
            let name = to_text(&pair.at(0, context)?, context)?;
            let value = to_text(&pair.at(1, context)?, context)?;
            append_entry(&mut entries, &name, &value);
        }
        return Ok(entries);
    }
    // 普通对象：键值对。
    let Some(object) = init.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("Headers init must be a Headers, object, or entry array")
            .into());
    };
    let keys = global_static_function(context, "Object", "keys")?;
    let names = keys.call(&JsValue::undefined(), &[object.clone().into()], context)?;
    let names = names
        .as_object()
        .and_then(|value| JsArray::from_object(value.clone()).ok())
        .ok_or_else(|| JsNativeError::typ().with_message("Object.keys returned a non-array"))?;
    let length = names.length(context)?;
    for index in 0..length {
        let key = names.at(index as i64, context)?;
        let name = to_text(&key, context)?;
        let value = object.get(js_string!(name.as_str()), context)?;
        let value = to_text(&value, context)?;
        append_entry(&mut entries, &name, &value);
    }
    Ok(entries)
}

/// `new Headers(init?)`。
fn build_headers(context: &mut Context, args: &[JsValue]) -> JsResult<JsObject> {
    let entries = header_entries_from_init(args.get_or_undefined(0), context)?;
    new_headers_object(entries, context)
}

fn headers_id_of(this: &JsValue, context: &mut Context) -> JsResult<usize> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on Headers")
            .into());
    };
    let id = object.get(js_string!("_headersId"), context)?;
    id.as_number().map(|number| number as usize).ok_or_else(|| {
        JsNativeError::typ()
            .with_message("method must be called on Headers")
            .into()
    })
}

fn headers_get(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    Ok(match lookup_header(&read_headers(id), &name) {
        Some(value) => JsValue::from(js_string!(value.as_str())),
        None => JsValue::null(),
    })
}

fn headers_set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    let value = to_text(args.get_or_undefined(1), context)?;
    let mut entries = read_headers(id);
    set_entry(&mut entries, &name, &value);
    write_headers(id, entries);
    Ok(JsValue::undefined())
}

fn headers_append(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    let value = to_text(args.get_or_undefined(1), context)?;
    let mut entries = read_headers(id);
    append_entry(&mut entries, &name, &value);
    write_headers(id, entries);
    Ok(JsValue::undefined())
}

fn headers_has(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    Ok(JsValue::from(
        lookup_header(&read_headers(id), &name).is_some(),
    ))
}

fn headers_delete(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    let mut entries = read_headers(id);
    delete_entry(&mut entries, &name);
    write_headers(id, entries);
    Ok(JsValue::undefined())
}

fn headers_for_each(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let Some(callback) = args.get_or_undefined(0).as_function() else {
        return Err(JsNativeError::typ()
            .with_message("forEach expects a callback function")
            .into());
    };
    let this_arg = args.get_or_undefined(2);
    // 组合视图：同名合并后再遍历，与规范 forEach 的去重组合行为一致。
    for entry in merged_pairs(read_headers(id)) {
        let (name, value) = entry;
        callback.call(
            this_arg,
            &[
                JsValue::from(js_string!(value.as_str())),
                JsValue::from(js_string!(name.as_str())),
                this.clone(),
            ],
            context,
        )?;
    }
    Ok(JsValue::undefined())
}

fn headers_entries(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let pairs: Vec<JsValue> = merged_pairs(read_headers(id))
        .into_iter()
        .map(|(name, value)| {
            JsValue::from(JsArray::from_iter(
                [
                    JsValue::from(js_string!(name.as_str())),
                    JsValue::from(js_string!(value.as_str())),
                ],
                context,
            ))
        })
        .collect();
    let array = JsArray::from_iter(pairs, context);
    array_iterator(&array.into(), context)
}

fn headers_keys(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let names = merged_pairs(read_headers(id))
        .into_iter()
        .map(|(name, _)| JsValue::from(js_string!(name.as_str())));
    let array = JsArray::from_iter(names, context);
    array_iterator(&array.into(), context)
}

fn headers_values(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = headers_id_of(this, context)?;
    let values = merged_pairs(read_headers(id))
        .into_iter()
        .map(|(_, value)| JsValue::from(js_string!(value.as_str())));
    let array = JsArray::from_iter(values, context);
    array_iterator(&array.into(), context)
}

/// 把可能重名的原始条目折叠成「首现顺序 + 同名 `, ` 合并」的配对序列。
fn merged_pairs(entries: HeaderEntries) -> Vec<(String, String)> {
    let mut order: Vec<String> = Vec::new();
    let mut map: Vec<(String, String)> = Vec::new();
    for (name, value) in entries {
        match map.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, existing)) => {
                existing.push_str(", ");
                existing.push_str(&value);
            }
            None => {
                order.push(name.clone());
                map.push((name, value));
            }
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

fn response_id_of(this: &JsValue, context: &mut Context) -> JsResult<usize> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on Response")
            .into());
    };
    let id = object.get(js_string!("_responseId"), context)?;
    id.as_number().map(|number| number as usize).ok_or_else(|| {
        JsNativeError::typ()
            .with_message("method must be called on Response")
            .into()
    })
}

fn read_response(id: usize) -> Option<StoredResponse> {
    RESPONSES.with(|table| {
        table
            .borrow()
            .iter()
            .find(|(existing, _)| *existing == id)
            .map(|(_, data)| StoredResponse {
                status: data.status,
                status_text: data.status_text.clone(),
                headers_id: data.headers_id,
                body: data.body.clone(),
                url: data.url.clone(),
                response_type: data.response_type.clone(),
            })
    })
}

/// body 值 → 字节：undefined/null 空；字符串 UTF-8；Blob/缓冲类取其字节。
pub(super) fn body_to_bytes(value: &JsValue, context: &mut Context) -> JsResult<Vec<u8>> {
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }
    if let Some(string) = value.as_string() {
        return Ok(string.to_std_string_escaped().into_bytes());
    }
    if let Some((bytes, _)) = super::blob::body_from_registry(value, context)? {
        return Ok(bytes);
    }
    if let Some(object) = value.as_object() {
        if let Ok(array) = JsUint8Array::from_object(object.clone()) {
            return array.to_vec(context);
        }
        if let Ok(buffer) = JsArrayBuffer::from_object(object.clone()) {
            return Ok(buffer.to_vec().unwrap_or_default());
        }
    }
    Err(JsNativeError::typ()
        .with_message("Response body must be a string, Blob or buffer-like object")
        .into())
}

fn build_response(context: &mut Context, args: &[JsValue]) -> JsResult<JsObject> {
    let body_arg = args.get_or_undefined(0);
    let mut body = body_to_bytes(body_arg, context)?;
    let mut status = 200u16;
    let mut status_text = String::new();
    let mut headers: HeaderEntries = Vec::new();

    if let Some(init) = args.get_or_undefined(1).as_object() {
        let status_value = init.get(js_string!("status"), context)?;
        if let Some(number) = status_value.as_number() {
            status = number as u16;
        }
        let text_value = init.get(js_string!("statusText"), context)?;
        if !text_value.is_undefined() && !text_value.is_null() {
            status_text = to_text(&text_value, context)?;
        }
        let headers_value = init.get(js_string!("headers"), context)?;
        headers = header_entries_from_init(&headers_value, context)?;
    }

    // 规范：体为字符串且未显式给 content-type 时自动设 `text/plain;charset=UTF-8`；
    // 体为 Blob 时沿用其 `type`。少了这一步，`new Response("hi")` 会丢掉 content-type，
    // 大量框架（如 Hono 的 `c.text()` 快速路径）依赖构造器自动补。
    if lookup_header(&headers, "content-type").is_none()
        && let Some(kind) = default_content_type(body_arg, context)?
    {
        set_entry(&mut headers, "content-type", &kind);
    }

    if !(100..600).contains(&status) {
        return Err(JsNativeError::range()
            .with_message(format!("status {status} 不在 100..=599 范围内"))
            .into());
    }
    // 204/205/304 规范上不得带体：构造时忽略 body。
    if matches!(status, 204 | 205 | 304) {
        body = Vec::new();
    }

    new_response_object(status, &status_text, headers, body, "", "default", context)
}

/// 字符串体的默认 `content-type`；Blob 体取其 `type`；其余体不设。
fn default_content_type(body: &JsValue, context: &mut Context) -> JsResult<Option<String>> {
    if body.is_undefined() || body.is_null() {
        return Ok(None);
    }
    if body.is_string() {
        return Ok(Some(String::from("text/plain;charset=UTF-8")));
    }
    let Some(object) = body.as_object() else {
        return Ok(None);
    };
    // 本实现 Blob 的 `type` 属性；空串等同未知。
    let kind = object.get(js_string!("type"), context)?;
    Ok(kind
        .as_string()
        .map(|value| value.to_std_string_escaped())
        .filter(|value| !value.is_empty()))
}

/// 以纯数据铸造一个 `Response`（构造器、`clone()`、`fetch` 三者共用同一入口）。
///
/// `headers` 会在内部登记为独立头表，实例上的 `headers` 是该表的 live 视图，
/// 因此 `res.headers.set(...)` 能真的影响 `serve` 写线。
pub(super) fn new_response_object(
    status: u16,
    status_text: &str,
    headers: HeaderEntries,
    body: Vec<u8>,
    url: &str,
    response_type: &str,
    context: &mut Context,
) -> JsResult<JsObject> {
    let view = new_headers_object(headers, context)?;
    let headers_id = view
        .get(js_string!("_headersId"), context)?
        .as_number()
        .ok_or_else(|| JsNativeError::typ().with_message("headers id unavailable"))?
        as usize;

    let id = NEXT_RESPONSE_ID.with(|cell| {
        let next = cell.get();
        cell.set(next + 1);
        next
    });
    RESPONSES.with(|table| {
        table.borrow_mut().push((
            id,
            StoredResponse {
                status,
                status_text: status_text.to_owned(),
                headers_id,
                body,
                url: url.to_owned(),
                response_type: response_type.to_owned(),
            },
        ))
    });

    let object = instantiate("Response", context)?;
    set_property(
        &object,
        js_string!("_responseId").into(),
        JsValue::from(id as u32),
        context,
    );
    set_property(
        &object,
        js_string!("status").into(),
        JsValue::from(u32::from(status)),
        context,
    );
    set_property(
        &object,
        js_string!("statusText").into(),
        JsValue::from(js_string!(status_text)),
        context,
    );
    set_property(
        &object,
        js_string!("ok").into(),
        JsValue::from((200..300).contains(&status)),
        context,
    );
    set_property(
        &object,
        js_string!("url").into(),
        JsValue::from(js_string!(url)),
        context,
    );
    set_property(
        &object,
        js_string!("type").into(),
        JsValue::from(js_string!(response_type)),
        context,
    );
    set_property(
        &object,
        js_string!("redirected").into(),
        JsValue::from(false),
        context,
    );
    set_property(&object, js_string!("headers").into(), view.into(), context);
    Ok(object)
}

/// `Response.prototype.clone()`：复制状态、头与体，新对象可独立读取。
fn response_clone(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = response_id_of(this, context)?;
    let data = read_response(id)
        .ok_or_else(|| JsNativeError::typ().with_message("Response data unavailable"))?;
    let object = new_response_object(
        data.status,
        &data.status_text,
        read_headers(data.headers_id),
        data.body,
        &data.url,
        &data.response_type,
        context,
    )?;
    Ok(object.into())
}

fn response_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = response_id_of(this, context)?;
    let data = read_response(id)
        .ok_or_else(|| JsNativeError::typ().with_message("Response data unavailable"))?;
    let text = String::from_utf8_lossy(&data.body).into_owned();
    Ok(JsPromise::resolve(JsValue::from(js_string!(text.as_str())), context)?.into())
}

fn response_json(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = response_id_of(this, context)?;
    let data = read_response(id)
        .ok_or_else(|| JsNativeError::typ().with_message("Response data unavailable"))?;
    let text = String::from_utf8_lossy(&data.body).into_owned();
    let parse = global_static_function(context, "JSON", "parse")?;
    let json = context.global_object().get(js_string!("JSON"), context)?;
    let parsed = parse.call(&json, &[JsValue::from(js_string!(text.as_str()))], context)?;
    Ok(JsPromise::resolve(parsed, context)?.into())
}

fn response_array_buffer(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = response_id_of(this, context)?;
    let data = read_response(id)
        .ok_or_else(|| JsNativeError::typ().with_message("Response data unavailable"))?;
    let view = JsUint8Array::from_iter(data.body, context)?;
    Ok(JsPromise::resolve(JsValue::from(view), context)?.into())
}

/// 以 Rust 头表铸造一个 `Headers` 实例（供 `serve` 构造 `Request.headers`）。
pub(super) fn new_headers(entries: HeaderEntries, context: &mut Context) -> JsResult<JsObject> {
    new_headers_object(entries, context)
}

/// 从本实现铸造的 `Response` 抽出 [`ResponseSpec`]（供 `serve` 写线）。
///
/// 非本实现 Response（无 `_responseId`）返回 [`None`]。
pub(super) fn response_to_spec(value: &JsValue, context: &mut Context) -> Option<ResponseSpec> {
    let object = value.as_object()?.clone();
    let id = object
        .get(js_string!("_responseId"), context)
        .ok()?
        .as_number()?;
    let data = read_response(id as usize)?;
    Some(ResponseSpec {
        status: data.status,
        status_text: data.status_text,
        headers: merged_pairs(read_headers(data.headers_id)),
        body: data.body,
    })
}
