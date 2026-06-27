//! 迁移器集成测试 — 验证 Hot→Warm→Cold→Ice 迁移链与 Ice→Hot 提升链
//!
//! 对应 SubTask 3.16:验证四级迁移链与 CapabilityTiered 事件正确发布
//!
//! # 测试覆盖
//! - Hot → Warm 迁移(LRU 驱逐触发)
//! - Warm → Cold 迁移(空闲超时触发)
//! - Cold → Ice 迁移(空闲超时触发)
//! - Ice → Hot 提升(访问触发)
//! - Warm → Hot 提升(访问触发)
//! - Cold → Hot 提升(访问触发)
//! - 完整迁移链:Hot → Warm → Cold → Ice
//! - 完整提升链:Ice → Hot
//! - CapabilityTiered 事件正确发布(每次迁移发布事件)
//! - LRU 驱逐时提升:Hot 层满时驱逐到 Warm
//!
//! 注:SubTask 9.1 将 WarmTier 所有方法改为 async + spawn_blocking,
//! 测试需在 async 方法调用后添加 `.await`,且 peek/get/delete 参数为 `String`。

use cmt_tiering::{
    CapabilityEntry, ColdTier, HotTier, IceTier, MigrationReason, Tier, TierMigrator, WarmTier,
};
use event_bus::{EventBus, EventReceiver, NexusEvent};

/// 构造测试用能力条目
fn make_entry(id: &str, tier: Tier) -> CapabilityEntry {
    CapabilityEntry::new(id, format!("content-{id}"), tier)
}

/// 构造测试用迁移器(所有层使用内存/临时存储)
fn make_migrator() -> (TierMigrator, EventReceiver) {
    let bus = EventBus::new();
    let rx = bus.subscribe();
    let hot = HotTier::new(256);
    let warm = WarmTier::open_in_memory(4096).unwrap();
    let cold = ColdTier::open_in_memory(65536).unwrap();
    let ice = IceTier::new(tempfile::tempdir().unwrap().path());

    let migrator = TierMigrator::new(hot, warm, cold, ice, bus);
    (migrator, rx)
}

#[tokio::test]
async fn test_migrate_hot_to_warm() {
    let (migrator, mut rx) = make_migrator();

    let entry = make_entry("cap-1", Tier::Hot);
    migrator.migrate_hot_to_warm(entry).await.unwrap();

    // 验证 Warm 层有条目
    let fetched = migrator.warm().peek("cap-1".to_string()).await.unwrap();
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().tier, Tier::Warm);

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
    let (migrator, mut rx) = make_migrator();

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
    assert_eq!(fetched.unwrap().tier, Tier::Cold);

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
    let (migrator, mut rx) = make_migrator();

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
    assert_eq!(fetched.unwrap().tier, Tier::Ice);

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
    let (migrator, mut rx) = make_migrator();

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
async fn test_promote_to_hot_from_cold() {
    let (migrator, mut rx) = make_migrator();

    // 先在 Cold 层插入条目
    let entry = make_entry("cap-1", Tier::Cold);
    migrator.cold().insert(entry).await.unwrap();

    // 读取条目并提升到 Hot 层
    let entry = migrator
        .cold()
        .peek("cap-1".to_string())
        .await
        .unwrap()
        .unwrap();
    migrator.promote_to_hot(entry, Tier::Cold).await.unwrap();

    // 验证 Cold 层无条目,Hot 层有条目
    assert!(migrator
        .cold()
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
            assert_eq!(from_tier, "Cold");
            assert_eq!(to_tier, "Hot");
        }
        other => panic!("expected CapabilityTiered, got {other:?}"),
    }
}

#[tokio::test]
async fn test_promote_to_hot_from_ice() {
    let (migrator, mut rx) = make_migrator();

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
async fn test_full_migration_chain_hot_to_ice() {
    // 完整迁移链:Hot → Warm → Cold → Ice
    let (migrator, mut rx) = make_migrator();

    // 1. Hot → Warm
    let entry = make_entry("cap-1", Tier::Hot);
    migrator.migrate_hot_to_warm(entry).await.unwrap();
    let _ = rx.recv().await.unwrap(); // 消费事件

    // 2. Warm → Cold
    migrator.migrate_warm_to_cold("cap-1").await.unwrap();
    let _ = rx.recv().await.unwrap(); // 消费事件

    // 3. Cold → Ice
    migrator.migrate_cold_to_ice("cap-1").await.unwrap();
    let _ = rx.recv().await.unwrap(); // 消费事件

    // 验证条目最终在 Ice 层
    assert!(migrator
        .warm()
        .peek("cap-1".to_string())
        .await
        .unwrap()
        .is_none());
    assert!(migrator
        .cold()
        .peek("cap-1".to_string())
        .await
        .unwrap()
        .is_none());
    let fetched = migrator.ice().get("cap-1".to_string()).await.unwrap();
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().id.as_str(), "cap-1");
}

