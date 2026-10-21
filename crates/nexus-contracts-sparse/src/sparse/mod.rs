//! 稀疏掩码体系组 — ID newtype + `SparseMask<T>` + `OmniSparseMasks`（L0 契约）。
//!
//! v3.5 ADR-212 W3：原 53 平铺模块收敛为内聚子模块树。crate 根对每个成员
//! re-export（`pub use <group>::<leaf>;`），使 `crate::<leaf>` 与
//! `nexus_contracts::<leaf>` 旧路径**逐字不变**，facade（`pub use` + `prelude`）不受影响。

pub mod ids;
pub mod masks;
pub mod omni_masks;
