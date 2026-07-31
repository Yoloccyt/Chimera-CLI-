//! 分层子枚举 — NexusEvent 按架构层拆分的分类实现
//!
//! WHY:将 88+ 变体的 NexusEvent 巨型枚举按架构层拆分为 8 个子枚举,
//! 每个子枚举实现 `EventClassification` trait(metadata/severity/type_name)。
//! NexusEvent 的方法委托给子枚举,将单个 300 行 match 拆分为 8 个 30-80 行 match,
//! 显著改善可维护性。
//!
//! # 渐进式方案(零破坏性)
//! NexusEvent 的变体结构保持不变(消费方代码零改动),子枚举仅作为
//! 方法委托目标。未来可逐步迁移消费方使用 `NexusEvent::Core(CoreEvent::X)`
//! 形式,最终将 NexusEvent 变为纯 wrapper enum。
//!
//! # 分组映射
//! - `CoreEvent`(5):L1 核心基础设施事件
//! - `MemoryEvent`(9):L2 记忆系统事件(MLC/HCW/NMC/CLV)
//! - `StorageEvent`(6):L3 存储/缓存事件(SCC/CMT/LSCT/DECB)
//! - `SecurityEvent`(7):L4 安全事件(Sandbox/Freeze/Budget/ASA/RedTeam)
//! - `RouterEvent`(13):L6 路由事件(OSA/KVBSR/FaaE/SESA/GEA/EDSB)
//! - `ExecutionEvent`(9):L7 执行事件(PVL/GQEP/MTPE/SSRA)
//! - `QuestEvent`(22):L8-L9 任务/议会事件(Quest/Checkpoint/Consensus/Parliament)
//! - `InterfaceEvent`(17):L10 界面/TUI/Agent/MCP/监控事件

use chrono::{DateTime, Utc};
use nexus_core::Quest;

use crate::types::{
    ActionSource, AgentStatus, BudgetMetricsPayload, ChatStatus, ClvSummary, ConsultUrgency,
    EventMetadata, EventSeverity, QuestStatus, RouterStatsPayload, TaskPriority, VoteValue,
};

// ============================================================
// EventClassification trait — 子枚举分类契约
// ============================================================

/// 事件分类 trait — 每个子枚举实现,提供元数据/严重级别/类型名
///
/// WHY:NexusEvent 的 metadata()/severity()/type_name() 方法通过此 trait
/// 委托给子枚举,将单个巨型 match 拆分为 8 个小型 match。
pub trait EventClassification {
    /// 获取事件元数据引用
    fn metadata(&self) -> &EventMetadata;
    /// 事件严重级别
    fn severity(&self) -> EventSeverity;
    /// 事件类型名(用于日志与序列化 tag)
    fn type_name(&self) -> &'static str;
}

// ============================================================
// CoreEvent — L1 核心基础设施事件(5 变体)
// ============================================================

/// L1 核心基础设施事件
///
/// 包含全局状态变更、模型路由、用户意图编码、背压告警、审计日志。
/// 这些事件属于系统核心层,不归属于特定领域(记忆/存储/安全等)。
///
/// WHY `#[allow(missing_docs)]`:子枚举字段文档已在 NexusEvent 对应变体上
/// 完整记录,此处不重复以避免维护两份相同文档。
#[allow(missing_docs)]
pub enum CoreEvent {
    /// NexusState 发生变更,MLC 需同步记忆快照
    NexusStateChanged {
        metadata: EventMetadata,
        state_hash: String,
        prev_hash: String,
    },
    /// Model Router 选定执行模型,Quest 据此调度
    ModelRouteSelected {
        metadata: EventMetadata,
        quest_id: String,
        model_id: String,
        route_reason: String,
    },
    /// NMC 编码用户意图完成,Quest Engine 据此分解任务
    UserIntentEncoded {
        metadata: EventMetadata,
        intent_id: String,
        raw_text: String,
        risk_level: u8,
    },
    /// 慢消费者被丢弃 `[Critical]` — 系统健康告警
    SlowConsumerDropped {
        metadata: EventMetadata,
        subscriber_id: String,
        lag: u64,
        dropped_count: u64,
    },
    /// 审计日志已记录,SecCore 据此做合规检查
    AuditLogged {
        metadata: EventMetadata,
        audit_hash: String,
        severity: String,
    },
}

// ============================================================
// MemoryEvent — L2 记忆系统事件(9 变体)
// ============================================================

/// L2 记忆系统事件
///
/// 包含 MLC 记忆指标、HCW 窗口切换/压缩、记忆分层、NMC 编码、CLV 快照。
#[allow(missing_docs)]
pub enum MemoryEvent {
    /// MLC 上报记忆指标 — 修正 V2 违规
    MemoryMetricsReported {
        metadata: EventMetadata,
        hit_rate: f32,
        evictions: u64,
    },
    /// 记忆分层完成,CMT/LSCT 据此迁移数据
    MemoryTiered {
        metadata: EventMetadata,
        tier: String,
        item_count: u32,
        memory_id: Option<String>,
    },
    /// HCW 窗口层级切换
    ContextWindowSwitched {
        metadata: EventMetadata,
        from_tier: String,
        to_tier: String,
        reason: String,
    },
    /// HCW 上下文压缩完成
    ContextCompressed {
        metadata: EventMetadata,
        original_size: u64,
        compressed_size: u64,
        ratio: f32,
    },
    /// NMC 多模态编码完成 — L2 Memory → L9 Quest
    NmcEncoded {
        metadata: EventMetadata,
        modality: String,
        content_hash: String,
        clv_dimension: usize,
    },
    /// CLV 快照报告 — L2 Memory → L10 Interface
    ClvSnapshotReported {
        metadata: EventMetadata,
        modality: String,
        content_hash: String,
        clv_summary: ClvSummary,
    },
}

