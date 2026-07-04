//! HCW × SCC 跨周集成测试
//!
//! 验证 HCW(L2 Memory)与 SCC(L3 Storage)的协作场景:
//! - HCW 压缩上下文后,SCC 缓存压缩结果供 PVL Producer/Verifier 共享
//! - 稀疏化 + 缓存协作
//! - 窗口切换与缓存独立生命周期
//!
//! # 类型映射
//! HCW `ContextEntry` → SCC `ContextEntry`:
//! - `id: String` → `ContextId::new(id)`
//! - `content: Arc<str>` → `content: Arc<str>`(直接 Clone)
//!
//! # 架构约束
//! - HCW(L2) → SCC(L3):向下依赖,合法
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 测试使用 `#[tokio::test]`,HCW 方法 async / SCC 方法 sync

use event_bus::EventBus;
use hcw_window::{ContextEntry as HcwContextEntry, HcwWindow, WindowTier};
use scc_cache::{ContextEntry as SccContextEntry, ContextId, SccCache, SccConfig};

// ============================================================
// 辅助函数
// ============================================================

/// 创建 HCW 条目(带 file_id)
fn hcw_entry(id: &str, file_id: &str, token_size: usize) -> HcwContextEntry {
    HcwContextEntry::new(id, file_id, format!("content-{id}"), token_size)
}

/// 将 HCW 条目转换为 SCC 条目
fn to_scc_entry(hcw: &HcwContextEntry) -> SccContextEntry {
    SccContextEntry::new(hcw.id.as_str(), hcw.content.clone())
}

// ============================================================
// 测试 1:完整 HCW → SCC 协作流程
// ============================================================

/// 验证 HCW 压缩上下文后 SCC 缓存压缩结果的完整协作流程:
/// 1. 创建 EventBus 和 HcwWindow(默认配置)
/// 2. 插入 5 个 ContextEntry(不同 file_id)
/// 3. 调用 select_window(0.8) 触发压缩(complexity ≥ 0.75 选 L3)
/// 4. 创建 SccCache(默认配置)
/// 5. 将 HCW 压缩后的条目(通过 get_arc 获取)转换为 SCC 的 ContextEntry 并插入
/// 6. 验证 SCC 命中率(第二次 get 应命中)
#[tokio::test]
async fn test_hcw_compress_scc_cache_collaboration() {
    // 1. 创建 EventBus 和 HcwWindow
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus.clone()).unwrap();
    assert_eq!(window.current_tier().await, WindowTier::L0);

    // 2. 插入 5 个 ContextEntry(不同 file_id)
    let ids = ["e-1", "e-2", "e-3", "e-4", "e-5"];
    for (i, id) in ids.iter().enumerate() {
        let entry = hcw_entry(id, &format!("file-{}", i + 1), 1000);
        window.insert(entry).await.unwrap();
    }
    assert_eq!(window.entry_count().await, 5);

    // 3. 调用 select_window(0.8) → complexity ≥ 0.75 选 L3
    let tier = window.select_window(0.8).await.unwrap();
    assert_eq!(tier, WindowTier::L3, "complexity 0.8 应选择 L3");

    // 4. 创建 SccCache(默认配置)
    let cache = SccCache::new(SccConfig::default(), bus.clone());

    // 5. 将 HCW 压缩后的条目转换为 SCC 条目并插入
    for id in &ids {
        let hcw_arc = window
            .get_arc(id)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("条目 {id} 应在 HCW 中"));
        let scc_entry = to_scc_entry(&hcw_arc);
        cache.insert(scc_entry);
    }

    // 6. 验证 SCC 命中率:所有条目二次 get 应命中
    for id in &ids {
        let ctx_id = ContextId::new(*id);
        let first = cache.get_or_prefetch(&ctx_id);
        assert!(
            first.is_some(),
            "SCC 首次 get_or_prefetch 应命中已插入的条目 {id}"
        );

        // 第二次 get 应命中(缓存验证)
        let second = cache.get_or_prefetch(&ctx_id);
        assert!(
            second.is_some(),
            "SCC 第二次 get_or_prefetch 应依然命中 {id}"
        );
    }

    // 验证缓存统计:5 次插入 + 10 次 get(5 首次 + 5 二次)
    // 注意:get_or_prefetch 命中时才递增 hit_count,首次调用时条目已插入所以命中
    assert_eq!(cache.len(), 5, "SCC 应缓存 5 个条目");
    assert!(cache.hit_count() >= 10, "至少 10 次命中(5 条目 × 2 次访问)");
}

// ============================================================
// 测试 2:稀疏化 + 缓存协作
// ============================================================

