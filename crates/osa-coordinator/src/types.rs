//! OSA 核心领域类型 — 全维稀疏协调器的统一数据模型
//!
//! 对应架构层:L6 Router
//! 对应创新点:OSA / Ω-Sparse(Omni-Sparse Architecture)
//!
//! # 类型职责
//! - `ToolId`/`FileId`/`MemoryId`/`OperationId`/`TaskId`:五维度稀疏化的标识类型
//! - `TaskProfile`:任务特征快照,作为掩码计算的输入
//! - `RiskLevel`/`TaskType`/`TimePressure`/`AffectedScope`:任务特征的枚举维度
//! - `ComplexityBand`:复杂度档位,驱动联动稀疏化策略
//!
//! # 设计决策(WHY)
//! - **标识类型为 String 别名**:便于与 EventBus 事件中的 ID 字段直接交互,
//!   避免额外转换开销。UUIDv7 生成由调用方负责
//! - **ComplexityBand 四档**:与架构手册 §复杂度联动稀疏化 对齐,
//!   < 0.25 / 0.25-0.5 / 0.5-0.75 / ≥ 0.75 对应 Simple/Regular/Complex/UltraComplex
//! - **TaskProfile 携带五维度候选集**:available_tools/files/memories/operations/tasks,
//!   OSA 据此选取 Top-K 生成稀疏掩码,无需访问外部存储

use serde::{Deserialize, Serialize};

// 五维度 ID 类型从 L0 nexus-contracts 统一导入(ADR-033, P2-W5.2)
// WHY:原用 nexus_core::id_newtype! 宏在本地展开生成独立名义类型,
// 导致 osa_coordinator::ToolId ≠ kvbsr_router::ToolId ≠ faae_router::ToolId。
// 改为 re-export nexus_contracts 类型后,跨 crate 共享同一类型,
// 消除星型耦合(Insight 2 消解),mask_hash 计算逻辑仍保留在本 crate(L6 依赖 sha2/hex)。
pub use nexus_contracts::{FileId, MemoryId, OperationId, TaskId, ToolId};

// Task 2: 从 L0 nexus-contracts 导入 MemoryTaskPhase(OSA memory 维度 S2 桥接)
// WHY: TaskProfile 携带 task_phase 供 compute_memory_mask 查询 S2 策略,
// 类型定义在 L0 避免 OSA 直接依赖 omega-learner (L6),依赖铁律 §2.2 合规
pub use nexus_contracts::MemoryTaskPhase;

/// 风险等级 — 影响审计采样率与预算保护策略
///
/// WHY:高风险任务需更密集审计(全审计)与更严格预算保护,
/// OSA 据此调整 audit_mask 与 budget_mask 的稀疏度
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RiskLevel {
    /// 低风险:只读操作、纯计算,audit 采样 10%
    Low,
    /// 中风险:常规读写,audit 采样 30%
    Medium,
    /// 高风险:外部调用、文件修改,audit 采样 70%
    High,
    /// 极高风险:特权操作、网络请求,audit 全审计
    Critical,
}

impl RiskLevel {
    /// 返回风险等级索引(0-3),用于索引配置数组
    pub fn as_index(&self) -> usize {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }

    /// 返回风险等级名称(用于事件 payload 与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }
}

/// 任务类型 — 影响 context 维度的稀疏化策略
///
/// WHY:不同任务类型对上下文的需求不同(如 Execute 需要更多文件上下文),
/// OSA 据此调整 context_mask 的 Top-K
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TaskType {
    /// 只读任务:查询、分析,context 需求较小
    Read,
    /// 写入任务:编辑、创建,context 需求中等
    Write,
    /// 执行任务:运行命令、测试,context 需求较大
    Execute,
    /// 分析任务:推理、规划,context 需求最大
    Analyze,
    /// 生成任务:代码生成、文档生成,context 需求最大
    Generate,
}

/// 时间压力 — 影响 budget 维度的稀疏化策略
///
/// WHY:高时间压力下需保留更多活跃任务以并行执行,
/// OSA 据此调整 budget_mask 的保护比例
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TimePressure {
    /// 低压力:无紧急截止,budget 保护比例默认
    Low,
    /// 中压力:近期截止,budget 保护比例略降
    Medium,
    /// 高压力:即将截止,budget 保护比例降低
    High,
    /// 极高压力:已超时,budget 保护比例最低
    Critical,
}

/// 影响范围 — 影响 context 维度的稀疏化策略
///
/// WHY:影响范围越广,需加载更多上下文文件,
/// OSA 据此调整 context_mask 的 Top-K
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AffectedScope {
    /// 局部影响:仅当前函数/模块
    Local,
    /// 模块影响:当前模块及其直接依赖
    Module,
    /// 系统影响:跨模块、跨子系统
    System,
    /// 全局影响:全项目、跨项目
    Global,
}

