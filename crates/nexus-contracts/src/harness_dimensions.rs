//! 六维控制面契约 — MemoHarness D1-D6 融合（设计文档 §5.3.1 + v3.4.0 §5.6）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §5.6
//! 对应论文: MemoHarness（六维控制面搜索 + 双层经验库）
//!           + OpenMLE（按需记忆合成 / 三因子算子选择 / 熵加权 / 错误签名 / 执行状态追踪）
//!           + PenguinHarness（Skills 渐进加载）
//!           + RSIBench（停止策略）
//!
//! # 核心职责
//!
//! 承载 MemoHarness 六维控制面的跨层契约定义，使 L6 Router / L9 Quest / L10 Interface
//! 能按统一契约调整 Harness 行为。六个维度分别对应：
//!
//! | 维度 | 契约 | 控制面 | 消费层 |
//! |------|------|--------|--------|
//! | D1 | [`ContextAssemblyContract`] | 上下文组装 | L6 OSA / L2 HCW |
//! | D2 | [`ToolInteractionContract`] | 工具交互 | L6 FaaE / L7 GQEP |
//! | D3 | [`GenerationControlContract`] | 生成控制 | L6 Router / L1 model-router |
//! | D4 | [`TaskOrchestrationContract`] | 任务编排 | L9 quest-engine / L8 parliament |
//! | D5 | [`MemoryManagementContract`] | Memory 管理 | L2 mlc-engine / L3 cmt-tiering |
//! | D6 | [`OutputProcessingContract`] | 输出处理 | L7 PVL / L10 TUI |
//!
//! # v3.4.0 融合扩展（2026-08-16）
//!
//! 在 v2.x 六维基础上补齐 OpenMLE/PenguinHarness/RSIBench 融合字段：
//! - **D1/D5**: 按需记忆合成（on_demand_synthesis）+ 祖先/兄弟检索深度
//! - **D2**: Skills 渐进加载（Index First, Body on Demand）
//! - **D3**: 算子选择策略（Greedy/ThreeFactor/UCB/Cooling）+ 熵加权
//! - **D4**: 搜索树深度 + 预算小时 + RSIBench 停止策略
//! - **D5**: 双层经验库 + 全局经验板
//! - **D6**: 错误签名收集 + 六类执行状态追踪
//!
//! 所有新增字段均标注 `#[serde(default)]`，旧 JSON 配置反序列化不破坏（向后兼容）。
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型 + 零逻辑**: 仅类型定义与基础构造函数，不含业务逻辑
//! - **零 crate 依赖**: 仅 `serde` derive（ADR-033 白名单例外）
//! - **f32 字段不 derive Eq/Hash**: temperature/top_p/budget_hours/score_gap_threshold
//!   等浮点字段仅 `PartialEq`（浮点比较红线，见项目记忆 f32 陷阱）
//! - **RetryPolicy 复用**: 复用 `harness_spec::RetryPolicy`（L0 同层引用），
//!   避免重复定义
//!
//! # 与 HarnessSpec 的职责边界
//!
//! - [`HarnessSpec`](crate::harness_spec::HarnessSpec): 静态规格定义（DSL），
//!   描述 Harness 的不可进化面与验收门，用于 GSOE 进化约束
//! - [`HarnessConfigContract`]: 运行时控制面配置，描述 Harness 的六维可调参数，
//!   用于 MemoHarness 搜索阶段的控制面调优
//!
//! 两者互补：HarnessSpec 定义"什么不能变"，HarnessConfigContract 定义"什么可以调"。

use serde::{Deserialize, Serialize};

// 复用 harness_spec 的 RetryPolicy（L0 同层引用，避免重复定义）
pub use crate::harness_spec::RetryPolicy;

// ============================================================
// 子枚举定义
// ============================================================

/// 上下文压缩策略 — D1 子枚举
///
/// WHY 闭集枚举: 压缩策略集合是封闭的，枚举提供编译期穷尽检查。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionStrategy {
    /// 不压缩 — 完整上下文传入
    None,
    /// 滑动窗口 — 保留最近 N 轮对话
    SlidingWindow,
    /// 摘要压缩 — 对历史对话生成摘要
    Summarization,
    /// 语义检索 — 按需检索相关片段（OSA 稀疏化）
    SemanticRetrieval,
}