/// 验证 HCW 稀疏化后 SCC 缓存不同 file_id 条目的协作:
/// 1. 创建 EventBus、HcwWindow、SccCache
/// 2. 插入多个条目,部分 file_id 匹配
/// 3. 应用 OSA 稀疏掩码(仅保留特定 file_id 的条目)
/// 4. 将稀疏化后的条目缓存到 SCC
/// 5. 验证 SCC 缓存不同 file_id 的条目
#[tokio::test]
async fn test_hcw_sparse_mask_scc_cache() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus.clone()).unwrap();
    let cache = SccCache::new(SccConfig::default(), bus.clone());

    // 2. 插入多个条目,分属不同 file_id
    // file-a: e-a1, e-a2
    // file-b: e-b1
    // file-c: e-c1, e-c2
    let entries = vec![
        ("e-a1", "file-a", 500),
        ("e-a2", "file-a", 500),
        ("e-b1", "file-b", 500),
        ("e-c1", "file-c", 500),
        ("e-c2", "file-c", 500),
    ];
    for (id, file_id, token_size) in &entries {
        window
            .insert(hcw_entry(id, file_id, *token_size))
            .await
            .unwrap();
    }
    assert_eq!(window.entry_count().await, 5);

    // 3. 应用稀疏掩码:仅保留 file-a 和 file-c 的条目(移除 file-b)
    let report = window
        .apply_sparse_mask(vec!["file-a".into(), "file-c".into()])
        .await
        .unwrap();
    assert_eq!(report.dropped_count, 1, "应移除 file-b 的 1 个条目");
    assert_eq!(
        report.retained_count, 4,
        "应保留 file-a(2) + file-c(2) = 4 个条目"
    );
    assert_eq!(window.entry_count().await, 4);

    // 4. 将稀疏化后的条目缓存到 SCC
    let retained_ids = ["e-a1", "e-a2", "e-c1", "e-c2"];
    for id in &retained_ids {
        let hcw_arc = window
            .get_arc(id)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("稀疏化后条目 {id} 应保留"));
        cache.insert(to_scc_entry(&hcw_arc));
    }

    // 5. 验证 SCC 缓存不同 file_id 的条目
    // file-a 的条目
    for id in &["e-a1", "e-a2"] {
        let ctx_id = ContextId::new(*id);
        let entry = cache.get_or_prefetch(&ctx_id);
        assert!(entry.is_some(), "SCC 应缓存 file-a 的条目 {id}");
    }
    // file-c 的条目
    for id in &["e-c1", "e-c2"] {
        let ctx_id = ContextId::new(*id);
        let entry = cache.get_or_prefetch(&ctx_id);
        assert!(entry.is_some(), "SCC 应缓存 file-c 的条目 {id}");
    }
    // file-b 的条目不应在 SCC 中(未被缓存)
    let ctx_b1 = ContextId::new("e-b1");
    assert!(
        !cache.contains(&ctx_b1),
        "SCC 不应包含被稀疏化移除的条目 e-b1"
    );

    assert_eq!(cache.len(), 4, "SCC 应缓存 4 个条目");
}

// ============================================================
// 测试 3:窗口切换 + 缓存失效(独立生命周期)
// ============================================================

/// 验证 HCW 窗口切换不影响 SCC 缓存(独立生命周期):
/// 1. 创建 EventBus、HcwWindow、SccCache
/// 2. 插入条目并缓存到 SCC
/// 3. 切换 HCW 窗口层级(L0 → L3 → L0)
/// 4. 验证 SCC 缓存不受影响(条目仍在、命中率不变)
#[tokio::test]
async fn test_hcw_window_switch_scc_invalidation() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus.clone()).unwrap();
    let cache = SccCache::new(SccConfig::default(), bus.clone());

    // 2. 插入条目并缓存到 SCC
    let ids = ["s-1", "s-2", "s-3"];
    for id in &ids {
        let entry = hcw_entry(id, &format!("file-{id}"), 500);
        window.insert(entry).await.unwrap();
    }
    assert_eq!(window.entry_count().await, 3);
    assert_eq!(window.current_tier().await, WindowTier::L0);

    // 缓存到 SCC
    for id in &ids {
        let hcw_arc = window.get_arc(id).await.unwrap().unwrap();
        cache.insert(to_scc_entry(&hcw_arc));
    }
    assert_eq!(cache.len(), 3, "SCC 应缓存 3 个条目");

    // 验证初始命中
    for id in &ids {
        let ctx_id = ContextId::new(*id);
        assert!(cache.get_or_prefetch(&ctx_id).is_some());
    }
    let hits_before = cache.hit_count();

    // 3. 切换 HCW 窗口层级:L0 → L3(升级)
    let tier = window.select_window(0.9).await.unwrap();
    assert_eq!(tier, WindowTier::L3, "升级到 L3");
    assert_eq!(window.current_tier().await, WindowTier::L3);

    // 验证 SCC 缓存不受影响
    for id in &ids {
        let ctx_id = ContextId::new(*id);
        assert!(
            cache.contains(&ctx_id),
            "HCW 升级到 L3 后 SCC 仍应缓存 {id}"
        );
    }
    assert_eq!(cache.len(), 3, "HCW 升级后 SCC 条目数不变");

    // 再切换:L3 → L0(降级)
    let tier = window.select_window(0.1).await.unwrap();
    assert_eq!(tier, WindowTier::L0, "降级到 L0");
    assert_eq!(window.current_tier().await, WindowTier::L0);

    // 验证 SCC 缓存仍不受影响
    for id in &ids {
        let ctx_id = ContextId::new(*id);
        let entry = cache.get_or_prefetch(&ctx_id);
        assert!(entry.is_some(), "HCW 降级到 L0 后 SCC 仍应命中 {id}");
    }
    assert_eq!(cache.len(), 3, "HCW 降级后 SCC 条目数不变");

    // 验证 SCC 命中数持续增长(独立生命周期:HCW 切换不重置 SCC 统计)
    assert!(
        cache.hit_count() > hits_before,
        "SCC 命中数应持续增长,不受 HCW 窗口切换影响"
    );
}
