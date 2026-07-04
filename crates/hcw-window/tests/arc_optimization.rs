//! M-01/M-02 修复验证:Arc 优化使 get_arc 真正零拷贝
//!
//! 对应问题:
//! - M-01: HCW get_arc() 内部仍 entry.clone()(先深拷贝再包 Arc,等价于深拷贝)
//! - M-02: HCW get() 深拷贝 ContextEntry(含 content String)
//!
//! 修复策略:entries 改为 `Vec<Arc<ContextEntry>>`,
//! get_arc 返回 `Arc::clone(&entries[idx])`(引用计数,零拷贝),
//! get 保持返回 ContextEntry(API 兼容,内部 clone Arc 内部值),
//! 新增 get_ref 返回 `&Arc<ContextEntry>`(完全零拷贝引用访问)。

use std::sync::Arc;

use event_bus::EventBus;
use hcw_window::{ContextEntry, HcwState, HcwWindow, WindowTier};

fn make_entry(id: &str, token_size: usize) -> ContextEntry {
    ContextEntry::new(
        id,
        format!("file-{id}"),
        format!("content-{id}"),
        token_size,
    )
}

// ============================================================
// M-01 修复验证:get_arc 真正零拷贝
// ============================================================

/// M-01:get_arc 返回的 Arc 与内部存储共享同一引用(Arc::ptr_eq)
///
/// WHY:原实现 `Arc::new(entry.clone())` 先深拷贝再包 Arc,
/// 返回的 Arc 与内部 entries 中的 Arc 不是同一引用。
/// 修复后 entries 改为 `Vec<Arc<ContextEntry>>`,
/// get_arc 返回 `Arc::clone(&entries[idx])`,Arc::ptr_eq 应为 true。
#[tokio::test]
async fn test_get_arc_shares_internal_arc() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.insert(make_entry("e-1", 100)).await.unwrap();

    let arc1 = window.get_arc("e-1").await.unwrap().unwrap();
    let arc2 = window.get_arc("e-1").await.unwrap().unwrap();

    assert!(
        Arc::ptr_eq(&arc1, &arc2),
        "get_arc 返回的 Arc 应与内部存储共享同一引用(Arc::ptr_eq 为 true)"
    );
}

/// M-01:多次 get_arc 调用不深拷贝 content(通过 Arc::ptr_eq 验证)
///
/// WHY:原实现每次 get_arc 都 `Arc::new(entry.clone())`,content String 被深拷贝。
/// 修复后 Arc::clone 仅增加引用计数,content 共享同一分配。
#[tokio::test]
async fn test_get_arc_no_deep_copy_on_large_content() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();

    // 插入大 content 条目(100KB),放大深拷贝开销
    let large_content = "x".repeat(100_000);
    let entry = ContextEntry::new("e-big", "file-1", large_content, 1000);
    window.insert(entry).await.unwrap();

    let arc1 = window.get_arc("e-big").await.unwrap().unwrap();
    let arc2 = window.get_arc("e-big").await.unwrap().unwrap();
    let arc3 = window.get_arc("e-big").await.unwrap().unwrap();

    assert!(Arc::ptr_eq(&arc1, &arc2), "arc1 与 arc2 应共享同一 Arc");
    assert!(Arc::ptr_eq(&arc2, &arc3), "arc2 与 arc3 应共享同一 Arc");
}

// ============================================================
// M-02 修复验证:get 保持 API 兼容性
// ============================================================

/// M-02:get 返回正确的值(API 兼容性,签名不变)
#[tokio::test]
async fn test_get_returns_correct_value() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.insert(make_entry("e-1", 100)).await.unwrap();

    let entry = window.get("e-1").await.unwrap().unwrap();
    assert_eq!(entry.id, "e-1");
    assert_eq!(entry.token_size, 100);
    // get 应递增访问次数(LRU 语义保持)
    assert_eq!(entry.access_count, 1);
}

