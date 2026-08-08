//! 能力内存四级分层 — 热/温/冷/冰四级能力存储与自动迁移
//!
//! 对应架构层:L3 Storage
//! 对应创新点:CMT(Capability Memory Tiering)
//!
//! # 核心职责
//! - 实现 HotTier(DashMap + LRU,容量 256,延迟 < 1μs)
//! - 实现 WarmTier(SQLite WAL 模式,容量 4096,延迟 < 5ms)
//! - 实现 ColdTier(SQLite 附加数据库,容量 65536,延迟 < 50ms)
//! - 实现 IceTier(归档只读文件,无容量上限,延迟 < 500ms)
//! - 通过 CmtCoordinator 统一接口聚合四级,自动跨层查找与提升
//! - 集成 EventBus,发布 CapabilityTiered 事件
//! - 基于访问频率与时间衰减的自动迁移(priority < 0.1 触发降级)
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - DashMap 写锁释放后再调用 async 方法(避免死锁,Week 2 经验教训)
//! - Cold 层文件 I/O 使用 tokio::task::spawn_blocking 包装
//! - 所有 async fn 满足 Send + 'static 约束
//!
//! # 快速示例
//! ```no_run
//! use cmt_tiering::{CmtCoordinator, CmtConfig, CapabilityEntry, Tier};
//! use event_bus::EventBus;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = EventBus::new();
//! let coordinator = CmtCoordinator::new_in_memory(CmtConfig::default(), bus)?;
//!
//! let entry = CapabilityEntry::new("cap-1", "内容", Tier::Hot);
//! coordinator.insert(entry).await?;
//!
//! let fetched = coordinator.get("cap-1").await?;
//! assert!(fetched.is_some());
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod cold;
pub mod config;
pub mod coordinator;
pub mod decay;
pub mod error;
pub mod hot;
pub mod ice;
pub mod migrator;
pub mod pool;
pub mod rl_migration;
/// polish-v2.7 P4-2:分层经验回放池(Hot/Warm/Cold/Ice 四层,ADR-049)
///
/// R2 冻结声明(ADR-042):经验数据仅供 R1 路径。
pub mod rl_replay_pool;
pub mod storage_impl;
pub mod types;
pub mod warm;

// === 关键类型重导出,简化外部导入 ===
pub use cold::ColdTier;
pub use config::CmtConfig;
pub use coordinator::CmtCoordinator;
pub use decay::{DecayCalculator, DEMOTION_THRESHOLD};
pub use error::CmtError;
pub use hot::HotTier;
pub use ice::IceTier;
pub use migrator::TierMigrator;
pub use pool::SqlitePool;
pub use storage_impl::PragmaConn;
pub use types::{CapabilityEntry, CapabilityId, MigrationReason, Tier};
pub use warm::WarmTier;

// === Task 3.3: L10 TUI 跨层协同 — 四层存储分布快照 ===

/// 四层存储分布 — 热/温/冷/冰四层字节数快照(Task 3.3)
///
/// WHY 结构体而非元组: 四层语义明确,命名字段避免混淆(热/温/冷/冰易记混位置)。
/// 所有字段标注 `#[doc]` 以保持 `missing_docs` lint 合规。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TierDistribution {
    /// 热层字节数(Hot Tier: DashMap + LRU,容量 256)
    pub hot: u64,
    /// 温层字节数(Warm Tier: SQLite WAL,容量 4096)
    pub warm: u64,
    /// 冷层字节数(Cold Tier: SQLite 附加数据库,容量 65536)
    pub cold: u64,
    /// 冰层字节数(Ice Tier: 归档只读文件,无容量上限)
    pub frozen: u64,
}

/// 返回四层存储分布快照(Task 3.3 跨层 Panel 数据管道)
///
/// TUI MemoryPanel 调用此函数显示"存储分布"字段。
/// 当前返回默认全零值(TODO: 真实接入 CmtCoordinator 统计各层字节数)。
///
/// # 示例
///
/// ```
/// use cmt_tiering::{tier_distribution, TierDistribution};
///
/// let dist = tier_distribution();
/// assert_eq!(dist.hot + dist.warm + dist.cold + dist.frozen, 0);
/// ```
pub fn tier_distribution() -> TierDistribution {
    // TODO: 真实接入 CmtCoordinator 统计各层条目大小
    // 当前返回默认值(全零),待 CmtCoordinator 暴露 per-tier byte count 后替换
    TierDistribution::default()
}

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::cold::ColdTier;
    pub use crate::config::CmtConfig;
    pub use crate::coordinator::CmtCoordinator;
    pub use crate::decay::{DecayCalculator, DEMOTION_THRESHOLD};
    pub use crate::error::CmtError;
    pub use crate::hot::HotTier;
    pub use crate::ice::IceTier;
    pub use crate::migrator::TierMigrator;
    pub use crate::pool::SqlitePool;
    pub use crate::types::{CapabilityEntry, CapabilityId, MigrationReason, Tier};
    pub use crate::warm::WarmTier;
}
