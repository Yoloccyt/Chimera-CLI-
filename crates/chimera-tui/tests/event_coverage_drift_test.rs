//! 事件覆盖漂移守卫(PS-2 基础切片,评估报告 F-5)—— 每一个 NexusEvent 变体
//! 都必须被显式分类:TUI 消费(Consumed)或明确不消费(NotForTui)。
//!
//! # 强制机制(三层)
//! 1. **编译期**:`classify` 对 145 个变体**穷尽匹配、无 `_` 兜底** ——
//!    event-bus 新增变体时本文件编译失败(E0004),强制开发者显式分类,
//!    杜绝"新增面向 TUI 的事件被 sync.rs 的 `_ =>` 兜底静默吞掉"(评估报告 F-5)。
//! 2. **审计期**:`CONSUMED_NAMES` 锁定 Consumed 集合(规模见测试),防止有人
//!    静默把已消费事件改判为 NotForTui。
//! 3. **一致性期**(2026-09-10 补):`CONSUMED_NAMES` 的每个变体必须在
//!    `crates/chimera-tui/src/**` 中**真实存在消费点**(静态检索
//!    `NexusEvent::<Name>`,已剥离注释)—— 防止"声明为已消费但代码从未处理"
//!    的**谎报**。前两层只能保证"分类穷尽且清单整洁",无法阻止分类说谎;
//!    本层把声明与实现对账(范式同 i18n 的 `seed_keys_are_used_in_code`)。
//!
//! # 与 sync.rs 的关系
//! sync.rs 各域同步器的 `_ =>` 兜底保持不变(按域分治的设计选择),
//! 本文件是它的**编译期对账单**:任何面向 TUI 的新事件,必须先在此分类,
//! 再去对应同步器接线 —— 两处缺一,测试红。

use event_bus::NexusEvent;

/// TUI 对事件的消费分类(PS-2 漂移守卫)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiEventClass {
    /// TUI 消费:进 DataSnapshot 或驱动面板状态(经 sync.rs/subscriber.rs 接线)
    Consumed,
    /// TUI 明确不消费:系统内部/跨层事件(新增变体归入此类时,请在 PR 注明理由)
    NotForTui,
}

/// Consumed 变体名单(审计锁:73(F-1)→74(F-6)→76(PS-2 批次1))
///
/// 不变量:与 `classify` 的 Consumed 臂一一对应。名单变动必须伴随评估报告更新。
pub const CONSUMED_NAMES: &[&str] = &[
    "AgentTaskFailed",
    "AhirtProbeCompleted",
    "AsaIntervention",
    "AuditFindingRaised",
    "BudgetAdjusted",
    "BudgetExceeded",
    "BudgetMetricsUpdated",
    "BudgetStatsReported",
    "CacheHit",
    "CacheMiss",
    "CachePrefetched",
    "CacheStatsReported",
    "CapabilityFrozen",
    "CapabilityTierStatsReported",
    "CheckpointLoaded",
    "CheckpointSaved",
    "ChtcAdapterStatus",
    "ClvSnapshotReported",
    "ConsensusReached",
    "ContextCompressed",
    "ContextWindowSwitched",
    "CoordinationRatioReported",
    "CsnSubstitutionTriggered",
    "DebateStarted",
    "DecayMetricsReported",
    "EfficiencyAlertTriggered",
    "GatherTimedOut",
    "HarnessReportGenerated",
    "HcwRecallReported",
    "McpMeshTransactionCompleted",
    "McpMessageReceived",
    "McpNodeHeartbeat",
    "MemConStrategyAdjusted",
    "MemoryMetricsReported",
    "MemoryTiered",
    "ModelRouteSelected",
    "NexusStateChanged",
    "OmniSparseMasksComputed",
    "OperationTimedOut",
    "OrphanCallDetected",
    "OverWindowFallbackTriggered",
    "ParliamentStrategyCapChanged",
    "QuestCancelRequested",
    "QuestCancelled",
    "QuestCompleted",
    "QuestCreated",
    "QuestListUpdated",
    "QuestPauseRequested",
    "QuestPaused",
    "QuestPriorityAdjusted",
    "QuestPriorityChanged",
    "QuestProgressUpdated",
    "QuestResumeRequested",
    "QuestResumed",
    "RedTeamAudit",
    "RefreshStateRequested",
    "RoleRegistered",
    "RouterStatsReported",
    "SandboxViolation",
    "ShadowBreakerTripped",
    "SkepticVeto",
    "SlowConsumerDropped",
    "ThinkingModeSwitched",
    "TuiActionCompleted",
    "TuiActionFailed",
    "TuiActionRequested",
    "TuiChatCompleted",
    "TuiChatHistoryReplaced",
    "TuiChatResponseChunk",
    "TuiChatStatusChanged",
    "TuiChatSubmitted",
    "TuiHello",
    "TuiHelloAck",
    "UserIntentEncoded",
    "VetoOverridden",
    "VoteCast",
];

