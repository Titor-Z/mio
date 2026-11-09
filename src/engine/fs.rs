//! `@std/fs` 模块的 Boa 引擎绑定层（Phase 2）。
//!
//! 职责：把 `stdlib::fs` 的纯 Rust 函数包装成脚本可见的 `NativeFunction`。
//!
//! 线程模型（Phase 2）：
//! - 所有真实 IO 交给一个小型工作线程池执行，不阻塞事件循环；
//! - 完成后经与 fetch 共用的事件通道送回 `LoopEvent::Fs`；
//! - 宿主事件循环在 `drive_event_loop` 中取出并 settle 对应 Promise。
//!
//! 两层 API：
//! - Convenience：`readFile`/`writeFile`/... 全部返回 Promise。
//! - Control（FD）：`open(path, mode?)` → `File`，`File.read/write/close` 亦返回 Promise。
//!
//! FD 句柄表是进程级共享的（工作线程要访问），脚本侧 `File._fd` 仅存数值 id。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use boa_engine::object::builtins::{JsArray, JsFunction, JsPromise, JsUint8Array};
use boa_engine::{
    Context, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction, js_string,
};

use crate::stdlib::fs as fs_core;

use super::fetch::{self as engine_fetch, LoopEvent};
use super::support::{attach_method, set_property};

// ============================================================================
// 类型：工作线程 → 引擎线程的结果载体（全部 Send）
// ============================================================================

/// 一次 fs 操作的结果值（纯 Rust 数据，到了引擎线程才转成 JS 值）。
pub(super) enum FsValue {
    /// 空值（写入、删除等）。
    Unit,
    /// 数值（写入字节数等）。
    Number(f64),
    /// 布尔（exists）。
    Bool(bool),
    /// 字节数组（readFile / File.read）。
    Bytes(Vec<u8>),
    /// 文本（readTextFile）。
    Text(String),
    /// 目录条目列表（readdir）。
    List(Vec<String>),
    /// 元数据（stat）。
    Info(fs_core::FileInfo),
    /// 打开的句柄 id（open）。
    Fd(u64),
}

/// 操作结果：`Ok` 值或错误消息。
pub(super) type FsOutcome = Result<FsValue, String>;

/// 事件通道里的一条 fs 完成消息：操作序号 + 结果。
pub(super) type FsResult = (u64, FsOutcome);

// ============================================================================
// 工作线程池
// ============================================================================

/// 一个待执行任务。
type Job = Box<dyn FnOnce() + Send + 'static>;

/// 极简线程池：共享一个任务接收端，工作线程并发执行。
struct Pool {
    sender: Sender<Job>,
}

static POOL: OnceLock<Pool> = OnceLock::new();

/// 取全局线程池，首次调用时惰性创建。
fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        // 工作线程数取可用并行度，限制在 1..=4，避免过度占核。
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 4);
        for _ in 0..workers {
            let receiver = Arc::clone(&receiver);
            std::thread::spawn(move || {
                loop {
                    // 持锁阻塞取任务；只有一个线程在 recv 上等待，其余在锁上等待。
                    let job = {
                        let guard = receiver.lock().expect("fs 工作线程接收端锁中毒");
                        guard.recv()
                    };
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                }
            });
        }
        Pool { sender }
    })
}

/// 提交一个任务到线程池。
fn submit(job: impl FnOnce() + Send + 'static) {
    let _ = pool().sender.send(Box::new(job));
}

// ============================================================================
// 句柄表与序号
// ============================================================================

/// FD 句柄表：id → 可跨线程共享的文件句柄。
type FdTable = Mutex<HashMap<u64, Arc<Mutex<std::fs::File>>>>;

static FD_TABLE: OnceLock<FdTable> = OnceLock::new();

/// 取全局 FD 句柄表。
fn fd_table() -> &'static FdTable {
    FD_TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 下一个 FD id。
static NEXT_FD: AtomicU64 = AtomicU64::new(1);

/// 下一个操作（Promise）序号。
static NEXT_OP: AtomicU64 = AtomicU64::new(1);

