//! `setTimeout` / `setInterval` / `queueMicrotask` 的引擎绑定层。
//!
//! 设计同 `fetch`：登记表是引擎线程的 thread-local，宿主事件循环
//! （`BoaEngine::drive_event_loop`）按最近到期时间阻塞等待并触发。
//! `setInterval` 会持续保持事件循环存活（与浏览器语义一致），全局预算
//! `run.timeoutMs` 耗尽时所有未触发的定时器被丢弃。
//!
//! 调度结构是「到期时刻最小堆 + 条目表」：事件循环每轮都要问最近到期时间，
//! 线性扫描会让成本随定时器数量增长。被清除的定时器在堆里留下陈旧键，
//! 浮到堆顶时惰性丢弃，于是清除也是常数级。
//!
//! 周期定时器**锚定在计划时刻的整数格**上重排，而非「实际触发时刻 + 周期」：
//! 后者会把每格的调度抖动累加进后续格子，长跑后周期彻底漂移。逾期未触发的格子
//! 直接丢弃（浏览器口径，不补发）。
//!
//! `queueMicrotask` 走 Boa 的 `GenericJob`，与 Promise 微任务同队列同序。

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::time::{Duration, Instant};

use boa_engine::job::{GenericJob, Job};
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::builtins::JsFunction;
use boa_engine::property::Attribute;
use boa_engine::realm::Realm;
use boa_engine::{Context, JsArgs, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

/// 一个已登记、尚未取消的定时器。
struct TimerEntry {
    callback: JsFunction,
    args: Vec<JsValue>,
    /// 本格的计划时刻；周期定时器据它锚定下一格。
    scheduled: Instant,
    /// `Some(period)` 为 interval（触发后重排），`None` 为一次性 timeout。
    period: Option<Duration>,
}

/// 最小堆的键：按到期时刻排序，同刻再按登记序号保序。
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
struct TimerKey {
    due: Instant,
    id: u64,
}

thread_local! {
    /// 定时器条目，按 id 索引；`take_due` 取出后即不在表内。
    static ENTRIES: RefCell<HashMap<u64, TimerEntry>> = RefCell::new(HashMap::new());
    /// 到期时刻堆。每个 id 在堆里至多一个键，所以「条目不在表内」即陈旧键的判据。
    static KEYS: RefCell<BinaryHeap<Reverse<TimerKey>>> = const { RefCell::new(BinaryHeap::new()) };
    static NEXT_TIMER_ID: Cell<u64> = const { Cell::new(1) };
    /// 回调执行期间被 clear 的 interval 序号：槽位已从 ENTRIES 摘出、clear 找不到，
    /// 记入此集合，重排前检查以实现「回调内 clear 自己」的规范语义。
    static CANCELLED: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
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
    ENTRIES.with(|entries| {
        KEYS.with(|keys| {
            let entries = entries.borrow();
            let mut keys = keys.borrow_mut();
            prune_stale(&entries, &mut keys);
            keys.peek().map(|Reverse(key)| key.due)
        })
    })
}

/// 触发全部到期定时器（interval 触发后按格锚定重排）。
///
/// 到期判定用进入本轮时的同一个 `now`：回调里新建的 0ms 定时器因此留到下一轮，
/// 自递归的 `setTimeout(f, 0)` 不会在本轮里转成死循环。重排锚点则取回调返回后的
/// 时刻，于是迟到的 interval 至多补发一格便回到正常刻度，且零周期也不会自旋。
///
/// 回调抛错即向上传播（v1 语义：定时器异常是显式失败而非静默吞掉）。
pub(super) fn fire_due(context: &mut Context) -> JsResult<()> {
    let now = Instant::now();
    while let Some((id, entry)) = take_due(now) {
        let TimerEntry {
            callback,
            args,
            scheduled,
            period,
        } = entry;
        let result = callback.call(&JsValue::undefined(), &args, context);
        // 回调执行期间被 clear：放弃重排（并消费掉待死信号，不泄漏给后续同名 id）。
        let cancelled_in_callback =
            period.is_some() && CANCELLED.with(|set| set.borrow_mut().remove(&id));
        if let (Some(period), false) = (period, cancelled_in_callback) {
            // 用回调返回后的时刻做锚点：本轮的 `now` 早于回调耗时，拿它算出的下一格
            // 可能仍不晚于 `now`，零周期 interval 会在本轮里自旋成死循环。
            let due = next_due(scheduled, period, Instant::now());
            ENTRIES.with(|entries| {
                entries.borrow_mut().insert(
                    id,
                    TimerEntry {
                        callback: callback.clone(),
                        args,
                        scheduled: due,
                        period: Some(period),
                    },
                )
            });
            KEYS.with(|keys| keys.borrow_mut().push(Reverse(TimerKey { due, id })));
        }
        result?;
    }
    Ok(())
}

/// 预算耗尽：丢弃全部定时器。
pub(super) fn clear_all() {
    ENTRIES.with(|entries| entries.borrow_mut().clear());
    KEYS.with(|keys| keys.borrow_mut().clear());
    CANCELLED.with(|set| set.borrow_mut().clear());
}

/// 丢弃堆顶那些「条目已不在表内」的陈旧键。
fn prune_stale(entries: &HashMap<u64, TimerEntry>, keys: &mut BinaryHeap<Reverse<TimerKey>>) {
    while keys
        .peek()
        .is_some_and(|Reverse(top)| !entries.contains_key(&top.id))
    {
        keys.pop();
    }
}

/// 摘出下一个到期（不晚于 `now`）的定时器，跳过陈旧键。
fn take_due(now: Instant) -> Option<(u64, TimerEntry)> {
    ENTRIES.with(|entries| {
        KEYS.with(|keys| {
            let mut entries = entries.borrow_mut();
            let mut keys = keys.borrow_mut();
            loop {
                let Reverse(top) = *keys.peek()?;
                if top.due > now {
                    return None;
                }
                keys.pop();
                if let Some(entry) = entries.remove(&top.id) {
                    return Some((top.id, entry));
                }
            }
        })
    })
}

/// 周期定时器的下一格时刻：锚定在计划时刻的整数格上，早于 `now` 的格子丢弃。
///
/// 结果是「第一个不早于 `now` 的格子」：未逾期就是紧邻下一格；逾期则向前跳到
/// `now` 所在或之后的格点，于是迟到的量既不会被补发成一串（浏览器口径），也不会
/// 转嫁给后续每一格形成累积漂移。
fn next_due(scheduled: Instant, period: Duration, now: Instant) -> Instant {
    if period.is_zero() {
        return now;
    }
    let Some(next) = scheduled.checked_add(period) else {
        return now;
    };
    if next >= now {
        return next;
    }
    // 跳到第一个不早于 now 的格点：now 正落在格点上时结果就是 now（本轮补发一格）。
    // std 的 Duration 只有向下取整的除法，故在纳秒上算 ceil。
    let overdue = now.saturating_duration_since(next).as_nanos();
    let cycles = u32::try_from(overdue.div_ceil(period.as_nanos())).unwrap_or(u32::MAX);
    next.checked_add(period.saturating_mul(cycles))
        .unwrap_or(now)
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
        let scheduled = Instant::now() + Duration::from_millis(delay_ms);
        ENTRIES.with(|entries| {
            entries.borrow_mut().insert(
                id,
                TimerEntry {
                    callback,
                    args: extra,
                    scheduled,
                    period: repeat.then_some(Duration::from_millis(delay_ms)),
                },
            )
        });
        KEYS.with(|keys| {
            keys.borrow_mut()
                .push(Reverse(TimerKey { due: scheduled, id }));
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
        if ENTRIES
            .with(|entries| entries.borrow_mut().remove(&id))
            .is_none()
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 各时刻都从同一基准算出，才能做精确断言：分别调用 `Instant::now()` 之间相差的
    /// 纳秒会让相等判断失败。
    fn timeline() -> impl Fn(u64) -> Instant {
        let base = Instant::now();
        move |millis| base + Duration::from_millis(millis)
    }

    #[test]
    fn on_time_tick_advances_by_exactly_one_period() {
        let at = timeline();
        let next = next_due(at(0), Duration::from_millis(20), at(1));
        assert_eq!(
            next.duration_since(at(0)),
            Duration::from_millis(20),
            "准点时下一格就是整数刻度上的下一格，不受调度余量影响"
        );
    }

    #[test]
    fn overdue_tick_skips_missed_slots_without_drifting() {
        let at = timeline();
        let period = Duration::from_millis(20);
        // 回调阻塞 75ms：逾期 3.75 格，应跳到第 4 格（80ms），不补发漏掉的 3 次。
        let next = next_due(at(0), period, at(75));
        assert_eq!(next.duration_since(at(0)), Duration::from_millis(80));
        // 再走一格仍落在 20ms 的整数刻度上：这次迟到没有转嫁成后续漂移。
        let after = next_due(next, period, at(78));
        assert_eq!(after.duration_since(at(0)), Duration::from_millis(100));
    }

    #[test]
    fn tick_landing_exactly_on_now_fires_this_round() {
        let at = timeline();
        assert_eq!(
            next_due(at(0), Duration::from_millis(20), at(40)),
            at(40),
            "落在格点上的逾期格应在本轮补发一次，不多等一周期"
        );
    }

    #[test]
    fn zero_period_interval_fires_again_at_round_start() {
        let at = timeline();
        assert_eq!(next_due(at(0), Duration::ZERO, at(5)), at(5));
    }
}