// ============================================================
// StorageEvent — L3 存储/缓存事件(6 变体)
// ============================================================

/// L3 存储/缓存事件
///
/// 包含 SCC 缓存操作、CMT 能力分层、LSCT 层级切换、缓存统计。
#[allow(missing_docs)]
pub enum StorageEvent {
    /// SCC 缓存命中
    CacheHit {
        metadata: EventMetadata,
        cache_key: String,
    },
    /// SCC 缓存未命中
    CacheMiss {
        metadata: EventMetadata,
        cache_key: String,
    },
    /// CMT 能力分层迁移
    CapabilityTiered {
        metadata: EventMetadata,
        capability_id: String,
        from_tier: String,
        to_tier: String,
        reason: String,
    },
    /// SCC 推测性预取完成
    CachePrefetched {
        metadata: EventMetadata,
        prefetched_ids: Vec<String>,
    },
    /// SCC 缓存统计
    CacheStatsReported {
        metadata: EventMetadata,
        hit_rate: f32,
        eviction_count: u64,
    },
    /// LSCT 层级切换
    LsctTierSwitched {
        metadata: EventMetadata,
        capability_id: String,
        from_tier: String,
        to_tier: String,
        reason: String,
    },
}

// ============================================================
// SecurityEvent — L4 安全事件(7 变体)
// ============================================================

/// L4 安全事件
///
/// 包含沙箱违规、能力冻结、预算超限/调整/统计/指标、ASA 安全干预。
#[allow(missing_docs)]
pub enum SecurityEvent {
    /// 沙箱检测到违规
    SandboxViolation {
        metadata: EventMetadata,
        violation_type: String,
        detail: String,
    },
    /// 能力被 Decay Engine 冻结
    CapabilityFrozen {
        metadata: EventMetadata,
        capability_id: String,
        reason: String,
    },
    /// 预算超限 `[Critical]`
    BudgetExceeded {
        metadata: EventMetadata,
        budget_type: String,
        current: u64,
        limit: u64,
    },
    /// DECB 预算档位调整
    BudgetAdjusted {
        metadata: EventMetadata,
        quest_id: String,
        old_tier: String,
        new_tier: String,
        coefficient: f32,
        reason: String,
    },
    /// ASA 安全干预动作 `[Critical]`
    AsaIntervention {
        metadata: EventMetadata,
        operation_id: String,
        action: String,
        safety_score: f32,
        block_reason: Option<String>,
        alternative_suggestion: Option<String>,
    },
    /// 预算消耗统计上报
    BudgetStatsReported {
        metadata: EventMetadata,
        total_consumption: f64,
        remaining_budget: f64,
        utilization_rate: f32,
    },
    /// 预算指标更新
    BudgetMetricsUpdated {
        metadata: EventMetadata,
        metrics: BudgetMetricsPayload,
    },
}

// ============================================================
// RouterEvent — L6 路由事件(13 变体)
// ============================================================

/// L6 路由事件
///
/// 包含 OSA 稀疏掩码、FaaE 工具路由/专家管理、GEA 激活、KVBSR 块重平衡、
/// SESA 稀疏激活、EDSB 熵均衡。
#[allow(missing_docs)]
pub enum RouterEvent {
    /// OSA 完全维稀疏掩码计算 — 修正 V1 违规
    OmniSparseMasksComputed {
        metadata: EventMetadata,
        mask_hash: String,
        sparsity: f32,
        context_mask: Vec<String>,
    },
    /// FaaE 工具路由完成
    ToolsRouted {
        metadata: EventMetadata,
        routed_count: u32,
        top_tool: String,
        routed_tools: Vec<String>,
    },
    /// GEA 专家激活完成
    ExpertActivated {
        metadata: EventMetadata,
        activated_experts: Vec<String>,
        suppressed_experts: Vec<String>,
        top_gate_value: f32,
    },
    /// GEA 激活阈值动态调整
    ActivationThresholdAdjusted {
        metadata: EventMetadata,
        old_threshold: f32,
        new_threshold: f32,
        load_factor: f32,
    },
    /// GEA 激活缓存统计
    ActivationCacheStats {
        metadata: EventMetadata,
        hit_rate: f32,
        entry_count: u32,
    },
    /// FaaE 专家路由完成
    ExpertRouted {
        metadata: EventMetadata,
        routed_tool: String,
        confidence: f32,
    },
    /// EDSB 熵均衡完成
    EntropyBalanced {
        metadata: EventMetadata,
        old_entropy: f32,
        new_entropy: f32,
        redistributed_count: u32,
    },
    /// FaaE 工具专家注册
    ExpertRegistered {
        metadata: EventMetadata,
        tool_id: String,
    },
    /// FaaE 工具专家注销
    ExpertUnregistered {
        metadata: EventMetadata,
        tool_id: String,
    },
    /// KVBSR 块重平衡完成
    BlocksRebalanced {
        metadata: EventMetadata,
        old_block_count: u32,
        new_block_count: u32,
    },
    /// SESA 激活完成
    SesaActivationCompleted {
        metadata: EventMetadata,
        total_experts: u32,
        active_experts: u32,
        sparsity_ratio: f32,
        latency_us: u64,
    },
}

