//! `Blob` / `FormData` 全局的引擎绑定（WHATWG FileAPI / HTML 表单数据族）。
//!
//! 数据承载：实例经内部序号（`_blobId` / `_formDataId`）关联线程局部登记表，
//! `append`/`set`/`delete` 即时生效；`fetch` 的 body 接受本实现铸造的
//! Blob / FormData / Uint8Array / ArrayBuffer，其中 FormData 在此序列化为
//! multipart/form-data（编码在纯层 `web::form_data`）。
//!
//! v1 边界：`Blob.stream()` 与 ReadableStream 族未实现；File 仅携带 `name`
//! 属性的 Blob 形态（无 lastModified）。

use std::cell::{Cell, RefCell};

use boa_engine::object::builtins::{JsArray, JsArrayBuffer, JsPromise, JsUint8Array};
use boa_engine::{
    Context, JsArgs, JsNativeError, JsObject, JsResult, JsSymbol, JsValue, js_string,
};

use crate::web::form_data::{self as webfd, Entries, FieldValue};

use super::support::{array_iterator, attach_method, register_constructor, set_property, to_text};

/// 登记的一个 Blob：字节 + MIME 类型 + 可选文件名（有文件名即 File 形态）。
#[derive(Clone)]
pub(super) struct BlobData {
    bytes: Vec<u8>,
    type_: String,
    name: Option<String>,
}

thread_local! {
    static BLOBS: RefCell<Vec<(usize, BlobData)>> = const { RefCell::new(Vec::new()) };
    static FORMS: RefCell<Vec<(usize, Entries)>> = const { RefCell::new(Vec::new()) };
    static NEXT_BLOB_ID: Cell<usize> = const { Cell::new(1) };
    static NEXT_FORM_ID: Cell<usize> = const { Cell::new(1) };
}

/// 安装 `Blob` 与 `FormData` 全局。
pub(super) fn install(context: &mut Context) {
    register_constructor(context, "Blob", build_blob);
    register_constructor(context, "FormData", build_form_data);
}

// ---------------------------------------------------------------------------
// Blob
// ---------------------------------------------------------------------------

fn build_blob(context: &mut Context, args: &[JsValue]) -> JsResult<JsObject> {
    let parts = args.get_or_undefined(0);
    let mut bytes: Vec<u8> = Vec::new();
    if !parts.is_undefined() && !parts.is_null() {
        let array =
            JsArray::from_object(parts.as_object().ok_or_else(|| {
                JsNativeError::typ().with_message("Blob parts must be an iterable")
            })?)
            .map_err(|_| JsNativeError::typ().with_message("Blob parts must be an array"))?;
        let length = array.length(context)?;
        for index in 0..length {
            let part = array.at(index as i64, context)?;
            flatten_blob_part(&part, &mut bytes, context)?;
        }
    }
    let mut type_ = String::new();
    if let Some(options) = args.get_or_undefined(1).as_object() {
        let candidate = options.get(js_string!("type"), context)?;
        if let Some(string) = candidate.as_string() {
            type_ = string.to_std_string_escaped().to_ascii_lowercase();
        }
    }
    new_blob_object(bytes, type_, None, context)
}

/// Blob part 展平：USVString / Blob / TypedArray / ArrayBuffer（规范允许的 v1 子集）。
fn flatten_blob_part(value: &JsValue, out: &mut Vec<u8>, context: &mut Context) -> JsResult<()> {
    if let Some(string) = value.as_string() {
        out.extend_from_slice(string.to_std_string_escaped().as_bytes());
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("Blob part must be a string, Blob or buffer-like object")
            .into());
    };
    if let Some(data) = blob_of(&object.clone().into(), context)? {
        out.extend_from_slice(&data.bytes);
        return Ok(());
    }
    if let Ok(array) = JsUint8Array::from_object(object.clone()) {
        out.extend_from_slice(&array.to_vec(context)?);
        return Ok(());
    }
    if let Ok(buffer) = JsArrayBuffer::from_object(object.clone())
        && let Some(bytes) = buffer.to_vec()
    {
        out.extend_from_slice(&bytes);
        return Ok(());
    }
    Err(JsNativeError::typ()
        .with_message("Blob part must be a string, Blob or buffer-like object")
        .into())
}

