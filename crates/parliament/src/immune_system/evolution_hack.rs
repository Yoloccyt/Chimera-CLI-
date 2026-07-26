//! EvolutionHack 探针 — 通道 B 否决率异常检测（ADR-046 决策 4）
//!
//! 对应架构层:L8 Parliament
//! 对应 ADR:ADR-046 决策 4（EvolutionHack 探针算法）
//!
//! # 免疫机制锚点（§8.2）
//! RHI-CG 双通道 + 不可进化面 + R2 冻结线
//!
//! # 算法（工程实施偏差,记录于最终报告）
//! ADR-046 原算法基于 ADR-044 RHI-CG 通道 B 事件 + CapabilityFrozen 事件 + R2 违反事件。
//! 由于 ADR-044 RHI-CG 通道 B 尚未落地（事件变体不存在）,本探针采用替代信号：
//! - **CapabilityFrozen 累计计数**: 反复冻结暗示不可进化面被试探
//! - **BudgetExceeded 频率**: 进化导致预算耗尽可能暗示奖励黑客
//!
//! 当镜像无任何信号时返回 `insufficient_data()`（ADR-046 决策 4）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::immune_system::types::{ParadoxProbe, ParadoxReport, ProbeType, Severity};
use crate::immune_system::StabilityMirror;

// ============================================================
// 可进化面参数（决策 9,允许 GSOE/AutoDPO 演化）
// ============================================================

/// CapabilityFrozen 计数 → paradox_rate 的转换系数（可进化面）
const CAPABILITY_FROZEN_WEIGHT: f32 = 0.1;

/// CapabilityFrozen 阈值（>3 触发严重告警,ADR-046 决策 4 步骤 3）
const FROZEN_ALERT_THRESHOLD: u32 = 3;

/// BudgetExceeded → paradox_rate 的转换系数（可进化面）
const BUDGET_EXCEEDED_WEIGHT: f32 = 0.05;

/// BudgetExceeded 时间窗口（毫秒,可进化面）
const BUDGET_EXCEEDED_WINDOW_MS: u64 = 60_000;

// ============================================================
// EvolutionHackProbe — 探针实现
// ============================================================

/// EvolutionHack 探针 — 检测通道 B 否决率异常（ADR-046 决策 4）
///
/// # 设计
/// - 持有 `Arc<StabilityMirror>` 共享镜像状态
/// - 复用镜像已维护的 capability_frozen_count + budget_exceeded_window
/// - KPI-03：<100ms（仅原子读取 + 简单计算）
#[derive(Clone)]
pub struct EvolutionHackProbe {
    mirror: Arc<StabilityMirror>,
}

impl EvolutionHackProbe {
    /// 创建 EvolutionHack 探针
    pub fn new(mirror: Arc<StabilityMirror>) -> Self {
        Self { mirror }
    }

    /// 返回镜像引用（供测试访问）
    pub fn mirror(&self) -> &Arc<StabilityMirror> {
        &self.mirror
    }
}

impl std::fmt::Debug for EvolutionHackProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvolutionHackProbe")
            .field(
                "capability_frozen_count",
                &self.mirror.capability_frozen_count(),
            )
            .finish_non_exhaustive()
    }
}

impl ParadoxProbe for EvolutionHackProbe {
    fn probe_type(&self) -> ProbeType {
        ProbeType::EvolutionHack
    }

