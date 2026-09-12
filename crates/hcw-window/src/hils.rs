//! HiLS-Attention — 分层稀疏注意力（设计文档 §7.4）
//!
//! 对应架构层: **L2 Memory**（hcw-window 子模块，用户已确认内嵌落点 D-2）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §7.4
//! 对应论文: HiLS-Attention（腾讯混元 arXiv，分层稀疏注意力长上下文机制）
//! 对应 ADR: ADR-049 决策 1（hils-attention 内嵌 hcw-window，保持 38 crate 基线）
//!
//! # 核心职责
//!
//! 替代/增强 `hcw-window` 的窗口选择器，支持长上下文（64K→512K）：
//! - **chunk-mass surrogate**: 块重要性 = 相关性项（CLV cosine exp）× 熵偏置项
//! - **两级 softmax**: 块间 Top-K 选择 → 块内 token 权重计算
//! - **local window**: 滑动窗口局部注意力（保持局部连贯）
//!
//! # 与 Chimera HCW 的映射
//!
//! HiLS 作为 `WindowSelector`（纯函数 O(1) 复杂度路由）的**互补增强**：
//! - `WindowSelector::select` 决定窗口层级（L0/L1/L2/L3）
//! - `HiLSAttention` 决定选定窗口内的块选择（稀疏化加载）
//! - 本模块提供独立的 [`HiLSWindowSelector`]，不侵入既有 `WindowSelector` 纯函数
//!   与 `HcwWindow` 结构（风险缓解：语义冲突最小化）
//!
//! # 设计约束
//!
//! - **红线 R8（Top-K O(n)）**: 块间 Top-K 用 `select_nth_unstable_by`，禁止 `sort_by`
//! - **f32 红线**: 块重要性/熵为 f32，仅 PartialEq
//! - **确定性**: 无随机源，同输入同输出（可测试）

use nexus_core::CLV;

// ============================================================
// 块（Chunk）
// ============================================================

/// 注意力块 — HiLS 的基本处理单元
///
/// 每块含一个 landmark token 的 CLV 表示（块间粗筛用）与块内统计。
#[derive(Clone, Debug)]
pub struct Chunk {
    /// 块 ID
    pub chunk_id: usize,
    /// landmark token 的 CLV 表示（块间相关性计算）
    pub landmark_key: CLV,
    /// 块内 token 数
    pub token_count: usize,
    /// 预计算的注意力熵（块内分布不确定性，越大越需关注）
    pub entropy: f32,
}

impl Chunk {
    /// 创建块
    pub fn new(chunk_id: usize, landmark_key: CLV, token_count: usize, entropy: f32) -> Self {
        Self {
            chunk_id,
            landmark_key,
            token_count,
            entropy,
        }
    }

    /// 块内注意力权重 — 基于熵的简化分布（均匀 + 熵偏置）
    ///
    /// 规范原型为块内 softmax；Rust 侧用熵偏置的均匀分布近似
    /// （高熵块权重更分散，低熵块权重更集中）。
    pub fn compute_intra_attention(&self, query: &CLV) -> Vec<f32> {
        if self.token_count == 0 {
            return Vec::new();
        }
        // 基础均匀分布
        let base = 1.0 / self.token_count as f32;
        // 熵偏置: 高熵 → 权重更均匀（接近 base）；低熵 → 首 token 权重更高
        let query_relevance = query.cosine_similarity(&self.landmark_key).max(0.0);
        let concentration = (1.0 - self.entropy.clamp(0.0, 1.0)) * query_relevance;
        let mut weights = vec![base; self.token_count];
        if !weights.is_empty() && concentration > 0.0 {
            // 首 token（landmark 位置）获得集中加权
            weights[0] += concentration;
            // 归一化
            let sum: f32 = weights.iter().sum();
            if sum > 0.0 {
                for w in weights.iter_mut() {
                    *w /= sum;
                }
            }
        }
        weights
    }
}

// ============================================================
// 注意力输出
// ============================================================

/// 注意力输出 — 两级 softmax 的选择结果
#[derive(Clone, Debug, Default)]
pub struct AttentionOutput {
    /// 选中的块 ID（Top-K）
    pub selected_chunk_ids: Vec<usize>,
    /// 选中块的重要性权重（与 selected_chunk_ids 对应）
    pub chunk_weights: Vec<f32>,
    /// 局部窗口大小（local window token 数）
    pub local_window_tokens: usize,
}

impl AttentionOutput {
    /// 创建空输出
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加选中块
    pub fn add_chunk(&mut self, chunk_id: usize, weight: f32) {
        self.selected_chunk_ids.push(chunk_id);
        self.chunk_weights.push(weight);
    }

    /// 设置局部窗口 token 数
    pub fn set_local_window(&mut self, tokens: usize) {
        self.local_window_tokens = tokens;
    }
}

// ============================================================
// HiLS 注意力
// ============================================================

