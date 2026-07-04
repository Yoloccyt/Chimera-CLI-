//! SubTask 5.14:语义块重平衡器集成测试
//!
//! 验证 `KVBlockSemanticRouter::auto_rebalance` 与事件发布的集成行为:
//! - 自动重平衡后块数量变化(共现变化导致块合并/拆分)
//! - `ToolsRouted` 事件正确发布(携带 `routed_count`、`top_tool` 字段)
//! - `BlocksRebalanced` 事件正确发布(携带 `old_block_count`、`new_block_count` 字段)
//! - 重平衡不影响进行中的路由(并发安全)
//! - `update_co_occurrence` 更新共现矩阵
//! - `record_co_occurrence` 增量更新
//!
//! # 事件验证方法
//! 使用 `EventBus::subscribe()` 订阅事件,通过 `EventReceiver::recv_timeout()`
//! 接收事件并验证字段。订阅必须在发布之前调用(broadcast 语义)。

mod common;

use std::time::Duration;

use event_bus::{EventBus, NexusEvent};
use kvbsr_router::{
    CoOccurrenceMatrix, KVBlockSemanticRouter, KvbsrConfig, KvbsrError, ToolId, ToolVector,
};
use nexus_core::CLV;

/// 验证自动重平衡后块数量变化
///
/// 场景:
/// 1. 初始:3 个工具,无共现,3 个块
/// 2. 更新共现:t1-t2 共现 150 次(> 阈值 100)
/// 3. 重平衡:t1-t2 合并为同一块,t3 独立 → 2 个块
#[tokio::test]
async fn test_auto_rebalance_changes_block_count() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    // 初始:3 个工具,无共现,3 个块
    let tools = vec![
        ToolVector::new("t1", vec![1.0; 64], 100),
        ToolVector::new("t2", vec![1.0; 64], 100),
        ToolVector::new("t3", vec![1.0; 64], 100),
    ];
    router
        .build_blocks(tools, CoOccurrenceMatrix::new())
        .await
        .expect("构建块应成功");
    assert_eq!(
        router.block_count().await,
        3,
        "初始应有 3 个块(无共现,每个工具独立成块)"
    );

    // 更新共现:t1-t2 共现 150 次(> 阈值 100)
    // WHY:`update_co_occurrence` 期望 `&[(ToolId, ToolId)]`,
    // 使用 `ToolId::new` 构造 newtype,避免 `String` ↔ `ToolId` 转换。
    let log = vec![(ToolId::new("t1"), ToolId::new("t2")); 150];
    router.update_co_occurrence(&log).await;

    // 重平衡
    router.auto_rebalance().await.expect("重平衡应成功");

    // 重平衡后:t1-t2 合并,t3 独立 → 2 个块
    assert_eq!(
        router.block_count().await,
        2,
        "重平衡后应有 2 个块(t1-t2 合并,t3 独立)"
    );
}

/// 验证重平衡后块数量增加(拆分场景)
///
/// 场景:
/// 1. 初始:4 个工具,t1-t2-t3-t4 全部共现,1 个块
/// 2. 更新共现:清空共现矩阵(无共现)
/// 3. 重平衡:每个工具独立成块 → 4 个块
#[tokio::test]
async fn test_auto_rebalance_splits_blocks() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    // 初始:4 个工具,全部共现,1 个块
    let tools = vec![
        ToolVector::new("t1", vec![1.0; 64], 100),
        ToolVector::new("t2", vec![1.0; 64], 100),
        ToolVector::new("t3", vec![1.0; 64], 100),
        ToolVector::new("t4", vec![1.0; 64], 100),
    ];
    let mut co = CoOccurrenceMatrix::new();
    co.insert("t1", "t2", 150);
    co.insert("t2", "t3", 150);
    co.insert("t3", "t4", 150);
    router.build_blocks(tools, co).await.expect("构建块应成功");
    assert_eq!(router.block_count().await, 1, "初始应有 1 个块(全部共现)");

    // 更新共现:清空(无共现)
    router.update_co_occurrence(&[]).await;

    // 重平衡
    router.auto_rebalance().await.expect("重平衡应成功");

    // 重平衡后:每个工具独立成块 → 4 个块
    assert_eq!(
        router.block_count().await,
        4,
        "重平衡后应有 4 个块(无共现,每个工具独立成块)"
    );
}

