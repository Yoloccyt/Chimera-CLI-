//! 集成测试共享工具模块(Concord W4 T4.3)
//!
//! 用法:测试文件顶部 `mod common;` 即可使用本模块下共享项(如 `mock::MockDataSource`),
//! `#[macro_use] mod common;` 同时引入 `build_scaled_timeout!`。
//!
//! # build_scaled_timeout! — 异步测试统一超时护栏
//! chimera-tui 此前零超时护栏(P8 盲区):异步测试卡死会拖垮整个测试进程。
//! 本宏按构建档位差异化超时——debug 构建开销大给 ×4 余量,release 给 ×1.5,
//! 避免"为防 flaky 一刀切放大超时"掩盖真实性能退化。
//! 本宏为档位放大语义(debug×4/release×1.5),与 nexus-contracts 共享版
//! `scaled_timeout!`(env scale 缩放)语义不同,更名为 `build_scaled_timeout!`
//! 避免同名混用。

/// 按构建档位缩放超时时长
///
/// # 参数
/// - `$base_secs`:release 档位基准秒数(表达式,支持浮点)
///
/// # 返回
/// `std::time::Duration`——debug 为基准 ×4,release 为基准 ×1.5。
///
/// # 语义区分
/// 本宏为档位放大语义(debug×4/release×1.5),与 nexus-contracts 共享版
/// `scaled_timeout!`(env scale 缩放)语义不同,更名为 `build_scaled_timeout!`
/// 避免同名混用。
///
/// # 示例
/// ```ignore
/// let t = build_scaled_timeout!(2);   // debug: 8s / release: 3s
/// tokio::time::timeout(build_scaled_timeout!(0.5), fut).await
/// ```
#[macro_export]
macro_rules! build_scaled_timeout {
    ($base_secs:expr) => {{
        let base: f64 = $base_secs;
        if cfg!(debug_assertions) {
            std::time::Duration::from_millis((base * 4000.0) as u64)
        } else {
            std::time::Duration::from_millis((base * 1500.0) as u64)
        }
    }};
}

/// 共享 mock 替身(R14 收敛)。
///
/// 注意:本模块仅部分 test-crate(4 个使用 MockDataSource 的集成测试)实际消费,
/// 其余 test-crate(如 overwindow/history_wiring 仅用 build_scaled_timeout!)编译
/// 进入 `mod common;` 但不用 mock——故以 `#[allow(dead_code)]` 抑制共享测试模块
/// 在未消费 crate 中的 dead_code 警告;模块内真实未使用项仍照常报警。
#[allow(dead_code)]
pub mod mock;
