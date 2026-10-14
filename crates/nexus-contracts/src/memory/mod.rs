//! 记忆与经验卡片契约组 — 归档单调性、记忆金字塔、经验卡片、Token 证据、Skill 生命周期、命名空间配额（L0 契约）。
//!
//! 成员经 crate 根 re-export，旧路径 `crate::<leaf>` / `nexus_contracts::<leaf>` 不变。

pub mod archive_monotonicity;
pub mod experience_card;
pub mod memory_pyramid;
pub mod quota;
pub mod skill_lifecycle;
pub mod token_evidence;