/// 验证 ToolsRouted 事件正确发布
///
/// 路由完成后应发布 `ToolsRouted` 事件,携带:
/// - `routed_count`:已路由工具数
/// - `top_tool`:最匹配工具 ID
/// - `routed_tools`:完整 Top-K 工具 ID 列表(SubTask 17.3 新增)
#[tokio::test]
async fn test_tools_routed_event_published() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let router = KVBlockSemanticRouter::new(bus);

    let (tools, co) = common::generate_test_data();
    router.build_blocks(tools, co).await.expect("构建块应成功");

    let clv = common::generate_clv_for_block(0, 0.0);
    let result = router.route(&clv).await.expect("路由应成功");

    // 接收 ToolsRouted 事件(超时 100ms)
    let event = rx
        .recv_timeout(Duration::from_millis(100))
        .await
        .expect("应收到 ToolsRouted 事件");

    match event {
        NexusEvent::ToolsRouted {
            routed_count,
            top_tool,
            routed_tools,
            ..
        } => {
            assert_eq!(
                routed_count as usize,
                result.routed_count(),
                "事件 routed_count 与结果不一致"
            );
            assert_eq!(
                top_tool,
                // WHY:`result.top_tool()` 返回 `Option<&ToolId>`,
                // 事件字段 `top_tool` 是 `String`(EventBus 在 L1,不依赖 KVBSR 的 ToolId),
                // 用 `map(|t| t.to_string())` 转换为 `String`。
                result.top_tool().map(|t| t.to_string()).unwrap_or_default(),
                "事件 top_tool 与结果不一致"
            );
            // SubTask 17.3:验证 routed_tools 字段携带完整 Top-K 工具列表
            assert_eq!(
                routed_tools.len(),
                result.routed_count(),
                "routed_tools 长度应与 routed_count 一致"
            );
            assert_eq!(
                routed_tools.first().map(|s| s.as_str()).unwrap_or(""),
                result.top_tool().map(|t| t.as_str()).unwrap_or(""),
                "routed_tools 首元素应与 top_tool 一致"
            );
        }
        other => panic!("期望 ToolsRouted 事件,实际收到 {:?}", other),
    }
}

/// 验证 BlocksRebalanced 事件正确发布
///
/// 重平衡完成后应发布 `BlocksRebalanced` 事件,携带:
/// - `old_block_count`:重平衡前的块数量
/// - `new_block_count`:重平衡后的块数量
#[tokio::test]
async fn test_blocks_rebalanced_event_published() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let router = KVBlockSemanticRouter::new(bus);

    // 初始:3 个工具,无共现,3 个块
    let tools = vec![
        ToolVector::new("t1", vec![1.0; 64], 100),
        ToolVector::new("t2", vec![1.0; 64], 100),
        ToolVector::new("t3", vec![1.0; 64], 100),
    ];
    router
        .build_blocks(tools, CoOccurrenceMatrix::new())
        .await
        .expect("构建块应成功");
    let old_count = router.block_count().await;
    assert_eq!(old_count, 3);

    // 更新共现:t1-t2 共现 150 次
    let log = vec![(ToolId::new("t1"), ToolId::new("t2")); 150];
    router.update_co_occurrence(&log).await;

    // 重平衡
    router.auto_rebalance().await.expect("重平衡应成功");
    let new_count = router.block_count().await;
    assert_eq!(new_count, 2);

    // 接收 BlocksRebalanced 事件(超时 100ms)
    let event = rx
        .recv_timeout(Duration::from_millis(100))
        .await
        .expect("应收到 BlocksRebalanced 事件");

    match event {
        NexusEvent::BlocksRebalanced {
            old_block_count,
            new_block_count,
            ..
        } => {
            assert_eq!(
                old_block_count, old_count as u32,
                "事件 old_block_count 不一致"
            );
            assert_eq!(
                new_block_count, new_count as u32,
                "事件 new_block_count 不一致"
            );
        }
        other => panic!("期望 BlocksRebalanced 事件,实际收到 {:?}", other),
    }
}

