//! ReasoningTrap 探针 — SkepticVeto 模式化绕过识别（ADR-046 决策 3）
//!
//! 对应架构层:L8 Parliament
//! 对应 ADR:ADR-046 决策 3（ReasoningTrap 探针算法）
//!
//! # 免疫机制锚点（§8.2）
//! Fast Path 80% 跳过 + 自白通道 + 复杂度预算
//!
//! # 算法（工程实施）
//! ADR-046 原算法基于 SkepticVeto + VetoOverridden 事件流 + ahirt 自白通道。
//! 本实现复用 StabilityMirror 已维护的 skeptic_veto_count / veto_overridden_count
//! 滑动窗口（无需重复维护）。
//!
//! # 计算公式（ADR-046 决策 3 步骤 3）
//! ```text
//! paradox_rate = if pattern_count > 5 { 0.6 } else { 0.2 }
//!              + veto_override_rate * 0.3
//! ```
//! 其中 `pattern_count` 简化为 `skeptic_veto_count`（窗口内总 veto 次数）,
//! `veto_override_rate` 为 `veto_overridden_count / max(veto_count, 1)`。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// WHY `Severity` 通过 `#[cfg(test)]` 单独导入：仅测试断言用,生产代码通过
// `ParadoxReport::new()` 内部 `Severity::from_paradox_rate()` 间接构造,无直接引用。
use crate::immune_system::types::{ParadoxProbe, ParadoxReport, ProbeType};
use crate::immune_system::StabilityMirror;

// ============================================================
// 可进化面参数（决策 9,允许 GSOE/AutoDPO 演化）
// ============================================================

/// SkepticVeto 窗口阈值（>5 触发模式化告警,ADR-046 决策 3）
const VETO_PATTERN_THRESHOLD: usize = 5;

/// 模式化告警基础分数（>阈值）
const PATTERN_ALERT_HIGH: f32 = 0.6;

/// 模式化告警基础分数（<=阈值）
const PATTERN_ALERT_LOW: f32 = 0.2;

/// VetoOverridden 占比权重（ADR-046 决策 3 步骤 3）
const VETO_OVERRIDE_RATE_WEIGHT: f32 = 0.3;

// ============================================================
// ReasoningTrapProbe — 探针实现
// ============================================================

/// ReasoningTrap 探针 — 检测 SkepticVeto 模式化绕过（ADR-046 决策 3）
///
/// # 设计
/// - 持有 `Arc<StabilityMirror>` 共享镜像状态
/// - 复用镜像已维护的 skeptic_veto_window / veto_overridden_window
/// - KPI-03：<100ms（仅原子读取 + 简单计算）
#[derive(Clone)]
pub struct ReasoningTrapProbe {
    mirror: Arc<StabilityMirror>,
}

impl ReasoningTrapProbe {
    /// 创建 ReasoningTrap 探针
    pub fn new(mirror: Arc<StabilityMirror>) -> Self {
        Self { mirror }
    }

    /// 返回镜像引用（供测试访问）
    pub fn mirror(&self) -> &Arc<StabilityMirror> {
        &self.mirror
    }
}

impl std::fmt::Debug for ReasoningTrapProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReasoningTrapProbe")
            .field("skeptic_veto_count", &self.mirror.skeptic_veto_count())
            .field(
                "veto_overridden_count",
                &self.mirror.veto_overridden_count(),
            )
            .finish_non_exhaustive()
    }
}

impl ParadoxProbe for ReasoningTrapProbe {
    fn probe_type(&self) -> ProbeType {
        ProbeType::ReasoningTrap
    }

