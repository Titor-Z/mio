//! 基于 Boa 的执行引擎实现（阶段 A）。
//!
//! 这是阶段 A 与阶段 B 之间第一个接缝的具体实现：`boa_*` 只允许出现在本模块，
//! 上层只通过 [`Engine`] 抽象与本文件交互。替换阶段 B 的自研虚拟机时，只需新增
//! 一个实现 [`Engine`] 的类型，上游命令行与加载逻辑不受影响。
//!
//! 当前实现覆盖里程碑 1 的垂直切片：读取单文件→转译擦除类型→以 ES Module 语义求值
//! →驱动微任务与宿主事件循环（在途 fetch 的等待与 settle、定时器到期触发）；
//! 入口模块预填入 [`TsModuleLoader`] 缓存，相对 `import` 经解析后递归转译加载；
//! `@std/*` 内建模块与 `fetch`/`console`/定时器全局已接入。

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use boa_engine::Module;
use boa_engine::builtins::promise::PromiseState;
use boa_engine::job::SimpleJobExecutor;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::property::{Attribute, PropertyDescriptor};
use boa_engine::{Context, JsError, JsObject, JsValue, NativeFunction, Source, js_string};

use super::budget::{StopKey, Watchdog, WatchdogHandle};
use super::fetch::{self as engine_fetch, FetchResult, LoopEvent};
use super::http as engine_http;
use super::loader::TsModuleLoader;
use super::timers as engine_timers;
use super::{Engine, EngineError};
use crate::resolver::ModuleResolver;
use crate::transpiler::{TranspileConfig, Transpiler};

/// Boa 引擎包装。持有唯一的活动 [`Context`]、共享的模块加载器与 fetch 完成通道。
pub struct BoaEngine {
    context: Context,
    loader: Rc<TsModuleLoader>,
    transpile: TranspileConfig,
    fetch_rx: Receiver<LoopEvent>,
    timeout_ms: u64,
    /// Boa 微任务执行器的取消令牌写入口：看门狗据此掐断排不到空的微任务风暴。
    job_stop: StopKey,
    /// 本次运行的看门狗句柄；仅在 [`BoaEngine::evaluate`] 期间为 `Some`。
    budget: Option<WatchdogHandle>,
}

impl Default for BoaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl BoaEngine {
    /// 构造引擎：装配带转译加载器的 `Context`，并注入最小可观测全局（`console.log`）。
    pub fn new() -> Self {
        Self::with_resolver(ModuleResolver::default())
    }

    /// 使用给定解析器（携带 Import Maps 与基准目录）构造引擎。
    ///
    /// 命令行将 `mio.json` 的 `imports` 映射与配置目录装配成 [`ModuleResolver`] 传入，
    /// 使别名导入在运行时生效。
    pub fn with_resolver(resolver: ModuleResolver) -> Self {
        Self::with_transpile(resolver, TranspileConfig::default())
    }

    /// 同上，并指定转译参数（`target` 降级与 `sourcemap` 开关由配置层传入）。
    pub fn with_transpile(resolver: ModuleResolver, transpile: TranspileConfig) -> Self {
        let loader = Rc::new(TsModuleLoader::new(resolver, transpile.clone()));
        // 显式持有微任务执行器：只为拿到它的取消令牌（见 `budget` 模块）。
        let jobs = Rc::new(SimpleJobExecutor::new());
        // 令牌类型属于 Boa 侧（portable_atomic），包一层闭包隔离掉。
        let token = Arc::clone(&jobs.get_cancellation_token());
        let job_stop: StopKey = Arc::new(move |value| token.store(value, Ordering::SeqCst));
        let mut context = Context::builder()
            .module_loader(loader.clone())
            .job_executor(jobs)
            .build()
            .expect("构建 Boa Context 失败");
        install_console(&mut context);
        let fetch_rx = engine_fetch::install(&mut context);
        engine_http::install(&mut context);
        engine_timers::install(&mut context);
        super::blob::install(&mut context);
        super::encoding::install(&mut context);
        super::url::install(&mut context);
        super::performance::install(&mut context);
        super::response::install(&mut context);
        Self {
            context,
            loader,
            transpile,
            fetch_rx,
            timeout_ms: 30_000,
            job_stop,
            budget: None,
        }
    }

    /// 设置宿主事件循环的总预算（毫秒）：既是在途 fetch 的超时上限，
    /// 也是微任务排空的看门狗触发时刻。
    ///
    /// 由命令行从 `run.timeoutMs` 配置传入；默认 30000。
    #[must_use]
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
}