/// 铸造 Blob（或 File 形态）实例；`name` 为 `Some` 时挂 `name` 属性。
fn new_blob_object(
    bytes: Vec<u8>,
    type_: String,
    name: Option<String>,
    context: &mut Context,
) -> JsResult<JsObject> {
    let id = NEXT_BLOB_ID.with(|cell| {
        let next = cell.get();
        cell.set(next + 1);
        next
    });
    let size = bytes.len();
    BLOBS.with(|table| {
        table.borrow_mut().push((
            id,
            BlobData {
                bytes,
                type_: type_.clone(),
                name: name.clone(),
            },
        ))
    });

    let object = JsObject::default(context.intrinsics());
    set_property(
        &object,
        js_string!("_blobId").into(),
        JsValue::from(id as u32),
        context,
    );
    set_property(
        &object,
        js_string!("size").into(),
        JsValue::from(size as u32),
        context,
    );
    set_property(
        &object,
        js_string!("type").into(),
        JsValue::from(js_string!(type_.as_str())),
        context,
    );
    if let Some(name) = name {
        set_property(
            &object,
            js_string!("name").into(),
            JsValue::from(js_string!(name.as_str())),
            context,
        );
    }
    attach_method(&object, "text", blob_text, context);
    attach_method(&object, "arrayBuffer", blob_array_buffer, context);
    Ok(object)
}

fn blob_id_of(this: &JsValue, context: &mut Context) -> JsResult<usize> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on Blob")
            .into());
    };
    let id = object.get(js_string!("_blobId"), context)?;
    id.as_number().map(|number| number as usize).ok_or_else(|| {
        JsNativeError::typ()
            .with_message("method must be called on Blob")
            .into()
    })
}

/// 读取本实现 Blob 的登记数据；非 Blob 返回 `None`（供 part 展平与 fetch 复用）。
pub(super) fn blob_of(value: &JsValue, context: &mut Context) -> JsResult<Option<BlobData>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let id = object.get(js_string!("_blobId"), context)?;
    let Some(number) = id.as_number() else {
        return Ok(None);
    };
    let id = number as usize;
    Ok(BLOBS.with(|table| {
        table
            .borrow()
            .iter()
            .find(|(existing, _)| *existing == id)
            .map(|(_, data)| data.clone())
    }))
}

fn blob_data(this: &JsValue, context: &mut Context) -> JsResult<BlobData> {
    let id = blob_id_of(this, context)?;
    BLOBS.with(|table| {
        table
            .borrow()
            .iter()
            .find(|(existing, _)| *existing == id)
            .map(|(_, data)| data.clone())
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("Blob data unavailable")
                    .into()
            })
    })
}

fn blob_text(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let data = blob_data(this, context)?;
    let text = String::from_utf8_lossy(&data.bytes).to_string();
    Ok(JsPromise::resolve(JsValue::from(js_string!(text.as_str())), context)?.into())
}

fn blob_array_buffer(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let data = blob_data(this, context)?;
    let view = JsUint8Array::from_iter(data.bytes, context)?;
    Ok(JsPromise::resolve(JsValue::from(view), context)?.into())
}

// ---------------------------------------------------------------------------
// FormData
// ---------------------------------------------------------------------------