/// 工作流类型 — D4 子枚举
///
/// WHY 闭集枚举: 任务编排模式集合是封闭的（单调用/计划执行/多 Agent）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowType {
    /// 单次调用 — 无分解，直接执行
    SingleCall,
    /// 计划执行 — Quest 分解为 DAG 后执行
    PlanExecute,
    /// 多 Agent 协同 — chimera-mas 四象限分工
    MultiAgent,
    /// 搜索树 — OpenMLE 搜索树模式（扩展/选择/剪枝/最优路径回溯）
    SearchTree,
}

/// 记忆保留策略 — D5 子枚举
///
/// WHY 闭集枚举: 保留策略决定记忆生命周期，集合封闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
    /// 会话级 — 会话结束即清除
    Session,
    /// 持久化 — 跨会话保留（LHQP 检查点）
    Persistent,
    /// 自适应 — 按任务阶段自适应调整（MemCon 策略）
    Adaptive,
}

/// 记忆驱逐策略 — D5 子枚举
///
/// WHY 闭集枚举: 驱逐策略决定记忆容量管理，集合封闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvictionStrategy {
    /// LRU — 最近最少使用
    Lru,
    /// LFU — 最不频繁使用
    Lfu,
    /// 语义相似度 — 驱逐与当前上下文最不相似的记忆
    SemanticDistance,
    /// 分层驱逐 — 按 ArchiveTier 逐级降级（INV-8 单调性）
    TieredArchive,
}

/// 算子选择策略 — D3 子枚举（OpenMLE）
///
/// 决定生成控制层如何选择四套原子算子（Draft/Improve/Debug/Crossover）。
/// WHY 闭集枚举: 策略集合封闭，编译期穷尽检查。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorSelectionStrategy {
    /// 贪心 — 始终选最高分算子
    Greedy,
    /// 三因子 — OpenMLE Q+P+N 综合选择
    ThreeFactor,
    /// UCB — 上置信界探索（访问计数平衡）
    Ucb,
    /// 冷却 — 温度退火随机选择
    Cooling,
}

impl Default for OperatorSelectionStrategy {
    /// 默认三因子策略（OpenMLE 实证最优）
    fn default() -> Self {
        Self::ThreeFactor
    }
}

/// 停止策略配置 — D4 子类型（RSIBench）
///
/// 搜索停止条件：尝试次数上限 / 停滞计数 / 分数差距阈值 / 保留最佳。
/// `score_gap_threshold` 为 f32 字段，仅 `PartialEq`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StopStrategyConfig {
    /// 最大尝试次数
    pub max_attempts: u32,
    /// 停滞计数阈值（连续 N 轮无改进即停滞）
    pub stagnation_threshold: u32,
    /// 分数差距阈值（低于该改进视为停滞）
    pub score_gap_threshold: f32,
    /// 是否保留历史最佳（RSIBench: 58.33% 超过首次，必须保留最佳）
    pub preserve_best: bool,
}

impl Default for StopStrategyConfig {
    /// 默认: 5 次尝试 / 3 轮停滞 / 0.05 差距 / 保留最佳
    fn default() -> Self {
        Self {
            max_attempts: 5,
            stagnation_threshold: 3,
            score_gap_threshold: 0.05,
            preserve_best: true,
        }
    }
}

/// 输出提取格式 — D6 子枚举
///
/// WHY 闭集枚举: 输出格式决定后处理管线，集合封闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionFormat {
    /// 纯文本 — 无结构化提取
    PlainText,
    /// JSON — 结构化 JSON 提取
    Json,
    /// Markdown — Markdown 格式提取
    Markdown,
    /// 代码块 — 仅提取代码块内容
    CodeBlock,
}

/// 回退策略 — D6 子枚举
///
/// WHY 闭集枚举: 输出校验失败时的回退路径，集合封闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackStrategy {
    /// 直接返回原始输出 — 不做回退处理
    Passthrough,
    /// 重试 — 重新生成（受 RetryPolicy 约束）
    Retry,
    /// 降级 — 使用简化模板生成
    Degraded,
    /// 人工审核 — 暂停并等待人工确认
    HumanReview,
}

/// 输出校验规则 — D6 子类型
///
/// WHY 结构体而非枚举: 校验规则需要携带参数（如最大长度、正则模式），
/// 结构体提供扩展灵活性。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationRule {
    /// 规则名称（如 "max_length" / "json_schema" / "no_pii"）
    pub rule_name: String,
    /// 规则参数（JSON 字符串，由消费方解析）
    pub rule_params: String,
    /// 是否为硬约束（true = 违反即拒绝，false = 警告）
    pub is_hard_constraint: bool,
}

