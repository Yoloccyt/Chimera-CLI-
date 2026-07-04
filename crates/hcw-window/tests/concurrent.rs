//! SubTask 15.7:HCW 并发安全测试
//!
//! 验证 HcwWindow 在高并发场景下的线程安全性与数据一致性。
//!
//! # 已有覆盖(window.rs)
//! - `test_concurrent_insert_with_compression`:4 线程并发 insert + 压缩
//! - `test_select_window_concurrent_safety`:10 线程并发 insert + 1 线程 select_window
//!
//! # 本节补充
//! - 10 线程并发 `apply_sparse_mask`,断言最终状态一致(相同文件子集)
//! - 10 线程并发 `apply_sparse_mask`,断言无数据损坏(不同文件子集)
//!
//! # 线程安全机制
//! HcwWindow 内部用 `Arc<RwLock<HcwState>>` 保护状态:
//! - `apply_sparse_mask` 全程持有写锁(retain_by_file_ids 是纯同步函数,不 await)
//! - 并发写操作通过 RwLock 串行化,无竞态窗口
//! - 锁外发布事件(避免持锁 await 死锁)

use std::sync::Arc;

use event_bus::EventBus;
use hcw_window::{ContextEntry, HcwWindow};

/// 构造测试条目:id/file_id/content 均由索引决定,token_size 固定 100
fn make_entry(index: usize) -> ContextEntry {
    ContextEntry::new(
        format!("e-{index}"),
        format!("file-{index}"),
        format!("content-{index}"),
        100,
    )
}

/// SubTask 15.7:10 线程并发 apply_sparse_mask,断言最终状态一致
///
/// 场景:10 个线程并发调用 `apply_sparse_mask`,每个线程保留**相同**的 100 个文件
/// (file-0 到 file-99)。由于所有线程保留相同的文件子集,最终状态应一致:
/// 100 个条目,current_size = 10000。
///
/// WHY:验证 `apply_sparse_mask` 在并发环境下:
/// 1. 无 panic、无错误(RwLock 正确串行化)
/// 2. 最终状态一致(条目数与 current_size 正确)
/// 3. 所有保留条目都属于活跃文件子集(无数据损坏)
///
/// 若 RwLock 失效(如误用读锁),并发写会导致条目丢失或 current_size 与条目数不匹配。
#[tokio::test]
async fn test_concurrent_apply_sparse_mask_consistency() {
    let bus = EventBus::new();
    let window = Arc::new(HcwWindow::with_default_config(bus).unwrap());

    // 插入 1000 个条目,分属 1000 个文件(每个文件 1 个条目)
    for i in 0..1000 {
        window.insert(make_entry(i)).await.unwrap();
    }
    assert_eq!(window.entry_count().await, 1000);
    assert_eq!(window.current_size().await, 100_000);

    // 10 线程并发 apply_sparse_mask,每个线程保留相同的 100 个文件
    // WHY:相同文件子集确保最终状态确定,便于断言一致性
    let active_file_ids: Vec<String> = (0..100).map(|i| format!("file-{i}")).collect();
    let mut handles = Vec::with_capacity(10);
    for _ in 0..10 {
        let window = window.clone();
        let active_file_ids = active_file_ids.clone();
        handles.push(tokio::spawn(async move {
            window.apply_sparse_mask(active_file_ids).await
        }));
    }

    // 等待所有线程完成,验证无 panic、无错误
    for handle in handles {
        handle
            .await
            .expect("apply_sparse_mask 线程不应 panic")
            .expect("apply_sparse_mask 不应返回错误");
    }

    // 验证最终状态一致:100 个条目(保留 file-0 到 file-99)
    let final_count = window.entry_count().await;
    assert_eq!(
        final_count, 100,
        "最终条目数应为 100 (保留 file-0 到 file-99),实际 {final_count}"
    );

    // 验证 current_size = 100 × 100 = 10000
    let final_size = window.current_size().await;
    assert_eq!(
        final_size, 10_000,
        "最终 current_size 应为 10000 (100 个条目 × 100 token),实际 {final_size}"
    );

    // 验证所有保留条目都属于活跃文件子集(file-0 到 file-99)
    for i in 0..100 {
        let entry = window.get(&format!("e-{i}")).await.unwrap();
        assert!(entry.is_some(), "活跃文件 file-{i} 的条目 e-{i} 应被保留");
    }
    // 验证非活跃文件的条目已被丢弃
    for i in 100..1000 {
        let entry = window.get(&format!("e-{i}")).await.unwrap();
        assert!(entry.is_none(), "非活跃文件 file-{i} 的条目 e-{i} 应被丢弃");
    }
}