// ============================================================
// ExecutionEvent — L7 执行事件(9 变体)
// ============================================================

/// L7 执行事件
///
/// 包含 PVL 操作/策略、GQEP 聚集/超时/孤儿、MTPE 预测。
#[allow(missing_docs)]
pub enum ExecutionEvent {
    /// PVL 生产验证完成一个操作
    OperationProduced {
        metadata: EventMetadata,
        op_id: String,
        content_hash: String,
    },
    /// PVL 验证评分
    PredictionVerified {
        metadata: EventMetadata,
        op_id: String,
        score: f32,
    },
    /// 执行流程完成
    ExecutionCompleted {
        metadata: EventMetadata,
        quest_id: String,
        result_hash: String,
    },
    /// GQEP 聚集执行完成
    GatherCompleted {
        metadata: EventMetadata,
        total: u32,
        succeeded: u32,
        failed: u32,
        latency_ms: f32,
    },
    /// GQEP 操作超时
    OperationTimedOut {
        metadata: EventMetadata,
        operation_id: String,
        timeout_ms: u64,
    },
    /// GQEP 全局 gather 超时 `[Critical]`(Phase V Task V-3 [N14])
    GatherTimedOut {
        metadata: EventMetadata,
        deadline_ms: u64,
        elapsed_ms: u64,
        total: u32,
        abandoned: u32,
    },
    /// GQEP 检测到孤儿调用 `[Critical]`
    OrphanCallDetected {
        metadata: EventMetadata,
        operation_id: String,
        spawn_location: String,
    },
    /// PVL Producer 策略调整
    ProducerStrategyAdjusted {
        metadata: EventMetadata,
        adjustment_reason: String,
        new_strategy: String,
    },
    /// MTPE 多步预测完成
    PredictionMade {
        metadata: EventMetadata,
        quest_id: String,
        n: usize,
        avg_confidence: f32,
    },
    /// MTPE 预测成功率统计
    PredictionStatsReported {
        metadata: EventMetadata,
        success_rate_by_n: std::collections::HashMap<usize, f32>,
    },
    /// MTPE 预测失败回退
    PredictionRolledBack {
        metadata: EventMetadata,
        failed_step: usize,
        rollback_to: usize,
    },
}

// ============================================================
// QuestEvent — L8-L9 任务/议会事件(24 变体)
// ============================================================

