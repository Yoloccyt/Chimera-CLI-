//! 对抗性议会 — 5 角色对抗性审议与决策治理
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)
//!
//! # 核心职责
//! - 维护 5 角色注册表(Architect/Skeptic/Optimizer/Librarian/Bard)
//! - 提案 → 辩论 → 投票 → 共识 全流程
//! - 加权投票:Skeptic 拥有否决权(红队防线)
//! - 否决覆盖:`VetoOverrideTicket` 提供受控的人工覆盖路径(P1-3)
//! - 共识判定:法定人数 + 赞成率双阈值
//! - 发布 `ConsensusReached`/`VoteCast`/`DebateStarted`/`SkepticVeto`/`CapabilityFrozen`/`RedTeamAudit`/`AhirtProbeCompleted` 事件通知订阅者
//!
//! # 5 角色职责
//! - **Architect(架构师)**:关注系统架构合理性、依赖方向、模块边界
//! - **Skeptic(怀疑者)**:红队视角,挑战提案风险,拥有否决权
//! - **Optimizer(优化者)**:关注性能、资源占用、执行效率
//! - **Librarian(图书馆员)**:关注知识检索、历史先例、文档完整性
//! - **Bard(吟游诗人)**:关注创意发散、用户体验、替代方案
//!
//! # 快速示例
//! ```
//! use parliament::{Parliament, ParliamentConfig, Proposal};
//! use event_bus::EventBus;
//! use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let config = ParliamentConfig::default();
//! let parliament = Parliament::new(config, bus);
//!
//! let quest = Quest {
//!     quest_id: "q-1".into(),
//!     title: "示例任务".into(),
//!     tasks: vec![Task {
//!         task_id: "t-1".into(),
//!         description: "首步".into(),
//!         status: TaskStatus::Pending,
//!         dependencies: vec![],
//!     }],
//!     thinking_mode: ThinkingMode::Standard,
//!     checkpoint_id: None,
//!     priority: 128,
//! };
//! let proposal = Proposal::new("p-1", "q-1", "执行计划", 0.3);
//! let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod ahirt;
/// MCA N7 厂商集中度免疫探针 — 供应商锁定的系统级免疫(ADR-067 决策 2)
///
/// 单厂商流量占比 EWMA > 70% 告警;独立模块(不并入 ImmuneSystem 固定三探针数组)。
pub mod concentration_probe;
pub mod config;
pub mod debate;
pub mod error;
/// FormalVerifier L4 骨架 — Parliament 形式化验证模块
///
/// 共识安全性的形式化验证：Security 一票否决不可覆盖 + 2/3 多数阈值正确性。
/// 类型复用 `nexus_contracts::formal_props`（L0 契约层）。
pub mod formal;
/// ImmuneSystem facade — 适应性免疫接口层（v5.0 §8.1 D7 / ADR-046）
///
/// 三探针（MemoryParadox/ReasoningTrap/EvolutionHack）+ 级联风险评估 + 膜厚控制。
/// 通过 event-bus 订阅 chimera-mas StabilityGuard 事件维护镜像状态，
/// 避免向 L9 向上依赖（§2.2 依赖铁律）。
pub mod immune_system;
/// P4-W14.3 S5 接缝 — Parliament 激活策略学习器持有器
///
/// 承载 `omega-learner` 异步下发的 `ParliamentPolicy`,为 `Parliament::deliberate_with_policy`
/// 提供策略感知能力。所有方法线程安全(`RwLock` 保护),C4 合规三层 fallback。
pub mod learner_holder;
/// MCA P7 跨厂商去相关 — Parliament 角色的 provider 绑定侧表(ADR-067)
///
/// Producer/Verifier/Skeptic 三方厂商绑定 + 去相关校验(同源相关失败修复),
/// 侧表方案不动 RoleProfile 与 5 角色默认配置(R8 构造点雪崩规避)。
pub mod provider_affinity;
pub mod reasoning;
pub mod roles;
/// 策略封顶守卫 — ratio 反馈驱动的审议深度降级封顶(推理悖论红线风控)
///
/// 消费 `CoordinationRatioReported` 事件,滞后带状态机维护审议策略上界
/// (Full→Simplified→FastPath);与 LinUCB S5 接缝互补(封顶为 min 上界,
/// 不替代学习);Skeptic 否决检查在任何封顶档位照常执行。
pub mod strategy_cap;
pub mod types;
/// polish-v2.7 P3-3:变体隔离池与规则式任务路由(ADR-051 决策 1/2/4)
pub mod variant_pool;
/// polish-v2.7 P3-4:变体三角色审议(Security 一票否决 + 2/3 多数,ADR-051 决策 3)
pub mod variant_review;
pub mod veto;
pub mod voting;

/// 自适应策略选择器 — 基于 ratio + 共识质量 + 系统负载的动态策略选择
///
/// 与 `StrategyCapGuard` 互补:Selector 是"建议"，CapGuard 是"强制上界"。
/// 最终策略 = min(selector 建议, cap 封顶)。
pub mod adaptive_strategy;

