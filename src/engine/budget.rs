//! 运行预算看门狗：让 `run.timeoutMs` 也能约束微任务排空。
//!
//! 动机：`SimpleJobExecutor::run_jobs` 会把队列排空到底，自递归的微任务
//! （`queueMicrotask(f)` 里再 `queueMicrotask(f)`）永远排不空，预算形同虚设，
//! 进程只能靠 Ctrl-C 结束。Boa 为这种情况留了出口：执行器的取消令牌置真后，
//! 排空循环在下一轮开头退出并丢弃队列。看门狗负责在预算到点时按下这个键。
//!
//! 线性化约定：看门狗线程**只在持有互斥锁时**写停止键，`disarm` 在同一把锁下
//! 置结束标志。因此宿主侧「先 disarm、再清键、后做优雅收尾」的顺序不会被
//! 迟到的按键打断——这条顺序是必需的，见 `BoaEngine::finish_with_timeout`。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// 按下停止键后，若宿主仍卡在排空里，隔这么久再按一次。
const REPEAT: Duration = Duration::from_millis(200);

/// 停止键的写入口：即 `SimpleJobExecutor::get_cancellation_token()`。
///
/// Boa 的令牌用的是 `portable_atomic::AtomicBool`（不同于 `std` 的同名类型），
/// 在此包成一层 `Fn(bool)` 闭包，预算模块就不必为类型名字一个额外依赖。
pub(super) type StopKey = Arc<dyn Fn(bool) + Send + Sync + 'static>;

#[derive(Debug)]
struct Inner {
    /// 宿主已结束本次运行，看门狗应当退出。
    finished: Mutex<bool>,
    cvar: Condvar,
}

/// 看门狗句柄：可克隆，供宿主在持有 `&mut self` 的间隙里查询与解除。
#[derive(Clone)]
pub(super) struct WatchdogHandle {
    inner: Arc<Inner>,
    expired: Arc<AtomicBool>,
}

impl WatchdogHandle {
    /// 预算是否已被掐断（即最近一次微任务排空是被中止的，而非自然排空）。
    pub(super) fn expired(&self) -> bool {
        self.expired.load(Ordering::SeqCst)
    }

    /// 停止看守：之后不会再按下停止键。
    pub(super) fn disarm(&self) {
        let mut finished = self
            .inner
            .finished
            .lock()
            .expect("看门狗锁已被中毒的线程污染");
        *finished = true;
        drop(finished);
        self.inner.cvar.notify_one();
    }
}

/// 运行中的看门狗。持有它即持有看守线程；`Drop` 自动解除。
pub(super) struct Watchdog {
    handle: WatchdogHandle,
}

impl Watchdog {
    /// 开始看守：到 `deadline` 就掐断引擎的微任务排空，直到句柄被解除或运行结束。
    pub(super) fn spawn(deadline: Instant, job_stop: StopKey) -> Watchdog {
        let handle = WatchdogHandle {
            inner: Arc::new(Inner {
                finished: Mutex::new(false),
                cvar: Condvar::new(),
            }),
            expired: Arc::new(AtomicBool::new(false)),
        };
        let watcher = Watchdog {
            handle: handle.clone(),
        };

        let inner = Arc::clone(&handle.inner);
        let expired = Arc::clone(&handle.expired);
        std::thread::spawn(move || {
            let mut next_fire = deadline;
            loop {
                let mut finished = inner.finished.lock().expect("看门狗锁已被中毒的线程污染");
                // 等待期间释放锁，被 notify 或到点唤醒后重新持有。
                while !*finished && Instant::now() < next_fire {
                    let remain = next_fire.saturating_duration_since(Instant::now());
                    let (guard, _) = inner
                        .cvar
                        .wait_timeout(finished, remain)
                        .expect("看门狗锁已被中毒的线程污染");
                    finished = guard;
                }
                if *finished {
                    return;
                }
                expired.store(true, Ordering::SeqCst);
                (job_stop)(true);
                next_fire = Instant::now() + REPEAT;
            }
        });

        watcher
    }

    /// 取得查询与解除用的句柄。
    pub(super) fn handle(&self) -> WatchdogHandle {
        self.handle.clone()
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.handle.disarm();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的停止键：写入一个 `std` 原子开关。
    fn stop_key(flag: Arc<AtomicBool>) -> StopKey {
        Arc::new(move |value| flag.store(value, Ordering::SeqCst))
    }

    fn watched_flag() -> (Arc<AtomicBool>, Watchdog) {
        let flag = Arc::new(AtomicBool::new(false));
        let watchdog = Watchdog::spawn(
            Instant::now() + Duration::from_millis(10),
            stop_key(Arc::clone(&flag)),
        );
        (flag, watchdog)
    }

    #[test]
    fn fires_stop_key_after_deadline() {
        let (flag, _watchdog) = watched_flag();
        // 到点即按键；给看门狗线程一点调度余量。
        for _ in 0..100 {
            if flag.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("预算已耗尽，停止键却没被按下");
    }

    #[test]
    fn disarm_prevents_later_fire() {
        let flag = Arc::new(AtomicBool::new(false));
        let watchdog = Watchdog::spawn(
            Instant::now() + Duration::from_millis(30),
            stop_key(Arc::clone(&flag)),
        );
        let handle = watchdog.handle();
        handle.disarm();
        std::thread::sleep(Duration::from_millis(80));
        assert!(
            !flag.load(Ordering::SeqCst),
            "已解除看守后不得再按下停止键：优雅收尾会被误杀"
        );
        assert!(!handle.expired());
    }

    #[test]
    fn drop_disarms() {
        let (flag, watchdog) = watched_flag();
        drop(watchdog);
        std::thread::sleep(Duration::from_millis(60));
        assert!(!flag.load(Ordering::SeqCst), "看门狗随作用域结束而解除");
    }
}
