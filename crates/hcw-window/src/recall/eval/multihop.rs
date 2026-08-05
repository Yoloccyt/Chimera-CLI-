//! 多跳推理任务 — RULER 式事实链（A→B→C）评测
//!
//! 对应 PROBE P0.2：四类评测基准之一（chain_success_rate 目标 ≥ 80%）
//!
//! # 设计
//! 3 条分散事实构成推理链（如"函数 X 调用了 Y，Y 调用了 Z"），
//! 缺一即错——量化选择器对"推理所需上下文完整性"的保障能力。
//! 每条链 = 3 枚同主题针（不同 ID）；选中集须包含链的全部 3 枚才计成功。

use super::{chain_success_rate, BlockId, CorpusBuilder, EvalCorpus, EvalError};

/// 单条链的事实块数（RULER 式 3 跳）
pub const CHAIN_LENGTH: usize = 3;

/// 多跳任务 — 事实链集合的评测输入
#[derive(Debug, Clone)]
pub struct MultihopTask {
    /// 评测语料（链针块 + 干扰块）
    pub corpus: EvalCorpus,
    /// 事实链集合（每链 = CHAIN_LENGTH 个针块 ID，按推理顺序）
    pub chains: Vec<Vec<BlockId>>,
}

impl MultihopTask {
    /// 计算链路成功率 chain_success_rate
    ///
    /// # 参数
    /// - `selected`: 选中块集
    ///
    /// # 返回值
    /// ∈ [0.0, 1.0]；`chains` 为空时 0.0
    pub fn success_rate(&self, selected: &[BlockId]) -> f32 {
        chain_success_rate(selected, &self.chains)
    }

    /// 返回全部链针 ID（去重，用于诊断）
    pub fn all_needles(&self) -> Vec<BlockId> {
        let mut ids: Vec<BlockId> = self.chains.iter().flatten().cloned().collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

/// 构造多跳评测语料 — 指定链数 × 每链 3 针 + 干扰块
///
/// # 参数
/// - `block_count`: 总块数（≥ chains × 3）
/// - `chain_count`: 事实链数（默认目标 ≥ 2 条）
///
/// # 返回值
/// 含 `chain_count × 3` 枚针的语料；块数不足返回 `EvalError::InvalidCorpus`
///
/// # 链构造
/// 针 ID 按链分组：`chain-{c}-{i}`（c 链号，i 跳序）；链顺序即语料针序。
pub fn multihop_corpus(block_count: usize, chain_count: usize) -> Result<MultihopTask, EvalError> {
    let needle_count = chain_count * CHAIN_LENGTH;
    if block_count < needle_count {
        return Err(EvalError::InvalidCorpus {
            reason: format!("block_count {block_count} < needles {needle_count}"),
        });
    }
    if chain_count == 0 {
        return Err(EvalError::InvalidCorpus {
            reason: "chain_count must be > 0".into(),
        });
    }
    let corpus = CorpusBuilder::new()
        .with_block_count(block_count)
        .with_needle_count(needle_count)
        .build()?;
    // 由语料针序构造链：每 CHAIN_LENGTH 针一组
    let ids = corpus.needle_ids_sorted();
    let chains: Vec<Vec<BlockId>> = ids.chunks(CHAIN_LENGTH).map(|c| c.to_vec()).collect();
    Ok(MultihopTask { corpus, chains })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multihop_corpus_build() {
        let task = multihop_corpus(256, 2).expect("build should succeed");
        assert_eq!(task.chains.len(), 2);
        assert_eq!(task.all_needles().len(), 6);
        // 每条链 3 针
        assert!(task.chains.iter().all(|c| c.len() == CHAIN_LENGTH));
        assert!(multihop_corpus(5, 2).is_err());
        assert!(multihop_corpus(64, 0).is_err());
    }

    #[test]
    fn test_chain_success() {
        let task = multihop_corpus(256, 2).unwrap();
        let all: Vec<BlockId> = task.corpus.blocks.iter().map(|b| b.id.clone()).collect();
        // 全中 → 1.0
        assert!((task.success_rate(&all) - 1.0).abs() < 1e-6);
        // 缺链 0 的一个块 → 0.5
        let missing = task.chains[0][1].clone();
        let partial: Vec<BlockId> = all.iter().filter(|id| **id != missing).cloned().collect();
        assert!((task.success_rate(&partial) - 0.5).abs() < 1e-6);
        // 空 → 0.0
        assert_eq!(task.success_rate(&[]), 0.0);
    }

    #[test]
    fn test_chain_needle_naming() {
        let task = multihop_corpus(256, 2).unwrap();
        // 链 0 的针 ID 为 needle-000..002（CorpusBuilder 命名）；链分组按排序针序
        let chain0 = &task.chains[0];
        assert_eq!(chain0.len(), 3);
        // 链内针互异
        assert!(chain0[0] != chain0[1] && chain0[1] != chain0[2]);
    }
}