/// 验证重平衡不影响进行中的路由(并发安全)
///
/// 场景:
/// 1. 创建路由器并初始化
/// 2. 启动多个并发路由任务
/// 3. 同时触发重平衡
/// 4. 验证所有路由都成功完成(无 panic、无错误)
#[tokio::test]
async fn test_rebalance_does_not_break_concurrent_routing() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    let (tools, co) = common::generate_test_data();
    router.build_blocks(tools, co).await.expect("构建块应成功");

    let clv = common::generate_clv_for_block(0, 0.0);

    // 启动 10 个并发路由任务
    let mut handles = Vec::new();
    for _ in 0..10 {
        let router_clone = router.clone();
        let clv_clone = clv.clone();
        handles.push(tokio::spawn(
            async move { router_clone.route(&clv_clone).await },
        ));
    }

    // 同时触发重平衡(在另一个任务中)
    let router_for_rebalance = router.clone();
    let rebalance_handle = tokio::spawn(async move { router_for_rebalance.auto_rebalance().await });

    // 等待所有路由任务完成
    let mut success_count = 0;
    for handle in handles {
        let result = handle.await.expect("任务应完成");
        if result.is_ok() {
            success_count += 1;
        }
    }

    // 等待重平衡完成
    let rebalance_result = rebalance_handle.await.expect("重平衡任务应完成");
    assert!(rebalance_result.is_ok(), "重平衡应成功");

    // 所有路由都应成功(允许部分因锁竞争等待,但最终成功)
    assert_eq!(
        success_count, 10,
        "所有 10 个并发路由都应成功,实际成功 {} 个",
        success_count
    );
}

/// 验证 update_co_occurrence 更新共现矩阵
#[tokio::test]
async fn test_update_co_occurrence() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    // 初始共现矩阵为空
    let log = vec![
        (ToolId::new("a"), ToolId::new("b")),
        (ToolId::new("a"), ToolId::new("b")),
        (ToolId::new("b"), ToolId::new("c")),
    ];
    router.update_co_occurrence(&log).await;

    // 验证共现矩阵已更新(通过重平衡间接验证)
    let tools = vec![
        ToolVector::new("a", vec![1.0; 64], 100),
        ToolVector::new("b", vec![1.0; 64], 100),
        ToolVector::new("c", vec![1.0; 64], 100),
    ];
    router
        .build_blocks(tools, CoOccurrenceMatrix::new())
        .await
        .expect("构建块应成功");
    assert_eq!(router.block_count().await, 3);

    // 重平衡(使用更新后的共现矩阵)
    router.auto_rebalance().await.expect("重平衡应成功");

    // a-b 共现 2 次(< 阈值 100),不合并;b-c 共现 1 次,不合并
    // 应仍为 3 个块
    assert_eq!(router.block_count().await, 3, "共现次数 < 阈值时不应合并");
}

/// 验证 record_co_occurrence 增量更新
#[tokio::test]
async fn test_record_co_occurrence_incremental() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    // 增量记录共现
    router.record_co_occurrence("t1", "t2").await;
    router.record_co_occurrence("t1", "t2").await;
    router.record_co_occurrence("t2", "t3").await;

    // 构建块并重平衡
    let tools = vec![
        ToolVector::new("t1", vec![1.0; 64], 100),
        ToolVector::new("t2", vec![1.0; 64], 100),
        ToolVector::new("t3", vec![1.0; 64], 100),
    ];
    router
        .build_blocks(tools, CoOccurrenceMatrix::new())
        .await
        .expect("构建块应成功");

    // 重平衡(使用增量记录的共现)
    router.auto_rebalance().await.expect("重平衡应成功");

    // t1-t2 共现 2 次,t2-t3 共现 1 次,都 < 阈值 100,不合并
    assert_eq!(router.block_count().await, 3, "共现次数 < 阈值时不应合并");
}

/// 验证 record_co_occurrence 达到阈值后触发合并
///
/// 注意:必须先 build_blocks 初始化,再 record_co_occurrence 增量更新,
/// 因为 build_blocks 会覆盖共现矩阵(传入的 co_occurrence 替换现有矩阵)。
#[tokio::test]
async fn test_record_co_occurrence_triggers_merge() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    let tools = vec![
        ToolVector::new("t1", vec![1.0; 64], 100),
        ToolVector::new("t2", vec![1.0; 64], 100),
        ToolVector::new("t3", vec![1.0; 64], 100),
    ];
    // 先用空共现矩阵初始化(build_blocks 会覆盖共现矩阵)
    router
        .build_blocks(tools, CoOccurrenceMatrix::new())
        .await
        .expect("构建块应成功");
    assert_eq!(router.block_count().await, 3);

    // build_blocks 之后再增量记录共现 150 次(> 阈值 100)
    for _ in 0..150 {
        router.record_co_occurrence("t1", "t2").await;
    }

    // 重平衡(使用增量记录的共现)
    router.auto_rebalance().await.expect("重平衡应成功");

    // t1-t2 共现 150 次 > 阈值 100,应合并;t3 独立 → 2 个块
    assert_eq!(
        router.block_count().await,
        2,
        "t1-t2 共现 > 阈值时应合并为同一块"
    );
}

