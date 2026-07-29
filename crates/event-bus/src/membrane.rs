//! 膜渗透过滤器 — 内环/外环选择性渗透决策（P2-W6.1, ADR-033 后续膜深化）
//!
//! 对应架构层:L1 Core(event-bus 深化,从"浅双通道总线"演化为"膜")
//! 对应设计源:`NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §3.3 膜控渗透 +
//!             spec.md L242-249 Membrane 选择性渗透 Scenario
//!
//! # 核心职责
//! 基于事件语义类别(EventCategory) + 内环负载档位(InnerLoad) + 膜厚度
//! (MembraneThickness) 三维度,为每个事件做出渗透决策(PermeationDecision):
//! - `PassToCore`:穿膜入内环(走 Critical mpsc 旁路或 broadcast)
//! - `LocalConsume`:外环本地消化(不走内环通道)
//!
//! # 分类维度
//! 与 [`crate::topic::EventTopic`] 不同维度:
//! - `EventTopic` 按"功能域"分类(Routing/Memory/Security 等 10 类),服务于
//!   `FilteredSubscriber` 选择性订阅
//! - `EventCategory` 按"是否影响内环状态"分类(Critical/MemoryWrite 等 7 类),
//!   服务于膜渗透决策
//!
//! # spec.md L246-248 渗透规则
//! - core_rules(穿膜):Critical / 记忆写 / 策略更新 / 高风险
//! - local_rules(本地):只读 / 缓存命中 / Normal 低优
//! - Critical 档只放行 Critical 事件族(spec.md L247)
//! - ImmuneSystem 级联风险 >0.7 → 膜自动增厚(spec.md L249,P5 接口预留)
//!
//! # 设计原则
//! - 同步纯函数 `decide()`:无锁无 await,符合 §4.4 反模式 1
//! - 穷尽 match:新增 NexusEvent 变体时编译器强制更新 categorize 映射
//! - 默认保守:未知类别归入 NormalLow(本地消化),避免未知事件冲垮内环

use crate::types::{EventSeverity, NexusEvent};

/// 内环负载档位 — 反向调节膜渗透规则(spec.md L244-248)
///
/// 4 档位对应内环认知子系统的承载压力,档位越高膜越严格(越少事件穿膜)。
/// 由内环监控器(未来 efficiency-monitor 扩展)周期性更新,MembraneFilter
/// 据此动态调节渗透规则与批窗口。
///
/// # 设计权衡
/// - 4 档而非连续值:离散档位易于调试与状态机推理,避免浮点比较精度问题
/// - 默认 Low:启动时内环无负载,宽松规则;压力上升时由监控器逐步升级
/// - Critical 档硬性约束:仅放行 Critical 事件族,其他全部本地消化
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InnerLoad {
    /// 低负载(默认):所有规则按基础档执行,core_rules 穿膜 / local_rules 本地
    #[default]
    Low,
    /// 中等负载:收紧 local_rules 批窗口(通过 thickness 协同),core_rules 不变
    Medium,
    /// 高负载:仅放行 core_rules(Critical/MemoryWrite/PolicyUpdate/HighRisk),
    /// local_rules 全部本地消化
    High,
    /// Critical 负载:只放行 Critical 事件族(spec.md L247 硬性要求),
    /// 其他全部本地消化(包括 MemoryWrite/PolicyUpdate/HighRisk)
    Critical,
}

/// 事件语义类别 — 按"是否影响内环状态"分类(7 类)
///
/// 与 [`crate::topic::EventTopic`] 不同维度:EventTopic 按功能域分类(10 类),
/// EventCategory 按内环影响分类(7 类)。一个 EventTopic::Memory 事件可能属于
/// EventCategory::MemoryWrite(NexusStateChanged)或 ReadMetric(MemoryMetricsReported)。
///
/// # 7 类划分(spec.md L246-248)
/// - **core_rules(4 类,穿膜)**:Critical / MemoryWrite / PolicyUpdate / HighRisk
/// - **local_rules(3 类,本地)**:CacheLocal / ReadMetric / NormalLow
///
/// # 穷尽性
/// `MembraneFilter::categorize` 对全部 NexusEvent 变体穷尽 match,
/// 新增变体时编译器强制更新映射,避免遗漏导致未知分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventCategory {
    /// Critical 安全/治理事件(10 个 severity()==Critical 变体)
    ///
    /// spec.md L186 列出的 6 个 Critical 事件 + 代码 severity() 实际标记
    /// Critical 的 10 个变体(CheckpointSaved/ConsensusReached/SlowConsumerDropped/
    /// OrphanCallDetected/SkepticVeto/VetoOverridden/RedTeamAudit/BudgetExceeded/
    /// AgentTaskFailed/AsaIntervention)
    Critical,

    /// 记忆写:修改记忆/上下文/能力分层状态(影响内环记忆一致性)
    ///
    /// 内环 9 crate 共享记忆状态,此类事件必须穿膜入内环以确保状态一致。
    /// 例:NexusStateChanged/MemoryTiered/ContextWindowSwitched/WikiUpdated 等
    MemoryWrite,

    /// 策略更新:修改策略/预算/能力配置(影响内环决策策略)
    ///
    /// 策略变更需立即被内环推理子系统感知,避免使用过期策略。
    /// 例:GsoePolicyUpdated/BudgetAdjusted/CapabilityFrozen/ThinkingModeSwitched
    PolicyUpdate,

    /// 高风险:风险等级 >= 71 或自描述 high-risk(影响内环安全决策)
    ///
    /// spec.md L200-206 高危操作强制升级通道:risk_score ∈ [71,100] 的事件
    /// 必须经 Parliament 辩论 + Merkle 审计链。UserIntentEncoded 的 risk_level
    /// 字段 >= 71 时归入此类。
    HighRisk,

    /// 缓存本地:缓存命中/未命中/统计(外环本地消化)
    ///
    /// 缓存事件属于外环 L3 Storage 子系统自治范畴,不影响内环认知状态。
    /// 例:CacheHit/CacheMiss/CachePrefetched/CacheStatsReported
    CacheLocal,

    /// 只读指标:心跳/统计报告(外环本地消化)
    ///
    /// 观测性事件,不影响内环状态。例:MemoryMetricsReported/AgentHeartbeat/
    /// McpNodeHeartbeat/RouterStatsReported 等周期性指标
    ReadMetric,

    /// Normal 低优:不属于以上类别的 Normal 事件(默认本地消化)
    ///
    /// 包括 Quest 生命周期事件/控制请求事件/路由执行结果等。
    /// 这些事件由外环各 crate 自治处理,不需要穿膜入内环。
    NormalLow,
}