/// L8-L9 任务与议会事件
///
/// 包含 Quest 生命周期、检查点、共识/投票、议会辩论/角色、
/// TUI 双向控制、R1 影子模式、SSRA 融合、GSOE 策略进化。
#[allow(missing_docs)]
pub enum QuestEvent {
    /// 新 Quest 创建完成
    QuestCreated {
        metadata: EventMetadata,
        quest_id: String,
        title: String,
        task_count: u32,
    },
    /// Quest 进度更新
    QuestProgressUpdated {
        metadata: EventMetadata,
        quest_id: String,
        completed: u32,
        total: u32,
    },
    /// Quest 完整列表更新
    QuestListUpdated {
        metadata: EventMetadata,
        quests: Vec<Quest>,
        source: String,
    },
    /// Quest 已完成
    QuestCompleted {
        metadata: EventMetadata,
        quest_id: String,
        status: QuestStatus,
    },
    /// TTG 切换思考模式
    ThinkingModeSwitched {
        metadata: EventMetadata,
        quest_id: String,
        from_mode: String,
        to_mode: String,
        reason: String,
    },
    /// 检查点已保存 `[Critical]`
    CheckpointSaved {
        metadata: EventMetadata,
        quest_id: String,
        checkpoint_id: String,
        memory_snapshot_hash: String,
    },
    /// 检查点已加载
    CheckpointLoaded {
        metadata: EventMetadata,
        quest_id: String,
        checkpoint_id: String,
    },
    /// 议会达成共识 `[Critical]`
    ConsensusReached {
        metadata: EventMetadata,
        quest_id: String,
        decision_hash: String,
        dpo_pair_id: Option<String>,
    },
    /// 议员投票
    VoteCast {
        metadata: EventMetadata,
        proposal_id: String,
        voter: String,
        vote: bool,
    },
    /// 议会辩论开始
    DebateStarted {
        metadata: EventMetadata,
        quest_id: String,
        proposal_id: String,
        participant_count: u8,
    },
    /// Skeptic 行使否决权 `[Critical]`
    SkepticVeto {
        metadata: EventMetadata,
        quest_id: String,
        veto_reason: String,
        frozen_capabilities: Vec<String>,
    },
    /// Skeptic 否决权被人工覆盖 `[Critical]`
    VetoOverridden {
        metadata: EventMetadata,
        quest_id: String,
        proposal_id: String,
        veto_reason: String,
        override_reason: String,
        override_by: String,
    },
    /// AHIRT 红队审计结果 `[Critical]`
    RedTeamAudit {
        metadata: EventMetadata,
        vulnerability_type: String,
        failed_probes: u32,
        total_probes: u32,
        detection_rate: f32,
        remediation_suggestion: String,
    },
    /// AHIRT 探测批次完成
    AhirtProbeCompleted {
        metadata: EventMetadata,
        probe_type: String,
        total: u32,
        passed: u32,
        failed: u32,
        detection_rate: f32,
    },
    /// 议会角色注册
    RoleRegistered {
        metadata: EventMetadata,
        role_id: String,
        role_name: String,
        voting_weight: f32,
    },
    /// Quest 暂停请求
    QuestPauseRequested {
        metadata: EventMetadata,
        quest_id: String,
        requested_by: String,
    },
    /// Quest 恢复请求
    QuestResumeRequested {
        metadata: EventMetadata,
        quest_id: String,
        requested_by: String,
    },
    /// 投票请求
    VoteCastRequested {
        metadata: EventMetadata,
        proposal_id: String,
        voter: String,
        vote: VoteValue,
    },
    /// Quest 已暂停
    QuestPaused {
        metadata: EventMetadata,
        quest_id: String,
        requested_by: String,
    },
    /// Quest 已恢复
    QuestResumed {
        metadata: EventMetadata,
        quest_id: String,
        requested_by: String,
    },
    /// Quest 取消请求
    QuestCancelRequested {
        metadata: EventMetadata,
        quest_id: String,
        requested_by: String,
    },
    /// Quest 已取消
    QuestCancelled {
        metadata: EventMetadata,
        quest_id: String,
        requested_by: String,
    },
    /// Quest 优先级变更请求
    QuestPriorityChanged {
        metadata: EventMetadata,
        quest_id: String,
        new_priority: u8,
        requested_by: String,
    },
    /// Quest 优先级已调整
    QuestPriorityAdjusted {
        metadata: EventMetadata,
        quest_id: String,
        new_priority: u8,
        requested_by: String,
    },
    /// R1 影子模式退化检测
    R1ShadowRegressionDetected {
        metadata: EventMetadata,
        report_date: DateTime<Utc>,
        regression_streak: u32,
    },
    /// R1 影子模式解冻就绪
    R1ShadowPromotionReady {
        metadata: EventMetadata,
        report_date: DateTime<Utc>,
        win_rate: f64,
        ewma_level: f32,
    },
    /// R1 影子模式回滚失败 `[Critical]`
    ///
    /// P2-13 扩展:新增 `trigger_type` / `triggered_at` / `details` / `diagnostic`
    /// 结构化字段,保留 `reason` 向后兼容。详见 `NexusEvent::R1ShadowRollbackFailed` 文档。
    ///
    /// NOTE: 子枚举不派生 Serialize/Deserialize,`#[serde(default)]` 仅在
    /// `NexusEvent`(types.rs)上生效,此处字段定义保持与 NexusEvent 对齐。
    R1ShadowRollbackFailed {
        metadata: EventMetadata,
        reason: String,
        trigger_type: crate::types::RollbackTriggerType,
        triggered_at: Option<DateTime<Utc>>,
        details: String,
        diagnostic: crate::types::RollbackDiagnosticContext,
    },
    /// SSRA 融合完成 — L7 → L5/L8
    SsraFusionCompleted {
        metadata: EventMetadata,
        quest_id: String,
        fused_template_id: String,
        latency_ms: u64,
        confidence: f32,
    },
    /// GSOE 策略进化完成 — L5 → L8/L7
    GsoePolicyUpdated {
        metadata: EventMetadata,
        generation: u64,
        improvement: f32,
        new_mutation_rate: f32,
        new_selection_pressure: f32,
    },
    /// Repo Wiki 更新完成 — L5 Knowledge → L9 Quest
    WikiUpdated {
        metadata: EventMetadata,
        wiki_hash: String,
        delta: u32,
    },
    /// GSOE 触发在线进化 — L5 Knowledge 同层通信
    EvolutionTriggered {
        metadata: EventMetadata,
        generation: u64,
        fitness: f32,
    },
    /// AutoDPO 生成训练对 — L5 Knowledge 同层通信
    DpoPairGenerated {
        metadata: EventMetadata,
        pair_id: String,
        chosen: String,
        rejected: String,
    },
}

// ============================================================
// InterfaceEvent — L10 界面/TUI/Agent/MCP/监控事件(17 变体)
// ============================================================

