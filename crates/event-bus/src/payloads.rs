//! 事件载荷与辅助类型定义 — NexusEvent 枚举依赖的结构化数据类型
//!
//! 对应架构:L1 Core 事件总线载荷层
//!
//! # 类型分类
//! - **元数据**: [`EventMetadata`]、[`EventSeverity`] — 每个事件携带的通用追踪信息
//! - **载荷结构体**: [`BudgetMetricsPayload`]、[`RouterStatsPayload`]、[`ClvSummary`]
//!   — 事件携带的结构化业务数据
//! - **枚举标签**: [`QuestStatus`]、[`VoteValue`]、[`TaskPriority`]、[`ConsultUrgency`]、
//!   [`AgentStatus`]、[`ActionSource`]、[`ChatStatus`] — 事件变体中的轻量级分类枚举
//! - **回滚诊断**: [`RollbackTriggerType`]、[`RollbackDiagnosticContext`] — ADR-043 回滚决策
//! - **指标**: [`CriticalEventDropped`] — Critical 通道丢弃计数

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================
// 事件元数据与严重级别
// ============================================================

/// 事件元数据 — 每个事件携带,用于追踪、审计与因果排序
///
/// WHY 字段说明:
/// - `event_id`:UUIDv7(时间有序),便于跨进程因果追踪与去重
/// - `timestamp`:单调时钟来源,审计日志按此排序
/// - `source`:发布者 crate 名(如 "osa-coordinator"),用于依赖方向校验
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventMetadata {
    /// 事件唯一标识(UUIDv7,时间有序)
    pub event_id: Uuid,
    /// 事件产生时刻(UTC)
    pub timestamp: DateTime<Utc>,
    /// 发布者 crate 名,用于依赖方向校验与审计
    pub source: String,
}

impl EventMetadata {
    /// 以指定 source 创建元数据,event_id 与 timestamp 自动生成
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            timestamp: Utc::now(),
            source: source.into(),
        }
    }
}

/// 事件严重级别 — 用于背压策略决定是否优先投递
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventSeverity {
    /// 普通事件:可被背压策略丢弃
    Normal,
    /// 信息级事件:控制请求/反馈等,不阻断系统但优先级高于 Normal
    ///
    /// WHY 新增(Task 1):TUI 双向控制事件(Quest 取消/优先级调整)属于
    /// 操作员意图传达,重要性高于普通遥测事件,但非安全关键(不会触发
    /// mpsc 旁路投递)。现有 `== Critical` 判定自动将其视为非关键,
    /// 与"不阻断系统"语义一致,无需改动 backpressure/bus/logging。
    Info,
    /// 关键事件:检查点、共识、安全告警等,不可丢弃
    ///
    /// WHY:CheckpointSaved 等事件丢失会导致 Quest 无法恢复,
    /// 必须标注 Critical 以触发 mpsc 点对点通道或保留优先级
    Critical,
}

// ============================================================
// Quest / 投票辅助枚举
// ============================================================

/// Quest 完成状态 — 用于 `QuestCompleted` 事件(P1.2 实时数据驱动面板)
///
/// WHY 定义在 event-bus 而非 nexus-core:原 nexus-core 仅有 `TaskStatus`,
/// 没有 Quest 级别的结束状态。为不修改核心领域类型(§3.3.1 要求 ADR),
/// 在 event-bus 这一跨层通信契约层定义轻量级状态枚举。
/// 注:此类型属于 P1.2 实时数据面板契约,非 M4 双向控制新增。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QuestStatus {
    /// Quest 成功完成
    Completed,
    /// Quest 执行失败
    Failed,
    /// Quest 被取消
    Cancelled,
}

/// 投票值 — 议会投票的赞成/反对/弃权选项
///
/// WHY 定义在 event-bus:VoteCast 原使用 bool,但控制面板的 :vote 命令
/// 需要显式表达 Abstain,因此在跨层通信契约层定义三值枚举。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum VoteValue {
    /// 赞成
    Yes,
    /// 反对
    No,
    /// 弃权
    Abstain,
}

impl VoteValue {
    /// 返回投票值的小写字符串表示,用于 UI 展示与命令编码。
    pub fn as_str(&self) -> &'static str {
        match self {
            VoteValue::Yes => "yes",
            VoteValue::No => "no",
            VoteValue::Abstain => "abstain",
        }
    }
}

impl std::str::FromStr for VoteValue {
    type Err = ();

    /// 从字符串解析投票值,大小写不敏感。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "yes" => Ok(VoteValue::Yes),
            "no" => Ok(VoteValue::No),
            "abstain" => Ok(VoteValue::Abstain),
            _ => Err(()),
        }
    }
}

