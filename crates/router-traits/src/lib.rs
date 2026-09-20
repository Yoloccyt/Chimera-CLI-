//! L6 Router Traits — 路由层公共 trait 定义
//!
//! ★ Insight: 星型耦合转 Trait 抽象是 Rust 惯用模式（如 serde/tonic 的设计哲学）。
//! 依赖倒置原则 (DIP): 高层模块不应依赖低层模块，都应依赖抽象。
//!
//! 本 crate 提供：
//! - `SparseMaskProvider`: OSA 稀疏掩码提供者接口
//! - `RouterConfig`: 路由配置契约
//! - `ZeroOrphanGuarantee`: QEEP 零孤儿调用保证（L6→L4 跨层规范）
//!
//! **依赖方向**:
//! - 生产依赖：nexus-contracts (L0), event-bus (L1) ✓ 向下依赖允许
//! - 消费方：osa-coordinator, kvbsr-router, faae-router 等实现 trait

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod masks;
pub mod security;

pub use config::{RouterConfig, RouterId};
pub use masks::{SparseMaskError, SparseMaskProvider};
pub use security::{OrphanDetector, OrphanReason, ZeroOrphanGuarantee};