/// L10 界面与交互事件
///
/// 包含 TUI 动作协议、TUI 对话协议、MCP 网格消息、CHTC 工具调用/适配器、
/// Agent 协作、衰减/路由器/效率监控指标、Spec 注册。
#[allow(missing_docs)]
pub enum InterfaceEvent {
    /// MCP 网格收到远端消息
    McpMessageReceived {
        metadata: EventMetadata,
        source_node: String,
        msg_type: String,
    },
    /// CHTC 接收到 IDE 工具调用
    ChtcToolCallReceived {
        metadata: EventMetadata,
        call_id: String,
        tool_id: String,
        ide_source: String,
        parameters_hash: String,
    },
    /// MCP Mesh 事务完成
    McpMeshTransactionCompleted {
        metadata: EventMetadata,
        transaction_id: String,
        participant_count: u32,
        latency_ms: u64,
        success: bool,
        /// 关联能力 ID(Task 0.7 v2.9.0-omega 引入,csn-substitutor 降级链精准推进)
        capability_id: Option<String>,
    },
    /// CSN 替代触发
    CsnSubstitutionTriggered {
        metadata: EventMetadata,
        original_capability_id: String,
        substitute_id: String,
        similarity_score: f32,
        degradation_level: u32,
    },
    /// 效率告警触发
    EfficiencyAlertTriggered {
        metadata: EventMetadata,
        rule_id: String,
        metric_name: String,
        triggered_value: f64,
        threshold: f64,
    },
    /// 衰减指标报告
    DecayMetricsReported {
        metadata: EventMetadata,
        coefficient: f32,
        recent_events: Vec<String>,
        cycle_start: DateTime<Utc>,
        /// P2-11: 本周期 fallback 触发次数(向后兼容,默认 0)
        ///
        /// NOTE: `#[serde(default)]` 仅在主 `NexusEvent`(派生 Serialize/Deserialize)
        /// 上生效;子枚举不参与序列化,此处无需重复标注。
        fallback_count_delta: u64,
    },
    /// 路由器统计报告
    RouterStatsReported {
        metadata: EventMetadata,
        kvbsr_stats: RouterStatsPayload,
        sesa_stats: RouterStatsPayload,
        faae_stats: RouterStatsPayload,
    },
    /// MCP 节点心跳
    McpNodeHeartbeat {
        metadata: EventMetadata,
        node_id: String,
        status: String,
        throughput: u64,
        last_seen: DateTime<Utc>,
    },
    /// CHTC 适配器状态
    ChtcAdapterStatus {
        metadata: EventMetadata,
        adapter_id: String,
        adapter_type: String,
        compatibility_score: u8,
        recent_requests: Vec<(String, u32)>,
        is_online: bool,
    },
    /// Agent 任务委派
    AgentTaskDelegated {
        metadata: EventMetadata,
        from: String,
        to: String,
        task_id: String,
        deadline: DateTime<Utc>,
        priority: TaskPriority,
    },
    /// Agent 任务完成
    AgentTaskCompleted {
        metadata: EventMetadata,
        from: String,
        to: String,
        task_id: String,
        result_summary: String,
    },
    /// Agent 任务失败 `[Critical]`
    AgentTaskFailed {
        metadata: EventMetadata,
        from: String,
        to: String,
        task_id: String,
        error: String,
        retry_count: u32,
    },
    /// Agent 咨询请求
    AgentConsultRequested {
        metadata: EventMetadata,
        from: String,
        to: String,
        question: String,
        context: String,
        urgency: ConsultUrgency,
    },
    /// Agent 咨询回复
    AgentConsultResponded {
        metadata: EventMetadata,
        from: String,
        to: String,
        answer: String,
        references: Vec<String>,
    },
    /// Agent 心跳
    AgentHeartbeat {
        metadata: EventMetadata,
        from: String,
        status: AgentStatus,
        current_task: Option<String>,
        token_usage: u64,
        memory_usage_mb: u64,
    },
    /// Agent 上下文溢出
    AgentContextOverflow {
        metadata: EventMetadata,
        agent_id: String,
        current_tokens: usize,
        max_tokens: usize,
    },
    /// TUI 动作请求
    TuiActionRequested {
        metadata: EventMetadata,
        action_id: String,
        payload: String,
        source: ActionSource,
    },
    /// TUI 动作进度
    TuiActionProgressed {
        metadata: EventMetadata,
        action_id: String,
        delta: String,
    },
    /// TUI 动作完成
    TuiActionCompleted {
        metadata: EventMetadata,
        action_id: String,
        result: String,
    },
    /// TUI 动作失败
    TuiActionFailed {
        metadata: EventMetadata,
        action_id: String,
        error: String,
    },
    /// TUI 对话提交
    TuiChatSubmitted {
        metadata: EventMetadata,
        session_id: String,
        query: String,
        slash_command: Option<String>,
    },
    /// TUI 对话流式分块
    TuiChatResponseChunk {
        metadata: EventMetadata,
        session_id: String,
        delta: String,
        cursor_hint: u32,
    },
    /// TUI 对话完成
    TuiChatCompleted {
        metadata: EventMetadata,
        session_id: String,
        tool_use: Option<String>,
    },
    /// TUI 对话状态变更
    TuiChatStatusChanged {
        metadata: EventMetadata,
        session_id: String,
        status: ChatStatus,
    },
    /// 状态刷新请求
    RefreshStateRequested {
        metadata: EventMetadata,
        requested_by: String,
    },
    /// P5.2.3: Spec 版本注册完成
    SpecRegistered {
        metadata: EventMetadata,
        spec_name: String,
        spec_version: u32,
        parent_version: Option<u32>,
        source: String,
    },
}

// ============================================================
// EventClassification 实现 — CoreEvent
// ============================================================

impl EventClassification for CoreEvent {
    fn metadata(&self) -> &EventMetadata {
        match self {
            Self::NexusStateChanged { metadata, .. }
            | Self::ModelRouteSelected { metadata, .. }
            | Self::UserIntentEncoded { metadata, .. }
            | Self::SlowConsumerDropped { metadata, .. }
            | Self::AuditLogged { metadata, .. } => metadata,
        }
    }

    fn severity(&self) -> EventSeverity {
        match self {
            // SlowConsumerDropped 为 Critical(系统健康告警)
            Self::SlowConsumerDropped { .. } => EventSeverity::Critical,
            _ => EventSeverity::Normal,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::NexusStateChanged { .. } => "NexusStateChanged",
            Self::ModelRouteSelected { .. } => "ModelRouteSelected",
            Self::UserIntentEncoded { .. } => "UserIntentEncoded",
            Self::SlowConsumerDropped { .. } => "SlowConsumerDropped",
            Self::AuditLogged { .. } => "AuditLogged",
        }
    }
}

// ============================================================
// EventClassification 实现 — MemoryEvent
// ============================================================

impl EventClassification for MemoryEvent {
    fn metadata(&self) -> &EventMetadata {
        match self {
            Self::MemoryMetricsReported { metadata, .. }
            | Self::MemoryTiered { metadata, .. }
            | Self::ContextWindowSwitched { metadata, .. }
            | Self::ContextCompressed { metadata, .. }
            | Self::NmcEncoded { metadata, .. }
            | Self::ClvSnapshotReported { metadata, .. } => metadata,
        }
    }

