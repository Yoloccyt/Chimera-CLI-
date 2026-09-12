//! 记忆金字塔 — MSCE + TencentDB + Chimera MLC 四级融合（设计文档 §7.3）
//!
//! 对应架构层: **L2 Memory**（mlc-engine 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §7.3
//! 对应论文: MSCE（三层记忆）+ TencentDB Agent Memory（四层金字塔 + 检索三方式 + 注入策略）
//! 对应 ADR: ADR-049 决策 1（memory-pyramid 落点 mlc-engine，内嵌模块）
//!
//! # 核心职责
//!
//! 四层记忆金字塔（复用 L0 契约类型，Phase 0 落地）：
//! - **L0 Raw**: 全量原始对话（[`RawLogEntry`]）
//! - **L1 Atomic**: 结构化原子卡片（[`nexus_contracts::AtomicMemoryCard`]）
//! - **L2 Scene**: 场景档案（[`nexus_contracts::SceneBlock`]）
//! - **L3 Persona**: 人格摘要（[`nexus_contracts::PersonaSummary`]）
//!
//! 检索三方式（TencentDB）+ 降级链 + 注入策略：
//! - **字面检索** ([`LiteralSearcher`]): 子串包含匹配
//! - **语义检索** ([`SemanticSearcher`]): CLV 余弦相似度
//! - **混合排序** ([`HybridRanker`]): 字面 + 语义融合
//! - **降级链** ([`DegradationChain`]): Hybrid/SemanticOnly/KeywordOnly/Empty 四态
//! - **注入策略** ([`MemoryPyramid::inject_context`]): 动态卡片→用户消息前；人格→系统提示末尾
//!
//! # 设计约束
//!
//! - **铁律1（零 Python）**: `distill_atomic_cards` 的模型提炼用 Rust 规则占位实现，
//!   模型接线留待后续（文档如实声明）；接口形态保持与规范一致
//! - **铁律5（懒加载不阻塞）**: `retrieve` 软超时兜底（超 timeout_ms 返回空）
//! - **铁律3**: 只读消费 L0 卡片类型
//! - **f32 红线**: RetrievalResult.score 仅 PartialEq

use std::time::Instant;

use nexus_contracts::memory_pyramid::{
    AtomicCardType, AtomicMemoryCard, PersonaSummary, SceneBlock,
};
use nexus_core::CLV;

// ============================================================
// L0 原始日志
// ============================================================

/// 原始对话日志 — TencentDB L0（毫秒级落盘）
#[derive(Clone, Debug)]
pub struct RawLogEntry {
    /// 日志 ID
    pub id: String,
    /// 会话 ID
    pub session_id: String,
    /// 用户消息
    pub user_message: String,
    /// 助手消息
    pub assistant_message: String,
    /// 时间戳（Unix 毫秒）
    pub timestamp: u64,
}

// ============================================================
// L1 原子卡片条目（卡片 + 可选语义嵌入）
// ============================================================

/// L1 金字塔条目 — 原子卡片 + 可选 CLV 嵌入（语义检索用）
#[derive(Clone, Debug)]
pub struct PyramidL1Entry {
    /// 原子记忆卡片（L0 契约）
    pub card: AtomicMemoryCard,
    /// 语义嵌入（None = 仅字面可检索）
    pub embedding: Option<CLV>,
}

// ============================================================
// 检索结果
// ============================================================

/// 检索来源
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetrievalSource {
    /// 字面匹配
    Literal,
    /// 语义匹配
    Semantic,
    /// 混合排序
    Hybrid,
}

/// 检索结果 — 卡片 + 分数 + 来源
#[derive(Clone, Debug)]
pub struct RetrievalResult {
    /// 命中的原子卡片（克隆，铁律3 只读消费）
    pub card: AtomicMemoryCard,
    /// 检索分数（0.0-1.0）
    pub score: f32,
    /// 检索来源
    pub source: RetrievalSource,
}