// ============================================================
// 结构化载荷
// ============================================================

/// 预算指标载荷 — TUI Budget 面板的结构化数据(P1.2 实时数据驱动面板)
///
/// WHY 定义在 event-bus:chimera-tui(L10)无法直接依赖 efficiency-monitor(L9),
/// 通过 event-bus(L1)传递结构化预算指标,避免面板侧从多个事件拼合。
/// 字段与 `chimera_tui::data::BudgetMetrics` 保持一致。
/// 注:此类型属于 P1.2 实时数据面板契约,非 M4 双向控制新增。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetMetricsPayload {
    /// 总消耗量(单位由预算类型决定)
    pub total_consumption: f64,
    /// 剩余预算
    pub remaining_budget: f64,
    /// 利用率 [0.0, 1.0]
    pub utilization_rate: f32,
    /// 当前预算档位(如 "High"/"Medium"/"Low")
    pub current_tier: String,
    /// 档位系数,1.0 为基准
    pub coefficient: f32,
    /// 是否已触发预算超限
    pub is_exceeded: bool,
    /// 最新告警信息(无告警为 None)
    pub alert: Option<String>,
}

/// 路由器统计载荷 — 三路由器(KVBSR/SESA/FaaE)的统一统计格式
///
/// WHY 定义在 event-bus:chimera-tui(L10)无法直接依赖 L6 的 kvbsr-router/
/// sesa-router/faae-router,通过 event-bus(L1)传递结构化路由统计,
/// 避免面板侧从多个事件拼合,也避免 L10→L6 类型泄漏。
/// 由 L9 efficiency-monitor 聚合三路由器指标后统一发布。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouterStatsPayload {
    /// 命中率 [0.0, 1.0]
    pub hit_rate: f32,
    /// P50 延迟(微秒)
    pub p50_latency_us: u64,
    /// P95 延迟(微秒)
    pub p95_latency_us: u64,
    /// P99 延迟(微秒)
    pub p99_latency_us: u64,
    /// 热点能力列表(能力 ID,调用次数),按热度降序
    pub hot_capabilities: Vec<(String, u64)>,
}

/// CLV 摘要载荷 — TUI ClvVector 面板的结构化数据
///
/// WHY 定义在 event-bus:chimera-tui(L10)需要展示 CLV 摘要,
/// 但不能携带完整 512 维向量(性能负担),通过此摘要结构传递
/// 8 分块均值 + L2 范数 + Top-8 维度索引,足以可视化向量分布。
/// ClvSummary::from_clv 计算方法将在 Task 2 中实现(event-bus 已依赖 nexus-core)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClvSummary {
    /// 8 分块均值(每块 64 维,512/8=64)
    /// 索引 0 = 维度 [0..64),索引 7 = 维度 [448..512)
    pub block_means: Vec<f32>,
    /// L2 范数 = sqrt(sum(v_i^2))
    pub l2_norm: f32,
    /// Top-8 维度(维度索引, 值),按 |值| 降序排列
    /// 长度 ≤ 8(向量维度不足 8 时返回全部)
    pub top_dims: Vec<(usize, f32)>,
}