// ============================================================================
// 挂起 Promise 表（仅存活于引擎线程）
// ============================================================================

/// 一个尚未 settle 的 fs Promise 两端句柄，及 File.read 的可选目标缓冲区。
struct PendingFs {
    id: u64,
    resolve: JsFunction,
    reject: JsFunction,
    /// File.read 的调用方缓冲区：settle 时把结果写回这里并返回读取字节数。
    buffer: Option<JsObject>,
}

thread_local! {
    static PENDING: RefCell<Vec<PendingFs>> = const { RefCell::new(Vec::new()) };
}

/// 是否有未完成的 fs 操作（事件循环据此决定是否继续等待）。
pub(super) fn has_pending() -> bool {
    PENDING.with(|list| !list.borrow().is_empty())
}

/// 从挂起表移除并返回指定序号条目。
fn take_pending(id: u64) -> Option<PendingFs> {
    PENDING.with(|list| {
        let mut list = list.borrow_mut();
        let index = list.iter().position(|pending| pending.id == id)?;
        Some(list.remove(index))
    })
}

// ============================================================================
// 提交操作的统一入口
// ============================================================================

/// 提交一个 fs 操作并返回待兑现的 Promise。
fn spawn(
    op: impl FnOnce() -> FsOutcome + Send + 'static,
    buffer: Option<JsObject>,
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = NEXT_OP.fetch_add(1, Ordering::Relaxed);
    let (promise, resolvers) = JsPromise::new_pending(context);
    PENDING.with(|list| {
        list.borrow_mut().push(PendingFs {
            id,
            resolve: resolvers.resolve,
            reject: resolvers.reject,
            buffer,
        })
    });

    let sender = engine_fetch::sender()
        .ok_or_else(|| JsNativeError::typ().with_message("fs 事件通道不可用"))?;
    submit(move || {
        let outcome = op();
        let _ = sender.send(LoopEvent::Fs((id, outcome)));
    });

    Ok(promise.into())
}

// ============================================================================
// settle：把完成结果兑现成 JS 值
// ============================================================================

/// settle 一批 fs 完成结果。
pub(super) fn settle(results: Vec<FsResult>, context: &mut Context) {
    for (id, outcome) in results {
        let Some(pending) = take_pending(id) else {
            continue;
        };
        let call = match outcome {
            Ok(value) => match produce(value, pending.buffer.as_ref(), context) {
                Ok(js) => pending.resolve.call(&JsValue::undefined(), &[js], context),
                Err(error) => {
                    let err = JsNativeError::typ()
                        .with_message(error.to_string())
                        .into_opaque(context);
                    pending
                        .reject
                        .call(&JsValue::undefined(), &[err.into()], context)
                }
            },
            Err(message) => {
                let error = JsNativeError::typ()
                    .with_message(message)
                    .into_opaque(context);
                pending
                    .reject
                    .call(&JsValue::undefined(), &[error.into()], context)
            }
        };
        if let Err(error) = call {
            eprintln!("mio: failed to settle fs promise: {error}");
        }
    }
}

/// 把 [`FsValue`] 转成脚本值；`buffer` 存在时（File.read）写回缓冲区并返回字节数。
fn produce(value: FsValue, buffer: Option<&JsObject>, context: &mut Context) -> JsResult<JsValue> {
    match value {
        FsValue::Unit => Ok(JsValue::undefined()),
        FsValue::Number(n) => Ok(JsValue::from(n)),
        FsValue::Bool(b) => Ok(JsValue::from(b)),
        FsValue::Text(text) => Ok(JsValue::from(JsString::from(text))),
        FsValue::List(items) => Ok(JsArray::from_iter(
            items
                .into_iter()
                .map(|item| JsValue::from(JsString::from(item))),
            context,
        )
        .into()),
        FsValue::Info(info) => info_to_object(&info, context),
        FsValue::Fd(id) => Ok(file_object(id, context)?.into()),
        FsValue::Bytes(bytes) => match buffer {
            Some(target) => {
                let count = bytes.len();
                let source = JsUint8Array::from_iter(bytes, context)?;
                let target = JsUint8Array::from_object(target.clone()).map_err(|_| {
                    JsNativeError::typ().with_message("File.read 的缓冲区必须是 Uint8Array")
                })?;
                target.set_values(source.into(), Some(0), context)?;
                Ok(JsValue::from(count as f64))
            }
            None => Ok(JsUint8Array::from_iter(bytes, context)?.into()),
        },
    }
}