    fn severity(&self) -> EventSeverity {
        EventSeverity::Normal
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::MemoryMetricsReported { .. } => "MemoryMetricsReported",
            Self::MemoryTiered { .. } => "MemoryTiered",
            Self::ContextWindowSwitched { .. } => "ContextWindowSwitched",
            Self::ContextCompressed { .. } => "ContextCompressed",
            Self::NmcEncoded { .. } => "NmcEncoded",
            Self::ClvSnapshotReported { .. } => "ClvSnapshotReported",
        }
    }
}

// ============================================================
// EventClassification 实现 — StorageEvent
// ============================================================

impl EventClassification for StorageEvent {
    fn metadata(&self) -> &EventMetadata {
        match self {
            Self::CacheHit { metadata, .. }
            | Self::CacheMiss { metadata, .. }
            | Self::CapabilityTiered { metadata, .. }
            | Self::CachePrefetched { metadata, .. }
            | Self::CacheStatsReported { metadata, .. }
            | Self::LsctTierSwitched { metadata, .. } => metadata,
        }
    }

    fn severity(&self) -> EventSeverity {
        EventSeverity::Normal
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::CacheHit { .. } => "CacheHit",
            Self::CacheMiss { .. } => "CacheMiss",
            Self::CapabilityTiered { .. } => "CapabilityTiered",
            Self::CachePrefetched { .. } => "CachePrefetched",
            Self::CacheStatsReported { .. } => "CacheStatsReported",
            Self::LsctTierSwitched { .. } => "LsctTierSwitched",
        }
    }
}

// ============================================================
// EventClassification 实现 — SecurityEvent
// ============================================================

impl EventClassification for SecurityEvent {
    fn metadata(&self) -> &EventMetadata {
        match self {
            Self::SandboxViolation { metadata, .. }
            | Self::CapabilityFrozen { metadata, .. }
            | Self::BudgetExceeded { metadata, .. }
            | Self::BudgetAdjusted { metadata, .. }
            | Self::AsaIntervention { metadata, .. }
            | Self::BudgetStatsReported { metadata, .. }
            | Self::BudgetMetricsUpdated { metadata, .. } => metadata,
        }
    }

    fn severity(&self) -> EventSeverity {
        match self {
            // BudgetExceeded:Hard Constraint 第 10 条要求 Critical
            Self::BudgetExceeded { .. } => EventSeverity::Critical,
            // AsaIntervention:对齐 spec.md L186 红线,统一 Critical
            Self::AsaIntervention { .. } => EventSeverity::Critical,
            _ => EventSeverity::Normal,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::SandboxViolation { .. } => "SandboxViolation",
            Self::CapabilityFrozen { .. } => "CapabilityFrozen",
            Self::BudgetExceeded { .. } => "BudgetExceeded",
            Self::BudgetAdjusted { .. } => "BudgetAdjusted",
            Self::AsaIntervention { .. } => "AsaIntervention",
            Self::BudgetStatsReported { .. } => "BudgetStatsReported",
            Self::BudgetMetricsUpdated { .. } => "BudgetMetricsUpdated",
        }
    }
}

// ============================================================
// EventClassification 实现 — RouterEvent
// ============================================================

impl EventClassification for RouterEvent {
    fn metadata(&self) -> &EventMetadata {
        match self {
            Self::OmniSparseMasksComputed { metadata, .. }
            | Self::ToolsRouted { metadata, .. }
            | Self::ExpertActivated { metadata, .. }
            | Self::ActivationThresholdAdjusted { metadata, .. }
            | Self::ActivationCacheStats { metadata, .. }
            | Self::ExpertRouted { metadata, .. }
            | Self::EntropyBalanced { metadata, .. }
            | Self::ExpertRegistered { metadata, .. }
            | Self::ExpertUnregistered { metadata, .. }
            | Self::BlocksRebalanced { metadata, .. }
            | Self::SesaActivationCompleted { metadata, .. } => metadata,
        }
    }

    fn severity(&self) -> EventSeverity {
        EventSeverity::Normal
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::OmniSparseMasksComputed { .. } => "OmniSparseMasksComputed",
            Self::ToolsRouted { .. } => "ToolsRouted",
            Self::ExpertActivated { .. } => "ExpertActivated",
            Self::ActivationThresholdAdjusted { .. } => "ActivationThresholdAdjusted",
            Self::ActivationCacheStats { .. } => "ActivationCacheStats",
            Self::ExpertRouted { .. } => "ExpertRouted",
            Self::EntropyBalanced { .. } => "EntropyBalanced",
            Self::ExpertRegistered { .. } => "ExpertRegistered",
            Self::ExpertUnregistered { .. } => "ExpertUnregistered",
            Self::BlocksRebalanced { .. } => "BlocksRebalanced",
            Self::SesaActivationCompleted { .. } => "SesaActivationCompleted",
        }
    }
}

// ============================================================
// EventClassification 实现 — ExecutionEvent
// ============================================================

impl EventClassification for ExecutionEvent {
    fn metadata(&self) -> &EventMetadata {
        match self {
            Self::OperationProduced { metadata, .. }
            | Self::PredictionVerified { metadata, .. }
            | Self::ExecutionCompleted { metadata, .. }
            | Self::GatherCompleted { metadata, .. }
            | Self::OperationTimedOut { metadata, .. }
            | Self::GatherTimedOut { metadata, .. }
            | Self::OrphanCallDetected { metadata, .. }
            | Self::ProducerStrategyAdjusted { metadata, .. }
            | Self::PredictionMade { metadata, .. }
            | Self::PredictionStatsReported { metadata, .. }
            | Self::PredictionRolledBack { metadata, .. } => metadata,
        }
    }

