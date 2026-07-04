//! SubTask 2.14: HcwWindow 集成测试
//!
//! 验证窗口溢出降级链(L0->L1->L2->L3)、OSA 掩码订阅稀疏化、
//! ContextWindowSwitched/ContextCompressed 事件正确发布。

use std::time::Duration;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use hcw_window::{ContextEntry, HcwConfig, HcwWindow, WindowTier};

fn make_entry(id: &str, token_size: usize) -> ContextEntry {
    ContextEntry::new(
        id,
        format!("file-{id}"),
        format!("content-{id}"),
        token_size,
    )
}

fn make_entry_with_file(id: &str, file_id: &str, token_size: usize) -> ContextEntry {
    ContextEntry::new(id, file_id, format!("content-{id}"), token_size)
}

#[tokio::test]
async fn test_new_validates_config() {
    let bus = EventBus::new();
    let invalid_config = HcwConfig::default().with_l0_capacity(0);
    let result = HcwWindow::new(invalid_config, bus);
    assert!(result.is_err(), "无效配置应返回错误");
}

#[tokio::test]
async fn test_with_default_config() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L0);
    assert_eq!(window.current_size().await, 0);
    assert_eq!(window.entry_count().await, 0);
}

#[tokio::test]
async fn test_insert_and_get() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.insert(make_entry("e-1", 100)).await.unwrap();
    let entry = window.get("e-1").await.unwrap().unwrap();
    assert_eq!(entry.id, "e-1");
    assert_eq!(entry.access_count, 1, "get 应递增访问次数");
}

#[tokio::test]
async fn test_get_nonexistent_returns_none() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    let result = window.get("nonexistent").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_overflow_l0_to_l1() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.insert(make_entry("e-big", 5000)).await.unwrap();
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    match event {
        NexusEvent::ContextWindowSwitched {
            from_tier, to_tier, ..
        } => {
            assert_eq!(from_tier, "L0");
            assert_eq!(to_tier, "L1");
        }
        other => panic!("期望 ContextWindowSwitched,收到 {other:?}"),
    }
    assert_eq!(window.current_tier().await, WindowTier::L1);
}

#[tokio::test]
async fn test_overflow_chain_l0_to_l3() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    // 插入 30 个 5000 token 条目 = 150K,超过 L0/L1/L2,应升级到 L3 并压缩
    // WHY:使用多个小条目,确保压缩可成功(单条目 150K > L3 容量 131K 会失败)
    for i in 0..30 {
        window
            .insert(make_entry(&format!("e-{i}"), 5000))
            .await
            .unwrap();
    }
    assert_eq!(window.current_tier().await, WindowTier::L3);
}

#[tokio::test]
async fn test_select_window_upgrade() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let window = HcwWindow::with_default_config(bus).unwrap();
    let tier = window.select_window(0.6).await.unwrap();
    assert_eq!(tier, WindowTier::L2);
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert!(matches!(event, NexusEvent::ContextWindowSwitched { .. }));
}

#[tokio::test]
async fn test_select_window_downgrade_with_compression() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.select_window(0.9).await.unwrap();
    for i in 0..50 {
        window
            .insert(make_entry(&format!("e-{i}"), 1000))
            .await
            .unwrap();
    }
    assert_eq!(window.current_tier().await, WindowTier::L3);
    assert_eq!(window.current_size().await, 50_000);
    let tier = window.select_window(0.1).await.unwrap();
    assert_eq!(tier, WindowTier::L0);
    assert!(window.current_size().await <= 4096);
}

