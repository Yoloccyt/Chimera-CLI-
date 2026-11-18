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
//! - **错位/定级错误**:由 tests/variant_count_test.rs 三层锁守护(计数锁 + 序数表交叉锁 + 三档分布锁 + Critical 名单与 bus.rs `LANE_FORBIDDEN_SHARD` 双向互锁;锁定数值以该测试文件常量为单一权威,散文不重复登记),注册表任一属性笔误都会使对应锁测试红灯;
//! - **metadata()**:所有变体第一字段均为 `metadata: EventMetadata`,
//!   注册表逐变体生成取元数据臂,与手写表逐字等价。
//!
//! # 双清单同步红线(刻意保留的人工清单)
//! bus.rs `is_critical_mpsc_event`(18 个 mpsc 旁路变体)与
//! `LANE_FORBIDDEN_SHARD`(22 个 Critical 变体名)**不**由本注册表生成,
//! 维持独立手写清单 —— 这是设计特性而非疏漏:两张独立清单 + 守护测试
//! 互锁(bus.rs `test_mpsc_required_events_hit_both_lists` 等)使
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
            /// 由 `define_event_registry!` 从注册表展开,全部变体(计数以
            /// tests/variant_count_test.rs 的 `VARIANT_COUNT_LOCK` 为权威)
            /// 显式定级（三档分布数值以同文件 `CRITICAL_LOCK`/`INFO_LOCK`/
            /// `NORMAL_LOCK` 常量为权威）,无通配符兜底:
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
            /// 由 `define_event_registry!` 从注册表展开,全部变体(计数以
            /// `VARIANT_COUNT_LOCK` 为权威)映射到
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

            /// 获取事件元数据的可变引用（O-4 / trace-id 传播基础件）
            ///
            /// 与 `metadata()` 同由 `define_event_registry!` 逐变体展开（第一字段均为
            /// `metadata`）。供传播点在**显式**设/继承 trace_id 与 correlation_id 时使用。
            /// 注：**故意不在 publish 路径自动 `ensure_trace_id`**——那会给发出的事件补
            /// trace_id 而使 `assert_eq!(recv, sent)` 类事件相等断言回归（sent 无、recv 有）；
            /// trace 传播须由发起方显式 `with_trace`/`ensure_trace_id`（保留事件不变性）。
            pub fn metadata_mut(&mut self) -> &mut EventMetadata {
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
    // [Critical·broadcast] 检查点保存:背压保护级别,不属 mpsc 旁路清单
    // (旁路成员的唯一权威是 `bus::is_critical_mpsc_event`,规模由
    // `CRITICAL_MPSC_VARIANTS` 持有;本行不复述数字,避免注释与常量漂移——
    // 历史上此处曾长期挂着已变的旧计数「13」。)
    CheckpointSaved => Critical, Quest;
    CheckpointLoaded => Normal, Quest;
    // [Critical·broadcast] 共识达成:同上,历史 Critical 只走 broadcast
    ConsensusReached => Critical, Parliament;
    VoteCast => Normal, Parliament;
    // L4 Security → L8 Parliament:能力冻结
    CapabilityFrozen => Normal, Security;
    // [Critical·mpsc] 影子模式熔断器永久跳闸(C-13/M09,2026-10-25 升级):
    // 不可逆 fail-closed 状态变更必须确保投递——丢失则 TUI/治理面无从感知
    // 熔断已生效(与 FormalVerificationFailed 同语义,双清单同步见 bus.rs;
    // 两生产方调用点不动:gsoe publish_critical 本就无条件入旁路,decay
    // publish_blocking 经 is_critical_mpsc_event 新臂自动入旁路)
    ShadowBreakerTripped => Critical, Security;
    // [Critical·mpsc] L4 深度优化 P1-1:形式化验证失败必须确保投递——
    // 丢失则被否决的违规候选继续进入后续阶段(与 FormalViolation 同语义,
    // 对齐九层防御 L0 + 进化悖论 L3→L4 跃迁红线;双清单同步见 bus.rs)
    FormalVerificationFailed => Critical, Security;
    // [Critical·mpsc] 预算耗尽 = 系统红线(Hard Constraint 第 10 条,F-001):
    // 资源达上限必须立即触发背压保护并通知 Parliament,标 Normal 会在
    // 背压场景被丢弃,导致超限无人响应、Quest 持续消耗直至 OOM
    BudgetExceeded => Critical, Parliament;
    // L4 Security → L9 Quest:沙箱违规
    // C-64b（M49 后半）：沙箱违规是**安全类** Critical——与 R2Freeze/Formal 同族，
    // 拦截一旦丢失，攻击面即无痕。定级 Normal→Critical，并同批同步 bus.rs 的
    // mpsc 旁路 / 计数 / 分片禁区三处与 spec 冻结面（G-117 parity）。
    SandboxViolation => Critical, Security;
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
    // HCW v3 蓝图 P0/P1(ADR-290):能力档案更新与窗口计划留痕(观测面)
    ContextCapabilityUpdated => Normal, System;
    WindowPlanApplied => Normal, System;
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
    // ADR-197:L1 计算池饱和观测面事件（由装配层周期上报；非 Critical、不进分片禁区）
    ComputePoolSaturated => Normal, System;
    // ADR-223 D2:L0-B 遥测治理面事件（校准写表结果 / 启动期配置降级；观测面，非 Critical）
    TelemetryCalibrationApplied => Normal, System;
    TelemetryReporterDegraded => Normal, System;
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
    // 定级 Normal —— 审议记录非"必须确保投递"类,故不属 mpsc 旁路清单;
    // 清单规模只由 `CRITICAL_MPSC_VARIANTS` 持有,本行不复述数字)
    VariantApproved => Normal, Parliament;
    // §16.4(Phase 10 Wave 4):三因子父本选择结果(L5 → L6/L9)
    ParentSelected => Normal, Knowledge;
    // [Critical·mpsc] §16.4:错误签名匹配丢失导致 Debug 算子无法检索同签名兄弟
    ErrorSignatureMatched => Critical, Security;
    // NOTE(B5/ADR-312): TokenLedgerRecorded 已随 token_ledger 子系统退役
    // （零生产发布点；变体锁 156→155 同批）。
    // §16.4:自我评估更新(L10 RuntimeAuditor → L9)
    AssessmentUpdated => Normal, System;
    // §16.5 L1 吞吐量观测(Phase 10 Wave 6)
    BusThroughputReported => Normal, System;
    // §16.5 L4 沙箱拦截率观测(Phase 10 Wave 6)
    SecurityInterceptionReported => Normal, System;
    // v2.3 (b.1): 裁决权分离三事件(Claim/Certify/Declaim) + (b.4) FailureRegistry
    // Claim/Declaim 为广播级(agent 可写);Certify 走 Critical 旁路(验证器语义独占);
    // FailureRegistered 走 Critical 旁路(丢失=全 swarm 重复踩坑)。mpsc 旁路同步见 bus.rs。
    ClaimIssued => Normal, Parliament;
    ClaimRetracted => Normal, Parliament;
    CertifyIssued => Critical, Parliament;
    FailureRegistered => Critical, Security;
    // v2.3 (b.2): 黑板条目 —— 广播级 Normal,不走 mpsc 旁路(Critical 清单不变)
    BlackboardEntry => Normal, Memory;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nexus_contracts::behavior_contract::ContractContext;

    /// Critical 变体规模 —— 与 `event-bus` 侧常量的**对齐点**。
    ///
    /// WHY 单一权威:全量 155 变体的总数与三档分布(Critical/Info/Normal)由
    /// `crates/event-bus/tests/variant_count_test.rs` 的
    /// `VARIANT_COUNT_LOCK`/`CRITICAL_LOCK` 持有,本 crate **不重复**那份
    /// 155 条构造清单(两份清单会各自漂移)。此处只锁「本文件 severity() 判为
    /// Critical 的构造样本集合」的规模,与 `event-bus/src/bus/mod.rs` 的
    /// `CRITICAL_TOTAL` 同值;任何定级改动(如 SandboxViolation
    /// Normal→Critical)都会让本断言与外部锁同时红灯。
    const EXPECTED_CRITICAL_TOTAL: usize = 22;

    /// 全部 22 个 Critical 变体的最小构造样本。
    ///
    /// 名单 = 注册表中 `Critical` 定级行。其中 18 个是 `bus::is_critical_mpsc_event()`
    /// 的 mpsc 旁路成员;差的 4 个(CheckpointSaved / ConsensusReached /
    /// SlowConsumerDropped / OrphanCallDetected)历史上只走 broadcast,
    /// 故本集合 ⊇ mpsc 旁路集合。
    ///
    /// WHY 逐变体构造而非枚举反射:Rust 无枚举反射,"全部变体"在运行时只能
    /// 来自人工清单;构造器与 enum 字段一一对应 ⇒ 变体改名/删字段立即编译错误。
    fn critical_samples() -> Vec<NexusEvent> {
        vec![
            NexusEvent::CheckpointSaved {
                metadata: EventMetadata::new("probe"),
                quest_id: "q".into(),
                checkpoint_id: "c".into(),
                memory_snapshot_hash: "h".into(),
            },
            NexusEvent::ConsensusReached {
                metadata: EventMetadata::new("probe"),
                quest_id: "q".into(),
                decision_hash: "h".into(),
                dpo_pair_id: None,
            },
            NexusEvent::ShadowBreakerTripped {
                metadata: EventMetadata::new("probe"),
                reason: "r".into(),
            },
            NexusEvent::FormalVerificationFailed {
                metadata: EventMetadata::new("probe"),
                property: "p".into(),
                counterexample: "ce".into(),
                generation: 1,
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("probe"),
                budget_type: "b".into(),
                current: 1,
                limit: 1,
            },
            NexusEvent::SandboxViolation {
                metadata: EventMetadata::new("probe"),
                violation_type: "v".into(),
                detail: "d".into(),
            },
            NexusEvent::SlowConsumerDropped {
                metadata: EventMetadata::new("probe"),
                subscriber_id: "s".into(),
                lag: 1,
                dropped_count: 1,
            },
            NexusEvent::OrphanCallDetected {
                metadata: EventMetadata::new("probe"),
                operation_id: "o".into(),
                spawn_location: "l".into(),
            },
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("probe"),
                quest_id: "q".into(),
                veto_reason: "r".into(),
                frozen_capabilities: vec![],
            },
            NexusEvent::VetoOverridden {
                metadata: EventMetadata::new("probe"),
                quest_id: "q".into(),
                proposal_id: "p".into(),
                veto_reason: "vr".into(),
                override_reason: "or".into(),
                override_by: "ob".into(),
            },
            NexusEvent::RedTeamAudit {
                metadata: EventMetadata::new("probe"),
                vulnerability_type: "v".into(),
                failed_probes: 1,
                total_probes: 1,
                detection_rate: 0.5,
                remediation_suggestion: "s".into(),
            },
            NexusEvent::AsaIntervention {
                metadata: EventMetadata::new("probe"),
                operation_id: "o".into(),
                action: "a".into(),
                safety_score: 0.5,
                block_reason: None,
                alternative_suggestion: None,
            },
            NexusEvent::AgentTaskFailed {
                metadata: EventMetadata::new("probe"),
                from: "a".into(),
                to: "b".into(),
                task_id: "t".into(),
                error: "e".into(),
                retry_count: 1,
            },
            NexusEvent::R1ShadowRollbackFailed {
                metadata: EventMetadata::new("probe"),
                reason: "r".into(),
                trigger_type: crate::payloads::RollbackTriggerType::Unknown,
                triggered_at: None,
                details: "d".into(),
                diagnostic: crate::payloads::RollbackDiagnosticContext::default(),
            },
            NexusEvent::R2FreezeViolation {
                metadata: EventMetadata::new("probe"),
                violation_type: "v".into(),
                evidence: "e".into(),
            },
            NexusEvent::R2FreezeRollbackFailed {
                metadata: EventMetadata::new("probe"),
                reason: "r".into(),
            },
            NexusEvent::AffinityQuotaExhausted {
                metadata: EventMetadata::new("probe"),
                route_key: "k".into(),
                reason: "r".into(),
            },
            NexusEvent::FormalViolation {
                metadata: EventMetadata::new("probe"),
                contract_id: "c".into(),
                target_type: "t".into(),
                violations: vec![],
                context: ContractContext::Runtime,
            },
            NexusEvent::StopRulingIssued {
                metadata: EventMetadata::new("probe"),
                quest_id: "q".into(),
                reason: "r".into(),
                preserve_best: true,
            },
            NexusEvent::ErrorSignatureMatched {
                metadata: EventMetadata::new("probe"),
                error_hash: "h".into(),
                matched_card_ids: vec![],
            },
            NexusEvent::CertifyIssued {
                metadata: EventMetadata::new("probe"),
                claim_ref: "c".into(),
                subject: "s".into(),
                verifier: "v".into(),
            },
            NexusEvent::FailureRegistered {
                metadata: EventMetadata::new("probe"),
                signature: "s".into(),
                affected_component: "c".into(),
                fix_status: "f".into(),
            },
        ]
    }

    #[test]
    fn critical_severity_set_matches_expected_total() {
        let samples = critical_samples();
        assert_eq!(
            samples.len(),
            EXPECTED_CRITICAL_TOTAL,
            "Critical 构造样本数与 EXPECTED_CRITICAL_TOTAL 不一致"
        );
        for event in &samples {
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 在注册表中应为 Critical",
                event.type_name()
            );
        }
    }

    #[test]
    fn broadcast_only_criticals_remain_critical() {
        // 这 4 个是 severity()=Critical 但不在 mpsc 旁路清单的历史成员:
        // 若有人把它们降级为 Normal,双清单"⊇"关系就被破坏(broadcast 集会
        // 变成 mpsc 集的真子集之外),此处作为本 crate 内的定级锚点。
        let names: Vec<&str> = critical_samples()
            .iter()
            .filter(|e| {
                matches!(
                    e.type_name(),
                    "CheckpointSaved"
                        | "ConsensusReached"
                        | "SlowConsumerDropped"
                        | "OrphanCallDetected"
                )
            })
            .map(|e| e.type_name())
            .collect();
        assert_eq!(names.len(), 4, "4 个历史 broadcast-only Critical 未被识别");
        assert!(critical_samples()
            .iter()
            .filter(|e| names.contains(&e.type_name()))
            .all(|e| e.severity() == EventSeverity::Critical));
    }

    #[test]
    fn type_name_is_non_empty_and_unique_over_critical_samples() {
        let mut seen = std::collections::HashSet::new();
        for event in critical_samples() {
            let name = event.type_name();
            assert!(!name.is_empty(), "type_name() 不得为空");
            assert!(name.starts_with(char::is_uppercase), "{name} 应为变体名");
            assert!(seen.insert(name), "type_name() 重复: {name}");
        }
        assert_eq!(seen.len(), EXPECTED_CRITICAL_TOTAL);
    }

    #[test]
    fn severity_and_topic_land_in_legal_sets() {
        let legal_topics = crate::topic::EventTopic::all();
        for event in critical_samples() {
            assert!(
                matches!(
                    event.severity(),
                    EventSeverity::Critical | EventSeverity::Info | EventSeverity::Normal
                ),
                "{} 的 severity 落在合法枚举集之外",
                event.type_name()
            );
            assert!(
                legal_topics.contains(&event.topic()),
                "{} 的 topic 不在 EventTopic 的合法变体集内",
                event.type_name()
            );
        }
    }

    #[test]
    fn metadata_returns_the_first_field_and_is_mutable() {
        for mut event in critical_samples() {
            assert_eq!(
                event.metadata().source,
                "probe",
                "{} 的 metadata() 未返回构造时传入的 EventMetadata",
                event.type_name()
            );
            event.metadata_mut().source = "rewritten".into();
            assert_eq!(
                event.metadata().source,
                "rewritten",
                "{} 的 metadata_mut() 未写回同一字段",
                event.type_name()
            );
        }
    }

    #[test]
    fn samples_cover_all_ten_topics() {
        // 7 个 Critical 主题 + 3 个仅由 Normal 样本覆盖的主题
        // (Routing/Memory/Storage) —— 保证 topic() 十条臂都被至少一个
        // 构造样本走过,而不是只靠外部 155 变体清单兜底。
        let mut samples = critical_samples();
        samples.push(NexusEvent::ExpertRegistered {
            metadata: EventMetadata::new("probe"),
            tool_id: "t".into(),
        });
        samples.push(NexusEvent::NmcEncoded {
            metadata: EventMetadata::new("probe"),
            modality: "Text".into(),
            content_hash: "h".into(),
            clv_dimension: 512,
        });
        samples.push(NexusEvent::CacheHit {
            metadata: EventMetadata::new("probe"),
            cache_key: "k".into(),
        });

        let covered: std::collections::HashSet<crate::topic::EventTopic> =
            samples.iter().map(|e| e.topic()).collect();
        assert_eq!(
            covered.len(),
            10,
            "构造样本应覆盖全部 10 类 EventTopic,实际覆盖 {covered:?}"
        );
        for event in &samples[EXPECTED_CRITICAL_TOTAL..] {
            assert_ne!(
                event.severity(),
                EventSeverity::Critical,
                "{} 是 Normal 样本,不应被判为 Critical",
                event.type_name()
            );
        }
    }

    #[test]
    fn type_name_matches_variant_identifier_for_sampled_variants() {
        // WHY 手写期望串:type_name() 由 `stringify!` 生成,变体改名而注册表
        // 行照抄旧名时,编译器只会在"枚举里没有这个变体"时报警——若有人把
        // 变体与注册表行同时改名,type_name() 会静默跟着变。跨进程线格式的
        // `type` tag 直接取该值,所以这 12 条逐字对照是名单漂移的嗅探器。
        let meta = || EventMetadata::new("probe");
        let samples: Vec<(NexusEvent, &str)> = vec![
            (
                NexusEvent::QuestCreated {
                    metadata: meta(),
                    quest_id: "q".into(),
                    title: "t".into(),
                    task_count: 1,
                },
                "QuestCreated",
            ),
            (
                NexusEvent::UserIntentEncoded {
                    metadata: meta(),
                    intent_id: "i".into(),
                    raw_text: "r".into(),
                    risk_level: 0,
                },
                "UserIntentEncoded",
            ),
            (
                NexusEvent::CheckpointSaved {
                    metadata: meta(),
                    quest_id: "q".into(),
                    checkpoint_id: "c".into(),
                    memory_snapshot_hash: "h".into(),
                },
                "CheckpointSaved",
            ),
            (
                NexusEvent::BudgetExceeded {
                    metadata: meta(),
                    budget_type: "token".into(),
                    current: 2,
                    limit: 1,
                },
                "BudgetExceeded",
            ),
            (
                NexusEvent::SkepticVeto {
                    metadata: meta(),
                    quest_id: "q".into(),
                    veto_reason: "r".into(),
                    frozen_capabilities: vec![],
                },
                "SkepticVeto",
            ),
            (
                NexusEvent::CertifyIssued {
                    metadata: meta(),
                    claim_ref: "c".into(),
                    subject: "s".into(),
                    verifier: "v".into(),
                },
                "CertifyIssued",
            ),
            (
                NexusEvent::AgentHeartbeat {
                    metadata: meta(),
                    from: "a".into(),
                    status: crate::payloads::AgentStatus::Idle,
                    current_task: None,
                    token_usage: 0,
                    memory_usage_mb: 0,
                },
                "AgentHeartbeat",
            ),
            (
                NexusEvent::TuiChatSubmitted {
                    metadata: meta(),
                    session_id: "s".into(),
                    query: "q".into(),
                    slash_command: None,
                },
                "TuiChatSubmitted",
            ),
            (
                NexusEvent::R2FreezeViolation {
                    metadata: meta(),
                    violation_type: "v".into(),
                    evidence: "e".into(),
                },
                "R2FreezeViolation",
            ),
            (
                NexusEvent::StopRulingIssued {
                    metadata: meta(),
                    quest_id: "q".into(),
                    reason: "r".into(),
                    preserve_best: true,
                },
                "StopRulingIssued",
            ),
            (
                NexusEvent::FailureRegistered {
                    metadata: meta(),
                    signature: "s".into(),
                    affected_component: "c".into(),
                    fix_status: "f".into(),
                },
                "FailureRegistered",
            ),
            (
                NexusEvent::BlackboardEntry {
                    metadata: meta(),
                    key: "k".into(),
                    summary: "s".into(),
                    ttl_secs: 1,
                    epistemic: "e".into(),
                    provenance: None,
                },
                "BlackboardEntry",
            ),
        ];
        for (event, expected) in samples {
            assert_eq!(
                event.type_name(),
                expected,
                "type_name() 与变体标识符不一致 —— serde `type` tag 会跟着漂移"
            );
        }
    }

    #[test]
    fn known_critical_variants_report_critical_severity() {
        // 三条红线级定级的显式锚点(与 bus.rs 的 mpsc 旁路清单独立):
        // BudgetExceeded = 预算红线,BudgetExceeded 降级会直接触发 OOM 路径;
        // SkepticVeto = 安全否决,SkepticVeto 降级会让高风险操作继续;
        // CertifyIssued = 验证器独占签发,CertifyIssued 降级等于"生成者自评"。
        for event in [
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("probe"),
                budget_type: "token".into(),
                current: 1,
                limit: 1,
            },
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("probe"),
                quest_id: "q".into(),
                veto_reason: "r".into(),
                frozen_capabilities: vec![],
            },
            NexusEvent::CertifyIssued {
                metadata: EventMetadata::new("probe"),
                claim_ref: "c".into(),
                subject: "s".into(),
                verifier: "v".into(),
            },
        ] {
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 必须定级 Critical(架构红线不可降级)",
                event.type_name()
            );
            assert!(
                EventTopic::all().contains(&event.topic()),
                "{} 的 topic 不在 EventTopic 合法集内",
                event.type_name()
            );
        }
    }
}
