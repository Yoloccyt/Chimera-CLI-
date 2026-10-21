//! S1-S9 学习接缝策略契约组 — 各接缝的静态配置契约 + RL 共享类型（L0 契约）。
//!
//! 成员经 crate 根 re-export，旧路径 `crate::<leaf>` / `nexus_contracts::<leaf>` 不变。

pub mod capability_token;
pub mod decay_profile;
pub mod density;
pub mod memory_strategy;
pub mod parliament_policy;
pub mod policy;
pub mod prefetch;
pub mod recall_quota;
pub mod reward;
pub mod rl_hooks;
pub mod rl_types;
pub mod strategy;