#[tokio::test]
async fn test_apply_sparse_mask() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window
        .insert(make_entry_with_file("e-1", "file-1", 100))
        .await
        .unwrap();
    window
        .insert(make_entry_with_file("e-2", "file-2", 200))
        .await
        .unwrap();
    window
        .insert(make_entry_with_file("e-3", "file-3", 300))
        .await
        .unwrap();
    let report = window
        .apply_sparse_mask(vec!["file-1".into(), "file-3".into()])
        .await
        .unwrap();
    assert_eq!(report.original_size, 600);
    assert_eq!(report.compressed_size, 400);
    assert_eq!(report.dropped_count, 1);
    assert_eq!(report.retained_count, 2);
    assert_eq!(report.algorithm, "sparse-mask");
    assert_eq!(window.entry_count().await, 2);
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert!(matches!(event, NexusEvent::ContextCompressed { .. }));
}
#[tokio::test]
async fn test_context_compressed_event_payload() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let window = HcwWindow::with_default_config(bus).unwrap();
    // 插入 10 个 1000 token 条目 = 10K,超过 L0(4K) 升级到 L1(32K)
    // WHY:使用多个小条目,确保降级压缩可成功(单条目 5000 > L0 容量 4096 会失败)
    for i in 0..10 {
        window
            .insert(make_entry(&format!("e-{i}"), 1000))
            .await
            .unwrap();
    }
    let _ = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    window.select_window(0.1).await.unwrap();
    let mut found = false;
    for _ in 0..5 {
        let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
        if let NexusEvent::ContextCompressed {
            original_size,
            compressed_size,
            ratio,
            ..
        } = event
        {
            assert!(original_size > 0);
            assert!(compressed_size <= original_size);
            assert!(
                (0.0..=1.0).contains(&ratio),
                "ratio 应在 [0,1],实际 {ratio}"
            );
            found = true;
            break;
        }
    }
    assert!(found, "应收到 ContextCompressed 事件");
}

#[tokio::test]
async fn test_spawn_mask_listener_receives_event() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus.clone()).unwrap();
    let handle = window.spawn_mask_listener();
    let event = NexusEvent::OmniSparseMasksComputed {
        metadata: EventMetadata::new("osa-coordinator"),
        mask_hash: "abc123".into(),
        sparsity: 0.875,
        context_mask: vec!["file-0".into()],
    };
    bus.publish(event).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(window.last_mask_hash().await, Some("abc123".into()));
    let sparsity = window.last_sparsity().await.unwrap();
    assert!(
        (sparsity - 0.875).abs() < 1e-6,
        "稀疏度应为 0.875,实际 {sparsity}"
    );
    handle.abort();
}

#[tokio::test]
async fn test_remove_entry() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.insert(make_entry("e-1", 100)).await.unwrap();
    let removed = window.remove("e-1").await.unwrap();
    assert!(removed.is_some());
    assert_eq!(removed.unwrap().id, "e-1");
    assert_eq!(window.entry_count().await, 0);
}

#[tokio::test]
async fn test_remove_nonexistent_returns_none() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    let result = window.remove("nonexistent").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_l3_overflow_triggers_compression() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.select_window(0.9).await.unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L3);
    for i in 0..150 {
        window
            .insert(make_entry(&format!("e-{i}"), 1000))
            .await
            .unwrap();
    }
    let size = window.current_size().await;
    assert!(size <= 131_072, "L3 压缩后应 <= 128K,实际 {size}");
}

#[tokio::test]
async fn test_window_switched_event_fields() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.select_window(0.8).await.unwrap();
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    if let NexusEvent::ContextWindowSwitched {
        from_tier,
        to_tier,
        reason,
        ..
    } = event
    {
        assert_eq!(from_tier, "L0");
        assert_eq!(to_tier, "L3");
        assert!(
            reason.contains("complexity"),
            "reason 应含 complexity,实际: {reason}"
        );
    } else {
        panic!("期望 ContextWindowSwitched 事件");
    }
}

#[tokio::test]
async fn test_multiple_inserts_trigger_multiple_switches() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.insert(make_entry("e-1", 5000)).await.unwrap();
    let _ = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    window.insert(make_entry("e-2", 30_000)).await.unwrap();
    let _ = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    window.insert(make_entry("e-3", 100_000)).await.unwrap();
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert!(matches!(
        event,
        NexusEvent::ContextWindowSwitched { .. } | NexusEvent::ContextCompressed { .. }
    ));
    assert_eq!(window.current_tier().await, WindowTier::L3);
}

