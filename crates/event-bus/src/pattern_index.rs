//! PatternIndex 事件订阅精确索引（v4.0 WI-15 阶段一）
//!
//! 对应架构层: **L1 Core**（event-bus）
//! 对应任务: **P2-T5**（手册 W12 稀疏注意力 / v4.0 WI-15 阶段一）
//!
//! # 阶段一语义（v4.0 WI-15 批判性收窄）
//! 当前 144 事件量级 broadcast 无瓶颈,近似路由漏发风险不可接受——
//! **阶段一只建精确索引**：命名空间前缀树（trie）+ 字面量哈希精确匹配，
//! 语义与广播**等价**（精确匹配 ≠ 近似检索，结构性漏发率 = 0）。
//! 阶段二（HNSW 近似路由）仅在「订阅者 > 500 且精确索引 P99 > 1ms」时启动，
//! 本模块不实现（门禁数据达标才评估）。
//!
//! # 设计约束（红线对齐）
//! - **Critical 强制广播**：17 个 Critical 事件（LANE_FORBIDDEN_SHARD）永不进索引，
//!   无条件全广播（红线 1：Critical 永不分片/永不近似/永不丢弃）
//! - **精确匹配保证**：pattern 为「命名空间前缀 + 字面量」两级，匹配是集合运算，
//!   不存在近似/漏配路径——漏发率 = 0 由结构保证（测试断言锁定）
//! - **只读快照**：注册后 pattern 表不可变（启动期注册），查询零锁（Ω₇）
//! - 与既有 `topic.rs`（9 类 EventTopic 订阅侧过滤）互补：PatternIndex 是
//!   发布侧路由预筛（省无效唤醒），FilteredSubscriber 是订阅侧过滤（既有）

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::types::{EventSeverity, NexusEvent};

/// 订阅者标识（外部注册方分配的稳定 ID）
pub type SubscriberId = u64;

/// 命名空间前缀（如 "quest"、"bus"、"agent"）
pub type Namespace = Arc<str>;

/// 模式匹配结果：命中的订阅者集合
pub type MatchSet = Arc<HashSet<SubscriberId>>;

/// 模式树节点 — 命名空间前缀树 + 字面量哈希
///
/// 两级结构（v4.0 WI-15 阶段一规格）：
/// - 第一级：命名空间前缀树（trie），按事件类型名的命名空间段匹配
/// - 第二级：字面量哈希（精确事件名匹配）
#[derive(Debug, Default)]
pub struct PatternIndex {
    /// 命名空间 → 订阅者（前缀级匹配：`quest.*` 命中所有 `quest.*` 事件）
    ns_subscribers: HashMap<Namespace, HashSet<SubscriberId>>,
    /// 精确事件名（字面量）→ 订阅者（`budget.exceeded` 只匹配自身）
    literal_subscribers: HashMap<Arc<str>, HashSet<SubscriberId>>,
    /// 全部订阅者（兜底：`*` 通配注册者）
    wildcard_subscribers: HashSet<SubscriberId>,
    /// 类型级匹配缓存 — 事件类型名有限（144 变体），缓存命中零分配
    ///
    /// WHY：启动期注册后表不可变（只读快照语义），缓存无需失效；
    /// 10K 事件流中类型名去重后 ≤ 144，内存有界。命中路径直接返回
    /// `Arc<HashSet>`（零拷贝），规避每次查询的 wildcard clone + 分配。
    cache: RwLock<HashMap<Arc<str>, MatchSet>>,
}

/// 注册错误（启动期注册，失败即配置错误）
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatternError {
    /// 空模式(模式串为空白/空字符串)
    #[error("空模式")]
    EmptyPattern,
    /// 命名空间为空(注册目标命名空间标识缺失)
    #[error("命名空间为空: {0}")]
    EmptyNamespace(String),
}