// ============================================================================
// 参数与对象工具
// ============================================================================

/// 取第 `index` 个参数并转成 String；缺失或非字符串时报 TypeError。
fn arg_string(args: &[JsValue], index: usize, function: &str) -> JsResult<String> {
    args.get(index)
        .and_then(|value| value.as_string())
        .map(|text| text.to_std_string_escaped())
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message(format!("{function} 缺少第 {} 个字符串参数", index + 1))
                .into()
        })
}

/// 从参数里取 `Uint8Array` 字节。
fn arg_bytes(
    args: &[JsValue],
    index: usize,
    function: &str,
    context: &mut Context,
) -> JsResult<Vec<u8>> {
    let value = args.get(index).ok_or_else(|| {
        JsNativeError::typ().with_message(format!("{function} 缺少第 {} 个数据参数", index + 1))
    })?;
    let object = value.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message(format!("{function} 的数据参数必须是 Uint8Array"))
    })?;
    let view = JsUint8Array::from_object(object.clone()).map_err(|_| {
        JsNativeError::typ().with_message(format!("{function} 的数据参数必须是 Uint8Array"))
    })?;
    view.to_vec(context)
}

/// 从 `this._fd` 取文件句柄 id。
fn fd_of(this: &JsValue, context: &mut Context) -> JsResult<u64> {
    this.as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("File 方法调用者非法"))?
        .get(js_string!("_fd"), context)
        .ok()
        .and_then(|value| value.as_number())
        .map(|n| n as u64)
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("File 缺少有效的 _fd")
                .into()
        })
}

/// 把 `FileInfo` 转成脚本侧普通对象。
fn info_to_object(info: &fs_core::FileInfo, context: &mut Context) -> JsResult<JsValue> {
    let object = JsObject::default(context.intrinsics());
    set_property(
        &object,
        js_string!("isFile").into(),
        JsValue::from(info.is_file),
        context,
    );
    set_property(
        &object,
        js_string!("isDirectory").into(),
        JsValue::from(info.is_directory),
        context,
    );
    set_property(
        &object,
        js_string!("isSymlink").into(),
        JsValue::from(info.is_symlink),
        context,
    );
    set_property(
        &object,
        js_string!("size").into(),
        JsValue::from(info.size as f64),
        context,
    );
    if let Some(modified) = info.modified {
        set_property(
            &object,
            js_string!("modified").into(),
            JsValue::from(modified),
            context,
        );
    }
    if let Some(accessed) = info.accessed {
        set_property(
            &object,
            js_string!("accessed").into(),
            JsValue::from(accessed),
            context,
        );
    }
    if let Some(created) = info.created {
        set_property(
            &object,
            js_string!("created").into(),
            JsValue::from(created),
            context,
        );
    }
    #[cfg(unix)]
    if info.permissions != 0 {
        set_property(
            &object,
            js_string!("permissions").into(),
            JsValue::from(info.permissions as f64),
            context,
        );
    }
    Ok(object.into())
}

/// 创建一个 `File` 对象：挂 `_fd` 与 `read`/`write`/`close` 方法。
fn file_object(fd: u64, context: &mut Context) -> JsResult<JsObject> {
    let object = JsObject::default(context.intrinsics());
    set_property(
        &object,
        js_string!("_fd").into(),
        JsValue::from(fd as f64),
        context,
    );
    attach_method(&object, "read", file_read, context);
    attach_method(&object, "write", file_write, context);
    attach_method(&object, "close", file_close, context);
    Ok(object)
}

// ============================================================================
// FD 控制层：工作线程侧的纯操作
// ============================================================================

