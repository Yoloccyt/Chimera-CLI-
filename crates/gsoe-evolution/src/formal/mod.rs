//! 形式化验证模块 — AEGIS Critic 单调性、谱系完整性等不变量验证
//!
//! 对应架构层: L4 FormalVerifier
//!
//! # 子模块
//!
//! - `critic_monotonicity`: AEGIS Critic 评分单调性验证（防奖励黑客核心保证）
//! - `lineage_checker`: 谱系图 DAG 性质 + 回滚可达性 + 变异幅度硬上限验证
//! - `invariant_closure`: 全系统不变量传递闭包验证（M2 属性 #7,依赖无环 +
//!   满足传播 + 终端锚点不可绕过）

pub mod critic_monotonicity;
pub mod invariant_closure;
pub mod lineage_checker;