impl ClvSummary {
    /// 从 CLV 512 维向量计算摘要
    ///
    /// 计算:
    /// 1. **8 分块均值**: 将 512 维分为 8 块(每块 64 维),计算每块算术均值
    /// 2. **L2 范数**: sqrt(sum(v_i^2))
    /// 3. **Top-8 维度**: 按 |v_i| 降序选取前 8 个(维度索引, 值)
    ///
    /// WHY 在 event-bus 实现: ClvSummary 定义在 event-bus(事件载荷),
    /// event-bus 已依赖 nexus-core,可直接访问 CLV::as_slice()。
    /// 避免在 nexus-core 定义 ClvSummary 造成类型重复。
    ///
    /// # 算法选择
    /// - 分块均值: O(n) 遍历,每块 64 维累加后除以 64
    /// - L2 范数: O(n) 遍历,累加 v_i^2 后开方
    /// - Top-8: 使用 `select_nth_unstable_by` O(n) 算法(架构红线要求,
    ///   禁止 sort_by O(n log n) 做 Top-K);Top-k 内部排序(k ≤ 8,成本可忽略)
    ///
    /// # 零向量边界
    /// CLV::zero() 返回 l2_norm = 0.0 + 全 0 block_means + 空 top_dims。
    /// 当 l2_norm == 0 时跳过 Top-8 计算(无显著维度)。
    ///
    /// # 参数
    /// clv: CLV 引用(nexus-core 的 512 维向量)
    ///
    /// # 返回
    /// ClvSummary 实例
    pub fn from_clv(clv: &nexus_core::clv::CLV) -> Self {
        let slice = clv.as_slice();
        let dim = nexus_core::clv::CLV::DIMENSION; // 512

        // 1. 计算 8 分块均值(每块 64 维)
        let block_size = dim / 8; // 64
        let block_means: Vec<f32> = (0..8)
            .map(|i| {
                let start = i * block_size;
                let end = start + block_size;
                let sum: f32 = slice[start..end].iter().sum();
                sum / block_size as f32
            })
            .collect();

        // 2. 计算 L2 范数 = sqrt(sum(v_i^2))
        let sum_sq: f32 = slice.iter().map(|&v| v * v).sum();
        let l2_norm = sum_sq.sqrt();

        // 3. 计算 Top-8 维度(按 |值| 降序)
        // 零向量边界:l2_norm == 0 表示全零,无显著维度,返回空
        let top_dims = if l2_norm == 0.0 {
            Vec::new()
        } else {
            // 构造 (维度索引, |值|, 值) 元组向量
            let mut indexed: Vec<(usize, f32, f32)> = slice
                .iter()
                .enumerate()
                .map(|(i, &v)| (i, v.abs(), v))
                .collect();

            let k = 8.min(indexed.len());
            if k == 0 {
                Vec::new()
            } else {
                // select_nth_unstable_by 按 |值| 降序分区到第 k-1 位:
                // before(k-1 个最大) + mid(第 k 个) = Top-k(无序)
                let (before, mid, _) = indexed.select_nth_unstable_by(k - 1, |a, b| {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                });

                // 收集 Top-k:前 k-1 个 + mid
                let mut top: Vec<(usize, f32, f32)> = before.to_vec();
                top.push(*mid);

                // Top-k 内部按 |值| 降序排序(k ≤ 8,排序成本可忽略)
                top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                // 转换为 (维度索引, 值) 格式
                top.into_iter().take(8).map(|(i, _abs, v)| (i, v)).collect()
            }
        };

        ClvSummary {
            block_means,
            l2_norm,
            top_dims,
        }
    }
}

// ============================================================
// CHIMERA-MAS Agent 辅助类型(ADR-026,Task 4)
// ============================================================

/// 任务优先级 — Agent 任务委派(AgentTaskDelegated)的调度优先级
///
/// WHY 独立定义在 event-bus(L1)而非 chimera-mas(L9):
/// §2.2 依赖铁律禁止 L1→L9 向上依赖。chimera-mas(L9)发布
/// AgentTaskDelegated 事件时需要此类型作为 payload 字段。若将
/// TaskPriority 定义在 chimera-mas,event-bus 无法引用(会触发
/// L1→L9 违规)。将轻量级枚举下沉到 event-bus(L1),chimera-mas
/// 通过向下依赖 event-bus 复用,符合依赖方向。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TaskPriority {
    /// 低优先级,空闲时调度
    Low,
    /// 中等优先级,正常调度队列
    Medium,
    /// 高优先级,优先调度
    High,
    /// 最高优先级,立即调度(可能抢占低优先级任务)
    Critical,
}

/// 咨询紧急度 — Agent 咨询请求(AgentConsultRequested)的紧急级别
///
/// WHY 独立定义在 event-bus:同 TaskPriority,避免 L1→L9 向上依赖。
/// 用于 Agent 间咨询请求的优先级标注,影响被咨询 Agent 的响应顺序。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ConsultUrgency {
    /// 低紧急度
    Low,
    /// 中等紧急度
    Medium,
    /// 高紧急度
    High,
    /// 最高紧急度(立即响应)
    Critical,
}

/// Agent 生命周期状态 — AgentHeartbeat 事件携带的 Agent 运行时状态
///
/// WHY 独立定义在 event-bus:同 TaskPriority,避免 L1→L9 向上依赖。
/// 变体语义与 chimera-mas::AgentStatus 保持一致(Idle/Running/Paused/
/// Completed/Failed/Crashed),但为独立类型定义,避免 event-bus 对
/// chimera-mas 的循环依赖。chimera-mas 在发布心跳事件时通过
/// `From<chimera_mas::AgentStatus> for event_bus::AgentStatus` 转换。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AgentStatus {
    /// 空闲状态,等待任务分配
    Idle,
    /// 运行中,正在执行任务
    Running,
    /// 已暂停,可恢复
    Paused,
    /// 任务已完成
    Completed,
    /// 任务执行失败
    Failed,
    /// Agent 崩溃,不可恢复
    Crashed,
}