fn build_form_data(context: &mut Context, _args: &[JsValue]) -> JsResult<JsObject> {
    let id = NEXT_FORM_ID.with(|cell| {
        let next = cell.get();
        cell.set(next + 1);
        next
    });
    FORMS.with(|table| table.borrow_mut().push((id, Vec::new())));

    let object = JsObject::default(context.intrinsics());
    set_property(
        &object,
        js_string!("_formDataId").into(),
        JsValue::from(id as u32),
        context,
    );
    for name in [
        "append", "set", "delete", "get", "getAll", "has", "forEach", "entries", "keys", "values",
    ] {
        attach_method(&object, name, form_method(name), context);
    }
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

fn form_method(name: &str) -> fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue> {
    match name {
        "append" => form_append,
        "set" => form_set,
        "delete" => form_delete,
        "get" => form_get,
        "getAll" => form_get_all,
        "has" => form_has,
        "forEach" => form_for_each,
        "entries" => form_entries,
        "keys" => form_keys,
        "values" => form_values,
        _ => unreachable!("未知的 formData 方法"),
    }
}

fn form_id_of(this: &JsValue, context: &mut Context) -> JsResult<usize> {
    let Some(object) = this.as_object() else {
        return Err(JsNativeError::typ()
            .with_message("method must be called on FormData")
            .into());
    };
    let id = object.get(js_string!("_formDataId"), context)?;
    id.as_number().map(|number| number as usize).ok_or_else(|| {
        JsNativeError::typ()
            .with_message("method must be called on FormData")
            .into()
    })
}

fn read_form(id: usize) -> Entries {
    FORMS.with(|table| {
        table
            .borrow()
            .iter()
            .find(|(existing, _)| *existing == id)
            .map(|(_, entries)| entries.clone())
            .unwrap_or_default()
    })
}

fn write_form(id: usize, entries: Entries) {
    FORMS.with(|table| {
        let mut table = table.borrow_mut();
        match table.iter_mut().find(|(existing, _)| *existing == id) {
            Some(entry) => entry.1 = entries,
            None => table.push((id, entries)),
        }
    });
}

/// JS 值 → 字段值：Blob → File 条目，缓冲类 → 无名文件，其余按 USVString。
fn to_field_value(
    value: &JsValue,
    filename: Option<String>,
    context: &mut Context,
) -> JsResult<FieldValue> {
    if let Some(data) = blob_of(value, context)? {
        return Ok(FieldValue::File {
            filename: filename.unwrap_or(data.name.clone().unwrap_or_else(|| "blob".to_string())),
            content_type: if data.type_.is_empty() {
                "application/octet-stream".to_string()
            } else {
                data.type_.clone()
            },
            bytes: data.bytes,
        });
    }
    if let Some(object) = value.as_object() {
        if let Ok(array) = JsUint8Array::from_object(object.clone()) {
            return Ok(FieldValue::File {
                filename: filename.unwrap_or_else(|| "blob".to_string()),
                content_type: "application/octet-stream".to_string(),
                bytes: array.to_vec(context)?,
            });
        }
        if let Ok(buffer) = JsArrayBuffer::from_object(object.clone()) {
            return Ok(FieldValue::File {
                filename: filename.unwrap_or_else(|| "blob".to_string()),
                content_type: "application/octet-stream".to_string(),
                bytes: buffer.to_vec().unwrap_or_default(),
            });
        }
    }
    Ok(FieldValue::Text(to_text(value, context)?))
}

fn form_append(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let mut entries = read_form(id);
    let name = to_text(args.get_or_undefined(0), context)?;
    let filename = match args.get_or_undefined(2) {
        value if value.is_undefined() => None,
        value => Some(to_text(value, context)?),
    };
    let value = to_field_value(args.get_or_undefined(1), filename, context)?;
    webfd::append(&mut entries, name, value);
    write_form(id, entries);
    Ok(JsValue::undefined())
}

fn form_set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let mut entries = read_form(id);
    let name = to_text(args.get_or_undefined(0), context)?;
    let filename = match args.get_or_undefined(2) {
        value if value.is_undefined() => None,
        value => Some(to_text(value, context)?),
    };
    let value = to_field_value(args.get_or_undefined(1), filename, context)?;
    webfd::set(&mut entries, name, value);
    write_form(id, entries);
    Ok(JsValue::undefined())
}

fn form_delete(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let mut entries = read_form(id);
    webfd::delete(&mut entries, &to_text(args.get_or_undefined(0), context)?);
    write_form(id, entries);
    Ok(JsValue::undefined())
}