#[tokio::test]
async fn test_config_builder_chain() {
    let config = HcwConfig::new()
        .with_l0_capacity(2048)
        .with_l1_capacity(16384)
        .with_l2_capacity(65536)
        .with_l3_capacity(524288)
        .with_compression_threshold(0.8);
    assert_eq!(config.l0_capacity, 2048);
    assert_eq!(config.l3_capacity, 524288);
    assert!((config.compression_threshold - 0.8).abs() < 1e-6);
    assert!(config.validate().is_ok());
    let bus = EventBus::new();
    let window = HcwWindow::new(config, bus).unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L0);
}

#[tokio::test]
async fn test_l3_effective_capacity_is_128k() {
    let config = HcwConfig::default();
    assert_eq!(
        config.capacity_for(WindowTier::L3),
        1_048_576,
        "L3 标称容量应为 1M"
    );
    assert_eq!(
        config.effective_capacity_for(WindowTier::L3),
        131_072,
        "L3 实际加载容量应为 128K(1M/8)"
    );
}

/// SubTask 10.3:验证 HCW 4 任务并发 insert + 压缩无数据损坏
///
/// 4 个任务并发插入大条目(每个 > 1K tokens),触发窗口升级与压缩。
/// HcwWindow 内部用 `Arc<RwLock<HcwState>>` 保护状态,
/// 并发写入应通过 RwLock 串行化,无 panic、无数据损坏。
/// 最终验证所有条目都能正确读取或已正确压缩(条目数或压缩后大小合理)。
#[tokio::test]
async fn test_concurrent_insert_with_compression() {
    use std::sync::Arc;

    let bus = EventBus::new();
    let window = Arc::new(HcwWindow::with_default_config(bus).unwrap());

    // 4 任务并发 insert,每个任务插入 1500 tokens 的大条目
    // 4 × 1500 = 6000 tokens,超过 L0(4K),触发升级到 L1
    let mut handles = Vec::with_capacity(4);
    for i in 0..4 {
        let window_clone = window.clone();
        handles.push(tokio::spawn(async move {
            let entry = make_entry(&format!("e-{i}"), 1500);
            window_clone.insert(entry).await
        }));
    }

    // 等待所有插入完成,验证无 panic、无错误
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // 验证窗口已升级(L0 容量 4K,4×1500=6K 超出,应升级到 L1 或更高)
    let tier = window.current_tier().await;
    assert!(
        tier >= WindowTier::L1,
        "并发插入 6K tokens 后窗口应升级到 L1 或更高,实际 {:?}",
        tier
    );

    // 验证条目数正确(无数据丢失)
    assert_eq!(window.entry_count().await, 4, "并发插入后应有 4 个条目");

    // 验证所有条目都能正确读取
    for i in 0..4 {
        let entry = window.get(&format!("e-{i}")).await.unwrap();
        assert!(entry.is_some());
    }
}