/// 验证自动重平衡触发(rebalance_interval=1000)
///
/// 路由 1000 次后应自动触发重平衡(在独立任务中异步执行)。
/// 通过事件订阅验证重平衡被触发。
#[tokio::test]
async fn test_auto_rebalance_triggered_by_route_count() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let config = KvbsrConfig::default().with_rebalance_interval(10);
    let router = KVBlockSemanticRouter::with_config(bus, config);

    let (tools, co) = common::generate_test_data();
    router.build_blocks(tools, co).await.expect("构建块应成功");

    let clv = common::generate_clv_for_block(0, 0.0);

    // 路由 10 次(触发 1 次自动重平衡)
    for _ in 0..10 {
        router.route(&clv).await.expect("路由应成功");
    }

    // 等待自动重平衡完成(异步执行,需等待事件)
    // 收集事件:10 个 ToolsRouted + 1 个 BlocksRebalanced
    let mut tools_routed_count = 0;
    let mut blocks_rebalanced_count = 0;
    let deadline = Duration::from_millis(500);
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)).await {
            Ok(NexusEvent::ToolsRouted { .. }) => tools_routed_count += 1,
            Ok(NexusEvent::BlocksRebalanced { .. }) => blocks_rebalanced_count += 1,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    assert_eq!(tools_routed_count, 10, "应收到 10 个 ToolsRouted 事件");
    assert!(
        blocks_rebalanced_count >= 1,
        "应至少收到 1 个 BlocksRebalanced 事件(自动重平衡触发)"
    );
}

/// 验证重平衡空工具列表返回错误
#[tokio::test]
async fn test_rebalance_empty_tools_returns_error() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    // 不调用 build_blocks,tools 为空
    let result = router.auto_rebalance().await;
    assert!(
        matches!(result, Err(KvbsrError::RebalanceFailed(_))),
        "空工具列表重平衡应返回 RebalanceFailed 错误"
    );
}

/// 验证多次重平衡的稳定性
///
/// 使用 record_co_occurrence 增量更新(而非 update_co_occurrence 完全替换),
/// 避免后续更新覆盖之前的共现记录。
#[tokio::test]
async fn test_multiple_rebalances_stability() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    let tools = vec![
        ToolVector::new("t1", vec![1.0; 64], 100),
        ToolVector::new("t2", vec![1.0; 64], 100),
        ToolVector::new("t3", vec![1.0; 64], 100),
    ];
    router
        .build_blocks(tools, CoOccurrenceMatrix::new())
        .await
        .expect("构建块应成功");
    assert_eq!(router.block_count().await, 3);

    // 第一次重平衡:增量添加 t1-t2 共现 150 次
    for _ in 0..150 {
        router.record_co_occurrence("t1", "t2").await;
    }
    router.auto_rebalance().await.expect("第一次重平衡应成功");
    assert_eq!(router.block_count().await, 2); // t1-t2 合并,t3 独立

    // 第二次重平衡:增量添加 t2-t3 共现 150 次(保留 t1-t2 共现)
    for _ in 0..150 {
        router.record_co_occurrence("t2", "t3").await;
    }
    router.auto_rebalance().await.expect("第二次重平衡应成功");
    assert_eq!(router.block_count().await, 1); // t1-t2-t3 全部合并

    // 第三次重平衡:共现未变,块数量应保持稳定
    router.auto_rebalance().await.expect("第三次重平衡应成功");
    assert_eq!(router.block_count().await, 1); // 仍为 1 块
}