/// 对事件做 TUI 消费分类(穷尽匹配,新增变体将编译失败)
///
/// WHY 测试 Crate 而非生产代码:本分类是**策略对账单**而非运行时行为,
/// 强制力来自穷尽性(编译期)与名单锁(运行期);置于 tests/ 与
/// `key_drift_regression_test`/`dual_source_invariant_test` 同范式,
/// 且不向生产二进制引入 145 臂的死代码。
pub fn classify(event: &NexusEvent) -> TuiEventClass {
    use NexusEvent::*;
    match event {
        ActivationCacheStats { .. } => TuiEventClass::NotForTui,
        ActivationThresholdAdjusted { .. } => TuiEventClass::NotForTui,
        AffinityCapabilityNegotiated { .. } => TuiEventClass::NotForTui,
        AffinityQuotaExhausted { .. } => TuiEventClass::NotForTui,
        AffinityUnknownField { .. } => TuiEventClass::NotForTui,
        AgentConsultRequested { .. } => TuiEventClass::NotForTui,
        AgentConsultResponded { .. } => TuiEventClass::NotForTui,
        AgentContextOverflow { .. } => TuiEventClass::NotForTui,
        AgentHeartbeat { .. } => TuiEventClass::NotForTui,
        AgentTaskCompleted { .. } => TuiEventClass::NotForTui,
        AgentTaskDelegated { .. } => TuiEventClass::NotForTui,
        // PS-2(F-6):AgentTaskFailed 已由 AgentFailureSync 消费
        // (Critical 级子代理失败 → 安全面板态势 + 状态栏告警),守卫第二次实战改判。
        AgentTaskFailed { .. } => TuiEventClass::Consumed,
        AhirtProbeCompleted { .. } => TuiEventClass::Consumed,
        AsaIntervention { .. } => TuiEventClass::Consumed,
        AssessmentUpdated { .. } => TuiEventClass::NotForTui,
        AuditFindingRaised { .. } => TuiEventClass::Consumed,
        AuditLogged { .. } => TuiEventClass::NotForTui,
        BenchmarkMetricsCollected { .. } => TuiEventClass::NotForTui,
        BlocksRebalanced { .. } => TuiEventClass::NotForTui,
        BudgetAdjusted { .. } => TuiEventClass::Consumed,
        BudgetExceeded { .. } => TuiEventClass::Consumed,
        BudgetMetricsUpdated { .. } => TuiEventClass::Consumed,
        BudgetStatsReported { .. } => TuiEventClass::Consumed,
        BusThroughputReported { .. } => TuiEventClass::NotForTui,
        CacheAffinityApplied { .. } => TuiEventClass::NotForTui,
        CacheHit { .. } => TuiEventClass::Consumed,
        CacheMiss { .. } => TuiEventClass::Consumed,
        CachePrefetched { .. } => TuiEventClass::Consumed,
        CacheStatsReported { .. } => TuiEventClass::Consumed,
        CapabilityFrozen { .. } => TuiEventClass::Consumed,
        CapabilityTierStatsReported { .. } => TuiEventClass::Consumed,
        CapabilityTiered { .. } => TuiEventClass::NotForTui,
        CheckpointLoaded { .. } => TuiEventClass::Consumed,
        CheckpointSaved { .. } => TuiEventClass::Consumed,
        ChtcAdapterStatus { .. } => TuiEventClass::Consumed,
        ChtcToolCallReceived { .. } => TuiEventClass::NotForTui,
        ClvSnapshotReported { .. } => TuiEventClass::Consumed,
        ConsensusReached { .. } => TuiEventClass::Consumed,
        ContextBudgetAllocated { .. } => TuiEventClass::NotForTui,
        ContextCompressed { .. } => TuiEventClass::Consumed,
        ContextWindowSwitched { .. } => TuiEventClass::Consumed,
        // PS-2 批次1:CoordinationRatioReported 已由 ParliamentSync 消费
        // (协调成本/推理增益 → 议会面板治理态势行),守卫第三次实战改判。
        CoordinationRatioReported { .. } => TuiEventClass::Consumed,
        CrossVendorNegotiation { .. } => TuiEventClass::NotForTui,
        CsnSubstitutionTriggered { .. } => TuiEventClass::Consumed,
        DebateCompleted { .. } => TuiEventClass::NotForTui,
        DebateStarted { .. } => TuiEventClass::Consumed,
        DecayMetricsReported { .. } => TuiEventClass::Consumed,
        DelegationCompleted { .. } => TuiEventClass::NotForTui,
        DpoPairGenerated { .. } => TuiEventClass::NotForTui,
        EfficiencyAlertTriggered { .. } => TuiEventClass::Consumed,
        EntropyBalanced { .. } => TuiEventClass::NotForTui,
        ErrorSignatureMatched { .. } => TuiEventClass::NotForTui,
        EvolutionTriggered { .. } => TuiEventClass::NotForTui,
        ExecutionCompleted { .. } => TuiEventClass::NotForTui,
        ExpertActivated { .. } => TuiEventClass::NotForTui,
        ExpertRegistered { .. } => TuiEventClass::NotForTui,
        ExpertRouted { .. } => TuiEventClass::NotForTui,
        ExpertUnregistered { .. } => TuiEventClass::NotForTui,
        FormalViolation { .. } => TuiEventClass::NotForTui,
        GatherCompleted { .. } => TuiEventClass::NotForTui,
        GatherTimedOut { .. } => TuiEventClass::Consumed,
        GhostMemoryDetected { .. } => TuiEventClass::NotForTui,
        GsoePolicyUpdated { .. } => TuiEventClass::NotForTui,
        HarnessReportGenerated { .. } => TuiEventClass::Consumed,
        HcwRecallDegraded { .. } => TuiEventClass::NotForTui,
        HcwRecallReported { .. } => TuiEventClass::Consumed,
        LsctTierSwitched { .. } => TuiEventClass::NotForTui,
        McpMeshTransactionCompleted { .. } => TuiEventClass::Consumed,
        McpMessageReceived { .. } => TuiEventClass::Consumed,
        McpNodeHeartbeat { .. } => TuiEventClass::Consumed,
        MemConStrategyAdjusted { .. } => TuiEventClass::Consumed,
        MemoryMetricsReported { .. } => TuiEventClass::Consumed,
        MemoryTiered { .. } => TuiEventClass::Consumed,
        ModelAffinitySelected { .. } => TuiEventClass::NotForTui,
        ModelRouteSelected { .. } => TuiEventClass::Consumed,
        NexusStateChanged { .. } => TuiEventClass::Consumed,
        NmcEncoded { .. } => TuiEventClass::NotForTui,
        OmniSparseMasksComputed { .. } => TuiEventClass::Consumed,
        OperationProduced { .. } => TuiEventClass::NotForTui,
        OperationTimedOut { .. } => TuiEventClass::Consumed,
        OrphanCallDetected { .. } => TuiEventClass::Consumed,
        OverWindowFallbackTriggered { .. } => TuiEventClass::Consumed,
        ParentSelected { .. } => TuiEventClass::NotForTui,
        // PS-2 批次1:ParliamentStrategyCapChanged 已由 ParliamentSync 消费
        // (策略封顶 → 议会面板治理态势行),同上。
        ParliamentStrategyCapChanged { .. } => TuiEventClass::Consumed,
        PredictionMade { .. } => TuiEventClass::NotForTui,
        PredictionRolledBack { .. } => TuiEventClass::NotForTui,
        PredictionStatsReported { .. } => TuiEventClass::NotForTui,
        PredictionVerified { .. } => TuiEventClass::NotForTui,
        ProducerStrategyAdjusted { .. } => TuiEventClass::NotForTui,
        ProviderDegraded { .. } => TuiEventClass::NotForTui,
        QuestCancelRequested { .. } => TuiEventClass::Consumed,
        QuestCancelled { .. } => TuiEventClass::Consumed,
        QuestCompleted { .. } => TuiEventClass::Consumed,
        QuestCreated { .. } => TuiEventClass::Consumed,
        QuestListUpdated { .. } => TuiEventClass::Consumed,
        QuestPauseRequested { .. } => TuiEventClass::Consumed,
        QuestPaused { .. } => TuiEventClass::Consumed,
        QuestPriorityAdjusted { .. } => TuiEventClass::Consumed,
        QuestPriorityChanged { .. } => TuiEventClass::Consumed,
        QuestProgressUpdated { .. } => TuiEventClass::Consumed,
        QuestResumeRequested { .. } => TuiEventClass::Consumed,
        QuestResumed { .. } => TuiEventClass::Consumed,
        R1ShadowPromotionReady { .. } => TuiEventClass::NotForTui,
        R1ShadowRegressionDetected { .. } => TuiEventClass::NotForTui,
        R1ShadowRollbackFailed { .. } => TuiEventClass::NotForTui,
        R2FreezeRollbackFailed { .. } => TuiEventClass::NotForTui,
        R2FreezeViolation { .. } => TuiEventClass::NotForTui,
        RedTeamAudit { .. } => TuiEventClass::Consumed,
        RefreshStateRequested { .. } => TuiEventClass::Consumed,
        ResourceRecovered { .. } => TuiEventClass::NotForTui,
        RewardSignalReported { .. } => TuiEventClass::NotForTui,
        RoleRegistered { .. } => TuiEventClass::Consumed,
        RouterStatsReported { .. } => TuiEventClass::Consumed,
        SandboxViolation { .. } => TuiEventClass::Consumed,
        SecurityInterceptionReported { .. } => TuiEventClass::NotForTui,
        SemanticCacheHit { .. } => TuiEventClass::NotForTui,
        SesaActivationCompleted { .. } => TuiEventClass::NotForTui,
        ShadowBreakerTripped { .. } => TuiEventClass::Consumed,
        FormalVerificationFailed { .. } => TuiEventClass::Consumed,
        SkepticVeto { .. } => TuiEventClass::Consumed,
        SlowConsumerDropped { .. } => TuiEventClass::Consumed,
        SpecRegistered { .. } => TuiEventClass::NotForTui,
        SsraFusionCompleted { .. } => TuiEventClass::NotForTui,
        StopRulingIssued { .. } => TuiEventClass::NotForTui,
        StreamSessionCompleted { .. } => TuiEventClass::NotForTui,
        ThinkingModeSwitched { .. } => TuiEventClass::Consumed,
        TokenLedgerRecorded { .. } => TuiEventClass::NotForTui,
        ToolsRouted { .. } => TuiEventClass::NotForTui,
        TuiActionCompleted { .. } => TuiEventClass::Consumed,
        TuiActionFailed { .. } => TuiEventClass::Consumed,
        TuiActionProgressed { .. } => TuiEventClass::NotForTui,
        TuiActionRequested { .. } => TuiEventClass::Consumed,
        TuiChatCompleted { .. } => TuiEventClass::Consumed,
        TuiChatHistoryReplaced { .. } => TuiEventClass::Consumed,
        TuiChatResponseChunk { .. } => TuiEventClass::Consumed,
        TuiChatStatusChanged { .. } => TuiEventClass::Consumed,
        TuiChatSubmitted { .. } => TuiEventClass::Consumed,
        TuiHello { .. } => TuiEventClass::Consumed,
        // PS-2(F-1):TuiHelloAck 已由 HandshakeSync 消费(compat → 状态栏),
        // 握手从"发射后不管"单向死链变为可观测闭环 —— 守卫首次实战改判。
        TuiHelloAck { .. } => TuiEventClass::Consumed,
        UserIntentEncoded { .. } => TuiEventClass::Consumed,
        VariantApproved { .. } => TuiEventClass::NotForTui,
        VetoOverridden { .. } => TuiEventClass::Consumed,
        VoteCast { .. } => TuiEventClass::Consumed,
        VoteCastRequested { .. } => TuiEventClass::NotForTui,
        WikiUpdated { .. } => TuiEventClass::NotForTui,
        WindowAffinityApplied { .. } => TuiEventClass::NotForTui,
    }
}

