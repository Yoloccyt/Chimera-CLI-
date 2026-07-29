//! FormalVerifier M1 — AutoDPO 形式化验证模块(P7-T3)
//!
//! 对应架构层: L4 FormalVerifier(auto-dpo 内验证器实现)
//! 对应 ADR: ADR-047(M1 集成验证)+ ADR-042(R2 解冻阶段 1 前置)
//!
//! 承载偏好对一致性验证器,类型复用 `nexus_contracts::formal_props`(L0 契约层),
//! 与 gsoe `formal/` / parliament `formal/` 构成 M1 五属性验证器矩阵。

pub mod preference_consistency;

pub use preference_consistency::PreferenceConsistencyChecker;
