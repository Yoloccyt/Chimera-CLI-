//! CMT 协调器 — 四级能力内存的统一接口与自动迁移
//!
//! 对应架构层:L3 Storage
//! 对应创新点:CMT(Capability Memory Tiering)
//!
//! # 核心职责
//! - 聚合 Hot/Warm/Cold/Ice 四级存储,提供统一的 CRUD 与自动迁移接口
//! - 跨层查找自动提升(`get` 找到后提升到 Hot 层)
//! - 跨层删除所有副本(`delete`)
//! - 基于衰减的自动降级(`run_decay_cycle`)
//! - 集成 EventBus,发布 `CapabilityTiered` 事件
//!
//! # 与 TierMigrator 的关系
//! - `CmtCoordinator` 提供高级自动管理(跨层查找、衰减降级)
//! - `TierMigrator` 提供低级手动迁移原语(见 `migrator.rs`)
//! - 两者在 `promote_to_hot` 逻辑上重复,保留独立实现(详见各自 WHY 注释)

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{debug, info};

use crate::cold::ColdTier;
use crate::config::CmtConfig;
use crate::decay::DecayCalculator;
use crate::error::CmtError;
use crate::hot::HotTier;
use crate::ice::IceTier;
use crate::types::{CapabilityEntry, CapabilityId, MigrationReason, Tier};
use crate::warm::WarmTier;

/// 衰减周期分批处理的批大小(SubTask 19.2)
///
/// WHY 1024:平衡事务大小与内存控制。每批 1024 条目,
/// 单批 content 内存约 1-4MB(取决于 content 大小),
/// 批间释放避免 65536 条目全量加载的内存峰值(80%+ 降低)。
const DECAY_BATCH_SIZE: usize = 1024;

/// CMT 协调器 — 四级能力内存的统一接口
///
/// 聚合 Hot/Warm/Cold/Ice 四级存储,提供统一的 CRUD 与自动迁移接口。
///
/// # 设计决策(WHY)
/// - **跨层查找自动提升**:`get(cap_id)` 自动跨层查找(Hot → Warm → Cold → Ice),
///   找到后提升到 Hot 层(若 Hot 未满,否则触发 LRU 驱逐后提升)
/// - **跨层删除**:`delete(cap_id)` 跨层删除所有副本,确保数据一致性
/// - **衰减周期**:`run_decay_cycle` 扫描所有层,将 priority < 0.1 的条目降级
/// - **Arc 包装**:CmtCoordinator 内部所有层级都是线程安全的,
///   Arc 包装支持跨任务共享(如 tokio::spawn)
///
/// # 线程安全
/// 所有层级都是线程安全的(HotTier DashMap,Warm/Cold Mutex,Ice 文件系统)。
/// `EventBus` 基于 `tokio::broadcast`,Clone 廉价(Arc 引用计数)。
/// 所有 async fn 满足 `Send + 'static` 约束。
pub struct CmtCoordinator {
    /// Hot 层(DashMap + LRU)
    hot: HotTier,
    /// Warm 层(SQLite WAL)
    warm: WarmTier,
    /// Cold 层(SQLite 附加数据库)
    cold: ColdTier,
    /// Ice 层(归档只读文件)
    ice: IceTier,
    /// 事件总线(用于发布 CapabilityTiered 事件)
    event_bus: EventBus,
    /// 引擎配置
    config: CmtConfig,
    /// 衰减计算器
    decay: Arc<DecayCalculator>,
}