// ============================================================
// D1: 上下文组装契约
// ============================================================

/// D1 上下文组装契约 — 控制上下文窗口的组装策略
///
/// 对应 MemoHarness D1 维度：上下文组装决定 Agent 看到什么信息。
/// 与 OSA 稀疏化（Ω₁-Sparse）和 HCW 分层窗口（Ω₂-Compress）协同。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextAssemblyContract {
    /// 最大 token 数（上下文窗口上限）
    pub max_tokens: usize,
    /// 压缩策略
    pub compression_strategy: CompressionStrategy,
    /// 是否注入示例（few-shot example injection）
    pub example_injection: bool,
    /// 是否使用结构化 prompt（system/user/assistant 角色分离）
    pub structured_prompt: bool,
    /// 是否启用按需记忆合成（OpenMLE: 懒加载祖先+兄弟节点，Prompt 降 60%-86%）
    #[serde(default = "default_true")]
    pub on_demand_synthesis: bool,
    /// 祖先检索深度（OpenMLE: 按需合成时的祖先节点回溯层数）
    #[serde(default)]
    pub ancestor_retrieval_depth: u32,
    /// 兄弟节点检索数量（OpenMLE: 按需合成时的兄弟节点取回数）
    #[serde(default)]
    pub sibling_retrieval_count: u32,
}

/// `#[serde(default = ...)]` 辅助 — 新增布尔字段默认启用
///
/// WHY 独立函数: serde(default) 无法直接引用 `true` 字面量。
fn default_true() -> bool {
    true
}

impl ContextAssemblyContract {
    /// 创建默认上下文组装契约
    ///
    /// 默认值：128K token / 语义检索压缩 / 注入示例 / 结构化 prompt /
    /// 按需合成开启 / 祖先深度 2 / 兄弟 3 个
    pub fn default_contract() -> Self {
        Self {
            max_tokens: 128_000,
            compression_strategy: CompressionStrategy::SemanticRetrieval,
            example_injection: true,
            structured_prompt: true,
            on_demand_synthesis: true,
            ancestor_retrieval_depth: 2,
            sibling_retrieval_count: 3,
        }
    }
}

// ============================================================
// D2: 工具交互契约
// ============================================================

/// D2 工具交互契约 — 控制工具调用的检索与执行策略
///
/// 对应 MemoHarness D2 维度：工具交互决定 Agent 如何使用工具。
/// 与 FaaE 工具路由（Ω₁-Sparse 工具维度）和 GQEP 聚集执行协同。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInteractionContract {
    /// 每步最大工具调用数
    pub max_tools_per_step: usize,
    /// 工具检索 Top-K（FaaE 语义路由）
    pub retrieval_top_k: usize,
    /// 是否启用重排序（reranking）
    pub reranking_enabled: bool,
    /// 工具执行超时（毫秒）
    pub tool_timeout_ms: u64,
    /// 是否启用 Skills 渐进加载（PenguinHarness: Index First, Body on Demand）
    #[serde(default = "default_true")]
    pub progressive_skill_loading: bool,
    /// 单步最大全量加载 Skill 数（超过则仅加载索引）
    #[serde(default)]
    pub max_full_skill_load: usize,
}

impl ToolInteractionContract {
    /// 创建默认工具交互契约
    ///
    /// 默认值：5 工具/步 / Top-10 检索 / 启用重排序 / 30s 超时 /
    /// 渐进加载开启 / 最多 4 个全量 Skill
    pub fn default_contract() -> Self {
        Self {
            max_tools_per_step: 5,
            retrieval_top_k: 10,
            reranking_enabled: true,
            tool_timeout_ms: 30_000,
            progressive_skill_loading: true,
            max_full_skill_load: 4,
        }
    }
}

// ============================================================
// D3: 生成控制契约
// ============================================================

