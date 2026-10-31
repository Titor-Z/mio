//! `URL` / `URLSearchParams` 全局的引擎绑定（WHATWG URL，解析走 `url` crate）。
//!
//! v1 边界：`URL` 各部件是构造时的只读快照（规范的分量 setter 未实现）；
//! `URLSearchParams` 经内部序号 `_paramsId` 关联可变登记表，增删改查即时生效；
//! `URL` 的 `searchParams` 为构造时快照，不回写 `href`。
//! 迭代协议经 `Array.prototype[Symbol.iterator]` 桥接，for..of / 展开可直接使用。

use std::cell::{Cell, RefCell};

use boa_engine::object::builtins::JsArray;
use boa_engine::property::PropertyKey;
use boa_engine::{
    Context, JsArgs, JsNativeError, JsObject, JsResult, JsSymbol, JsValue, js_string,
};
use url::Url;

use crate::web::url::{self as weburl, Pairs};

use super::support::{
    array_iterator, attach_method, global_static_function, register_constructor, set_property,
    to_text,
};

thread_local! {
    /// URLSearchParams 可变状态登记表：序号 → 配对表。
    static PARAMS: RefCell<Vec<(usize, Pairs)>> = const { RefCell::new(Vec::new()) };
    static NEXT_PARAMS_ID: Cell<usize> = const { Cell::new(1) };
}

/// 安装 `URL` 与 `URLSearchParams` 全局。
pub(super) fn install(context: &mut Context) {
    register_constructor(context, "URLSearchParams", build_params);
    register_constructor(context, "URL", build_url);
}

// ---------------------------------------------------------------------------
// 实例铸造（引擎内部复用：URL.searchParams、fetch 返回等）
// ---------------------------------------------------------------------------

/// 铸造一个 URLSearchParams 实例。
fn new_params_object(pairs: Pairs, context: &mut Context) -> JsResult<JsObject> {
    let id = NEXT_PARAMS_ID.with(|cell| {
        let next = cell.get();
        cell.set(next + 1);
        next
    });
    PARAMS.with(|table| table.borrow_mut().push((id, pairs)));

    let object = JsObject::default(context.intrinsics());
    set_property(
        &object,
        js_string!("_paramsId").into(),
        JsValue::from(id as u32),
        context,
    );
    sync_size(&object, id, context);
    for name in [
        "append", "delete", "get", "getAll", "has", "set", "sort", "toString", "forEach",
        "entries", "keys", "values",
    ] {
        attach_method(&object, name, params_method(name), context);
    }
    // [Symbol.iterator] 与 entries 同语义。
    let entries_fn = object
        .get(js_string!("entries"), context)?
        .as_function()
        .expect("entries 刚注册必为函数");
    set_property(
        &object,
        JsSymbol::iterator().into(),
        JsValue::from(entries_fn),
        context,
    );
    Ok(object)
}

fn params_method(name: &str) -> fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue> {
    match name {
        "append" => params_append,
        "delete" => params_delete,
        "get" => params_get,
        "getAll" => params_get_all,
        "has" => params_has,
        "set" => params_set,
        "sort" => params_sort,
        "toString" => params_to_string,
        "forEach" => params_for_each,
        "entries" => params_entries,
        "keys" => params_keys,
        "values" => params_values,
        _ => unreachable!("未知的 params 方法"),
    }
}

/// 铸造一个 URL 实例：解析失败按规范抛 TypeError。
fn new_url_object(parsed: Url, context: &mut Context) -> JsResult<JsObject> {
    let href = parsed.as_str().to_owned();
    let origin = parsed.origin().ascii_serialization();
    let host = match parsed.host_str() {
        Some(hostname) => match parsed.port() {
            Some(port) => format!("{hostname}:{port}"),
            None => hostname.to_owned(),
        },
        None => String::new(),
    };
    let port = parsed
        .port()
        .map(|port| port.to_string())
        .unwrap_or_default();
    let pathname = parsed.path().to_owned();
    let search = parsed
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let hash = parsed
        .fragment()
        .map(|fragment| format!("#{fragment}"))
        .unwrap_or_default();

    let object = JsObject::default(context.intrinsics());
    let text = |value: &str| JsValue::from(js_string!(value));
    set_property(&object, js_string!("href").into(), text(&href), context);
    set_property(&object, js_string!("origin").into(), text(&origin), context);
    set_property(
        &object,
        js_string!("protocol").into(),
        text(&format!("{}:", parsed.scheme())),
        context,
    );
    set_property(
        &object,
        js_string!("username").into(),
        text(parsed.username()),
        context,
    );
    set_property(
        &object,
        js_string!("password").into(),
        text(parsed.password().unwrap_or("")),
        context,
    );
    set_property(&object, js_string!("host").into(), text(&host), context);
    set_property(
        &object,
        js_string!("hostname").into(),
        text(parsed.host_str().unwrap_or("")),
        context,
    );
    set_property(&object, js_string!("port").into(), text(&port), context);
    set_property(
        &object,
        js_string!("pathname").into(),
        text(&pathname),
        context,
    );
    set_property(&object, js_string!("search").into(), text(&search), context);
    set_property(&object, js_string!("hash").into(), text(&hash), context);

    let params = new_params_object(weburl::parse_query(parsed.query().unwrap_or("")), context)?;
    set_property(
        &object,
        js_string!("searchParams").into(),
        params.into(),
        context,
    );

    attach_method(&object, "toString", url_to_string, context);
    Ok(object)
}

