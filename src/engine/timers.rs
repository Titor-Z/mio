//! `setTimeout` / `setInterval` / `queueMicrotask` 的引擎绑定层。
//!
//! 设计同 `fetch`：登记表是引擎线程的 thread-local，宿主事件循环
//! （`BoaEngine::drive_event_loop`）按最近到期时间阻塞等待并触发。
//! `setInterval` 会持续保持事件循环存活（与浏览器语义一致），全局预算
//! `run.timeoutMs` 耗尽时所有未触发的定时器被静默丢弃。
//!
//! `queueMicrotask` 走 Boa 的 `GenericJob`，与 Promise 微任务同队列同序。

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use boa_engine::job::{GenericJob, Job};
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::JsFunction;
use boa_engine::property::Attribute;
use boa_engine::realm::Realm;
use boa_engine::{Context, JsArgs, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

/// 一个已登记、尚未取消的定时器。
struct TimerEntry {
    id: u64,
    callback: JsFunction,
    args: Vec<JsValue>,
    due: Instant,
    /// `Some(ms)` 为 interval（触发后重排），`None` 为一次性 timeout。
    interval: Option<u64>,
}

thread_local! {
    static TIMERS: RefCell<Vec<TimerEntry>> = const { RefCell::new(Vec::new()) };
    static NEXT_TIMER_ID: Cell<u64> = const { Cell::new(1) };
    /// 回调执行期间被 clear 的 interval 序号：槽位已从 TIMERS 摘出、clear 找不到，
    /// 记入此集合，重排前检查以实现「回调内 clear 自己」的规范语义。
    static CANCELLED: RefCell<std::collections::HashSet<u64>> =
        RefCell::new(std::collections::HashSet::new());
}

/// 安装五个定时器全局；由 `BoaEngine::with_transpile` 调用。
pub(super) fn install(context: &mut Context) {
    install_timeout(context, "setTimeout", false);
    install_timeout(context, "setInterval", true);
    // HTML 规范：clearTimeout 与 clearInterval 行为等价，共用一张登记表。
    install_clear(context, "clearTimeout");
    install_clear(context, "clearInterval");
    install_queue_microtask(context);
}

/// 最早定时器的到期时刻（事件循环据此决定阻塞时长与是否继续存活）。
pub(super) fn next_deadline() -> Option<Instant> {
    TIMERS.with(|list| list.borrow().iter().map(|entry| entry.due).min())
}

/// 触发全部到期定时器（interval 触发后重排）。
///
/// 回调抛错即向上传播（v1 语义：定时器异常是显式失败而非静默吞掉）。
pub(super) fn fire_due(context: &mut Context) -> JsResult<()> {
    let now = Instant::now();
    while let Some(entry) = TIMERS.with(|list| take_due(list, now)) {
        let result = entry
            .callback
            .call(&JsValue::undefined(), &entry.args, context);
        if entry.interval.is_some() && CANCELLED.with(|set| set.borrow_mut().remove(&entry.id)) {
            // 回调执行期间被 clear：放弃重排，但保留回调的抛错传播。
            return result.map(|_| ());
        }
        if let Some(ms) = entry.interval {
            // 重排：从本次触发起算，慢触发不会造成连环补发。
            TIMERS.with(|list| {
                list.borrow_mut().push(TimerEntry {
                    id: entry.id,
                    callback: entry.callback.clone(),
                    args: entry.args.clone(),
                    due: Instant::now() + Duration::from_millis(ms),
                    interval: entry.interval,
                });
            });
        }
        result?;
    }
    Ok(())
}

/// 预算耗尽：丢弃全部定时器。
pub(super) fn clear_all() {
    TIMERS.with(|list| list.borrow_mut().clear());
    CANCELLED.with(|set| set.borrow_mut().clear());
}

fn take_due(list: &RefCell<Vec<TimerEntry>>, now: Instant) -> Option<TimerEntry> {
    let index = list.borrow().iter().position(|entry| entry.due <= now)?;
    Some(list.borrow_mut().remove(index))
}

fn install_timeout(context: &mut Context, name: &'static str, repeat: bool) {
    let register = NativeFunction::from_copy_closure(move |_this, args, _context| {
        let Some(callback) = args.get_or_undefined(0).as_function() else {
            return Err(JsNativeError::typ()
                .with_message(format!("{name} expects a callback function"))
                .into());
        };
        let delay_ms = timer_delay(args.get_or_undefined(1));
        let id = NEXT_TIMER_ID.with(|cell| {
            let next = cell.get();
            cell.set(next + 1);
            next
        });
        // HTML 规范：setTimeout(f, ms, ...args) 的第三参数起为回调实参。
        let extra = if args.len() > 2 {
            args[2..].to_vec()
        } else {
            Vec::new()
        };
        TIMERS.with(|list| {
            list.borrow_mut().push(TimerEntry {
                id,
                callback,
                args: extra,
                due: Instant::now() + Duration::from_millis(delay_ms),
                interval: repeat.then_some(delay_ms),
            });
        });
        Ok(JsValue::from(id))
    });
    let function = FunctionObjectBuilder::new(context.realm(), register)
        .name(js_string!(name))
        .build();
    context
        .register_global_property(js_string!(name), function, Attribute::all())
        .expect("安装定时器全局失败");
}

/// HTML 的 timer timeout 规范化：非数字 / NaN / 负值归零。
fn timer_delay(value: &JsValue) -> u64 {
    value
        .as_number()
        .filter(|number| number.is_finite() && *number > 0.0)
        .map(|number| number as u64)
        .unwrap_or(0)
}

fn install_clear(context: &mut Context, name: &'static str) {
    let clear = NativeFunction::from_copy_closure(move |_this, args, _context| {
        let Some(id) = args.get_or_undefined(0).as_number() else {
            return Ok(JsValue::undefined());
        };
        let id = id as u64;
        let removed = TIMERS.with(|list| {
            let mut list = list.borrow_mut();
            if let Some(index) = list.iter().position(|entry| entry.id == id) {
                list.remove(index);
                true
            } else {
                false
            }
        });
        if !removed {
            // 可能正处在该 interval 的回调执行中：记录待死信号，重排时消费。
            CANCELLED.with(|set| set.borrow_mut().insert(id));
        }
        Ok(JsValue::undefined())
    });
    let function = FunctionObjectBuilder::new(context.realm(), clear)
        .name(js_string!(name))
        .build();
    context
        .register_global_property(js_string!(name), function, Attribute::all())
        .expect("安装清除定时器全局失败");
}

fn install_queue_microtask(context: &mut Context) {
    let enqueue = NativeFunction::from_copy_closure(|_this, args, context| {
        let Some(callback) = args.get_or_undefined(0).as_function() else {
            return Err(JsNativeError::typ()
                .with_message("queueMicrotask expects a callback function")
                .into());
        };
        let realm: Realm = context.realm().clone();
        let job = GenericJob::new(
            move |context| callback.call(&JsValue::undefined(), &[], context),
            realm,
        );
        context.enqueue_job(Job::GenericJob(job));
        Ok(JsValue::undefined())
    });
    let function = FunctionObjectBuilder::new(context.realm(), enqueue)
        .name(js_string!("queueMicrotask"))
        .build();
    context
        .register_global_property(js_string!("queueMicrotask"), function, Attribute::all())
        .expect("安装 queueMicrotask 全局失败");
}
