//! 事件注册表 — NexusEvent 分类属性的单一声明点(M5,架构重构方向 3-P1 同步点收敛)
//!
//! 对应架构层:L1 Core(event-bus)
//!
//! # WHY(本模块存在的理由)
//! 历史上 `NexusEvent` 的分类属性散在四张人工维护的巨型 match 里,新增一个
//! 变体需要同步 4 处(types.rs `metadata()`、classification.rs `severity()`/
//! `type_name()`、topic.rs `topic()`),types.rs 注释曾自认"新增变体需 4 处
//! 同步"。本模块用 `define_event_registry!` 宏把这四张表收敛为**单一声明点**:
//! 每个变体一行 `变体名 => severity, topic;`,宏展开同时生成四个无通配符
//! 的 match。enum 本体(字段/serde 属性)仍由 types.rs 声明 —— 注册表与
//! enum 的耦合由编译器穷举检查强制:注册表漏登变体 → 非穷尽 match 编译错误;
//! 注册表重名/错名 → unreachable pattern / 无此变体编译错误。
//!
//! # 防护强度模型(与 M1 fail-closed 等价,不降级)
//! - **漏变体**:四个生成 match 均无 `_` 兜底,编译期拦截(M1 的
//!   fail-closed 属性原样保留);
//! - **重名**:同一变体在注册表登记两次 → 展开出重复 match 臂,
//!   `unreachable_patterns` 在 `-D warnings` 下即编译失败;
//! - **错位/定级错误**:由 tests/variant_count_test.rs 三层锁守护
//!   (146 计数锁 + 序数表交叉锁 + 18/11/117 分布锁 + Critical 名单与
//!   bus.rs `LANE_FORBIDDEN_SHARD` 双向互锁),注册表任一属性笔误都会
//!   使对应锁测试红灯;
//! - **metadata()**:所有变体第一字段均为 `metadata: EventMetadata`,
//!   注册表逐变体生成取元数据臂,与手写表逐字等价。
//!
//! # 双清单同步红线(刻意保留的人工清单)
//! bus.rs `is_critical_mpsc_event`(14 个 mpsc 旁路变体)与
//! `LANE_FORBIDDEN_SHARD`(18 个 Critical 变体名)**不**由本注册表生成,
//! 维持独立手写清单 —— 这是设计特性而非疏漏:两张独立清单 + 守护测试
//! 互锁(bus.rs `test_critical_severity_implies_mpsc_bypass` 等)使
//! "severity 定级"与"mpsc 旁路判定"的漂移可被双向捕获;若二者同源于
//! 注册表,互锁测试将退化为自比较而失去意义。

use crate::topic::EventTopic;
use crate::types::{EventMetadata, EventSeverity, NexusEvent};

/// 单一声明点展开宏 — 变体分类属性注册表(见模块文档)
///
/// 每行一条:`VariantName => Severity, Topic;`
/// - `VariantName`:NexusEvent 变体名(同时也是 `type_name()` 返回值,
///   经 `stringify!` 生成,杜绝名-串分离笔误);
/// - `Severity`:`Critical` / `Info` / `Normal`(映射 `EventSeverity::$severity`);
/// - `Topic`:EventTopic 变体名(映射 `EventTopic::$topic`)。
///
/// 展开产物:`impl NexusEvent` 的 severity()/type_name()/topic()/metadata()
/// 四个 match(均无通配符兜底)。条目之间的 `//` 注释会被宏解析忽略,
/// 可自由承载定级理据(WHY)。
macro_rules! define_event_registry {
    ($($variant:ident => $severity:ident, $topic:ident;)+) => {
        impl NexusEvent {
            /// 判断事件是否为关键事件(Critical)
            ///
            /// 由 `define_event_registry!` 从注册表展开,全部 146 个变体
            /// 显式定级(Critical 18 / Info 11 / Normal 117),无通配符兜底:
            /// 新增变体不显式定级即编译错误(fail-closed,M1 属性)。
            /// Critical 语义与红线文档见模块文档;旁路通道判定见 bus.rs
            /// `is_critical_mpsc_event`(双清单同步红线)。
            pub fn severity(&self) -> EventSeverity {
                match self {
                    $(Self::$variant { .. } => EventSeverity::$severity,)+
                }
            }

            /// 事件类型名(用于序列化 tag 与日志)
            ///
            /// 由 `define_event_registry!` 经 `stringify!` 展开,返回值与
            /// serde 线格式 `type` tag 同源同值。
            pub fn type_name(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => stringify!($variant),)+
                }
            }

            /// 获取事件所属主题(10 类 EventTopic)
            ///
            /// 由 `define_event_registry!` 从注册表展开,146 变体映射到
            /// 10 类 topic,无通配符兜底,新增变体编译器强制登记。
            pub fn topic(&self) -> EventTopic {
                match self {
                    $(Self::$variant { .. } => EventTopic::$topic,)+
                }
            }

            /// 获取事件元数据引用
            ///
            /// 由 `define_event_registry!` 从注册表展开;所有变体第一字段
            /// 均为 `metadata: EventMetadata`,逐变体生成取元数据臂。
            pub fn metadata(&self) -> &EventMetadata {
                match self {
                    $(Self::$variant { metadata, .. } => metadata,)+
                }
            }
        }
    };
}

