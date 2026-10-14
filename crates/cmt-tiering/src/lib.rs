//! CRATE-CONTRACT BEGIN
//! LAYER:    L3
//! ROLE:     能力内存四级分层 — 热/温/冷/冰四级能力存储与自动迁移
//! BACKEND:  sqlite
//! PRODUCERS: 3 CapabilityTierStatsReported,CapabilityTiered,LsctTierSwitched
//! CONSUMERS: 1 chimera-cli
//! MATURITY: NOT-IMPL-CN
//! CRATE-CONTRACT END
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
//! - polish-v2.7 P4-2:分层经验回放池(TieredReplayPool,Hot/Warm/Cold/Ice 四层,
//!   失败案例 Cold 权重 0.5,R2 冻结仅供 R1 路径)
//! - Milestone B-3b:回放池完整性审计(IntegrityReport,九层防御 L3)
//! - DQN 迁移经验记录(rl_migration,MigrationExperience 回放队列)
//! - L3 深度优化:LSCT 订阅闭环(spawn_lsct_subscriber 订阅 LsctTierSwitched
//!   事件执行实际迁移)+ 四层统计事件(CapabilityTierStatsReported)
//! - M10(ADR-160 偿还):LSCT 策略装配闭环(spawn_lsct_policy 订阅 QuestCreated,
//!   reconcile 种子 + handle_quest_created 出策略,补齐决策腿)
//!   ⚠ 装配态订正(2026-10-09 v3.5 W6-4):“两腿齐备”仅在 **crate 内**成立——
//!   `spawn_lsct_policy` / `spawn_lsct_subscriber` 的调用点全在
//!   `tests/lsct_policy.rs` 与文档示例里；`CmtCoordinator::new*` 在
//!   `crates/*/src` 的非测试作用域内**零构造点**（2026-10-09 实测：仅 coordinator.rs
//!   自身 `#[cfg(test)]` 与本文档示例）。`CONSUMERS: 1 chimera-cli` 的实指消费面是
//!   `ExperienceCardStorage`（`chimera-cli/src/experience_loop.rs:29`），**不是** coordinator。
//!   ⇒ 本行应读作“落盘/迁移/经验卡片持久化已交付，**分层决策闭环未接线**”，
//!   而不是“决策腿已在线上跑”。接线属行为变更，需先定 N-4 型降级姿态并同批改台账。
//! - Phase 3 §8.1:金字塔存储映射(pyramid_storage,MemoryPyramidLevel→热温冷冰
//!   + 分层采样 25/25/50/0 + INV-8 迁移单调性)
//! - Phase 3 §8.2:经验卡片持久化(experience_card_storage,SQLite 五复合索引
//!   + 热缓存 + 三因子/错误签名查询 + 训练证据落盘 + 完整性审计)
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
/// Phase 3 §8.2:经验卡片持久化(SQLite 五复合索引 + 热缓存,ADR-049 内嵌)
pub mod experience_card_storage;
pub mod hot;
pub mod ice;
/// LSCT 任务感知策略子模块(ADR-193 架构减法:原独立 crate `lsct-tiering` 整并)
///
/// 自 ADR-160(M10)起 lsct 的唯一生产消费方就是本 crate 的 `spawn_lsct_policy`
/// 装配腿;整并后 Tier 类型统一为 `crate::types::Tier` 单一实现,
/// 跨 crate 双向转换守护层(`to_lsct_tier`/`from_lsct_tier`)随之退役。
/// 子模块完整契约见 `lsct/mod.rs` MODULE-CONTRACT 头。
pub mod lsct;
/// LSCT 策略装配(`cmt → lsct` 由跨 crate 生产边变为同 crate 模块装配,ADR-193)
///
/// `spawn_lsct_policy` 装配决策腿(QuestCreated → LSCT 策略),
/// 与既有 `spawn_lsct_subscriber`(执行腿)组成完整闭环。
pub mod lsct_policy;
pub mod migrator;
pub mod pool;
/// Phase 3 §8.1:金字塔存储映射(TencentDB 四层→热温冷冰,ADR-049 内嵌)
pub mod pyramid_storage;
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
pub use lsct_policy::LsctPolicyHandle;
pub use migrator::TierMigrator;
pub use pool::SqlitePool;
pub use storage_impl::PragmaConn;
pub use types::{
    assert_archive_monotonicity, CapabilityEntry, CapabilityId, MigrationReason, Tier,
};
pub use warm::WarmTier;

// === Phase 3 L3 存储层五大组件重导出(§8.1/§8.2) ===
pub use experience_card_storage::{CardStorageIntegrityReport, ExperienceCardStorage};
pub use pyramid_storage::{PyramidStorageMapper, PYRAMID_SAMPLE_RATIOS};

// === L3 深度优化 P1-1: 四层存储分布(事件驱动化) ===

/// 四层存储分布 — 热/温/冷/冰四层**条目数**快照
///
/// L3 深度优化语义变更:字段原标注"字节数"实为 TODO 占位(恒零),
/// 现改为条目计数(由 `CmtCoordinator::tier_stats()` 采集:Hot len /
/// Warm·Cold COUNT(*)/Ice 归档文件数),经 CapabilityTierStatsReported
/// 事件发布,消费方从事件流派生显示。
///
/// WHY 结构体而非元组: 四层语义明确,命名字段避免混淆(热/温/冷/冰易记混位置)。
/// 所有字段标注 `#[doc]` 以保持 `missing_docs` lint 合规。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TierDistribution {
    /// 热层条目数(Hot Tier: DashMap + LRU,容量 256)
    pub hot: u64,
    /// 温层条目数(Warm Tier: SQLite WAL,容量 4096)
    pub warm: u64,
    /// 冷层条目数(Cold Tier: SQLite 附加数据库,容量 65536)
    pub cold: u64,
    /// 冰层条目数(Ice Tier: 归档只读文件,无容量上限)
    pub frozen: u64,
}

/// v2.3 §(b.5) 落盘扫描策略(本 crate 单一口径)。
///
/// 只拦截硬载荷（私钥 PEM / 云凭证 / 服务令牌 = `Block`），高熵串与键名
/// 启发式降为 `Pass` 防误报（误报会让正常能力被无声丢弃）。
///
/// WHY 单源：此前本 crate 的 warm/cold/ice/experience_card_storage 各自逐字
/// 复制同一匿名结构体（五份）。策略一旦分叉，“各层同口径”只能靠人记住，
/// 而 G-84 升入口级后需要逐入口插桩——插桩点数增加必然放大复制风险。
/// 以 `pub(crate)` 收口：对外 API 零变化，仅 crate 内部共享。
pub(crate) fn hard_payload_scan_policy() -> nexus_contracts::sensitive::ScanPolicy {
    nexus_contracts::sensitive::ScanPolicy {
        on_high_entropy: nexus_contracts::sensitive::SensitiveDisposition::Pass,
        on_secret_kv: nexus_contracts::sensitive::SensitiveDisposition::Pass,
        ..Default::default()
    }
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
    pub use crate::types::{
        assert_archive_monotonicity, CapabilityEntry, CapabilityId, MigrationReason, Tier,
    };
    pub use crate::warm::WarmTier;
    // Phase 3 L3 存储层五大组件(§8.1/§8.2,与顶层导出同集)
    pub use crate::experience_card_storage::{CardStorageIntegrityReport, ExperienceCardStorage};
    pub use crate::pyramid_storage::{PyramidStorageMapper, PYRAMID_SAMPLE_RATIOS};
}
