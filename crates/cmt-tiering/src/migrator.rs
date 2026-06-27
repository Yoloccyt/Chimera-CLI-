//! 层级迁移器 — 四级存储间的自动迁移逻辑
//!
//! 对应架构层:L3 Storage
//!
//! # 设计决策(WHY)
//! - **迁移原子性**:先从源层读取/移除,再插入目标层,失败时回滚
//! - **事件发布**:每次迁移发布 `CapabilityTiered` 事件,携带源层、目标层、原因
//! - **回滚机制**:目标层插入失败时,重新插入源层(避免数据丢失)
//! - **DashMap 写锁释放后再调用 async 方法**:避免死锁(Week 2 经验教训)
//!
//! # 与 CmtCoordinator 的关系(WHY 保留两者)
//! - `TierMigrator` 提供**低级迁移原语**:`migrate_hot_to_warm`、`migrate_warm_to_cold`、
//!   `migrate_cold_to_ice`、`promote_to_hot`,适用于手动触发迁移或外部集成
//! - `CmtCoordinator` 提供**高级自动管理**:`get` 自动跨层提升、`run_decay_cycle`
//!   基于衰减的自动降级,内部 `promote_to_hot_internal` 与 `TierMigrator::promote_to_hot`
//!   逻辑相同但内联了 LRU 驱逐处理以减少方法调用开销
//! - 两者保留的原因:`TierMigrator` 的手动迁移方法(`migrate_warm_to_cold` 等)
//!   在 `CmtCoordinator` 中没有对应 API(`run_decay_cycle` 是自动衰减,不能替代手动迁移)
//!
//! # 迁移链路
//! ```text
//! Hot → Warm:Hot 层 LRU 驱逐时,迁移到 Warm
//! Warm → Cold:Warm 层条目 24 小时未被访问,迁移到 Cold
//! Cold → Ice:Cold 层条目 7 天未被访问,迁移到 Ice
//! Ice → Cold:Ice 层条目被访问时,提升到 Cold(并更新访问时戳)
//! Cold/Warm → Hot:被访问时提升到 Hot(若 Hot 未满)
//! ```

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{debug, info, warn};

use crate::cold::ColdTier;
use crate::error::CmtError;
use crate::hot::HotTier;
use crate::ice::IceTier;
use crate::types::{MigrationReason, Tier};
use crate::warm::WarmTier;

/// 层级迁移器 — 四级存储间的自动迁移
///
/// 封装 Hot/Warm/Cold/Ice 四级间的迁移逻辑,每次迁移发布 `CapabilityTiered` 事件。
///
/// # 线程安全
/// 所有层级都是线程安全的(HotTier DashMap,Warm/Cold Mutex,Ice 文件系统)。
/// `EventBus` 基于 `tokio::broadcast`,Clone 廉价(Arc 引用计数)。
/// 所有 async fn 满足 `Send + 'static` 约束。
pub struct TierMigrator {
    /// Hot 层引用
    hot: HotTier,
    /// Warm 层引用
    warm: WarmTier,
    /// Cold 层引用
    cold: ColdTier,
    /// Ice 层引用
    ice: IceTier,
    /// 事件总线(用于发布 CapabilityTiered 事件)
    event_bus: EventBus,
}

impl TierMigrator {
    /// 创建迁移器,传入四级存储引用与事件总线
    pub fn new(
        hot: HotTier,
        warm: WarmTier,
        cold: ColdTier,
        ice: IceTier,
        event_bus: EventBus,
    ) -> Self {
        Self {
            hot,
            warm,
            cold,
            ice,
            event_bus,
        }
    }

    /// 获取 Hot 层引用(用于测试验证与外部查询)
    pub fn hot(&self) -> &HotTier {
        &self.hot
    }

    /// 获取 Warm 层引用(用于测试验证与外部查询)
    pub fn warm(&self) -> &WarmTier {
        &self.warm
    }

    /// 获取 Cold 层引用(用于测试验证与外部查询)
    pub fn cold(&self) -> &ColdTier {
        &self.cold
    }

    /// 获取 Ice 层引用(用于测试验证与外部查询)
    pub fn ice(&self) -> &IceTier {
        &self.ice
    }