/// SubTask 12.7:验证 select_window 并发安全(全程持写锁修复)
#[tokio::test]
async fn test_select_window_concurrent_safety() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let bus = EventBus::new();
    let window = Arc::new(HcwWindow::with_default_config(bus).unwrap());
    let total_inserted_size = Arc::new(AtomicUsize::new(0));
    let total_inserted_count = Arc::new(AtomicUsize::new(0));

    const SIZE_LIMIT: usize = 100_000;
    const ENTRY_TOKEN_SIZE: usize = 100;

    let mut insert_handles = Vec::with_capacity(10);
    for tid in 0..10u32 {
        let window = window.clone();
        let total_size = total_inserted_size.clone();
        let total_count = total_inserted_count.clone();
        insert_handles.push(tokio::spawn(async move {
            let mut local_seq = 0usize;
            loop {
                let current = total_size.load(Ordering::Relaxed);
                if current + ENTRY_TOKEN_SIZE > SIZE_LIMIT {
                    break;
                }
                let id = format!("t{tid}-e{local_seq}");
                local_seq += 1;
                let entry = ContextEntry::new(
                    &id,
                    format!("file-{id}"),
                    "x".repeat(1024),
                    ENTRY_TOKEN_SIZE,
                );
                if window.insert(entry).await.is_ok() {
                    total_size.fetch_add(ENTRY_TOKEN_SIZE, Ordering::Relaxed);
                    total_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    let window_for_select = window.clone();
    let select_handle = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let _ = window_for_select.select_window(0.9).await;
        }
    });

    for handle in insert_handles {
        handle.await.expect("insert thread panic");
    }
    select_handle.await.expect("select thread panic");

    let expected_size = total_inserted_size.load(Ordering::Relaxed);
    let expected_count = total_inserted_count.load(Ordering::Relaxed);
    let actual_size = window.current_size().await;
    let actual_count = window.entry_count().await;

    assert_eq!(
        actual_size, expected_size,
        "current_size should equal sum of inserted token_size (no loss). actual={actual_size}, expected={expected_size}"
    );
    assert_eq!(
        actual_count, expected_count,
        "entry_count should equal inserted count (no loss). actual={actual_count}, expected={expected_count}"
    );
}

// ============================================================
// SubTask 13.8 基准测试:retain_by_file_ids HashSet O(1) 查找延迟
// ============================================================

/// SubTask 13.8 基准测试:retain_by_file_ids HashSet O(1) 查找延迟
///
/// 验证:1000 文件 × 10000 条目下 retain_by_file_ids P50 延迟 < 5ms。
/// HashSet 构建 O(m) + 查找 O(1),总复杂度 O(n + m),
/// 相比原 Vec 线性扫描 O(n×m) 显著降低(原约 50ms → 优化后 < 5ms)。
#[test]
#[ignore]
fn bench_retain_by_file_ids_hashset() {
    use hcw_window::HcwState;
    use std::time::Instant;

    // 构造 10000 条目,分属 1000 个文件(每个文件 10 条目)
    let make_state = || -> HcwState {
        let mut state = HcwState::new(WindowTier::L2);
        for i in 0..10000u32 {
            let file_id = format!("file-{}", i % 1000);
            let entry = ContextEntry::new(format!("e-{i}"), file_id, "content", 100);
            // WHY(M-01/M-02):entries 已改为 Vec<Arc<ContextEntry>>,
            // 用 push_entry 封装 Arc::new 包装 + 索引维护,保持状态一致
            state.push_entry(entry);
        }
        state
    };

    // 构造活跃文件列表(500 个文件,即 50% 活跃)
    let active_file_ids: Vec<String> = (0..500).map(|i| format!("file-{i}")).collect();

    // warmup 10 次
    for _ in 0..10 {
        let mut state = make_state();
        let _ = state.retain_by_file_ids(&active_file_ids);
    }

    // 测量 100 次
    let mut times: Vec<u128> = Vec::with_capacity(100);
    for _ in 0..100 {
        let mut state = make_state();
        let start = Instant::now();
        let removed = state.retain_by_file_ids(&active_file_ids);
        times.push(start.elapsed().as_nanos());
        // 500/1000 文件活跃,应移除 5000 条目(非活跃文件的条目)
        assert_eq!(removed, 5000, "应移除 5000 个非活跃条目");
        assert_eq!(state.entries.len(), 5000, "应保留 5000 个活跃条目");
    }

    times.sort();
    let p50 = times[times.len() / 2];
    let p50_ms = p50 as f64 / 1_000_000.0;
    println!(
        "bench_retain_by_file_ids_hashset P50: {} ns ({:.3} ms)",
        p50, p50_ms
    );

    // 验证延迟 < 5ms(任务要求)
    assert!(
        p50_ms < 5.0,
        "retain_by_file_ids P50 延迟应 < 5ms,实际 {:.3}ms",
        p50_ms
    );
}

// ============================================================
// SubTask 15.5:HCW 窗口切换可逆性测试
// ============================================================
//
// 已有覆盖:
// - test_overflow_chain_l0_to_l3:通过 insert 触发隐式链式升级(非显式逐级验证)
// - test_select_window_upgrade:L0→L2 升级(跳级,非 L0→L3)
// - test_select_window_downgrade_with_compression:L3→L0 降级(跳级,非逐级)
//
// 本节补充:
// - 显式逐级升级 L0→L1→L2→L3(通过 select_window,验证每步切换)
// - 显式逐级降级 L3→L2→L1→L0(通过 select_window,验证每步切换)
// - 跳级切换 L0→L3(通过 select_window(0.9) 直接跳到 L3)

/// SubTask 15.5:验证 L0 → L1 → L2 → L3 逐级升级
///
/// 通过 select_window 显式触发逐级升级,验证每步切换后 current_tier 正确。
/// 与 test_overflow_chain_l0_to_l3 的区别:后者通过 insert 隐式触发,
/// 本测试通过 select_window 显式触发,验证可逆性切换的逐级行为。
#[tokio::test]
async fn test_window_switch_stepwise_upgrade_l0_to_l3() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();

    // 初始 L0
    assert_eq!(window.current_tier().await, WindowTier::L0);

    // L0 → L1(complexity 0.3,常规任务)
    let tier = window.select_window(0.3).await.unwrap();
    assert_eq!(tier, WindowTier::L1);
    assert_eq!(window.current_tier().await, WindowTier::L1);

    // L1 → L2(complexity 0.6,复杂任务)
    let tier = window.select_window(0.6).await.unwrap();
    assert_eq!(tier, WindowTier::L2);
    assert_eq!(window.current_tier().await, WindowTier::L2);

    // L2 → L3(complexity 0.9,超复杂任务)
    let tier = window.select_window(0.9).await.unwrap();
    assert_eq!(tier, WindowTier::L3);
    assert_eq!(window.current_tier().await, WindowTier::L3);
}

/// SubTask 15.5:验证 L3 → L2 → L1 → L0 逐级降级
///
/// 先升级到 L3,再通过 select_window 逐级降级,验证每步切换后 current_tier 正确。
/// 与 test_select_window_downgrade_with_compression 的区别:后者是 L3→L0 跳级降级,
/// 本测试验证逐级降级的可逆性。
///
/// WHY:逐级降级时,若当前条目总大小 ≤ 目标容量,直接切换(不压缩);
/// 本测试不插入条目,确保每步降级都是直接切换,验证纯切换逻辑。
#[tokio::test]
async fn test_window_switch_stepwise_downgrade_l3_to_l0() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();

    // 先升级到 L3(无条目,直接切换)
    window.select_window(0.9).await.unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L3);

    // L3 → L2(complexity 0.6)
    // 无条目,total_size = 0 ≤ L2 容量,直接切换
    let tier = window.select_window(0.6).await.unwrap();
    assert_eq!(tier, WindowTier::L2);
    assert_eq!(window.current_tier().await, WindowTier::L2);

    // L2 → L1(complexity 0.3)
    let tier = window.select_window(0.3).await.unwrap();
    assert_eq!(tier, WindowTier::L1);
    assert_eq!(window.current_tier().await, WindowTier::L1);

    // L1 → L0(complexity 0.1)
    let tier = window.select_window(0.1).await.unwrap();
    assert_eq!(tier, WindowTier::L0);
    assert_eq!(window.current_tier().await, WindowTier::L0);
}