/// 膜渗透决策
///
/// `MembraneFilter::decide` 的返回值,指示事件是否穿膜入内环。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermeationDecision {
    /// 穿膜入内环(走 Critical mpsc 旁路或 broadcast,内环 9 crate 感知)
    PassToCore,
    /// 外环本地消化(不走内环通道,外环 crate 自治处理)
    LocalConsume,
}

impl PermeationDecision {
    /// 是否穿膜入内环
    pub fn passes_to_core(self) -> bool {
        matches!(self, Self::PassToCore)
    }

    /// 是否外环本地消化
    pub fn is_local_consume(self) -> bool {
        matches!(self, Self::LocalConsume)
    }
}

/// 膜厚度 — ImmuneSystem 级联风险调节的独立维度(spec.md L249)
///
/// # 设计理据
/// spec.md L249:"ImmuneSystem 级联风险 >0.7 时膜自动增厚"。
/// 厚度是独立于 InnerLoad 的"长期压力"维度:
/// - `InnerLoad`:内环实时负载(短期,秒级波动)
/// - `MembraneThickness`:持续风险等级(长期,分钟级持续)
///
/// 两者协同:thickness 达 MAX 时等同 InnerLoad::Critical 档(仅放行 Critical)。
/// P5 阶段 ImmuneSystem facade 落地后,会调用 `thicken` 增厚膜。
///
/// # 范围
/// 0-10 整数,0=最薄(默认)、10=最厚(仅 Critical 穿膜)。
/// 增厚 / 衰减均饱和处理,不会溢出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct MembraneThickness(u8);

impl MembraneThickness {
    /// 最小厚度(0 = 最宽松,所有规则按 InnerLoad 基础档执行)
    pub const MIN: u8 = 0;

    /// 最大厚度(10 = 最严格,仅 Critical 事件穿膜)
    pub const MAX: u8 = 10;

    /// 高风险阈值(>= 7 视为增厚状态,触发额外保守策略)
    ///
    /// WHY 7 而非 10:给 ImmuneSystem 一个早期预警区间,在达到 MAX 前就开始
    /// 收紧规则,避免在临界点突然切换。7-9 区间额外限制 NormalLow 穿膜。
    pub const HIGH_RISK_THRESHOLD: u8 = 7;

    /// 从 u8 构造厚度,饱和到 [MIN, MAX] 范围
    pub const fn new(value: u8) -> Self {
        let v = if value > Self::MAX { Self::MAX } else { value };
        Self(v)
    }

    /// 获取厚度值
    pub const fn value(self) -> u8 {
        self.0
    }

    /// 是否达到最大厚度(仅 Critical 穿膜)
    pub const fn is_maxed(self) -> bool {
        self.0 >= Self::MAX
    }

    /// 是否处于高风险增厚状态(>= HIGH_RISK_THRESHOLD)
    pub const fn is_high_risk(self) -> bool {
        self.0 >= Self::HIGH_RISK_THRESHOLD
    }

    /// 增厚膜(P5 ImmuneSystem 级联风险 >0.7 时调用)
    ///
    /// 饱和到 MAX,不会溢出。返回是否实际增厚(false 表示已达 MAX)。
    pub fn thicken(&mut self, delta: u8) -> bool {
        if self.0 >= Self::MAX {
            return false;
        }
        self.0 = self.0.saturating_add(delta).min(Self::MAX);
        true
    }

    /// 衰减膜(定期恢复,每周期减 1)
    ///
    /// 饱和到 MIN,不会下溢。返回是否实际衰减(false 表示已达 MIN)。
    pub fn thin(&mut self, delta: u8) -> bool {
        // WHY `==` 而非 `<=`:Self::MIN = 0 是 u8 的类型最小值,
        // `self.0 <= 0` 等价于 `self.0 == 0`(clippy::absurd-extreme-comparisons)。
        if self.0 == Self::MIN {
            return false;
        }
        // saturating_sub 已饱和到 0(= Self::MIN),无需额外 .max()(clippy::unnecessary-min-or-max)
        self.0 = self.0.saturating_sub(delta);
        true
    }
}

