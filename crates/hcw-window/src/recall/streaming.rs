//! HCW-Sparse v2.0 增量流式 — Top-10% 关键块同步放行,剩余后台补满
//!
//! 对应任务: P3-W10.2（spec.md P3 内环升级）
//! 对应病理修复: D1（HCW 无学习机制,静态加载无法适配动态首 token 延迟）
//!
//! # 算法设计（继承 v5.0 设计文档 §4.4 增量流式）
//!
//! ## 两阶段流式加载
//! 1. **同步阶段（Fast <500ms / Deep <2s）**:
//!    - 从 `RerankFillOutput.filled_blocks`（已按密度降序）取 Top-N 关键块
//!    - N = `ceil(total × critical_ratio)`,Fast 模式 ratio=0.1（10%）,
//!      Deep 模式 ratio=0.2（20%）
//!    - 关键块立即返回,LLM 开始 prefill → 首 token 就绪
//! 2. **后台阶段（异步补满）**:
//!    - 剩余 90%（Fast）/ 80%（Deep）的 Block 作为 `background_blocks` 返回
//!    - 由调用方（L7 执行层）在 LLM decode 间隙异步加载
//!    - hcw-window（L2）仅做数据分割,不涉及 spawn 调度（职责分离）
//!
//! ## 延迟-质量权衡（Pareto 前沿）
//! - **Fast 模式**: critical_ratio=0.1,适配 <500ms 首 token（GPU prefill 25.6K tokens）
//!   - 适用于交互式场景（用户等待首 token < 500ms 感知阈值）
//!   - 10% 关键上下文经验上覆盖 80%+ 注意力权重（LLM 长上下文稀疏性）
//! - **Deep 模式**: critical_ratio=0.2,适配 <2s 首 token（更大模型或更深推理）
//!   - 适用于推理密集场景（代码生成/架构设计需更多上下文）
//!   - 20% 关键上下文提升复杂任务的首 token 质量（减少幻觉）
//!
//! ## 与重排填充的衔接
//! - `RerankFillOutput.filled_blocks` 已按密度（`score × (1 + diversity) / token_count`）降序
//! - 增量流式直接取头部 N 个作为关键块,**不重新排序**（保证与重排填充的一致性）
//! - 头部 N 个是"性价比最高"的 Block（高 score / 低 token_count / 模块多样性加成）
//!
//! # 性能预算
//! - 同步阶段分割: O(N) 切片 + O(N×ratio) 拷贝 ≈ 0.01ms（500 Block）
//! - 首 token 延迟: 由 LLM 推理层决定（关键块 token 数 × prefill 速度）
//!   - Fast: 25.6K tokens prefill ≈ 200-400ms（GPU）/ 1-2s（CPU）
//!   - Deep: 51.2K tokens prefill ≈ 400-800ms（GPU）/ 2-4s（CPU）
//! - 本模块仅负责"数据就绪",实际首 token 由 LLM 决定
//!
//! # 架构铁律合规
//! - hcw-window（L2）不向上依赖,不涉及 spawn / async 调度（L7 执行层职责）
//! - `background_blocks` 由调用方决定如何后台加载（EventBus / spawn_blocking / 流式拉取）
//! - 本模块是纯数据分割函数,无副作用,可被多线程并发调用

use std::time::Instant;

use super::rerank::RerankFillOutput;
use super::types::{BlockScore, RecallError};

// ============================================================
// 常量定义
// ============================================================

/// Fast 模式默认关键块比例（Top-10%）
///
/// WHY 0.1: 256K 窗口的 10% = 25.6K tokens,接近主流大模型 prefill 预算,
/// 经验上覆盖 80%+ 注意力权重（LLM 长上下文稀疏性现象）
pub const FAST_CRITICAL_RATIO: f32 = 0.1;

/// Deep 模式默认关键块比例（Top-20%）
///
/// WHY 0.2: 256K 窗口的 20% = 51.2K tokens,为复杂任务提供更多上下文,
/// 首 token 延迟可接受 <2s（推理密集场景）
pub const DEEP_CRITICAL_RATIO: f32 = 0.2;

/// Fast 模式首 token 目标延迟（毫秒）
pub const FAST_FIRST_TOKEN_TARGET_MS: u64 = 500;

/// Deep 模式首 token 目标延迟（毫秒）
pub const DEEP_FIRST_TOKEN_TARGET_MS: u64 = 2000;

