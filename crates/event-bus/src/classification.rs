//! 事件分类实现 — NexusEvent severity()/type_name() 巨型 match 外移(P1-3 拆分)
//!
//! 对应架构层:L1 Core(event-bus)
//!
//! # 拆分背景(WHY)
//! types.rs 曾达 4451 行/189KB(enum 定义 + metadata()/severity()/type_name()
//! 三大 match + 测试同文件),每新增事件需同步修改多处触点。本模块承接
//! severity()/type_name() 两个分类 match,types.rs 保留 enum 定义与 metadata()。
//! 同 crate 内 `impl NexusEvent` 跨文件分块,调用路径零变化(`event.severity()`),
//! 33 个依赖方零感知(公共 API 导出集不变,方法经类型自身可见)。
//!
//! # 架构红线(不可移动)
//! - severity() 判定逻辑必须留在 event-bus(Critical 事件 mpsc 保障);
//!   本模块仍在 event-bus 内,红线合规。
//! - **双清单同步红线**:本模块 severity() 的 Critical 清单与 bus.rs
//!   `is_critical_mpsc_event` 是两张独立清单,新增 Critical 事件必须同时修改
//!   两处;同步性由 bus.rs 测试 `test_critical_severity_implies_mpsc_bypass` 守护。

use crate::types::{EventSeverity, NexusEvent};

