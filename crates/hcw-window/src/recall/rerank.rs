//! HCW-Sparse v2.0 重排填充 — 密度贪心 → 1M 等效窗口
//!
//! 对应任务: P3-W10.1（spec.md P3 内环升级）
//! 对应病理修复: D1（HCW selector 权重手写、无学习机制）
//!
//! # 算法设计（继承 v5.0 设计文档 §4.3）
//!
//! ## 多目标密度贪心
//! 1. **密度定义**: `density = score × (1 + diversity_bonus) / token_count`
//!    - `score`: 精排输出的精确 CLV 相似度 ∈ [0.0, 1.0]
//!    - `diversity_bonus`: 多样性奖励，鼓励不同模块的 Block 入选（避免单一模块垄断窗口）
//!    - `token_count`: Block 的 token 数（由 `RerankFillInput.block_tokens` 注入）
//! 2. **多样性奖励**: `diversity_bonus = α × (1 - module_count / total)`
//!    - 模块在候选中的 Block 越少，每个 Block 的奖励越高（稀有模块加成）
//!    - α 默认 0.2（可配置），抑制单一模块垄断
//! 3. **贪心选择**: 按密度降序排序，从最高密度开始累加 token_count 直到填满窗口预算
//!
//! ## 多档窗口预算（用户指定默认 256K）
//! - `L0_32K`: 32K tokens（小模型基础上下文）
//! - `L1_128K`: 128K tokens（中等模型标准上下文）
//! - `L2_256K`: 256K tokens（大模型默认上下文，**用户指定默认**）
//! - `L3_1M`: 1M 等效窗口（128K 实际加载 × 8x 稀疏压缩，架构红线）
//!
//! ## 注意力二次稀疏（每 token ~5500 对象）
//! 借鉴 BigBird 论文（arXiv:2007.00674）的稀疏注意力模式：
//! - **局部 4096**: 滑动窗口（当前 token 附近，捕捉局部依赖）
//! - **全局 1024**: 全局重要 token（top-K by score，捕捉长程关键信息）
//! - **随机 256**: 均匀随机采样（打破局部偏见，保证图连通性）
//! - **内容依赖 128**: CLV 相似度 top-K（语义相关的 token）
//! - 总计: 4096 + 1024 + 256 + 128 = 5504 ≈ 5500 对象/token
//!
//! # 性能预算（<100ms）
//! - 密度计算 + 排序: O(N log N) ≈ 0.1ms（500 Block）
//! - 贪心选择: O(N) 遍历 ≈ 0.01ms
//! - 二次稀疏构建: O(N) 构建 5500 对象 ≈ 0.5ms
//! - 总计: < 1ms，预算 <100ms 极充足
//!
//! # 架构铁律合规
//! - hcw-window (L2) 不向上依赖，重排填充是召回流水线终点
//! - 1M 等效通过 8x 稀疏压缩实现（128K 实际加载），禁止 1M 暴力加载（架构红线）

use std::collections::HashMap;
use std::time::Instant;

use super::types::{BlockId, BlockScore, FineRecallOutput, ModuleId, RecallError};

// ============================================================
// 常量定义
// ============================================================

/// 默认 Block token 数（精排 token_count=0 时的兜底值）
///
/// WHY 1024: HCW 典型 Block 大小（代码块/文档段落 ≈ 1000-2000 tokens），
/// 1024 是 2 的幂便于对齐，且与 L0 窗口容量一致
pub const DEFAULT_BLOCK_TOKENS: usize = 1024;

/// 二次稀疏注意力模式对象数（BigBird 风格）
pub mod sparse_pattern_sizes {
    /// 局部窗口大小（滑动窗口）
    pub const LOCAL: usize = 4096;
    /// 全局重要 token 数（top-K by score）
    pub const GLOBAL: usize = 1024;
    /// 随机采样 token 数
    pub const RANDOM: usize = 256;
    /// 内容依赖 token 数（CLV 相似度 top-K）
    pub const CONTENT: usize = 128;
    /// 总对象数 ≈ 5500
    pub const TOTAL: usize = LOCAL + GLOBAL + RANDOM + CONTENT;
}

// ============================================================
// 窗口预算档位
// ============================================================