    /// Hot → Warm 迁移(Hot 层 LRU 驱逐时调用)
    ///
    /// 将条目从 Hot 层移除,插入 Warm 层。
    /// 迁移原因:`MigrationReason::LruEviction`
    pub async fn migrate_hot_to_warm(
        &self,
        entry: crate::types::CapabilityEntry,
    ) -> Result<(), CmtError> {
        let cap_id = entry.id.clone();

        // WarmTier::insert 已改为 async(spawn_blocking 包装 SQLite 操作)
        self.warm
            .insert(entry)
            .await
            .map_err(|e| CmtError::MigrationFailed {
                from: Tier::Hot.as_str().into(),
                to: Tier::Warm.as_str().into(),
                reason: e.to_string(),
            })?;

        self.publish_tiered_event(&cap_id, Tier::Hot, Tier::Warm, MigrationReason::LruEviction)
            .await?;

        info!(cap_id = %cap_id, "Hot → Warm 迁移完成");
        Ok(())
    }

    /// Warm → Cold 迁移(Warm 层条目 24 小时未被访问时调用)
    ///
    /// 将条目从 Warm 层移除,插入 Cold 层。
    /// 迁移原因:`MigrationReason::IdleTimeout`
    pub async fn migrate_warm_to_cold(&self, cap_id: &str) -> Result<(), CmtError> {
        // peek 不更新访问时间,避免迁移时刷新空闲计时
        let entry = self
            .warm
            .peek(cap_id.to_string())
            .await
            .map_err(|e| CmtError::MigrationFailed {
                from: Tier::Warm.as_str().into(),
                to: Tier::Cold.as_str().into(),
                reason: e.to_string(),
            })?
            .ok_or_else(|| CmtError::EntryNotFound(format!("Warm 层条目: {cap_id}")))?;

        // WHY:先写入目标层(Cold),确认成功后再删除源层(Warm)。
        // 此前实现是"先删除 Warm → 再插入 Cold",Cold 插入失败时用假数据回滚 Warm 层,
        // 导致原始 entry 的 content/created_at/access_count 丢失。改为"先插入 Cold →
        // 再删除 Warm"后,Cold 失败时 Warm 层未受影响,无需回滚,原始数据完整保留
        // (与 SubTask 12.6 MLC migrate 修复策略一致)。
        if let Err(e) = self.cold.insert(entry).await {
            warn!(cap_id = cap_id, error = %e, "Cold 层插入失败,Warm 层条目保留");
            return Err(CmtError::MigrationFailed {
                from: Tier::Warm.as_str().into(),
                to: Tier::Cold.as_str().into(),
                reason: e.to_string(),
            });
        }

        // Cold 层写入成功,删除 Warm 层条目
        self.warm
            .delete(cap_id.to_string())
            .await
            .map_err(|e| CmtError::MigrationFailed {
                from: Tier::Warm.as_str().into(),
                to: Tier::Cold.as_str().into(),
                reason: e.to_string(),
            })?;

        self.publish_tiered_event(cap_id, Tier::Warm, Tier::Cold, MigrationReason::IdleTimeout)
            .await?;

        info!(cap_id = cap_id, "Warm → Cold 迁移完成");
        Ok(())
    }

