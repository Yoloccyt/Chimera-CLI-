//! FormalVerifier M1 — 事件因果一致性形式化验证(P7-T4)
//!
//! 对应架构层: L4 FormalVerifier(event-bus 内验证器实现)
//! 状态(ADR-181):VERIFIER-EVIDENCE-ONLY —— 仅供 FormalVerifierGate
//!             证据链(经根 E2E 消费),非生产运行时路径;不退役。
//! 对应 ADR: ADR-047(M1 Property #4:跨层事件因果一致性)
//! 对应计划: `IMPLEMENTATION_PLAN_Harness_Engineering_V3.md` Phase 7 P7-T4
//!
//! # 核心保证(Property #4)
//!
//! 本模块提供三个纯函数验证器,确保事件流的元数据满足因果一致性:
//!
//! 1. **event_id 时序性**: UUIDv7 event_id 按发布序单调递增
//!    (UUIDv7 前 48 位是毫秒时间戳,乱序 = 事件重放/时钟回拨/伪造注入)
//! 2. **timestamp 单调性**: 同一 source 的事件时间戳非减
//!    (违反 = 该发布者时钟异常或事件被乱序重排)
//! 3. **event_id 唯一性**: 流内无重复 event_id
//!    (重复 = 事件被重复投递,下游幂等假设被破坏)
//!
//! # 设计决策(WHY)
//!
//! - **验证 EventMetadata 序列而非 NexusEvent**: 因果性质只依赖元数据
//!   (event_id/timestamp/source),与 107+ 事件变体的载荷解耦——
//!   变体演进(append-only)不影响验证器
//! - **同 source 才约束 timestamp**: 跨 source 时钟不可比
//!   (分布式常识),全局序由 UUIDv7 的 event_id 承载
//! - **纯函数 + `VerificationResult`**: 与 gsoe/parliament/auto-dpo/
//!   omega-learner 验证器同款模式,FormalVerifier 管线统一消费

use crate::EventMetadata;
use nexus_contracts::formal_props::VerificationResult;
use std::collections::{HashMap, HashSet};

/// 事件因果一致性验证器
///
/// 所有方法为纯函数,不修改内部状态,可在 FormalVerifier 管线中并发调用。
#[derive(Debug, Default, Clone, Copy)]
pub struct CausalConsistencyChecker;

impl CausalConsistencyChecker {
    /// 创建因果一致性验证器实例
    pub fn new() -> Self {
        Self
    }

    /// 验证 event_id(UUIDv7)按发布序单调递增
    ///
    /// WHY 可行: UUIDv7 高位为毫秒时间戳 + 单调计数器,
    /// 字节序比较即时间序比较(RFC 9562 §5.7)。
    ///
    /// # 返回
    /// - `Skipped`: 事件数 < 2(无相邻对可验证)
    #[must_use]
    pub fn verify_event_id_ordering(&self, events: &[EventMetadata]) -> VerificationResult {
        if events.len() < 2 {
            return VerificationResult::Skipped {
                reason: format!("事件数 {} < 2,无相邻对可验证", events.len()),
            };
        }

        let mut violations: Vec<String> = Vec::new();
        for (i, pair) in events.windows(2).enumerate() {
            if pair[0].event_id >= pair[1].event_id {
                violations.push(format!(
                    "位置 {i}: event_id {} >= {} (UUIDv7 时序倒置)",
                    pair[0].event_id, pair[1].event_id
                ));
            }
        }

        Self::to_result(violations, (events.len() - 1) as u64)
    }