/// HiLS-Attention — 分层稀疏注意力核心
#[derive(Clone, Debug)]
pub struct HiLSAttention {
    /// 块大小（tokens/chunk，默认 128）
    pub chunk_size: usize,
    /// 块间 Top-K 选择数
    pub top_k_chunks: usize,
    /// 滑动窗口大小（local window tokens）
    pub sliding_window_size: usize,
    /// 查询打包数（高效 kernel，默认 16）
    pub m_query_pack: usize,
}

impl Default for HiLSAttention {
    fn default() -> Self {
        Self {
            chunk_size: 128,
            top_k_chunks: 8,
            sliding_window_size: 256,
            m_query_pack: 16,
        }
    }
}

impl HiLSAttention {
    /// 创建 HiLS 注意力（自定义参数）
    pub fn new(chunk_size: usize, top_k_chunks: usize, sliding_window_size: usize) -> Self {
        Self {
            chunk_size: chunk_size.max(1),
            top_k_chunks: top_k_chunks.max(1),
            sliding_window_size,
            ..Self::default()
        }
    }

    /// chunk-mass surrogate — 块重要性 = 相关性项 × 熵偏置项
    ///
    /// - 相关性项: `exp(cosine(query, landmark_key))`（越大越相关）
    /// - 熵偏置项: `1 + entropy`（高熵块更需关注）
    pub fn compute_chunk_importance(&self, query: &CLV, chunk: &Chunk) -> f32 {
        let relevance = query.cosine_similarity(&chunk.landmark_key).exp();
        let entropy_bias = 1.0 + chunk.entropy.max(0.0);
        relevance * entropy_bias
    }

    /// 前向 — 两级 softmax（块间 Top-K 选择 + 块内权重 + local window）
    ///
    /// 块间 Top-K 用 `select_nth_unstable_by`（红线 R8，O(n)）。
    pub fn forward(&self, query: &CLV, chunks: &[Chunk]) -> AttentionOutput {
        if chunks.is_empty() {
            return AttentionOutput::new();
        }
        // 块间打分
        let mut chunk_scores: Vec<(usize, f32)> = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (i, self.compute_chunk_importance(query, c)))
            .collect();

        // Top-K 选择（O(n)，红线 R8）
        let k = self.top_k_chunks.min(chunk_scores.len());
        if k < chunk_scores.len() {
            chunk_scores.select_nth_unstable_by(k, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
            chunk_scores.truncate(k);
        }
        // 选中块按分数降序排列（k 通常很小，sort 可接受）
        chunk_scores
            .sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut output = AttentionOutput::new();
        for (idx, score) in chunk_scores {
            // 归一化权重（softmax 近似: score / sum，此处用相对分数）
            output.add_chunk(chunks[idx].chunk_id, score);
        }
        // local window（滑动窗口局部注意力）
        output.set_local_window(self.sliding_window_size);
        output
    }

    /// 批量前向 — 打包 M 个查询（高效 kernel，union chunks 复用）
    pub fn forward_batched(&self, queries: &[CLV], chunks: &[Chunk]) -> Vec<AttentionOutput> {
        let mut outputs = Vec::with_capacity(queries.len());
        for query_batch in queries.chunks(self.m_query_pack.max(1)) {
            // 批次内各查询独立前向（union chunks 优化留待 kernel 层）
            for query in query_batch {
                outputs.push(self.forward(query, chunks));
            }
        }
        outputs
    }
}

// ============================================================
// HiLS 窗口选择器（HCW 集成接口）
// ============================================================

/// HiLS 窗口选择器 — 封装 HiLSAttention 的块选择能力（HCW 集成）
///
/// 作为 `WindowSelector`（复杂度→层级路由）的互补：
/// `WindowSelector` 决定窗口层级，本选择器决定层级内的稀疏块加载。
/// 独立类型，不侵入既有 `WindowSelector` 纯函数与 `HcwWindow` 结构。
#[derive(Clone, Debug)]
pub struct HiLSWindowSelector {
    /// 内嵌 HiLS 注意力
    pub hils: HiLSAttention,
}

impl HiLSWindowSelector {
    /// 创建 HiLS 窗口选择器
    pub fn new(hils: HiLSAttention) -> Self {
        Self { hils }
    }