/// 膜渗透过滤器 — 三维度决策事件是否穿膜入内环
///
/// 三维度:
/// 1. `inner_load`:内环实时负载档位(Low/Medium/High/Critical)
/// 2. `thickness`:膜厚度(0-10,ImmuneSystem 长期风险调节)
/// 3. 事件语义类别(通过 `categorize` 静态方法获取)
///
/// # 决策规则(spec.md L244-249)
/// - InnerLoad::Critical → 仅 Critical 类别 PassToCore,其他全部 LocalConsume
/// - thickness.is_maxed() → 等同 InnerLoad::Critical(长期高压保护)
/// - InnerLoad::High → core_rules(Critical/MemoryWrite/PolicyUpdate/HighRisk)
///   PassToCore,local_rules(CacheLocal/ReadMetric/NormalLow)LocalConsume
/// - InnerLoad::Low/Medium → 默认规则(core_rules 穿膜,local_rules 本地)
///
/// WHY thickness 的调节粒度:thickness 是离散阶梯式收紧——
/// - `is_maxed()`(= 10):仅 Critical 穿膜(最严格,等同 Critical 档)
/// - `is_high_risk()`(>= 7):P5 ImmuneSystem 预警区间,预留接口待落地
/// - < 7:不产生额外收紧(local_rules 本就 LocalConsume,无法进一步拒绝)
///
/// # 使用方式
/// MembraneFilter 是同步纯结构,无锁无 await,可自由 Clone。
/// 内环监控器(未来 efficiency-monitor 扩展)周期性调用 `set_load` 更新负载档位,
/// ImmuneSystem(P5)调用 `thicken`/`thin` 调节厚度。
/// 事件发布路径在 `publish` 前调用 `decide` 判定是否穿膜。
///
/// # 向后兼容
/// MembraneFilter 是新增模块,不修改既有 `publish`/`publish_critical` 签名。
/// 既有调用方默认走原通道(broadcast + mpsc 旁路),不感知膜存在。
/// P2-W7.1 将在膜内集成 P1 有界 Critical 通道,届时膜成为发布路径的必经节点。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MembraneFilter {
    /// 内环当前负载档位
    inner_load: InnerLoad,
    /// 膜厚度(0-10,ImmuneSystem 调节)
    thickness: MembraneThickness,
}

impl MembraneFilter {
    /// 创建默认膜(Low 负载 + 0 厚度)
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建指定负载档位的膜(厚度默认 0)
    pub fn with_load(inner_load: InnerLoad) -> Self {
        Self {
            inner_load,
            thickness: MembraneThickness::default(),
        }
    }

    /// 创建指定负载档位与初始厚度的膜
    pub fn with_load_and_thickness(inner_load: InnerLoad, thickness: MembraneThickness) -> Self {
        Self {
            inner_load,
            thickness,
        }
    }

    /// 获取内环负载档位
    ///
    /// WHY `&self` 而非 `self`:避免调用后 MembraneFilter 所有权转移,
    /// 允许在测试与连续调用中复用同一实例(如 `f.load()` 后再 `f.decide()`)。
    pub fn load(&self) -> InnerLoad {
        self.inner_load
    }

    /// 设置内环负载档位(内环监控器周期性调用)
    pub fn set_load(&mut self, load: InnerLoad) {
        self.inner_load = load;
    }

    /// 获取膜厚度
    ///
    /// WHY `&self` 而非 `self`:同 `load()`,避免所有权转移。
    pub fn thickness(&self) -> MembraneThickness {
        self.thickness
    }

    /// 增厚膜(P5 ImmuneSystem 级联风险 >0.7 时调用,spec.md L249)
    ///
    /// 返回是否实际增厚(false 表示已达 MAX)。
    pub fn thicken(&mut self, delta: u8) -> bool {
        self.thickness.thicken(delta)
    }

    /// 衰减膜(定期恢复,每周期减 delta)
    ///
    /// 返回是否实际衰减(false 表示已达 MIN)。
    pub fn thin(&mut self, delta: u8) -> bool {
        self.thickness.thin(delta)
    }