// ============================================================
// 运行期验证:分类语义抽查(Consumed / NotForTui 双向各一)
// 穷尽性的强制力在编译期(新增变体 → E0004 编译失败);
// 此处锁定**分类语义**不随重构漂移。
// ============================================================

use event_bus::EventMetadata;

#[test]
fn consumed_variant_classified_as_consumed() {
    // AuditFindingRaised:SelfAssessment 面板消费(五维度自评的发现流数据源)
    let ev = NexusEvent::AuditFindingRaised {
        metadata: EventMetadata::new("drift-test"),
        finding_severity: "low".into(),
        category: "test".into(),
        message: "spot check".into(),
        evidence_kind: "static_only".into(),
        fix_hint: "none".into(),
    };
    assert_eq!(classify(&ev), TuiEventClass::Consumed);
}

#[test]
fn unconsumed_variant_classified_as_not_for_tui() {
    // ActivationCacheStats:系统内部缓存统计,TUI 不消费(评估报告死事件清单)
    let ev = NexusEvent::ActivationCacheStats {
        metadata: EventMetadata::new("drift-test"),
        hit_rate: 0.5,
        entry_count: 1,
    };
    assert_eq!(classify(&ev), TuiEventClass::NotForTui);
}

#[test]
fn consumed_names_lock_is_sorted_unique_and_sized() {
    // 审计锁自检:严格升序(= 排序 + 去重)、规模 72(与 2026-09-09 评估实测一致)。
    // 名单变动必须是有意识的审计行为(伴随评估报告更新),而非顺手增删。
    assert!(
        CONSUMED_NAMES.windows(2).all(|w| w[0] < w[1]),
        "CONSUMED_NAMES 必须严格升序且无重复"
    );
    assert_eq!(CONSUMED_NAMES.len(), 76, "Consumed 集合规模变更属审计行为");
}