/// 复杂度档位 — 驱动联动稀疏化策略的四档分级
///
/// 对应架构手册 §复杂度联动稀疏化:
/// - `Simple`(< 0.25):routing Top-8,context 1 文件,audit 10%
/// - `Regular`(0.25-0.5):routing Top-16,context 10 文件,audit 50%
/// - `Complex`(0.5-0.75):routing Top-24,context 100 文件,audit 100%
/// - `UltraComplex`(≥ 0.75):routing Top-32,context 1000 文件,audit 100% + 告警
///
/// WHY:复杂度越高,稀疏度越低(保留更多活跃项),
/// 因为复杂任务需要更多工具、上下文与审计覆盖
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ComplexityBand {
    /// 简单任务(complexity < 0.25):最小稀疏化配置
    Simple,
    /// 常规任务(0.25 ≤ complexity < 0.5):标准稀疏化配置
    Regular,
    /// 复杂任务(0.5 ≤ complexity < 0.75):增强稀疏化配置
    Complex,
    /// 超复杂任务(complexity ≥ 0.75):最大稀疏化配置 + 实时告警
    UltraComplex,
}

impl ComplexityBand {
    /// 根据复杂度分数判定档位(使用默认阈值 0.25/0.5/0.75)
    ///
    /// 阈值:0.25 / 0.5 / 0.75(对应架构手册四档分级)
    /// 边界处理:分数 < 0.0 归为 Simple,≥ 1.0 归为 UltraComplex
    ///
    /// WHY:SubTask 14.4 — 此方法保留默认阈值,向后兼容。
    /// 需要自定义阈值时使用 `from_complexity_with_thresholds`
    pub fn from_complexity(score: f32) -> Self {
        Self::from_complexity_with_thresholds(score, (0.25, 0.5, 0.75))
    }

    /// 根据复杂度分数与自定义阈值判定档位(SubTask 14.4)
    ///
    /// 阈值 (t1, t2, t3) 将 [0.0, 1.0] 分为四档:
    /// - `[0.0, t1)`:Simple
    /// - `[t1, t2)`:Regular
    /// - `[t2, t3)`:Complex
    /// - `[t3, 1.0]`:UltraComplex
    ///
    /// WHY:阈值从 `OsaConfig.complexity_thresholds` 传入,支持配置化调优。
    /// 调用方负责确保阈值满足 0 < t1 < t2 < t3 < 1.0(由 `OsaConfig::validate` 校验)
    pub fn from_complexity_with_thresholds(score: f32, thresholds: (f32, f32, f32)) -> Self {
        let (t1, t2, t3) = thresholds;
        if score < t1 {
            Self::Simple
        } else if score < t2 {
            Self::Regular
        } else if score < t3 {
            Self::Complex
        } else {
            Self::UltraComplex
        }
    }

    /// 返回档位索引(0-3),用于索引配置数组
    pub fn as_index(&self) -> usize {
        match self {
            Self::Simple => 0,
            Self::Regular => 1,
            Self::Complex => 2,
            Self::UltraComplex => 3,
        }
    }

    /// 返回档位名称(用于事件 payload 与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Simple => "Simple",
            Self::Regular => "Regular",
            Self::Complex => "Complex",
            Self::UltraComplex => "UltraComplex",
        }
    }
}