    /// Cold → Ice 迁移(Cold 层条目 7 天未被访问时调用)
    ///
    /// 将条目从 Cold 层移除,归档到 Ice 层。
    /// 迁移原因:`MigrationReason::IdleTimeout`
    pub async fn migrate_cold_to_ice(&self, cap_id: &str) -> Result<(), CmtError> {
        // peek 不更新访问时间,避免迁移时刷新空闲计时
        let entry = self
            .cold
            .peek(cap_id.to_string())
            .await
            .map_err(|e| CmtError::MigrationFailed {
                from: Tier::Cold.as_str().into(),
                to: Tier::Ice.as_str().into(),
                reason: e.to_string(),
            })?
            .ok_or_else(|| CmtError::EntryNotFound(format!("Cold 层条目: {cap_id}")))?;

        // WHY:先写入目标层(Ice),确认成功后再删除源层(Cold)。
        // 此前实现是"先删除 Cold → 再归档 Ice",Ice 归档失败时用假数据回滚 Cold 层,
        // 导致原始 entry 的 content/created_at/access_count 丢失。改为"先归档 Ice →
        // 再删除 Cold"后,Ice 失败时 Cold 层未受影响,无需回滚,原始数据完整保留
        // (与 SubTask 12.6 MLC migrate 修复策略一致)。
        if let Err(e) = self.ice.archive(entry).await {
            warn!(cap_id = cap_id, error = %e, "Ice 层归档失败,Cold 层条目保留");
            return Err(CmtError::MigrationFailed {
                from: Tier::Cold.as_str().into(),
                to: Tier::Ice.as_str().into(),
                reason: e.to_string(),
            });
        }

        // Ice 层归档成功,删除 Cold 层条目
        self.cold
            .delete(cap_id.to_string())
            .await
            .map_err(|e| CmtError::MigrationFailed {
                from: Tier::Cold.as_str().into(),
                to: Tier::Ice.as_str().into(),
                reason: e.to_string(),
            })?;

        self.publish_tiered_event(cap_id, Tier::Cold, Tier::Ice, MigrationReason::IdleTimeout)
            .await?;

        info!(cap_id = cap_id, "Cold → Ice 迁移完成");
        Ok(())
    }

    /// 提升条目到 Hot 层(被访问时调用)
    ///
    /// 从源层读取条目,删除源层副本,插入 Hot 层。
    /// 若 Hot 层已满,先驱逐 LRU 条目到 Warm 层,再插入。
    /// 迁移原因:`MigrationReason::AccessPromotion`
    ///
    /// # 与 CmtCoordinator::promote_to_hot_internal 的关系(WHY)
    /// 两者逻辑相同,但保留独立实现:
    /// - `TierMigrator` 作为低级迁移原语,可被外部直接调用
    /// - `CmtCoordinator::promote_to_hot_internal` 内联了 LRU 驱逐处理,
    ///   避免额外的方法调用开销(高频路径优化)
    /// - 修改任一实现时,需同步更新另一处以保持行为一致
    ///
    /// # 参数
    /// - `entry`:要提升的条目(从源层读取的完整数据)
    /// - `from`:源层级
    pub async fn promote_to_hot(
        &self,
        mut entry: crate::types::CapabilityEntry,
        from: Tier,
    ) -> Result<(), CmtError> {
        let cap_id = entry.id.clone();

        match from {
            Tier::Hot => {
                // 已在 Hot 层,无需提升
                return Ok(());
            }
            Tier::Warm => {
                self.warm
                    .delete(cap_id.clone())
                    .await
                    .map_err(|e| CmtError::MigrationFailed {
                        from: from.as_str().into(),
                        to: Tier::Hot.as_str().into(),
                        reason: e.to_string(),
                    })?;
            }
            Tier::Cold => {
                self.cold
                    .delete(cap_id.clone())
                    .await
                    .map_err(|e| CmtError::MigrationFailed {
                        from: from.as_str().into(),
                        to: Tier::Hot.as_str().into(),
                        reason: e.to_string(),
                    })?;
            }
            Tier::Ice => {
                self.ice
                    .delete(cap_id.clone())
                    .await
                    .map_err(|e| CmtError::MigrationFailed {
                        from: from.as_str().into(),
                        to: Tier::Hot.as_str().into(),
                        reason: e.to_string(),
                    })?;
            }
        }

        entry.tier = Tier::Hot;

        if let Some(evicted) = self
            .hot
            .insert(entry)
            .map_err(|e| CmtError::MigrationFailed {
                from: from.as_str().into(),
                to: Tier::Hot.as_str().into(),
                reason: e.to_string(),
            })?
        {
            debug!(
                evicted_id = %evicted.id,
                "Hot 层满,驱逐条目到 Warm 层"
            );
            self.migrate_hot_to_warm(evicted).await?;
        }

        self.publish_tiered_event(&cap_id, from, Tier::Hot, MigrationReason::AccessPromotion)
            .await?;

        info!(cap_id = %cap_id, from = from.as_str(), "提升到 Hot 层完成");
        Ok(())
    }