    /// 分类事件语义类别(静态方法,无需实例)
    ///
    /// 对全部 NexusEvent 变体穷尽 match,新增变体时编译器强制更新映射。
    /// 优先级:Critical(severity) > 字段条件判定(risk_level) > 变体名匹配。
    ///
    /// # 分类规则
    /// 1. severity()==Critical → EventCategory::Critical(10 个变体)
    /// 2. UserIntentEncoded.risk_level >= 71 → HighRisk(spec.md L200 高危操作)
    /// 3. 字段条件不满足时,按变体名 match 归入对应类别
    /// 4. 未知变体(不应发生,因 match 穷尽)→ 默认 NormalLow(保守策略)
    pub fn categorize(event: &NexusEvent) -> EventCategory {
        // 1. Critical 事件族优先(severity==Critical 的 10 个变体)
        if event.severity() == EventSeverity::Critical {
            return EventCategory::Critical;
        }

        // 2. 字段条件判定:UserIntentEncoded 的 risk_level 字段
        // WHY 先判字段再判变体名:UserIntentEncoded 默认 severity==Normal,
        // 但 risk_level >= 71 时应归入 HighRisk(对应 spec.md L200 高危操作)
        if let NexusEvent::UserIntentEncoded { risk_level, .. } = event {
            return if *risk_level >= 71 {
                EventCategory::HighRisk
            } else {
                EventCategory::NormalLow
            };
        }

        // 3. 按变体名 match 归入对应类别(穷尽覆盖全部 100 个变体)
        match event {
            // === MemoryWrite:修改记忆/上下文/能力分层状态 ===
            NexusEvent::NexusStateChanged { .. }
            | NexusEvent::MemoryTiered { .. }
            | NexusEvent::ContextWindowSwitched { .. }
            | NexusEvent::ContextCompressed { .. }
            | NexusEvent::CapabilityTiered { .. }
            | NexusEvent::BlocksRebalanced { .. }
            | NexusEvent::CheckpointLoaded { .. }
            | NexusEvent::WikiUpdated { .. }
            | NexusEvent::LsctTierSwitched { .. }
            | NexusEvent::NmcEncoded { .. }
            | NexusEvent::AgentContextOverflow { .. } => EventCategory::MemoryWrite,

            // === PolicyUpdate:修改策略/预算/能力配置 ===
            NexusEvent::GsoePolicyUpdated { .. }
            | NexusEvent::ActivationThresholdAdjusted { .. }
            | NexusEvent::BudgetAdjusted { .. }
            | NexusEvent::CapabilityFrozen { .. }
            | NexusEvent::ThinkingModeSwitched { .. }
            | NexusEvent::QuestPriorityAdjusted { .. }
            | NexusEvent::QuestPriorityChanged { .. }
            | NexusEvent::ExpertRegistered { .. }
            | NexusEvent::ExpertUnregistered { .. }
            | NexusEvent::RoleRegistered { .. }
            // P4-W16.2.2:R1 影子模式策略生命周期（退化检测/解冻就绪，Normal 级策略通知）
            | NexusEvent::R1ShadowRegressionDetected { .. }
            | NexusEvent::R1ShadowPromotionReady { .. }
            // P5.2.3:Spec 版本注册完成(L5 Knowledge → 任意订阅者,策略级通知)
            // WHY 归入 PolicyUpdate:与 GsoePolicyUpdated 同属策略/配置生命周期事件,
            //    下游(parliament / efficiency-monitor)需更新 spec 版本快照
            | NexusEvent::SpecRegistered { .. } => EventCategory::PolicyUpdate,

            // === HighRisk:自描述 high-risk 事件(不含 UserIntentEncoded,已上面处理) ===
            // SandboxViolation:沙箱违规,安全告警(虽 severity==Normal,但语义高风险)
            // OperationProduced:沙箱执行产物,可能影响安全决策
            // CsnSubstitutionTriggered:CSN 降级链触发,容灾高风险
            // EfficiencyAlertTriggered:效率告警,可能是级联故障前兆
            NexusEvent::SandboxViolation { .. }
            | NexusEvent::OperationProduced { .. }
            | NexusEvent::CsnSubstitutionTriggered { .. }
            | NexusEvent::EfficiencyAlertTriggered { .. } => EventCategory::HighRisk,

            // === CacheLocal:缓存命中/未命中/统计(外环本地消化) ===
            NexusEvent::CacheHit { .. }
            | NexusEvent::CacheMiss { .. }
            | NexusEvent::CachePrefetched { .. }
            | NexusEvent::CacheStatsReported { .. } => EventCategory::CacheLocal,

            // === ReadMetric:只读指标/心跳/统计报告(外环本地消化) ===
            NexusEvent::MemoryMetricsReported { .. }
            | NexusEvent::BudgetStatsReported { .. }
            | NexusEvent::BudgetMetricsUpdated { .. }
            | NexusEvent::DecayMetricsReported { .. }
            | NexusEvent::RouterStatsReported { .. }
            | NexusEvent::PredictionStatsReported { .. }
            | NexusEvent::ActivationCacheStats { .. }
            | NexusEvent::ClvSnapshotReported { .. }
            | NexusEvent::McpNodeHeartbeat { .. }
            | NexusEvent::ChtcAdapterStatus { .. }
            | NexusEvent::AgentHeartbeat { .. }
            | NexusEvent::McpMessageReceived { .. }
            // P2-1:协调成本/推理增益比值报告(L9 quest-engine 发布,只读指标)
            | NexusEvent::CoordinationRatioReported { .. }
            // polish-v2.7 P1-2:RuntimeAuditor 审计发现与五维度报告(L9 efficiency-monitor 发布,
            // 观察性事实陈述,外环本地消化即可,不需穿膜进内环)
            | NexusEvent::AuditFindingRaised { .. }
            | NexusEvent::HarnessReportGenerated { .. } => EventCategory::ReadMetric,

            // === NormalLow:剩余 Normal 事件(默认本地消化) ===
            // 包括 Quest 生命周期/控制请求/路由执行结果/Agent 协作等
            // UserIntentEncoded 已在前面 if 分支处理,此处 match 不再列出
            //
            // OmniSparseMasksComputed:OSA 稀疏掩码计算结果(L6 路由层产物),
            // 不直接影响内环记忆/策略/安全状态,归入 NormalLow(同 ToolsRouted)。
            // PredictionVerified:PVL 生产验证结果(L7 执行层产物),
            // 归入 NormalLow(同 ExecutionCompleted/GatherCompleted)。
            NexusEvent::OmniSparseMasksComputed { .. }
            | NexusEvent::PredictionVerified { .. }
            | NexusEvent::ModelRouteSelected { .. }
            | NexusEvent::QuestCreated { .. }
            | NexusEvent::QuestProgressUpdated { .. }
            | NexusEvent::QuestListUpdated { .. }
            | NexusEvent::QuestCompleted { .. }
            | NexusEvent::VoteCast { .. }
            | NexusEvent::ToolsRouted { .. }
            | NexusEvent::ExecutionCompleted { .. }
            | NexusEvent::EvolutionTriggered { .. }
            | NexusEvent::DpoPairGenerated { .. }
            | NexusEvent::AuditLogged { .. }
            | NexusEvent::ExpertActivated { .. }
            | NexusEvent::ExpertRouted { .. }
            | NexusEvent::EntropyBalanced { .. }
            | NexusEvent::DebateStarted { .. }
            | NexusEvent::AhirtProbeCompleted { .. }
            | NexusEvent::GatherCompleted { .. }
            | NexusEvent::OperationTimedOut { .. }
            | NexusEvent::GatherTimedOut { .. }
            | NexusEvent::ProducerStrategyAdjusted { .. }
            | NexusEvent::PredictionMade { .. }
            | NexusEvent::PredictionRolledBack { .. }
            | NexusEvent::McpMeshTransactionCompleted { .. }
            | NexusEvent::SesaActivationCompleted { .. }
            | NexusEvent::SsraFusionCompleted { .. }
            | NexusEvent::ChtcToolCallReceived { .. }
            // Quest 控制请求与状态反馈(外环 L9 Quest 自治处理)
            | NexusEvent::QuestPauseRequested { .. }
            | NexusEvent::QuestResumeRequested { .. }
            | NexusEvent::QuestCancelRequested { .. }
            | NexusEvent::QuestCancelled { .. }
            | NexusEvent::QuestPaused { .. }
            | NexusEvent::QuestResumed { .. }
            | NexusEvent::VoteCastRequested { .. }
            | NexusEvent::RefreshStateRequested { .. }
            // Agent 协作事件(外环 L9 chimera-mas 自治处理)
            | NexusEvent::AgentTaskDelegated { .. }
            | NexusEvent::AgentTaskCompleted { .. }
            | NexusEvent::AgentConsultRequested { .. }
            | NexusEvent::AgentConsultResponded { .. }
            // TUI 交互式动作协议(外环 L10 Interface 自治处理)
            | NexusEvent::TuiActionRequested { .. }
            | NexusEvent::TuiActionProgressed { .. }
            | NexusEvent::TuiActionCompleted { .. }
            | NexusEvent::TuiActionFailed { .. }
            | NexusEvent::TuiChatSubmitted { .. }
            | NexusEvent::TuiChatResponseChunk { .. }
            | NexusEvent::TuiChatCompleted { .. }
            | NexusEvent::TuiChatStatusChanged { .. }
            // UserIntentEncoded 的 Normal 分支(risk_level < 71)
            | NexusEvent::UserIntentEncoded { .. } => EventCategory::NormalLow,

            // === Critical:severity()==Critical 的 10 个变体 ===
            // WHY 列在此处:虽然函数顶部 `if event.severity() == Critical` 已
            // 提前 return EventCategory::Critical,但编译器不做跨函数的常量传播,
            // 无法推断这 10 个变体已被顶部 if 覆盖。match 穷尽性检查要求显式列出
            // 所有 NexusEvent 变体,故在此补上 Critical 变体分支(运行时不可达,
            // 但保证 match 穷尽,新增变体时编译器强制更新)。
            // 语义上与顶部 if 一致:均归入 EventCategory::Critical。
            NexusEvent::CheckpointSaved { .. }
            | NexusEvent::ConsensusReached { .. }
            | NexusEvent::SlowConsumerDropped { .. }
            | NexusEvent::OrphanCallDetected { .. }
            | NexusEvent::SkepticVeto { .. }
            | NexusEvent::VetoOverridden { .. }
            | NexusEvent::RedTeamAudit { .. }
            | NexusEvent::BudgetExceeded { .. }
            | NexusEvent::AgentTaskFailed { .. }
            | NexusEvent::AsaIntervention { .. }
            // P4-W16.2.2:R1 影子模式回滚失败为 Critical（与 AsaIntervention 同级）
            | NexusEvent::R1ShadowRollbackFailed { .. }
            // ADR-042 决策 4:R2 冻结违反及回滚失败为 Critical(奖励黑客风险立即生效,
            // 必须走 mpsc 旁路通道投递到 SecCore/Parliament 进行处置)
            | NexusEvent::R2FreezeViolation { .. }
            | NexusEvent::R2FreezeRollbackFailed { .. } => EventCategory::Critical,
        }
    }

