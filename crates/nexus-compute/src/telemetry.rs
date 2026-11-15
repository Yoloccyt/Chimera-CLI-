//! 计算面遥测（nexus-core 侧）— L0-B 契约的有状态适配（ADR-197 / ADR-222 D3）
//!
//! 对应架构层: **L0-B Core Foundation**（机器层号 1）
//!
//! # 三块内容的分工（ADR-222 D3）
//!
//! - **纯判定 + `SaturationSource` 契约 + 上报循环** → `nexus-telemetry`（L0-B 同层）
//! - **有状态计数器**（`KindCounters`，引用 `TaskKind`/`hts::kind_index`）→ [`super::counters`]
//! - **本模块**：把前两者接起来的适配器 [`ComputeTelemetry`] + 旧签名包装
//!   [`spawn_saturation_reporter`]（3 处装配点因此零改动）
//!
//! # 依赖方向
//!
//! `nexus-core -> nexus-telemetry` 单向；本模块**不得**被 `nexus-telemetry`
//! 反向引用，否则 `KindCounters`（用 `TaskKind`）与契约之间形成 crate 环。
//! 该方向由 G-125/G-126 机器守护（`scripts/check_l0_sublayer.py`）。

use std::sync::{Arc, OnceLock};

use event_bus::EventBus;

use super::bridge;
use super::calibration;
use super::utilization::UtilizationSampler;

// 兼容 re-export：`nexus_core::compute::{SaturationMode, saturation_ratio_bp, ...}`
// 与 `nexus_core::compute::telemetry::*` 旧路径逐字不变（拆分只是搬家，不是改 API）。
pub use super::counters::{KindCounters, KindStat};
pub use nexus_telemetry::{
    clamped_period, parse_saturation_mode, parse_threshold_bp, report_needed, reporter_tick,
    saturation_mode, saturation_ratio_bp,
    spawn_saturation_reporter as spawn_saturation_reporter_with_source, threshold_bp,
    CalibrationEvidence, SaturationMode, SaturationSource, TickReport,
};

/// 有状态遥测源 —— 把 `nexus-core` 的采样器/校准器/计算桥接到 L0-B 契约
///
/// WHY 是零尺寸类型：采样器本身是进程级 `static OnceLock`（与上提前的
/// `spawn_saturation_reporter` 完全同形），适配器只做方法转发，不持有新状态。
#[derive(Debug, Clone, Copy, Default)]
pub struct ComputeTelemetry;

impl SaturationSource for ComputeTelemetry {
    fn sample_and_report(&self, bus: &EventBus) {
        static SAMPLER: OnceLock<UtilizationSampler> = OnceLock::new();
        SAMPLER
            .get_or_init(UtilizationSampler::default)
            .sample_and_report(bus);
    }

    fn apply_calibration(&self) -> usize {
        calibration::flush(bridge::bridge())
    }

    fn calibration_evidence(&self) -> Vec<CalibrationEvidence> {
        calibration::snapshot()
            .into_iter()
            .map(|stat| CalibrationEvidence {
                kind: format!("{:?}", stat.kind),
                inline_samples: stat.inline_samples,
                pool_samples: stat.pool_samples,
                decided: format!("{:?}", stat.decided),
                candidate_threshold: stat.candidate_threshold,
                applied: stat.applied,
                mode: format!("{:?}", calibration::mode()),
            })
            .collect()
    }
}

/// 饱和观测上报器句柄（旧签名，装配层零改动）
///
/// 实现已迁至 `nexus-telemetry`；本函数只负责把 [`ComputeTelemetry`] 注入。
/// 需要自定义源时改用 [`spawn_saturation_reporter_with_source`]。
pub fn spawn_saturation_reporter(bus: EventBus, period_secs: u64) -> tokio::task::JoinHandle<()> {
    nexus_telemetry::spawn_saturation_reporter(bus, period_secs, Arc::new(ComputeTelemetry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_telemetry_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ComputeTelemetry>();
    }
}