// ============================================================
// 流式模式
// ============================================================

/// 增量流式模式 — 延迟-质量权衡的 Pareto 选择
///
/// # 两种模式
/// - `Fast`: 关键块比例 10%,首 token 目标 <500ms（交互式场景）
/// - `Deep`: 关键块比例 20%,首 token 目标 <2s（推理密集场景）
///
/// # 设计决策（WHY）
/// - 用枚举而非 `f32` 比例参数:模式是"语义化策略",固定值便于基准验证红线
/// - 比例值通过 `critical_ratio()` 方法暴露,允许调用方查询但不允许修改（不可变）
/// - 后续 P3-W10.3 selector 权重外置会复用同样的"枚举 + 不可变值"模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamingMode {
    /// Fast 模式: Top-10% 关键块同步放行,首 token <500ms
    ///
    /// 适用于:交互式对话/代码补全/快速问答（用户等待感知阈值 <500ms）
    #[default]
    Fast,
    /// Deep 模式: Top-20% 关键块同步放行,首 token <2s
    ///
    /// 适用于:代码生成/架构设计/复杂推理（需更多上下文减少幻觉）
    Deep,
}

impl StreamingMode {
    /// 返回该模式的关键块比例 ∈ (0.0, 1.0]
    ///
    /// - `Fast` → 0.1（Top-10%）
    /// - `Deep` → 0.2（Top-20%）
    pub fn critical_ratio(&self) -> f32 {
        match self {
            Self::Fast => FAST_CRITICAL_RATIO,
            Self::Deep => DEEP_CRITICAL_RATIO,
        }
    }

    /// 返回该模式的首 token 目标延迟（毫秒）
    ///
    /// 用于基准断言:同步阶段 + LLM prefill 应 < 此值
    pub fn first_token_target_ms(&self) -> u64 {
        match self {
            Self::Fast => FAST_FIRST_TOKEN_TARGET_MS,
            Self::Deep => DEEP_FIRST_TOKEN_TARGET_MS,
        }
    }
}

// ============================================================
// 增量流式配置
// ============================================================

/// 增量流式配置 — 控制关键块比例与流式模式
///
/// # 设计决策（WHY）
/// - `mode`: 流式模式（Fast/Deep）,决定 critical_ratio 与首 token 目标
/// - `custom_ratio`: 自定义关键块比例（None 用模式默认值,Some 覆盖）
///   - 允许调用方在 Fast/Deep 之外微调（如 15% = 介于 Fast 与 Deep 之间）
///   - 必须约束在 (0.0, 1.0] 范围,超出用 clamp 收敛（不报错,容错优先）
/// - `min_critical_blocks`: 最少关键块数（默认 1,避免空关键块导致首 token 无数据）
///
/// # 与 P3-W10.3 SelectorPolicy 的衔接
/// 后续 selector 权重外置会引入 `SelectorPolicy::Static/Learned` 模式,
/// 本配置的 `mode` 字段与之理念一致:固定值编译进二进制,非运行时旗（C4 合规）
#[derive(Debug, Clone, PartialEq)]
pub struct StreamingFillConfig {
    /// 流式模式（Fast/Deep）,决定默认 critical_ratio
    pub mode: StreamingMode,
    /// 自定义关键块比例（None 用模式默认值,Some 覆盖,clamp 到 [0.01, 1.0]）
    pub custom_ratio: Option<f32>,
    /// 最少关键块数（默认 1,避免空关键块）
    pub min_critical_blocks: usize,
}

impl Default for StreamingFillConfig {
    fn default() -> Self {
        Self {
            mode: StreamingMode::default(),
            custom_ratio: None,
            min_critical_blocks: 1,
        }
    }
}

impl StreamingFillConfig {
    /// 创建 Fast 模式配置（critical_ratio=0.1）
    pub fn fast() -> Self {
        Self {
            mode: StreamingMode::Fast,
            ..Default::default()
        }
    }

    /// 创建 Deep 模式配置（critical_ratio=0.2）
    pub fn deep() -> Self {
        Self {
            mode: StreamingMode::Deep,
            ..Default::default()
        }
    }

    /// 创建自定义比例配置（覆盖模式默认值）
    ///
    /// # 参数
    /// - `ratio`: 关键块比例,会被 clamp 到 [0.01, 1.0]
    ///   - < 0.01: 太小,关键块不足以支撑首 token,clamp 到 0.01
    ///   - > 1.0: 超出,clamp 到 1.0（全部同步加载,无后台补满）
    pub fn with_custom_ratio(mut self, ratio: f32) -> Self {
        self.custom_ratio = Some(ratio.clamp(0.01, 1.0));
        self
    }

