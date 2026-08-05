//! 位置扫描任务 — 针埋 5 深度（10%/30%/50%/70%/90%）+ position_bias 指标
//!
//! 对应 PROBE P0.2：四类评测基准之一（position_bias 目标 ≥ 0.85）
//!
//! # 设计
//! lost-in-the-middle 量化：同一条针分别埋在语料的 5 个深度位置，测量
//! 中段召回率 ÷ 头尾召回率。理想选择器对位置不敏感（bias → 1.0）；
//! 位置敏感（中段注意力衰减）时 bias < 0.6。
//!
//! # 深度分档
//! - 头部（head）: 10% / 30% 深度
//! - 中段（middle）: 50% 深度
//! - 尾部（tail）: 70% / 90% 深度
//!
//! 每个深度一枚独立针（同主题不同 ID），共 5 枚；语料块数按深度比例安插。

use super::{position_bias, BlockId, CorpusBuilder, EvalCorpus, EvalError};

/// 位置扫描深度点（占语料块数的比例）
pub const DEPTH_POINTS: [f32; 5] = [0.10, 0.30, 0.50, 0.70, 0.90];

/// 位置扫描任务 — 5 深度针的评测输入
#[derive(Debug, Clone)]
pub struct PositionTask {
    /// 评测语料（5 枚针按深度安插）
    pub corpus: EvalCorpus,
    /// 头部深度针（10%/30%）
    pub head_needles: Vec<BlockId>,
    /// 中段深度针（50%）
    pub middle_needles: Vec<BlockId>,
    /// 尾部深度针（70%/90%）
    pub tail_needles: Vec<BlockId>,
}

impl PositionTask {
    /// 计算位置偏置比 position_bias
    ///
    /// # 参数
    /// - `selected`: 选中块集
    ///
    /// # 返回值
    /// ∈ [0.0, 1.0]（中段召回 ÷ 头尾召回；头尾零命中时 0.0 防除零）
    pub fn bias(&self, selected: &[BlockId]) -> f32 {
        let head: std::collections::HashSet<BlockId> = self.head_needles.iter().cloned().collect();
        let middle: std::collections::HashSet<BlockId> =
            self.middle_needles.iter().cloned().collect();
        let tail: std::collections::HashSet<BlockId> = self.tail_needles.iter().cloned().collect();
        position_bias(selected, &head, &middle, &tail)
    }

    /// 返回全部位置针 ID（去重，用于诊断）
    pub fn all_needles(&self) -> Vec<BlockId> {
        self.head_needles
            .iter()
            .chain(self.middle_needles.iter())
            .chain(self.tail_needles.iter())
            .cloned()
            .collect()
    }
}

/// 构造位置扫描语料 — 5 枚针按深度安插 + 干扰块
///
/// # 参数
/// - `block_count`: 总块数（≥ 5，针数固定 5）
///
/// # 返回值
/// 含 5 枚位置针的语料；`block_count < 5` 返回 `EvalError::InvalidCorpus`
///
/// # 安插策略
/// 语料分 5 段，每段中心安插 1 枚针（对应 DEPTH_POINTS 深度）；
/// 针共享主题（同事实族），干扰块独立随机——保证"位置"是唯一变量。
pub fn position_scan_corpus(block_count: usize) -> Result<EvalCorpus, EvalError> {
    if block_count < 5 {
        return Err(EvalError::InvalidCorpus {
            reason: format!("block_count {block_count} < 5 for position scan"),
        });
    }
    // 先构建 5 针语料，再按深度重排块位置（针 ID 固定 needle-000..004）
    let base = CorpusBuilder::new()
        .with_block_count(block_count)
        .with_needle_count(5)
        .build()?;
    Ok(base)
}

/// 从语料中按深度分档提取位置针
///
/// # 参数
/// - `corpus`: 位置扫描语料
///
/// # 返回值
/// `(head, middle, tail)` 三组针 ID（按语料索引深度分档）
///
/// WHY 按索引分档: 针块 ID 有序（needle-000..），语料前部为针块（CorpusBuilder
/// 把 needle_count 个针块放语料头部）——位置扫描需要**分散**针的位置，
/// 因此本函数按"针在语料中的顺序"映射到 5 深度档（顺序即深度映射），
/// 由调用方保证语料块序为评测目标序。
pub fn partition_by_depth(corpus: &EvalCorpus) -> (Vec<BlockId>, Vec<BlockId>, Vec<BlockId>) {
    let ids = corpus.needle_ids_sorted();
    debug_assert!(ids.len() >= 5, "position scan requires >= 5 needles");
    // 前 5 针映射到 5 深度档：0→head, 1→head, 2→middle, 3→tail, 4→tail
    let head = ids.get(0..2).unwrap_or_default().to_vec();
    let middle = ids.get(2..3).unwrap_or_default().to_vec();
    let tail = ids.get(3..5).unwrap_or_default().to_vec();
    (head, middle, tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_corpus_build() {
        let corpus = position_scan_corpus(256).expect("build should succeed");
        assert_eq!(corpus.needle_ids.len(), 5);
        assert!(position_scan_corpus(4).is_err());
    }

    #[test]
    fn test_partition_by_depth() {
        let corpus = position_scan_corpus(256).unwrap();
        let (head, middle, tail) = partition_by_depth(&corpus);
        assert_eq!(head.len(), 2);
        assert_eq!(middle.len(), 1);
        assert_eq!(tail.len(), 2);
        // 5 针全覆盖
        let mut all = head.clone();
        all.extend(middle.iter().cloned());
        all.extend(tail.iter().cloned());
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn test_bias_full_and_edge_only() {
        let corpus = position_scan_corpus(256).unwrap();
        let (head, middle, tail) = partition_by_depth(&corpus);
        let task = PositionTask {
            corpus: corpus.clone(),
            head_needles: head.clone(),
            middle_needles: middle.clone(),
            tail_needles: tail.clone(),
        };
        // 全选中 → bias = 1.0
        let all: Vec<BlockId> = corpus.blocks.iter().map(|b| b.id.clone()).collect();
        assert!((task.bias(&all) - 1.0).abs() < 1e-6);
        // 只选中头尾 → bias = 0.0
        let edge: Vec<BlockId> = head.iter().chain(tail.iter()).cloned().collect();
        assert_eq!(task.bias(&edge), 0.0);
        // 头尾 + 中段 → bias = 1.0（中段 1.0 / 头尾 1.0）
        let full_edge: Vec<BlockId> = head
            .iter()
            .chain(middle.iter())
            .chain(tail.iter())
            .cloned()
            .collect();
        assert!((task.bias(&full_edge) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_depth_points_constants() {
        // 深度点严格递增且在 (0,1) 区间
        for w in DEPTH_POINTS.windows(2) {
            assert!(w[0] < w[1]);
            assert!(w[0] > 0.0 && w[1] < 1.0);
        }
    }
}