impl NexusEvent {
    /// 判断事件是否为关键事件(Critical)
    ///
    /// 关键事件:CheckpointSaved、ConsensusReached、SlowConsumerDropped、
    /// OrphanCallDetected(Week 4 新增)、SkepticVeto/RedTeamAudit(Week 5 新增)、
    /// VetoOverridden(P1-3 新增:否决覆盖审计)、
    /// BudgetExceeded(F-001 修复:Hard Constraint 第 10 条要求)
    /// 这些事件丢失会导致系统状态不一致或告警遗漏
    ///
    /// WHY BudgetExceeded 标记为 Critical:预算耗尽是系统红线,意味着资源
    /// 已达上限,必须立即触发背压保护(走 mpsc 点对点通道确保投递)并通知
    /// Parliament 触发降级或终止。若标为 Normal,在背压场景下可能被丢弃,
    /// 导致预算超限无人响应、Quest 持续消耗资源直至 OOM,违反架构红线
    /// "1M Token 暴力加载"的预防机制。此为 Hard Constraint 第 10 条的
    /// 强制要求(F-001 修复)。
    ///
    /// WHY:Week 3 新增的 4 个变体(ContextWindowSwitched/ContextCompressed/
    /// CapabilityTiered/BlocksRebalanced)均为 Normal 级别,由通配符分支
    /// 自动覆盖。Week 4 新增的 16 个变体中,仅 OrphanCallDetected 为 Critical
    /// (对应 Claude Code 尸检 5.4% 孤儿调用教训),其余 15 个为 Normal,
    /// 由通配符分支自动覆盖。Week 5 新增的 8 个变体中,SkepticVeto(否决权
    /// 行使)与 RedTeamAudit(红队漏洞审计)为 Critical(丢失导致安全机制
    /// 失效),其余 6 个为 Normal,由通配符分支自动覆盖。P1-3 新增
    /// VetoOverridden 为 Critical(否决覆盖审计,丢失导致覆盖行为不可追溯)。
    /// 若未来新增 Critical 事件,必须在此显式列出,避免被通配符误判为 Normal。
    pub fn severity(&self) -> EventSeverity {
        match self {
            Self::CheckpointSaved { .. }
            | Self::ConsensusReached { .. }
            | Self::SlowConsumerDropped { .. }
            | Self::OrphanCallDetected { .. }
            | Self::SkepticVeto { .. }
            | Self::VetoOverridden { .. }
            | Self::RedTeamAudit { .. }
            | Self::BudgetExceeded { .. }
            // CHIMERA-MAS:AgentTaskFailed 为 Critical(Task 4,ADR-026)
            // WHY:任务失败可能影响 Quest 完整性,必须保证投递到 SecCore 与
            // Parliament 进行补救决策。丢失会导致失败无人响应、Quest 持续等待已死 Agent 结果。
            | Self::AgentTaskFailed { .. }
            // P1-W2.1.4:AsaIntervention 统一标记为 Critical(对齐 spec.md L186 红线)
            // WHY 历史:原设计认为 severity() 是同步函数不应依赖运行时值(action
            // 字段),故走通配符返回 Normal。但 spec.md L186 与 §6.2 红线均将
            // AsaIntervention 列为 6 个 Critical 事件之一(W1.2 TDD 暴露偏差)。
            // 修复策略:统一返回 Critical,无论 action 是 Allow/Warn/Block。
            // 保守策略确保所有 ASA 安全干预走 Critical 通道,Allow/Warn 为低频
            // 事件(每个安全操作最多一个干预),不会产生大量 Critical 事件。
            // Block 级别更需 Critical 投递保证(丢失导致高风险操作继续执行)。
            | Self::AsaIntervention { .. }
            // P4-W16.2.2 步骤 5:R1 影子模式回滚失败为 Critical
            // WHY:回滚失败意味着退化策略可能仍在生效,必须保证投递到 SecCore
            // 与 Parliament 进行紧急干预。丢失导致 Quest 持续受退化策略影响。
            | Self::R1ShadowRollbackFailed { .. }
            // ADR-042 决策 4:R2 冻结违反 + 回滚失败为 Critical
            // WHY:R2 违反等同于安全事件(奖励黑客风险立即生效),回滚失败意味着
            // R2 路径代码可能仍在生效。必须走 mpsc 旁路通道确保投递,对齐 §6.2 红线 5。
            | Self::R2FreezeViolation { .. }
            | Self::R2FreezeRollbackFailed { .. }
            // MCA M0(ADR-065):厂商额度耗尽为 Critical
            // WHY:额度耗尽 = 通道即刻不可用,丢失导致降级链(csn-substitutor)
            // 无人触发、请求持续打向死通道。语义对齐 BudgetExceeded(资源红线
            // 必须确保投递);bus.rs is_critical_mpsc_event() 已同步列入(双清单)。
            | Self::AffinityQuotaExhausted { .. } => EventSeverity::Critical,
            // P1-5: FormalViolation 升级为 Critical(违反即否决,丢失导致契约违反
            // 无人审议、候选继续进入后续阶段,违反九层防御 L0 语义;双清单同步见 bus.rs)
            | Self::FormalViolation { .. } => EventSeverity::Critical,
            // §16.4 跨层事件协议补齐(Phase 10 Wave 4):停止裁决与错误签名匹配
            // 为 Critical——停止裁决丢失导致 Quest 无界运行;错误修复路径丢失
            // 导致 Debug 算子无法检索同签名兄弟。双清单同步见 bus.rs(Wave 5)。
            Self::StopRulingIssued { .. } | Self::ErrorSignatureMatched { .. } => {
                EventSeverity::Critical
            }
            // 控制事件(请求/反馈):不阻断系统,不触发 mpsc 旁路投递
            Self::QuestCancelRequested { .. }
            | Self::QuestCancelled { .. }
            | Self::QuestPriorityChanged { .. }
            | Self::QuestPriorityAdjusted { .. }
            // TUI 交互式动作协议(ADR-029):请求/终态为 Info,高频流式为 Normal
            | Self::TuiActionRequested { .. }
            | Self::TuiActionCompleted { .. }
            | Self::TuiActionFailed { .. }
            | Self::TuiChatSubmitted { .. }
            | Self::TuiChatCompleted { .. } => EventSeverity::Info,
            // Concord W10 T10.1(ADR-082):协议握手为一次性信道建立事件,
            // 丢失可由 TUI 超时降级兜底,Info 级别即可
            Self::TuiHello { .. } | Self::TuiHelloAck { .. } => EventSeverity::Info,
            // TuiActionProgressed / TuiChatResponseChunk / TuiChatStatusChanged
            // 为高频流式事件,走 Normal(broadcast),由通配符分支覆盖
            // MCA P5:窗口亲和折减结果 + MCA A3:缓存亲和策略应用结果 + MCA M0:跨厂商协商(均为观测面事件)
            // WHY Normal:CrossVendorNegotiation 记录 PVL 辩论中的跨厂商去相关决策,
            // 同 WindowAffinityApplied 等观测面事件,仅用于审计与监控留痕,
            // 不阻塞系统关键路径,无需 mpsc 旁路投递。
            Self::WindowAffinityApplied { .. }
            | Self::CacheAffinityApplied { .. }
            | Self::CrossVendorNegotiation { .. }
            // P2-8 MemCon:幽灵记忆检测与策略调整(均为观测面事件,不阻断系统)
            | Self::GhostMemoryDetected { .. }
            | Self::MemConStrategyAdjusted { .. }
            | Self::BenchmarkMetricsCollected { .. } => EventSeverity::Normal,
            _ => EventSeverity::Normal,
        }
    }