    /// 返回实际生效的关键块比例（考虑 custom_ratio 覆盖）
    pub fn effective_ratio(&self) -> f32 {
        match self.custom_ratio {
            Some(r) => r.clamp(0.01, 1.0),
            None => self.mode.critical_ratio(),
        }
    }
}

// ============================================================
// 增量流式输入/输出
// ============================================================

/// 增量流式输入 — 复用重排填充输出
///
/// # 字段
/// - `rerank_output`: 重排填充输出（filled_blocks 已按密度降序）
///
/// # 设计决策（WHY）
/// - 直接复用 `RerankFillOutput`,不重新计算密度（保证与重排填充一致性）
/// - 用引用 `&'a` 避免所有权转移,调用方可继续使用 rerank_output
pub struct StreamingFillInput<'a> {
    /// 重排填充输出（filled_blocks 已按密度降序）
    pub rerank_output: &'a RerankFillOutput,
}

/// 增量流式输出 — 关键块（同步）+ 后台块（异步补满）
///
/// # 字段
/// - `critical_blocks`: 同步加载的关键块（Top-N by density,立即返回给 LLM）
/// - `background_blocks`: 后台补满的块（剩余,由调用方异步加载）
/// - `critical_token_count`: 关键块总 token 数（LLM prefill 预算）
/// - `background_token_count`: 后台块总 token 数（异步补满的 token 总量）
/// - `first_token_ready`: 首 token 是否就绪（关键块已分割完成）
/// - `critical_ratio`: 实际生效的关键块比例（effective_ratio 快照）
/// - `elapsed_us`: 同步阶段耗时（微秒）,用于基准断言 <500ms / <2s
///
/// # 不变性（INV）
/// - `critical_blocks.len() + background_blocks.len() == rerank_output.filled_blocks.len()`
/// - `critical_token_count + background_token_count == rerank_output.total_tokens`
/// - `critical_blocks` 是 `filled_blocks` 的前 N 个连续切片（保序）
/// - `background_blocks` 是 `filled_blocks` 的剩余连续切片（保序）
#[derive(Debug, Clone, PartialEq)]
pub struct StreamingFillOutput {
    /// 同步加载的关键块（Top-N by density,已按密度降序）
    pub critical_blocks: Vec<BlockScore>,
    /// 后台补满的块（剩余,保序）
    pub background_blocks: Vec<BlockScore>,
    /// 关键块总 token 数（LLM prefill 预算）
    pub critical_token_count: usize,
    /// 后台块总 token 数（异步补满总量）
    pub background_token_count: usize,
    /// 首 token 是否就绪（关键块已分割）
    pub first_token_ready: bool,
    /// 实际生效的关键块比例
    pub critical_ratio: f32,
    /// 同步阶段耗时（微秒）
    pub elapsed_us: u64,
}

impl StreamingFillOutput {
    /// 返回关键块占比 ∈ [0.0, 1.0]
    pub fn critical_block_ratio(&self) -> f32 {
        let total = self.critical_blocks.len() + self.background_blocks.len();
        if total == 0 {
            0.0
        } else {
            self.critical_blocks.len() as f32 / total as f32
        }
    }

    /// 返回关键块 token 占比 ∈ [0.0, 1.0]
    pub fn critical_token_ratio(&self) -> f32 {
        let total = self.critical_token_count + self.background_token_count;
        if total == 0 {
            0.0
        } else {
            self.critical_token_count as f32 / total as f32
        }
    }

    /// 返回总 Block 数（关键 + 后台）
    pub fn total_block_count(&self) -> usize {
        self.critical_blocks.len() + self.background_blocks.len()
    }

    /// 返回总 token 数（关键 + 后台）
    pub fn total_token_count(&self) -> usize {
        self.critical_token_count + self.background_token_count
    }
}

// ============================================================
// 增量流式引擎
// ============================================================