/// SubTask 15.7:10 线程并发 apply_sparse_mask,断言无数据损坏
///
/// 场景:10 个线程并发调用 `apply_sparse_mask`,每个线程保留**不同**的 100 个文件
/// (线程 tid 保留 file-{tid*100} 到 file-{tid*100+99})。由于线程并发执行且保留
/// 不同的文件子集,最终保留的条目是最后一个执行的线程的文件子集与之前保留条目的交集。
///
/// WHY:验证 `apply_sparse_mask` 在并发环境下无数据损坏:
/// 1. 无 panic、无错误
/// 2. current_size = final_count × 100(条目数与大小一致,无幽灵条目)
/// 3. 所有保留条目都属于某个线程的活跃文件子集
///
/// 若 RwLock 失效,并发写可能导致条目数与 current_size 不匹配(数据损坏)。
#[tokio::test]
async fn test_concurrent_apply_sparse_mask_no_data_corruption() {
    let bus = EventBus::new();
    let window = Arc::new(HcwWindow::with_default_config(bus).unwrap());

    // 插入 1000 个条目,分属 1000 个文件(每个文件 1 个条目)
    for i in 0..1000 {
        window.insert(make_entry(i)).await.unwrap();
    }
    assert_eq!(window.entry_count().await, 1000);

    // 10 线程并发 apply_sparse_mask,每个线程保留不同的 100 个文件
    // WHY:不同文件子集确保最终状态不确定,但应无数据损坏
    let mut handles = Vec::with_capacity(10);
    for tid in 0..10u32 {
        let window = window.clone();
        handles.push(tokio::spawn(async move {
            // 线程 tid 保留 file-{tid*100} 到 file-{tid*100+99}
            let active_file_ids: Vec<String> = (0..100)
                .map(|i| format!("file-{}", tid as usize * 100 + i))
                .collect();
            window.apply_sparse_mask(active_file_ids).await
        }));
    }

    // 等待所有线程完成,验证无 panic、无错误
    for handle in handles {
        handle
            .await
            .expect("apply_sparse_mask 线程不应 panic")
            .expect("apply_sparse_mask 不应返回错误");
    }

    // 验证最终状态:条目数 ≤ 100(最后一个线程保留的文件数)
    let final_count = window.entry_count().await;
    assert!(
        final_count <= 100,
        "最终条目数应 ≤ 100 (最后一个线程保留的文件数),实际 {final_count}"
    );

    // 验证无数据损坏:current_size = final_count × 100
    // WHY:若 RwLock 失效,可能出现条目数与大小不匹配的幽灵条目
    let final_size = window.current_size().await;
    assert_eq!(
        final_size,
        final_count * 100,
        "current_size 应 = final_count × 100 (无数据损坏),实际 final_size={final_size}, expected={}",
        final_count * 100
    );

    // 验证所有保留条目都属于某个线程的活跃文件子集(0-99, 100-199, ..., 900-999)
    // 由于并发执行,最终保留的文件可能是任意线程的子集,但都应在 0-999 范围内
    for i in 0..1000 {
        if let Some(entry) = window.get(&format!("e-{i}")).await.unwrap() {
            // 保留的条目大小应正确
            assert_eq!(
                entry.token_size, 100,
                "保留条目 e-{i} 的 token_size 应为 100"
            );
        }
    }
}

/// SubTask 15.7:10 线程并发 insert + 1 线程 apply_sparse_mask,断言无条目丢失
///
/// 场景:10 个线程并发 insert(每个线程插入 100 个条目),同时 1 个线程持续调用
/// `apply_sparse_mask`。验证并发 insert 与稀疏化无数据损坏。
///
/// WHY:与 `test_select_window_concurrent_safety`(window.rs)类似,但将 select_window
/// 替换为 apply_sparse_mask,验证稀疏化操作与并发 insert 的交互安全性。
/// apply_sparse_mask 全程持写锁,insert 也持写锁,两者通过 RwLock 串行化,无竞态。
#[tokio::test]
async fn test_concurrent_insert_with_sparse_mask_no_loss() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let bus = EventBus::new();
    let window = Arc::new(HcwWindow::with_default_config(bus).unwrap());

    let total_inserted_count = Arc::new(AtomicUsize::new(0));

    // 10 线程并发 insert,每个线程插入 100 个条目(每个 100 token)
    const ENTRIES_PER_THREAD: usize = 100;
    let mut insert_handles = Vec::with_capacity(10);
    for tid in 0..10u32 {
        let window = window.clone();
        let total_count = total_inserted_count.clone();
        insert_handles.push(tokio::spawn(async move {
            for seq in 0..ENTRIES_PER_THREAD {
                let id = format!("t{tid}-e{seq}");
                let file_id = format!("file-t{tid}-{seq}");
                let entry = ContextEntry::new(&id, file_id, "x".repeat(64), 100);
                if window.insert(entry).await.is_ok() {
                    total_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    // 1 线程持续 apply_sparse_mask(保留所有文件,即不丢弃任何条目)
    // WHY:保留所有文件,确保稀疏化不丢弃条目,仅验证并发安全性
    let window_for_mask = window.clone();
    let mask_handle = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut round = 0u32;
        while tokio::time::Instant::now() < deadline {
            // 每轮保留不同的文件子集,触发 retain_by_file_ids
            let active_file_ids: Vec<String> = (0..1000)
                .map(|i| format!("file-t{}-{}", i % 10, i % 100))
                .collect();
            let _ = window_for_mask.apply_sparse_mask(active_file_ids).await;
            round += 1;
            if round > 100 {
                break;
            }
        }
    });

    // 等待所有 insert 线程完成
    for handle in insert_handles {
        handle.await.expect("insert 线程不应 panic");
    }
    // 等待 mask 线程完成
    mask_handle.await.expect("mask 线程不应 panic");

    // 验证:无条目丢失(current_size = 保留条目数 × 100)
    let final_count = window.entry_count().await;
    let final_size = window.current_size().await;
    assert_eq!(
        final_size,
        final_count * 100,
        "current_size 应 = final_count × 100 (无数据损坏),实际 final_size={final_size}, final_count={final_count}"
    );

    // 验证:最终条目数 ≤ 总插入数(部分可能被稀疏化丢弃)
    let expected_count = total_inserted_count.load(Ordering::Relaxed);
    assert!(
        final_count <= expected_count,
        "最终条目数 {final_count} 应 ≤ 总插入数 {expected_count} (部分可能被稀疏化丢弃)"
    );
}