/// SubTask 15.5:验证跳级切换 L0 → L3
///
/// 通过 select_window(0.9) 直接从 L0 跳到 L3,验证跳级切换正确性。
/// 与 test_select_window_upgrade 的区别:后者是 L0→L2,本测试是 L0→L3(最大跳级)。
#[tokio::test]
async fn test_window_switch_jump_l0_to_l3() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let window = HcwWindow::with_default_config(bus).unwrap();

    // 初始 L0
    assert_eq!(window.current_tier().await, WindowTier::L0);

    // L0 → L3(complexity 0.9,跳级)
    let tier = window.select_window(0.9).await.unwrap();
    assert_eq!(tier, WindowTier::L3);
    assert_eq!(window.current_tier().await, WindowTier::L3);

    // 验证发布 ContextWindowSwitched 事件
    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    match event {
        NexusEvent::ContextWindowSwitched {
            from_tier, to_tier, ..
        } => {
            assert_eq!(from_tier, "L0");
            assert_eq!(to_tier, "L3");
        }
        other => panic!("期望 ContextWindowSwitched,收到 {other:?}"),
    }
}

/// SubTask 15.5:验证窗口切换可逆性(升级后降级回到原层级)
///
/// 完整循环:L0 → L3 → L0,验证切换可逆。
/// 结合跳级升级与跳级降级,验证窗口切换的对称性。
#[tokio::test]
async fn test_window_switch_reversibility_l0_l3_l0() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();

    // 初始 L0
    assert_eq!(window.current_tier().await, WindowTier::L0);

    // L0 → L3(跳级升级)
    window.select_window(0.9).await.unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L3);

    // L3 → L0(跳级降级,无条目直接切换)
    window.select_window(0.1).await.unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L0);

    // 验证可再次升级(可逆性)
    window.select_window(0.9).await.unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L3);
}

