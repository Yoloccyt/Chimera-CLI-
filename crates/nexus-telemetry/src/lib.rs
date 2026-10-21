//! CRATE-CONTRACT BEGIN
//! LAYER:    L1
//! ROLE:     NEXUS-OMEGA L0-B 核心基础面 — 计算面遥测契约与上报编排（ADR-222 D3）
//! BACKEND:  memory
//! PRODUCERS: 0 -
//! CONSUMERS: 1 nexus-core
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! NEXUS-OMEGA L0-B 核心基础面 — 计算面遥测契约与上报编排（ADR-222 D3）
//!
//! 对应架构层: **L0-B Core Foundation**（机器层号 1；与 `nexus-core`/`event-bus` 同亚层）
//! 对应 ADR: **ADR-222**（L0 双层边界）/ **ADR-197**（计算面可观测闭环）
//!
//! # 职责边界（与 nexus-core 的分工）
//!
//! - 本 crate：**无状态**的判定纯函数、env 阈值/策略解析、上报循环，以及
//!   [`SaturationSource`] 契约（依赖倒置的接缝）；
//! - `nexus-core::compute::telemetry`：有状态的 `KindCounters` 与
//!   `ComputeTelemetry` 适配器（持有采样器/校准器/计算桥）。
//!
//! 依赖方向固定为 `nexus-core -> nexus-telemetry`；本 crate **不得**依赖
//! `nexus-core`，否则 `KindCounters`（用 `TaskKind`）与本 crate 的判定函数
//! 会形成 crate 级环。该方向由 G-125/G-126 守护。
//!
//! # 纯度口径
//!
//! 本 crate 属 **L0-B**，**不是** L0-A 纯契约：允许 tokio / tracing / env 读取。
//! L0-A 的依赖白名单（serde/chrono/uuid/thiserror）与 G-99 不适用于本 crate
//! （ADR-222 D3 的诚实边界）。
//!
//! # 数值纪律
//!
//! 比率一律用**千分比 `u32`**，不进浮点相等比较；`u64` 大数百分比以 `f64`
//! 作中间值（项目红线）。这些规则从 `nexus-core` 的 ADR-197 实现原样继承。

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod mode;
pub mod report;
pub mod source;
pub mod threshold;

pub use mode::{parse_saturation_mode, SaturationMode};
pub use report::{
    clamped_period, report_needed, reporter_tick, saturation_ratio_bp, spawn_saturation_reporter,
    TickReport,
};
pub use source::{CalibrationEvidence, SaturationSource};
pub use threshold::{
    parse_threshold_bp, saturation_mode, threshold_bp, DEFAULT_SATURATION_BP, THRESHOLD_BP_RANGE,
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    // 占用率、阈值解析、去抖三张表驱动用例已搬到 `tests/contract.rs`：
    // 它们只走公共 API，放外部视图能额外证明「外部 crate 确实拿得到这些入口」，
    // 而 crate 内单测证明不了这一点（也不在此重复一份断言）。

    /// 处置策略解析表：三态合法值、非法值与旧严格开关兼容
    #[test]
    fn parse_saturation_mode_table() {
        use SaturationMode::{Degrade, Observe, Strict};
        assert_eq!(
            parse_saturation_mode(None, None),
            Observe,
            "默认必须是不改分支的 Observe"
        );
        assert_eq!(parse_saturation_mode(Some("observe"), None), Observe);
        assert_eq!(
            parse_saturation_mode(Some("DEGRADE"), None),
            Degrade,
            "大小写不敏感"
        );
        assert_eq!(parse_saturation_mode(Some(" inline "), None), Degrade);
        assert_eq!(parse_saturation_mode(Some("strict"), None), Strict);
        assert_eq!(parse_saturation_mode(Some("error"), None), Strict);
        // 非法值不猜意图:回到零行为变更默认态
        assert_eq!(parse_saturation_mode(Some("yes-please"), None), Observe);
        assert_eq!(parse_saturation_mode(Some(""), None), Observe);
        // 旧开关仍生效,且仅当新变量未设置时
        assert_eq!(parse_saturation_mode(None, Some("1")), Strict);
        assert_eq!(parse_saturation_mode(None, Some("true")), Strict);
        assert_eq!(parse_saturation_mode(None, Some("0")), Observe);
        assert_eq!(
            parse_saturation_mode(Some("degrade"), Some("1")),
            Degrade,
            "新变量优先于旧变量"
        );
    }

    /// 对象安全：上报器以 `Arc<dyn SaturationSource>` 持有源，编译期锁死。
    #[test]
    fn saturation_source_is_object_safe() {
        struct Probe;
        impl SaturationSource for Probe {
            fn sample_and_report(&self, _bus: &event_bus::EventBus) {}
            fn apply_calibration(&self) -> usize {
                0
            }
            fn calibration_evidence(&self) -> Vec<CalibrationEvidence> {
                Vec::new()
            }
        }
        let boxed: Box<dyn SaturationSource> = Box::new(Probe);
        assert_eq!(boxed.apply_calibration(), 0);
        assert!(boxed.calibration_evidence().is_empty());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// 占用率对 active 单调不减(固定 threads)
        ///
        /// WHY 先 `let` 再断言:`prop_assert!` 会把表达式文本化并进格式串,
        /// 含 `{}` 的 if/else 会被误读为占位符（invalid format string）。
        #[test]
        fn ratio_monotone_in_active(a in 0usize..1_000_000, b in 0usize..1_000_000, t in 1usize..64) {
            let ra = saturation_ratio_bp(a, t);
            let rb = saturation_ratio_bp(b, t);
            let ok = if a <= b { ra <= rb } else { ra >= rb };
            prop_assert!(ok);
        }

        /// 占用率对 threads 单调不增(active 固定,且 threads>=1)
        #[test]
        fn ratio_antitone_in_threads(a in 0usize..100_000, t1 in 1usize..64, t2 in 1usize..64) {
            let lo = saturation_ratio_bp(a, t1.min(t2));
            let hi = saturation_ratio_bp(a, t1.max(t2));
            prop_assert!(lo >= hi);
        }

        /// 零 active 恒为 0,零 threads 不 panic
        #[test]
        fn zero_active_is_zero(threads in 0usize..256) {
            prop_assert_eq!(saturation_ratio_bp(0, threads), 0);
        }

        /// 阈值解析永远落在安全区间或回落默认值,绝不接受"0=永不降级"
        #[test]
        fn threshold_never_zero_or_huge(raw in proptest::option::of(0u32..20_001)) {
            let text = raw.map(|v| v.to_string());
            let bp = parse_threshold_bp(text.as_deref());
            prop_assert!(bp == DEFAULT_SATURATION_BP || THRESHOLD_BP_RANGE.contains(&bp));
        }
    }
}
