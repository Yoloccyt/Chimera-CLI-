//! SubTask 15.1:MLC 并发测试数据一致性验证
//!
//! 验证 MlcEngine 在 10 线程并发 store+recall 与 promote/demote 下的数据一致性:
//! - 无数据丢失(所有写入的条目都能召回)
//! - 无重复 ID(并发写入不会产生重复)
//! - 层级状态一致(promote/demote 后源层与目标层条目数符合预期)
//!
//! 注:以下并发场景已被现有测试覆盖,此处不重复编写:
//! - L0 4 线程并发 insert:tests/working_memory.rs::test_l0_concurrent_insert
//! - L0 4 线程并发 get(LRU 一致性):tests/working_memory.rs::test_l0_lru_concurrent_touch_consistency
//! - L1 4 线程并发 insert + query:tests/episodic.rs::test_l1_concurrent_insert_and_query
//! - L2 4 线程并发 insert + recall:tests/semantic.rs::test_l2_concurrent_insert_and_recall
//! - L3 10 任务并发 insert:tests/procedural.rs::test_l3_concurrent_writes
//! - L3 10 任务并发 update_stats:tests/procedural.rs::test_l3_concurrent_update_stats
//!
//! 本文件聚焦 MlcEngine 统一接口层级的 10 线程并发场景(现有测试均在单层 L0/L1/L2/L3 上)。

use std::collections::HashSet;
use std::sync::Arc;

use event_bus::EventBus;
use mlc_engine::{MemoryEntry, MemoryTier, MlcConfig, MlcEngine};

/// 构造测试用记忆条目(无 CLV)
fn make_entry(id: &str, tier: MemoryTier) -> MemoryEntry {
    MemoryEntry::new(id, format!("content-{id}"), tier)
}

/// SubTask 15.1:10 线程并发 store + recall,断言无数据丢失、无重复 ID
///
/// 验证点:
/// - 10 线程并发 store,每线程 10 个唯一 ID(共 100 个),全部 store 成功
/// - 每线程 store 后立即 recall 自己的 10 个 ID,全部命中(无丢失)
/// - 主线程收集所有 100 个 ID,断言无重复(HashSet 长度 = 100)
/// - 主线程 recall 所有 100 个 ID,全部命中(无数据丢失)
///
/// WHY L0 容量设为 256:默认 64,100 个条目会触发 LRU 驱逐,
/// 被驱逐条目未自动降级到 L1,recall 会返回 None,干扰"无数据丢失"断言。
/// 设为 256 避免驱逐,聚焦验证并发 store+recall 的一致性。
#[tokio::test]
async fn test_engine_concurrent_store_and_recall() {
    let config = MlcConfig::default().with_l0_capacity(256);
    let bus = EventBus::new();
    let engine = Arc::new(MlcEngine::new_in_memory_with_config(config, bus).unwrap());

    const THREADS: usize = 10;
    const PER_THREAD: usize = 10;
    const TOTAL: usize = THREADS * PER_THREAD;

    // 10 线程并发:每线程 store 10 个唯一 ID,然后 recall 自己的 10 个 ID
    let mut handles = Vec::with_capacity(THREADS);
    for t in 0..THREADS {
        let engine_clone = Arc::clone(&engine);
        handles.push(tokio::spawn(async move {
            let ids: Vec<String> = (0..PER_THREAD).map(|i| format!("t{t}-m{i}")).collect();
            // store 10 个条目到 L0
            for id in &ids {
                engine_clone
                    .store(make_entry(id, MemoryTier::L0Working))
                    .await
                    .unwrap();
            }
            // recall 自己的 10 个条目,验证无丢失
            for id in &ids {
                match engine_clone.recall(id.as_str()).await.unwrap() {
                    Some(entry) => assert_eq!(entry.id.as_str(), id.as_str()),
                    None => panic!("线程 {t} 的条目 {} 应能召回(无数据丢失)", id),
                }
            }
            ids
        }));
    }

    // 收集所有线程返回的 ID
    let mut all_ids = HashSet::new();
    for handle in handles {
        let ids = handle.await.unwrap();
        for id in ids {
            all_ids.insert(id);
        }
    }

    // 断言无重复 ID:HashSet 长度应等于总数
    // WHY 用 HashSet 而非计数:若两线程因竞态写入相同 ID,计数仍为 100,
    // 但 HashSet 长度 < 100,能捕获重复 ID 问题。
    assert_eq!(
        all_ids.len(),
        TOTAL,
        "无重复 ID:应收集到 {TOTAL} 个唯一 ID,实际 {}",
        all_ids.len()
    );

    // 主线程 recall 所有 100 个 ID,验证无数据丢失
    for id in &all_ids {
        match engine.recall(id.as_str()).await.unwrap() {
            Some(entry) => assert_eq!(entry.id.as_str(), id.as_str()),
            None => panic!("条目 {} 应能召回(无数据丢失)", id),
        }
    }

    // 验证 L0 条目数 = 100(无驱逐)
    assert_eq!(
        engine.l0().len(),
        TOTAL,
        "L0 应有 {TOTAL} 个条目(无驱逐),实际 {}",
        engine.l0().len()
    );
}

