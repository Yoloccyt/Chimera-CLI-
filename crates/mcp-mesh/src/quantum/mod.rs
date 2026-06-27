//! 量子子系统模块入口
//!
//! 对应架构层:L10 Interface
//!
//! ## 子模块
//! - `transaction`:量子事务状态机(2PC 占位)
//! - `superposition`:超位置查询并发 fanout
//! - `entanglement`:纠缠链接与同步策略

pub mod entanglement;
pub mod superposition;
pub mod transaction;
