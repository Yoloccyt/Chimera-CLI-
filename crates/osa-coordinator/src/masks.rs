//! 稀疏掩码容器 — 从 L0 nexus-contracts 统一导入(ADR-033, P2-W5.2)
//!
//! 对应架构层:L6 Router(类型定义上提至 L0 Contracts)
//!
//! # 迁移说明
//! 原 `SparseMask<T>` 定义在本 crate,与 nexus-contracts 中的定义完全一致。
//! P2-W5.2 将类型定义上提至 L0,本 crate 改为 re-export,
//! 消除跨 crate 的类型分裂(osa_coordinator::SparseMask = nexus_contracts::SparseMask)。
//!
//! SparseMask 的全部方法(empty / full / select_top_k / is_active / active_count / sparsity)
//! 与字段(active_ids / sparsity_ratio / active_set)均由 nexus-contracts 提供,
//! 单元测试亦在 nexus-contracts 中覆盖(47 测试全绿)。

// SparseMask 从 L0 nexus-contracts 统一导入
// WHY:消除与 kvbsr-router/faae-router 的 SparseMask 类型分裂,
// 三 crate 现共享同一 SparseMask<T> 泛型容器。
pub use nexus_contracts::SparseMask;