define_event_registry! {
    // ============================================================
    // L10 Interface → L9 Quest:用户意图编码完成
    // ============================================================
    UserIntentEncoded => Normal, Quest;
    // L1 Core → L2 Memory:全局状态变更
    NexusStateChanged => Normal, Memory;
    // L1 Core → L9 Quest:模型路由选定
    ModelRouteSelected => Normal, Quest;
    // L9 Quest → L8 Parliament:任务生命周期
    QuestCreated => Normal, Quest;
    QuestProgressUpdated => Normal, Quest;
    QuestListUpdated => Normal, Quest;
    QuestCompleted => Normal, Quest;
    ThinkingModeSwitched => Normal, Quest;
    // [Critical·broadcast] 检查点保存:背压保护级别,非 mpsc 旁路 13 清单成员
    CheckpointSaved => Critical, Quest;
    CheckpointLoaded => Normal, Quest;
    // [Critical·broadcast] 共识达成:同上,历史 Critical 只走 broadcast
    ConsensusReached => Critical, Parliament;
    VoteCast => Normal, Parliament;
    // L4 Security → L8 Parliament:能力冻结
    CapabilityFrozen => Normal, Security;
    ShadowBreakerTripped => Normal, Security;
    // [Critical·mpsc] L4 深度优化 P1-1:形式化验证失败必须确保投递——
    // 丢失则被否决的违规候选继续进入后续阶段(与 FormalViolation 同语义,
    // 对齐九层防御 L0 + 进化悖论 L3→L4 跃迁红线;双清单同步见 bus.rs)
    FormalVerificationFailed => Critical, Security;
    // [Critical·mpsc] 预算耗尽 = 系统红线(Hard Constraint 第 10 条,F-001):
    // 资源达上限必须立即触发背压保护并通知 Parliament,标 Normal 会在
    // 背压场景被丢弃,导致超限无人响应、Quest 持续消耗直至 OOM
    BudgetExceeded => Critical, Parliament;
    // L4 Security → L9 Quest:沙箱违规
    SandboxViolation => Normal, Security;
    // L7 Execution → L6 Router:操作产出
    OperationProduced => Normal, Execution;
    PredictionVerified => Normal, Execution;
    // L6 Router → L5 Knowledge:稀疏掩码/工具路由
    OmniSparseMasksComputed => Normal, Routing;
    ToolsRouted => Normal, Routing;
    // L6 Router → L9 Quest:执行完成
    ExecutionCompleted => Normal, Execution;
    // L2 Memory → L9 Quest:记忆指标上报(修正 V2 违规)
    MemoryMetricsReported => Normal, Memory;
    MemoryTiered => Normal, Memory;
    // L3 Storage → L6 Router:缓存命中/未命中
    CacheHit => Normal, Storage;
    CacheMiss => Normal, Storage;
    // L5 Knowledge → L9 Quest:知识沉淀
    WikiUpdated => Normal, Knowledge;
    EvolutionTriggered => Normal, Knowledge;
    DpoPairGenerated => Normal, Knowledge;
    // L6 Router → L4 Security:审计日志
    AuditLogged => Normal, Security;
    // L10 Interface:MCP 网格消息
    McpMessageReceived => Normal, System;
    // [Critical·broadcast] 慢消费者丢弃:系统级告警,历史 Critical 只走 broadcast
    SlowConsumerDropped => Critical, System;
    // Week 3 扩展:HCW/CMT/KVBSR 跨层通信事件
    ContextWindowSwitched => Normal, Memory;
    ContextCompressed => Normal, Memory;
    CapabilityTiered => Normal, Memory;
    // L3 深度优化:四层统计快照(归属 Memory 臂,与 CapabilityTiered 同族;
    // 原 topic.rs 行内注释"Storage 归类"为历史误笔,实际映射以臂为准)
    CapabilityTierStatsReported => Normal, Memory;
    BlocksRebalanced => Normal, Routing;
    // Week 4 扩展:执行优化层(L6 + L7)跨层通信事件
    ExpertActivated => Normal, Routing;
    ActivationThresholdAdjusted => Normal, Routing;
    ActivationCacheStats => Normal, Routing;
    GatherCompleted => Normal, Execution;
    OperationTimedOut => Normal, Execution;
    GatherTimedOut => Normal, Execution;
    // [Critical·broadcast] 孤儿调用检测:§6.1 红线观测面,历史 Critical 只走 broadcast
    OrphanCallDetected => Critical, Execution;
    ProducerStrategyAdjusted => Normal, Execution;
    PredictionMade => Normal, Execution;
    PredictionStatsReported => Normal, Execution;
    PredictionRolledBack => Normal, Execution;
    CachePrefetched => Normal, Storage;
    CacheStatsReported => Normal, Storage;
    ExpertRouted => Normal, Routing;
    EntropyBalanced => Normal, Routing;
    ExpertRegistered => Normal, Routing;
    ExpertUnregistered => Normal, Routing;
    // Week 5 扩展(SubTask 37.1):Parliament/Security/Budget 跨层通信事件
    DebateStarted => Normal, Parliament;
    // [Critical·mpsc] 怀疑者否决:丢失导致高风险操作继续执行(§6.2 红线)
    SkepticVeto => Critical, Security;
    // [Critical·mpsc] 否决覆盖审计:P3-14/Phase 10 Wave 5 双清单对齐
    VetoOverridden => Critical, Security;
    // [Critical·mpsc] 红队审计:安全事件必须确保投递
    RedTeamAudit => Critical, Security;
    BudgetAdjusted => Normal, Parliament;
    // [Critical·mpsc] ASA 安全干预:无论 action 是 Allow/Warn/Block 均统一
    // Critical(P1-W2.1.4 修复,对齐 spec.md L186 红线;Allow/Warn 为低频事件,
    // Block 更需 Critical 投递保证 —— 丢失导致高风险操作继续执行)
    AsaIntervention => Critical, Security;
    AhirtProbeCompleted => Normal, Security;
    RoleRegistered => Normal, Parliament;
    BudgetStatsReported => Normal, Parliament;
    BudgetMetricsUpdated => Normal, Parliament;
    // Week 6 扩展:NMC 多模态编码完成事件
    NmcEncoded => Normal, Memory;
    ChtcToolCallReceived => Normal, System;
    // Week 6 扩展:SSRA 融合完成事件
    SsraFusionCompleted => Normal, Execution;
    GsoePolicyUpdated => Normal, Knowledge;
    // Week 6 扩展:LSCT 层级切换事件
    LsctTierSwitched => Normal, Storage;
    McpMeshTransactionCompleted => Normal, System;
    CsnSubstitutionTriggered => Normal, System;
    SesaActivationCompleted => Normal, Routing;
    EfficiencyAlertTriggered => Normal, System;
    // M4 扩展:TUI 双向控制请求事件(控制事件:不阻断系统,不触发 mpsc 旁路)
    QuestPauseRequested => Normal, Quest;
    QuestResumeRequested => Normal, Quest;
    VoteCastRequested => Normal, Parliament;
    RefreshStateRequested => Normal, Quest;
    QuestPaused => Normal, Quest;
    QuestResumed => Normal, Quest;
    // 控制事件(请求/反馈):不阻断系统,不触发 mpsc 旁路投递
    QuestCancelRequested => Info, Quest;
    QuestCancelled => Info, Quest;
    QuestPriorityChanged => Info, Quest;
    QuestPriorityAdjusted => Info, Quest;
    // P2.1:衰减指标报告(L4 decay-engine 发布)
    DecayMetricsReported => Normal, Security;
    // P2.3:三路由器统计聚合报告(L9 聚合发布,消费 L6 数据)
    RouterStatsReported => Normal, Routing;
    // P2.4/P2.5:MCP 节点心跳 / CHTC 适配器状态(L10 发布)
    McpNodeHeartbeat => Normal, System;
    ChtcAdapterStatus => Normal, System;
    // TUI v1.8:CLV 快照报告(NMC 编码器发布,携带 CLV 摘要)
    ClvSnapshotReported => Normal, Memory;
    // CHIMERA-MAS Agent 协作事件(ADR-026,Task 4)
    AgentTaskDelegated => Normal, Agent;
    AgentTaskCompleted => Normal, Agent;
    // [Critical·mpsc] 任务失败影响 Quest 完整性,丢失导致失败无人响应、
    // Quest 持续等待已死 Agent 结果(§6.2 红线)
    AgentTaskFailed => Critical, Agent;
    AgentConsultRequested => Normal, Agent;
    AgentConsultResponded => Normal, Agent;
    AgentHeartbeat => Normal, Agent;
    AgentContextOverflow => Normal, Agent;
    // TUI 交互式动作协议(ADR-029):请求/终态为 Info,高频流式为 Normal
    TuiActionRequested => Info, System;
    TuiActionProgressed => Normal, System;
    TuiActionCompleted => Info, System;
    TuiActionFailed => Info, System;
    TuiChatSubmitted => Info, System;
    TuiChatResponseChunk => Normal, System;
    TuiChatCompleted => Info, System;
    TuiChatStatusChanged => Normal, System;
    // FC-2(ADR-081):/compact 策展回写
    TuiChatHistoryReplaced => Normal, System;
    // Concord W10 T10.1(ADR-082):协议握手为一次性信道建立事件,
    // 丢失可由 TUI 超时降级兜底,Info 级别即可
    TuiHello => Info, System;
    TuiHelloAck => Info, System;
    // P4-W16.2.2:R1 影子模式事件(学习策略生命周期:退化/解冻/回滚)
    R1ShadowRegressionDetected => Normal, Knowledge;
    R1ShadowPromotionReady => Normal, Knowledge;
    // [Critical·mpsc] R1 影子模式回滚失败:退化策略可能仍在生效,
    // 必须投递到 SecCore 与 Parliament 紧急干预
    R1ShadowRollbackFailed => Critical, Knowledge;
    // P5.2.3:Spec 版本注册完成(L5 gsoe-evolution 发布)
    SpecRegistered => Normal, Knowledge;
    // [Critical·mpsc] ADR-042 决策 4:R2 违反等同安全事件(奖励黑客风险
    // 立即生效),必须走 mpsc 旁路确保投递,对齐 §6.2 红线 5
    R2FreezeViolation => Critical, Knowledge;
    // [Critical·mpsc] R2 回滚失败:R2 路径代码可能仍在生效
    R2FreezeRollbackFailed => Critical, Knowledge;
    // P2-1:协调成本/推理增益比值报告(三重悖论推理悖论红线度量)
    CoordinationRatioReported => Normal, Quest;
    // polish-v2.7 P1-2:RuntimeAuditor 审计事件(L9 efficiency-monitor 发布,
    // 与 EfficiencyAlertTriggered 同属跨层监控/自评类,归 System 主题)
    AuditFindingRaised => Normal, System;
    HarnessReportGenerated => Normal, System;
    // L8 协调度量接线闭环:审议完成观测事件
    DebateCompleted => Normal, Parliament;
    // L8 协调度量接线闭环:委托批次完成观测事件(chimera-mas 发布)
    DelegationCompleted => Normal, Agent;
    // L8 推理悖论风控:策略封顶变更(StrategyCapGuard 发布)
    ParliamentStrategyCapChanged => Normal, Parliament;
    // MCA M0(ADR-065):mca-gateway 会话级/治理级事件(L10 Interface 通道层)
    ModelAffinitySelected => Normal, System;
    // MCA P2-1:跨厂商辩论通道选择(parliament 发布,MCA 通道层事件)
    CrossVendorNegotiation => Normal, System;
    ProviderDegraded => Normal, System;
    AffinityCapabilityNegotiated => Normal, System;
    // [Critical·mpsc] MCA M0:厂商额度耗尽 = 通道即刻不可用,丢失导致降级链
    // (csn-substitutor)无人触发、请求持续打向死通道;语义对齐 BudgetExceeded
    AffinityQuotaExhausted => Critical, System;
    AffinityUnknownField => Normal, System;
    StreamSessionCompleted => Normal, System;
    // MCA P5:窗口亲和折减结果(观测面事件,不阻塞系统关键路径)
    WindowAffinityApplied => Normal, System;
    // MCA A3:缓存亲和策略应用结果(观测面事件)
    CacheAffinityApplied => Normal, System;
    // ADR-069:Token 效率优化事件(观测面)
    ContextBudgetAllocated => Normal, System;
    SemanticCacheHit => Normal, System;
    // P2-8 MemCon:幽灵记忆检测与策略调整(L2 Memory 子系统观测面事件)
    GhostMemoryDetected => Normal, Memory;
    MemConStrategyAdjusted => Normal, Memory;
    // 基准模式观测面事件(效率监控器发布,遥测数据归 System 主题)
    BenchmarkMetricsCollected => Normal, System;
    // PROBE P0:HCW 召回评测事件(观测面遥测,与 BenchmarkMetricsCollected 同族)
    HcwRecallReported => Normal, System;
    HcwRecallDegraded => Normal, System;
    OverWindowFallbackTriggered => Normal, System;
    // Milestone B-2:Ambient 资源恢复(与 BudgetExceeded 成对)
    ResourceRecovered => Normal, Quest;
    // [Critical·mpsc] P1-5:违反即否决,丢失导致契约违反无人审议、候选继续
    // 进入后续阶段,违反九层防御 L0 语义
    FormalViolation => Critical, Quest;
    // Milestone C-1:奖励信号流(L0 RewardSpec 统一框架)
    RewardSignalReported => Normal, Quest;
    // [Critical·mpsc] §16.4(Phase 10 Wave 4):停止裁决丢失导致 Quest 无界运行
    StopRulingIssued => Critical, Parliament;
    // §16.4(Phase 10 Wave 4):变体审议通过(L8 三因子裁决事件化;
    // 定级 Normal —— 审议记录非"必须确保投递"类,不进 mpsc 13 清单)
    VariantApproved => Normal, Parliament;
    // §16.4(Phase 10 Wave 4):三因子父本选择结果(L5 → L6/L9)
    ParentSelected => Normal, Knowledge;
    // [Critical·mpsc] §16.4:错误签名匹配丢失导致 Debug 算子无法检索同签名兄弟
    ErrorSignatureMatched => Critical, Security;
    // §16.4(Phase 10 Wave 4):Token 证据记录(L1 → L3 持久化通知)
    TokenLedgerRecorded => Normal, Storage;
    // §16.4:自我评估更新(L10 RuntimeAuditor → L9)
    AssessmentUpdated => Normal, System;
    // §16.5 L1 吞吐量观测(Phase 10 Wave 6)
    BusThroughputReported => Normal, System;
    // §16.5 L4 沙箱拦截率观测(Phase 10 Wave 6)
    SecurityInterceptionReported => Normal, System;
}
