//! `performance` 全局的引擎绑定：单调、亚毫秒分辨率的计时时钟。
//!
//! 存在的理由是给定时器一个可测量的量具：`Date.now()` 是墙上时钟、毫秒粒度且会被
//! 系统调整，量不出调度抖动。本时钟取自 `Instant`，只增不减、纳秒粒度，输出为浮点
//! 毫秒（`web` 模块约定 2 的时间形态）。
//!
//! v1 边界：只有 `now()`，无 `timeOrigin`、标记点（mark/measure）与资源条目。

use std::sync::OnceLock;
use std::time::Instant;

use boa_engine::property::Attribute;
use boa_engine::{Context, JsObject, JsResult, JsValue, js_string};

use super::support::attach_method;

/// 计时起点。安装 `performance` 时初始化，因此读数近似于运行时启动时刻。
fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// 安装 `performance` 全局。
pub(super) fn install(context: &mut Context) {
    // 先固化起点，避免首次 `now()` 调用才初始化导致时间轴原点漂到脚本执行中途。
    let _ = origin();

    let object = JsObject::default(context.intrinsics());
    attach_method(&object, "now", now_method, context);
    context
        .register_global_property(js_string!("performance"), object, Attribute::all())
        .expect("安装 performance 全局失败");
}

/// `performance.now()`：自起点起的浮点毫秒（微秒精度，与 DOMHighResTimeStamp 同形）。
fn now_method(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let micros = origin().elapsed().as_nanos() as f64 / 1_000.0;
    Ok(JsValue::from(micros.round() / 1_000.0))
}
