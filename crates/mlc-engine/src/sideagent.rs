//! Memory Sideagent — 记忆召回二次验证(polish-v2.7 P4-4)
//!
//! 对应架构层:L2 Memory(mlc-engine 子模块)
//! 对应 ADR:ADR-049 决策 1(memory-sideagent 落点 mlc-engine)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §6.3(jcode 二次验证)
//!
//! # 核心思想(jcode)
//!
//! 向量召回的 Top-K 并非全部可信:陈旧记忆、历史错误模式会污染上下文
//! (对应三重悖论"幽灵记忆"红线,§3.4.5)。Sideagent 在召回后做
//! 四检查项加权二次验证,拦截低分记忆不进入上下文窗口。
//!
//! # 四检查项(方案 §6.3 权重)
//!
//! | 检查 | 权重 | 语义 |
//! |---|---|---|
//! | 语义相关度 | 0.3 | 与当前任务 CLV 的余弦相似度 |
//! | 新鲜度 | 0.2 | 距上次验证的老化衰减(半衰期式) |
//! | 错误模式惩罚 | -0.3 | 与历史错误关联的记忆扣分 |
//! | 成功关联加分 | 0.2 | 与成功任务关联的记忆加分 |
//!
//! 综合分 > 0.6 通过;否则拒绝(附带各检查项明细供审计)。

use nexus_core::CLV;

use crate::memory_graph::{MemoryNode, MemoryNodeType};

/// 验证通过阈值(方案 §6.3)
const VERIFY_PASS_THRESHOLD: f32 = 0.6;

/// 单条记忆的验证明细
#[derive(Debug, Clone)]
pub struct VerifiedMemory {
    /// 记忆节点 ID
    pub node_id: String,
    /// 综合验证分
    pub score: f32,
    /// 检查项明细(名称, 得分)— 供审计与 TUI 展示
    pub checks: Vec<(&'static str, f32)>,
}

/// 二次验证结果 — 通过/拒绝两个分组
#[derive(Debug, Clone, Default)]
pub struct SideagentVerdict {
    /// 通过验证的记忆(可进入上下文窗口)
    pub verified: Vec<VerifiedMemory>,
    /// 被拦截的记忆(附明细供审计)
    pub rejected: Vec<VerifiedMemory>,
}

/// 记忆 Sideagent — 无状态四检查项验证器
#[derive(Debug, Default, Clone, Copy)]
pub struct MemorySideagent;

impl MemorySideagent {
    /// 创建 Sideagent
    pub fn new() -> Self {
        Self
    }

    /// 对召回结果做二次验证
    ///
    /// # 参数
    /// - `recalled`:向量召回的候选记忆集
    /// - `current_task`:当前任务 CLV(相关度基准)
    /// - `freshness`:各记忆的新鲜度评分 [0.0, 1.0](调用方按记忆年龄计算,
    ///   索引与 `recalled` 对齐;缺失按 0.5 中性处理)
    pub fn verify(
        &self,
        recalled: &[MemoryNode],
        current_task: &CLV,
        freshness: &[f32],
    ) -> SideagentVerdict {
        let mut verdict = SideagentVerdict::default();

        for (i, node) in recalled.iter().enumerate() {
            let mut score = 0.0f32;
            let mut checks = Vec::with_capacity(4);

            // 检查 1:语义相关度(权重 0.3)
            let relevance = node.embedding.cosine_similarity(current_task);
            score += relevance * 0.3;
            checks.push(("relevance", relevance));

            // 检查 2:新鲜度(权重 0.2;缺失按中性 0.5)
            let fresh = freshness.get(i).copied().unwrap_or(0.5).clamp(0.0, 1.0);
            score += fresh * 0.2;
            checks.push(("freshness", fresh));

            // 检查 3:错误模式惩罚(-0.3)— 幽灵记忆红线的直接防线
            let is_error = node.node_type == MemoryNodeType::ErrorPattern;
            if is_error {
                score -= 0.3;
            }
            checks.push(("error_free", if is_error { 0.0 } else { 1.0 }));

            // 检查 4:成功关联加分(+0.2)
            if node.success_associated {
                score += 0.2;
            }
            checks.push((
                "success_associated",
                if node.success_associated { 1.0 } else { 0.0 },
            ));

            let entry = VerifiedMemory {
                node_id: node.node_id.clone(),
                score,
                checks,
            };
            if score > VERIFY_PASS_THRESHOLD {
                verdict.verified.push(entry);
            } else {
                verdict.rejected.push(entry);
            }
        }
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_clv(dim: usize) -> CLV {
        let mut v = vec![0.0f32; CLV::DIMENSION];
        v[dim] = 1.0;
        CLV::from_vec(v).expect("512 维合法")
    }

    fn node(id: &str, clv: CLV, node_type: MemoryNodeType, success: bool) -> MemoryNode {
        MemoryNode {
            node_id: id.into(),
            content: String::new(),
            embedding: clv,
            node_type,
            success_associated: success,
        }
    }

    #[test]
    fn test_relevant_fresh_successful_memory_passes() {
        let sideagent = MemorySideagent::new();
        let task = unit_clv(0);
        // 满分画像:相关度 1.0×0.3 + 新鲜 1.0×0.2 + 无错误 + 成功关联 +0.2 = 0.7 > 0.6
        let recalled = vec![node("good", unit_clv(0), MemoryNodeType::Solution, true)];
        let verdict = sideagent.verify(&recalled, &task, &[1.0]);
        assert_eq!(verdict.verified.len(), 1);
        assert!(verdict.rejected.is_empty());
    }

    #[test]
    fn test_error_pattern_memory_rejected() {
        let sideagent = MemorySideagent::new();
        let task = unit_clv(0);
        // 高相关但错误模式:0.3 + 0.2 - 0.3 = 0.2 < 0.6 → 拦截(幽灵记忆防线)
        let recalled = vec![node(
            "bad",
            unit_clv(0),
            MemoryNodeType::ErrorPattern,
            false,
        )];
        let verdict = sideagent.verify(&recalled, &task, &[1.0]);
        assert!(verdict.verified.is_empty());
        assert_eq!(verdict.rejected.len(), 1);
        assert!(verdict.rejected[0]
            .checks
            .iter()
            .any(|(name, v)| *name == "error_free" && *v == 0.0));
    }

    #[test]
    fn test_irrelevant_memory_rejected() {
        let sideagent = MemorySideagent::new();
        let task = unit_clv(0);
        // 语义正交:0.0×0.3 + 1.0×0.2 + 0.2 = 0.4 < 0.6 → 拦截
        let recalled = vec![node("off", unit_clv(1), MemoryNodeType::Solution, true)];
        let verdict = sideagent.verify(&recalled, &task, &[1.0]);
        assert_eq!(verdict.rejected.len(), 1);
    }

    #[test]
    fn test_missing_freshness_defaults_neutral() {
        let sideagent = MemorySideagent::new();
        let task = unit_clv(0);
        let recalled = vec![node("m", unit_clv(0), MemoryNodeType::Solution, true)];
        // freshness 缺失 → 中性 0.5:0.3 + 0.1 + 0.2 = 0.6,不 > 0.6 → 拒绝(边界)
        let verdict = sideagent.verify(&recalled, &task, &[]);
        assert_eq!(verdict.rejected.len(), 1);
    }
}
