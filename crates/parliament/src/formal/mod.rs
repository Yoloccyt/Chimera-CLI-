//! FormalVerifier L4 骨架 — Parliament 形式化验证模块
//!
//! 架构层归属: L8 (Parliament)
//! 核心职责: 共识安全性的形式化验证
//!
//! # 设计决策(WHY)
//!
//! - 本模块为 Parliament 的形式化验证入口，验证共识机制的核心安全属性
//! - 所有验证函数为纯函数，无副作用，便于 proptest 属性测试与手动证明
//! - 类型定义复用 `nexus_contracts::formal_props`（L0 契约层），
//!   遵循 §2.2 依赖铁律（L8 → L0 向下依赖允许）

pub mod consensus_safety;