#[tokio::test]
async fn test_full_promotion_chain_ice_to_hot() {
    // 完整提升链:Ice → Hot(直接提升,跨多层)
    let (migrator, mut rx) = make_migrator();

    // 先将条目归档到 Ice 层
    let entry = make_entry("cap-1", Tier::Ice);
    migrator.ice().archive(entry).await.unwrap();

    // 从 Ice 层读取并直接提升到 Hot 层
    let entry = migrator
        .ice()
        .get("cap-1".to_string())
        .await
        .unwrap()
        .unwrap();
    migrator.promote_to_hot(entry, Tier::Ice).await.unwrap();

    // 消费事件
    let _ = rx.recv().await.unwrap();

    // 验证条目在 Hot 层,不在 Ice 层
    assert!(migrator.hot().contains("cap-1"));
    assert!(migrator
        .ice()
        .get("cap-1".to_string())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_promote_to_hot_with_lru_eviction() {
    // Hot 层满时提升,应触发 LRU 驱逐到 Warm 层
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
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

    // 应至少收到 1 个事件(LRU 驱逐或提升)
    let mut event_count = 0;
    while let Ok(Some(_)) = rx.try_recv() {
        event_count += 1;
    }
    assert!(event_count >= 1, "应至少发布 1 个 CapabilityTiered 事件");
}

#[tokio::test]
async fn test_migrate_nonexistent_returns_error() {
    let (migrator, _rx) = make_migrator();

    // 迁移不存在的条目应返回错误
    let result = migrator.migrate_warm_to_cold("nonexistent").await;
    assert!(matches!(
        result,
        Err(cmt_tiering::CmtError::EntryNotFound(_))
    ));

    let result = migrator.migrate_cold_to_ice("nonexistent").await;
    assert!(matches!(
        result,
        Err(cmt_tiering::CmtError::EntryNotFound(_))
    ));
}

#[tokio::test]
async fn test_promote_from_hot_is_noop() {
    // 从 Hot 层提升到 Hot 层是 no-op
    let (migrator, _rx) = make_migrator();

    let entry = make_entry("cap-1", Tier::Hot);
    migrator.hot().insert(entry.clone()).unwrap();

    // 提升应立即返回 Ok(无需操作)
    let result = migrator.promote_to_hot(entry, Tier::Hot).await;
    assert!(result.is_ok());

    // Hot 层应仍有条目
    assert!(migrator.hot().contains("cap-1"));
}

#[tokio::test]
async fn test_migration_preserves_entry_data() {
    // 迁移过程中条目数据(content)应保持不变
    let (migrator, _rx) = make_migrator();

    let original_content = "important-capability-content-with-详细-中文";
    let mut entry = make_entry("cap-1", Tier::Hot);
    entry.content = original_content.to_string();

    // Hot → Warm
    migrator.migrate_hot_to_warm(entry).await.unwrap();
    let fetched = migrator
        .warm()
        .peek("cap-1".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.content, original_content);

    // Warm → Cold
    migrator.migrate_warm_to_cold("cap-1").await.unwrap();
    let fetched = migrator
        .cold()
        .peek("cap-1".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.content, original_content);

    // Cold → Ice
    migrator.migrate_cold_to_ice("cap-1").await.unwrap();
    let fetched = migrator
        .ice()
        .get("cap-1".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.content, original_content);

    // Ice → Hot(提升)
    let entry = migrator
        .ice()
        .get("cap-1".to_string())
        .await
        .unwrap()
        .unwrap();
    migrator.promote_to_hot(entry, Tier::Ice).await.unwrap();
    let fetched = migrator.hot().get("cap-1").unwrap();
    assert_eq!(fetched.content, original_content);
}

#[tokio::test]
async fn test_multiple_migrations_publish_events() {
    // 多次迁移应发布多个事件
    let (migrator, mut rx) = make_migrator();

    // 迁移 3 个不同的条目
    migrator
        .migrate_hot_to_warm(make_entry("cap-1", Tier::Hot))
        .await
        .unwrap();
    migrator
        .migrate_hot_to_warm(make_entry("cap-2", Tier::Hot))
        .await
        .unwrap();
    migrator
        .migrate_hot_to_warm(make_entry("cap-3", Tier::Hot))
        .await
        .unwrap();

    // 应收到 3 个事件
    let mut event_count = 0;
    while let Ok(Some(_)) = rx.try_recv() {
        event_count += 1;
    }
    assert_eq!(event_count, 3);
}

#[tokio::test]
async fn test_migration_reason_in_event() {
    // 验证事件中的 reason 字段正确
    let (migrator, mut rx) = make_migrator();

    // Hot → Warm(LRU 驱逐)
    migrator
        .migrate_hot_to_warm(make_entry("cap-1", Tier::Hot))
        .await
        .unwrap();
    let event = rx.recv().await.unwrap();
    if let NexusEvent::CapabilityTiered { reason, .. } = event {
        assert_eq!(reason, MigrationReason::LruEviction.as_str());
    } else {
        panic!("expected CapabilityTiered event");
    }

    // Warm → Cold(空闲超时)
    migrator
        .warm()
        .insert(make_entry("cap-2", Tier::Warm))
        .await
        .unwrap();
    migrator.migrate_warm_to_cold("cap-2").await.unwrap();
    let event = rx.recv().await.unwrap();
    if let NexusEvent::CapabilityTiered { reason, .. } = event {
        assert_eq!(reason, MigrationReason::IdleTimeout.as_str());
    } else {
        panic!("expected CapabilityTiered event");
    }

    // Cold → Ice(空闲超时)
    migrator
        .cold()
        .insert(make_entry("cap-3", Tier::Cold))
        .await
        .unwrap();
    migrator.migrate_cold_to_ice("cap-3").await.unwrap();
    let event = rx.recv().await.unwrap();
    if let NexusEvent::CapabilityTiered { reason, .. } = event {
        assert_eq!(reason, MigrationReason::IdleTimeout.as_str());
    } else {
        panic!("expected CapabilityTiered event");
    }
}

#[tokio::test]
async fn test_migrate_warm_to_cold_rollback_preserves_data() {
    // 模拟 Cold 层写入失败,验证 Warm 层仍保留原始条目(非假数据)
    // WHY:此前实现用 "rollback-content" 假数据回滚,导致原始 content 丢失;
    // 修复后改为"先写入 Cold → 再删除 Warm",Cold 失败时 Warm 未受影响
    let bus = EventBus::new();
    let hot = HotTier::new(256);
    let warm = WarmTier::open_in_memory(4096).unwrap();
    let cold = ColdTier::open_in_memory(65536).unwrap();
    let ice = IceTier::new(tempfile::tempdir().unwrap().path());
    let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

    // 在 Warm 层插入带特殊 content 的条目
    let original_content = "important-warm-content-with-data";
    let mut entry = make_entry("cap-warm", Tier::Warm);
    entry.content = original_content.to_string();
    migrator.warm().insert(entry).await.unwrap();

    // 破坏 Cold 层:DROP TABLE 使后续 INSERT 因"no such table"失败
    // WHY:ColdTier 使用内存 SQLite 连接,删除文件不会让 INSERT 失败(连接持有句柄)。
    // break_for_testing 通过 DROP TABLE 可靠地让后续 insert 返回 Err。
    migrator.cold().break_for_testing().await.unwrap();

    // 尝试迁移到 Cold 层,应失败(表已不存在)
    let result = migrator.migrate_warm_to_cold("cap-warm").await;
    assert!(result.is_err(), "Cold 层写入失败应返回错误");

    // 验证 Warm 层仍保留原始条目,content 不是假数据
    let fetched = migrator
        .warm()
        .peek("cap-warm".to_string())
        .await
        .unwrap()
        .expect("Warm 层应保留原始条目");
    assert_eq!(
        fetched.content, original_content,
        "Warm 层应保留原始 content"
    );
    assert_ne!(fetched.content, "preserved-content", "不应是假数据");
    assert_ne!(fetched.content, "rollback-content", "不应是假数据");
}

#[tokio::test]
async fn test_migrate_cold_to_ice_rollback_preserves_data() {
    // 模拟 Ice 层写入失败,验证 Cold 层仍保留原始条目(非假数据)
    // WHY:此前实现用 "rollback-content" 假数据回滚,导致原始 content 丢失;
    // 修复后改为"先归档 Ice → 再删除 Cold",Ice 失败时 Cold 未受影响
    let bus = EventBus::new();
    let hot = HotTier::new(256);
    let warm = WarmTier::open_in_memory(4096).unwrap();
    let cold = ColdTier::open_in_memory(65536).unwrap();

    // 创建一个文件作为 Ice 层目录的父路径,导致 create_dir_all 失败
    // (父路径是文件不是目录,无法在其下创建子目录)
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let invalid_ice_dir = blocker.path().join("ice");
    let ice = IceTier::new(invalid_ice_dir);
    let migrator = TierMigrator::new(hot, warm, cold, ice, bus);

    // 在 Cold 层插入带特殊 content 的条目
    let original_content = "important-cold-content-with-data";
    let mut entry = make_entry("cap-cold", Tier::Cold);
    entry.content = original_content.to_string();
    migrator.cold().insert(entry).await.unwrap();

    // 尝试迁移到 Ice 层,应失败(Ice 层路径无效:父路径是文件)
    let result = migrator.migrate_cold_to_ice("cap-cold").await;
    assert!(result.is_err(), "Ice 层写入失败应返回错误");

    // 验证 Cold 层仍保留原始条目,content 不是假数据
    let fetched = migrator
        .cold()
        .peek("cap-cold".to_string())
        .await
        .unwrap()
        .expect("Cold 层应保留原始条目");
    assert_eq!(
        fetched.content, original_content,
        "Cold 层应保留原始 content"
    );
    assert_ne!(fetched.content, "preserved-content", "不应是假数据");
    assert_ne!(fetched.content, "rollback-content", "不应是假数据");
}