// ---------------------------------------------------------------------------
// 构造器
// ---------------------------------------------------------------------------

fn build_params(context: &mut Context, args: &[JsValue]) -> JsResult<JsObject> {
    let pairs = params_init(args.get_or_undefined(0), context)?.unwrap_or_default();
    new_params_object(pairs, context)
}

/// 解析 URLSearchParams 的 init：串 / 配对数组 / 记录对象 / 另一实例。
fn params_init(init: &JsValue, context: &mut Context) -> JsResult<Option<Pairs>> {
    if init.is_undefined() || init.is_null() {
        return Ok(None);
    }
    if let Some(string) = init.as_string() {
        let query = string.to_std_string_escaped();
        let query = query.strip_prefix('?').unwrap_or(query.as_str()).to_owned();
        return Ok(Some(weburl::parse_query(&query)));
    }
    let Some(object) = init.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("URLSearchParams init must be a string, sequence or record")
            .into());
    };
    // 另一实例：按 _paramsId 拷贝配对表。
    let marker = object.get(js_string!("_paramsId"), context)?;
    if let Some(number) = marker.as_number() {
        return Ok(Some(read_params(number as usize)));
    }
    // 配对数组 [[k, v], ...]。
    if let Ok(array) = JsArray::from_object(object.clone()) {
        let length = array.length(context)?;
        let mut pairs = Pairs::new();
        for index in 0..length {
            let entry = array.at(index as i64, context)?;
            let Some(entry) = entry.as_object() else {
                return Err(JsNativeError::typ()
                    .with_message("URLSearchParams sequence entries must be pairs")
                    .into());
            };
            let key = to_text(&entry.get(index_key(0), context)?, context)?;
            let value = to_text(&entry.get(index_key(1), context)?, context)?;
            pairs.push((key, value));
        }
        return Ok(Some(pairs));
    }
    // 记录对象 {k: v}。
    let keys = global_static_function(context, "Object", "keys")?;
    let names = keys.call(&JsValue::undefined(), &[object.clone().into()], context)?;
    let names = names
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("Object.keys returned a non-array"))?;
    let length = names
        .get(js_string!("length"), context)?
        .as_number()
        .unwrap_or(0.0) as u32;
    let mut pairs = Pairs::new();
    for index in 0..length {
        let key = names.get(index, context)?;
        let Some(key_string) = key.as_string() else {
            continue;
        };
        let key = key_string.to_std_string_escaped();
        let value = object.get(js_string!(key.as_str()), context)?;
        pairs.push((key, to_text(&value, context)?));
    }
    Ok(Some(pairs))
}

fn index_key(index: u32) -> PropertyKey {
    PropertyKey::from(index)
}

fn build_url(context: &mut Context, args: &[JsValue]) -> JsResult<JsObject> {
    let input = args.get_or_undefined(0);
    // URL 实例可直接作为输入（规范允许，String(url) 同义）。
    let input = match input.as_string() {
        Some(string) => string.to_std_string_escaped(),
        None => to_text(input, context)?,
    };
    let base = args.get_or_undefined(1);
    let parsed = if base.is_undefined() || base.is_null() {
        Url::parse(&input)
    } else {
        let base_text = to_text(base, context)?;
        Url::parse(&base_text)
            .map_err(|_| {
                JsNativeError::typ().with_message(format!("invalid base URL: {base_text}"))
            })?
            .join(&input)
    }
    .map_err(|error| {
        JsNativeError::typ().with_message(format!("invalid URL: {input}（{error}）"))
    })?;
    new_url_object(parsed, context)
}

// ---------------------------------------------------------------------------
// 登记表读写
// ---------------------------------------------------------------------------

fn read_params(id: usize) -> Pairs {
    PARAMS.with(|table| {
        table
            .borrow()
            .iter()
            .find(|(existing, _)| *existing == id)
            .map(|(_, pairs)| pairs.clone())
            .unwrap_or_default()
    })
}