    /// 决策事件是否穿膜入内环(spec.md L244-249)
    ///
    /// 同步纯函数:无锁无 await,符合 §4.4 反模式 1(禁止持锁 .await)。
    ///
    /// # 决策优先级
    /// 1. InnerLoad::Critical → 仅 Critical 类别 PassToCore(spec.md L247 硬性要求)
    /// 2. thickness.is_maxed() → 等同 Critical 档(长期高压保护)
    /// 3. InnerLoad::High → core_rules PassToCore,local_rules LocalConsume
    /// 4. InnerLoad::Low/Medium → 默认规则 + thickness 高风险额外收紧
    ///
    /// # 示例
    /// ```
    /// use event_bus::membrane::{EventCategory, InnerLoad, MembraneFilter, PermeationDecision};
    /// use event_bus::{EventMetadata, NexusEvent};
    ///
    /// let mut filter = MembraneFilter::new();
    ///
    /// // Critical 事件始终穿膜
    /// let critical = NexusEvent::CheckpointSaved {
    ///     metadata: EventMetadata::new("quest-engine"),
    ///     quest_id: "q1".into(),
    ///     checkpoint_id: "c1".into(),
    ///     memory_snapshot_hash: "h".into(),
    /// };
    /// assert_eq!(filter.decide(&critical), PermeationDecision::PassToCore);
    ///
    /// // 缓存命中事件本地消化
    /// let cache_hit = NexusEvent::CacheHit {
    ///     metadata: EventMetadata::new("scc-cache"),
    ///     cache_key: "k".into(),
    /// };
    /// assert_eq!(filter.decide(&cache_hit), PermeationDecision::LocalConsume);
    ///
    /// // Critical 负载档仅放行 Critical 事件族
    /// filter.set_load(InnerLoad::Critical);
    /// let memory_write = NexusEvent::MemoryTiered {
    ///     metadata: EventMetadata::new("cmt-tiering"),
    ///     tier: "Warm".into(),
    ///     item_count: 1,
    ///     memory_id: Some("m1".into()),
    /// };
    /// assert_eq!(filter.decide(&memory_write), PermeationDecision::LocalConsume);
    /// ```
    pub fn decide(&self, event: &NexusEvent) -> PermeationDecision {
        let category = Self::categorize(event);

        // 1. InnerLoad::Critical 档只放行 Critical 事件族(spec.md L247 硬性要求)
        if self.inner_load == InnerLoad::Critical {
            return match category {
                EventCategory::Critical => PermeationDecision::PassToCore,
                _ => PermeationDecision::LocalConsume,
            };
        }

        // 2. 膜厚度达到 MAX 时,等同 Critical 档(ImmuneSystem 持续级联风险)
        // WHY 独立于 InnerLoad:即使内环负载 Low,持续高风险也应收紧渗透
        if self.thickness.is_maxed() {
            return match category {
                EventCategory::Critical => PermeationDecision::PassToCore,
                _ => PermeationDecision::LocalConsume,
            };
        }

        // 3. InnerLoad::High 档仅放行 core_rules
        if self.inner_load == InnerLoad::High {
            return match category {
                EventCategory::Critical
                | EventCategory::MemoryWrite
                | EventCategory::PolicyUpdate
                | EventCategory::HighRisk => PermeationDecision::PassToCore,
                EventCategory::CacheLocal
                | EventCategory::ReadMetric
                | EventCategory::NormalLow => PermeationDecision::LocalConsume,
            };
        }

        // 4. InnerLoad::Low/Medium 档默认规则:core_rules 穿膜,local_rules 本地。
        //    thickness 在此档不产生额外收紧(local_rules 本就 LocalConsume,
        //    无法进一步拒绝);thickness 的调节作用仅在 is_maxed() 时生效(分支 2)。
        match category {
            EventCategory::Critical
            | EventCategory::MemoryWrite
            | EventCategory::PolicyUpdate
            | EventCategory::HighRisk => PermeationDecision::PassToCore,
            // NormalLow / CacheLocal / ReadMetric 均为 local_rules,始终本地消化。
            // WHY thickness 不影响 local_rules:这些类别本就不穿膜(始终 LocalConsume),
            // 膜厚度增厚无法进一步收紧已拒绝的类别。thickness 的调节作用体现在
            // is_maxed() 时将所有非 Critical 类别(含 core_rules 的 MemoryWrite/
            // PolicyUpdate/HighRisk)全部本地消化(见上方分支 2)。
            EventCategory::CacheLocal | EventCategory::ReadMetric | EventCategory::NormalLow => {
                PermeationDecision::LocalConsume
            }
        }
    }

