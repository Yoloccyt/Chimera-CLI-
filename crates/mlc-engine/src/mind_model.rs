//! MindModel 心智模型缓存 — 固定问题的预计算答案，O(1) 读取 + 失效重写（v2.3 §(b.3)）
//!
//! 对应架构层:L2 Memory（mlc-engine）
//! 对应设计源:`Chimera架构重构与优化方案_v2.3` §(b.3)「MindModel（新建）」
//!
//! # WHY
//! §(b.3) 把记忆从「按保留时长分层」升级为「层级 × 认识类型」二维模型，其中
//! `EpistemicType::MindModel` 表达「固定问题的预计算答案」：读取 = 一次哈希查询、
//! **零模型调用**（MoNe「查询 O(1)」理念的产品化低配版 = 缓存表 + 失效事件）。
//! 本模块提供该缓存表与其失效语义；调用方 `lookup` 未命中时自行计算并 `insert` 回填，
//! 关联对象变更时以 tag 触发 `invalidate_by_tag` 实现「失效重写」。
//!
//! 与 `MemoryEntry::epistemic_type`（§(b.3) 正交维标签）配套：条目可标注为 MindModel，
//! 而本 store 承载其「答案缓存 + 来源标注 + 失效」的运行面。

use std::collections::HashMap;

/// 一条预计算答案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MindModelAnswer {
    /// 预计算的答案文本
    pub answer: String,
    /// 来源标注（指向支撑该答案的原始记录条目 id；与 Observation 溯源同构）
    pub source_refs: Vec<String>,
    /// 失效触发标签（如关联对象/Quest id；命中即整条作废，促发重算）
    pub invalidation_tags: Vec<String>,
}

/// MindModel 缓存表：`key`（归一化固定问题）→ 预计算答案。
#[derive(Debug, Clone, Default)]
pub struct MindModelStore {
    entries: HashMap<String, MindModelAnswer>,
}

impl MindModelStore {
    /// 创建空缓存表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入/覆盖一条预计算答案；返回被替换的旧值（若有）。
    pub fn insert(
        &mut self,
        key: impl Into<String>,
        answer: MindModelAnswer,
    ) -> Option<MindModelAnswer> {
        self.entries.insert(key.into(), answer)
    }

    /// O(1) 读取，零模型调用。返回 `None` 表示未命中，调用方需计算后回填。
    #[must_use]
    pub fn lookup(&self, key: &str) -> Option<&MindModelAnswer> {
        self.entries.get(key)
    }

    /// 失效事件触发：丢弃所有带 `tag` 的条目，下次 `lookup` 未命中即重算（失效重写）。
    /// 返回被失效的条目数。
    pub fn invalidate_by_tag(&mut self, tag: &str) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|_, v| !v.invalidation_tags.iter().any(|t| t == tag));
        before - self.entries.len()
    }

    /// 当前缓存条目数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 缓存是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试码 unwrap 为 Rust 惯用法；E-5 lint 意在治理生产码
mod tests {
    use super::*;

    fn answer(text: &str, tags: &[&str]) -> MindModelAnswer {
        MindModelAnswer {
            answer: text.into(),
            source_refs: vec!["src-1".into()],
            invalidation_tags: tags.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn lookup_hit_and_miss() {
        let mut s = MindModelStore::new();
        assert!(s.is_empty());
        s.insert("what-is-x", answer("X is 42", &["obj-1"]));
        assert_eq!(s.len(), 1);
        assert_eq!(
            s.lookup("what-is-x").map(|a| a.answer.as_str()),
            Some("X is 42")
        );
        assert!(s.lookup("unknown").is_none());
    }

    #[test]
    fn insert_overwrites_and_returns_prev() {
        let mut s = MindModelStore::new();
        s.insert("k", answer("v1", &[]));
        let prev = s.insert("k", answer("v2", &[]));
        assert_eq!(prev.map(|a| a.answer), Some("v1".to_string()));
        assert_eq!(s.lookup("k").unwrap().answer, "v2");
    }

    #[test]
    fn invalidate_by_tag_forces_recompute() {
        let mut s = MindModelStore::new();
        s.insert("q1", answer("a1", &["obj-1", "shared"]));
        s.insert("q2", answer("a2", &["obj-2"]));
        s.insert("q3", answer("a3", &["shared"]));
        // obj 无关的 shared 失效命中 q1/q3
        assert_eq!(s.invalidate_by_tag("shared"), 2);
        assert_eq!(s.len(), 1);
        assert!(s.lookup("q1").is_none());
        assert!(s.lookup("q2").is_some());
        // 失效后未命中 → 调用方可重算回填（失效重写语义）
        s.insert("q1", answer("a1-recomputed", &["shared"]));
        assert_eq!(s.lookup("q1").unwrap().answer, "a1-recomputed");
    }

    #[test]
    fn invalidate_unknown_tag_is_noop() {
        let mut s = MindModelStore::new();
        s.insert("k", answer("v", &["t"]));
        assert_eq!(s.invalidate_by_tag("nope"), 0);
        assert_eq!(s.len(), 1);
    }
}
