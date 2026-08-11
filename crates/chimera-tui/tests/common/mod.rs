//! 集成测试共享工具模块(Concord W4 T4.3)
//!
//! 用法:测试文件顶部 `#[macro_use] mod common;` 即可使用 `scaled_timeout!`。
//!
//! # scaled_timeout! — 异步测试统一超时护栏
//! chimera-tui 此前零超时护栏(P8 盲区):异步测试卡死会拖垮整个测试进程。
//! 本宏按构建档位差异化超时——debug 构建开销大给 ×4 余量,release 给 ×1.5,
//! 避免"为防 flaky 一刀切放大超时"掩盖真实性能退化。

/// 按构建档位缩放超时时长
///
/// # 参数
/// - `$base_secs`:release 档位基准秒数(表达式,支持浮点)
///
/// # 返回
/// `std::time::Duration`——debug 为基准 ×4,release 为基准 ×1.5。
///
/// # 示例
/// ```ignore
/// let t = scaled_timeout!(2);       // debug: 8s / release: 3s
/// tokio::time::timeout(scaled_timeout!(0.5), fut).await
/// ```
#[macro_export]
macro_rules! scaled_timeout {
    ($base_secs:expr) => {{
        let base: f64 = $base_secs;
        if cfg!(debug_assertions) {
            std::time::Duration::from_millis((base * 4000.0) as u64)
        } else {
            std::time::Duration::from_millis((base * 1500.0) as u64)
        }
    }};
}