    fn severity(&self) -> EventSeverity {
        match self {
            // OrphanCallDetected:Claude Code 尸检 5.4% 孤儿调用教训
            Self::OrphanCallDetected { .. } => EventSeverity::Critical,
            _ => EventSeverity::Normal,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::OperationProduced { .. } => "OperationProduced",
            Self::PredictionVerified { .. } => "PredictionVerified",
            Self::ExecutionCompleted { .. } => "ExecutionCompleted",
            Self::GatherCompleted { .. } => "GatherCompleted",
            Self::OperationTimedOut { .. } => "OperationTimedOut",
            Self::GatherTimedOut { .. } => "GatherTimedOut",
            Self::OrphanCallDetected { .. } => "OrphanCallDetected",
            Self::ProducerStrategyAdjusted { .. } => "ProducerStrategyAdjusted",
            Self::PredictionMade { .. } => "PredictionMade",
            Self::PredictionStatsReported { .. } => "PredictionStatsReported",
            Self::PredictionRolledBack { .. } => "PredictionRolledBack",
        }
    }
}

// ============================================================
// EventClassification 实现 — QuestEvent
// ============================================================

impl EventClassification for QuestEvent {
    fn metadata(&self) -> &EventMetadata {
        match self {
            Self::QuestCreated { metadata, .. }
            | Self::QuestProgressUpdated { metadata, .. }
            | Self::QuestListUpdated { metadata, .. }
            | Self::QuestCompleted { metadata, .. }
            | Self::ThinkingModeSwitched { metadata, .. }
            | Self::CheckpointSaved { metadata, .. }
            | Self::CheckpointLoaded { metadata, .. }
            | Self::ConsensusReached { metadata, .. }
            | Self::VoteCast { metadata, .. }
            | Self::DebateStarted { metadata, .. }
            | Self::SkepticVeto { metadata, .. }
            | Self::VetoOverridden { metadata, .. }
            | Self::RedTeamAudit { metadata, .. }
            | Self::AhirtProbeCompleted { metadata, .. }
            | Self::RoleRegistered { metadata, .. }
            | Self::QuestPauseRequested { metadata, .. }
            | Self::QuestResumeRequested { metadata, .. }
            | Self::VoteCastRequested { metadata, .. }
            | Self::QuestPaused { metadata, .. }
            | Self::QuestResumed { metadata, .. }
            | Self::QuestCancelRequested { metadata, .. }
            | Self::QuestCancelled { metadata, .. }
            | Self::QuestPriorityChanged { metadata, .. }
            | Self::QuestPriorityAdjusted { metadata, .. }
            | Self::R1ShadowRegressionDetected { metadata, .. }
            | Self::R1ShadowPromotionReady { metadata, .. }
            | Self::R1ShadowRollbackFailed { metadata, .. }
            | Self::SsraFusionCompleted { metadata, .. }
            | Self::GsoePolicyUpdated { metadata, .. }
            | Self::WikiUpdated { metadata, .. }
            | Self::EvolutionTriggered { metadata, .. }
            | Self::DpoPairGenerated { metadata, .. } => metadata,
        }
    }

    fn severity(&self) -> EventSeverity {
        match self {
            // Critical 事件
            Self::CheckpointSaved { .. }
            | Self::ConsensusReached { .. }
            | Self::SkepticVeto { .. }
            | Self::VetoOverridden { .. }
            | Self::RedTeamAudit { .. }
            | Self::R1ShadowRollbackFailed { .. } => EventSeverity::Critical,
            // Info 控制事件(请求/反馈):不阻断系统,不触发 mpsc 旁路投递
            Self::QuestCancelRequested { .. }
            | Self::QuestCancelled { .. }
            | Self::QuestPriorityChanged { .. }
            | Self::QuestPriorityAdjusted { .. } => EventSeverity::Info,
            _ => EventSeverity::Normal,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::QuestCreated { .. } => "QuestCreated",
            Self::QuestProgressUpdated { .. } => "QuestProgressUpdated",
            Self::QuestListUpdated { .. } => "QuestListUpdated",
            Self::QuestCompleted { .. } => "QuestCompleted",
            Self::ThinkingModeSwitched { .. } => "ThinkingModeSwitched",
            Self::CheckpointSaved { .. } => "CheckpointSaved",
            Self::CheckpointLoaded { .. } => "CheckpointLoaded",
            Self::ConsensusReached { .. } => "ConsensusReached",
            Self::VoteCast { .. } => "VoteCast",
            Self::DebateStarted { .. } => "DebateStarted",
            Self::SkepticVeto { .. } => "SkepticVeto",
            Self::VetoOverridden { .. } => "VetoOverridden",
            Self::RedTeamAudit { .. } => "RedTeamAudit",
            Self::AhirtProbeCompleted { .. } => "AhirtProbeCompleted",
            Self::RoleRegistered { .. } => "RoleRegistered",
            Self::QuestPauseRequested { .. } => "QuestPauseRequested",
            Self::QuestResumeRequested { .. } => "QuestResumeRequested",
            Self::VoteCastRequested { .. } => "VoteCastRequested",
            Self::QuestPaused { .. } => "QuestPaused",
            Self::QuestResumed { .. } => "QuestResumed",
            Self::QuestCancelRequested { .. } => "QuestCancelRequested",
            Self::QuestCancelled { .. } => "QuestCancelled",
            Self::QuestPriorityChanged { .. } => "QuestPriorityChanged",
            Self::QuestPriorityAdjusted { .. } => "QuestPriorityAdjusted",
            Self::R1ShadowRegressionDetected { .. } => "R1ShadowRegressionDetected",
            Self::R1ShadowPromotionReady { .. } => "R1ShadowPromotionReady",
            Self::R1ShadowRollbackFailed { .. } => "R1ShadowRollbackFailed",
            Self::SsraFusionCompleted { .. } => "SsraFusionCompleted",
            Self::GsoePolicyUpdated { .. } => "GsoePolicyUpdated",
            Self::WikiUpdated { .. } => "WikiUpdated",
            Self::EvolutionTriggered { .. } => "EvolutionTriggered",
            Self::DpoPairGenerated { .. } => "DpoPairGenerated",
        }
    }
}