/// ADR-064:质量趋势分析器 — 滑动窗口跟踪共识质量趋势
///
/// 跟踪最近 N 次审议的 ConsensusQualityMetrics 滑动窗口，
/// 检测分歧度异常、弃权趋势，计算综合健康评分(0-100)。
pub mod quality_trend;

/// 悖论风险实时监控仪表盘 — 三信号融合(ratio/否决异常率/共识健康分)
///
/// 单信号超标→Yellow 预警降档，两信号超标→Red 熔断。
pub mod paradox_dashboard;

// === 关键类型重导出,简化外部导入 ===
pub use ahirt::{
    AhirtRedTeam, AhirtStats, ProbePayload, ProbePayloadLibrary, ProbeResult, ProbeType,
    SecurityReport, TypeStats,
};
pub use concentration_probe::ProviderConcentrationProbe;
pub use config::{AhirtConfig, ParliamentConfig};
pub use debate::{DpoPair, DpoPairGenerator, Parliament};
pub use error::ParliamentError;
pub use provider_affinity::{validate_cross_provider, ProviderAffinityRegistry, ProviderBinding};
// ImmuneSystem facade（ADR-046）：关键类型重导出,简化外部导入
// WHY 不重导出 `ProbeType` / `Severity`：前者与 `ahirt::ProbeType` 冲突,
// 后者与 `veto::Severity` 冲突,需通过 `parliament::immune_system::types::` 全路径访问。
// WHY 从子模块路径 re-export：immune_system.rs 私有 `use` 已将同名符号引入作用域,
// 若在 immune_system.rs 内 `pub use` 会触发 E0252,故 lib.rs 直接从子模块路径导入。
pub use immune_system::evolution_hack::EvolutionHackProbe;
pub use immune_system::membrane::MembraneController;
pub use immune_system::memory_paradox::MemoryParadoxProbe;
pub use immune_system::reasoning_trap::ReasoningTrapProbe;
pub use immune_system::types::{ImmuneSystemError, ParadoxProbe, ParadoxReport, ParadoxRiskReport};
// 直接定义在 immune_system.rs 的符号（非子模块重导出）,可直接导入
pub use immune_system::{
    compute_cascade_risk, immune_system_status, ImmuneSystem, ImmuneSystemStatus, StabilityMirror,
};
// P4-W14.3 S5 接缝:Parliament 激活策略学习器持有器
pub use learner_holder::ParliamentLearnerHolder;
pub use reasoning::{transition, ReasoningEvent, ReasoningState};
pub use roles::RoleRegistry;
// 推理悖论红线风控:策略封顶守卫公开 API
pub use strategy_cap::{
    spawn_strategy_cap_subscriber, CapChange, StrategyCapConfig, StrategyCapGuard,
};
// 自适应策略选择器公开 API
pub use adaptive_strategy::{AdaptiveStrategyConfig, AdaptiveStrategySelector, SystemLoadProbe};
// ADR-064:质量趋势分析器公开 API
pub use quality_trend::{QualityReport, QualityTrendAnalyzer};
// 悖论风险实时监控仪表盘公开 API
// WHY 不重导出 ParadoxRiskReport:与 immune_system::types::ParadoxRiskReport 冲突(E0252),
// 外部需通过 `parliament::paradox_dashboard::ParadoxRiskReport` 全路径访问。
pub use paradox_dashboard::{AlertSeverity, ParadoxRiskAlert, ParadoxRiskDashboard, RiskLevel};
pub use types::{
    Consensus, DebateResult, DeliberationCache, Opinion, Proposal, ProposalKey, Role, RoleId,
    RoleProfile,
};
// polish-v2.7 P3:变体池与审议公开 API 重导出(ADR-051)
pub use variant_pool::VariantPool;
pub use variant_review::{ReviewDecision, VariantReview};
pub use veto::{
    IntentRule, MaliciousIntentRuleBook, MaliciousIntentType, RuleAction, Severity, Skeptic,
    VetoOverrideTicket, VetoReason,
};
pub use voting::{ConsensusQualityMetrics, VoteCounter, VoteResult};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::ahirt::{
        AhirtRedTeam, AhirtStats, ProbePayload, ProbePayloadLibrary, ProbeResult, ProbeType,
        SecurityReport, TypeStats,
    };
    pub use crate::config::{AhirtConfig, ParliamentConfig};
    pub use crate::debate::{DpoPair, DpoPairGenerator, Parliament};
    pub use crate::error::ParliamentError;
    pub use crate::reasoning::{transition, ReasoningEvent, ReasoningState};
    pub use crate::roles::RoleRegistry;
    pub use crate::types::{
        Consensus, DebateResult, DeliberationCache, Opinion, Proposal, ProposalKey, Role, RoleId,
        RoleProfile,
    };
    pub use crate::veto::{
        IntentRule, MaliciousIntentRuleBook, MaliciousIntentType, RuleAction, Severity, Skeptic,
        VetoOverrideTicket, VetoReason,
    };
    pub use crate::voting::{VoteCounter, VoteResult};
    // ADR-064:质量趋势分析器
    pub use crate::quality_trend::{QualityReport, QualityTrendAnalyzer};
}