impl Engine for BoaEngine {
    fn eval_module_file(&mut self, path: &Path) -> Result<String, EngineError> {
        // 规范成绝对路径：既保证入口文件可读，又让入口的相对 import 与后续模块的
        // 解析落在同一基准目录上，缓存键因此稳定。
        let canonical = std::fs::canonicalize(path)
            .map_err(|error| EngineError::new("读取", format!("{}：{error}", path.display())))?;

        let raw = std::fs::read_to_string(&canonical).map_err(|error| {
            EngineError::new("读取", format!("{}：{error}", canonical.display()))
        })?;

        let output = Transpiler::transpile(&raw, &canonical, &self.transpile)
            .map_err(|diagnostics| EngineError::new("转译", diagnostics.join("\n")))?;
        if let Some(mapping) = output.sourcemap {
            self.loader.register_source_map(&canonical, mapping);
        }

        let module = Module::parse(
            Source::from_bytes(&output.code).with_path(&canonical),
            None,
            &mut self.context,
        )
        .map_err(|error| {
            EngineError::new("解析", self.loader.remap_error_text(&error.to_string()))
        })?;

        // 预注册入口：被间接 import 时复用同一实例，避免二次转译并支持环依赖。
        self.loader.register(canonical.clone(), module.clone());

        self.evaluate(module)
    }

    fn eval_module_source(
        &mut self,
        source: &str,
        hint: Option<&Path>,
    ) -> Result<String, EngineError> {
        let module = match hint {
            Some(path) => Module::parse(
                Source::from_bytes(source).with_path(path),
                None,
                &mut self.context,
            ),
            None => Module::parse(Source::from_bytes(source), None, &mut self.context),
        }
        .map_err(|error| {
            EngineError::new("解析", self.loader.remap_error_text(&error.to_string()))
        })?;

        self.evaluate(module)
    }

    fn eval_script(&mut self, source: &str) -> Result<String, EngineError> {
        let value = self
            .context
            .eval(Source::from_bytes(source))
            .map_err(|error| EngineError::new("运行期", error.to_string()))?;
        Ok(display_value(&value))
    }

    fn run_jobs(&mut self) -> Result<(), EngineError> {
        self.context.run_jobs().map_err(|error| {
            EngineError::new("微任务", self.loader.remap_error_text(&error.to_string()))
        })
    }
}

impl BoaEngine {
    /// 驱动模块完整生命周期（加载→链接→求值），再跑宿主事件循环直到就绪，
    /// 最后回收拒绝状态为引擎错误。
    ///
    /// 同步求值、微任务排空与事件循环共用同一个截止时刻，并由看门狗看守：
    /// 预算耗尽时它能掐断排不到空的微任务风暴，否则进程只能靠 Ctrl-C 结束。
    fn evaluate(&mut self, module: Module) -> Result<String, EngineError> {
        let deadline = Instant::now() + Duration::from_millis(self.timeout_ms);
        let watchdog = Watchdog::spawn(deadline, Arc::clone(&self.job_stop));
        self.budget = Some(watchdog.handle());
        let result = self.evaluate_inner(&module, deadline);
        self.budget = None;
        drop(watchdog);
        result
    }

    fn evaluate_inner(
        &mut self,
        module: &Module,
        deadline: Instant,
    ) -> Result<String, EngineError> {
        let promise = module.load_link_evaluate(&mut self.context);
        self.run_jobs_checked()?;
        self.drive_event_loop(deadline)?;
        if let PromiseState::Rejected(reason) = promise.state() {
            let message = JsError::from_opaque(reason).to_string();
            return Err(EngineError::new(
                "运行期",
                self.loader.remap_error_text(&message),
            ));
        }
        Ok(String::new())
    }