    /// 批量决策(批窗口场景):对事件切片做出决策向量
    ///
    /// 内环负载 Medium 档时,外环可能批量积累事件后一次性决策,减少锁竞争。
    /// 此方法不修改 self 状态,纯函数式应用当前规则到事件切片。
    pub fn decide_batch(&self, events: &[NexusEvent]) -> Vec<PermeationDecision> {
        events.iter().map(|e| self.decide(e)).collect()
    }

    /// 统计批量事件中需要穿膜入内环的数量
    ///
    /// 用于内环负载评估:若穿膜事件数超过阈值,提升 InnerLoad 档位。
    pub fn count_pass_to_core(&self, events: &[NexusEvent]) -> usize {
        events
            .iter()
            .filter(|e| self.decide(e).passes_to_core())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventMetadata;

    // ============================================================
    // MembraneThickness 单元测试
    // ============================================================

    #[test]
    fn test_thickness_default_is_min() {
        let t = MembraneThickness::default();
        assert_eq!(t.value(), 0);
        assert!(!t.is_maxed());
        assert!(!t.is_high_risk());
    }

    #[test]
    fn test_thickness_new_saturates_to_max() {
        let t = MembraneThickness::new(15);
        assert_eq!(t.value(), MembraneThickness::MAX);
        assert!(t.is_maxed());
    }

    #[test]
    fn test_thickness_new_zero() {
        let t = MembraneThickness::new(0);
        assert_eq!(t.value(), 0);
    }

    #[test]
    fn test_thickness_thicken_saturates() {
        let mut t = MembraneThickness::new(8);
        assert!(t.thicken(5));
        assert_eq!(t.value(), MembraneThickness::MAX);
        // 已达 MAX,继续增厚返回 false
        assert!(!t.thicken(1));
    }

    #[test]
    fn test_thickness_thin_saturates() {
        let mut t = MembraneThickness::new(3);
        assert!(t.thin(5));
        assert_eq!(t.value(), MembraneThickness::MIN);
        // 已达 MIN,继续衰减返回 false
        assert!(!t.thin(1));
    }

    #[test]
    fn test_thickness_high_risk_threshold() {
        let t = MembraneThickness::new(MembraneThickness::HIGH_RISK_THRESHOLD);
        assert!(t.is_high_risk());
        let t_below = MembraneThickness::new(MembraneThickness::HIGH_RISK_THRESHOLD - 1);
        assert!(!t_below.is_high_risk());
    }

    // ============================================================
    // PermeationDecision 单元测试
    // ============================================================

    #[test]
    fn test_permeation_decision_predicates() {
        assert!(PermeationDecision::PassToCore.passes_to_core());
        assert!(!PermeationDecision::PassToCore.is_local_consume());
        assert!(!PermeationDecision::LocalConsume.passes_to_core());
        assert!(PermeationDecision::LocalConsume.is_local_consume());
    }

    // ============================================================
    // InnerLoad 默认值测试
    // ============================================================

    #[test]
    fn test_inner_load_default_is_low() {
        assert_eq!(InnerLoad::default(), InnerLoad::Low);
    }

    // ============================================================
    // MembraneFilter 构造与状态测试
    // ============================================================

    #[test]
    fn test_filter_default() {
        let f = MembraneFilter::new();
        assert_eq!(f.load(), InnerLoad::Low);
        assert_eq!(f.thickness().value(), 0);
    }

    #[test]
    fn test_filter_with_load() {
        let f = MembraneFilter::with_load(InnerLoad::High);
        assert_eq!(f.load(), InnerLoad::High);
        assert_eq!(f.thickness().value(), 0);
    }

    #[test]
    fn test_filter_with_load_and_thickness() {
        let f =
            MembraneFilter::with_load_and_thickness(InnerLoad::Medium, MembraneThickness::new(5));
        assert_eq!(f.load(), InnerLoad::Medium);
        assert_eq!(f.thickness().value(), 5);
    }

    #[test]
    fn test_filter_set_load() {
        let mut f = MembraneFilter::new();
        f.set_load(InnerLoad::Critical);
        assert_eq!(f.load(), InnerLoad::Critical);
    }

    #[test]
    fn test_filter_thicken_thin() {
        let mut f = MembraneFilter::new();
        assert!(f.thicken(3));
        assert_eq!(f.thickness().value(), 3);
        assert!(f.thin(1));
        assert_eq!(f.thickness().value(), 2);
    }

    // ============================================================
    // categorize() 关键类别测试
    // ============================================================

    #[test]
    fn test_categorize_critical_checkpoint_saved() {
        let e = NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q1".into(),
            checkpoint_id: "c1".into(),
            memory_snapshot_hash: "h".into(),
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::Critical);
    }

