//! CRATE-CONTRACT BEGIN
//! LAYER:    L0
//! ROLE:     NEXUS-OMEGA L0-A 契约域 — `safety`（ADR-222 D2）
//! BACKEND:  memory
//! PRODUCERS: 0 -
//! CONSUMERS: 1 nexus-contracts
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! NEXUS-OMEGA L0-A 契约域 — `safety`（ADR-222 D2）
//!
//! 对应架构层: **L0-A 纯契约**（机器层号 0；L0 = L0-A + L0-B 见 ADR-222 D1）
//! 对应 ADR: ADR-033（受控纯逻辑）/ ADR-222 D2（1+6 拆分边界）
//!
//! # 为什么单独成 crate
//!
//! `safety` 是 ADR-222 D2 实测组间 DAG 上的一个独立域。sibling 内部保留原组目录
//! （`src/safety/`），因此 G-105/G-113/G-115/G-119 的模块键逐字不变 —— 拆分对
//! 台账是零换键的。
//!
//! # facade 兼容
//!
//! `nexus-contracts`（umbrella）以 `pub use nexus_contracts_safety::safety;` + 根级 leaf re-export
//! 保持 `nexus_contracts::<leaf>::Type` 与 `nexus_contracts::safety::<leaf>::Type`
//! 两条路径逐字不变（34 个消费 crate 零改动）。
//!
//! # 纯度
//!
//! 本 crate 属 L0-A：`#![forbid(unsafe_code)]`，依赖白名单由 G-99 机器锁。

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod safety;

// 根 re-export：保持拆分前 `crate::<leaf>` 路径（组内源码的交叉引用）。
pub use safety::adjudication;
pub use safety::budget_tier;
pub use safety::calibration;
pub use safety::command_validation;
pub use safety::errors;
pub use safety::sensitive;