/// TUI 交互动作来源 — `TuiActionRequested` 事件的触发入口标识(ADR-029)
///
/// WHY 独立定义在 event-bus:TUI 交互协议(Action)是 L10 与编排层
/// (chimera-cli)之间经 L1 EventBus 通信的契约,来源标识用于审计与
/// UI 反馈定位(区分同一 Action 由哪个入口触发)。三入口共享同一
/// Action 协议,行为一致性由 chimera-tui 的 ActionRegistry 单源保证。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ActionSource {
    /// 来自 Chat 面板的斜杠命令
    Chat,
    /// 来自命令面板(模糊搜索)
    Palette,
    /// 来自面板上下文动作(焦点面板 Enter/Space 唤出)
    Panel,
}

/// TUI 交互式对话状态 — `TuiChatStatusChanged` 事件携带的 Agent 会话状态(ADR-029)
///
/// WHY 独立定义在 event-bus:与 `AgentStatus`(chimera-mas 多 Agent 生命周期)
/// 区分——`ChatStatus` 面向单条交互式会话的 UI 呈现(思考中/工具执行中/空闲),
/// 由 chimera-cli 编排器在流式回答过程中广播,驱动 Chat 面板状态指示器。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum ChatStatus {
    /// 思考中(已提交查询,等待/生成首 token)
    Thinking,
    /// 工具执行中(Agent 调用工具,暂停 token 流)
    ToolExecuting,
    /// 空闲(本轮完成,等待下一次输入)
    ///
    /// WHY `#[default]`:`DataSnapshot`/`TuiState` 派生 `Default`,对话初始态
    /// 天然为"空闲"(尚未提交任何查询)。
    #[default]
    Idle,
}

// ============================================================
// R1 影子模式回滚辅助类型(ADR-043 决策 4)
// ============================================================

/// R1 影子模式回滚触发条件类型(ADR-043 决策 4)— P2-13 结构化理由记录
///
/// 对应 ADR-043 决策 4 定义的 4 种回滚触发条件 + Unknown 兜底。
///
/// WHY 枚举而非 String:结构化记录便于审计图表按触发条件分类统计,
/// 同时支持 efficiency-monitor 告警规则精确匹配(避免字符串模糊匹配的歧义)。
/// 旧版 `R1ShadowRollbackFailed.reason: String` 保留为人类可读描述,
/// `trigger_type` 字段提供机器可读的分类标签。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackTriggerType {
    /// 连续 3 天 R1 显著差于 L3(决策 4 触发条件 1)
    ///
    /// 对应 `ShadowComparisonReport::ComparisonResult::R1SignificantlyWorse`
    /// 连续出现 3 天的退化模式。
    ConsecutiveRegression,

    /// AsaIntervention 触发(决策 4 触发条件 2)
    ///
    /// R1 接缝触发 AsaIntervention 事件,ADR-037 决策 3 自动置为 Cooldown。
    AsaIntervention,

    /// EWMA 崩塌:24h 内下降 ≥ 0.3(决策 4 触发条件 3)
    ///
    /// 如从 0.7 跌至 0.4 以下,表明 R1 训练策略严重退化。
    EwmaCollapse,

    /// 召回率显著下降:较 L3 基线下降 ≥ 5%(决策 4 触发条件 4)
    ///
    /// 绝对值下降,如从 95% 降至 90%。
    RecallRateDrop,

    /// 未知触发条件(兜底)
    ///
    /// 用于未分类的回滚失败,如 CapabilityTokenRegistry 内部错误导致的
    /// 回滚失败(不属于 4 种退化模式,但回滚操作本身失败)。
    Unknown,
}

impl Default for RollbackTriggerType {
    /// 默认值 = Unknown
    ///
    /// WHY Unknown 而非 ConsecutiveRegression:确保未显式设置 trigger_type 的
    /// 旧版本事件反序列化后不会误归类为某一具体触发条件。
    fn default() -> Self {
        Self::Unknown
    }
}

impl RollbackTriggerType {
    /// 获取触发条件的人类可读描述(用于日志与 TUI 展示)
    pub fn description(&self) -> &'static str {
        match self {
            Self::ConsecutiveRegression => "R1 significantly worse than L3 for 3 consecutive days",
            Self::AsaIntervention => "ASA intervention triggered on R1 seam",
            Self::EwmaCollapse => "EWMA collapsed by >=0.3 within 24h",
            Self::RecallRateDrop => "Recall rate dropped >=5% vs L3 baseline",
            Self::Unknown => "Unknown rollback trigger",
        }
    }
}

