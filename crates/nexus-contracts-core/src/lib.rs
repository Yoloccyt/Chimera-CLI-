//! CRATE-CONTRACT BEGIN
//! LAYER:    L0
//! ROLE:     NEXUS-OMEGA L0-A 领域契约 — `core_domain`（ADR-222 D2）
//! BACKEND:  memory
//! PRODUCERS: 0 -
//! CONSUMERS: 2 nexus-contracts,nexus-contracts-memory
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! NEXUS-OMEGA L0-A 领域契约 — `core_domain`（ADR-222 D2）
//!
//! 对应架构层: **L0-A 纯契约**（机器层号 0；L0-A/L0-B 双层定义见 ADR-222 D1）
//! 对应 ADR: **ADR-033**（L0 建立 + 受控纯逻辑）/ **ADR-054 决策 6**（领域类型上提）/
//! **ADR-222 D2**（1+6 拆分边界）
//!
//! # 为什么单独成 crate
//!
//! `core_domain` 是 L0-A 内所有其它契约域的**基座**（`memory` / `spec` 都依赖它）。
//! ADR-222 D2 以实测组间 DAG 为依据把它拆为独立 sibling：它没有 L0-A 内出边，
//! 因而可以先行编译，也不会把 umbrella 变成 siblings 的循环依赖节点。
//!
//! # facade 兼容
//!
//! 下方 `pub use core_domain::<leaf>;` 保持拆分前 `crate::<leaf>` 的 crate 内路径
//! （`core_domain/*.rs` 的交叉引用正是这些路径）；`nexus-contracts` 的 umbrella
//! 仍以 `pub use nexus_contracts_core::core_domain;` + 根级 `pub use core_domain::<leaf>;`
//! 保持 `nexus_contracts::<leaf>::Type` 与 `nexus_contracts::core_domain::<leaf>::Type`
//! 两条外部路径逐字不变（34 个消费 crate 零改动）。
//!
//! # 纯度
//!
//! 本 crate 属 L0-A：`#![forbid(unsafe_code)]`，依赖白名单仅
//! serde/chrono/uuid（G-99 扫描辖区，见 `scripts/check_l0_purity.py`）。

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod core_domain;

// 根 re-export：保持拆分前 `crate::<leaf>` 路径（core_domain/*.rs 的交叉引用）。
pub use core_domain::checkpoint;
pub use core_domain::domain;
pub use core_domain::event_metadata;
pub use core_domain::event_payload;
pub use core_domain::event_v2;
pub use core_domain::graph_identity;
pub use core_domain::task;
pub use core_domain::temporal;