impl CmtCoordinator {
    /// 创建 CMT 协调器,使用指定配置与 EventBus
    ///
    /// 会自动打开 Warm/Cold/Ice 层的持久化存储(路径从 config 读取,展开 `~`)
    pub fn new(config: CmtConfig, event_bus: EventBus) -> Result<Self, CmtError> {
        config.validate()?;
        let decay = Arc::new(DecayCalculator::from_config(&config)?);

        // 展开 `~` 并打开 Warm 层 SQLite 数据库
        let warm_db_path = CmtConfig::expand_tilde(&config.warm_db_path);
        if let Some(parent) = warm_db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CmtError::StorageError(format!(
                    "创建 Warm 层数据库目录失败: {} - {}",
                    parent.display(),
                    e
                ))
            })?;
        }
        let warm = WarmTier::open(&warm_db_path, config.warm_capacity)?;

        // 展开 `~` 并打开 Cold 层(附加数据库)
        let cold_dir = CmtConfig::expand_tilde(&config.cold_dir);
        std::fs::create_dir_all(&cold_dir).map_err(|e| {
            CmtError::StorageError(format!(
                "创建 Cold 层目录失败: {} - {}",
                cold_dir.display(),
                e
            ))
        })?;
        let cold = ColdTier::open(&cold_dir, config.cold_capacity)?;

        // 展开 `~` 并打开 Ice 层(归档文件)
        let ice_dir = CmtConfig::expand_tilde(&config.ice_dir);
        std::fs::create_dir_all(&ice_dir).map_err(|e| {
            CmtError::StorageError(format!(
                "创建 Ice 层目录失败: {} - {}",
                ice_dir.display(),
                e
            ))
        })?;
        let ice = IceTier::new(ice_dir);

        let hot = HotTier::new(config.hot_capacity);

        Ok(Self {
            hot,
            warm,
            cold,
            ice,
            event_bus,
            config,
            decay,
        })
    }

    /// 创建用于测试的 CMT 协调器(所有层使用内存/临时存储)
    ///
    /// WHY:测试场景不需要持久化,内存数据库更快且自动清理。
    /// Ice 层使用系统临时目录下的唯一子目录,避免测试间相互干扰。
    ///
    /// # 唯一性保证
    /// 使用静态原子计数器 + 进程 ID + 时间戳纳秒生成唯一目录名,
    /// 确保并发测试不会生成相同路径(时间戳毫秒精度不足)。
    /// 创建前清理可能存在的同名目录,防止遗留文件污染。
    pub fn new_in_memory(config: CmtConfig, event_bus: EventBus) -> Result<Self, CmtError> {
        // 静态原子计数器:每次调用递增,确保同一进程内唯一
        static ICE_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

        config.validate()?;
        let decay = Arc::new(DecayCalculator::from_config(&config)?);

        let hot = HotTier::new(config.hot_capacity);
        let warm = WarmTier::open_in_memory(config.warm_capacity)?;
        let cold = ColdTier::open_in_memory(config.cold_capacity)?;

        // Ice 层使用系统临时目录下的唯一子目录
        // WHY:不依赖 tempfile crate(它是 dev-dependency,不能在正式代码中使用)。
        // 使用 进程ID + 原子计数器 + 时间戳纳秒 三重保证唯一性,
        // 避免并发测试间冲突(时间戳毫秒精度不足,可能生成相同路径)
        let counter = ICE_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let ice_tmp_dir = std::env::temp_dir().join(format!(
            "cmt-tiering-test-{}-{}-{}",
            std::process::id(),
            counter,
            Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));

        // 清理可能存在的同名目录(防止遗留文件污染测试)
        // WHY:虽然目录名包含原子计数器,理论上不会重复,但为防御性编程仍清理一次
        if ice_tmp_dir.exists() {
            let _ = std::fs::remove_dir_all(&ice_tmp_dir);
        }

        let ice = IceTier::new(ice_tmp_dir);

        Ok(Self {
            hot,
            warm,
            cold,
            ice,
            event_bus,
            config,
            decay,
        })
    }

    /// 获取配置引用
    pub fn config(&self) -> &CmtConfig {
        &self.config
    }

    /// 获取 Hot 层引用
    pub fn hot(&self) -> &HotTier {
        &self.hot
    }

    /// 获取 Warm 层引用
    pub fn warm(&self) -> &WarmTier {
        &self.warm
    }

    /// 获取 Cold 层引用
    pub fn cold(&self) -> &ColdTier {
        &self.cold
    }

    /// 获取 Ice 层引用
    pub fn ice(&self) -> &IceTier {
        &self.ice
    }

    /// 插入能力条目到 Hot 层
    ///
    /// 若 Hot 层已满,LRU 驱逐最久未访问的条目到 Warm 层。
    /// 插入后发布 `CapabilityTiered` 事件(若发生 LRU 驱逐)。
    pub async fn insert(&self, entry: CapabilityEntry) -> Result<(), CmtError> {
        let cap_id = entry.id.clone();

        let evicted = self.hot.insert(entry)?;

        if let Some(evicted_entry) = evicted {
            debug!(
                evicted_id = %evicted_entry.id,
                "Hot 层 LRU 驱逐,迁移到 Warm 层"
            );
            self.warm.insert(evicted_entry.clone()).await?;

            let event = NexusEvent::CapabilityTiered {
                metadata: EventMetadata::new("cmt-tiering"),
                capability_id: evicted_entry.id.to_string(),
                from_tier: Tier::Hot.as_str().to_string(),
                to_tier: Tier::Warm.as_str().to_string(),
                reason: MigrationReason::LruEviction.as_str().to_string(),
            };
            self.event_bus.publish(event).await?;
        }

        debug!(cap_id = %cap_id, "能力条目已插入 Hot 层");
        Ok(())
    }

    /// 跨层查找能力条目(自动提升到 Hot 层)
    ///
    /// 查找顺序:Hot → Warm → Cold → Ice
    /// 找到后提升到 Hot 层(若 Hot 未满,否则触发 LRU 驱逐后提升)
    ///
    /// 返回条目克隆;若所有层都未找到返回 None。
    pub async fn get(&self, cap_id: &str) -> Result<Option<CapabilityEntry>, CmtError> {
        // 1. Hot 层查找(若命中,直接返回)
        if let Ok(entry) = self.hot.get(cap_id) {
            debug!(cap_id = cap_id, tier = "Hot", "跨层查找命中");
            return Ok(Some(entry));
        }

        // 2. Warm 层查找(若命中,提升到 Hot 层)
        if let Some(entry) = self.warm.get(cap_id.to_string()).await? {
            debug!(cap_id = cap_id, tier = "Warm", "跨层查找命中,提升到 Hot");
            let entry_to_promote = entry.clone();
            self.promote_to_hot_internal(entry_to_promote, Tier::Warm)
                .await?;
            return Ok(Some(entry));
        }

        // 3. Cold 层查找(若命中,提升到 Hot 层)
        if let Some(entry) = self.cold.get(cap_id.to_string()).await? {
            debug!(cap_id = cap_id, tier = "Cold", "跨层查找命中,提升到 Hot");
            let entry_to_promote = entry.clone();
            self.promote_to_hot_internal(entry_to_promote, Tier::Cold)
                .await?;
            return Ok(Some(entry));
        }

        // 4. Ice 层查找(若命中,提升到 Hot 层)
        if let Some(entry) = self.ice.get(cap_id.to_string()).await? {
            debug!(cap_id = cap_id, tier = "Ice", "跨层查找命中,提升到 Hot");
            let entry_to_promote = entry.clone();
            self.promote_to_hot_internal(entry_to_promote, Tier::Ice)
                .await?;
            return Ok(Some(entry));
        }

        debug!(cap_id = cap_id, "跨层查找未命中");
        Ok(None)
    }

    /// 内部提升方法:将条目从源层提升到 Hot 层
    ///
    /// WHY 内部方法:与 TierMigrator::promote_to_hot 逻辑相同,但内联了 LRU 驱逐
    /// 处理以减少方法调用开销(高频路径优化)。修改时需同步更新 TierMigrator::promote_to_hot。
    /// 此方法发布提升事件与可能的 LRU 驱逐事件
    ///
    /// # 并发安全(SubTask 18.2)
    /// delete 源层条目时幂等化:若条目已被其他线程删除(并发 get 提升或 delete),
    /// delete 返回 `Ok(false)` 或 `Err(EntryNotFound)`,均视为"已被删除",
    /// 记录 debug 日志后继续完成提升,而非中断返回错误。
    async fn promote_to_hot_internal(
        &self,
        mut entry: CapabilityEntry,
        from: Tier,
    ) -> Result<(), CmtError> {
        let cap_id = entry.id.clone();

        // 从源层删除(幂等化:条目可能已被并发 get 提升或 delete 删除)
        match from {
            Tier::Hot => {
                // 已在 Hot 层,无需提升
                return Ok(());
            }
            Tier::Warm => {
                // SubTask 18.2:幂等化 delete — Ok(false) 或 EntryNotFound 均视为已删除
                match self.warm.delete(cap_id.clone()).await {
                    Ok(true) => {}
                    Ok(false) | Err(CmtError::EntryNotFound(_)) => {
                        debug!(
                            cap_id = %cap_id,
                            tier = "Warm",
                            "Warm 层条目已被其他线程删除,继续提升"
                        );
                    }
                    Err(e) => return Err(e),
                }
            }
            Tier::Cold => match self.cold.delete(cap_id.clone()).await {
                Ok(true) => {}
                Ok(false) | Err(CmtError::EntryNotFound(_)) => {
                    debug!(
                        cap_id = %cap_id,
                        tier = "Cold",
                        "Cold 层条目已被其他线程删除,继续提升"
                    );
                }
                Err(e) => return Err(e),
            },
            Tier::Ice => match self.ice.delete(cap_id.clone()).await {
                Ok(true) => {}
                Ok(false) | Err(CmtError::EntryNotFound(_)) => {
                    debug!(
                        cap_id = %cap_id,
                        tier = "Ice",
                        "Ice 层条目已被其他线程删除,继续提升"
                    );
                }
                Err(e) => return Err(e),
            },
        }

        // 更新 tier 为 Hot
        entry.tier = Tier::Hot;

        if let Some(evicted) = self.hot.insert(entry)? {
            debug!(
                evicted_id = %evicted.id,
                "提升时 Hot 层满,驱逐条目到 Warm 层"
            );
            self.warm.insert(evicted.clone()).await?;

            let event = NexusEvent::CapabilityTiered {
                metadata: EventMetadata::new("cmt-tiering"),
                capability_id: evicted.id.to_string(),
                from_tier: Tier::Hot.as_str().to_string(),
                to_tier: Tier::Warm.as_str().to_string(),
                reason: MigrationReason::LruEviction.as_str().to_string(),
            };
            self.event_bus.publish(event).await?;
        }

        let event = NexusEvent::CapabilityTiered {
            metadata: EventMetadata::new("cmt-tiering"),
            capability_id: cap_id.to_string(),
            from_tier: from.as_str().to_string(),
            to_tier: Tier::Hot.as_str().to_string(),
            reason: MigrationReason::AccessPromotion.as_str().to_string(),
        };
        self.event_bus.publish(event).await?;

        Ok(())
    }

    /// 跨层删除能力条目(删除所有副本)
    ///
    /// 遍历 Hot/Warm/Cold/Ice 四层,删除指定 ID 的所有副本。
    /// 返回是否删除了至少一个副本。
    pub async fn delete(&self, cap_id: &str) -> Result<bool, CmtError> {
        let mut deleted = false;

        if self.hot.remove(cap_id).is_some() {
            deleted = true;
            debug!(cap_id = cap_id, tier = "Hot", "跨层删除命中");
        }

        if self.warm.delete(cap_id.to_string()).await? {
            deleted = true;
            debug!(cap_id = cap_id, tier = "Warm", "跨层删除命中");
        }

        if self.cold.delete(cap_id.to_string()).await? {
            deleted = true;
            debug!(cap_id = cap_id, tier = "Cold", "跨层删除命中");
        }

        if self.ice.delete(cap_id.to_string()).await? {
            deleted = true;
            debug!(cap_id = cap_id, tier = "Ice", "跨层删除命中");
        }

        if deleted {
            debug!(cap_id = cap_id, "跨层删除完成");
        }
        Ok(deleted)
    }

    /// 列出指定层级的所有条目
    ///
    /// 返回指定层级的所有条目克隆(用于快照或迁移)。
    pub async fn list(&self, tier: Tier) -> Result<Vec<CapabilityEntry>, CmtError> {
        match tier {
            Tier::Hot => Ok(self.hot.list_all()),
            Tier::Warm => self.warm.list_all().await,
            Tier::Cold => self.cold.list_all().await,
            Tier::Ice => self.ice.list_all().await,
        }
    }

    /// 运行衰减周期,将 priority < 0.1 的条目降级
    ///
    /// 扫描 Hot/Warm/Cold 三层(Ice 层不衰减,已是最低层),
    /// 将 `priority < 0.1` 的条目降级到下层。
    ///
    /// 降级链路:
    /// - Hot → Warm:Hot 层条目 priority < 0.1,迁移到 Warm
    /// - Warm → Cold:Warm 层条目 priority < 0.1,迁移到 Cold
    /// - Cold → Ice:Cold 层条目 priority < 0.1,迁移到 Ice
    ///
    /// # 流式处理 + 仅查 metadata(SubTask 19.2)
    /// WHY:原实现 `list_all()` 加载所有条目的完整数据(含 content),
    /// 65536 条目全量加载导致内存峰值过高。现改为:
    /// 1. Warm/Cold 层使用 `list_idle_metadata()` 只查 ID + 时间戳 + 计数(不含 content)
    /// 2. 衰减判断用 `should_demote_metadata`(仅需 access_count + last_accessed_at)
    /// 3. 降级时通过 `peek` 按需读取完整条目(含 content)
    /// 4. 分批处理(每批 1024),批间释放 content 内存
    ///
    /// 内存峰值降低 80%+(原:65536 × content_size,现:1024 × content_size)
    ///
    /// # 避免级联降级(WHY)
    /// 先采集所有层的候选,然后只对候选中的条目进行降级判断。
    /// Hot 层降级的条目进入 Warm 层后,不会被 Warm 层的扫描捕获,
    /// 避免同一轮中条目被多次降级(Hot→Warm→Cold→Ice)。
    ///
    /// # 事件语义保持
    /// 每个条目的迁移仍发布独立的 `CapabilityTiered` 事件,
    /// 事件消费者不感知批量处理内部实现。
    pub async fn run_decay_cycle(&self) -> Result<u64, CmtError> {
        let now = Utc::now();

        // 1. Hot 层快照(内存操作,廉价,保留完整条目用于迁移)
        let hot_snapshot = self.hot.list_all();

        // 2. Warm/Cold 层仅查 metadata(不含 content,降低内存峰值 80%+)
        let warm_metadata = self.warm.list_idle_metadata().await?;
        let cold_metadata = self.cold.list_idle_metadata().await?;

        // 3. 基于 metadata 筛选降级候选(无需加载 content)
        // Hot → Warm:从完整快照筛选(Hot 是内存层,快照廉价)
        let hot_to_warm: Vec<CapabilityEntry> = hot_snapshot
            .into_iter()
            .filter(|e| self.decay.should_demote(e, now))
            .map(|mut e| {
                e.tier = Tier::Warm;
                e
            })
            .collect();
        let hot_demote_ids: HashSet<CapabilityId> =
            hot_to_warm.iter().map(|e| e.id.clone()).collect();

        // Warm → Cold:从 metadata 筛选,排除刚从 Hot 降级的条目(避免级联降级)
        let warm_to_cold_ids: Vec<CapabilityId> = warm_metadata
            .into_iter()
            .filter(|(id, last_at, count)| {
                !hot_demote_ids.contains(id)
                    && self.decay.should_demote_metadata(*last_at, *count, now)
            })
            .map(|(id, _, _)| id)
            .collect();
        let warm_demote_ids: HashSet<CapabilityId> = warm_to_cold_ids.iter().cloned().collect();

        // Cold → Ice:从 metadata 筛选,排除刚从 Warm 降级的条目(避免级联降级)
        let cold_to_ice_ids: Vec<CapabilityId> = cold_metadata
            .into_iter()
            .filter(|(id, last_at, count)| {
                !warm_demote_ids.contains(id)
                    && self.decay.should_demote_metadata(*last_at, *count, now)
            })
            .map(|(id, _, _)| id)
            .collect();

        // 4. 分批执行降级迁移(每批 1024,批间释放 content 内存)
        let mut demoted_count: u64 = 0;
        demoted_count += self.demote_hot_to_warm(hot_to_warm).await?;
        demoted_count += self.demote_warm_to_cold(warm_to_cold_ids).await?;
        demoted_count += self.demote_cold_to_ice(cold_to_ice_ids).await?;

        if demoted_count > 0 {
            info!(demoted_count, "衰减周期完成,共降级条目");
        }
        Ok(demoted_count)
    }

    /// Hot → Warm 分批降级迁移
    ///
    /// WHY 分批:虽然 Hot 层条目已在内存中,分批处理仍可控制单批 insert_batch
    /// 的事务大小,避免超大事务阻塞其他读写操作。
    async fn demote_hot_to_warm(&self, hot_to_warm: Vec<CapabilityEntry>) -> Result<u64, CmtError> {
        if hot_to_warm.is_empty() {
            return Ok(0);
        }

        let mut demoted_count: u64 = 0;
        for chunk in hot_to_warm.chunks(DECAY_BATCH_SIZE) {
            // SubTask 18.3:双重检查 — 过滤掉已不在 Hot 层的条目
            // WHY:快照采集后、迁移执行前,条目可能被并发 get 提升或 delete
            let to_migrate: Vec<CapabilityEntry> = chunk
                .iter()
                .filter(|e| self.hot.contains(&e.id))
                .cloned()
                .collect();

            if to_migrate.is_empty() {
                continue;
            }

            let cap_ids: Vec<CapabilityId> = to_migrate.iter().map(|e| e.id.clone()).collect();
            let migrate_count = cap_ids.len() as u64;

            for id in &cap_ids {
                self.hot.remove(id);
            }
            self.warm.insert_batch(to_migrate).await?;

            for cap_id in cap_ids {
                debug!(cap_id = %cap_id, tier = "Hot", "衰减降级触发");
                let event = NexusEvent::CapabilityTiered {
                    metadata: EventMetadata::new("cmt-tiering"),
                    capability_id: cap_id.to_string(),
                    from_tier: Tier::Hot.as_str().to_string(),
                    to_tier: Tier::Warm.as_str().to_string(),
                    reason: MigrationReason::DecayExpired.as_str().to_string(),
                };
                self.event_bus.publish(event).await?;
            }

            demoted_count += migrate_count;
        }
        Ok(demoted_count)
    }

    /// Warm → Cold 分批降级迁移
    ///
    /// WHY 分批 + 按需 peek:候选 ID 列表来自 metadata(不含 content),
    /// 降级时通过 `peek` 按需读取完整条目。每批 1024 条,批间释放 content 内存。
    async fn demote_warm_to_cold(&self, candidate_ids: Vec<CapabilityId>) -> Result<u64, CmtError> {
        if candidate_ids.is_empty() {
            return Ok(0);
        }

        let mut demoted_count: u64 = 0;
        for chunk in candidate_ids.chunks(DECAY_BATCH_SIZE) {
            // 按需 peek 完整条目(含 content),同时双重检查条目仍在 Warm 层
            let mut to_migrate = Vec::with_capacity(chunk.len());
            for id in chunk {
                if let Some(mut entry) = self.warm.peek(id.to_string()).await? {
                    entry.tier = Tier::Cold;
                    to_migrate.push(entry);
                }
            }

            if to_migrate.is_empty() {
                continue;
            }

            let cap_ids: Vec<CapabilityId> = to_migrate.iter().map(|e| e.id.clone()).collect();
            let migrate_count = cap_ids.len() as u64;

            self.warm.delete_batch(cap_ids.clone()).await?;
            self.cold.insert_batch(to_migrate).await?;

            for cap_id in cap_ids {
                debug!(cap_id = %cap_id, tier = "Warm", "衰减降级触发");
                let event = NexusEvent::CapabilityTiered {
                    metadata: EventMetadata::new("cmt-tiering"),
                    capability_id: cap_id.to_string(),
                    from_tier: Tier::Warm.as_str().to_string(),
                    to_tier: Tier::Cold.as_str().to_string(),
                    reason: MigrationReason::DecayExpired.as_str().to_string(),
                };
                self.event_bus.publish(event).await?;
            }

            demoted_count += migrate_count;
        }
        Ok(demoted_count)
    }

    /// Cold → Ice 分批降级迁移
    ///
    /// WHY 分批 + 按需 peek:候选 ID 列表来自 metadata(不含 content),
    /// 降级时通过 `peek` 按需读取完整条目。每批 1024 条,批间释放 content 内存。
    async fn demote_cold_to_ice(&self, candidate_ids: Vec<CapabilityId>) -> Result<u64, CmtError> {
        if candidate_ids.is_empty() {
            return Ok(0);
        }

        let mut demoted_count: u64 = 0;
        for chunk in candidate_ids.chunks(DECAY_BATCH_SIZE) {
            // 按需 peek 完整条目(含 content),同时双重检查条目仍在 Cold 层
            let mut to_migrate = Vec::with_capacity(chunk.len());
            for id in chunk {
                if let Some(mut entry) = self.cold.peek(id.to_string()).await? {
                    entry.tier = Tier::Ice;
                    to_migrate.push(entry);
                }
            }

            if to_migrate.is_empty() {
                continue;
            }

            let cap_ids: Vec<CapabilityId> = to_migrate.iter().map(|e| e.id.clone()).collect();
            let migrate_count = cap_ids.len() as u64;

            self.cold.delete_batch(cap_ids.clone()).await?;
            for entry in to_migrate {
                debug!(cap_id = %entry.id, tier = "Cold", "衰减降级触发");
                self.ice.archive(entry).await?;
            }

            for cap_id in cap_ids {
                let event = NexusEvent::CapabilityTiered {
                    metadata: EventMetadata::new("cmt-tiering"),
                    capability_id: cap_id.to_string(),
                    from_tier: Tier::Cold.as_str().to_string(),
                    to_tier: Tier::Ice.as_str().to_string(),
                    reason: MigrationReason::DecayExpired.as_str().to_string(),
                };
                self.event_bus.publish(event).await?;
            }

            demoted_count += migrate_count;
        }
        Ok(demoted_count)
    }

    /// 获取衰减计算器引用
    pub fn decay(&self) -> &DecayCalculator {
        &self.decay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str) -> CapabilityEntry {
        CapabilityEntry::new(id, format!("content-{id}"), Tier::Hot)
    }

    #[tokio::test]
    async fn test_insert_and_get_hot() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        coord.insert(make_entry("cap-1")).await.unwrap();

        let fetched = coord.get("cap-1").await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id.as_str(), "cap-1");
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        let fetched = coord.get("nonexistent").await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_get_promote_warm_to_hot() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        coord.warm.insert(make_entry("cap-1")).await.unwrap();
        assert!(!coord.hot.contains("cap-1"));

        let fetched = coord.get("cap-1").await.unwrap();
        assert!(fetched.is_some());
        assert!(coord.hot.contains("cap-1"));
        assert!(coord
            .warm
            .peek("cap-1".to_string())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_get_promote_cold_to_hot() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        coord.cold.insert(make_entry("cap-1")).await.unwrap();

        let fetched = coord.get("cap-1").await.unwrap();
        assert!(fetched.is_some());
        assert!(coord.hot.contains("cap-1"));
    }

    #[tokio::test]
    async fn test_get_promote_ice_to_hot() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        coord.ice.archive(make_entry("cap-1")).await.unwrap();

        let fetched = coord.get("cap-1").await.unwrap();
        assert!(fetched.is_some());
        assert!(coord.hot.contains("cap-1"));
    }

    #[tokio::test]
    async fn test_delete_cross_layer() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        coord.hot.insert(make_entry("cap-1")).unwrap();
        coord.warm.insert(make_entry("cap-1")).await.unwrap();
        coord.cold.insert(make_entry("cap-1")).await.unwrap();
        coord.ice.archive(make_entry("cap-1")).await.unwrap();

        let deleted = coord.delete("cap-1").await.unwrap();
        assert!(deleted);

        assert!(!coord.hot.contains("cap-1"));
        assert!(coord
            .warm
            .peek("cap-1".to_string())
            .await
            .unwrap()
            .is_none());
        assert!(coord
            .cold
            .peek("cap-1".to_string())
            .await
            .unwrap()
            .is_none());
        assert!(coord.ice.get("cap-1".to_string()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_returns_false() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        let deleted = coord.delete("nonexistent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_list_by_tier() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        coord.hot.insert(make_entry("hot-1")).unwrap();
        coord.warm.insert(make_entry("warm-1")).await.unwrap();
        coord.cold.insert(make_entry("cold-1")).await.unwrap();
        coord.ice.archive(make_entry("ice-1")).await.unwrap();

        let hot_list = coord.list(Tier::Hot).await.unwrap();
        assert_eq!(hot_list.len(), 1);

        let warm_list = coord.list(Tier::Warm).await.unwrap();
        assert_eq!(warm_list.len(), 1);

        let cold_list = coord.list(Tier::Cold).await.unwrap();
        assert_eq!(cold_list.len(), 1);

        let ice_list = coord.list(Tier::Ice).await.unwrap();
        assert_eq!(ice_list.len(), 1);
    }

    #[tokio::test]
    async fn test_insert_with_lru_eviction() {
        let bus = EventBus::new();
        let config = CmtConfig::default().with_hot_capacity(2);
        let coord = CmtCoordinator::new_in_memory(config, bus).unwrap();

        coord.insert(make_entry("cap-1")).await.unwrap();
        coord.insert(make_entry("cap-2")).await.unwrap();

        coord.insert(make_entry("cap-3")).await.unwrap();

        assert_eq!(coord.hot.len(), 2);
        assert!(
            coord
                .warm
                .peek("cap-1".to_string())
                .await
                .unwrap()
                .is_some()
                || coord
                    .warm
                    .peek("cap-2".to_string())
                    .await
                    .unwrap()
                    .is_some()
        );
    }

    #[tokio::test]
    async fn test_capability_tiered_event_on_lru_eviction() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let config = CmtConfig::default().with_hot_capacity(1);
        let coord = CmtCoordinator::new_in_memory(config, bus).unwrap();

        coord.insert(make_entry("cap-1")).await.unwrap();
        coord.insert(make_entry("cap-2")).await.unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::CapabilityTiered {
                from_tier, to_tier, ..
            } => {
                assert_eq!(from_tier, "Hot");
                assert_eq!(to_tier, "Warm");
            }
            other => panic!("expected CapabilityTiered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_capability_tiered_event_on_promote() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        coord.warm.insert(make_entry("cap-1")).await.unwrap();

        coord.get("cap-1").await.unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::CapabilityTiered {
                from_tier, to_tier, ..
            } => {
                assert_eq!(from_tier, "Warm");
                assert_eq!(to_tier, "Hot");
            }
            other => panic!("expected CapabilityTiered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_run_decay_cycle_no_demotion() {
        let bus = EventBus::new();
        let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus).unwrap();

        coord.insert(make_entry("cap-1")).await.unwrap();

        let demoted = coord.run_decay_cycle().await.unwrap();
        assert_eq!(demoted, 0);
    }
}