/// 任务特征快照 — OSA 掩码计算的统一输入
///
/// 携带五维度候选集与任务元信息,OSA 据此一次性计算全维掩码。
/// 所有字段在创建时填充,不可变(无需内部可变性)。
///
/// # 字段分组
/// - **元信息**:task_id / task_type / complexity_score / risk_level / time_pressure / affected_scope
/// - **五维度候选集**:available_tools / available_files / available_memories /
///   recent_operations / active_tasks
/// - **五维度评分**(可选):routing_scores / context_scores / memory_scores
///
/// # 评分字段语义与 None fallback 行为
///
/// `routing_scores` / `context_scores` / `memory_scores` 为可选评分向量,
/// 分别与 `available_tools` / `available_files` / `available_memories` 一一对应
/// (长度必须一致,否则 OSA 会安全降级为空掩码)。
///
/// - **Some(vec)**:OSA 用这些分数做 Top-K 选择,实现基于真实相关性的稀疏化
/// - **None**:OSA fallback 到 `heuristic_scores`(索引负相关评分,退化为"前 K 个"),
///   保持与旧签名相同的行为,确保向后兼容
///
/// WHY:原 `heuristic_scores` 用 `1.0 - i/len` 使 Top-K 退化为"前 K 个",
/// 无法基于真实相关性选择。新增评分字段让上游(NMC 编码或 Quest Engine)
/// 可注入语义相关性分数,实现真正的 Top-K。`Option` 包装区分"未提供"与"空向量"。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskProfile {
    /// 任务唯一标识
    pub task_id: TaskId,
    /// 任务类型(影响 context 维度稀疏化)
    pub task_type: TaskType,
    /// 复杂度分数 [0.0, 1.0],驱动联动稀疏化四档分级
    ///
    /// WHY:由上游(NMC 编码或 Quest Engine)计算,OSA 据此判定 ComplexityBand
    pub complexity_score: f32,
    /// 风险等级(影响 audit 与 budget 维度稀疏化)
    pub risk_level: RiskLevel,
    /// 时间压力(影响 budget 维度稀疏化)
    pub time_pressure: TimePressure,
    /// 影响范围(影响 context 维度稀疏化)
    pub affected_scope: AffectedScope,
    /// 可用工具列表(routing 维度候选集,OSA 选 Top-K)
    pub available_tools: Vec<ToolId>,
    /// 可用文件列表(context 维度候选集,OSA 选 Top-K)
    pub available_files: Vec<FileId>,
    /// 可用记忆列表(memory 维度候选集,OSA 选 Top-K)
    pub available_memories: Vec<MemoryId>,
    /// 近期操作列表(audit 维度候选集,OSA 按采样率选取)
    pub recent_operations: Vec<OperationId>,
    /// 活跃任务列表(budget 维度候选集,OSA 按保护比例选取)
    pub active_tasks: Vec<TaskId>,
    /// routing 维度评分(与 available_tools 一一对应,可选)
    ///
    /// WHY:Some 时用真实评分做 Top-K,None 时 fallback 到 heuristic_scores。
    /// `#[serde(default)]` 保证旧数据(无此字段)反序列化为 None,向后兼容
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_scores: Option<Vec<f32>>,
    /// context 维度评分(与 available_files 一一对应,可选)
    ///
    /// WHY:Some 时用真实评分做 Top-K,None 时 fallback 到 heuristic_scores。
    /// `#[serde(default)]` 保证旧数据(无此字段)反序列化为 None,向后兼容
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_scores: Option<Vec<f32>>,
    /// memory 维度评分(与 available_memories 一一对应,可选)
    ///
    /// WHY:Some 时用真实评分做 Top-K,None 时 fallback 到 heuristic_scores。
    /// `#[serde(default)]` 保证旧数据(无此字段)反序列化为 None,向后兼容
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_scores: Option<Vec<f32>>,
    /// 任务阶段(Task 2:OSA memory 维度 S2 自适应记忆策略输入)
    ///
    /// WHY: OSA memory 维度通过 `MemoryStrategyProvider` 根据 task_phase 选择
    /// 记忆策略(MinimalRecall/StandardTopK/QueryReformulation/AggressivePruning/TimeFocused),
    /// 再用策略的 `k_multiplier()` 调整基础 Top-K,实现记忆策略随任务阶段自适应。
    /// None 时 OSA fallback 到 `MemoryTaskPhase::default()`(Initial),保守召回。
    /// `#[serde(default)]` 保证旧数据(无此字段)反序列化为 None,向后兼容
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_phase: Option<MemoryTaskPhase>,
}

impl TaskProfile {
    /// 创建新任务特征快照
    ///
    /// # 参数
    /// - `task_id`:任务唯一标识(接受 `TaskId`/`String`/`&str`,通过 `Into<TaskId>` 转换)
    /// - `complexity_score`:复杂度分数 [0.0, 1.0]
    /// - `risk_level`:风险等级
    #[allow(clippy::too_many_arguments)]
    pub fn new(task_id: impl Into<TaskId>, complexity_score: f32, risk_level: RiskLevel) -> Self {
        Self {
            task_id: task_id.into(),
            task_type: TaskType::Read,
            complexity_score,
            risk_level,
            time_pressure: TimePressure::Low,
            affected_scope: AffectedScope::Local,
            available_tools: Vec::new(),
            available_files: Vec::new(),
            available_memories: Vec::new(),
            recent_operations: Vec::new(),
            active_tasks: Vec::new(),
            // 评分字段默认 None:调用方未提供时 OSA fallback 到 heuristic_scores
            routing_scores: None,
            context_scores: None,
            memory_scores: None,
            // Task 2: task_phase 默认 None,调用方未提供时 OSA fallback 到 Initial
            task_phase: None,
        }
    }

    /// 获取复杂度档位(基于 complexity_score 判定,使用默认阈值)
    ///
    /// WHY:SubTask 14.4 — 保留默认阈值版本,向后兼容。
    /// 需要自定义阈值时使用 `complexity_band_with_thresholds`
    pub fn complexity_band(&self) -> ComplexityBand {
        ComplexityBand::from_complexity(self.complexity_score)
    }