/// 打开文件：登记到 FD 表并返回 id。
fn fd_open_op(path: String, options: fs_core::FsOpenOptions) -> FsOutcome {
    let file = fs_core::open_file(&path, &options)?;
    let id = NEXT_FD.fetch_add(1, Ordering::Relaxed);
    fd_table()
        .lock()
        .expect("FD 表锁中毒")
        .insert(id, Arc::new(Mutex::new(file)));
    Ok(FsValue::Fd(id))
}

/// 从文件读取至多 `len` 字节。
fn fd_read_op(fd: u64, len: usize) -> FsOutcome {
    let handle = fd_table().lock().expect("FD 表锁中毒").get(&fd).cloned();
    match handle {
        Some(handle) => {
            let mut file = handle.lock().expect("文件句柄锁中毒");
            fs_core::read_chunk(&mut file, len).map(FsValue::Bytes)
        }
        None => Err(format!("无效的文件句柄：{fd}")),
    }
}

/// 向文件写入数据。
fn fd_write_op(fd: u64, data: Vec<u8>) -> FsOutcome {
    let handle = fd_table().lock().expect("FD 表锁中毒").get(&fd).cloned();
    match handle {
        Some(handle) => {
            let mut file = handle.lock().expect("文件句柄锁中毒");
            fs_core::write_chunk(&mut file, &data).map(|n| FsValue::Number(n as f64))
        }
        None => Err(format!("无效的文件句柄：{fd}")),
    }
}

/// 关闭文件：从 FD 表移除（drop 时关闭）。
fn fd_close_op(fd: u64) -> FsOutcome {
    match fd_table().lock().expect("FD 表锁中毒").remove(&fd) {
        Some(handle) => {
            drop(handle);
            Ok(FsValue::Unit)
        }
        None => Err(format!("无效的文件句柄：{fd}")),
    }
}

// ============================================================================
// File 对象方法（脚本可见）
// ============================================================================

/// `File.read(buffer)` → `Promise<number>`：读取至多 buffer 长度字节并写回。
fn file_read(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let fd = fd_of(this, context)?;
    let buffer = args
        .first()
        .and_then(|value| value.as_object())
        .ok_or_else(|| JsNativeError::typ().with_message("File.read 需要 Uint8Array 缓冲区"))?;
    let len = JsUint8Array::from_object(buffer.clone())
        .map_err(|_| JsNativeError::typ().with_message("File.read 缓冲区必须是 Uint8Array"))?
        .length(context)?;
    spawn(move || fd_read_op(fd, len), Some(buffer), context)
}

/// `File.write(data)` → `Promise<number>`：写入 Uint8Array，返回写入字节数。
fn file_write(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let fd = fd_of(this, context)?;
    let data = arg_bytes(args, 0, "File.write", context)?;
    spawn(move || fd_write_op(fd, data), None, context)
}

/// `File.close()` → `Promise<void>`。
fn file_close(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let fd = fd_of(this, context)?;
    spawn(move || fd_close_op(fd), None, context)
}

// ============================================================================
// Convenience 层（脚本可见，全部异步）
// ============================================================================

/// `open(path, mode?)` → `Promise<File>`，mode 为 `"r"`/`"w"`/`"a"`。
fn open(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "open")?;
    let mode = args
        .get(1)
        .and_then(|value| value.as_string())
        .map(|text| text.to_std_string_escaped())
        .unwrap_or_else(|| String::from("r"));
    let options = fs_core::FsOpenOptions::from_mode(&mode);
    spawn(move || fd_open_op(path, options), None, context)
}

/// `readFile(path)` → `Promise<Uint8Array>`
fn read_file(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "readFile")?;
    spawn(
        move || fs_core::read_file(&path).map(FsValue::Bytes),
        None,
        context,
    )
}

/// `readTextFile(path)` → `Promise<string>`
fn read_text_file(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "readTextFile")?;
    spawn(
        move || fs_core::read_text_file(&path).map(FsValue::Text),
        None,
        context,
    )
}

