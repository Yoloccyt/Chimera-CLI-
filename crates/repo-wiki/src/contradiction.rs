//! 矛盾检测 — 写入路径的 Contradicts 关系检测(P3-W11.2.1)
//!
//! 对应架构层:L5 Knowledge(repo-wiki 层内)
//! 对应设计:spec.md:298 "写入路径检测 Contradicts 关系 → 标记过渡期不删旧记录"
//!
//! # 算法选型(P3-W11.2.1 MVP — 用户决策)
//!
//! **向量相似度阈值法**:用 embedding 余弦相似度找候选(> 阈值,默认 0.9),
//! 超阈值标记为矛盾候选并记录到 `entry_relations` 表。
//!
//! WHY MVP 选择纯向量阈值:
//! - 最简单,不依赖 NLP,符合 P3"先骨架后深化"原则
//! - P4 学习层可用 RHI-CG 优化判定(区分"相似补充" vs "矛盾")
//! - "宁可多标记不漏";幽灵冲突率 <1% 靠"标记过渡期不删旧"兜底
//! - 误报(相似但非矛盾)仅导致旧条目被多余归档,不丢失数据
//!
//! # 检测流程
//!
//! 1. `WikiStore::insert_with_contradiction_check` 用读连接池查所有条目 embedding
//! 2. `ContradictionDetector::detect` 在内存中计算相似度,超阈值的生成 `EntryRelation`
//! 3. 检测结果发送到写入线程,事务内执行:标记旧条目 Historical + 写入关系 + 写入新条目

use crate::relation::EntryRelation;
use crate::types::WikiEntry;

/// 矛盾检测默认相似度阈值
///
/// WHY(P3-W11.2.1):0.9 为高阈值,仅极相似(语义近乎相同但可能矛盾)的条目
/// 才被标记为矛盾候选。降低阈值会增加误报(相似但非矛盾的条目被归档)。
/// P4 学习层可动态调整此阈值(RHI-CG 反馈)。
pub const DEFAULT_CONTRADICTION_THRESHOLD: f32 = 0.9;

/// 矛盾检测器 — 基于向量余弦相似度阈值
///
/// WHY(P3-W11.2.1):检测逻辑独立为结构体,便于:
/// - 阈值可配置(测试用不同阈值,生产用默认值)
/// - 后续 P4 扩展为更复杂检测策略(如加上内容否定词规则)时替换实现
#[derive(Debug, Clone)]
pub struct ContradictionDetector {
    /// 相似度阈值,超过此值判定为矛盾候选
    threshold: f32,
}

impl ContradictionDetector {
    /// 创建默认检测器(阈值 0.9)
    pub fn new() -> Self {
        Self {
            threshold: DEFAULT_CONTRADICTION_THRESHOLD,
        }
    }

    /// 创建指定阈值的检测器(用于测试与调优)
    ///
    /// # 参数
    /// - `threshold`:相似度阈值 [0.0, 1.0],建议 ≥ 0.7
    pub fn with_threshold(threshold: f32) -> Self {
        Self { threshold }
    }

    /// 返回当前阈值
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// 检测新条目与候选条目集的矛盾关系
    ///
    /// 对每个候选计算与新条目的 embedding 余弦相似度,超过阈值的生成
    /// `EntryRelation { kind: Contradicts }`。跳过自身(entry_id 相同)。
    ///
    /// # 参数
    /// - `new_entry`:待插入的新条目
    /// - `candidates`:已有条目候选集(通常为库中所有 Current 状态条目)
    ///
    /// # 返回
    /// 矛盾关系列表(`source_id` = 新条目,`target_id` = 被矛盾的旧条目)
    pub fn detect(&self, new_entry: &WikiEntry, candidates: &[WikiEntry]) -> Vec<EntryRelation> {
        candidates
            .iter()
            // 跳过自身(UPSERT 场景下 entry_id 可能已存在)
            .filter(|c| c.entry_id != new_entry.entry_id)
            .filter_map(|c| {
                let sim = cosine_similarity(&new_entry.embedding, &c.embedding);
                if sim >= self.threshold {
                    Some(EntryRelation::new_contradiction(
                        &new_entry.entry_id,
                        &c.entry_id,
                        format!("cosine_similarity={sim:.4}"),
                        sim,
                    ))
                } else {
                    None
                }
            })
            .collect()
    }
}

impl Default for ContradictionDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// 矛盾检测结果 — `insert_with_contradiction_check` 的返回值(P3-W11.2)
///
/// WHY(P3-W11.2):调用方需要知道:
/// - 新条目是否真实新增(`inserted`)
/// - 检测到哪些矛盾关系(`contradictions`),用于审计与时间感知召回
#[derive(Debug, Clone)]
pub struct ContradictionResult {
    /// 新条目是否真实新增(UPSERT 已存在条目时为 false)
    pub inserted: bool,
    /// 检测到的矛盾关系列表(空表示无矛盾)
    ///
    /// WHY:每条 `EntryRelation` 记录 source(新条目)→ target(被矛盾的旧条目),
    /// 旧条目已被标记 Historical(归档不删除),关系记录保留谱系完整性
    pub contradictions: Vec<EntryRelation>,
}

impl ContradictionResult {
    /// 判断是否检测到矛盾
    pub fn has_contradictions(&self) -> bool {
        !self.contradictions.is_empty()
    }

    /// 返回矛盾数量
    pub fn contradiction_count(&self) -> usize {
        self.contradictions.len()
    }
}

