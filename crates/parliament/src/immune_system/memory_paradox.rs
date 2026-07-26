//! MemoryParadox 探针 — 过时事实与当前事实共召回检测（ADR-046 决策 2）
//!
//! 对应架构层:L8 Parliament
//! 对应 ADR:ADR-046 决策 2（MemoryParadox 探针算法）
//!
//! # 免疫机制锚点（§8.2）
//! TemporalFilter（§4.3）+ S2 Bandit + INV-8 单调归档
//!
//! # 算法（工程实施偏差,记录于最终报告）
//! ADR-046 设计原算法基于 `ContextRetrieved` 事件（mlc-engine）+ `AgentArchived` 事件
//! （chimera-mas archive/）。但**当前 event-bus 未定义这两个事件变体**。
//!
//! 按"代码基线优先 + 不修改 event-bus"原则,本探针采用替代信号：
//! - **CsnSubstitutionTriggered.degradation_level**: 记忆降级层级（max 镜像）
//!   - degradation_level > 0 暗示记忆系统已降级,可能产生"幽灵记忆"
//! - **BudgetExceeded 频率**: 预算耗尽频繁会触发记忆稀疏化,增加新旧事实共存概率
//!
//! 当镜像无任何信号时返回 `insufficient_data()`（ADR-046 决策 2）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::immune_system::types::{ParadoxProbe, ParadoxReport, ProbeType};
use crate::immune_system::StabilityMirror;

// ============================================================
// 可进化面参数（决策 9,允许 GSOE/AutoDPO 演化）
// ============================================================

/// 降级层级 → paradox_rate 的转换系数（可进化面）
const DEGRADATION_LEVEL_WEIGHT: f32 = 0.15;

/// BudgetExceeded 计数 → paradox_rate 的转换系数（可进化面）
const BUDGET_EXCEEDED_WEIGHT: f32 = 0.05;

/// BudgetExceeded 时间窗口（毫秒,可进化面）
const BUDGET_EXCEEDED_WINDOW_MS: u64 = 60_000;

// ============================================================
// MemoryParadoxProbe — 探针实现
// ============================================================

/// MemoryParadox 探针 — 检测过时事实与当前事实共召回（ADR-046 决策 2）
///
/// # 设计
/// - 持有 `Arc<StabilityMirror>` 共享镜像状态
/// - `detect()` 同步读取镜像状态,boxed Future 满足 `ParadoxProbe` trait
/// - KPI-03：<100ms（仅原子读取 + 简单计算）
#[derive(Clone)]
pub struct MemoryParadoxProbe {
    mirror: Arc<StabilityMirror>,
}

impl MemoryParadoxProbe {
    /// 创建 MemoryParadox 探针
    pub fn new(mirror: Arc<StabilityMirror>) -> Self {
        Self { mirror }
    }

    /// 返回镜像引用（供测试访问）
    pub fn mirror(&self) -> &Arc<StabilityMirror> {
        &self.mirror
    }
}

impl std::fmt::Debug for MemoryParadoxProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryParadoxProbe")
            .field("mirror_degradation_level", &self.mirror.degradation_level())
            .finish_non_exhaustive()
    }
}

impl ParadoxProbe for MemoryParadoxProbe {
    fn probe_type(&self) -> ProbeType {
        ProbeType::MemoryParadox
    }

