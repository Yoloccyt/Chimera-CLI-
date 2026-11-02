//! 向量与共享算法组 — 向量存储抽象、Top-K 收敛工具、MCSM Sinkhorn 投影、RRF 融合（L0 受控纯逻辑）。
//!
//! 成员经 crate 根 re-export，旧路径 `crate::<leaf>` / `nexus_contracts::<leaf>` 不变。

pub mod mcsm;
pub mod recall_fusion;
pub mod util;
pub mod vector;