    /// 验证同一 source 的事件时间戳非减
    ///
    /// # 返回
    /// - `Skipped`: 事件数 < 2
    #[must_use]
    pub fn verify_per_source_timestamp_monotonic(
        &self,
        events: &[EventMetadata],
    ) -> VerificationResult {
        if events.len() < 2 {
            return VerificationResult::Skipped {
                reason: format!("事件数 {} < 2,无相邻对可验证", events.len()),
            };
        }

        // 按 source 追踪最近时间戳(保持发布序,单遍扫描)
        let mut last_seen: HashMap<&str, (usize, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        let mut violations: Vec<String> = Vec::new();
        let mut samples_tested: u64 = 0;

        for (i, meta) in events.iter().enumerate() {
            if let Some((prev_idx, prev_ts)) = last_seen.get(meta.source.as_str()) {
                samples_tested += 1;
                if meta.timestamp < *prev_ts {
                    violations.push(format!(
                        "source '{}': 位置 {prev_idx} → {i} 时间戳回退 {} → {}",
                        meta.source, prev_ts, meta.timestamp
                    ));
                }
            }
            last_seen.insert(meta.source.as_str(), (i, meta.timestamp));
        }

        if samples_tested == 0 {
            return VerificationResult::Skipped {
                reason: "无同 source 相邻事件对(所有事件来源互异)".to_string(),
            };
        }
        Self::to_result(violations, samples_tested)
    }

    /// 验证流内 event_id 唯一(无重复投递)
    ///
    /// # 返回
    /// - `Skipped`: 空序列
    #[must_use]
    pub fn verify_event_id_unique(&self, events: &[EventMetadata]) -> VerificationResult {
        if events.is_empty() {
            return VerificationResult::Skipped {
                reason: "事件序列为空".to_string(),
            };
        }

        let mut seen: HashSet<uuid::Uuid> = HashSet::with_capacity(events.len());
        let mut violations: Vec<String> = Vec::new();
        for (i, meta) in events.iter().enumerate() {
            if !seen.insert(meta.event_id) {
                violations.push(format!(
                    "位置 {i}: event_id {} 重复(重复投递)",
                    meta.event_id
                ));
            }
        }

        Self::to_result(violations, events.len() as u64)
    }

    /// 违规列表 → VerificationResult(三验证器共享的收敛逻辑)
    fn to_result(violations: Vec<String>, samples_tested: u64) -> VerificationResult {
        if violations.is_empty() {
            VerificationResult::Satisfied { samples_tested }
        } else {
            VerificationResult::Violated {
                counterexample: violations.join("; "),
                samples_tested,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造按序发布的元数据序列(UUIDv7 自动时序)
    fn ordered_events(sources: &[&str]) -> Vec<EventMetadata> {
        sources.iter().map(|s| EventMetadata::new(*s)).collect()
    }

    // ============================================================
    // event_id 时序性
    // ============================================================

    #[test]
    fn test_event_id_ordering_satisfied() {
        let checker = CausalConsistencyChecker::new();
        let events = ordered_events(&["a", "b", "a"]);
        assert!(checker.verify_event_id_ordering(&events).is_satisfied());
    }

    #[test]
    fn test_event_id_ordering_violated_on_reversal() {
        let checker = CausalConsistencyChecker::new();
        let mut events = ordered_events(&["a", "b"]);
        events.reverse(); // 人为倒置发布序
        assert!(matches!(
            checker.verify_event_id_ordering(&events),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_event_id_ordering_single_skipped() {
        let checker = CausalConsistencyChecker::new();
        let events = ordered_events(&["a"]);
        assert!(matches!(
            checker.verify_event_id_ordering(&events),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // 同 source 时间戳单调性
    // ============================================================

    #[test]
    fn test_per_source_timestamp_satisfied() {
        let checker = CausalConsistencyChecker::new();
        let events = ordered_events(&["a", "b", "a", "b"]);
        assert!(checker
            .verify_per_source_timestamp_monotonic(&events)
            .is_satisfied());
    }

    #[test]
    fn test_per_source_timestamp_violated_on_clock_rollback() {
        let checker = CausalConsistencyChecker::new();
        let mut events = ordered_events(&["a", "a"]);
        // 人为制造 source "a" 的时钟回拨
        events[1].timestamp = events[0].timestamp - chrono::Duration::seconds(10);
        assert!(matches!(
            checker.verify_per_source_timestamp_monotonic(&events),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_per_source_all_distinct_sources_skipped() {
        // 所有事件来源互异:无同 source 对可验证
        let checker = CausalConsistencyChecker::new();
        let events = ordered_events(&["a", "b", "c"]);
        assert!(matches!(
            checker.verify_per_source_timestamp_monotonic(&events),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // event_id 唯一性
    // ============================================================

    #[test]
    fn test_event_id_unique_satisfied() {
        let checker = CausalConsistencyChecker::new();
        let events = ordered_events(&["a", "b", "c"]);
        assert!(checker.verify_event_id_unique(&events).is_satisfied());
    }

    #[test]
    fn test_event_id_duplicate_violated() {
        let checker = CausalConsistencyChecker::new();
        let mut events = ordered_events(&["a", "b"]);
        // 人为重复投递(同 event_id)
        events[1].event_id = events[0].event_id;
        let result = checker.verify_event_id_unique(&events);
        match result {
            VerificationResult::Violated { counterexample, .. } => {
                assert!(counterexample.contains("重复"));
            }
            other => panic!("期望 Violated,实际: {other:?}"),
        }
    }

    #[test]
    fn test_event_id_unique_empty_skipped() {
        let checker = CausalConsistencyChecker::new();
        assert!(matches!(
            checker.verify_event_id_unique(&[]),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // 综合:三验证器对同一合法流全部满足
    // ============================================================

    #[test]
    fn test_all_checks_pass_on_legal_stream() {
        let checker = CausalConsistencyChecker::new();
        let events = ordered_events(&["quest-engine", "parliament", "quest-engine", "seccore"]);
        assert!(checker.verify_event_id_ordering(&events).is_satisfied());
        assert!(checker
            .verify_per_source_timestamp_monotonic(&events)
            .is_satisfied());
        assert!(checker.verify_event_id_unique(&events).is_satisfied());
    }
}