/// 增量流式引擎 — Top-N 关键块同步放行,剩余后台补满
///
/// # 构建器模式
/// 用 `StreamingFill::new(config)` 或 `StreamingFill::with_default_config()` 构造,
/// 调用 `split()` 执行增量流式分割。
///
/// # 线程安全
/// 引擎本身无可变状态（`&self` 调用）,可被多线程并发调用。
///
/// # 示例
/// ```
/// use hcw_window::recall::{
///     StreamingFill, StreamingFillInput, StreamingFillConfig,
///     RerankFill, RerankFillInput, StreamingMode,
/// };
/// use hcw_window::recall::types::FineRecallOutput;
/// use std::collections::HashMap;
///
/// # fn main() {
/// let fine_output = FineRecallOutput {
///     blocks: vec![],
///     elapsed_us: 0,
///     candidate_count: 0,
/// };
/// let block_tokens: HashMap<String, usize> = HashMap::new();
/// let rerank = RerankFill::with_default_config();
/// let rerank_output = rerank.fill(RerankFillInput {
///     fine_output: &fine_output,
///     block_tokens: &block_tokens,
/// }).unwrap();
///
/// let streaming = StreamingFill::with_default_config();
/// let stream_output = streaming.split(StreamingFillInput {
///     rerank_output: &rerank_output,
/// }).unwrap();
/// # }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct StreamingFill {
    /// 增量流式配置（模式 + 自定义比例 + 最少关键块数）
    config: StreamingFillConfig,
}

impl StreamingFill {
    /// 创建增量流式引擎,使用指定配置
    pub fn new(config: StreamingFillConfig) -> Self {
        Self { config }
    }

    /// 创建增量流式引擎,使用默认配置（Fast 模式 + 10% 关键块）
    pub fn with_default_config() -> Self {
        Self::new(StreamingFillConfig::default())
    }

    /// 创建 Fast 模式引擎（critical_ratio=0.1,首 token <500ms）
    pub fn fast() -> Self {
        Self::new(StreamingFillConfig::fast())
    }

    /// 创建 Deep 模式引擎（critical_ratio=0.2,首 token <2s）
    pub fn deep() -> Self {
        Self::new(StreamingFillConfig::deep())
    }

    /// 返回配置引用（只读）
    pub fn config(&self) -> &StreamingFillConfig {
        &self.config
    }

    /// 执行增量流式分割 — Top-N 关键块同步,剩余后台补满
    ///
    /// # 算法步骤
    /// 1. 计算关键块数 N = `max(ceil(total × ratio), min_critical_blocks)`
    /// 2. 切片: `critical_blocks = filled_blocks[..N]`, `background_blocks = filled_blocks[N..]`
    /// 3. 累加 token_count 分别得到 critical_token_count 与 background_token_count
    /// 4. 置 first_token_ready = true（关键块已分割,可送 LLM prefill）
    ///
    /// # 边界处理
    /// - `filled_blocks` 为空: 返回空输出,first_token_ready=false
    /// - N > total: 全部作为关键块,background_blocks 为空（无后台补满）
    /// - `min_critical_blocks` 保证至少 1 个关键块（避免空 prefill）
    ///
    /// # 性能
    /// - O(N) 切片 + O(N×ratio) 拷贝,500 Block ≈ 0.01ms
    /// - 输出 `elapsed_us` 字段记录同步阶段耗时,供基准断言 <500ms / <2s
    pub fn split(&self, input: StreamingFillInput<'_>) -> Result<StreamingFillOutput, RecallError> {
        let start = Instant::now();
        let filled = &input.rerank_output.filled_blocks;

        // 边界: 重排填充输出为空,返回空结果（首 token 未就绪）
        if filled.is_empty() {
            return Ok(StreamingFillOutput {
                critical_blocks: Vec::new(),
                background_blocks: Vec::new(),
                critical_token_count: 0,
                background_token_count: 0,
                first_token_ready: false,
                critical_ratio: self.config.effective_ratio(),
                elapsed_us: start.elapsed().as_micros() as u64,
            });
        }

        // 1. 计算关键块数 N
        let total = filled.len();
        let ratio = self.config.effective_ratio();
        // ceil(total × ratio): 用 (total as f32 * ratio).ceil() 避免整除丢块
        let n_by_ratio = (total as f32 * ratio).ceil() as usize;
        // 保证最少 min_critical_blocks 个关键块（避免空 prefill）
        let n = n_by_ratio.max(self.config.min_critical_blocks).min(total);

        // 2. 切片（filled_blocks 已按密度降序,前 N 个是性价比最高的）
        //    WHY 用 split_at 而非手动切片: 标准库方法,边界安全且零成本
        let (critical_slice, background_slice) = filled.split_at(n);

        // 3. 累加 token_count（用 block.token_count,0 兜底为 DEFAULT_BLOCK_TOKENS 在重排阶段已处理）
        //    WHY 不重新兜底: 重排填充已将 token_count 注入为非 0 值,
        //    此处直接用 block.token_count 即可,避免重复兜底逻辑
        let critical_token_count: usize = critical_slice
            .iter()
            .map(|b| {
                if b.token_count == 0 {
                    super::rerank::DEFAULT_BLOCK_TOKENS
                } else {
                    b.token_count
                }
            })
            .sum();
        let background_token_count: usize = background_slice
            .iter()
            .map(|b| {
                if b.token_count == 0 {
                    super::rerank::DEFAULT_BLOCK_TOKENS
                } else {
                    b.token_count
                }
            })
            .sum();

        // 4. 构造输出（克隆切片,避免借用 rerank_output）
        let output = StreamingFillOutput {
            critical_blocks: critical_slice.to_vec(),
            background_blocks: background_slice.to_vec(),
            critical_token_count,
            background_token_count,
            first_token_ready: true,
            critical_ratio: ratio,
            elapsed_us: start.elapsed().as_micros() as u64,
        };

        // 不变量自检（debug_assert: 生产零开销,仅 debug 模式验证）
        debug_assert_eq!(
            output.critical_blocks.len() + output.background_blocks.len(),
            total,
            "INV 违规: 关键块 + 后台块数 ≠ 总块数"
        );
        debug_assert_eq!(
            output.critical_token_count + output.background_token_count,
            input.rerank_output.total_tokens,
            "INV 违规: 关键 token + 后台 token ≠ 总 token"
        );

        Ok(output)
    }