// ============================================================
// EventClassification 实现 — InterfaceEvent
// ============================================================

impl EventClassification for InterfaceEvent {
    fn metadata(&self) -> &EventMetadata {
        match self {
            Self::McpMessageReceived { metadata, .. }
            | Self::ChtcToolCallReceived { metadata, .. }
            | Self::McpMeshTransactionCompleted { metadata, .. }
            | Self::CsnSubstitutionTriggered { metadata, .. }
            | Self::EfficiencyAlertTriggered { metadata, .. }
            | Self::DecayMetricsReported { metadata, .. }
            | Self::RouterStatsReported { metadata, .. }
            | Self::McpNodeHeartbeat { metadata, .. }
            | Self::ChtcAdapterStatus { metadata, .. }
            | Self::AgentTaskDelegated { metadata, .. }
            | Self::AgentTaskCompleted { metadata, .. }
            | Self::AgentTaskFailed { metadata, .. }
            | Self::AgentConsultRequested { metadata, .. }
            | Self::AgentConsultResponded { metadata, .. }
            | Self::AgentHeartbeat { metadata, .. }
            | Self::AgentContextOverflow { metadata, .. }
            | Self::TuiActionRequested { metadata, .. }
            | Self::TuiActionProgressed { metadata, .. }
            | Self::TuiActionCompleted { metadata, .. }
            | Self::TuiActionFailed { metadata, .. }
            | Self::TuiChatSubmitted { metadata, .. }
            | Self::TuiChatResponseChunk { metadata, .. }
            | Self::TuiChatCompleted { metadata, .. }
            | Self::TuiChatStatusChanged { metadata, .. }
            | Self::RefreshStateRequested { metadata, .. }
            | Self::SpecRegistered { metadata, .. } => metadata,
        }
    }

    fn severity(&self) -> EventSeverity {
        match self {
            // AgentTaskFailed:任务失败可能影响 Quest 完整性
            Self::AgentTaskFailed { .. } => EventSeverity::Critical,
            // TUI 交互式动作协议:请求/终态为 Info
            Self::TuiActionRequested { .. }
            | Self::TuiActionCompleted { .. }
            | Self::TuiActionFailed { .. }
            | Self::TuiChatSubmitted { .. }
            | Self::TuiChatCompleted { .. } => EventSeverity::Info,
            // 其余为 Normal(高频流式/监控/Agent 通信)
            _ => EventSeverity::Normal,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::McpMessageReceived { .. } => "McpMessageReceived",
            Self::ChtcToolCallReceived { .. } => "ChtcToolCallReceived",
            Self::McpMeshTransactionCompleted { .. } => "McpMeshTransactionCompleted",
            Self::CsnSubstitutionTriggered { .. } => "CsnSubstitutionTriggered",
            Self::EfficiencyAlertTriggered { .. } => "EfficiencyAlertTriggered",
            Self::DecayMetricsReported { .. } => "DecayMetricsReported",
            Self::RouterStatsReported { .. } => "RouterStatsReported",
            Self::McpNodeHeartbeat { .. } => "McpNodeHeartbeat",
            Self::ChtcAdapterStatus { .. } => "ChtcAdapterStatus",
            Self::AgentTaskDelegated { .. } => "AgentTaskDelegated",
            Self::AgentTaskCompleted { .. } => "AgentTaskCompleted",
            Self::AgentTaskFailed { .. } => "AgentTaskFailed",
            Self::AgentConsultRequested { .. } => "AgentConsultRequested",
            Self::AgentConsultResponded { .. } => "AgentConsultResponded",
            Self::AgentHeartbeat { .. } => "AgentHeartbeat",
            Self::AgentContextOverflow { .. } => "AgentContextOverflow",
            Self::TuiActionRequested { .. } => "TuiActionRequested",
            Self::TuiActionProgressed { .. } => "TuiActionProgressed",
            Self::TuiActionCompleted { .. } => "TuiActionCompleted",
            Self::TuiActionFailed { .. } => "TuiActionFailed",
            Self::TuiChatSubmitted { .. } => "TuiChatSubmitted",
            Self::TuiChatResponseChunk { .. } => "TuiChatResponseChunk",
            Self::TuiChatCompleted { .. } => "TuiChatCompleted",
            Self::TuiChatStatusChanged { .. } => "TuiChatStatusChanged",
            Self::RefreshStateRequested { .. } => "RefreshStateRequested",
            Self::SpecRegistered { .. } => "SpecRegistered",
        }
    }
}