/// R1 影子模式回滚诊断上下文(ADR-043 决策 4)— P2-13 结构化理由记录
///
/// 承载回滚失败时的诊断快照,便于专家团队复盘根因。
///
/// # 设计原则
/// - **所有字段 Option**:不同触发条件(`RollbackTriggerType`)只有部分字段有意义。
///   如 `ConsecutiveRegression` 触发时 `regression_streak` 有值但 `recall_rate_drop` 无意义。
/// - **`#[serde(default)]` 全字段**:确保旧版本序列化的事件(无此结构体)能被
///   反序列化为全 None 的默认值,保证向后兼容。
/// - **纯数据类型**:无方法,仅承载快照,便于序列化与持久化到审计日志。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RollbackDiagnosticContext {
    /// 触发回滚时的 EWMA 水平(None 表示未测量)
    ///
    /// 对应 `CapabilityToken.level` 字段快照。`EwmaCollapse` 触发条件必填。
    #[serde(default)]
    pub ewma_level: Option<f32>,

    /// 观察期已进行天数(None 表示未进入观察期)
    ///
    /// 范围 [0, 14],14 天为 ADR-043 决策 3 定义的完整观察期。
    #[serde(default)]
    pub observation_days: Option<u32>,

    /// 连续退化天数(None 表示非 ConsecutiveRegression 触发)
    ///
    /// `ConsecutiveRegression` 触发条件必填,值为 `regression_streak`(≥3 满足触发)。
    #[serde(default)]
    pub regression_streak: Option<u32>,

    /// 召回率下降幅度(绝对值,None 表示非 RecallRateDrop 触发)
    ///
    /// 如 0.05 表示下降 5%。`RecallRateDrop` 触发条件必填,值 ≥ 0.05。
    #[serde(default)]
    pub recall_rate_drop: Option<f32>,

    /// 回滚目标版本号(None 表示无版本号)
    ///
    /// R1 训练策略的版本号,用于追踪具体哪个版本的策略导致退化。
    #[serde(default)]
    pub rollback_target_version: Option<u32>,
}

impl RollbackDiagnosticContext {
    /// 创建空的诊断上下文(所有字段 None)
    ///
    /// 用于触发条件未知或不需要诊断上下文的场景。
    pub fn empty() -> Self {
        Self::default()
    }

    /// builder 模式:设置 EWMA 水平
    pub fn with_ewma_level(mut self, level: f32) -> Self {
        self.ewma_level = Some(level);
        self
    }

    /// builder 模式:设置观察期天数
    pub fn with_observation_days(mut self, days: u32) -> Self {
        self.observation_days = Some(days);
        self
    }

    /// builder 模式:设置连续退化天数
    pub fn with_regression_streak(mut self, streak: u32) -> Self {
        self.regression_streak = Some(streak);
        self
    }

    /// builder 模式:设置召回率下降幅度
    pub fn with_recall_rate_drop(mut self, drop: f32) -> Self {
        self.recall_rate_drop = Some(drop);
        self
    }

    /// builder 模式:设置回滚目标版本号
    pub fn with_rollback_target_version(mut self, version: u32) -> Self {
        self.rollback_target_version = Some(version);
        self
    }
}

// ============================================================
// Critical 通道指标
// ============================================================

/// Critical 通道丢弃事件指标载荷 — P1-W2.1(D3 改造)
///
/// 当 Critical 通道(有界 mpsc::Sender<4096>)因消费者跟不上而满载时,
/// `publish_critical` 内部 `try_send` 失败会丢弃事件并递增计数。此结构体
/// 作为指标载荷,供 efficiency-monitor 拉取并发布 `EfficiencyAlertTriggered`
/// 告警,同时供 TUI 显示丢弃计数(spec.md L188:丢弃事件计入
/// CriticalEventDropped 指标 + TUI 告警)。
///
/// # 设计约束
/// - 轻量级(u64 单字段),实现 Copy 以便廉价传递
/// - 不持有 Mutex(计数由 EventBus 内部 Arc<AtomicU64> 维护,
///   此结构体仅为快照,避免 §4.4 红线 1 "持锁跨 await")
/// - 实现 Serialize/Deserialize 以支持跨进程指标传递(MCP Mesh)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriticalEventDropped {
    /// 累计丢弃的 Critical 事件数(单调递增,不重置)
    dropped_count: u64,
}

impl CriticalEventDropped {
    /// 创建指标载荷快照
    ///
    /// `count` 通常来自 `EventBus::critical_dropped_count()` 的当前快照值。
    pub fn new(dropped_count: u64) -> Self {
        Self { dropped_count }
    }

    /// 获取累计丢弃的 Critical 事件数
    pub fn dropped_count(&self) -> u64 {
        self.dropped_count
    }
}