    /// 选择 Top-K 相关块 — 给定查询与候选块，返回选中块 ID
    pub fn select_top_chunks(&self, query: &CLV, chunks: &[Chunk]) -> Vec<usize> {
        let output = self.hils.forward(query, chunks);
        output.selected_chunk_ids
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 单位 CLV: 指定维度置 1
    fn unit_clv(dim: usize) -> CLV {
        // 算法体已收敛到 L1 `nexus_core::CLV::basis`(单一权威构造器),
        // 此处仅保留本地签名,避免改动本文件内数十处调用点。
        // basis 越界返回 None;夹具若下标非法则该测试无效,直接 expect 暴露。
        CLV::basis(dim).expect("测试夹具:下标须在 CLV::DIMENSION 内")
    }

    fn chunk(id: usize, dim: usize, entropy: f32) -> Chunk {
        Chunk::new(id, unit_clv(dim), 64, entropy)
    }

    #[test]
    fn chunk_importance_relevance_times_entropy() {
        let hils = HiLSAttention::default();
        let query = unit_clv(0);
        let relevant = chunk(1, 0, 0.0); // 同向，相关性 exp(1)
        let orthogonal = chunk(2, 1, 0.0); // 正交，相关性 exp(0)=1
        let imp_relevant = hils.compute_chunk_importance(&query, &relevant);
        let imp_orthogonal = hils.compute_chunk_importance(&query, &orthogonal);
        assert!(
            imp_relevant > imp_orthogonal,
            "相关块重要性应更高（{} vs {})",
            imp_relevant,
            imp_orthogonal
        );
    }

    #[test]
    fn chunk_importance_entropy_bias() {
        let hils = HiLSAttention::default();
        let query = unit_clv(0);
        let low_entropy = chunk(1, 0, 0.0);
        let high_entropy = chunk(2, 0, 1.0); // 同相关性，高熵
        let imp_low = hils.compute_chunk_importance(&query, &low_entropy);
        let imp_high = hils.compute_chunk_importance(&query, &high_entropy);
        assert!(imp_high > imp_low, "高熵块应获得更高偏置（熵 bias）");
    }

    #[test]
    fn forward_selects_top_k_chunks() {
        let hils = HiLSAttention::new(128, 2, 256);
        let query = unit_clv(0);
        // 3 块: chunk 0 最相关（dim 0），chunk 1/2 正交
        let chunks = vec![chunk(0, 0, 0.0), chunk(1, 1, 0.0), chunk(2, 2, 0.0)];
        let output = hils.forward(&query, &chunks);
        assert_eq!(output.selected_chunk_ids.len(), 2, "应选 Top-2 块");
        assert_eq!(output.selected_chunk_ids[0], 0, "最相关块应排第一");
        assert_eq!(output.local_window_tokens, 256);
    }

    #[test]
    fn forward_empty_chunks_returns_empty() {
        let hils = HiLSAttention::default();
        let query = unit_clv(0);
        let output = hils.forward(&query, &[]);
        assert!(output.selected_chunk_ids.is_empty());
    }

    #[test]
    fn forward_top_k_exceeds_chunk_count() {
        let hils = HiLSAttention::new(128, 10, 256); // top_k=10 > 3 块
        let query = unit_clv(0);
        let chunks = vec![chunk(0, 0, 0.0), chunk(1, 1, 0.0), chunk(2, 2, 0.0)];
        let output = hils.forward(&query, &chunks);
        assert_eq!(output.selected_chunk_ids.len(), 3, "top_k 超过块数时全选");
    }

    #[test]
    fn forward_batched_processes_all_queries() {
        let hils = HiLSAttention::new(128, 2, 256);
        let queries = vec![unit_clv(0), unit_clv(1), unit_clv(2)];
        let chunks = vec![chunk(0, 0, 0.0), chunk(1, 1, 0.0), chunk(2, 2, 0.0)];
        let outputs = hils.forward_batched(&queries, &chunks);
        assert_eq!(outputs.len(), 3, "应为每个查询产生输出");
        // 各查询应选中各自最相关的块
        assert_eq!(outputs[0].selected_chunk_ids[0], 0);
        assert_eq!(outputs[1].selected_chunk_ids[0], 1);
        assert_eq!(outputs[2].selected_chunk_ids[0], 2);
    }

    #[test]
    fn intra_attention_weights_normalized() {
        let c = Chunk::new(1, unit_clv(0), 4, 0.5);
        let query = unit_clv(0);
        let weights = c.compute_intra_attention(&query);
        assert_eq!(weights.len(), 4);
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "块内权重应归一化（实际 {})", sum);
    }

    #[test]
    fn intra_attention_empty_chunk() {
        let c = Chunk::new(1, unit_clv(0), 0, 0.5);
        let query = unit_clv(0);
        assert!(c.compute_intra_attention(&query).is_empty());
    }

    #[test]
    fn hils_window_selector_top_chunks() {
        let selector = HiLSWindowSelector::new(HiLSAttention::new(128, 2, 256));
        let query = unit_clv(0);
        let chunks = vec![chunk(0, 0, 0.0), chunk(1, 1, 0.0), chunk(2, 2, 0.0)];
        let top = selector.select_top_chunks(&query, &chunks);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0], 0, "HCW 集成选择器应选最相关块");
    }

    #[test]
    fn deterministic_forward() {
        // 确定性: 同输入同输出（无随机源）
        let hils = HiLSAttention::new(128, 2, 256);
        let query = unit_clv(0);
        let chunks = vec![chunk(0, 0, 0.3), chunk(1, 1, 0.5), chunk(2, 2, 0.1)];
        let out1 = hils.forward(&query, &chunks);
        let out2 = hils.forward(&query, &chunks);
        assert_eq!(out1.selected_chunk_ids, out2.selected_chunk_ids);
        assert_eq!(out1.chunk_weights, out2.chunk_weights);
    }
}