    #[test]
    fn test_categorize_critical_skeptic_veto() {
        let e = NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q1".into(),
            veto_reason: "test".into(),
            frozen_capabilities: vec![],
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::Critical);
    }

    #[test]
    fn test_categorize_critical_budget_exceeded() {
        let e = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 120_000,
            limit: 100_000,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::Critical);
    }

    #[test]
    fn test_categorize_memory_write_nexus_state_changed() {
        let e = NexusEvent::NexusStateChanged {
            metadata: EventMetadata::new("nexus-core"),
            state_hash: "h1".into(),
            prev_hash: "h0".into(),
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::MemoryWrite);
    }

    #[test]
    fn test_categorize_memory_write_memory_tiered() {
        let e = NexusEvent::MemoryTiered {
            metadata: EventMetadata::new("cmt-tiering"),
            tier: "Warm".into(),
            item_count: 1,
            memory_id: Some("m1".into()),
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::MemoryWrite);
    }

    #[test]
    fn test_categorize_policy_update_gsoe_policy_updated() {
        let e = NexusEvent::GsoePolicyUpdated {
            metadata: EventMetadata::new("gsoe-evolution"),
            generation: 2,
            improvement: 0.15,
            new_mutation_rate: 0.05,
            new_selection_pressure: 1.5,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::PolicyUpdate);
    }

    #[test]
    fn test_categorize_policy_update_capability_frozen() {
        let e = NexusEvent::CapabilityFrozen {
            metadata: EventMetadata::new("parliament"),
            capability_id: "shell-exec".into(),
            reason: "security violation".into(),
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::PolicyUpdate);
    }

    #[test]
    fn test_categorize_high_risk_user_intent_high_risk_level() {
        let e = NexusEvent::UserIntentEncoded {
            metadata: EventMetadata::new("nmc-encoder"),
            intent_id: "i1".into(),
            raw_text: "rm -rf /".into(),
            risk_level: 95,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::HighRisk);
    }

    #[test]
    fn test_categorize_normal_low_user_intent_low_risk_level() {
        let e = NexusEvent::UserIntentEncoded {
            metadata: EventMetadata::new("nmc-encoder"),
            intent_id: "i1".into(),
            raw_text: "hello".into(),
            risk_level: 30,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::NormalLow);
    }

    #[test]
    fn test_categorize_high_risk_user_intent_boundary_71() {
        // 边界值:risk_level == 71 应归入 HighRisk(spec.md L200:>=71 为高危)
        let e = NexusEvent::UserIntentEncoded {
            metadata: EventMetadata::new("nmc-encoder"),
            intent_id: "i1".into(),
            raw_text: "dangerous".into(),
            risk_level: 71,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::HighRisk);
    }

    #[test]
    fn test_categorize_normal_low_user_intent_boundary_70() {
        // 边界值:risk_level == 70 应归入 NormalLow(spec.md L200:<71 为非高危)
        let e = NexusEvent::UserIntentEncoded {
            metadata: EventMetadata::new("nmc-encoder"),
            intent_id: "i1".into(),
            raw_text: "safe".into(),
            risk_level: 70,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::NormalLow);
    }

    #[test]
    fn test_categorize_high_risk_sandbox_violation() {
        let e = NexusEvent::SandboxViolation {
            metadata: EventMetadata::new("seccore"),
            violation_type: "path_traversal".into(),
            detail: "attempted /etc/passwd".into(),
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::HighRisk);
    }

    #[test]
    fn test_categorize_cache_local_cache_hit() {
        let e = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::CacheLocal);
    }

    #[test]
    fn test_categorize_cache_local_cache_miss() {
        let e = NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::CacheLocal);
    }

    #[test]
    fn test_categorize_read_metric_memory_metrics_reported() {
        let e = NexusEvent::MemoryMetricsReported {
            metadata: EventMetadata::new("mlc-engine"),
            hit_rate: 0.85,
            evictions: 3,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::ReadMetric);
    }

    #[test]
    fn test_categorize_read_metric_agent_heartbeat() {
        let e = NexusEvent::AgentHeartbeat {
            metadata: EventMetadata::new("chimera-mas"),
            from: "a1".into(),
            status: crate::types::AgentStatus::Running,
            current_task: None,
            token_usage: 0,
            memory_usage_mb: 64,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::ReadMetric);
    }

    #[test]
    fn test_categorize_normal_low_quest_created() {
        let e = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q1".into(),
            title: "test".into(),
            task_count: 3,
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::NormalLow);
    }

    #[test]
    fn test_categorize_normal_low_control_event() {
        let e = NexusEvent::QuestPauseRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: "q1".into(),
            requested_by: "user".into(),
        };
        assert_eq!(MembraneFilter::categorize(&e), EventCategory::NormalLow);
    }

    // ============================================================
    // decide() InnerLoad 档位测试
    // ============================================================

    #[test]
    fn test_decide_low_load_critical_passes() {
        let f = MembraneFilter::with_load(InnerLoad::Low);
        let e = NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q1".into(),
            checkpoint_id: "c1".into(),
            memory_snapshot_hash: "h".into(),
        };
        assert_eq!(f.decide(&e), PermeationDecision::PassToCore);
    }

    #[test]
    fn test_decide_low_load_memory_write_passes() {
        let f = MembraneFilter::with_load(InnerLoad::Low);
        let e = NexusEvent::MemoryTiered {
            metadata: EventMetadata::new("cmt-tiering"),
            tier: "Warm".into(),
            item_count: 1,
            memory_id: Some("m1".into()),
        };
        assert_eq!(f.decide(&e), PermeationDecision::PassToCore);
    }

    #[test]
    fn test_decide_low_load_cache_local_consumes() {
        let f = MembraneFilter::with_load(InnerLoad::Low);
        let e = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k".into(),
        };
        assert_eq!(f.decide(&e), PermeationDecision::LocalConsume);
    }

    #[test]
    fn test_decide_critical_load_only_critical_passes() {
        let f = MembraneFilter::with_load(InnerLoad::Critical);
        // Critical 事件穿膜
        let critical = NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q1".into(),
            checkpoint_id: "c1".into(),
            memory_snapshot_hash: "h".into(),
        };
        assert_eq!(f.decide(&critical), PermeationDecision::PassToCore);

        // MemoryWrite 在 Critical 档被本地消化(spec.md L247)
        let memory_write = NexusEvent::MemoryTiered {
            metadata: EventMetadata::new("cmt-tiering"),
            tier: "Warm".into(),
            item_count: 1,
            memory_id: Some("m1".into()),
        };
        assert_eq!(
            f.decide(&memory_write),
            PermeationDecision::LocalConsume,
            "Critical 档仅放行 Critical 事件族,MemoryWrite 应本地消化"
        );

        // HighRisk 在 Critical 档被本地消化
        let high_risk = NexusEvent::SandboxViolation {
            metadata: EventMetadata::new("seccore"),
            violation_type: "test".into(),
            detail: "attempted /etc".into(),
        };
        assert_eq!(f.decide(&high_risk), PermeationDecision::LocalConsume);
    }

    #[test]
    fn test_decide_high_load_core_rules_pass() {
        let f = MembraneFilter::with_load(InnerLoad::High);
        // core_rules 穿膜
        let memory_write = NexusEvent::WikiUpdated {
            metadata: EventMetadata::new("repo-wiki"),
            wiki_hash: "h1".into(),
            delta: 3,
        };
        assert_eq!(f.decide(&memory_write), PermeationDecision::PassToCore);

        // local_rules 本地消化
        let cache = NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k".into(),
        };
        assert_eq!(f.decide(&cache), PermeationDecision::LocalConsume);

        let metric = NexusEvent::AgentHeartbeat {
            metadata: EventMetadata::new("chimera-mas"),
            from: "a1".into(),
            status: crate::types::AgentStatus::Running,
            current_task: None,
            token_usage: 0,
            memory_usage_mb: 64,
        };
        assert_eq!(f.decide(&metric), PermeationDecision::LocalConsume);
    }

    // ============================================================
    // decide() MembraneThickness 协同测试
    // ============================================================

    #[test]
    fn test_decide_thickness_maxed_only_critical_passes() {
        let f = MembraneFilter::with_load_and_thickness(
            InnerLoad::Low,
            MembraneThickness::new(MembraneThickness::MAX),
        );
        // 即使 InnerLoad::Low,thickness MAX 也仅放行 Critical
        let critical = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 120_000,
            limit: 100_000,
        };
        assert_eq!(f.decide(&critical), PermeationDecision::PassToCore);

        let memory_write = NexusEvent::MemoryTiered {
            metadata: EventMetadata::new("cmt-tiering"),
            tier: "Warm".into(),
            item_count: 1,
            memory_id: Some("m1".into()),
        };
        assert_eq!(f.decide(&memory_write), PermeationDecision::LocalConsume);
    }

    // ============================================================
    // decide_batch / count_pass_to_core 批量测试
    // ============================================================

    #[test]
    fn test_decide_batch_returns_vector() {
        let f = MembraneFilter::new();
        let events = vec![
            NexusEvent::CheckpointSaved {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q1".into(),
                checkpoint_id: "c1".into(),
                memory_snapshot_hash: "h".into(),
            },
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k".into(),
            },
            NexusEvent::MemoryTiered {
                metadata: EventMetadata::new("cmt-tiering"),
                tier: "Warm".into(),
                item_count: 1,
                memory_id: Some("m1".into()),
            },
        ];
        let decisions = f.decide_batch(&events);
        assert_eq!(decisions.len(), 3);
        assert_eq!(decisions[0], PermeationDecision::PassToCore); // Critical
        assert_eq!(decisions[1], PermeationDecision::LocalConsume); // CacheLocal
        assert_eq!(decisions[2], PermeationDecision::PassToCore); // MemoryWrite
    }

    #[test]
    fn test_count_pass_to_core() {
        let f = MembraneFilter::new();
        let events = vec![
            NexusEvent::CheckpointSaved {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q1".into(),
                checkpoint_id: "c1".into(),
                memory_snapshot_hash: "h".into(),
            },
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k".into(),
            },
            NexusEvent::CacheMiss {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k2".into(),
            },
        ];
        assert_eq!(f.count_pass_to_core(&events), 1);
    }

    // ============================================================
    // Clone/Eq 测试(确保 MembraneFilter 可在任务间 Clone 传递)
    // ============================================================

    #[test]
    fn test_filter_clone_eq() {
        let f1 =
            MembraneFilter::with_load_and_thickness(InnerLoad::High, MembraneThickness::new(5));
        let f2 = f1.clone();
        assert_eq!(f1, f2);
    }
}