/// D3 生成控制契约 — 控制 LLM 生成的采样参数
///
/// 对应 MemoHarness D3 维度：生成控制决定 LLM 如何生成文本。
/// 与 model-router 的 CACR 路由和 MCA 亲和体系协同。
///
/// # f32 约束
///
/// temperature / top_p 为 f32 字段，故仅 `PartialEq`（不 derive `Eq`/`Hash`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationControlContract {
    /// 最大输出 token 数
    pub max_output_tokens: usize,
    /// 采样温度 [0.0, 2.0]
    pub temperature: f32,
    /// Top-P 采样阈值 [0.0, 1.0]
    pub top_p: f32,
    /// 是否启用候选采样（多候选生成后选择最优）
    pub candidate_sampling: bool,
    /// 算子选择策略（OpenMLE: 四策略路由）
    #[serde(default)]
    pub operator_selection: OperatorSelectionStrategy,
    /// 是否启用熵加权（OpenMLE: 放大最优样本梯度权重 ~4 倍）
    #[serde(default = "default_true")]
    pub entropy_weighting: bool,
}

impl GenerationControlContract {
    /// 创建默认生成控制契约
    ///
    /// 默认值：4096 token / temperature=0.7 / top_p=0.9 / 不启用候选采样 /
    /// 三因子算子选择 / 熵加权开启
    pub fn default_contract() -> Self {
        Self {
            max_output_tokens: 4096,
            temperature: 0.7,
            top_p: 0.9,
            candidate_sampling: false,
            operator_selection: OperatorSelectionStrategy::default(),
            entropy_weighting: true,
        }
    }
}

// ============================================================
// D4: 任务编排契约
// ============================================================

/// D4 任务编排契约 — 控制任务分解与执行的工作流模式
///
/// 对应 MemoHarness D4 维度：任务编排决定 Agent 如何组织执行步骤。
/// 与 quest-engine DAG 管理和 parliament 审议协同。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskOrchestrationContract {
    /// 工作流类型
    pub workflow_type: WorkflowType,
    /// 最大迭代次数（PlanExecute 模式的 DAG 深度上限）
    pub max_iterations: u32,
    /// 重试策略（复用 harness_spec::RetryPolicy）
    pub retry_policy: RetryPolicy,
    /// 搜索树深度上限（OpenMLE: SearchTree 工作流）
    #[serde(default)]
    pub search_tree_depth: u32,
    /// 任务预算小时数（OpenMLE: 预算熔断）
    #[serde(default)]
    pub budget_hours: f32,
    /// 停止策略（RSIBench: 尝试/停滞/差距/保留最佳）
    #[serde(default)]
    pub stop_strategy: StopStrategyConfig,
}

impl TaskOrchestrationContract {
    /// 创建默认任务编排契约
    ///
    /// 默认值：PlanExecute / 10 次迭代 / 默认重试策略 /
    /// 搜索深度 8 / 预算 24h / 默认停止策略
    pub fn default_contract() -> Self {
        Self {
            workflow_type: WorkflowType::PlanExecute,
            max_iterations: 10,
            retry_policy: RetryPolicy::default(),
            search_tree_depth: 8,
            budget_hours: 24.0,
            stop_strategy: StopStrategyConfig::default(),
        }
    }
}

// ============================================================
// D5: Memory 管理契约
// ============================================================

/// D5 Memory 管理契约 — 控制记忆的保留、摘要与驱逐策略
///
/// 对应 MemoHarness D5 维度：Memory 管理决定 Agent 如何维护长期记忆。
/// 与 MLC 四级记忆（Ω₂-Compress）和 CMT 分层存储协同。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryManagementContract {
    /// 记忆保留策略
    pub retention_policy: RetentionPolicy,
    /// 摘要触发阈值（记忆条目数达到此值时触发摘要压缩）
    pub summarization_trigger: usize,
    /// 记忆驱逐策略
    pub eviction_strategy: EvictionStrategy,
    /// 是否启用按需记忆合成（OpenMLE: 懒加载，不阻塞主流程）
    #[serde(default = "default_true")]
    pub on_demand_synthesis: bool,
    /// 祖先检索深度（OpenMLE）
    #[serde(default)]
    pub ancestor_retrieval_depth: u32,
    /// 兄弟节点检索数量（OpenMLE）
    #[serde(default)]
    pub sibling_retrieval_count: u32,
    /// 是否启用双层经验库（MemoHarness: 案例级 + 全局）
    #[serde(default = "default_true")]
    pub dual_experience_bank: bool,
    /// 是否启用全局经验板（OpenMLE: 搜索树全局统计 + 错误聚类）
    #[serde(default = "default_true")]
    pub global_experience_board: bool,
}