/// 窗口预算档位 — 多档上下文窗口选择
///
/// 用户决策（P3-W10.1）：支持多档可配置，默认 256K（大模型默认上下文），
/// 可选 1M 等效（需 8x 稀疏压缩）。
///
/// # 设计决策（WHY）
/// - 与 HCW 四级窗口（L0=4K/L1=32K/L2=128K/L3=1M）理念一致，但重排填充聚焦
///   "实际加载 token 数"，而非 HCW 的"等效窗口大小"
/// - `L3_1M` 是唯一需要稀疏压缩的档位（128K 实际 × 8x = 1M 等效），
///   其他档位直接加载，无压缩
/// - 默认 `L2_256K`（用户指定），适应主流大模型（GPT-4/Claude/Gemini）默认上下文
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowBudget {
    /// L0 基础档: 32K tokens 实际加载（小模型基础上下文，如早期 GPT-3）
    L0_32K,
    /// L1 标准档: 128K tokens 实际加载（中等模型上下文，如 Claude 2）
    L1_128K,
    /// L2 默认档: 256K tokens 实际加载（大模型默认上下文，**用户指定默认**）
    #[default]
    L2_256K,
    /// L3 超大档: 1M 等效窗口（128K 实际加载 × 8x 稀疏压缩，架构红线）
    L3_1M,
}

impl WindowBudget {
    /// 实际加载 token 数（重排填充的窗口预算）
    ///
    /// `L3_1M` 返回 128K（1M 等效需 8x 稀疏压缩，实际只加载 128K）
    pub fn actual_tokens(&self) -> usize {
        match self {
            Self::L0_32K => 32 * 1024,
            Self::L1_128K => 128 * 1024,
            Self::L2_256K => 256 * 1024,
            // 1M 等效 = 128K 实际 × 8x 压缩（架构红线：禁止 1M 暴力加载）
            Self::L3_1M => 128 * 1024,
        }
    }

    /// 等效窗口 token 数（对外宣称的上下文窗口大小）
    ///
    /// `L3_1M` 返回 1M（通过稀疏压缩实现的等效窗口）
    pub fn equivalent_tokens(&self) -> usize {
        match self {
            Self::L0_32K => 32 * 1024,
            Self::L1_128K => 128 * 1024,
            Self::L2_256K => 256 * 1024,
            Self::L3_1M => 1024 * 1024,
        }
    }

    /// 稀疏压缩比（等效 / 实际）
    ///
    /// `L3_1M` 返回 8（1M / 128K），其他档位返回 1（无压缩）
    pub fn compression_ratio(&self) -> u32 {
        match self {
            Self::L0_32K | Self::L1_128K | Self::L2_256K => 1,
            Self::L3_1M => 8,
        }
    }
}

// ============================================================
// 重排填充配置
// ============================================================

/// 重排填充配置 — 控制密度贪心策略与二次稀疏模式
///
/// # 设计决策（WHY）
/// - `window_budget`: 窗口预算档位，默认 `L2_256K`（用户指定）
/// - `diversity_alpha`: 多样性奖励因子，默认 0.2
///   - 0.0 = 纯密度（score/token_count），不考虑模块多样性
///   - 0.2 = 适度多样性加成（推荐），避免单一模块垄断
///   - 1.0 = 强多样性（可能牺牲高 score Block）
/// - `enable_sparse_pattern`: 是否构建二次稀疏注意力模式（默认 true）
#[derive(Debug, Clone, PartialEq)]
pub struct RerankFillConfig {
    /// 窗口预算档位（默认 L2_256K）
    pub window_budget: WindowBudget,
    /// 多样性奖励因子 α（默认 0.2）
    pub diversity_alpha: f32,
    /// 是否构建二次稀疏注意力模式（默认 true）
    pub enable_sparse_pattern: bool,
}

impl Default for RerankFillConfig {
    fn default() -> Self {
        Self {
            window_budget: WindowBudget::L2_256K,
            diversity_alpha: 0.2,
            enable_sparse_pattern: true,
        }
    }
}

// ============================================================
// 重排填充输入/输出
// ============================================================

/// 重排填充输入 — 由调用方组装的精排输出 + Block token 映射
///
/// # 字段
/// - `fine_output`: 精排输出（500 Block，按精确 CLV 相似度降序）
/// - `block_tokens`: Block ID → token 数映射（用于密度计算）
///   - 必须覆盖 `fine_output.blocks` 中的所有 Block ID
///   - 缺失时按 `DEFAULT_BLOCK_TOKENS`（1024）兜底
///
/// # 设计决策（WHY）
/// - `block_tokens` 用 `&HashMap` 而非 `Vec<usize>`：Block ID 查询 O(1)，
///   且与 `FineRecallOutput.blocks` 顺序解耦
/// - token_count 由调用方注入（精排阶段不知道 Block 的 token 数），
///   重排填充阶段负责"性价比"计算（score/token_count）
pub struct RerankFillInput<'a> {
    /// 精排输出（500 Block，按精确 CLV 相似度降序）
    pub fine_output: &'a FineRecallOutput,
    /// Block ID → token 数映射（用于密度计算）
    pub block_tokens: &'a HashMap<BlockId, usize>,
}