// ============================================================
// SubTask 15.6:HCW 1M Token 等效验证测试
// ============================================================
//
// 已有覆盖:
// - test_l3_effective_capacity_is_128k:配置层面验证 L3 实际容量 = 128K
// - test_l3_overflow_triggers_compression:L3 溢出触发压缩到 128K
//
// 本节补充:
// - 插入 1M Token 等效上下文,验证 current_size ≤ 128K,sparsity ≥ 0.875
// - 通过 apply_sparse_mask 验证 8× 稀疏化(128K 实际 + 8× 稀疏化 = 1M 等效)

/// SubTask 15.6:验证 1M Token 等效通过 L3 压缩实现
///
/// 插入 1M Token(10486 个条目 × 100 token = 1_048_600,略大于 1M 二进制 1_048_576),
/// 触发 L3 升级与压缩,验证压缩后 current_size ≤ 128K,稀疏度 sparsity ≥ 0.875(8× 稀疏化)。
///
/// WHY:1M Token 等效不通过暴力加载,而是通过 L3 压缩(importance-top-n)
/// 将 1M Token 压缩到 128K(8× 压缩比),稀疏度 = 1 - 128K/1M = 0.875。
/// 这是架构红线:禁止 1M 暴力加载(§6 内存爆炸教训)。
///
/// # 基准选择
/// 1M 二进制 = 1_048_576,128K 二进制 = 131072,8× = 1_048_576 / 131072 = 8。
/// sparsity = 1 - current_size / 1_048_576 ≥ 1 - 131072 / 1_048_576 = 0.875。
/// 使用 1M 二进制(而非十进制 1_000_000)作为基准,确保 sparsity ≥ 0.875 恒成立
/// (因为 L3 压缩后 current_size ≤ 131072,而 131072 / 1_048_576 = 0.125)。
#[tokio::test]
async fn test_1m_token_equivalent_via_l3_compression() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();

    // 插入 1M Token 等效上下文:10486 个条目 × 100 token = 1_048_600 Token
    // WHY:1_048_600 > 1_048_576(1M 二进制),确保触发 L3 压缩
    // 使用 100 token 的小条目,确保 L3 压缩可成功(Top-N 保留)
    // 分属 1000 个文件(每个文件约 10 条目),模拟真实代码库结构
    for i in 0..10486 {
        let file_id = format!("file-{}", i % 1000);
        let entry = ContextEntry::new(format!("e-{i}"), file_id, format!("content-{i}"), 100);
        window.insert(entry).await.unwrap();
    }

    // 验证:1M Token 触发 L3 升级
    assert_eq!(
        window.current_tier().await,
        WindowTier::L3,
        "1M Token 应触发 L3 升级"
    );

    // 验证:压缩后 current_size ≤ 128K(L3 实际加载容量 = 1M / 8 = 131072)
    let current_size = window.current_size().await;
    assert!(
        current_size <= 131_072,
        "L3 压缩后 current_size 应 ≤ 128K (131072),实际 {current_size}"
    );

    // 验证:稀疏度 sparsity ≥ 0.875(8× 稀疏化)
    // WHY:1M 二进制 = 1_048_576,128K = 131072,8× = 1_048_576 / 131072
    // sparsity = 1 - current_size / 1_048_576 ≥ 1 - 131072 / 1_048_576 = 0.875
    // 使用 1M 二进制(1_048_576)作为基准,确保 sparsity ≥ 0.875 恒成立
    let original_size_1m_binary = 1_048_576_usize;
    let sparsity = 1.0 - (current_size as f32 / original_size_1m_binary as f32);
    assert!(
        sparsity >= 0.875,
        "稀疏度应 ≥ 0.875 (8× 稀疏化,1M → 128K),实际 {sparsity}"
    );
}