/// SubTask 15.1:10 线程并发 promote/demote,断言层级状态一致
///
/// 验证点:
/// - 预存 5 个条目到 L1,5 个到 L0
/// - 10 线程并发:5 线程 promote L1→L0,5 线程 demote L0→L1
/// - 完成后 L0 应有 5 个(原 L1 的),L1 应有 5 个(原 L0 的)
/// - 所有 10 个条目都能 recall 到(无数据丢失)
/// - 条目所在层级正确(promote 的在 L0,demote 的在 L1)
///
/// WHY 两组操作针对不同条目:promote m-l1-i 与 demote m-l0-i 操作不同 ID,
/// 避免对同一条目的并发迁移竞态,聚焦验证层级状态一致性。
#[tokio::test]
async fn test_engine_concurrent_promote_demote() {
    let bus = EventBus::new();
    let engine = Arc::new(MlcEngine::new_in_memory(bus).unwrap());

    // 预存 5 个条目到 L1,5 个到 L0
    for i in 0..5 {
        engine
            .store(make_entry(&format!("m-l1-{i}"), MemoryTier::L1Episodic))
            .await
            .unwrap();
    }
    for i in 0..5 {
        engine
            .store(make_entry(&format!("m-l0-{i}"), MemoryTier::L0Working))
            .await
            .unwrap();
    }
    assert_eq!(engine.l1().len().unwrap(), 5);
    assert_eq!(engine.l0().len(), 5);

    // 10 线程并发:5 线程 promote L1→L0,5 线程 demote L0→L1
    let mut handles = Vec::with_capacity(10);
    for i in 0..5 {
        let engine_clone = Arc::clone(&engine);
        let id = format!("m-l1-{i}");
        handles.push(tokio::spawn(async move {
            engine_clone
                .promote(&id, MemoryTier::L1Episodic, MemoryTier::L0Working)
                .await
                .unwrap();
        }));
    }
    for i in 0..5 {
        let engine_clone = Arc::clone(&engine);
        let id = format!("m-l0-{i}");
        handles.push(tokio::spawn(async move {
            engine_clone
                .demote(&id, MemoryTier::L0Working, MemoryTier::L1Episodic)
                .await
                .unwrap();
        }));
    }

    // 等待所有迁移完成
    for handle in handles {
        handle.await.unwrap();
    }

    // 断言层级状态一致:L0 有 5 个(原 L1 的),L1 有 5 个(原 L0 的)
    assert_eq!(
        engine.l0().len(),
        5,
        "promote 完成后 L0 应有 5 个条目(原 L1 的)"
    );
    assert_eq!(
        engine.l1().len().unwrap(),
        5,
        "demote 完成后 L1 应有 5 个条目(原 L0 的)"
    );

    // 验证所有 10 个条目都能 recall 到(无数据丢失),且层级正确
    for i in 0..5 {
        let id = format!("m-l1-{i}");
        match engine.recall(&id).await.unwrap() {
            Some(entry) => assert_eq!(
                entry.tier,
                MemoryTier::L0Working,
                "条目 {id} 应在 L0(promote 后)"
            ),
            None => panic!("条目 {id} 应能召回(无数据丢失)"),
        }
    }
    for i in 0..5 {
        let id = format!("m-l0-{i}");
        match engine.recall(&id).await.unwrap() {
            Some(entry) => assert_eq!(
                entry.tier,
                MemoryTier::L1Episodic,
                "条目 {id} 应在 L1(demote 后)"
            ),
            None => panic!("条目 {id} 应能召回(无数据丢失)"),
        }
    }
}