    /// 发布 CapabilityTiered 事件
    ///
    /// 携带 capability_id、from_tier、to_tier、reason 字段。
    async fn publish_tiered_event(
        &self,
        cap_id: &str,
        from: Tier,
        to: Tier,
        reason: MigrationReason,
    ) -> Result<(), CmtError> {
        let event = NexusEvent::CapabilityTiered {
            metadata: EventMetadata::new("cmt-tiering"),
            capability_id: cap_id.to_string(),
            from_tier: from.as_str().to_string(),
            to_tier: to.as_str().to_string(),
            reason: reason.as_str().to_string(),
        };
        self.event_bus.publish(event).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CapabilityEntry;

    fn make_entry(id: &str, tier: Tier) -> CapabilityEntry {
        CapabilityEntry::new(id, format!("content-{id}"), tier)
    }

    #[tokio::test]
    async fn test_migrate_hot_to_warm() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let hot = HotTier::new(256);
        let warm = WarmTier::open_in_memory(4096).unwrap();
        let cold = ColdTier::open_in_memory(65536).unwrap();
        let ice = IceTier::new(tempfile::tempdir().unwrap().path());

        let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

        let entry = make_entry("cap-1", Tier::Hot);
        migrator.migrate_hot_to_warm(entry).await.unwrap();

        // 验证 Warm 层有条目
        let fetched = migrator.warm().peek("cap-1".to_string()).await.unwrap();
        assert!(fetched.is_some());

        // 验证事件发布
        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::CapabilityTiered {
                capability_id,
                from_tier,
                to_tier,
                reason,
                ..
            } => {
                assert_eq!(capability_id, "cap-1");
                assert_eq!(from_tier, "Hot");
                assert_eq!(to_tier, "Warm");
                assert_eq!(reason, "lru_eviction");
            }
            other => panic!("expected CapabilityTiered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_migrate_warm_to_cold() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let hot = HotTier::new(256);
        let warm = WarmTier::open_in_memory(4096).unwrap();
        let cold = ColdTier::open_in_memory(65536).unwrap();
        let ice = IceTier::new(tempfile::tempdir().unwrap().path());

        let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

        // 先在 Warm 层插入条目
        let entry = make_entry("cap-1", Tier::Warm);
        migrator.warm().insert(entry).await.unwrap();

        // 迁移到 Cold 层
        migrator.migrate_warm_to_cold("cap-1").await.unwrap();

        // 验证 Warm 层无条目,Cold 层有条目
        assert!(migrator
            .warm()
            .peek("cap-1".to_string())
            .await
            .unwrap()
            .is_none());
        let fetched = migrator.cold().peek("cap-1".to_string()).await.unwrap();
        assert!(fetched.is_some());

        // 验证事件发布
        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::CapabilityTiered {
                from_tier, to_tier, ..
            } => {
                assert_eq!(from_tier, "Warm");
                assert_eq!(to_tier, "Cold");
            }
            other => panic!("expected CapabilityTiered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_migrate_cold_to_ice() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let hot = HotTier::new(256);
        let warm = WarmTier::open_in_memory(4096).unwrap();
        let cold = ColdTier::open_in_memory(65536).unwrap();
        let ice = IceTier::new(tempfile::tempdir().unwrap().path());

        let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

        // 先在 Cold 层插入条目
        let entry = make_entry("cap-1", Tier::Cold);
        migrator.cold().insert(entry).await.unwrap();

        // 迁移到 Ice 层
        migrator.migrate_cold_to_ice("cap-1").await.unwrap();

        // 验证 Cold 层无条目,Ice 层有条目
        assert!(migrator
            .cold()
            .peek("cap-1".to_string())
            .await
            .unwrap()
            .is_none());
        let fetched = migrator.ice().get("cap-1".to_string()).await.unwrap();
        assert!(fetched.is_some());

        // 验证事件发布
        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::CapabilityTiered {
                from_tier, to_tier, ..
            } => {
                assert_eq!(from_tier, "Cold");
                assert_eq!(to_tier, "Ice");
            }
            other => panic!("expected CapabilityTiered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_promote_to_hot_from_warm() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let hot = HotTier::new(256);
        let warm = WarmTier::open_in_memory(4096).unwrap();
        let cold = ColdTier::open_in_memory(65536).unwrap();
        let ice = IceTier::new(tempfile::tempdir().unwrap().path());

        let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

        // 先在 Warm 层插入条目
        let entry = make_entry("cap-1", Tier::Warm);
        migrator.warm().insert(entry).await.unwrap();

        // 读取条目并提升到 Hot 层
        let entry = migrator
            .warm()
            .peek("cap-1".to_string())
            .await
            .unwrap()
            .unwrap();
        migrator.promote_to_hot(entry, Tier::Warm).await.unwrap();

        // 验证 Warm 层无条目,Hot 层有条目
        assert!(migrator
            .warm()
            .peek("cap-1".to_string())
            .await
            .unwrap()
            .is_none());
        assert!(migrator.hot().contains("cap-1"));

        // 验证事件发布
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
    async fn test_promote_to_hot_from_ice() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let hot = HotTier::new(256);
        let warm = WarmTier::open_in_memory(4096).unwrap();
        let cold = ColdTier::open_in_memory(65536).unwrap();
        let ice = IceTier::new(tempfile::tempdir().unwrap().path());

        let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

        // 先在 Ice 层归档条目
        let entry = make_entry("cap-1", Tier::Ice);
        migrator.ice().archive(entry).await.unwrap();

        // 读取条目并提升到 Hot 层
        let entry = migrator
            .ice()
            .get("cap-1".to_string())
            .await
            .unwrap()
            .unwrap();
        migrator.promote_to_hot(entry, Tier::Ice).await.unwrap();

        // 验证 Ice 层无条目,Hot 层有条目
        assert!(migrator
            .ice()
            .get("cap-1".to_string())
            .await
            .unwrap()
            .is_none());
        assert!(migrator.hot().contains("cap-1"));

        // 验证事件发布
        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::CapabilityTiered {
                from_tier, to_tier, ..
            } => {
                assert_eq!(from_tier, "Ice");
                assert_eq!(to_tier, "Hot");
            }
            other => panic!("expected CapabilityTiered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_promote_to_hot_with_lru_eviction() {
        let bus = EventBus::new();
        let hot = HotTier::new(2); // 容量 2,容易触发 LRU
        let warm = WarmTier::open_in_memory(4096).unwrap();
        let cold = ColdTier::open_in_memory(65536).unwrap();
        let ice = IceTier::new(tempfile::tempdir().unwrap().path());

        let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

        // 填满 Hot 层
        migrator
            .hot()
            .insert(make_entry("hot-1", Tier::Hot))
            .unwrap();
        migrator
            .hot()
            .insert(make_entry("hot-2", Tier::Hot))
            .unwrap();
        assert_eq!(migrator.hot().len(), 2);

        // 提升第三个条目,应触发 LRU 驱逐
        let entry = make_entry("cap-3", Tier::Warm);
        migrator.warm().insert(entry.clone()).await.unwrap();
        migrator.promote_to_hot(entry, Tier::Warm).await.unwrap();

        // Hot 层应仍为 2(容量上限),但 cap-3 应在其中
        assert_eq!(migrator.hot().len(), 2);
        assert!(migrator.hot().contains("cap-3"));

        // 被驱逐的条目应在 Warm 层(hot-1 或 hot-2)
        assert!(
            migrator
                .warm()
                .peek("hot-1".to_string())
                .await
                .unwrap()
                .is_some()
                || migrator
                    .warm()
                    .peek("hot-2".to_string())
                    .await
                    .unwrap()
                    .is_some()
        );
    }

    #[tokio::test]
    async fn test_migrate_nonexistent_returns_error() {
        let bus = EventBus::new();
        let hot = HotTier::new(256);
        let warm = WarmTier::open_in_memory(4096).unwrap();
        let cold = ColdTier::open_in_memory(65536).unwrap();
        let ice = IceTier::new(tempfile::tempdir().unwrap().path());

        let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

        // 迁移不存在的条目应返回错误
        let result = migrator.migrate_warm_to_cold("nonexistent").await;
        assert!(matches!(result, Err(CmtError::EntryNotFound(_))));
    }
}