impl MemoryManagementContract {
    /// 创建默认 Memory 管理契约
    ///
    /// 默认值：自适应保留 / 100 条触发摘要 / 分层驱逐 /
    /// 按需合成开启 / 祖先深度 2 / 兄弟 3 个 / 双层经验库 / 全局经验板
    pub fn default_contract() -> Self {
        Self {
            retention_policy: RetentionPolicy::Adaptive,
            summarization_trigger: 100,
            eviction_strategy: EvictionStrategy::TieredArchive,
            on_demand_synthesis: true,
            ancestor_retrieval_depth: 2,
            sibling_retrieval_count: 3,
            dual_experience_bank: true,
            global_experience_board: true,
        }
    }
}

// ============================================================
// D6: 输出处理契约
// ============================================================

/// D6 输出处理契约 — 控制输出的提取、校验与回退策略
///
/// 对应 MemoHarness D6 维度：输出处理决定 Agent 如何后处理 LLM 输出。
/// 与 PVL 生产验证循环和 L4 SecCore 输出校验协同。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputProcessingContract {
    /// 输出提取格式
    pub extraction_format: ExtractionFormat,
    /// 输出校验规则集合
    pub validation_rules: Vec<ValidationRule>,
    /// 回退策略
    pub fallback_strategy: FallbackStrategy,
    /// 是否收集错误签名（OpenMLE: 结构化收集 + 哈希去重聚类，铁律7）
    #[serde(default = "default_true")]
    pub collect_error_signatures: bool,
    /// 是否追踪六类执行状态（OpenMLE: Success/Error/MissingCode/NoSubmit/ScoreFailed/Timeout，铁律8）
    #[serde(default = "default_true")]
    pub track_execution_status: bool,
}

impl OutputProcessingContract {
    /// 创建默认输出处理契约
    ///
    /// 默认值：Markdown 提取 / 无校验规则 / 重试回退 /
    /// 错误签名收集开启 / 执行状态追踪开启
    pub fn default_contract() -> Self {
        Self {
            extraction_format: ExtractionFormat::Markdown,
            validation_rules: Vec::new(),
            fallback_strategy: FallbackStrategy::Retry,
            collect_error_signatures: true,
            track_execution_status: true,
        }
    }
}

// ============================================================
// 聚合体：HarnessConfigContract
// ============================================================

/// Harness 配置契约 — 六维控制面的完整聚合体
///
/// 承载 MemoHarness 六维控制面的完整配置，用于：
/// - MemoHarness 搜索阶段的控制面调优（Ω₆-Reuse）
/// - AEGIS Evolver 生成变体时的控制面约束输入
/// - Runtime Auditor 审计控制面配置合规性
///
/// # 版本与哈希
///
/// `version` 字段为语义化版本号（如 "1.0.0"），`hash` 为配置内容的
/// SHA-256 哈希（由消费方计算，L0 仅承载字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessConfigContract {
    /// D1: 上下文组装契约
    pub d1_context: ContextAssemblyContract,
    /// D2: 工具交互契约
    pub d2_tool: ToolInteractionContract,
    /// D3: 生成控制契约
    pub d3_generation: GenerationControlContract,
    /// D4: 任务编排契约
    pub d4_orchestration: TaskOrchestrationContract,
    /// D5: Memory 管理契约
    pub d5_memory: MemoryManagementContract,
    /// D6: 输出处理契约
    pub d6_output: OutputProcessingContract,
    /// 配置版本号（语义化版本，如 "1.0.0"）
    pub version: String,
    /// 配置内容哈希（SHA-256，由消费方计算）
    pub hash: String,
}