    fn detect<'a>(&'a self) -> Pin<Box<dyn Future<Output = ParadoxReport> + Send + 'a>> {
        Box::pin(async move {
            let frozen = self.mirror.capability_frozen_count();
            // WHY 使用 mirror.last_update_ts() 而非 SystemTime::now()：
            //   探针需与事件时间戳保持同一时间坐标系。SystemTime::now()（~1.7e12 ms）
            //   会导致测试注入的事件时间戳（如 1000ms）全部落在滑动窗口外,
            //   使 budget_exceeded_recent_count 始终返回 0。
            //   详见 memory_paradox.rs 同名注释。
            let now_ms = self.mirror.last_update_ts();
            let budget_recent = self
                .mirror
                .budget_exceeded_recent_count(BUDGET_EXCEEDED_WINDOW_MS, now_ms);

            if frozen == 0 && budget_recent == 0 {
                return ParadoxReport::insufficient_data(ProbeType::EvolutionHack);
            }

            // WHY f32 全程：§4.4 #6 红线禁止 f32 隐式转 f64 比较
            // frozen 计数贡献
            let frozen_rate = if frozen > FROZEN_ALERT_THRESHOLD {
                // 超阈值：基础 0.5 + 额外计数 * 权重
                0.5 + (frozen - FROZEN_ALERT_THRESHOLD) as f32 * CAPABILITY_FROZEN_WEIGHT
            } else {
                frozen as f32 * CAPABILITY_FROZEN_WEIGHT
            };

            let budget_rate = budget_recent as f32 * BUDGET_EXCEEDED_WEIGHT;
            let paradox_rate = frozen_rate + budget_rate;

            let details = format!(
                "frozen_count={}, budget_exceeded_recent={}",
                frozen, budget_recent
            );

            let report =
                ParadoxReport::new(ProbeType::EvolutionHack, paradox_rate.min(1.0), details);
            // WHY 显式提升 severity：frozen 超阈值视为不可进化面试探,强制 Critical
            if frozen > FROZEN_ALERT_THRESHOLD {
                ParadoxReport {
                    severity: Severity::Critical,
                    ..report
                }
            } else {
                report
            }
        })
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::immune_system::StabilityMirror;
    use event_bus::{EventMetadata, NexusEvent};

    #[test]
    fn test_evolution_hack_probe_type() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe = EvolutionHackProbe::new(mirror);
        assert_eq!(probe.probe_type(), ProbeType::EvolutionHack);
    }

    #[tokio::test]
    async fn test_evolution_hack_probe_insufficient_data_when_empty() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe = EvolutionHackProbe::new(mirror);
        let report = probe.detect().await;
        assert!(report.insufficient_data);
        assert_eq!(report.probe_type, ProbeType::EvolutionHack);
    }

    #[tokio::test]
    async fn test_evolution_hack_probe_low_frozen() {
        let mirror = Arc::new(StabilityMirror::new());
        // 2 次 CapabilityFrozen（<= 3 阈值）
        for _ in 0..2 {
            let event = NexusEvent::CapabilityFrozen {
                metadata: EventMetadata::new("parliament"),
                capability_id: "cap-1".into(),
                reason: "test".into(),
            };
            mirror.update_from_event(&event, 1000);
        }

        let probe = EvolutionHackProbe::new(mirror);
        let report = probe.detect().await;
        assert!(!report.insufficient_data);
        // 2 * 0.1 = 0.2
        assert!(
            (report.paradox_rate - 0.2).abs() < 1e-6,
            "frozen=2 应得 0.2,实际 = {}",
            report.paradox_rate
        );
    }

    #[tokio::test]
    async fn test_evolution_hack_probe_high_frozen_critical() {
        let mirror = Arc::new(StabilityMirror::new());
        // 5 次 CapabilityFrozen（> 3 阈值）
        for _ in 0..5 {
            let event = NexusEvent::CapabilityFrozen {
                metadata: EventMetadata::new("parliament"),
                capability_id: "cap-1".into(),
                reason: "test".into(),
            };
            mirror.update_from_event(&event, 1000);
        }

        let probe = EvolutionHackProbe::new(mirror);
        let report = probe.detect().await;
        // 0.5 + (5-3) * 0.1 = 0.7
        assert!(
            (report.paradox_rate - 0.7).abs() < 1e-6,
            "frozen=5 应得 0.7,实际 = {}",
            report.paradox_rate
        );
        assert_eq!(report.severity, Severity::Critical);
    }

    #[tokio::test]
    async fn test_evolution_hack_probe_budget_exceeded() {
        let mirror = Arc::new(StabilityMirror::new());
        for ts in [1000u64, 2000, 3000] {
            let event = NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("acb-governor"),
                budget_type: "token".into(),
                current: 1000,
                limit: 1000,
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = EvolutionHackProbe::new(mirror);
        let report = probe.detect().await;
        // 3 * 0.05 = 0.15
        assert!(
            (report.paradox_rate - 0.15).abs() < 1e-6,
            "budget=3 应得 0.15,实际 = {}",
            report.paradox_rate
        );
    }

    #[tokio::test]
    async fn test_evolution_hack_probe_combines_signals() {
        let mirror = Arc::new(StabilityMirror::new());
        // frozen=2 + budget=4
        for _ in 0..2 {
            let event = NexusEvent::CapabilityFrozen {
                metadata: EventMetadata::new("parliament"),
                capability_id: "cap-1".into(),
                reason: "test".into(),
            };
            mirror.update_from_event(&event, 1000);
        }
        for ts in [1000u64, 2000, 3000, 4000] {
            let event = NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("acb-governor"),
                budget_type: "token".into(),
                current: 1000,
                limit: 1000,
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = EvolutionHackProbe::new(mirror);
        let report = probe.detect().await;
        // 2*0.1 + 4*0.05 = 0.2 + 0.2 = 0.4
        assert!(
            (report.paradox_rate - 0.4).abs() < 1e-6,
            "组合信号应得 0.4,实际 = {}",
            report.paradox_rate
        );
    }

    #[tokio::test]
    async fn test_evolution_hack_probe_clamps_to_one() {
        let mirror = Arc::new(StabilityMirror::new());
        // 高 frozen + 高 budget 触发 clamp
        for _ in 0..10 {
            let event = NexusEvent::CapabilityFrozen {
                metadata: EventMetadata::new("parliament"),
                capability_id: "cap-1".into(),
                reason: "test".into(),
            };
            mirror.update_from_event(&event, 1000);
        }
        for ts in 1000..2000 {
            let event = NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("acb-governor"),
                budget_type: "token".into(),
                current: 1000,
                limit: 1000,
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = EvolutionHackProbe::new(mirror);
        let report = probe.detect().await;
        assert!(
            (report.paradox_rate - 1.0).abs() < 1e-6,
            "应 clamp 到 1.0,实际 = {}",
            report.paradox_rate
        );
    }

    #[test]
    fn test_evolution_hack_probe_clone_preserves_mirror() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe1 = EvolutionHackProbe::new(Arc::clone(&mirror));
        let probe2 = probe1.clone();
        assert!(Arc::ptr_eq(probe1.mirror(), probe2.mirror()));
    }
}
