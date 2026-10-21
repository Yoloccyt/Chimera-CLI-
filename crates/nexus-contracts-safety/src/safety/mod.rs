//! 安全与治理契约组 — 命令验证、预算档位、裁决权/认识类型、校准、敏感扫描、统一错误层级（L0 契约）。
//!
//! 成员经 crate 根 re-export，旧路径 `crate::<leaf>` / `nexus_contracts::<leaf>` 不变。

pub mod adjudication;
pub mod budget_tier;
pub mod calibration;
pub mod command_validation;
pub mod errors;
pub mod sensitive;