impl HarnessConfigContract {
    /// 创建默认六维控制面配置
    ///
    /// 所有维度使用默认值，version = "0.1.0"，hash 为空（由消费方填充）。
    pub fn default_contract() -> Self {
        Self {
            d1_context: ContextAssemblyContract::default_contract(),
            d2_tool: ToolInteractionContract::default_contract(),
            d3_generation: GenerationControlContract::default_contract(),
            d4_orchestration: TaskOrchestrationContract::default_contract(),
            d5_memory: MemoryManagementContract::default_contract(),
            d6_output: OutputProcessingContract::default_contract(),
            version: "0.1.0".to_string(),
            hash: String::new(),
        }
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------
    // 默认值测试
    // ----------------------------------------------------------

    #[test]
    fn test_context_assembly_default() {
        let c = ContextAssemblyContract::default_contract();
        assert_eq!(c.max_tokens, 128_000);
        assert_eq!(
            c.compression_strategy,
            CompressionStrategy::SemanticRetrieval
        );
        assert!(c.example_injection);
        assert!(c.structured_prompt);
        // v3.4.0 OpenMLE 扩展默认值
        assert!(c.on_demand_synthesis);
        assert_eq!(c.ancestor_retrieval_depth, 2);
        assert_eq!(c.sibling_retrieval_count, 3);
    }

    #[test]
    fn test_tool_interaction_default() {
        let c = ToolInteractionContract::default_contract();
        assert_eq!(c.max_tools_per_step, 5);
        assert_eq!(c.retrieval_top_k, 10);
        assert!(c.reranking_enabled);
        assert_eq!(c.tool_timeout_ms, 30_000);
        // v3.4.0 PenguinHarness 扩展默认值
        assert!(c.progressive_skill_loading);
        assert_eq!(c.max_full_skill_load, 4);
    }

    #[test]
    fn test_generation_control_default() {
        let c = GenerationControlContract::default_contract();
        assert_eq!(c.max_output_tokens, 4096);
        assert!((c.temperature - 0.7).abs() < f32::EPSILON);
        assert!((c.top_p - 0.9).abs() < f32::EPSILON);
        assert!(!c.candidate_sampling);
        // v3.4.0 OpenMLE 扩展默认值
        assert_eq!(c.operator_selection, OperatorSelectionStrategy::ThreeFactor);
        assert!(c.entropy_weighting);
    }

    #[test]
    fn test_task_orchestration_default() {
        let c = TaskOrchestrationContract::default_contract();
        assert_eq!(c.workflow_type, WorkflowType::PlanExecute);
        assert_eq!(c.max_iterations, 10);
        assert_eq!(c.retry_policy.max_attempts, 5);
        // v3.4.0 OpenMLE/RSIBench 扩展默认值
        assert_eq!(c.search_tree_depth, 8);
        assert_eq!(c.budget_hours, 24.0);
        assert_eq!(c.stop_strategy.max_attempts, 5);
        assert!(c.stop_strategy.preserve_best);
    }

    #[test]
    fn test_memory_management_default() {
        let c = MemoryManagementContract::default_contract();
        assert_eq!(c.retention_policy, RetentionPolicy::Adaptive);
        assert_eq!(c.summarization_trigger, 100);
        assert_eq!(c.eviction_strategy, EvictionStrategy::TieredArchive);
        // v3.4.0 OpenMLE/MemoHarness 扩展默认值
        assert!(c.on_demand_synthesis);
        assert_eq!(c.ancestor_retrieval_depth, 2);
        assert_eq!(c.sibling_retrieval_count, 3);
        assert!(c.dual_experience_bank);
        assert!(c.global_experience_board);
    }

    #[test]
    fn test_output_processing_default() {
        let c = OutputProcessingContract::default_contract();
        assert_eq!(c.extraction_format, ExtractionFormat::Markdown);
        assert!(c.validation_rules.is_empty());
        assert_eq!(c.fallback_strategy, FallbackStrategy::Retry);
        // v3.4.0 OpenMLE 扩展默认值（铁律7/8）
        assert!(c.collect_error_signatures);
        assert!(c.track_execution_status);
    }

    #[test]
    fn test_harness_config_default() {
        let c = HarnessConfigContract::default_contract();
        assert_eq!(c.version, "0.1.0");
        assert!(c.hash.is_empty());
        assert_eq!(c.d1_context.max_tokens, 128_000);
        assert_eq!(c.d2_tool.max_tools_per_step, 5);
        assert_eq!(c.d4_orchestration.workflow_type, WorkflowType::PlanExecute);
    }

    // ----------------------------------------------------------
    // serde roundtrip 测试
    // ----------------------------------------------------------

    #[test]
    fn test_context_assembly_serde_roundtrip() {
        let c = ContextAssemblyContract::default_contract();
        let json = serde_json::to_string(&c).expect("序列化失败");
        let back: ContextAssemblyContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, back);
    }

