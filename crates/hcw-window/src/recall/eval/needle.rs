//! 针测试任务 — 单针 + 多针（1/4/8 needle）评测
//!
//! 对应 PROBE P0.2：四类评测基准之一（needle_recall@8 目标 ≥ 90%）
//!
//! # 设计
//! - 单针任务：语料中埋 1 条事实针，query 询问该事实 → recall@tier（0/1）
//! - 多针任务：语料中埋 4/8 条针，全部召回才算满分 → needle_recall@k（命中数/k）
//! - 针 = 语料中带 ground truth 标记的块（`EvalCorpus::needle_ids`）
//! - 选中块集由外部选择路径产生（Static/recall 管线），本模块只做指标计算

use nexus_core::CLV;

use super::{needle_hit, needle_recall_at_k, BlockId, CorpusBuilder, EvalCorpus, EvalError};

/// 针测试任务 — 单针或多针评测的输入描述
///
/// # 字段
/// - `corpus`: 评测语料（含针块 ground truth）
/// - `query_clv`: 查询向量（探针打分场景下为 query 的 CLV；静态路径下可为零向量）
/// - `k`: 召回深度（多针目标 @8）
#[derive(Debug, Clone)]
pub struct NeedleTask {
    /// 评测语料（针块标记为 ground truth）
    pub corpus: EvalCorpus,
    /// 查询向量（探针打分输入；静态路径可为 `CLV::zero()`）
    pub query_clv: CLV,
    /// 召回深度 k（目标 needle_recall@8 → k = 8）
    pub k: usize,
}

impl NeedleTask {
    /// 创建针测试任务
    ///
    /// # 参数
    /// - `corpus`: 评测语料
    /// - `query_clv`: 查询向量
    /// - `k`: 召回深度
    pub fn new(corpus: EvalCorpus, query_clv: CLV, k: usize) -> Self {
        Self {
            corpus,
            query_clv,
            k,
        }
    }

    /// 返回针块 ID 列表（语料 ground truth，排序保证确定性）
    pub fn needle_ids(&self) -> Vec<BlockId> {
        self.corpus.needle_ids_sorted()
    }

    /// 计算多针召回率 needle_recall@k
    ///
    /// # 参数
    /// - `selected`: 选中块集（≤ k 个，由选择路径产生）
    ///
    /// # 返回值
    /// ∈ [0.0, 1.0]，命中针数 / 针总数
    pub fn recall_at_k(&self, selected: &[BlockId]) -> f32 {
        needle_recall_at_k(selected, &self.corpus.needle_ids)
    }

    /// 计算单针召回（单针任务专用）
    ///
    /// # 参数
    /// - `selected`: 选中块集
    ///
    /// # 返回值
    /// 0.0（未命中）或 1.0（命中）
    ///
    /// WHY: 单针任务只有一根针，recall 为 0/1 布尔值（浮点化便于对照表汇总）
    pub fn single_recall(&self, selected: &[BlockId]) -> f32 {
        let ids = self.needle_ids();
        if ids.len() != 1 {
            // 非单针语料：退化为多针口径（仍可评分）
            return self.recall_at_k(selected);
        }
        f32::from(needle_hit(selected, &ids[0]))
    }
}

/// 构造单针评测语料（1 针 + 干扰块）
///
/// # 参数
/// - `block_count`: 总块数（≥ 1）
///
/// # 返回值
/// 含 1 根针的语料；构造失败返回 `EvalError::InvalidCorpus`
pub fn single_needle_corpus(block_count: usize) -> Result<EvalCorpus, EvalError> {
    CorpusBuilder::new()
        .with_block_count(block_count)
        .with_needle_count(1)
        .build()
}

/// 构造多针评测语料（`needle_count` 根针 + 干扰块）
///
/// # 参数
/// - `block_count`: 总块数（≥ needle_count）
/// - `needle_count`: 针数（目标 4/8）
///
/// # 返回值
/// 含指定针数的语料；构造失败返回 `EvalError::InvalidCorpus`
pub fn multi_needle_corpus(
    block_count: usize,
    needle_count: usize,
) -> Result<EvalCorpus, EvalError> {
    CorpusBuilder::new()
        .with_block_count(block_count)
        .with_needle_count(needle_count)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_needle_task() {
        let corpus = single_needle_corpus(64).expect("build should succeed");
        let task = NeedleTask::new(corpus.clone(), CLV::zero(), 8);
        assert_eq!(task.needle_ids().len(), 1);

        // 命中 → 1.0
        let hit: Vec<BlockId> = vec![task.needle_ids()[0].clone()];
        assert_eq!(task.single_recall(&hit), 1.0);
        // 未命中 → 0.0
        assert_eq!(task.single_recall(&["miss".into()]), 0.0);
        // 空 → 0.0
        assert_eq!(task.single_recall(&[]), 0.0);
    }

    #[test]
    fn test_multi_needle_task_at_8() {
        let corpus = multi_needle_corpus(128, 8).expect("build should succeed");
        let task = NeedleTask::new(corpus, CLV::zero(), 8);
        let needles = task.needle_ids();
        assert_eq!(needles.len(), 8);

        // 全中 → 1.0
        let all: Vec<BlockId> = corpus_blocks(&task);
        assert!((task.recall_at_k(&all) - 1.0).abs() < 1e-6);
        // 中 4 → 0.5
        let half = needles.iter().take(4).cloned().collect::<Vec<_>>();
        assert!((task.recall_at_k(&half) - 0.5).abs() < 1e-6);
        // 空 → 0.0
        assert_eq!(task.recall_at_k(&[]), 0.0);
    }

    #[test]
    fn test_invalid_corpus() {
        assert!(single_needle_corpus(0).is_err());
        assert!(multi_needle_corpus(4, 8).is_err());
    }

    /// 测试辅助：取语料全部块 ID
    fn corpus_blocks(task: &NeedleTask) -> Vec<BlockId> {
        task.corpus.blocks.iter().map(|b| b.id.clone()).collect()
    }
}
