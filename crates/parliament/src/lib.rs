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
pub mod reasoning;
pub mod roles;
pub mod types;
/// polish-v2.7 P3-3:变体隔离池与规则式任务路由(ADR-051 决策 1/2/4)
pub mod variant_pool;
/// polish-v2.7 P3-4:变体三角色审议(Security 一票否决 + 2/3 多数,ADR-051 决策 3)
pub mod variant_review;
pub mod veto;
pub mod voting;

// === 关键类型重导出,简化外部导入 ===
pub use ahirt::{
    AhirtRedTeam, AhirtStats, ProbePayload, ProbePayloadLibrary, ProbeResult, ProbeType,
    SecurityReport, TypeStats,
};
pub use config::{AhirtConfig, ParliamentConfig};
pub use debate::{DpoPair, DpoPairGenerator, Parliament};
pub use error::ParliamentError;
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
pub use immune_system::{compute_cascade_risk, ImmuneSystem, StabilityMirror};
// P4-W14.3 S5 接缝:Parliament 激活策略学习器持有器
pub use learner_holder::ParliamentLearnerHolder;
pub use reasoning::{transition, ReasoningEvent, ReasoningState};
pub use roles::RoleRegistry;
pub use types::{Consensus, DebateResult, Opinion, Proposal, Role, RoleId, RoleProfile};
// polish-v2.7 P3:变体池与审议公开 API 重导出(ADR-051)
pub use variant_pool::VariantPool;
pub use variant_review::{ReviewDecision, VariantReview};
pub use veto::{
    IntentRule, MaliciousIntentRuleBook, MaliciousIntentType, RuleAction, Severity, Skeptic,
    VetoOverrideTicket, VetoReason,
};
pub use voting::{VoteCounter, VoteResult};

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
    pub use crate::types::{Consensus, DebateResult, Opinion, Proposal, Role, RoleId, RoleProfile};
    pub use crate::veto::{
        IntentRule, MaliciousIntentRuleBook, MaliciousIntentType, RuleAction, Severity, Skeptic,
        VetoOverrideTicket, VetoReason,
    };
    pub use crate::voting::{VoteCounter, VoteResult};
}