    /// 返回该引擎的目标首 token 延迟（毫秒）
    ///
    /// 便捷方法,委托给 config.mode
    pub fn first_token_target_ms(&self) -> u64 {
        self.config.mode.first_token_target_ms()
    }
}

impl Default for StreamingFill {
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
    use crate::recall::rerank::{RerankFill, RerankFillConfig, RerankFillInput, WindowBudget};
    use crate::recall::types::{BlockScore, FineRecallOutput};
    use std::collections::HashMap;

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
    fn make_block_tokens(blocks: &[BlockScore]) -> HashMap<String, usize> {
        blocks
            .iter()
            .map(|b| (b.block_id.clone(), b.token_count))
            .collect()
    }

    /// 通过 RerankFill 生成 RerankFillOutput（端到端测试用）
    fn make_rerank_output(blocks: Vec<BlockScore>, budget: WindowBudget) -> RerankFillOutput {
        let fine_output = make_fine_output(blocks);
        let block_tokens = make_block_tokens(&fine_output.blocks);
        let rerank = RerankFill::new(RerankFillConfig {
            window_budget: budget,
            diversity_alpha: 0.0,
            enable_sparse_pattern: false,
        });
        rerank
            .fill(RerankFillInput {
                fine_output: &fine_output,
                block_tokens: &block_tokens,
            })
            .expect("rerank fill should succeed")
    }

    // ============================================================
    // 流式模式测试
    // ============================================================

    #[test]
    fn test_streaming_mode_default_is_fast() {
        let mode = StreamingMode::default();
        assert_eq!(mode, StreamingMode::Fast);
        assert!((mode.critical_ratio() - 0.1).abs() < 1e-5);
        assert_eq!(mode.first_token_target_ms(), 500);
    }

    #[test]
    fn test_streaming_mode_deep() {
        let mode = StreamingMode::Deep;
        assert!((mode.critical_ratio() - 0.2).abs() < 1e-5);
        assert_eq!(mode.first_token_target_ms(), 2000);
    }

    // ============================================================
    // 配置测试
    // ============================================================

    #[test]
    fn test_config_default_is_fast() {
        let config = StreamingFillConfig::default();
        assert_eq!(config.mode, StreamingMode::Fast);
        assert!(config.custom_ratio.is_none());
        assert_eq!(config.min_critical_blocks, 1);
        assert!((config.effective_ratio() - 0.1).abs() < 1e-5);
    }

    #[test]
    fn test_config_fast_constructor() {
        let config = StreamingFillConfig::fast();
        assert_eq!(config.mode, StreamingMode::Fast);
        assert!((config.effective_ratio() - 0.1).abs() < 1e-5);
    }