impl RetrievalResult {
    /// 从字面命中构造（分数 1.0 表示精确包含）
    pub fn from_literal(card: AtomicMemoryCard) -> Self {
        Self {
            card,
            score: 1.0,
            source: RetrievalSource::Literal,
        }
    }
}

// ============================================================
// 降级链
// ============================================================

/// 检索策略 — 降级链决策结果
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetrieveStrategy {
    /// 混合（字面 + 语义）
    Hybrid,
    /// 仅语义
    SemanticOnly,
    /// 仅关键词（字面）
    KeywordOnly,
    /// 空（全部不可用）
    Empty,
}

/// 降级链 — TencentDB 故障容忍（语义/关键词可用性决策）
#[derive(Clone, Debug, Default)]
pub struct DegradationChain {
    /// 语义检索可用
    pub semantic_available: bool,
    /// 关键词检索可用
    pub keyword_available: bool,
}

impl DegradationChain {
    /// 创建降级链（默认全可用）
    pub fn new(semantic_available: bool, keyword_available: bool) -> Self {
        Self {
            semantic_available,
            keyword_available,
        }
    }

    /// 决策检索策略
    pub fn retrieve_strategy(&self) -> RetrieveStrategy {
        match (self.semantic_available, self.keyword_available) {
            (true, true) => RetrieveStrategy::Hybrid,
            (true, false) => RetrieveStrategy::SemanticOnly,
            (false, true) => RetrieveStrategy::KeywordOnly,
            (false, false) => RetrieveStrategy::Empty,
        }
    }
}

// ============================================================
// 检索三方式
// ============================================================

/// 字面检索器 — 子串包含匹配（content / scene 字段）
#[derive(Debug, Default, Clone, Copy)]
pub struct LiteralSearcher;

impl LiteralSearcher {
    /// 字面检索 — query 作为子串匹配 content/scene
    pub fn search(&self, query: &str, entries: &[PyramidL1Entry]) -> Vec<RetrievalResult> {
        let query_lower = query.to_lowercase();
        entries
            .iter()
            .filter(|e| {
                e.card.content.to_lowercase().contains(&query_lower)
                    || e.card.scene.to_lowercase().contains(&query_lower)
            })
            .map(|e| RetrievalResult::from_literal(e.card.clone()))
            .collect()
    }
}

/// 语义检索器 — CLV 余弦相似度（需卡片附带嵌入）
#[derive(Debug, Default, Clone, Copy)]
pub struct SemanticSearcher;