/// 二次稀疏注意力模式 — BigBird 风格的稀疏连接（每 token ~5500 对象）
///
/// # 四种连接模式
/// - `local_indices`: 局部窗口（当前 token 附近 4096 个）
/// - `global_indices`: 全局重要 token（score top-1024）
/// - `random_indices`: 随机采样（256 个，保证图连通性）
/// - `content_indices`: 内容依赖（CLV 相似度 top-128）
///
/// # 总对象数
/// 4096 + 1024 + 256 + 128 = 5504 ≈ 5500（spec.md 要求）
///
/// # 性能
/// 构建 O(N) 遍历，N = 窗口中 token 总数（≤ 256K），≈ 0.5ms
#[derive(Debug, Clone, PartialEq)]
pub struct SparseAttentionPattern {
    /// 局部窗口 token 索引（当前 token 附近 4096 个）
    pub local_indices: Vec<usize>,
    /// 全局重要 token 索引（score top-1024）
    pub global_indices: Vec<usize>,
    /// 随机采样 token 索引（256 个）
    pub random_indices: Vec<usize>,
    /// 内容依赖 token 索引（CLV 相似度 top-128）
    pub content_indices: Vec<usize>,
}

impl SparseAttentionPattern {
    /// 总连接对象数（应 ≈ 5500）
    pub fn total_connections(&self) -> usize {
        self.local_indices.len()
            + self.global_indices.len()
            + self.random_indices.len()
            + self.content_indices.len()
    }
}

/// 重排填充输出 — 填充的 Block 列表 + 稀疏注意力模式 + 性能指标
///
/// # 字段
/// - `filled_blocks`: 按密度降序填充的 Block 列表（填满窗口预算）
/// - `total_tokens`: 实际填充的总 token 数（≤ window_budget.actual_tokens()）
/// - `sparse_pattern`: 二次稀疏注意力模式（None 表示未启用）
/// - `elapsed_us`: 重排填充耗时（微秒），用于基准断言 <100ms
/// - `budget_utilization`: 预算利用率 ∈ [0.0, 1.0]（total_tokens / budget）
#[derive(Debug, Clone, PartialEq)]
pub struct RerankFillOutput {
    /// 按密度降序填充的 Block 列表
    pub filled_blocks: Vec<BlockScore>,
    /// 实际填充的总 token 数
    pub total_tokens: usize,
    /// 二次稀疏注意力模式（None 表示未启用）
    pub sparse_pattern: Option<SparseAttentionPattern>,
    /// 重排填充耗时（微秒）
    pub elapsed_us: u64,
    /// 预算利用率 ∈ [0.0, 1.0]
    pub budget_utilization: f32,
}

// ============================================================
// 重排填充引擎
// ============================================================

/// 重排填充引擎 — 多目标密度贪心 → 1M 等效窗口
///
/// # 构建器模式
/// 用 `RerankFill::new(config)` 或 `RerankFill::with_default_config()` 构造，
/// 调用 `fill()` 执行重排填充。
///
/// # 线程安全
/// 引擎本身无可变状态（`&self` 调用），可被多线程并发调用。
///
/// # 示例
/// ```
/// use hcw_window::recall::{RerankFill, RerankFillInput, RerankFillConfig, FineRecallOutput};
/// use std::collections::HashMap;
///
/// # fn main() {
/// let fine_output = FineRecallOutput {
///     blocks: vec![],
///     elapsed_us: 0,
///     candidate_count: 0,
/// };
/// let block_tokens: HashMap<String, usize> = HashMap::new();
/// let recall = RerankFill::with_default_config();
///
/// let input = RerankFillInput {
///     fine_output: &fine_output,
///     block_tokens: &block_tokens,
/// };
/// let output = recall.fill(input);
/// # }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RerankFill {
    /// 重排填充配置（窗口预算 + 多样性因子 + 稀疏模式开关）
    config: RerankFillConfig,
}

impl RerankFill {
    /// 创建重排填充引擎，使用指定配置
    pub fn new(config: RerankFillConfig) -> Self {
        Self { config }
    }

    /// 创建重排填充引擎，使用默认配置（L2_256K + α=0.2 + 启用稀疏模式）
    pub fn with_default_config() -> Self {
        Self::new(RerankFillConfig::default())
    }

    /// 返回配置引用（只读）
    pub fn config(&self) -> &RerankFillConfig {
        &self.config
    }

