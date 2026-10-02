//! Observation 归纳结论分库 — 派生结论与原始事实分离 + 来源标注（v2.3 §(b.3)）
//!
//! 对应架构层:L2 Memory（mlc-engine）
//! 对应设计源:`Chimera架构重构与优化方案_v2.3` §(b.3)「Observation（新建）」
//!
//! # WHY
//! §(b.3) 主线③要求「归纳结论与原始事实**分库**，查询结果带**来源标注**」——与
//! §(b.1) Claim/Certify 同构：**原始记录与派生结论必须可区分**。`EpistemicType::Observation`
//! 标注条目性质，本 ledger 提供其分库存储：派生结论各自携带 `evidence_fact_ids`
//! 指向支撑它的原始事实，并建**反向索引**（fact → observations），使任一归纳结论可
//! 追溯到来源、任一原始事实可查其派生。recall 集成属后续接线；本模块是可测试的核心结构。

use std::collections::HashMap;

/// 一条归纳结论（派生自若干原始事实）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// 归纳结论文本
    pub conclusion: String,
    /// 证据指针：支撑该结论的原始事实条目 id（≥1；无来源的不应登记为 Observation）
    pub evidence_fact_ids: Vec<String>,
}

/// Observation 分库：`id → 结论` + `fact_id → 相关结论 id`（来源标注与反查）。
#[derive(Debug, Clone, Default)]
pub struct ObservationLedger {
    by_id: HashMap<String, Observation>,
    by_fact: HashMap<String, Vec<String>>,
}

impl ObservationLedger {
    /// 创建空分库。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一条归纳结论；空证据指针的结论被拒绝（返回 false），保证「派生必带来源」。
    pub fn record(&mut self, id: impl Into<String>, observation: Observation) -> bool {
        let id = id.into();
        if observation.evidence_fact_ids.is_empty() {
            return false;
        }
        for fact in &observation.evidence_fact_ids {
            self.by_fact
                .entry(fact.clone())
                .or_default()
                .push(id.clone());
        }
        self.by_id.insert(id, observation);
        true
    }

    /// 按 id 取结论。
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Observation> {
        self.by_id.get(id)
    }

    /// 来源反查：给定原始事实 id，返回由它支撑的所有归纳结论 id。
    #[must_use]
    pub fn observations_for_fact(&self, fact_id: &str) -> &[String] {
        self.by_fact.get(fact_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// 原始事实作废时，移除其支撑的归纳结论（来源失效 ⇒ 派生结论失效），返回移除数。
    pub fn invalidate_by_fact(&mut self, fact_id: &str) -> usize {
        let Some(obs_ids) = self.by_fact.remove(fact_id) else {
            return 0;
        };
        let mut removed = 0;
        for oid in obs_ids {
            if let Some(obs) = self.by_id.remove(&oid) {
                for f in &obs.evidence_fact_ids {
                    if let Some(list) = self.by_fact.get_mut(f) {
                        list.retain(|x| *x != oid);
                    }
                }
                removed += 1;
            }
        }
        removed
    }

    /// 已登记结论数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试码 unwrap 为 Rust 惯用法；E-5 lint 意在治理生产码
mod tests {
    use super::*;

    fn obs(conclusion: &str, facts: &[&str]) -> Observation {
        Observation {
            conclusion: conclusion.into(),
            evidence_fact_ids: facts.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn record_requires_evidence() {
        let mut l = ObservationLedger::new();
        assert!(!l.record("o1", obs("no-evidence", &[])), "空证据须被拒");
        assert!(l.record("o2", obs("has-evidence", &["f1"])));
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn provenance_forward_and_reverse() {
        let mut l = ObservationLedger::new();
        l.record("o1", obs("derived", &["f1", "f2"]));
        assert_eq!(l.get("o1").unwrap().evidence_fact_ids, vec!["f1", "f2"]);
        assert_eq!(l.observations_for_fact("f1"), ["o1".to_string()]);
        assert!(l.observations_for_fact("f9").is_empty());
    }

    #[test]
    fn invalidate_by_fact_cascades() {
        let mut l = ObservationLedger::new();
        l.record("o1", obs("a", &["f1"]));
        l.record("o2", obs("b", &["f1", "f2"]));
        assert_eq!(l.invalidate_by_fact("f1"), 2);
        assert!(l.is_empty());
        assert!(l.observations_for_fact("f2").is_empty(), "反查索引不留悬挂");
    }

    #[test]
    fn engine_partition_by_epistemic_splits_observation() {
        // §(b.3) 召回结果分库:Observation 与原始事实可区分
        use crate::engine::MlcEngine;
        use crate::types::{MemoryEntry, MemoryTier};
        use nexus_contracts::EpistemicType;
        let fact = MemoryEntry::new("m-1", "raw", MemoryTier::L2Semantic);
        let mut derived = MemoryEntry::new("m-2", "conclusion", MemoryTier::L2Semantic)
            .with_epistemic(EpistemicType::Observation);
        derived.evidence_ptrs = vec!["m-1".to_string()];
        let entries = [fact, derived];
        let (observations, facts) = MlcEngine::partition_by_epistemic(&entries);
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].content, "conclusion");
        assert_eq!(observations[0].evidence_ptrs, vec!["m-1".to_string()]);
        assert_eq!(facts.len(), 1);
    }
}