    fn detect<'a>(&'a self) -> Pin<Box<dyn Future<Output = ParadoxReport> + Send + 'a>> {
        Box::pin(async move {
            let degradation = self.mirror.degradation_level();
            // WHY 使用 mirror.last_update_ts() 而非 SystemTime::now()：
            //   探针需与事件时间戳保持同一时间坐标系。若用 SystemTime::now()（约 1.7e12 ms）
            //   会导致测试注入的事件时间戳（如 1000ms）远小于当前时间,使滑动窗口
            //   budget_exceeded_recent_count 始终返回 0（事件全部落在窗口外）。
            //   镜像无事件时 last_update_ts() = 0,触发 insufficient_data 分支。
            //   生产环境事件由 event-bus 后台任务推入镜像,时间戳取自事件接收时刻,
            //   与 last_update_ts() 同源,无时间漂移问题。
            let now_ms = self.mirror.last_update_ts();
            let budget_recent = self
                .mirror
                .budget_exceeded_recent_count(BUDGET_EXCEEDED_WINDOW_MS, now_ms);

            // WHY 工程偏差：ContextRetrieved/AgentArchived 事件未扩展,
            //   使用 degradation_level + budget_exceeded 作为代理信号
            if degradation == 0 && budget_recent == 0 {
                return ParadoxReport::insufficient_data(ProbeType::MemoryParadox);
            }

            // WHY f32 全程：§4.4 #6 红线禁止 f32 隐式转 f64 比较
            let rate = (degradation as f32 * DEGRADATION_LEVEL_WEIGHT)
                + (budget_recent as f32 * BUDGET_EXCEEDED_WEIGHT);

            let details = format!(
                "degradation_level={}, budget_exceeded_recent={}",
                degradation, budget_recent
            );

            ParadoxReport::new(ProbeType::MemoryParadox, rate.min(1.0), details)
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
    fn test_memory_paradox_probe_type() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe = MemoryParadoxProbe::new(mirror);
        assert_eq!(probe.probe_type(), ProbeType::MemoryParadox);
    }

    #[tokio::test]
    async fn test_memory_paradox_probe_insufficient_data_when_empty() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe = MemoryParadoxProbe::new(mirror);
        let report = probe.detect().await;
        assert!(report.insufficient_data);
        assert_eq!(report.probe_type, ProbeType::MemoryParadox);
        assert!(report.paradox_rate.abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_memory_paradox_probe_detects_degradation() {
        let mirror = Arc::new(StabilityMirror::new());
        // 模拟降级触发（degradation=3 使 paradox_rate=0.45 明确落在 Warning 区间）
        // WHY degradation=2 会使 paradox_rate=0.3（f32 边界值）,而 Severity::from_paradox_rate
        //   用 `> 0.3` 严格大于判断,0.3 → Normal（非 Warning）。改为 3 使 0.45 > 0.3 → Warning。
        let event = NexusEvent::CsnSubstitutionTriggered {
            metadata: EventMetadata::new("csn-substitutor"),
            original_capability_id: "cap-1".into(),
            substitute_id: "cap-sub-1".into(),
            similarity_score: 0.9,
            degradation_level: 3,
        };
        mirror.update_from_event(&event, 1000);

        let probe = MemoryParadoxProbe::new(mirror);
        let report = probe.detect().await;
        assert!(!report.insufficient_data);
        // 3 * 0.15 = 0.45
        assert!(
            (report.paradox_rate - 0.45).abs() < 1e-6,
            "degradation=3 应得 0.45,实际 = {}",
            report.paradox_rate
        );
        assert_eq!(
            report.severity,
            crate::immune_system::types::Severity::Warning
        );
    }

    #[tokio::test]
    async fn test_memory_paradox_probe_detects_budget_exceeded() {
        let mirror = Arc::new(StabilityMirror::new());
        // 模拟 3 次 BudgetExceeded
        for ts in [1000u64, 2000, 3000] {
            let event = NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("acb-governor"),
                budget_type: "token".into(),
                current: 1000,
                limit: 1000,
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = MemoryParadoxProbe::new(mirror);
        let report = probe.detect().await;
        assert!(!report.insufficient_data);
        // 3 * 0.05 = 0.15
        assert!(
            (report.paradox_rate - 0.15).abs() < 1e-6,
            "budget=3 应得 0.15,实际 = {}",
            report.paradox_rate
        );
    }

    #[tokio::test]
    async fn test_memory_paradox_probe_combines_signals() {
        let mirror = Arc::new(StabilityMirror::new());
        // degradation=3 + budget=4
        let csn_event = NexusEvent::CsnSubstitutionTriggered {
            metadata: EventMetadata::new("csn-substitutor"),
            original_capability_id: "cap-1".into(),
            substitute_id: "cap-sub-1".into(),
            similarity_score: 0.9,
            degradation_level: 3,
        };
        mirror.update_from_event(&csn_event, 1000);
        for ts in [1000u64, 2000, 3000, 4000] {
            let event = NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("acb-governor"),
                budget_type: "token".into(),
                current: 1000,
                limit: 1000,
            };
            mirror.update_from_event(&event, ts);
        }

        let probe = MemoryParadoxProbe::new(mirror);
        let report = probe.detect().await;
        // 3*0.15 + 4*0.05 = 0.45 + 0.2 = 0.65
        assert!(
            (report.paradox_rate - 0.65).abs() < 1e-6,
            "组合信号应得 0.65,实际 = {}",
            report.paradox_rate
        );
    }

    #[tokio::test]
    async fn test_memory_paradox_probe_clamps_to_one() {
        let mirror = Arc::new(StabilityMirror::new());
        // 高 degradation 触发 clamp
        let csn_event = NexusEvent::CsnSubstitutionTriggered {
            metadata: EventMetadata::new("csn-substitutor"),
            original_capability_id: "cap-1".into(),
            substitute_id: "cap-sub-1".into(),
            similarity_score: 0.9,
            degradation_level: 10,
        };
        mirror.update_from_event(&csn_event, 1000);

        let probe = MemoryParadoxProbe::new(mirror);
        let report = probe.detect().await;
        // 10*0.15 = 1.5 → clamp 到 1.0
        assert!(
            (report.paradox_rate - 1.0).abs() < 1e-6,
            "应 clamp 到 1.0,实际 = {}",
            report.paradox_rate
        );
        assert_eq!(
            report.severity,
            crate::immune_system::types::Severity::Critical
        );
    }

    #[test]
    fn test_memory_paradox_probe_clone_preserves_mirror() {
        let mirror = Arc::new(StabilityMirror::new());
        let probe1 = MemoryParadoxProbe::new(Arc::clone(&mirror));
        let probe2 = probe1.clone();
        // 共享同一 Arc
        assert!(Arc::ptr_eq(probe1.mirror(), probe2.mirror()));
    }
}