    fn detect<'a>(&'a self) -> Pin<Box<dyn Future<Output = ParadoxReport> + Send + 'a>> {
        Box::pin(async move {
            let veto_count = self.mirror.skeptic_veto_count();
            let override_count = self.mirror.veto_overridden_count();

            if veto_count == 0 {
                return ParadoxReport::insufficient_data(ProbeType::ReasoningTrap);
            }

            // WHY f32 全程：§4.4 #6 红线禁止 f32 隐式转 f64 比较
            // 模式化基础分数
            let pattern_base = if veto_count > VETO_PATTERN_THRESHOLD {
                PATTERN_ALERT_HIGH
            } else {
                PATTERN_ALERT_LOW
            };

            // veto_override_rate = override_count / veto_count
            let veto_override_rate = override_count as f32 / veto_count as f32;
            let paradox_rate = pattern_base + veto_override_rate * VETO_OVERRIDE_RATE_WEIGHT;

            let details = format!(
                "veto_count={}, override_count={}, override_rate={:.3}",
                veto_count, override_count, veto_override_rate
            );

            ParadoxReport::new(ProbeType::ReasoningTrap, paradox_rate.min(1.0), details)
        })
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::immune_system::types::Severity;
    use crate::immune_system::StabilityMirror;
    use event_bus::{EventMetadata, NexusEvent};

    #[test]
    fn test_reasoning_trap_probe_type() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe = ReasoningTrapProbe::new(mirror);
        assert_eq!(probe.probe_type(), ProbeType::ReasoningTrap);
    }

    #[tokio::test]
    async fn test_reasoning_trap_probe_insufficient_data_when_empty() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe = ReasoningTrapProbe::new(mirror);
        let report = probe.detect().await;
        assert!(report.insufficient_data);
        assert_eq!(report.probe_type, ProbeType::ReasoningTrap);
    }

    #[tokio::test]
    async fn test_reasoning_trap_probe_low_pattern() {
        let mirror = Arc::new(StabilityMirror::new());
        // 3 次 SkepticVeto（<= 5 阈值）
        for ts in [1000u64, 2000, 3000] {
            let event = NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                veto_reason: "test".into(),
                frozen_capabilities: vec![],
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = ReasoningTrapProbe::new(mirror);
        let report = probe.detect().await;
        assert!(!report.insufficient_data);
        // 0.2 + 0 * 0.3 = 0.2
        assert!(
            (report.paradox_rate - 0.2).abs() < 1e-6,
            "低模式应得 0.2,实际 = {}",
            report.paradox_rate
        );
    }

    #[tokio::test]
    async fn test_reasoning_trap_probe_high_pattern() {
        let mirror = Arc::new(StabilityMirror::new());
        // 6 次 SkepticVeto（> 5 阈值）
        for ts in [1000u64, 2000, 3000, 4000, 5000, 6000] {
            let event = NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                veto_reason: "test".into(),
                frozen_capabilities: vec![],
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = ReasoningTrapProbe::new(mirror);
        let report = probe.detect().await;
        // 0.6 + 0 * 0.3 = 0.6
        assert!(
            (report.paradox_rate - 0.6).abs() < 1e-6,
            "高模式应得 0.6,实际 = {}",
            report.paradox_rate
        );
        assert_eq!(report.severity, Severity::Warning);
    }

    #[tokio::test]
    async fn test_reasoning_trap_probe_with_override() {
        let mirror = Arc::new(StabilityMirror::new());
        // 4 次 SkepticVeto + 2 次 VetoOverridden
        for ts in [1000u64, 2000, 3000, 4000] {
            let event = NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                veto_reason: "test".into(),
                frozen_capabilities: vec![],
            };
            mirror.update_from_event(&event, ts);
        }
        for ts in [1500u64, 2500] {
            let event = NexusEvent::VetoOverridden {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                proposal_id: "p-1".into(),
                veto_reason: "test".into(),
                override_reason: "false positive".into(),
                override_by: "admin".into(),
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = ReasoningTrapProbe::new(mirror);
        let report = probe.detect().await;
        // 0.2 + (2/4=0.5) * 0.3 = 0.2 + 0.15 = 0.35
        assert!(
            (report.paradox_rate - 0.35).abs() < 1e-6,
            "覆盖场景应得 0.35,实际 = {}",
            report.paradox_rate
        );
    }

    #[tokio::test]
    async fn test_reasoning_trap_probe_high_pattern_with_override() {
        let mirror = Arc::new(StabilityMirror::new());
        // 6 次 SkepticVeto + 3 次 VetoOverridden
        for ts in [1000u64, 2000, 3000, 4000, 5000, 6000] {
            let event = NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                veto_reason: "test".into(),
                frozen_capabilities: vec![],
            };
            mirror.update_from_event(&event, ts);
        }
        for ts in [1500u64, 2500, 3500] {
            let event = NexusEvent::VetoOverridden {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                proposal_id: "p-1".into(),
                veto_reason: "test".into(),
                override_reason: "false positive".into(),
                override_by: "admin".into(),
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = ReasoningTrapProbe::new(mirror);
        let report = probe.detect().await;
        // 0.6 + (3/6=0.5) * 0.3 = 0.6 + 0.15 = 0.75
        assert!(
            (report.paradox_rate - 0.75).abs() < 1e-6,
            "高模式 + 覆盖应得 0.75,实际 = {}",
            report.paradox_rate
        );
        assert_eq!(report.severity, Severity::Critical);
    }

    #[tokio::test]
    async fn test_reasoning_trap_probe_clamps_to_one() {
        let mirror = Arc::new(StabilityMirror::new());
        // 6 次 SkepticVeto + 6 次 VetoOverridden（覆盖率 1.0）
        for ts in [1000u64, 2000, 3000, 4000, 5000, 6000] {
            let event = NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                veto_reason: "test".into(),
                frozen_capabilities: vec![],
            };
            mirror.update_from_event(&event, ts);
        }
        for ts in [1100u64, 2100, 3100, 4100, 5100, 6100] {
            let event = NexusEvent::VetoOverridden {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                proposal_id: "p-1".into(),
                veto_reason: "test".into(),
                override_reason: "false positive".into(),
                override_by: "admin".into(),
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = ReasoningTrapProbe::new(mirror);
        let report = probe.detect().await;
        // 0.6 + 1.0 * 0.3 = 0.9（未超 1.0）
        assert!(
            (report.paradox_rate - 0.9).abs() < 1e-6,
            "应得 0.9,实际 = {}",
            report.paradox_rate
        );
    }

    #[test]
    fn test_reasoning_trap_probe_clone_preserves_mirror() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe1 = ReasoningTrapProbe::new(Arc::clone(&mirror));
        let probe2 = probe1.clone();
        assert!(Arc::ptr_eq(probe1.mirror(), probe2.mirror()));
    }
}