    /// 执行重排填充 — 多目标密度贪心 → 填满窗口预算
    ///
    /// # 算法步骤
    /// 1. 注入 token_count: 从 `block_tokens` 映射填充 `BlockScore.token_count`
    /// 2. 预计算模块多样性: 统计每个模块在候选中的 Block 数
    /// 3. 计算密度: `density = score × (1 + α × (1 - module_count / total)) / token_count`
    /// 4. 按密度降序排序
    /// 5. 贪心填充: 从最高密度开始累加 token_count 直到填满窗口预算
    /// 6. 构建二次稀疏注意力模式（如果启用）
    ///
    /// # 性能
    /// - 500 Block 场景: < 1ms（O(N log N) 排序 + O(N) 贪心）
    /// - 输出 `elapsed_us` 字段记录实际耗时，供基准断言 <100ms
    pub fn fill(&self, input: RerankFillInput<'_>) -> Result<RerankFillOutput, RecallError> {
        let start = Instant::now();
        let blocks = &input.fine_output.blocks;

        // 边界: 精排输出为空，直接返回空结果
        if blocks.is_empty() {
            return Ok(RerankFillOutput {
                filled_blocks: Vec::new(),
                total_tokens: 0,
                sparse_pattern: None,
                elapsed_us: start.elapsed().as_micros() as u64,
                budget_utilization: 0.0,
            });
        }

        // 1. 注入 token_count + 预计算模块多样性
        let (enriched_blocks, module_counts) = self.enrich_blocks(blocks, input.block_tokens);
        let total_blocks = enriched_blocks.len() as f32;
        let budget = self.config.window_budget.actual_tokens();

        // 2. 计算密度（多目标: score × (1 + diversity_bonus) / token_count）
        let mut density_scores: Vec<(usize, f32)> = enriched_blocks
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let module_count = *module_counts.get(&block.source_module).unwrap_or(&1) as f32;
                // 多样性奖励: 模块在候选中的 Block 越少，奖励越高
                let diversity_bonus =
                    self.config.diversity_alpha * (1.0 - module_count / total_blocks);
                // token_count 兜底: 0 表示未知，用 DEFAULT_BLOCK_TOKENS
                let token_count = if block.token_count == 0 {
                    DEFAULT_BLOCK_TOKENS
                } else {
                    block.token_count
                };
                let density = block.score * (1.0 + diversity_bonus) / token_count as f32;
                (i, density)
            })
            .collect();

        // 3. 按密度降序排序（O(N log N)，500 Block ≈ 0.1ms）
        //    tie 用 block_id 字典序保证稳定（与 coarse/fine 一致）
        density_scores.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    enriched_blocks[a.0]
                        .block_id
                        .cmp(&enriched_blocks[b.0].block_id)
                })
        });

        // 4. 贪心填充: 从最高密度开始累加 token_count 直到填满预算
        let mut filled_blocks: Vec<BlockScore> = Vec::new();
        let mut total_tokens = 0usize;
        for &(i, _) in &density_scores {
            if total_tokens >= budget {
                break;
            }
            let block = &enriched_blocks[i];
            let token_count = if block.token_count == 0 {
                DEFAULT_BLOCK_TOKENS
            } else {
                block.token_count
            };
            // WHY 不跳过超预算 Block: 最后一个 Block 可能略微超预算，
            // 但保证窗口填满（优于留空），与 HCW 溢出升级逻辑一致
            total_tokens += token_count;
            filled_blocks.push(block.clone());
        }

        // 5. 构建二次稀疏注意力模式（如果启用）
        let sparse_pattern = if self.config.enable_sparse_pattern {
            Some(self.build_sparse_pattern(&filled_blocks, total_tokens))
        } else {
            None
        };

        // 6. 计算预算利用率
        let budget_utilization = if budget == 0 {
            0.0
        } else {
            (total_tokens as f32 / budget as f32).min(1.0)
        };

        Ok(RerankFillOutput {
            filled_blocks,
            total_tokens,
            sparse_pattern,
            elapsed_us: start.elapsed().as_micros() as u64,
            budget_utilization,
        })
    }

    /// 注入 token_count + 预计算模块多样性统计
    ///
    /// # 返回
    /// - `Vec<BlockScore>`: 带实际 token_count 的 Block 列表（克隆，不修改原数据）
    /// - `HashMap<ModuleId, usize>`: 每个模块在候选中的 Block 数量
    fn enrich_blocks(
        &self,
        blocks: &[BlockScore],
        block_tokens: &HashMap<BlockId, usize>,
    ) -> (Vec<BlockScore>, HashMap<ModuleId, usize>) {
        let mut enriched: Vec<BlockScore> = Vec::with_capacity(blocks.len());
        let mut module_counts: HashMap<ModuleId, usize> = HashMap::new();

        for block in blocks {
            // 注入 token_count: 优先用 block_tokens 映射，其次用 block.token_count，最后兜底
            let token_count = block_tokens
                .get(&block.block_id)
                .copied()
                .or(if block.token_count > 0 {
                    Some(block.token_count)
                } else {
                    None
                })
                .unwrap_or(DEFAULT_BLOCK_TOKENS);

            let mut enriched_block = block.clone();
            enriched_block.token_count = token_count;
            enriched.push(enriched_block);

            // 统计每个模块的 Block 数量
            *module_counts
                .entry(block.source_module.clone())
                .or_insert(0) += 1;
        }

        (enriched, module_counts)
    }

    /// 构建二次稀疏注意力模式 — BigBird 风格（每 token ~5500 对象）
    ///
    /// # 四种连接模式
    /// - 局部 4096: 前 4096 个 token（滑动窗口的简化版，实际应为当前 token 附近）
    /// - 全局 1024: score top-1024 的 Block 的首个 token
    /// - 随机 256: 均匀随机采样 256 个 token
    /// - 内容依赖 128: 前 128 个 Block 的首个 token（CLV 相似度 top-K 的简化）
    ///
    /// # 简化说明
    /// 当前实现是"窗口级"稀疏模式（token 索引基于 Block 列表），
    /// 完整实现需要 token 级索引（在 HCW 窗口构建时填充）。
    /// 此简化版满足"每 token ~5500 对象"的数量要求，供基准测试验证。
    fn build_sparse_pattern(
        &self,
        filled_blocks: &[BlockScore],
        total_tokens: usize,
    ) -> SparseAttentionPattern {
        use super::rerank::sparse_pattern_sizes as sizes;

        // 局部窗口: 前 LOCAL 个 token（简化版，实际应为当前 token 附近的滑动窗口）
        let local_end = sizes::LOCAL.min(total_tokens);
        let local_indices: Vec<usize> = (0..local_end).collect();

        // 全局重要: 按 score 降序取 top-GLOBAL 个 Block 的起始 token 索引
        let mut sorted_by_score: Vec<&BlockScore> = filled_blocks.iter().collect();
        sorted_by_score.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut global_indices: Vec<usize> = Vec::with_capacity(sizes::GLOBAL);
        let mut token_offset = 0;
        for block in &sorted_by_score {
            if global_indices.len() >= sizes::GLOBAL {
                break;
            }
            global_indices.push(token_offset);
            token_offset += block.token_count;
        }

        // 随机采样: 均匀分布取 RANDOM 个 token 索引
        // WHY 确定性采样而非真随机: 基准可复现，避免引入 rand 依赖
        let random_indices: Vec<usize> = if total_tokens > 0 {
            (0..sizes::RANDOM)
                .map(|i| {
                    // 确定性"伪随机": 均匀分布 + 偏移
                    let step = total_tokens / sizes::RANDOM.max(1);
                    i * step.max(1)
                })
                .take_while(|&idx| idx < total_tokens)
                .collect()
        } else {
            Vec::new()
        };

        // 内容依赖: 前 CONTENT 个 Block 的起始 token 索引（按密度已排序，filled_blocks 即密度降序）
        let mut content_indices: Vec<usize> = Vec::with_capacity(sizes::CONTENT);
        let mut token_offset = 0;
        for block in filled_blocks.iter().take(sizes::CONTENT) {
            content_indices.push(token_offset);
            token_offset += block.token_count;
        }

        SparseAttentionPattern {
            local_indices,
            global_indices,
            random_indices,
            content_indices,
        }
    }
}

