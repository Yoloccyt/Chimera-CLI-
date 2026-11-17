//! FormalVerifier M1 — 事件因果一致性验证器（测试夹具）
//!
//! 对应架构层: L4 FormalVerifier
//! 对应 ADR: ADR-047(M1 Property #4:跨层事件因果一致性)
//!
//! # 为什么住在测试侧而不是生产 crate
//!
//! L1-B-3 波次（2026-11-16）把该类型从 `event-bus/src/formal.rs` 迁到本夹具：
//! 它的**唯一**消费方是根 E2E（`formal_verifier_m1_e2e.rs`），生产 crate 内零消费。
//! 按「零装配面物理删除」口径，把只服务验证证据链的代码留在生产 crate 里，
//! 等于让生产二进制背着一条永不执行的验证路径——迁移到夹具后，生产面干净，
//! 而**验证能力一条不少**（三条属性判据与其全部单测原样随迁）。
//!
//! 归属登记：`scripts/check_formal_property_ledger.py` 的名册↔checker 对账行
//! 已同步指向本文件（checker 仍然存在，只是宿主换了）。

use event_bus::EventMetadata;
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

        // 类型由 `meta.event_id` 推断（避免在夹具里引入 uuid 直接依赖）
        let mut seen: HashSet<_> = HashSet::with_capacity(events.len());
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