    /// 宿主事件循环：只要有在途 fetch 或待触发定时器就继续运转。
    ///
    /// 阻塞等待取两者优先：fetch 完成消息（到达即 settle）或最近定时器到期（到点即
    /// 触发）；每次推进后都排空微任务队列。
    /// 阻塞等待取三者优先：fetch 完成、进来的请求、或最近定时器到期；每次推进都排空微任务。
    fn drive_event_loop(&mut self, deadline: Instant) -> Result<(), EngineError> {
        loop {
            let budget = deadline.saturating_duration_since(Instant::now());
            if budget.is_zero() {
                return self.finish_with_timeout();
            }

            let has_fetch = engine_fetch::has_pending();
            let has_serve = engine_http::is_listening();
            let timer_wait = engine_timers::next_deadline()
                .map(|due| due.saturating_duration_since(Instant::now()).min(budget));
            if !has_fetch && !has_serve && timer_wait.is_none() {
                return Ok(());
            }
            // 单一等待点：事件通道（fetch 完成 + 进来的请求）或定时器超时，先到者唤醒。
            let wait = timer_wait.unwrap_or(budget);
            match self.fetch_rx.recv_timeout(wait) {
                Ok(first) => {
                    let mut batch = vec![first];
                    while let Ok(more) = self.fetch_rx.try_recv() {
                        batch.push(more);
                    }
                    self.dispatch(batch)?;
                    self.run_jobs_checked()?;
                }
                Err(RecvTimeoutError::Timeout) => {
                    // wait 恒不大于 budget：到点即预算耗尽，否则是定时器到期。
                    if Instant::now() >= deadline {
                        return self.finish_with_timeout();
                    }
                    self.fire_timers_and_drain()?;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // 通道断开：放弃等待，在途请求随进程自然终局。
        Ok(())
    }

    /// 分发一批事件：fetch 结果合并后 settle；进来的请求逐条**同步**交给 handler。
    ///
    /// 顺序：先把 fetch 结果 settle（可能入队微任务），再处理 server（调用 JS）；
    /// 二者产生的微任务统一交由调用方随后的 `run_jobs_checked` 排空。
    fn dispatch(&mut self, events: Vec<LoopEvent>) -> Result<(), EngineError> {
        let mut fetches: Vec<FetchResult> = Vec::new();
        for event in events {
            match event {
                LoopEvent::Fetch(result) => fetches.push(result),
                LoopEvent::Server(inbound) => {
                    engine_http::dispatch_server(inbound, &mut self.context)
                }
            }
        }
        if !fetches.is_empty() {
            engine_fetch::settle(&fetches, &mut self.context);
        }
        Ok(())
    }

    /// 预算耗尽的优雅收尾：让在途 fetch 以可捕获的 `TypeError` 拒绝、丢弃全部定时器，
    /// 再把剩下的微任务排空，使 JS 侧能观察到超时而不是静默退出。
    ///
    /// 顺序不可调换：先解除看门狗、再清掉它按下的停止键，之后才排空。否则
    /// `SimpleJobExecutor` 会在排空开头直接丢弃队列，顶层 `await` 的 Promise 永不
    /// 落定，超时就退化成「正常退出 0」。
    fn finish_with_timeout(&mut self) -> Result<(), EngineError> {
        if let Some(budget) = self.budget.clone() {
            budget.disarm();
            (self.job_stop)(false);
        }
        engine_fetch::reject_all_pending("fetch timed out", &mut self.context);
        engine_timers::clear_all();
        self.run_jobs()
    }

    /// 排空微任务，并检查这次排空是否被看门狗中止过——微任务风暴的唯一出口。
    fn run_jobs_checked(&mut self) -> Result<(), EngineError> {
        self.run_jobs()?;
        let expired = self.budget.as_ref().is_some_and(|budget| budget.expired());
        if expired {
            return Err(EngineError::new(
                "事件循环",
                format!(
                    "运行超出 run.timeoutMs 预算（{}ms）：微任务队列未能排空，在途请求与未触发定时器已丢弃",
                    self.timeout_ms
                ),
            ));
        }
        Ok(())
    }

    /// 触发到期定时器并排空随后的微任务；错误经源码映射包装为引擎错误。
    fn fire_timers_and_drain(&mut self) -> Result<(), EngineError> {
        engine_timers::fire_due(&mut self.context).map_err(|error| {
            EngineError::new("定时器", self.loader.remap_error_text(&error.to_string()))
        })?;
        self.run_jobs_checked()
    }
}

/// 注入 `console` 全局对象，当前仅实现 `log`。
///
/// 完整版应落在 `web` 层；在依赖隔离约束下（`boa_*` 仅限 `src/engine/`）先由本文件提供，
/// 阶段 B 引擎替换时一并迁移为与引擎无关的全局描述符。
fn install_console(context: &mut Context) {
    let log = NativeFunction::from_copy_closure(|_this, args, _ctx| {
        let line = args
            .iter()
            .map(format_for_log)
            .collect::<Vec<_>>()
            .join(" ");
        println!("{line}");
        Ok(JsValue::undefined())
    });

    let console = JsObject::default(context.intrinsics());
    let function = FunctionObjectBuilder::new(context.realm(), log)
        .name(js_string!("log"))
        .build();
    let descriptor = PropertyDescriptor::builder()
        .value(function)
        .writable(true)
        .enumerable(true)
        .configurable(true)
        .build();
    console.insert_property(js_string!("log"), descriptor);

    context
        .register_global_property(js_string!("console"), console, Attribute::all())
        .expect("安装 console 全局失败");
}

/// 把求值结果转成面向人类的文本；未定义值渲染为空串。
fn display_value(value: &JsValue) -> String {
    if value.is_undefined() {
        String::new()
    } else {
        value.display().to_string()
    }
}

/// `console.log` 的实参格式化：字符串按原样输出（不带引号），其余走调试展示。
fn format_for_log(value: &JsValue) -> String {
    match value.as_string() {
        Some(string) => string.to_std_string_escaped(),
        None => value.display().to_string(),
    }
}