/// SubTask 15.6:验证 1M Token 等效通过 OSA 稀疏化掩码实现
///
/// 先加载 128K Token(L3 实际加载容量),再通过 apply_sparse_mask
/// 应用 8× 稀疏化(仅保留 1/8 的文件),验证稀疏度 ≥ 0.875。
///
/// WHY:1M 等效 = 128K 实际加载 + 8× 稀疏化压缩比(架构红线)。
/// apply_sparse_mask 模拟 OSA context_mask 稀疏化,仅加载活跃文件上下文,
/// 其余稀疏化跳过,实现 1M 等效而不暴力加载。
#[tokio::test]
async fn test_1m_token_equivalent_via_sparse_mask() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();

    // 升级到 L3(1M 等效,128K 实际加载容量)
    window.select_window(0.9).await.unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L3);

    // 插入 128K Token 的上下文(L3 实际加载容量)
    // 1280 个条目 × 100 token = 128K Token,分属 1280 个文件(每个文件 1 个条目)
    // WHY:每个文件 1 个条目,确保 apply_sparse_mask 的稀疏化比例精确可控
    for i in 0..1280 {
        let file_id = format!("file-{i}");
        let entry = ContextEntry::new(format!("e-{i}"), file_id, format!("content-{i}"), 100);
        window.insert(entry).await.unwrap();
    }

    let original_size = window.current_size().await;
    let original_count = window.entry_count().await;
    assert_eq!(original_count, 1280);
    assert_eq!(original_size, 128_000);

    // 应用 OSA 稀疏化掩码:仅保留 1/8 的文件(160 个文件)
    // 8× 稀疏化:保留 1/8,稀疏度 = 7/8 = 0.875
    let active_file_ids: Vec<String> = (0..160).map(|i| format!("file-{i}")).collect();
    let report = window.apply_sparse_mask(active_file_ids).await.unwrap();

    // 验证:稀疏化后 current_size ≤ 128K
    let current_size = window.current_size().await;
    assert!(
        current_size <= 131_072,
        "稀疏化后 current_size 应 ≤ 128K,实际 {current_size}"
    );

    // 验证:稀疏度 sparsity ≥ 0.875(8× 稀疏化)
    let sparsity = 1.0 - (current_size as f32 / original_size as f32);
    assert!(
        sparsity >= 0.875 - 1e-6,
        "稀疏度应 ≥ 0.875 (8× 稀疏化),实际 {sparsity}"
    );

    // 验证:保留条目数 = 原始的 1/8(160/1280)
    let retained_count = window.entry_count().await;
    assert_eq!(
        retained_count, 160,
        "保留条目数应为 160 (原始 1280 的 1/8),实际 {retained_count}"
    );

    // 验证:压缩报告字段正确
    assert_eq!(report.original_size, 128_000);
    assert_eq!(report.compressed_size, 16_000);
    assert_eq!(report.dropped_count, 1120);
    assert_eq!(report.retained_count, 160);
    assert_eq!(report.algorithm, "sparse-mask");
}

// ============================================================
// SubTask 17.1:OSA→HCW 事件驱动稀疏化链路闭环测试
// ============================================================