    #[test]
    fn test_tool_interaction_serde_roundtrip() {
        let c = ToolInteractionContract::default_contract();
        let json = serde_json::to_string(&c).expect("序列化失败");
        let back: ToolInteractionContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, back);
    }

    #[test]
    fn test_generation_control_serde_roundtrip() {
        let c = GenerationControlContract::default_contract();
        let json = serde_json::to_string(&c).expect("序列化失败");
        let back: GenerationControlContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, back);
    }

    #[test]
    fn test_task_orchestration_serde_roundtrip() {
        let c = TaskOrchestrationContract::default_contract();
        let json = serde_json::to_string(&c).expect("序列化失败");
        let back: TaskOrchestrationContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, back);
    }

    #[test]
    fn test_memory_management_serde_roundtrip() {
        let c = MemoryManagementContract::default_contract();
        let json = serde_json::to_string(&c).expect("序列化失败");
        let back: MemoryManagementContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, back);
    }

    #[test]
    fn test_output_processing_serde_roundtrip() {
        let c = OutputProcessingContract::default_contract();
        let json = serde_json::to_string(&c).expect("序列化失败");
        let back: OutputProcessingContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, back);
    }

    #[test]
    fn test_harness_config_serde_roundtrip() {
        let c = HarnessConfigContract::default_contract();
        let json = serde_json::to_string(&c).expect("序列化失败");
        let back: HarnessConfigContract = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, back);
    }

    // ----------------------------------------------------------
    // 线格式冻结测试（serde tag / 字段名冻结）
    // ----------------------------------------------------------

    #[test]
    fn test_compression_strategy_json_wire_format_frozen() {
        assert_eq!(
            serde_json::to_string(&CompressionStrategy::None).unwrap(),
            "\"none\""
        );
        assert_eq!(
            serde_json::to_string(&CompressionStrategy::SlidingWindow).unwrap(),
            "\"sliding_window\""
        );
        assert_eq!(
            serde_json::to_string(&CompressionStrategy::Summarization).unwrap(),
            "\"summarization\""
        );
        assert_eq!(
            serde_json::to_string(&CompressionStrategy::SemanticRetrieval).unwrap(),
            "\"semantic_retrieval\""
        );
    }

    #[test]
    fn test_workflow_type_json_wire_format_frozen() {
        assert_eq!(
            serde_json::to_string(&WorkflowType::SingleCall).unwrap(),
            "\"single_call\""
        );
        assert_eq!(
            serde_json::to_string(&WorkflowType::PlanExecute).unwrap(),
            "\"plan_execute\""
        );
        assert_eq!(
            serde_json::to_string(&WorkflowType::MultiAgent).unwrap(),
            "\"multi_agent\""
        );
        // v3.4.0 新增 SearchTree 变体
        assert_eq!(
            serde_json::to_string(&WorkflowType::SearchTree).unwrap(),
            "\"search_tree\""
        );
    }

    #[test]
    fn test_operator_selection_strategy_wire_format_frozen() {
        assert_eq!(
            serde_json::to_string(&OperatorSelectionStrategy::Greedy).unwrap(),
            "\"greedy\""
        );
        assert_eq!(
            serde_json::to_string(&OperatorSelectionStrategy::ThreeFactor).unwrap(),
            "\"three_factor\""
        );
        assert_eq!(
            serde_json::to_string(&OperatorSelectionStrategy::Ucb).unwrap(),
            "\"ucb\""
        );
        assert_eq!(
            serde_json::to_string(&OperatorSelectionStrategy::Cooling).unwrap(),
            "\"cooling\""
        );
        // 默认策略 = 三因子
        assert_eq!(
            OperatorSelectionStrategy::default(),
            OperatorSelectionStrategy::ThreeFactor
        );
    }

    #[test]
    fn test_stop_strategy_config_default() {
        let s = StopStrategyConfig::default();
        assert_eq!(s.max_attempts, 5);
        assert_eq!(s.stagnation_threshold, 3);
        assert!((s.score_gap_threshold - 0.05).abs() < f32::EPSILON);
        assert!(s.preserve_best);
    }

    #[test]
    fn test_old_json_backward_compatible() {
        // 向后兼容: 不含 v3.4.0 新字段的旧 JSON 必须能反序列化（serde(default)）
        let old_d1 = r#"{"max_tokens":128000,"compression_strategy":"semantic_retrieval","example_injection":true,"structured_prompt":true}"#;
        let c: ContextAssemblyContract =
            serde_json::from_str(old_d1).expect("旧 JSON 反序列化失败");
        assert!(c.on_demand_synthesis, "缺失字段应回退默认值 true");
        assert_eq!(c.ancestor_retrieval_depth, 0, "缺失字段应回退默认值 0");

        let old_d4 = r#"{"workflow_type":"plan_execute","max_iterations":10,"retry_policy":{"max_attempts":5,"backoff_ms":1000,"exponential":true}}"#;
        let d: TaskOrchestrationContract =
            serde_json::from_str(old_d4).expect("旧 JSON 反序列化失败");
        assert_eq!(d.search_tree_depth, 0);
        assert_eq!(d.stop_strategy.max_attempts, 5);
        assert!(d.stop_strategy.preserve_best);

        let old_d6 =
            r#"{"extraction_format":"markdown","validation_rules":[],"fallback_strategy":"retry"}"#;
        let o: OutputProcessingContract =
            serde_json::from_str(old_d6).expect("旧 JSON 反序列化失败");
        assert!(o.collect_error_signatures);
        assert!(o.track_execution_status);
    }

    #[test]
    fn test_retention_policy_json_wire_format_frozen() {
        assert_eq!(
            serde_json::to_string(&RetentionPolicy::Session).unwrap(),
            "\"session\""
        );
        assert_eq!(
            serde_json::to_string(&RetentionPolicy::Persistent).unwrap(),
            "\"persistent\""
        );
        assert_eq!(
            serde_json::to_string(&RetentionPolicy::Adaptive).unwrap(),
            "\"adaptive\""
        );
    }

    #[test]
    fn test_eviction_strategy_json_wire_format_frozen() {
        assert_eq!(
            serde_json::to_string(&EvictionStrategy::Lru).unwrap(),
            "\"lru\""
        );
        assert_eq!(
            serde_json::to_string(&EvictionStrategy::TieredArchive).unwrap(),
            "\"tiered_archive\""
        );
    }

    #[test]
    fn test_extraction_format_json_wire_format_frozen() {
        assert_eq!(
            serde_json::to_string(&ExtractionFormat::PlainText).unwrap(),
            "\"plain_text\""
        );
        assert_eq!(
            serde_json::to_string(&ExtractionFormat::Json).unwrap(),
            "\"json\""
        );
        assert_eq!(
            serde_json::to_string(&ExtractionFormat::CodeBlock).unwrap(),
            "\"code_block\""
        );
    }

    #[test]
    fn test_fallback_strategy_json_wire_format_frozen() {
        assert_eq!(
            serde_json::to_string(&FallbackStrategy::Passthrough).unwrap(),
            "\"passthrough\""
        );
        assert_eq!(
            serde_json::to_string(&FallbackStrategy::Retry).unwrap(),
            "\"retry\""
        );
        assert_eq!(
            serde_json::to_string(&FallbackStrategy::HumanReview).unwrap(),
            "\"human_review\""
        );
    }

    // ----------------------------------------------------------
    // 枚举闭集测试：未知变体拒绝
    // ----------------------------------------------------------

    #[test]
    fn test_compression_strategy_rejects_unknown() {
        let err = serde_json::from_str::<CompressionStrategy>("\"unknown\"").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn test_workflow_type_rejects_unknown() {
        let err = serde_json::from_str::<WorkflowType>("\"unknown\"").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    // ----------------------------------------------------------
    // ValidationRule 结构体测试
    // ----------------------------------------------------------

    #[test]
    fn test_validation_rule_serde_roundtrip() {
        let rule = ValidationRule {
            rule_name: "max_length".to_string(),
            rule_params: r#"{"max": 4096}"#.to_string(),
            is_hard_constraint: true,
        };
        let json = serde_json::to_string(&rule).expect("序列化失败");
        let back: ValidationRule = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(rule, back);
    }

    // ----------------------------------------------------------
    // RetryPolicy 复用验证
    // ----------------------------------------------------------

    #[test]
    fn test_retry_policy_reused_from_harness_spec() {
        // 验证 TaskOrchestrationContract 中的 retry_policy 与 harness_spec::RetryPolicy 同类型
        let c = TaskOrchestrationContract::default_contract();
        assert_eq!(c.retry_policy.max_attempts, 5);
        assert_eq!(c.retry_policy.backoff_ms, 1000);
        assert!(c.retry_policy.exponential);
    }
}