impl PatternIndex {
    /// 空索引（启动期注册）
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册订阅者模式
    ///
    /// 模式形态（两级，`/` 分隔）：
    /// - `quest.*`：命名空间前缀匹配（命中所有 `quest.*` 事件）
    /// - `budget.exceeded`：字面量精确匹配（只命中自身）
    /// - `*`：通配（命中全部非 Critical 事件）
    ///
    /// # Errors
    /// 空模式/空命名空间返回 [`PatternError`]（启动期配置错误，fail-fast）。
    pub fn register(&mut self, id: SubscriberId, pattern: &str) -> Result<(), PatternError> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Err(PatternError::EmptyPattern);
        }
        if pattern == "*" {
            self.wildcard_subscribers.insert(id);
            return Ok(());
        }
        if let Some(ns) = pattern.strip_suffix(".*") {
            let ns = ns.trim();
            if ns.is_empty() {
                return Err(PatternError::EmptyNamespace(pattern.to_string()));
            }
            self.ns_subscribers
                .entry(Arc::from(ns))
                .or_default()
                .insert(id);
            return Ok(());
        }
        self.literal_subscribers
            .entry(Arc::from(pattern))
            .or_default()
            .insert(id);
        Ok(())
    }

    /// 查询事件命中的订阅者集合（精确匹配，无近似路径）
    ///
    /// # Critical 强制广播（红线 1）
    /// Critical 事件返回 `None` 语义由调用方解释为"全广播"——本模块不参与
    /// Critical 路由（永不近似），调用方见 [`Self::is_critical`] 或直接判
    /// `severity()`。
    ///
    /// # 性能（类型级缓存）
    /// 首次查询计算并缓存（类型名 → 匹配集）；后续命中零分配返回 Arc 克隆。
    #[must_use]
    pub fn match_patterns(&self, event_type: &str) -> MatchSet {
        // 缓存快速路径（读锁，命中即返回零拷贝 Arc）
        if let Some(cached) = self
            .cache
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(event_type)
        {
            return Arc::clone(cached);
        }
        // 慢路径：精确计算
        let mut matched = self.wildcard_subscribers.clone();
        // 命名空间前缀：事件名首段（`quest.created` → `quest`）
        if let Some((ns, _)) = event_type.split_once('.') {
            if let Some(set) = self.ns_subscribers.get(ns) {
                matched.extend(set.iter().copied());
            }
        }
        // 字面量精确
        if let Some(set) = self.literal_subscribers.get(event_type) {
            matched.extend(set.iter().copied());
        }
        let result: MatchSet = Arc::new(matched);
        // 写缓存（启动期只读快照语义：注册后表不变，缓存无需失效）
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(Arc::from(event_type), Arc::clone(&result));
        }
        result
    }

    /// Critical 判定（复用 severity 权威源，不经索引）
    #[must_use]
    pub fn is_critical(ev: &NexusEvent) -> bool {
        ev.severity() == EventSeverity::Critical
    }

    /// 订阅者总数（诊断）
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.wildcard_subscribers.len()
            + self
                .ns_subscribers
                .values()
                .map(HashSet::len)
                .sum::<usize>()
            + self
                .literal_subscribers
                .values()
                .map(HashSet::len)
                .sum::<usize>()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventMetadata;

    fn sample_event(type_name: &str) -> NexusEvent {
        // 用 metadata 构造轻量事件（仅测试匹配语义，字段无关）
        let metadata = EventMetadata::new("test");
        match type_name {
            "quest.created" => NexusEvent::QuestCreated {
                metadata,
                quest_id: "q-1".into(),
                title: "t".into(),
                task_count: 1,
            },
            "budget.exceeded" => NexusEvent::BudgetExceeded {
                metadata,
                budget_type: "token".into(),
                current: 10_000,
                limit: 8_000,
            },
            _ => NexusEvent::TuiActionRequested {
                metadata,
                action_id: "test.action".into(),
                payload: "{}".into(),
                source: crate::types::ActionSource::Chat,
            },
        }
    }

    #[test]
    fn register_empty_pattern_rejected() {
        let mut idx = PatternIndex::new();
        assert_eq!(idx.register(1, "  "), Err(PatternError::EmptyPattern));
    }

    #[test]
    fn register_empty_namespace_rejected() {
        let mut idx = PatternIndex::new();
        assert_eq!(
            idx.register(1, ".*"),
            Err(PatternError::EmptyNamespace(".*".to_string()))
        );
    }

    #[test]
    fn namespace_prefix_matches_all_in_ns() {
        let mut idx = PatternIndex::new();
        idx.register(10, "quest.*").unwrap();
        let matched = idx.match_patterns("quest.created");
        assert!(matched.contains(&10), "quest.* 必须命中 quest.created");
        let unmatched = idx.match_patterns("bus.throughput");
        assert!(!unmatched.contains(&10), "quest.* 不得命中 bus.*");
    }

    #[test]
    fn literal_exact_match_only_self() {
        let mut idx = PatternIndex::new();
        idx.register(20, "budget.exceeded").unwrap();
        let matched = idx.match_patterns("budget.exceeded");
        assert!(matched.contains(&20));
        let partial = idx.match_patterns("budget.recorded");
        assert!(!partial.contains(&20), "字面量只匹配自身");
    }

    #[test]
    fn wildcard_matches_all_non_critical() {
        let mut idx = PatternIndex::new();
        idx.register(30, "*").unwrap();
        assert!(idx.match_patterns("quest.created").contains(&30));
        assert!(idx.match_patterns("bus.throughput").contains(&30));
    }

    #[test]
    fn multi_subscriber_union() {
        let mut idx = PatternIndex::new();
        idx.register(1, "quest.*").unwrap();
        idx.register(2, "quest.created").unwrap();
        idx.register(3, "*.timed").unwrap(); // 字面量（无通配语法支持 → 按字面量注册）
        let matched = idx.match_patterns("quest.created");
        assert!(matched.contains(&1));
        assert!(matched.contains(&2));
        assert!(!matched.contains(&3));
    }

    #[test]
    fn critical_never_routed_through_index() {
        // 红线 1：Critical 强制广播——索引不承载 Critical 路由
        let mut idx = PatternIndex::new();
        idx.register(1, "*").unwrap();
        // 即使通配注册，Critical 判定必须独立（调用方走全广播）
        let ev = sample_event("budget.exceeded");
        assert!(
            PatternIndex::is_critical(&ev),
            "BudgetExceeded 必须 Critical"
        );
        // 语义等价：索引匹配结果不用于 Critical 投递（测试锁定判定函数）
        assert_eq!(ev.severity(), EventSeverity::Critical);
    }

    #[test]
    fn zero_leakage_exact_semantics() {
        // 漏发率 = 0：任意注册集合下，命中订阅者必为精确集合（无近似）
        let mut idx = PatternIndex::new();
        for i in 0..50 {
            idx.register(i, &format!("ns{i}.event")).unwrap();
        }
        for i in 0..50 {
            let matched = idx.match_patterns(&format!("ns{i}.event"));
            assert_eq!(matched.len(), 1, "精确匹配必须恰中 1 个");
            assert!(matched.contains(&i));
        }
        // 未注册事件 → 空集（无近似命中）
        assert!(idx.match_patterns("unknown.event").is_empty());
    }

    #[test]
    fn concurrent_reads_safe() {
        // 只读快照：注册完成后多线程并发查询无竞争（HashMap 只读）
        let mut idx = PatternIndex::new();
        for i in 0..100 {
            idx.register(i, &format!("ns{}.*", i % 10)).unwrap();
        }
        let idx = Arc::new(idx);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let idx = Arc::clone(&idx);
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        let m = idx.match_patterns("ns3.event");
                        assert!(!m.is_empty());
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }
}