/// M-02:get 不存在条目返回 None(API 兼容性)
#[tokio::test]
async fn test_get_nonexistent_returns_none() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    let result = window.get("nonexistent").await.unwrap();
    assert!(result.is_none());
}

// ============================================================
// get_ref 新增验证:HcwState 与 HcwWindow 双层提供
// ============================================================

/// HcwState::get_ref 返回 `&Arc<ContextEntry>`(零拷贝引用访问)
#[test]
fn test_hcw_state_get_ref_returns_arc_reference() {
    let mut state = HcwState::new(WindowTier::L0);
    state.push_entry(ContextEntry::new("e-1", "file-1", "content", 100));

    let arc_ref = state.get_ref("e-1").expect("应找到 e-1");
    let arc_clone = Arc::clone(arc_ref);
    assert_eq!(arc_clone.id, "e-1");
    assert_eq!(arc_clone.token_size, 100);

    // 验证 get_ref 返回的 &Arc 与内部存储共享同一 Arc
    let arc_ref2 = state.get_ref("e-1").unwrap();
    assert!(
        Arc::ptr_eq(arc_ref, arc_ref2),
        "get_ref 返回的 &Arc 应指向内部存储的同一 Arc"
    );
}

/// HcwState::get_ref 不存在条目返回 None
#[test]
fn test_hcw_state_get_ref_nonexistent_returns_none() {
    let state = HcwState::new(WindowTier::L0);
    assert!(state.get_ref("nonexistent").is_none());
}

/// HcwWindow::get_ref 返回 Arc<ContextEntry>(引用语义,零拷贝)
#[tokio::test]
async fn test_window_get_ref_returns_arc() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.insert(make_entry("e-1", 100)).await.unwrap();

    let arc = window.get_ref("e-1").await.unwrap().unwrap();
    assert_eq!(arc.id, "e-1");
    assert_eq!(arc.token_size, 100);
}

/// HcwWindow::get_ref 与 get_arc 返回共享同一 Arc
#[tokio::test]
async fn test_window_get_ref_shares_arc_with_get_arc() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();
    window.insert(make_entry("e-1", 100)).await.unwrap();

    let arc_via_ref = window.get_ref("e-1").await.unwrap().unwrap();
    let arc_via_get_arc = window.get_arc("e-1").await.unwrap().unwrap();

    assert!(
        Arc::ptr_eq(&arc_via_ref, &arc_via_get_arc),
        "get_ref 与 get_arc 应共享内部同一 Arc"
    );
}

// ============================================================
// API 兼容性回归:其他方法不受 Arc 改造影响
// ============================================================

/// push_entry + get + remove 全链路(API 兼容性回归)
#[tokio::test]
async fn test_push_get_remove_roundtrip() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();

    window.insert(make_entry("e-1", 100)).await.unwrap();
    window.insert(make_entry("e-2", 200)).await.unwrap();

    assert_eq!(window.entry_count().await, 2);

    let e1 = window.get("e-1").await.unwrap().unwrap();
    assert_eq!(e1.id, "e-1");

    let removed = window.remove("e-1").await.unwrap().unwrap();
    assert_eq!(removed.id, "e-1");
    assert_eq!(window.entry_count().await, 1);
}

/// 压缩路径仍正确工作(Arc 改造不影响 compress/select_window)
#[tokio::test]
async fn test_compression_still_works() {
    let bus = EventBus::new();
    let window = HcwWindow::with_default_config(bus).unwrap();

    // 插入超过 L0(4K)的条目触发升级
    for i in 0..10 {
        window
            .insert(make_entry(&format!("e-{i}"), 1000))
            .await
            .unwrap();
    }

    // 应升级到 L1 或更高
    assert!(window.current_tier().await >= WindowTier::L1);
    assert_eq!(window.entry_count().await, 10);

    // 降级触发压缩
    let tier = window.select_window(0.1).await.unwrap();
    assert_eq!(tier, WindowTier::L0);
    assert!(window.current_size().await <= 4096);
}