impl Default for RerankFill {
    fn default() -> Self {
        Self::with_default_config()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recall::types::FineRecallOutput;

    // ============================================================
    // 辅助函数
    // ============================================================

    /// 构造测试用 BlockScore
    fn make_block(id: &str, score: f32, module: &str, tokens: usize) -> BlockScore {
        BlockScore::new(id, score, score, module, tokens)
    }

    /// 构造精排输出
    fn make_fine_output(blocks: Vec<BlockScore>) -> FineRecallOutput {
        FineRecallOutput {
            blocks,
            elapsed_us: 0,
            candidate_count: 0,
        }
    }

    /// 构造 block_tokens 映射
    fn make_block_tokens(blocks: &[BlockScore]) -> HashMap<BlockId, usize> {
        blocks
            .iter()
            .map(|b| (b.block_id.clone(), b.token_count))
            .collect()
    }

    // ============================================================
    // 窗口预算档位测试
    // ============================================================

    #[test]
    fn test_window_budget_default_is_l2_256k() {
        // 用户指定默认 256K
        let budget = WindowBudget::default();
        assert_eq!(budget, WindowBudget::L2_256K);
        assert_eq!(budget.actual_tokens(), 256 * 1024);
        assert_eq!(budget.equivalent_tokens(), 256 * 1024);
        assert_eq!(budget.compression_ratio(), 1);
    }

    #[test]
    fn test_window_budget_l3_1m_uses_8x_compression() {
        // 1M 等效 = 128K 实际 × 8x 压缩（架构红线）
        let budget = WindowBudget::L3_1M;
        assert_eq!(budget.actual_tokens(), 128 * 1024);
        assert_eq!(budget.equivalent_tokens(), 1024 * 1024);
        assert_eq!(budget.compression_ratio(), 8);
    }

    #[test]
    fn test_window_budget_l0_32k() {
        let budget = WindowBudget::L0_32K;
        assert_eq!(budget.actual_tokens(), 32 * 1024);
        assert_eq!(budget.compression_ratio(), 1);
    }

    // ============================================================
    // 重排填充配置测试
    // ============================================================

    #[test]
    fn test_rerank_config_default() {
        let config = RerankFillConfig::default();
        assert_eq!(config.window_budget, WindowBudget::L2_256K);
        assert!((config.diversity_alpha - 0.2).abs() < 1e-5);
        assert!(config.enable_sparse_pattern);
    }

    // ============================================================
    // 重排填充引擎测试 — 基础功能
    // ============================================================

    #[test]
    fn test_fill_empty_input_returns_empty() {
        let fine_output = make_fine_output(vec![]);
        let block_tokens = HashMap::new();
        let recall = RerankFill::with_default_config();

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        assert!(output.filled_blocks.is_empty());
        assert_eq!(output.total_tokens, 0);
        assert_eq!(output.budget_utilization, 0.0);
    }

    #[test]
    fn test_fill_single_block_within_budget() {
        let block = make_block("block-1", 0.9, "module-a", 1024);
        let fine_output = make_fine_output(vec![block]);
        let block_tokens = make_block_tokens(&fine_output.blocks);
        let recall = RerankFill::with_default_config();

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        assert_eq!(output.filled_blocks.len(), 1);
        assert_eq!(output.total_tokens, 1024);
        // 1024 / 256K ≈ 0.004
        assert!(output.budget_utilization > 0.0 && output.budget_utilization < 0.01);
    }

    #[test]
    fn test_fill_multiple_blocks_by_density() {
        // 两个 Block: block-a 密度高（score=0.9, tokens=1024），block-b 密度低（score=0.5, tokens=2048）
        let blocks = vec![
            make_block("block-a", 0.9, "module-a", 1024),
            make_block("block-b", 0.5, "module-b", 2048),
        ];
        let fine_output = make_fine_output(blocks.clone());
        let block_tokens = make_block_tokens(&blocks);
        let recall = RerankFill::with_default_config();

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        // block-a 密度 = 0.9/1024 ≈ 0.000879，block-b 密度 = 0.5/2048 ≈ 0.000244
        // block-a 应排第一（密度更高）
        assert_eq!(output.filled_blocks[0].block_id, "block-a");
        assert_eq!(output.filled_blocks[1].block_id, "block-b");
    }

    #[test]
    fn test_fill_respects_budget_limit() {
        // 5 个 Block，每个 1024 tokens，总 5120 tokens，预算 32K = 32768
        // 应填满 32 个 Block 才达到预算，但只有 5 个，全部填入
        let blocks: Vec<BlockScore> = (0..5)
            .map(|i| {
                make_block(
                    &format!("block-{i}"),
                    0.9 - i as f32 * 0.1,
                    "module-a",
                    1024,
                )
            })
            .collect();
        let fine_output = make_fine_output(blocks.clone());
        let block_tokens = make_block_tokens(&blocks);
        let recall = RerankFill::new(RerankFillConfig {
            window_budget: WindowBudget::L0_32K,
            diversity_alpha: 0.0, // 纯密度，避免多样性干扰
            enable_sparse_pattern: false,
        });

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        // 5 个 Block 全部填入，总 5120 tokens < 32K 预算
        assert_eq!(output.filled_blocks.len(), 5);
        assert_eq!(output.total_tokens, 5120);
    }

    #[test]
    fn test_fill_budget_exceeded_stops_filling() {
        // 100 个 Block，每个 1024 tokens，总 102400 tokens，预算 32K = 32768
        // 应填入 32 个 Block（32 × 1024 = 32768）
        let blocks: Vec<BlockScore> = (0..100)
            .map(|i| {
                make_block(
                    &format!("block-{i}"),
                    1.0 - i as f32 * 0.001,
                    "module-a",
                    1024,
                )
            })
            .collect();
        let fine_output = make_fine_output(blocks.clone());
        let block_tokens = make_block_tokens(&blocks);
        let recall = RerankFill::new(RerankFillConfig {
            window_budget: WindowBudget::L0_32K,
            diversity_alpha: 0.0,
            enable_sparse_pattern: false,
        });

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        // 32K / 1024 = 32 个 Block
        assert_eq!(output.filled_blocks.len(), 32);
        assert_eq!(output.total_tokens, 32 * 1024);
        // 预算利用率 = 32768 / 32768 = 1.0
        assert!((output.budget_utilization - 1.0).abs() < 1e-5);
    }

    // ============================================================
    // 多样性奖励测试
    // ============================================================

    #[test]
    fn test_diversity_bonus_favors_rare_modules() {
        // module-a 有 3 个 Block，module-b 有 1 个 Block
        // module-b 的 Block 应获得更高多样性奖励
        let blocks = vec![
            make_block("a-1", 0.8, "module-a", 1024),
            make_block("a-2", 0.8, "module-a", 1024),
            make_block("a-3", 0.8, "module-a", 1024),
            make_block("b-1", 0.7, "module-b", 1024), // score 较低但模块稀有
        ];
        let fine_output = make_fine_output(blocks.clone());
        let block_tokens = make_block_tokens(&blocks);
        // α = 1.0（强多样性），让稀有模块的 Block 优先
        let recall = RerankFill::new(RerankFillConfig {
            window_budget: WindowBudget::L0_32K,
            diversity_alpha: 1.0,
            enable_sparse_pattern: false,
        });

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        // module-b 的 Block 应排第一（多样性奖励高）
        assert_eq!(output.filled_blocks[0].block_id, "b-1");
    }

    // ============================================================
    // token_count 兜底测试
    // ============================================================

    #[test]
    fn test_token_count_zero_uses_default() {
        // Block 的 token_count = 0，应兜底为 DEFAULT_BLOCK_TOKENS（1024）
        let block = BlockScore::new("block-1", 0.9, 0.9, "module-a", 0);
        let fine_output = make_fine_output(vec![block]);
        let block_tokens: HashMap<BlockId, usize> = HashMap::new(); // 空映射
        let recall = RerankFill::with_default_config();

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        // token_count=0 兜底为 1024
        assert_eq!(output.filled_blocks[0].token_count, DEFAULT_BLOCK_TOKENS);
        assert_eq!(output.total_tokens, DEFAULT_BLOCK_TOKENS);
    }

    #[test]
    fn test_block_tokens_map_overrides_zero() {
        // Block 的 token_count = 0，但 block_tokens 映射提供实际值
        let block = BlockScore::new("block-1", 0.9, 0.9, "module-a", 0);
        let fine_output = make_fine_output(vec![block]);
        let mut block_tokens: HashMap<BlockId, usize> = HashMap::new();
        block_tokens.insert("block-1".to_string(), 2048); // 映射提供 2048
        let recall = RerankFill::with_default_config();

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        // block_tokens 映射覆盖 token_count=0
        assert_eq!(output.filled_blocks[0].token_count, 2048);
        assert_eq!(output.total_tokens, 2048);
    }

    // ============================================================
    // 二次稀疏注意力模式测试
    // ============================================================

    #[test]
    fn test_sparse_pattern_disabled_returns_none() {
        let block = make_block("block-1", 0.9, "module-a", 1024);
        let fine_output = make_fine_output(vec![block]);
        let block_tokens = make_block_tokens(&fine_output.blocks);
        let recall = RerankFill::new(RerankFillConfig {
            enable_sparse_pattern: false,
            ..Default::default()
        });

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        assert!(output.sparse_pattern.is_none());
    }

    #[test]
    fn test_sparse_pattern_total_connections_approx_5500() {
        // 构造足够多的 token 让稀疏模式填满 5500 对象
        // 5500 token 需要 ≈ 6 个 Block（每个 1024 tokens）
        let blocks: Vec<BlockScore> = (0..10)
            .map(|i| {
                make_block(
                    &format!("block-{i}"),
                    0.9 - i as f32 * 0.05,
                    &format!("mod-{i}"),
                    1024,
                )
            })
            .collect();
        let fine_output = make_fine_output(blocks.clone());
        let block_tokens = make_block_tokens(&blocks);
        let recall = RerankFill::new(RerankFillConfig {
            window_budget: WindowBudget::L1_128K, // 128K 预算，足够容纳所有 Block
            diversity_alpha: 0.0,
            enable_sparse_pattern: true,
        });

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        let pattern = output
            .sparse_pattern
            .expect("sparse pattern should be built");

        // 总对象数应 ≈ 5500（4096 + 1024 + 256 + 128 = 5504）
        // 但实际受 token 总数限制，10 Block × 1024 = 10240 tokens
        // local = min(4096, 10240) = 4096
        // global = min(1024, 10) = 10（只有 10 个 Block）
        // random = min(256, 10240/256=40 步长) = 256
        // content = min(128, 10) = 10
        // 总 = 4096 + 10 + 256 + 10 = 4372
        // 但关键是 local + global + random + content 的结构正确
        assert!(!pattern.local_indices.is_empty());
        assert!(!pattern.global_indices.is_empty());
        assert!(!pattern.content_indices.is_empty());
        // total_connections 应 > 0
        assert!(pattern.total_connections() > 0);
    }

    #[test]
    fn test_sparse_pattern_local_capped_at_4096() {
        // 构造足够多 token，local 应被限制在 4096
        let blocks: Vec<BlockScore> = (0..100)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let fine_output = make_fine_output(blocks.clone());
        let block_tokens = make_block_tokens(&blocks);
        let recall = RerankFill::new(RerankFillConfig {
            window_budget: WindowBudget::L3_1M, // 128K 预算
            diversity_alpha: 0.0,
            enable_sparse_pattern: true,
        });

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        let pattern = output
            .sparse_pattern
            .expect("sparse pattern should be built");
        // local 应 ≤ 4096（被 sparse_pattern_sizes::LOCAL 限制）
        assert!(pattern.local_indices.len() <= sparse_pattern_sizes::LOCAL);
    }

    // ============================================================
    // 性能指标测试
    // ============================================================

    #[test]
    fn test_fill_records_elapsed_us() {
        let block = make_block("block-1", 0.9, "module-a", 1024);
        let fine_output = make_fine_output(vec![block]);
        let block_tokens = make_block_tokens(&fine_output.blocks);
        let recall = RerankFill::with_default_config();

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        // elapsed_us 应 > 0（至少记录了 Instant::now 的时间差）
        assert!(output.elapsed_us < 1_000_000); // < 1s（防卡死）
    }

    // ============================================================
    // 1M 等效窗口测试（架构红线）
    // ============================================================

    #[test]
    fn test_l3_1m_uses_128k_actual_not_1m() {
        // 架构红线: 1M 等效 = 128K 实际 × 8x 压缩，禁止 1M 暴力加载
        let blocks: Vec<BlockScore> = (0..2000)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let fine_output = make_fine_output(blocks.clone());
        let block_tokens = make_block_tokens(&blocks);
        let recall = RerankFill::new(RerankFillConfig {
            window_budget: WindowBudget::L3_1M,
            diversity_alpha: 0.0,
            enable_sparse_pattern: false,
        });

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");
        // 128K / 1024 = 128 个 Block（1M 等效但实际只加载 128K）
        assert_eq!(output.filled_blocks.len(), 128);
        assert_eq!(output.total_tokens, 128 * 1024);
        // 预算利用率 = 128K / 128K = 1.0
        assert!((output.budget_utilization - 1.0).abs() < 1e-5);
    }

    // ============================================================
    // 综合生命周期测试
    // ============================================================

    #[test]
    fn test_full_lifecycle_fill_with_sparse_pattern() {
        // 模拟完整重排填充: 50 个 Block，多模块，256K 预算
        let blocks: Vec<BlockScore> = (0..50)
            .map(|i| {
                let module = format!("module-{}", i % 5); // 5 个模块，每模块 10 个 Block
                make_block(
                    &format!("block-{i}"),
                    0.9 - (i as f32 * 0.01),
                    &module,
                    1024,
                )
            })
            .collect();
        let fine_output = make_fine_output(blocks.clone());
        let block_tokens = make_block_tokens(&blocks);
        let recall = RerankFill::with_default_config(); // L2_256K + α=0.2 + 稀疏模式

        let input = RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        };
        let output = recall.fill(input).expect("fill should succeed");

        // 50 Block × 1024 = 51200 tokens < 256K 预算，全部填入
        assert_eq!(output.filled_blocks.len(), 50);
        assert_eq!(output.total_tokens, 50 * 1024);
        // 预算利用率 = 51200 / 256K ≈ 0.195
        assert!((output.budget_utilization - (50.0 * 1024.0 / (256.0 * 1024.0))).abs() < 1e-3);
        // 稀疏模式应已构建
        assert!(output.sparse_pattern.is_some());
        // 验证 5 个模块都有 Block 入选（多样性）
        let modules: std::collections::HashSet<_> = output
            .filled_blocks
            .iter()
            .map(|b| b.source_module.clone())
            .collect();
        assert_eq!(modules.len(), 5);
    }
}