    /// 获取复杂度档位(基于自定义阈值判定,SubTask 14.4)
    ///
    /// 阈值从 `OsaConfig.complexity_thresholds` 传入,支持配置化调优
    pub fn complexity_band_with_thresholds(&self, thresholds: (f32, f32, f32)) -> ComplexityBand {
        ComplexityBand::from_complexity_with_thresholds(self.complexity_score, thresholds)
    }

    /// 计算稀疏度 [0.0, 1.0]
    ///
    /// 公式:`sparsity = 1.0 - complexity_score`
    /// 复杂度越高,稀疏度越低(保留更多活跃项)
    pub fn sparsity(&self) -> f32 {
        1.0 - self.complexity_score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level_as_index() {
        assert_eq!(RiskLevel::Low.as_index(), 0);
        assert_eq!(RiskLevel::Medium.as_index(), 1);
        assert_eq!(RiskLevel::High.as_index(), 2);
        assert_eq!(RiskLevel::Critical.as_index(), 3);
    }

    #[test]
    fn test_complexity_band_thresholds() {
        assert_eq!(ComplexityBand::from_complexity(0.0), ComplexityBand::Simple);
        assert_eq!(
            ComplexityBand::from_complexity(0.24),
            ComplexityBand::Simple
        );
        assert_eq!(
            ComplexityBand::from_complexity(0.25),
            ComplexityBand::Regular
        );
        assert_eq!(
            ComplexityBand::from_complexity(0.49),
            ComplexityBand::Regular
        );
        assert_eq!(
            ComplexityBand::from_complexity(0.5),
            ComplexityBand::Complex
        );
        assert_eq!(
            ComplexityBand::from_complexity(0.74),
            ComplexityBand::Complex
        );
        assert_eq!(
            ComplexityBand::from_complexity(0.75),
            ComplexityBand::UltraComplex
        );
        assert_eq!(
            ComplexityBand::from_complexity(1.0),
            ComplexityBand::UltraComplex
        );
    }

    #[test]
    fn test_task_profile_sparsity() {
        let profile = TaskProfile::new("t-1", 0.3, RiskLevel::Low);
        assert!((profile.sparsity() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_task_profile_complexity_band() {
        let profile = TaskProfile::new("t-1", 0.6, RiskLevel::Medium);
        assert_eq!(profile.complexity_band(), ComplexityBand::Complex);
    }

    /// SubTask 14.4:验证自定义阈值产生不同档位
    #[test]
    fn test_from_complexity_with_custom_thresholds() {
        // 自定义阈值 (0.3, 0.6, 0.9)
        let thresholds = (0.3, 0.6, 0.9);
        // [0.0, 0.3) → Simple
        assert_eq!(
            ComplexityBand::from_complexity_with_thresholds(0.2, thresholds),
            ComplexityBand::Simple
        );
        // [0.3, 0.6) → Regular
        assert_eq!(
            ComplexityBand::from_complexity_with_thresholds(0.4, thresholds),
            ComplexityBand::Regular
        );
        // [0.6, 0.9) → Complex
        assert_eq!(
            ComplexityBand::from_complexity_with_thresholds(0.7, thresholds),
            ComplexityBand::Complex
        );
        // [0.9, 1.0] → UltraComplex
        assert_eq!(
            ComplexityBand::from_complexity_with_thresholds(0.95, thresholds),
            ComplexityBand::UltraComplex
        );
    }

    /// SubTask 14.4:验证自定义阈值与默认阈值在边界值产生不同档位
    #[test]
    fn test_custom_thresholds_differ_from_default() {
        // 0.26:默认阈值 → Regular(≥0.25),自定义阈值 (0.3, 0.6, 0.9) → Simple(<0.3)
        assert_eq!(
            ComplexityBand::from_complexity(0.26),
            ComplexityBand::Regular
        );
        assert_eq!(
            ComplexityBand::from_complexity_with_thresholds(0.26, (0.3, 0.6, 0.9)),
            ComplexityBand::Simple
        );
    }

    /// SubTask 14.4:验证 TaskProfile::complexity_band_with_thresholds
    #[test]
    fn test_task_profile_complexity_band_with_thresholds() {
        let profile = TaskProfile::new("t-1", 0.4, RiskLevel::Low);
        // 默认阈值:0.4 ∈ [0.25, 0.5) → Regular
        assert_eq!(profile.complexity_band(), ComplexityBand::Regular);
        // 自定义阈值 (0.3, 0.6, 0.9):0.4 ∈ [0.3, 0.6) → Regular(恰好相同)
        assert_eq!(
            profile.complexity_band_with_thresholds((0.3, 0.6, 0.9)),
            ComplexityBand::Regular
        );
        // 自定义阈值 (0.5, 0.7, 0.9):0.4 < 0.5 → Simple
        assert_eq!(
            profile.complexity_band_with_thresholds((0.5, 0.7, 0.9)),
            ComplexityBand::Simple
        );
    }
}