    /// 事件类型名(用于序列化 tag 与日志)
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::UserIntentEncoded { .. } => "UserIntentEncoded",
            Self::NexusStateChanged { .. } => "NexusStateChanged",
            Self::ModelRouteSelected { .. } => "ModelRouteSelected",
            Self::QuestCreated { .. } => "QuestCreated",
            Self::QuestProgressUpdated { .. } => "QuestProgressUpdated",
            Self::QuestListUpdated { .. } => "QuestListUpdated",
            Self::QuestCompleted { .. } => "QuestCompleted",
            Self::ThinkingModeSwitched { .. } => "ThinkingModeSwitched",
            Self::CheckpointSaved { .. } => "CheckpointSaved",
            Self::CheckpointLoaded { .. } => "CheckpointLoaded",
            Self::ConsensusReached { .. } => "ConsensusReached",
            Self::VoteCast { .. } => "VoteCast",
            Self::CapabilityFrozen { .. } => "CapabilityFrozen",
            // L4 深度优化:影子模式熔断跳闸(severity 走通配符 Normal——
            // 熔断是 fail-closed 状态变更,非 9 类 Critical mpsc 清单)
            Self::ShadowBreakerTripped { .. } => "ShadowBreakerTripped",
            Self::BudgetExceeded { .. } => "BudgetExceeded",
            Self::SandboxViolation { .. } => "SandboxViolation",
            Self::OperationProduced { .. } => "OperationProduced",
            Self::PredictionVerified { .. } => "PredictionVerified",
            Self::OmniSparseMasksComputed { .. } => "OmniSparseMasksComputed",
            Self::ToolsRouted { .. } => "ToolsRouted",
            Self::ExecutionCompleted { .. } => "ExecutionCompleted",
            Self::MemoryMetricsReported { .. } => "MemoryMetricsReported",
            Self::MemoryTiered { .. } => "MemoryTiered",
            Self::CacheHit { .. } => "CacheHit",
            Self::CacheMiss { .. } => "CacheMiss",
            Self::WikiUpdated { .. } => "WikiUpdated",
            Self::EvolutionTriggered { .. } => "EvolutionTriggered",
            Self::DpoPairGenerated { .. } => "DpoPairGenerated",
            Self::AuditLogged { .. } => "AuditLogged",
            Self::McpMessageReceived { .. } => "McpMessageReceived",
            Self::SlowConsumerDropped { .. } => "SlowConsumerDropped",
            Self::ContextWindowSwitched { .. } => "ContextWindowSwitched",
            Self::ContextCompressed { .. } => "ContextCompressed",
            Self::CapabilityTiered { .. } => "CapabilityTiered",
            // L3 深度优化:四层统计快照(Normal 级走通配符)
            Self::CapabilityTierStatsReported { .. } => "CapabilityTierStatsReported",
            Self::BlocksRebalanced { .. } => "BlocksRebalanced",
            Self::ExpertActivated { .. } => "ExpertActivated",
            Self::ActivationThresholdAdjusted { .. } => "ActivationThresholdAdjusted",
            Self::ActivationCacheStats { .. } => "ActivationCacheStats",
            Self::GatherCompleted { .. } => "GatherCompleted",
            Self::OperationTimedOut { .. } => "OperationTimedOut",
            Self::GatherTimedOut { .. } => "GatherTimedOut",
            Self::OrphanCallDetected { .. } => "OrphanCallDetected",
            Self::ProducerStrategyAdjusted { .. } => "ProducerStrategyAdjusted",
            Self::PredictionMade { .. } => "PredictionMade",
            Self::PredictionStatsReported { .. } => "PredictionStatsReported",
            Self::PredictionRolledBack { .. } => "PredictionRolledBack",
            Self::CachePrefetched { .. } => "CachePrefetched",
            Self::CacheStatsReported { .. } => "CacheStatsReported",
            Self::ExpertRouted { .. } => "ExpertRouted",
            Self::EntropyBalanced { .. } => "EntropyBalanced",
            Self::ExpertRegistered { .. } => "ExpertRegistered",
            Self::ExpertUnregistered { .. } => "ExpertUnregistered",
            Self::DebateStarted { .. } => "DebateStarted",
            Self::SkepticVeto { .. } => "SkepticVeto",
            Self::VetoOverridden { .. } => "VetoOverridden",
            Self::RedTeamAudit { .. } => "RedTeamAudit",
            Self::BudgetAdjusted { .. } => "BudgetAdjusted",
            Self::AsaIntervention { .. } => "AsaIntervention",
            Self::AhirtProbeCompleted { .. } => "AhirtProbeCompleted",
            Self::RoleRegistered { .. } => "RoleRegistered",
            Self::BudgetStatsReported { .. } => "BudgetStatsReported",
            Self::BudgetMetricsUpdated { .. } => "BudgetMetricsUpdated",
            Self::NmcEncoded { .. } => "NmcEncoded",
            Self::ChtcToolCallReceived { .. } => "ChtcToolCallReceived",
            Self::SsraFusionCompleted { .. } => "SsraFusionCompleted",
            Self::GsoePolicyUpdated { .. } => "GsoePolicyUpdated",
            Self::LsctTierSwitched { .. } => "LsctTierSwitched",
            Self::McpMeshTransactionCompleted { .. } => "McpMeshTransactionCompleted",
            Self::CsnSubstitutionTriggered { .. } => "CsnSubstitutionTriggered",
            Self::SesaActivationCompleted { .. } => "SesaActivationCompleted",
            Self::EfficiencyAlertTriggered { .. } => "EfficiencyAlertTriggered",
            Self::QuestPauseRequested { .. } => "QuestPauseRequested",
            Self::QuestResumeRequested { .. } => "QuestResumeRequested",
            Self::VoteCastRequested { .. } => "VoteCastRequested",
            Self::RefreshStateRequested { .. } => "RefreshStateRequested",
            Self::QuestPaused { .. } => "QuestPaused",
            Self::QuestResumed { .. } => "QuestResumed",
            Self::DecayMetricsReported { .. } => "DecayMetricsReported",
            Self::RouterStatsReported { .. } => "RouterStatsReported",
            Self::McpNodeHeartbeat { .. } => "McpNodeHeartbeat",
            Self::ChtcAdapterStatus { .. } => "ChtcAdapterStatus",
            Self::ClvSnapshotReported { .. } => "ClvSnapshotReported",
            Self::QuestCancelRequested { .. } => "QuestCancelRequested",
            Self::QuestCancelled { .. } => "QuestCancelled",
            Self::QuestPriorityChanged { .. } => "QuestPriorityChanged",
            Self::QuestPriorityAdjusted { .. } => "QuestPriorityAdjusted",
            // CHIMERA-MAS Agent 事件(Task 4,ADR-026)
            Self::AgentTaskDelegated { .. } => "AgentTaskDelegated",
            Self::AgentTaskCompleted { .. } => "AgentTaskCompleted",
            Self::AgentTaskFailed { .. } => "AgentTaskFailed",
            Self::AgentConsultRequested { .. } => "AgentConsultRequested",
            Self::AgentConsultResponded { .. } => "AgentConsultResponded",
            Self::AgentHeartbeat { .. } => "AgentHeartbeat",
            Self::AgentContextOverflow { .. } => "AgentContextOverflow",
            // TUI 交互式动作协议(ADR-029)
            Self::TuiActionRequested { .. } => "TuiActionRequested",
            Self::TuiActionProgressed { .. } => "TuiActionProgressed",
            Self::TuiActionCompleted { .. } => "TuiActionCompleted",
            Self::TuiActionFailed { .. } => "TuiActionFailed",
            Self::TuiChatSubmitted { .. } => "TuiChatSubmitted",
            Self::TuiChatResponseChunk { .. } => "TuiChatResponseChunk",
            Self::TuiChatCompleted { .. } => "TuiChatCompleted",
            Self::TuiChatStatusChanged { .. } => "TuiChatStatusChanged",
            // Concord W10 T10.1(ADR-082):协议握手事件
            Self::TuiHello { .. } => "TuiHello",
            Self::TuiHelloAck { .. } => "TuiHelloAck",
            // P4-W16.2.2 步骤 5:R1 影子模式事件（3 个新变体）
            Self::R1ShadowRegressionDetected { .. } => "R1ShadowRegressionDetected",
            Self::R1ShadowPromotionReady { .. } => "R1ShadowPromotionReady",
            Self::R1ShadowRollbackFailed { .. } => "R1ShadowRollbackFailed",
            // P5.2.3: SpecRegistered 事件
            Self::SpecRegistered { .. } => "SpecRegistered",
            // ADR-042 决策 4:R2 冻结违反处置事件(2 个新变体)
            Self::R2FreezeViolation { .. } => "R2FreezeViolation",
            Self::R2FreezeRollbackFailed { .. } => "R2FreezeRollbackFailed",
            // P2-1: 协调成本/推理增益比值报告(三重悖论推理悖论红线度量)
            Self::CoordinationRatioReported { .. } => "CoordinationRatioReported",
            // polish-v2.7 P1-2: RuntimeAuditor 审计事件(2 个新变体)
            Self::AuditFindingRaised { .. } => "AuditFindingRaised",
            Self::HarnessReportGenerated { .. } => "HarnessReportGenerated",
            // L8 协调度量接线闭环:观测事件(2 个新变体,Normal 级走通配符)
            Self::DebateCompleted { .. } => "DebateCompleted",
            Self::DelegationCompleted { .. } => "DelegationCompleted",
            // L8 推理悖论风控:策略封顶变更(Normal 级走通配符)
            Self::ParliamentStrategyCapChanged { .. } => "ParliamentStrategyCapChanged",
            // MCA M0(ADR-065):mca-gateway 事件(6 个新变体,仅 AffinityQuotaExhausted 为 Critical)
            Self::ModelAffinitySelected { .. } => "ModelAffinitySelected",
            // MCA P2-1:跨厂商辩论通道选择
            Self::CrossVendorNegotiation { .. } => "CrossVendorNegotiation",
            Self::ProviderDegraded { .. } => "ProviderDegraded",
            Self::AffinityCapabilityNegotiated { .. } => "AffinityCapabilityNegotiated",
            Self::AffinityQuotaExhausted { .. } => "AffinityQuotaExhausted",
            Self::AffinityUnknownField { .. } => "AffinityUnknownField",
            Self::StreamSessionCompleted { .. } => "StreamSessionCompleted",
            // MCA P5:窗口亲和折减结果
            Self::WindowAffinityApplied { .. } => "WindowAffinityApplied",
            // MCA A3:缓存亲和策略应用结果
            Self::CacheAffinityApplied { .. } => "CacheAffinityApplied",
            // ADR-069: Token 效率优化事件
            Self::ContextBudgetAllocated { .. } => "ContextBudgetAllocated",
            Self::SemanticCacheHit { .. } => "SemanticCacheHit",
            // P2-8 MemCon:幽灵记忆检测
            Self::GhostMemoryDetected { .. } => "GhostMemoryDetected",
            // P2-8 MemCon:策略调整
            Self::MemConStrategyAdjusted { .. } => "MemConStrategyAdjusted",
            Self::BenchmarkMetricsCollected { .. } => "BenchmarkMetricsCollected",
            // PROBE P0:HCW 召回评测事件
            Self::HcwRecallReported { .. } => "HcwRecallReported",
            Self::HcwRecallDegraded { .. } => "HcwRecallDegraded",
            Self::OverWindowFallbackTriggered { .. } => "OverWindowFallbackTriggered",
            Self::ResourceRecovered { .. } => "ResourceRecovered",
            Self::FormalViolation { .. } => "FormalViolation",
            Self::RewardSignalReported { .. } => "RewardSignalReported",
            // §16.4 跨层事件协议补齐(Phase 10 Wave 4)
            Self::StopRulingIssued { .. } => "StopRulingIssued",
            Self::VariantApproved { .. } => "VariantApproved",
            Self::ParentSelected { .. } => "ParentSelected",
            Self::ErrorSignatureMatched { .. } => "ErrorSignatureMatched",
            Self::TokenLedgerRecorded { .. } => "TokenLedgerRecorded",
            Self::AssessmentUpdated { .. } => "AssessmentUpdated",
            // §16.5 L1 吞吐量观测(Phase 10 Wave 6)
            Self::BusThroughputReported { .. } => "BusThroughputReported",
            // §16.5 L4 沙箱拦截率观测(Phase 10 Wave 6)
            Self::SecurityInterceptionReported { .. } => "SecurityInterceptionReported",
        }
    }
}