impl SemanticSearcher {
    /// 语义检索 — query CLV 与卡片嵌入余弦相似度，超阈值返回
    pub fn search(
        &self,
        query_clv: &CLV,
        entries: &[PyramidL1Entry],
        threshold: f32,
    ) -> Vec<RetrievalResult> {
        entries
            .iter()
            .filter_map(|e| {
                let embedding = e.embedding.as_ref()?;
                let sim = query_clv.cosine_similarity(embedding);
                if sim >= threshold {
                    Some(RetrievalResult {
                        card: e.card.clone(),
                        score: sim,
                        source: RetrievalSource::Semantic,
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

/// 混合排序器 — 字面 + 语义结果融合（按分数降序，字面命中加权）
#[derive(Debug, Default, Clone, Copy)]
pub struct HybridRanker;

impl HybridRanker {
    /// 混合排序 — 合并去重（按 card_id），按分数降序
    pub fn rank(
        &self,
        literal: Vec<RetrievalResult>,
        semantic: Vec<RetrievalResult>,
    ) -> Vec<RetrievalResult> {
        let mut merged: Vec<RetrievalResult> = Vec::new();
        // 字面命中优先标记为 Hybrid，分数取两者较大
        for mut lit in literal {
            lit.source = RetrievalSource::Hybrid;
            merged.push(lit);
        }
        for sem in semantic {
            // 去重: 已有同 card_id 的字面命中则跳过（字面优先）
            let already = merged.iter().any(|m| m.card.card_id == sem.card.card_id);
            if !already {
                let mut s = sem;
                s.source = RetrievalSource::Hybrid;
                merged.push(s);
            }
        }
        // 按分数降序（Top-K 规模小用 sort；遵循红线，大集合消费方自行 select_nth）
        merged.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        merged
    }
}

// ============================================================
// 记忆金字塔
// ============================================================

/// 记忆金字塔 — 四层记忆 + 检索三方式 + 注入策略
#[derive(Debug, Default)]
pub struct MemoryPyramid {
    /// L0 原始日志
    l0_raw_logs: Vec<RawLogEntry>,
    /// L1 原子卡片（含可选嵌入）
    l1_atomic: Vec<PyramidL1Entry>,
    /// L2 场景档案
    l2_scene_blocks: Vec<SceneBlock>,
    /// L3 人格摘要
    l3_personas: Vec<PersonaSummary>,
    /// 降级链
    degradation_chain: DegradationChain,
    /// 字面检索器
    literal_searcher: LiteralSearcher,
    /// 语义检索器
    semantic_searcher: SemanticSearcher,
    /// 混合排序器
    hybrid_ranker: HybridRanker,
    /// 语义检索相似度阈值
    semantic_threshold: f32,
}

impl MemoryPyramid {
    /// 创建记忆金字塔（降级链默认全可用，语义阈值 0.5）
    pub fn new() -> Self {
        Self {
            degradation_chain: DegradationChain::new(true, true),
            semantic_threshold: 0.5,
            ..Self::default()
        }
    }

    /// 设置降级链（故障容忍配置）
    pub fn with_degradation_chain(mut self, chain: DegradationChain) -> Self {
        self.degradation_chain = chain;
        self
    }

    /// L0/L1/L2/L3 各层条目数（可观测性）
    pub fn level_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.l0_raw_logs.len(),
            self.l1_atomic.len(),
            self.l2_scene_blocks.len(),
            self.l3_personas.len(),
        )
    }

    /// 写入原始对话 — TencentDB L0 落盘
    pub fn write_raw_log(
        &mut self,
        session_id: &str,
        user_msg: &str,
        assistant_msg: &str,
    ) -> String {
        let id = format!("raw-{}", self.l0_raw_logs.len());
        let entry = RawLogEntry {
            id: id.clone(),
            session_id: session_id.to_string(),
            user_message: user_msg.to_string(),
            assistant_message: assistant_msg.to_string(),
            timestamp: current_timestamp_ms(),
        };
        self.l0_raw_logs.push(entry);
        id
    }

    /// L0 → L1 提炼 — 原始日志转原子卡片
    ///
    /// **铁律1 占位**: 规范原型为后台模型调用（约 6 秒）；Rust 侧用规则提炼
    /// （每条日志生成一个 Event 类型卡片），模型接线留待后续。接口形态保持一致。
    pub fn distill_atomic_cards(&mut self, session_id: &str) -> Vec<AtomicMemoryCard> {
        let logs: Vec<&RawLogEntry> = self
            .l0_raw_logs
            .iter()
            .filter(|l| l.session_id == session_id)
            .collect();
        let mut cards = Vec::new();
        for log in logs {
            // 规则提炼: 用户消息作为 content，场景 = session
            let card = AtomicMemoryCard::new(
                &format!("atomic-{}", log.id),
                AtomicCardType::Event,
                100,
                session_id,
                &log.user_message,
                None,
                Some(&log.assistant_message),
                None,
                None,
                log.timestamp,
            );
            self.l1_atomic.push(PyramidL1Entry {
                card: card.clone(),
                embedding: None,
            });
            cards.push(card);
        }
        cards
    }

    /// 直接写入 L1 原子卡片（含可选嵌入，供外部提炼器使用）
    pub fn insert_atomic_card(&mut self, card: AtomicMemoryCard, embedding: Option<CLV>) {
        self.l1_atomic.push(PyramidL1Entry { card, embedding });
    }

    /// 写入 L2 场景档案
    pub fn insert_scene_block(&mut self, block: SceneBlock) {
        self.l2_scene_blocks.push(block);
    }

    /// 写入 L3 人格摘要
    pub fn insert_persona(&mut self, persona: PersonaSummary) {
        self.l3_personas.push(persona);
    }

    /// 检索三方式融合 — 降级链决策 + 软超时兜底（铁律5）
    ///
    /// - `query`: 检索词（字面）
    /// - `query_clv`: 语义查询向量（None = 仅字面）
    /// - `timeout_ms`: 软超时（超时返回已收集结果）
    pub fn retrieve(
        &self,
        query: &str,
        query_clv: Option<&CLV>,
        timeout_ms: u32,
    ) -> Vec<RetrievalResult> {
        let start = Instant::now();
        let strategy = self.degradation_chain.retrieve_strategy();
        if strategy == RetrieveStrategy::Empty {
            return Vec::new();
        }
        // 字面检索（KeywordOnly / Hybrid）
        let literal_results = if matches!(
            strategy,
            RetrieveStrategy::Hybrid | RetrieveStrategy::KeywordOnly
        ) {
            self.literal_searcher.search(query, &self.l1_atomic)
        } else {
            Vec::new()
        };
        // 语义检索（SemanticOnly / Hybrid，需 query_clv）
        let semantic_results = if matches!(
            strategy,
            RetrieveStrategy::Hybrid | RetrieveStrategy::SemanticOnly
        ) {
            query_clv
                .map(|clv| {
                    self.semantic_searcher
                        .search(clv, &self.l1_atomic, self.semantic_threshold)
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // 融合或降级
        let results = match strategy {
            RetrieveStrategy::Hybrid => self.hybrid_ranker.rank(literal_results, semantic_results),
            RetrieveStrategy::SemanticOnly => semantic_results,
            RetrieveStrategy::KeywordOnly => literal_results,
            RetrieveStrategy::Empty => Vec::new(),
        };
        // 软超时兜底（铁律5: 不阻塞主流程）
        if start.elapsed().as_millis() > timeout_ms as u128 {
            return Vec::new();
        }
        results
    }

    /// 注入策略 — 动态卡片→用户消息前；人格偏好→系统提示末尾（TencentDB 优化）
    pub fn inject_context(
        &self,
        retrieved: &[RetrievalResult],
        user_message: &mut String,
        system_prompt: &mut String,
    ) {
        // 动态卡片（非 Preference）取前 3，注入用户消息前
        let dynamic_cards: Vec<&RetrievalResult> = retrieved
            .iter()
            .filter(|r| r.card.card_type != AtomicCardType::Preference)
            .take(3)
            .collect();
        if !dynamic_cards.is_empty() {
            let injection = dynamic_cards
                .iter()
                .map(|r| format!("[记忆] {}: {}", r.card.scene, r.card.content))
                .collect::<Vec<_>>()
                .join("\n");
            *user_message = format!("{injection}\n\n{user_message}");
        }
        // 人格偏好卡片注入系统提示末尾
        let persona_cards: Vec<&RetrievalResult> = retrieved
            .iter()
            .filter(|r| r.card.card_type == AtomicCardType::Preference)
            .collect();
        if !persona_cards.is_empty() {
            let persona_injection = persona_cards
                .iter()
                .map(|r| r.card.content.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            *system_prompt = format!("{system_prompt}\n\n[用户画像] {persona_injection}");
        }
    }
}

/// 当前 Unix 毫秒时间戳
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn atomic_card(id: &str, content: &str, card_type: AtomicCardType) -> AtomicMemoryCard {
        AtomicMemoryCard::new(
            id,
            card_type,
            100,
            "scene-1",
            content,
            None,
            None,
            None,
            None,
            1_700_000_000_000,
        )
    }

    /// 单位 CLV: 指定维度置 1
    fn unit_clv(dim: usize) -> CLV {
        // 算法体已收敛到 L1 `nexus_core::CLV::basis`(单一权威构造器),
        // 此处仅保留本地签名,避免改动本文件内数十处调用点。
        // basis 越界返回 None;夹具若下标非法则该测试无效,直接 expect 暴露。
        CLV::basis(dim).expect("测试夹具:下标须在 CLV::DIMENSION 内")
    }

    #[test]
    fn write_raw_log_and_distill() {
        let mut pyramid = MemoryPyramid::new();
        pyramid.write_raw_log("s1", "如何修复 E0308?", "使用类型标注修复");
        pyramid.write_raw_log("s1", "如何优化性能?", "使用缓存");
        pyramid.write_raw_log("s2", "其他会话", "无关");
        let cards = pyramid.distill_atomic_cards("s1");
        assert_eq!(cards.len(), 2, "应只提炼 s1 会话的 2 条日志");
        assert_eq!(pyramid.level_counts().1, 2);
    }

    #[test]
    fn literal_search_substring_match() {
        // KeywordOnly 降级链验证纯字面检索（source 保持 Literal，不经混合排序）
        let mut pyramid =
            MemoryPyramid::new().with_degradation_chain(DegradationChain::new(false, true));
        pyramid.insert_atomic_card(
            atomic_card("c1", "修复 E0308 类型错误", AtomicCardType::Event),
            None,
        );
        pyramid.insert_atomic_card(
            atomic_card("c2", "性能优化缓存策略", AtomicCardType::Event),
            None,
        );
        let results = pyramid.retrieve("E0308", None, 1000);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].card.card_id.as_ref(), "c1");
        assert_eq!(results[0].source, RetrievalSource::Literal);
    }

    #[test]
    fn hybrid_strategy_marks_literal_hits_as_hybrid() {
        // 默认 Hybrid 策略：字面命中经混合排序标记为 Hybrid
        let mut pyramid = MemoryPyramid::new();
        pyramid.insert_atomic_card(
            atomic_card("c1", "修复 E0308 类型错误", AtomicCardType::Event),
            None,
        );
        let results = pyramid.retrieve("E0308", None, 1000);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, RetrievalSource::Hybrid);
    }

    #[test]
    fn semantic_search_by_clv_similarity() {
        let mut pyramid = MemoryPyramid::new();
        let clv_a = unit_clv(0);
        let clv_b = unit_clv(1);
        pyramid.insert_atomic_card(
            atomic_card("c1", "内容A", AtomicCardType::Event),
            Some(clv_a.clone()),
        );
        pyramid.insert_atomic_card(
            atomic_card("c2", "内容B", AtomicCardType::Event),
            Some(clv_b),
        );
        // query 与 c1 同向（相似度 1.0）
        let results = pyramid.retrieve("不匹配字面", Some(&clv_a), 1000);
        assert_eq!(results.len(), 1, "应语义命中 c1");
        assert_eq!(results[0].card.card_id.as_ref(), "c1");
        assert_eq!(results[0].source, RetrievalSource::Hybrid);
    }

    #[test]
    fn degradation_chain_four_states() {
        assert_eq!(
            DegradationChain::new(true, true).retrieve_strategy(),
            RetrieveStrategy::Hybrid
        );
        assert_eq!(
            DegradationChain::new(true, false).retrieve_strategy(),
            RetrieveStrategy::SemanticOnly
        );
        assert_eq!(
            DegradationChain::new(false, true).retrieve_strategy(),
            RetrieveStrategy::KeywordOnly
        );
        assert_eq!(
            DegradationChain::new(false, false).retrieve_strategy(),
            RetrieveStrategy::Empty
        );
    }

    #[test]
    fn empty_degradation_returns_nothing() {
        let mut pyramid =
            MemoryPyramid::new().with_degradation_chain(DegradationChain::new(false, false));
        pyramid.insert_atomic_card(atomic_card("c1", "E0308 修复", AtomicCardType::Event), None);
        let results = pyramid.retrieve("E0308", None, 1000);
        assert!(results.is_empty(), "全不可用降级链应返回空");
    }

    #[test]
    fn keyword_only_degradation_skips_semantic() {
        let mut pyramid =
            MemoryPyramid::new().with_degradation_chain(DegradationChain::new(false, true));
        let clv = unit_clv(5);
        pyramid.insert_atomic_card(
            atomic_card("c1", "E0308 修复", AtomicCardType::Event),
            Some(clv.clone()),
        );
        // 字面不匹配但语义可匹配——KeywordOnly 应不返回语义结果
        let results = pyramid.retrieve("无关词", Some(&clv), 1000);
        assert!(results.is_empty(), "KeywordOnly 降级不应返回语义结果");
    }

    #[test]
    fn inject_context_dynamic_cards_before_user_message() {
        let pyramid = MemoryPyramid::new();
        let retrieved = vec![RetrievalResult {
            card: atomic_card("c1", "修复方案", AtomicCardType::Event),
            score: 1.0,
            source: RetrievalSource::Hybrid,
        }];
        let mut user_msg = "请帮我实现".to_string();
        let mut system = "你是助手".to_string();
        pyramid.inject_context(&retrieved, &mut user_msg, &mut system);
        assert!(user_msg.contains("[记忆]"), "动态卡片应注入用户消息前");
        assert!(user_msg.ends_with("请帮我实现"), "原用户消息应在后");
        assert!(
            !system.contains("[用户画像]"),
            "非 Preference 不注入系统提示"
        );
    }

    #[test]
    fn inject_context_preference_to_system_prompt() {
        let pyramid = MemoryPyramid::new();
        let retrieved = vec![RetrievalResult {
            card: atomic_card("p1", "偏好简洁注释", AtomicCardType::Preference),
            score: 1.0,
            source: RetrievalSource::Hybrid,
        }];
        let mut user_msg = "实现功能".to_string();
        let mut system = "你是助手".to_string();
        pyramid.inject_context(&retrieved, &mut user_msg, &mut system);
        assert!(
            system.contains("[用户画像]"),
            "Preference 应注入系统提示末尾"
        );
        assert!(system.contains("偏好简洁注释"));
        assert!(!user_msg.contains("[记忆]"), "Preference 不注入用户消息");
    }

    #[test]
    fn hybrid_ranker_dedup_prefers_literal() {
        let ranker = HybridRanker;
        let card = atomic_card("c1", "内容", AtomicCardType::Event);
        let literal = vec![RetrievalResult::from_literal(card.clone())];
        let semantic = vec![RetrievalResult {
            card: card.clone(),
            score: 0.9,
            source: RetrievalSource::Semantic,
        }];
        let merged = ranker.rank(literal, semantic);
        assert_eq!(merged.len(), 1, "同 card_id 应去重");
        assert_eq!(merged[0].source, RetrievalSource::Hybrid);
    }

    #[test]
    fn four_level_counts_tracking() {
        let mut pyramid = MemoryPyramid::new();
        pyramid.write_raw_log("s1", "u", "a");
        pyramid.insert_atomic_card(atomic_card("c1", "x", AtomicCardType::Event), None);
        pyramid.insert_scene_block(SceneBlock::new("b1", "scene", vec![], "摘要"));
        pyramid.insert_persona(PersonaSummary::new(
            "p1",
            "u1",
            "画像",
            vec![],
            vec![],
            1_700_000_000_000,
        ));
        assert_eq!(pyramid.level_counts(), (1, 1, 1, 1));
    }
}