/// `writeFile(path, data)` → `Promise<void>`
fn write_file(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "writeFile")?;
    let data = arg_bytes(args, 1, "writeFile", context)?;
    spawn(
        move || fs_core::write_file(&path, &data).map(|()| FsValue::Unit),
        None,
        context,
    )
}

/// `writeTextFile(path, text)` → `Promise<void>`
fn write_text_file(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "writeTextFile")?;
    let text = arg_string(args, 1, "writeTextFile")?;
    spawn(
        move || fs_core::write_text_file(&path, &text).map(|()| FsValue::Unit),
        None,
        context,
    )
}

/// `stat(path)` → `Promise<FileInfo>`
fn stat(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "stat")?;
    spawn(
        move || fs_core::stat(&path).map(FsValue::Info),
        None,
        context,
    )
}

/// `exists(path)` → `Promise<boolean>`
fn exists(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "exists")?;
    spawn(
        move || fs_core::exists(&path).map(FsValue::Bool),
        None,
        context,
    )
}

/// `mkdir(path)` → `Promise<void>`
fn mkdir(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "mkdir")?;
    spawn(
        move || fs_core::mkdir(&path).map(|()| FsValue::Unit),
        None,
        context,
    )
}

/// `readdir(path)` → `Promise<string[]>`
fn readdir(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "readdir")?;
    spawn(
        move || fs_core::readdir(&path).map(FsValue::List),
        None,
        context,
    )
}

/// `remove(path)` → `Promise<void>`
fn remove(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = arg_string(args, 0, "remove")?;
    spawn(
        move || fs_core::remove(&path).map(|()| FsValue::Unit),
        None,
        context,
    )
}

/// `copyFile(src, dest)` → `Promise<void>`
fn copy_file(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let src = arg_string(args, 0, "copyFile")?;
    let dest = arg_string(args, 1, "copyFile")?;
    spawn(
        move || fs_core::copy_file(&src, &dest).map(|()| FsValue::Unit),
        None,
        context,
    )
}

/// `rename(oldPath, newPath)` → `Promise<void>`
fn rename(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let old = arg_string(args, 0, "rename")?;
    let new_path = arg_string(args, 1, "rename")?;
    spawn(
        move || fs_core::rename(&old, &new_path).map(|()| FsValue::Unit),
        None,
        context,
    )
}

// ============================================================================
// 构建 @std/fs 合成模块
// ============================================================================

/// `@std/fs` 导出函数的统一签名。
type NativeMethod = fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>;

/// `@std/fs` 导出的函数表：脚本名与实现。
const EXPORTS: &[(&str, NativeMethod)] = &[
    ("open", open),
    ("readFile", read_file),
    ("readTextFile", read_text_file),
    ("writeFile", write_file),
    ("writeTextFile", write_text_file),
    ("stat", stat),
    ("exists", exists),
    ("mkdir", mkdir),
    ("readdir", readdir),
    ("remove", remove),
    ("copyFile", copy_file),
    ("rename", rename),
];

/// 构建 `@std/fs` 合成模块。由 `loader.rs::build_builtin` 在 `name == "fs"` 时调用。
pub(crate) fn build_fs_module(context: &mut Context) -> JsResult<boa_engine::Module> {
    use boa_engine::Module;
    use boa_engine::module::SyntheticModuleInitializer;
    use boa_engine::object::FunctionObjectBuilder;

    let export_names: Vec<boa_engine::JsString> =
        EXPORTS.iter().map(|(name, _)| js_string!(*name)).collect();

    let initializer = SyntheticModuleInitializer::from_copy_closure(move |synthetic, ctx| {
        for (name, method) in EXPORTS {
            let function =
                FunctionObjectBuilder::new(ctx.realm(), NativeFunction::from_fn_ptr(*method))
                    .name(js_string!(*name))
                    .build();
            synthetic.set_export(&js_string!(*name), function.into())?;
        }
        Ok(())
    });

    Ok(Module::synthetic(
        &export_names,
        initializer,
        Some(std::path::PathBuf::from("@std/fs")),
        None,
        context,
    ))
}