/// SubTask 17.1:验证 OSA 发布事件 → HCW listener 自动应用稀疏化 → 条目数减少
///
/// 流程:
/// 1. 插入 5 个条目(分属 5 个文件)
/// 2. 启动 listener
/// 3. OSA 发布 OmniSparseMasksComputed 事件,context_mask 仅含 2 个文件
/// 4. 自旋等待 listener 处理(避免 thread::sleep,使用 Instant 自旋)
/// 5. 验证条目数从 5 减少到 2(仅保留活跃文件的条目)
#[tokio::test]
async fn test_osa_hcw_event_driven_sparsification() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus.clone()).unwrap();

    // 1. 插入 5 个条目,分属 5 个文件
    for i in 0..5 {
        window
            .insert(make_entry_with_file(
                &format!("e-{i}"),
                &format!("file-{i}"),
                100,
            ))
            .await
            .unwrap();
    }
    assert_eq!(window.entry_count().await, 5);

    // 2. 启动 listener
    let handle = window.spawn_mask_listener();

    // 3. OSA 发布 OmniSparseMasksComputed 事件,context_mask 仅含 file-0 和 file-2
    let event = NexusEvent::OmniSparseMasksComputed {
        metadata: EventMetadata::new("osa-coordinator"),
        mask_hash: "mask-001".into(),
        sparsity: 0.6,
        context_mask: vec!["file-0".into(), "file-2".into()],
    };
    bus.publish(event).await.unwrap();

    // 4. 自旋等待 listener 处理(避免 thread::sleep,使用 Instant + yield_now)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if window.entry_count().await == 2 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let count = window.entry_count().await;
            panic!("listener 未在 2s 内应用稀疏化,当前条目数 {count}(期望 2)");
        }
        tokio::task::yield_now().await;
    }

    // 5. 验证仅保留 file-0 和 file-2 的条目
    assert_eq!(window.entry_count().await, 2);
    assert!(window.get("e-0").await.unwrap().is_some());
    assert!(window.get("e-2").await.unwrap().is_some());
    assert!(window.get("e-1").await.unwrap().is_none());
    assert!(window.get("e-3").await.unwrap().is_none());
    assert!(window.get("e-4").await.unwrap().is_none());

    // 验证 mask_hash 与 sparsity 已更新
    assert_eq!(window.last_mask_hash().await, Some("mask-001".into()));
    let sparsity = window.last_sparsity().await.unwrap();
    assert!((sparsity - 0.6).abs() < 1e-6);

    handle.abort();
}

/// SubTask 17.1:验证 listener 应用稀疏化后发布 ContextCompressed 事件
#[tokio::test]
async fn test_osa_hcw_sparsification_publishes_compressed_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let window = HcwWindow::with_default_config(bus.clone()).unwrap();

    // 插入 3 个条目,分属 3 个文件
    for i in 0..3 {
        window
            .insert(make_entry_with_file(
                &format!("e-{i}"),
                &format!("file-{i}"),
                100,
            ))
            .await
            .unwrap();
    }

    // 启动 listener
    let handle = window.spawn_mask_listener();

    // OSA 发布事件,context_mask 仅含 file-0
    let event = NexusEvent::OmniSparseMasksComputed {
        metadata: EventMetadata::new("osa-coordinator"),
        mask_hash: "mask-002".into(),
        sparsity: 0.667,
        context_mask: vec!["file-0".into()],
    };
    bus.publish(event).await.unwrap();

    // 自旋等待 ContextCompressed 事件(listener 应用稀疏化后发布)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut found_compressed = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)).await {
            Ok(NexusEvent::ContextCompressed { .. }) => {
                found_compressed = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => {
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                continue;
            }
        }
    }
    assert!(
        found_compressed,
        "listener 应用稀疏化后应发布 ContextCompressed 事件"
    );

    handle.abort();
}

/// SubTask 17.1:验证空 context_mask 不触发稀疏化
#[tokio::test]
async fn test_osa_hcw_empty_context_mask_no_op() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus.clone()).unwrap();

    window
        .insert(make_entry_with_file("e-1", "file-1", 100))
        .await
        .unwrap();
    window
        .insert(make_entry_with_file("e-2", "file-2", 100))
        .await
        .unwrap();
    assert_eq!(window.entry_count().await, 2);

    let handle = window.spawn_mask_listener();

    // OSA 发布事件,context_mask 为空(不应触发稀疏化)
    let event = NexusEvent::OmniSparseMasksComputed {
        metadata: EventMetadata::new("osa-coordinator"),
        mask_hash: "mask-003".into(),
        sparsity: 0.0,
        context_mask: vec![],
    };
    bus.publish(event).await.unwrap();

    // 等待短暂时间确保 listener 有机会处理
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    while tokio::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }

    // 条目数应不变(空 context_mask 不触发稀疏化)
    assert_eq!(
        window.entry_count().await,
        2,
        "空 context_mask 不应触发稀疏化"
    );

    handle.abort();
}