    #[test]
    fn test_config_deep_constructor() {
        let config = StreamingFillConfig::deep();
        assert_eq!(config.mode, StreamingMode::Deep);
        assert!((config.effective_ratio() - 0.2).abs() < 1e-5);
    }

    #[test]
    fn test_config_custom_ratio_overrides_mode() {
        let config = StreamingFillConfig::fast().with_custom_ratio(0.15);
        assert!(config.custom_ratio.is_some());
        assert!((config.effective_ratio() - 0.15).abs() < 1e-5);
    }

    #[test]
    fn test_config_custom_ratio_clamped_to_min() {
        // 0.001 < 0.01,应 clamp 到 0.01
        let config = StreamingFillConfig::fast().with_custom_ratio(0.001);
        assert!((config.effective_ratio() - 0.01).abs() < 1e-5);
    }

    #[test]
    fn test_config_custom_ratio_clamped_to_max() {
        // 1.5 > 1.0,应 clamp 到 1.0
        let config = StreamingFillConfig::fast().with_custom_ratio(1.5);
        assert!((config.effective_ratio() - 1.0).abs() < 1e-5);
    }

    // ============================================================
    // 引擎基础功能测试
    // ============================================================

    #[test]
    fn test_split_empty_input_returns_empty() {
        let rerank_output = make_rerank_output(vec![], WindowBudget::L2_256K);
        let streaming = StreamingFill::with_default_config();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        assert!(output.critical_blocks.is_empty());
        assert!(output.background_blocks.is_empty());
        assert_eq!(output.critical_token_count, 0);
        assert!(!output.first_token_ready);
    }

    #[test]
    fn test_split_single_block_all_critical() {
        // 1 个 Block,Fast 模式 10% → ceil(1 × 0.1) = 1,min_critical=1
        // 全部作为关键块,background 为空
        let block = make_block("block-1", 0.9, "module-a", 1024);
        let rerank_output = make_rerank_output(vec![block], WindowBudget::L2_256K);
        let streaming = StreamingFill::fast();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        assert_eq!(output.critical_blocks.len(), 1);
        assert!(output.background_blocks.is_empty());
        assert_eq!(output.critical_token_count, 1024);
        assert_eq!(output.background_token_count, 0);
        assert!(output.first_token_ready);
    }