/// 字段值 → JS 值：文本回字符串，文件回 File 形态 Blob。
fn field_to_js(value: &FieldValue, context: &mut Context) -> JsResult<JsValue> {
    match value {
        FieldValue::Text(text) => Ok(JsValue::from(js_string!(text.as_str()))),
        FieldValue::File {
            filename,
            content_type,
            bytes,
        } => Ok(JsValue::from(new_blob_object(
            bytes.clone(),
            content_type.clone(),
            Some(filename.clone()),
            context,
        )?)),
    }
}

fn form_get(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    let entries = read_form(id);
    match webfd::get(&entries, &name) {
        Some(value) => field_to_js(value, context),
        None => Ok(JsValue::null()),
    }
}

fn form_get_all(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    let values: Vec<JsValue> = webfd::get_all(&read_form(id), &name)
        .into_iter()
        .map(|value| field_to_js(&value, context))
        .collect::<JsResult<_>>()?;
    Ok(JsValue::from(JsArray::from_iter(values, context)))
}

fn form_has(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let name = to_text(args.get_or_undefined(0), context)?;
    Ok(JsValue::from(webfd::has(&read_form(id), &name)))
}

fn form_for_each(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let Some(callback) = args.get_or_undefined(0).as_function() else {
        return Err(JsNativeError::typ()
            .with_message("forEach expects a callback function")
            .into());
    };
    let this_arg = args.get_or_undefined(1);
    for (name, value) in read_form(id) {
        let js_value = field_to_js(&value, context)?;
        callback.call(
            this_arg,
            &[
                js_value,
                JsValue::from(js_string!(name.as_str())),
                this.clone(),
            ],
            context,
        )?;
    }
    Ok(JsValue::undefined())
}

fn form_entries(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let pairs: Vec<JsValue> = read_form(id)
        .into_iter()
        .map(|(name, value)| {
            let js_value = field_to_js(&value, context)?;
            Ok(JsValue::from(JsArray::from_iter(
                [JsValue::from(js_string!(name)), js_value],
                context,
            )))
        })
        .collect::<JsResult<_>>()?;
    let array = JsArray::from_iter(pairs, context);
    array_iterator(&array.into(), context)
}

fn form_keys(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let names = read_form(id)
        .into_iter()
        .map(|(name, _)| JsValue::from(js_string!(name)));
    let array = JsArray::from_iter(names, context);
    array_iterator(&array.into(), context)
}

fn form_values(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = form_id_of(this, context)?;
    let values: Vec<JsValue> = read_form(id)
        .into_iter()
        .map(|(_, value)| field_to_js(&value, context))
        .collect::<JsResult<_>>()?;
    let array = JsArray::from_iter(values, context);
    array_iterator(&array.into(), context)
}

// ---------------------------------------------------------------------------
// fetch 集成
// ---------------------------------------------------------------------------

/// 把 body 值解释为本实现的数据承载对象并产出（字节，可选 content-type）。
///
/// 命中 FormData / Blob 时返回 `Some`；其余类型返回 `None` 交回调用方继续
/// 按字符串 / typed array 处理。
pub(super) fn body_from_registry(
    value: &JsValue,
    context: &mut Context,
) -> JsResult<Option<(Vec<u8>, Option<String>)>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if let Some(id) = object.get(js_string!("_formDataId"), context)?.as_number() {
        let entries = read_form(id as usize);
        let boundary = webfd::generate_boundary(current_entropy());
        let bytes = webfd::encode_multipart(&entries, &boundary);
        return Ok(Some((
            bytes,
            Some(format!("multipart/form-data; boundary={boundary}")),
        )));
    }
    if let Some(data) = blob_of(value, context)? {
        let content_type = (!data.type_.is_empty()).then_some(data.type_.clone());
        return Ok(Some((data.bytes, content_type)));
    }
    Ok(None)
}

/// 分隔串种子：时钟纳秒 + 进程级计数器，足以保证同进程内不重复。
fn current_entropy() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0);
    let tick = COUNTER.fetch_add(1, Ordering::Relaxed);
    nanos.rotate_left(12) ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}
