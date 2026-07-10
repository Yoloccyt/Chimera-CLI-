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
pub mod distributed_veto;
pub mod error;
pub mod roles;
pub mod types;
pub mod veto;
pub mod voting;

// === 关键类型重导出,简化外部导入 ===
pub use ahirt::{
    AhirtRedTeam, AhirtStats, ProbePayload, ProbePayloadLibrary, ProbeResult, ProbeType,
    SecurityReport, TypeStats,
};
pub use config::{AhirtConfig, ParliamentConfig};
pub use debate::{DpoPair, DpoPairGenerator, Parliament};
pub use distributed_veto::{
    DistributedSkepticCluster, DistributedVetoResult, NodeVetoReason, SkepticNode,
};
pub use error::ParliamentError;
pub use roles::RoleRegistry;
pub use types::{Consensus, DebateResult, Opinion, Proposal, Role, RoleId, RoleProfile};
pub use veto::{
    IntentRule, MaliciousIntentRuleBook, MaliciousIntentType, RuleAction, Severity, Skeptic,
    VetoOverrideTicket, VetoReason,
};
pub use voting::{BordaVoteCounter, VoteCounter, VoteResult};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::ahirt::{
        AhirtRedTeam, AhirtStats, ProbePayload, ProbePayloadLibrary, ProbeResult, ProbeType,
        SecurityReport, TypeStats,
    };
    pub use crate::config::{AhirtConfig, ParliamentConfig};
    pub use crate::debate::{DpoPair, DpoPairGenerator, Parliament};
    pub use crate::distributed_veto::{
        DistributedSkepticCluster, DistributedVetoResult, NodeVetoReason, SkepticNode,
    };
    pub use crate::error::ParliamentError;
    pub use crate::roles::RoleRegistry;
    pub use crate::types::{Consensus, DebateResult, Opinion, Proposal, Role, RoleId, RoleProfile};
    pub use crate::veto::{
        IntentRule, MaliciousIntentRuleBook, MaliciousIntentType, RuleAction, Severity, Skeptic,
        VetoOverrideTicket, VetoReason,
    };
    pub use crate::voting::{BordaVoteCounter, VoteCounter, VoteResult};
}