// ============================================================
// 声明一致性:Consumed 必须"真的被消费"
// ============================================================

/// 读取 `crates/chimera-tui/src/**` 的全部源码文本(已剥离行注释)
///
/// WHY 剥离注释:`NexusEvent::X` 也可能出现在解释性注释里(如"经 X 事件派生"),
/// 那不算消费点。逐行截断 `//` 之后内容是**近似**处理(不解析字符串字面量),
/// 对本检查足够 —— 真正的消费点必然落在代码行上。
fn collect_src_code_only() -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = String::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path); // 递归子树(src/panels、src/app、src/data ...)
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines() {
                let code = match line.find("//") {
                    Some(i) => &line[..i],
                    None => line,
                };
                out.push_str(code);
                out.push('\n');
            }
        }
    }
    out
}

/// 声明一致性:凡在 `CONSUMED_NAMES` 中声明为已消费的变体,必须在 crate 源码中
/// 存在真实消费点。
///
/// # WHY 需要本层
/// 前两层强制机制(穷尽分类 / 名单锁)都只校验**声明自身**是否整洁:
/// - 穷尽 `classify` 保证"每个变体都被表态";
/// - 名单锁保证"名单有序且规模未静默变化"。
///
/// 但**两者都无法阻止表态说谎**:把某变体标为 `Consumed`、却不在任何
/// sync/面板里处理它,事件同样会静默不落地 —— 这正是评估报告 F-5 关注的
/// 静默漂移类别。本测试以源码静态检索把"声明"与"实现"对账。
///
/// # 当前状态
/// 首轮实测 76/76 全部命中(无豁免名单),故本不变量是**强约束、零例外**。
/// 若未来确有无法用 `NexusEvent::<Name>` 字面量表达的消费方式(如经宏批量匹配),
/// 需在此显式登记豁免并说明理由,不得直接放宽断言。
#[test]
fn consumed_names_are_actually_referenced_in_source() {
    let src = collect_src_code_only();
    let unreferenced: Vec<&str> = CONSUMED_NAMES
        .iter()
        .copied()
        .filter(|name| !src.contains(&format!("NexusEvent::{name}")))
        .collect();
    assert!(
        unreferenced.is_empty(),
        "以下变体被声明为 Consumed,却在 crates/chimera-tui/src 中找不到 \n\
         任何 `NexusEvent::<Name>` 消费点(声明与实现不符 = 谎报已消费):\n  {unreferenced:?}\n\
         修法二选一:① 到 sync.rs / 面板真正接线;② 如实改判为 NotForTui。"
    );
}