fn write_params(id: usize, pairs: Pairs) {
    PARAMS.with(|table| {
        let mut table = table.borrow_mut();
        match table.iter_mut().find(|(existing, _)| *existing == id) {
            Some(entry) => entry.1 = pairs,
            None => table.push((id, pairs)),
        }
    });
}

/// 从 `this` 取 `_paramsId`；不是本实现实例则 TypeError。
fn params_id_of(this: &JsValue, context: &mut Context) -> JsResult<usize> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on URLSearchParams")
            .into());
    };
    let id = object.get(js_string!("_paramsId"), context)?;
    id.as_number().map(|number| number as usize).ok_or_else(|| {
        JsNativeError::typ()
            .with_message("method must be called on URLSearchParams")
            .into()
    })
}

/// size 是数据属性：任何改变配对数的操作后同步。
fn sync_size(object: &JsObject, id: usize, context: &mut Context) {
    let length = read_params(id).len();
    set_property(
        object,
        js_string!("size").into(),
        JsValue::from(length as u32),
        context,
    );
}

// ---------------------------------------------------------------------------
// 方法实现
// ---------------------------------------------------------------------------

fn params_append(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let mut pairs = read_params(id);
    weburl::append(
        &mut pairs,
        to_text(args.get_or_undefined(0), context)?,
        to_text(args.get_or_undefined(1), context)?,
    );
    write_params(id, pairs);
    if let Some(object) = this.as_object() {
        sync_size(&object, id, context);
    }
    Ok(JsValue::undefined())
}

fn params_set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let mut pairs = read_params(id);
    weburl::set(
        &mut pairs,
        to_text(args.get_or_undefined(0), context)?,
        to_text(args.get_or_undefined(1), context)?,
    );
    write_params(id, pairs);
    if let Some(object) = this.as_object() {
        sync_size(&object, id, context);
    }
    Ok(JsValue::undefined())
}

fn params_delete(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let mut pairs = read_params(id);
    weburl::delete(&mut pairs, &to_text(args.get_or_undefined(0), context)?);
    write_params(id, pairs);
    if let Some(object) = this.as_object() {
        sync_size(&object, id, context);
    }
    Ok(JsValue::undefined())
}

fn params_sort(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let mut pairs = read_params(id);
    weburl::sort(&mut pairs);
    write_params(id, pairs);
    Ok(JsValue::undefined())
}

fn params_get(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    let pairs = read_params(id);
    Ok(match weburl::get(&pairs, &name) {
        Some(value) => JsValue::from(js_string!(value)),
        None => JsValue::null(),
    })
}

fn params_get_all(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    let values = weburl::get_all(&read_params(id), &name);
    let array = JsArray::from_iter(
        values
            .into_iter()
            .map(|value| JsValue::from(js_string!(value))),
        context,
    );
    Ok(array.into())
}

fn params_has(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    Ok(JsValue::from(weburl::has(&read_params(id), &name)))
}

fn params_to_string(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    Ok(JsValue::from(js_string!(
        weburl::serialize_query(&read_params(id)).as_str()
    )))
}

fn params_for_each(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let Some(callback) = args.get_or_undefined(0).as_function() else {
        return Err(JsNativeError::typ()
            .with_message("forEach expects a callback function")
            .into());
    };
    let this_arg = args.get_or_undefined(1);
    for (name, value) in read_params(id) {
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

fn params_entries(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let pairs: Vec<JsValue> = read_params(id)
        .into_iter()
        .map(|(name, value)| {
            JsValue::from(JsArray::from_iter(
                [
                    JsValue::from(js_string!(name)),
                    JsValue::from(js_string!(value)),
                ],
                context,
            ))
        })
        .collect();
    let array = JsArray::from_iter(pairs, context);
    array_iterator(&array.into(), context)
}

fn params_keys(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let names = read_params(id)
        .into_iter()
        .map(|(name, _)| JsValue::from(js_string!(name)));
    let array = JsArray::from_iter(names, context);
    array_iterator(&array.into(), context)
}

fn params_values(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = params_id_of(this, context)?;
    let values = read_params(id)
        .into_iter()
        .map(|(_, value)| JsValue::from(js_string!(value)));
    let array = JsArray::from_iter(values, context);
    array_iterator(&array.into(), context)
}

fn url_to_string(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on URL")
            .into());
    };
    object.get(js_string!("href"), context)
}

/// 供 `fetch` 绑定复用：识别本实现铸造的 URL 实例并取 href。
pub(super) fn url_href(value: &JsValue, context: &mut Context) -> JsResult<Option<String>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let href = object.get(js_string!("href"), context)?;
    Ok(href
        .as_string()
        .map(|string| string.to_std_string_escaped()))
}