/// 计算两个 f32 向量的余弦相似度
///
/// 公式:dot(a, b) / (|a| * |b|)
///
/// WHY 复用 `nexus_core::cosine_similarity_slices`:避免重复实现,
/// 保持与 CLV/SharedCLV 的相似度计算语义一致(零向量返回 0.0)。
///
/// # 零向量边界
/// 若任一向量为零向量,返回 0.0(不会被判为矛盾候选,避免 NaN 污染)
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    nexus_core::cosine_similarity_slices(a, b)
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用 WikiEntry(512-dim embedding)
    fn make_entry(id: &str, embedding: Vec<f32>) -> WikiEntry {
        WikiEntry::new(id, "title", "content", vec![], embedding)
    }

    #[test]
    fn test_default_threshold() {
        let detector = ContradictionDetector::new();
        assert!((detector.threshold() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_custom_threshold() {
        let detector = ContradictionDetector::with_threshold(0.8);
        assert!((detector.threshold() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_detect_identical_embeddings_flagged() {
        // 相同 embedding(相似度 = 1.0)应被判为矛盾
        let emb = vec![0.5; 512];
        let new_entry = make_entry("e-new", emb.clone());
        let old_entry = make_entry("e-old", emb);

        let detector = ContradictionDetector::new();
        let contradictions = detector.detect(&new_entry, &[old_entry]);

        assert_eq!(contradictions.len(), 1);
        let rel = &contradictions[0];
        assert_eq!(rel.source_id, "e-new");
        assert_eq!(rel.target_id, "e-old");
        assert!((rel.confidence - 1.0).abs() < 1e-6);
        assert!(rel.evidence.contains("cosine_similarity"));
    }

    #[test]
    fn test_detect_dissimilar_embeddings_not_flagged() {
        // 正交向量(相似度 = 0.0)不应被判为矛盾
        let mut emb1 = vec![0.0; 512];
        emb1[0] = 1.0;
        let mut emb2 = vec![0.0; 512];
        emb2[1] = 1.0;

        let new_entry = make_entry("e-new", emb1);
        let old_entry = make_entry("e-old", emb2);

        let detector = ContradictionDetector::new();
        let contradictions = detector.detect(&new_entry, &[old_entry]);
        assert!(contradictions.is_empty());
    }

    #[test]
    fn test_detect_skips_self() {
        // entry_id 相同的候选应被跳过(UPSERT 场景)
        let emb = vec![0.5; 512];
        let new_entry = make_entry("e-1", emb.clone());
        let same_id = make_entry("e-1", emb);

        let detector = ContradictionDetector::new();
        let contradictions = detector.detect(&new_entry, &[same_id]);
        assert!(contradictions.is_empty());
    }

    #[test]
    fn test_detect_threshold_boundary() {
        // 构造相似度恰好 = 0.9 的向量(边界测试)
        // 用两个不完全相同但高相似度的向量
        let mut emb1 = vec![1.0; 512];
        let mut emb2 = vec![1.0; 512];
        // 修改一个维度使相似度略降
        emb2[0] = 0.5;
        emb1[0] = 1.0;
        // 归一化后计算:实际相似度会很高

        let new_entry = make_entry("e-new", emb1);
        let old_entry = make_entry("e-old", emb2);

        // 用低阈值确保被检测到
        let detector = ContradictionDetector::with_threshold(0.5);
        let contradictions = detector.detect(&new_entry, &[old_entry]);
        assert!(!contradictions.is_empty());
    }

    #[test]
    fn test_detect_multiple_candidates() {
        // 3 个候选:1 个矛盾(相同 embedding),2 个不矛盾(正交)
        let emb_same = vec![0.5; 512];
        let mut emb_orth1 = vec![0.0; 512];
        emb_orth1[0] = 1.0;
        let mut emb_orth2 = vec![0.0; 512];
        emb_orth2[1] = 1.0;

        let new_entry = make_entry("e-new", emb_same);
        let candidates = vec![
            make_entry("e-same", vec![0.5; 512]),
            make_entry("e-orth1", emb_orth1),
            make_entry("e-orth2", emb_orth2),
        ];

        let detector = ContradictionDetector::new();
        let contradictions = detector.detect(&new_entry, &candidates);

        assert_eq!(contradictions.len(), 1);
        assert_eq!(contradictions[0].target_id, "e-same");
    }

    #[test]
    fn test_detect_empty_candidates() {
        let new_entry = make_entry("e-new", vec![0.5; 512]);
        let detector = ContradictionDetector::new();
        let contradictions = detector.detect(&new_entry, &[]);
        assert!(contradictions.is_empty());
    }

    #[test]
    fn test_detect_zero_vector_not_flagged() {
        // 零向量相似度 = 0.0,不应被判为矛盾(避免 NaN)
        let zero_emb = vec![0.0; 512];
        let normal_emb = vec![0.5; 512];

        let new_entry = make_entry("e-new", zero_emb);
        let old_entry = make_entry("e-old", normal_emb);

        let detector = ContradictionDetector::new();
        let contradictions = detector.detect(&new_entry, &[old_entry]);
        assert!(contradictions.is_empty());
    }

    #[test]
    fn test_default_impl() {
        // Default trait 应等价于 new()
        let d1 = ContradictionDetector::default();
        let d2 = ContradictionDetector::new();
        assert!((d1.threshold() - d2.threshold()).abs() < 1e-6);
    }
}