    #[test]
    fn test_split_fast_mode_10_percent() {
        // 100 个 Block,Fast 模式 10% → ceil(100 × 0.1) = 10 个关键块
        // 注意: WindowBudget::L3_1M 预算 128K,100 块 × 1024 = 100K < 128K,全部填入
        let blocks: Vec<BlockScore> = (0..100)
            .map(|i| {
                make_block(
                    &format!("block-{i}"),
                    1.0 - i as f32 * 0.005, // score 递减,保证密度降序
                    "module-a",
                    1024,
                )
            })
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L3_1M);
        let streaming = StreamingFill::fast();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // Fast 10%: 100 块 × 0.1 = 10 → ceil = 10 个关键块
        assert_eq!(output.critical_blocks.len(), 10);
        assert_eq!(output.background_blocks.len(), 100 - 10);
        assert!(output.first_token_ready);
    }

    #[test]
    fn test_split_deep_mode_20_percent() {
        // 同样 100 个 Block,Deep 模式 20% → ceil(100 × 0.2) = 20 个关键块
        let blocks: Vec<BlockScore> = (0..100)
            .map(|i| {
                make_block(
                    &format!("block-{i}"),
                    1.0 - i as f32 * 0.005,
                    "module-a",
                    1024,
                )
            })
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L3_1M);
        let streaming = StreamingFill::deep();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // Deep 20%: 100 块 × 0.2 = 20 → ceil = 20 个关键块
        assert_eq!(output.critical_blocks.len(), 20);
        assert_eq!(output.background_blocks.len(), 100 - 20);
        assert!(output.first_token_ready);
    }

    // ============================================================
    // 关键块保序性测试（密度降序）
    // ============================================================

    #[test]
    fn test_critical_blocks_preserve_density_order() {
        // 构造密度降序的 Block（rerank.fill 后已排序）
        let blocks: Vec<BlockScore> = (0..10)
            .map(|i| {
                make_block(
                    &format!("block-{i}"),
                    1.0 - i as f32 * 0.05, // score 递减 → 密度降序
                    "module-a",
                    1024,
                )
            })
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L1_128K);
        let streaming = StreamingFill::fast();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // Fast 10%: 10 块 × 0.1 = 1 → ceil = 1,min_critical=1
        // 关键块应为密度最高的 block-0
        assert_eq!(output.critical_blocks.len(), 1);
        assert_eq!(output.critical_blocks[0].block_id, "block-0");
    }

    #[test]
    fn test_background_blocks_preserve_order() {
        // 10 个 Block,Fast 10% → 1 个关键 + 9 个后台
        let blocks: Vec<BlockScore> = (0..10)
            .map(|i| {
                make_block(
                    &format!("block-{i}"),
                    1.0 - i as f32 * 0.05,
                    "module-a",
                    1024,
                )
            })
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L1_128K);
        let streaming = StreamingFill::fast();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // 后台块应为 block-1 ~ block-9（保序）
        assert_eq!(output.background_blocks.len(), 9);
        assert_eq!(output.background_blocks[0].block_id, "block-1");
        assert_eq!(output.background_blocks[8].block_id, "block-9");
    }

    // ============================================================
    // 不变量测试
    // ============================================================

    #[test]
    fn test_invariant_total_blocks_preserved() {
        let blocks: Vec<BlockScore> = (0..50)
            .map(|i| make_block(&format!("block-{i}"), 0.9, &format!("mod-{}", i % 5), 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L1_128K);
        let streaming = StreamingFill::with_default_config();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // INV: critical + background = total
        assert_eq!(
            output.critical_blocks.len() + output.background_blocks.len(),
            rerank_output.filled_blocks.len()
        );
    }

    #[test]
    fn test_invariant_total_tokens_preserved() {
        let blocks: Vec<BlockScore> = (0..50)
            .map(|i| make_block(&format!("block-{i}"), 0.9, &format!("mod-{}", i % 5), 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L1_128K);
        let streaming = StreamingFill::with_default_config();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // INV: critical_token + background_token = total_tokens
        assert_eq!(
            output.critical_token_count + output.background_token_count,
            rerank_output.total_tokens
        );
    }

    // ============================================================
    // 边界情况测试
    // ============================================================

    #[test]
    fn test_min_critical_blocks_enforced() {
        // 5 个 Block,Fast 10% → ceil(5 × 0.1) = 1,但 min_critical=1 保证至少 1 个
        let blocks: Vec<BlockScore> = (0..5)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L0_32K);
        let streaming = StreamingFill::fast();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // 至少 1 个关键块
        assert!(!output.critical_blocks.is_empty());
        assert!(output.first_token_ready);
    }

    #[test]
    fn test_custom_ratio_100_percent_all_critical() {
        // 自定义 100% → 全部作为关键块,background 为空
        let blocks: Vec<BlockScore> = (0..10)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L1_128K);
        let streaming = StreamingFill::new(StreamingFillConfig::fast().with_custom_ratio(1.0));

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // 全部关键块,无后台补满
        assert_eq!(
            output.critical_blocks.len(),
            rerank_output.filled_blocks.len()
        );
        assert!(output.background_blocks.is_empty());
        assert_eq!(output.background_token_count, 0);
    }

    #[test]
    fn test_n_exceeds_total_all_critical() {
        // 3 个 Block,Deep 20% → ceil(3 × 0.2) = 1,但 min_critical=1
        // 实际 n = max(1, 1).min(3) = 1,关键块 1 个,后台 2 个
        let blocks: Vec<BlockScore> = (0..3)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L1_128K);
        let streaming = StreamingFill::deep();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        assert_eq!(output.critical_blocks.len(), 1);
        assert_eq!(output.background_blocks.len(), 2);
    }

    // ============================================================
    // token_count 兜底测试
    // ============================================================

    #[test]
    fn test_token_count_zero_uses_default_in_split() {
        // Block 的 token_count = 0,split 阶段应兜底为 DEFAULT_BLOCK_TOKENS（1024）
        // 注意: rerank.fill 已将 token_count 注入为非 0,但若直接构造 RerankFillOutput
        // 跳过 rerank（如测试场景）,split 需要兜底
        // 为保持 INV 一致性, total_tokens 必须与兜底后的值匹配
        let default_tokens = super::super::rerank::DEFAULT_BLOCK_TOKENS;
        let block = BlockScore::new("block-1", 0.9, 0.9, "module-a", 0);
        let rerank_output = RerankFillOutput {
            filled_blocks: vec![block],
            // 与 split 兜底逻辑一致: token_count=0 时按 DEFAULT_BLOCK_TOKENS 计算
            total_tokens: default_tokens,
            sparse_pattern: None,
            elapsed_us: 0,
            budget_utilization: 0.0,
        };
        let streaming = StreamingFill::fast();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // token_count=0 兜底为 DEFAULT_BLOCK_TOKENS（1024）
        assert_eq!(output.critical_token_count, default_tokens);
    }

    // ============================================================
    // 输出辅助方法测试
    // ============================================================

    #[test]
    fn test_output_critical_block_ratio() {
        // 100 个 Block,Fast 10% → 13 个关键 / 115 个后台（L3_1M = 128K / 1024 = 128 块）
        // 实际 100 块 < 128 块预算,全部填入 → 100 块,10% = 10 关键
        let blocks: Vec<BlockScore> = (0..100)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L3_1M);
        let streaming = StreamingFill::fast();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // critical_block_ratio = 10 / 100 = 0.1
        assert!((output.critical_block_ratio() - 0.1).abs() < 1e-2);
    }

    #[test]
    fn test_output_total_block_count() {
        let blocks: Vec<BlockScore> = (0..50)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L1_128K);
        let streaming = StreamingFill::with_default_config();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        assert_eq!(
            output.total_block_count(),
            rerank_output.filled_blocks.len()
        );
    }

    #[test]
    fn test_output_total_token_count() {
        let blocks: Vec<BlockScore> = (0..50)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L1_128K);
        let streaming = StreamingFill::with_default_config();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        assert_eq!(output.total_token_count(), rerank_output.total_tokens);
    }

    // ============================================================
    // 性能指标测试
    // ============================================================

    #[test]
    fn test_split_records_elapsed_us() {
        let blocks: Vec<BlockScore> = (0..100)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L3_1M);
        let streaming = StreamingFill::fast();

        let output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");

        // elapsed_us 应远小于 500ms（Fast 模式目标）
        assert!(output.elapsed_us < 500_000);
    }

    #[test]
    fn test_first_token_target_ms_delegation() {
        let fast = StreamingFill::fast();
        assert_eq!(fast.first_token_target_ms(), 500);

        let deep = StreamingFill::deep();
        assert_eq!(deep.first_token_target_ms(), 2000);
    }

    // ============================================================
    // 综合生命周期测试
    // ============================================================

    #[test]
    fn test_full_lifecycle_rerank_then_streaming() {
        // 端到端: 50 个 Block → rerank.fill → streaming.split
        let blocks: Vec<BlockScore> = (0..50)
            .map(|i| {
                let module = format!("module-{}", i % 5);
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

        // 阶段 1: rerank.fill
        let rerank = RerankFill::with_default_config();
        let rerank_output = rerank
            .fill(RerankFillInput {
                fine_output: &fine_output,
                block_tokens: &block_tokens,
            })
            .expect("rerank fill should succeed");

        // 阶段 2: streaming.split（Fast 模式）
        let streaming = StreamingFill::fast();
        let stream_output = streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("streaming split should succeed");

        // 验证不变量
        assert_eq!(
            stream_output.critical_blocks.len() + stream_output.background_blocks.len(),
            rerank_output.filled_blocks.len()
        );
        assert_eq!(
            stream_output.critical_token_count + stream_output.background_token_count,
            rerank_output.total_tokens
        );
        assert!(stream_output.first_token_ready);

        // Fast 模式 10%: 50 块 × 0.1 = 5 → ceil = 5 个关键块
        assert_eq!(stream_output.critical_blocks.len(), 5);
        assert_eq!(stream_output.background_blocks.len(), 45);

        // 关键块 token 占比应接近 10%
        let ratio = stream_output.critical_token_ratio();
        assert!(
            ratio > 0.05 && ratio < 0.15,
            "critical token ratio = {ratio}"
        );
    }

    #[test]
    fn test_deep_mode_more_critical_than_fast() {
        // 同样的 Block,Deep 模式应有更多关键块
        let blocks: Vec<BlockScore> = (0..100)
            .map(|i| make_block(&format!("block-{i}"), 0.9, "module-a", 1024))
            .collect();
        let rerank_output = make_rerank_output(blocks, WindowBudget::L3_1M);

        let fast_output = StreamingFill::fast()
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("fast split should succeed");

        let deep_output = StreamingFill::deep()
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("deep split should succeed");

        // Deep 应有更多关键块（20% > 10%）
        assert!(deep_output.critical_blocks.len() > fast_output.critical_blocks.len());
    }
}