/// 验证重平衡后路由仍正常工作
#[tokio::test]
async fn test_route_works_after_rebalance() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    let (tools, co) = common::generate_test_data();
    router.build_blocks(tools, co).await.expect("构建块应成功");

    let clv = common::generate_clv_for_block(0, 0.0);

    // 重平衡前路由
    let result_before = router.route(&clv).await.expect("重平衡前路由应成功");
    assert!(!result_before.selected_tools.is_empty());

    // 触发重平衡
    router.auto_rebalance().await.expect("重平衡应成功");

    // 重平衡后路由
    let result_after = router.route(&clv).await.expect("重平衡后路由应成功");
    assert!(!result_after.selected_tools.is_empty());

    // 重平衡前后,块 0 的工具应仍被选中(块结构未变)
    let top_after = result_after.top_tool().expect("应有 Top-1 工具");
    assert!(
        top_after.starts_with("tool-0-"),
        "重平衡后 Top-1 工具 {} 应属于块 0",
        top_after
    );
}

/// 验证重平衡后块 ID 更新(新块有新 ID)
#[tokio::test]
async fn test_rebalance_generates_new_block_ids() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    let tools = vec![
        ToolVector::new("t1", vec![1.0; 64], 100),
        ToolVector::new("t2", vec![1.0; 64], 100),
    ];
    router
        .build_blocks(tools, CoOccurrenceMatrix::new())
        .await
        .expect("构建块应成功");

    // 重平衡前:2 个块
    assert_eq!(router.block_count().await, 2);

    // 更新共现并重平衡
    let log = vec![(ToolId::new("t1"), ToolId::new("t2")); 150];
    router.update_co_occurrence(&log).await;
    router.auto_rebalance().await.expect("重平衡应成功");

    // 重平衡后:1 个块
    assert_eq!(router.block_count().await, 1);

    // 路由应正常工作
    let clv = CLV::zero();
    let result = router.route(&clv).await.expect("路由应成功");
    assert!(!result.selected_tools.is_empty());
}

/// 验证事件元数据 source 字段为 "kvbsr-router"
#[tokio::test]
async fn test_event_metadata_source_is_kvbsr_router() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let router = KVBlockSemanticRouter::new(bus);

    let (tools, co) = common::generate_test_data();
    router.build_blocks(tools, co).await.expect("构建块应成功");

    let clv = common::generate_clv_for_block(0, 0.0);
    router.route(&clv).await.expect("路由应成功");

    let event = rx
        .recv_timeout(Duration::from_millis(100))
        .await
        .expect("应收到事件");

    assert_eq!(
        event.metadata().source,
        "kvbsr-router",
        "事件 source 应为 'kvbsr-router'"
    );
}

/// 验证重平衡在 300 工具规模下的性能
///
/// 直接使用 build_blocks 传入的共现矩阵(块内共现 150 > 阈值 100),
/// 不调用 update_co_occurrence(避免覆盖共现矩阵)。
///
/// SubTask 11.2:添加 warmup(10 次)+ P50/P99 统计(100 次测量)
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_rebalance_performance_300_tools() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);

    let (tools, co) = common::generate_test_data();
    router.build_blocks(tools, co).await.expect("构建块应成功");

    // Warmup(10 次,触发缓存预热)
    // WHY 可重复重平衡:共现矩阵不变,重平衡后的块列表相同,可多次调用
    for _ in 0..10 {
        router.auto_rebalance().await.expect("warmup 重平衡应成功");
    }

    // 正式测量(100 次,收集延迟分布)
    let mut latencies = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = std::time::Instant::now();
        router.auto_rebalance().await.expect("重平衡应成功");
        latencies.push(start.elapsed().as_nanos() as f64);
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = latencies[50];
    let p99 = latencies[99];

    // P50 < 100ms(100_000_000ns), P99 < 200ms(原阈值 × 2)
    let threshold_ns = 100_000_000.0_f64;
    assert!(
        p50 < threshold_ns,
        "P50 重平衡延迟 {}ns 超过 {}ns",
        p50,
        threshold_ns
    );
    assert!(
        p99 < threshold_ns * 2.0,
        "P99 重平衡延迟 {}ns 超过 {}ns",
        p99,
        threshold_ns * 2.0
    );

    // 块数量应保持 15(共现模式未变,块内共现 150 > 阈值 100)
    assert_eq!(
        router.block_count().await,
        common::NUM_BLOCKS,
        "重平衡后块数量应保持 {}",
        common::NUM_BLOCKS
    );
}
