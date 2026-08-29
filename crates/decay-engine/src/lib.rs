//! 能力衰减引擎 — 连续 [0.0, 1.0] 权限流体衰减模型
//!
//! 对应架构层:L4 Security
//! 对应 ADR-002:能力衰减模型设计(连续权限流体)
//! 对应尸检教训:Claude 安全权限提升(权限不应离散 0/1)
//!
//! 双驱动衰减:
//! - 时间驱动:随时间自然递减(防止权限长期闲置累积)
//! - 事件驱动:违规事件触发惩罚性衰减
//!
//! 冻结/解冻 API 对应 Skeptic 否决权(Week 5 Parliament 实现)
//!
//! # 承载职责清单(L4 深度优化补充)
//! - 衰减引擎本体(engine.rs,双驱动模型)
//! - P4-W14.4 S6 接缝(learner_holder.rs,DecayLearnerHolder 策略异步下发)
//! - P4-W14.5 能力场令牌注册表(capability_registry.rs,C4 灰度授权)
//! - R2 解冻阶段③ 影子模式熔断开关(shadow_breaker.rs,fail-closed 护栏;
//!   L4 深度优化 P1-1:跳闸经 ShadowBreakerTripped 事件发布,事件驱动化)
//! - FormalVerifier M2 衰减一致性验证(formal.rs)
//!
//! # P4-W14.4 S6 接缝扩展
//!
//! 新增 `decay_with_policy` 方法支持策略感知衰减(详见 `learner_holder` 模块):
//! - `DecayLearnerHolder`: 运行时可变 `DecayPolicy` 容器(RwLock 保护)
//! - `decay_with_policy(id, event, policy)`: 接收 L0 契约层 `DecayPolicy`,
//!   从中提取 `DecayProfile` 转换为临时 `DecayConfig` 应用到本次衰减
//! - C4 合规三层 fallback: 默认 Static(Standard) + PoisonError 自动回退 + 熔断入口
//!
//! # 快速示例
//! WHY 选此示例:展示最常用路径 —— 注册能力 + 事件驱动惩罚衰减,体现双驱动模型的核心。
//! ```
//! use decay_engine::{DecayEngine, DecayConfig, DecayEvent};
//!
//! let engine = DecayEngine::new(DecayConfig::default());
//! engine.register_capability("file_write", "文件写入", 1.0).unwrap();
//! // 违规事件触发惩罚性衰减(severity=2.0 加重违规,penalty=0.1×2.0=0.2)
//! let level = engine.decay("file_write", DecayEvent::ViolationPenalty {
//!     capability_id: "file_write".into(),
//!     severity: 2.0,
//! }).unwrap();
//! assert!(level.value() < 1.0, "违规后权限应下降");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

/// P4-W14.5: 能力场令牌注册表（C4 合规灰度授权管理）
pub mod capability_registry;
pub mod engine;
pub mod error;
/// FormalVerifier M2 — 能力衰减一致性形式化验证（Phase 8.1,ADR-047 Property #6）
///
/// 验证衰减观测轨迹（衰减单调性/有界性/Freeze 归零不可逆）而非引擎内部状态,
/// 类型复用 `nexus_contracts::formal_props`（L0 契约层）。是 R2 解冻阶段① 前置。
pub mod formal;
/// P4-W14.4: DecayEngine 学习器持有器（S6 接缝策略异步下发 + 本地 fallback）
pub mod learner_holder;
/// P1-T14: 批量能力衰减的 ComputeBridge 并行注入（快照分离 + 串行提交 + 回退）
///
/// 批量衰减热点（v4.0 §7.5.1 L-a）经 [`nexus_core::compute`] 全局计算池并行:
/// 逐能力独立衰减 → 主线程快照 → rayon 闭包纯计算 → 串行提交;
/// `CHIMERA_NO_PARALLEL_DECAY` env + [`DecayEngine::parallel_batch`] 双重回退。
pub mod parallel;
/// R2 解冻阶段③ 前置 3 — 影子模式熔断开关（fail-closed 安全护栏，ADR-052 待办 3）
///
/// 消费 FormalVerifier 验证结果流，任一属性 Violated 即永久跳闸拒绝 RL 更新，
/// 复用 decay-engine Freeze 不可逆哲学。不执行 RL 训练，不含 R2 扫描关键词。
pub mod shadow_breaker;
pub mod types;

pub use capability_registry::{current_utc_secs, CapabilityTokenRegistry};
pub use engine::DecayEngine;
pub use error::DecayError;
// FormalVerifier M2:衰减一致性验证器重导出（Phase 8.1）
pub use formal::{DecayConsistencyChecker, DecayEventKind, LevelTransition};
pub use learner_holder::DecayLearnerHolder;
// R2 解冻阶段③ 前置 3:影子模式熔断开关重导出
pub use shadow_breaker::{BreakerState, RlGateVerdict, ShadowModeCircuitBreaker};
// 评审 S-2.1:复位授权凭证重导出
pub use shadow_breaker::{ResetAuthError, ResetAuthorization};
pub use types::{Capability, CapabilityLevel, DecayConfig, DecayEvent};

/// 默认衰减配置
///
/// 生产推荐值:
/// - time_decay_rate: 0.001(每秒衰减 0.1%)
/// - event_decay_penalty: 0.1(标准违规惩罚)
/// - freeze_threshold: 0.05(5% 以下自动冻结)
pub fn default_config() -> DecayConfig {
    DecayConfig::default()
}

// L4 深度优化 P1-1/P1-2:原 Task 3.4 的 shadow_breaker_status() 全局函数与
// ShadowBreakerStatus 三虚设字段结构体(token_burn/memory_freeze/network_isolate)
// 已移除。三字段中仅 memory_freeze 有实现载体(ShadowModeCircuitBreaker 单维度
// Armed/Tripped 状态机),其余两维无实现——保留即制造虚假语义。
// 熔断状态的可观测传播改走 Ω₄-Event:ShadowModeCircuitBreaker::with_event_bus
// 装配后跳闸发布 ShadowBreakerTripped 事件,TUI DecayPanel 从 latest_events
// 派生显示——与 L2/L3 任务事件驱动化治理同构。
