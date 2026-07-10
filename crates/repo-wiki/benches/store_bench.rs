//! WikiStore 读写并发基准
//!
//! 对应 Phase III-4:验证"写入时不阻塞读取"的性能收益。
//!
//! # 基准项
//! - `read_only_get_latency`:无写入负载时 `WikiStore::get` 延迟基线
//! - `concurrent_read_during_write`:后台持续写入时 `WikiStore::get` 延迟
//!
//! # 运行
//! ```bash
//! cargo bench -p repo-wiki
//! ```

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use repo_wiki::{WikiEntry, WikiStore};
use tokio::runtime::Runtime;

/// 预填充条目数
///
/// WHY:100 条足够让 SQLite 页缓存生效,同时保持预填充开销可控,
/// 使 get 测量的是稳定状态下的命中延迟而非首次加载抖动。
const PRE_FILL_COUNT: usize = 100;

/// 构建测试条目
fn make_entry(id: u64) -> WikiEntry {
    WikiEntry::new(
        format!("bench-entry-{id}"),
        format!("Benchmark Title {id}"),
        format!(
            "Benchmark content body for entry {id}. \
             Repeated text to add realistic payload for latency measurement."
        ),
        vec!["bench".into(), format!("tag-{}", id % 10)],
        vec![0.01_f32 * (id % 100) as f32; 512],
    )
}

/// 基线:无写入负载时的 get 延迟
///
/// WHY:单独测量读路径,作为"写入时不阻塞读取"改造前的对照基线。
/// 改造后此基线应基本不变,而并发场景的延迟应趋近基线。
fn read_only_latency(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let tmp = tempfile::tempdir().expect("创建临时目录失败");
    let store = WikiStore::open(tmp.path().join("wiki.db").as_path()).expect("打开 WikiStore 失败");

    // 预填充固定数据集,确保每次 get 都命中(不测量未命中路径)
    rt.block_on(async {
        for i in 0..PRE_FILL_COUNT {
            store
                .insert(make_entry(i as u64))
                .await
                .expect("预填充失败");
        }
    });

    let mut idx = 0usize;
    c.bench_function("read_only_get_latency", |b| {
        b.iter(|| {
            rt.block_on(async {
                let id = format!("bench-entry-{}", idx % PRE_FILL_COUNT);
                idx += 1;
                let entry = store.get(black_box(id)).await.expect("get 失败");
                assert!(entry.is_some(), "基线读取必须命中预填充条目");
            });
        });
    });
}

/// 并发场景:后台持续写入时测量 get 延迟
///
/// WHY:通过对比写入负载下的 get 延迟与基线,量化"写入不阻塞读取"改造的收益。
/// 改造前(单 Mutex 连接)读会排队等待写锁;改造后(写线程 + 只读连接池)
/// 读延迟应接近基线。
fn concurrent_read_during_write(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let tmp = tempfile::tempdir().expect("创建临时目录失败");
    let store = WikiStore::open(tmp.path().join("wiki.db").as_path()).expect("打开 WikiStore 失败");
    let store = Arc::new(store);

    // 预填充,保证 get 命中(不测量未命中路径)
    rt.block_on(async {
        for i in 0..PRE_FILL_COUNT {
            store
                .insert(make_entry(i as u64))
                .await
                .expect("预填充失败");
        }
    });

    // 启动后台写入任务,持续插入新条目模拟写负载
    // WHY:WikiStore 是 Clone(Arc 包装),可安全跨任务共享
    let write_store = Arc::clone(&store);
    let write_counter = Arc::new(AtomicU64::new(PRE_FILL_COUNT as u64));
    let writer = rt.spawn(async move {
        loop {
            let id = write_counter.fetch_add(1, Ordering::Relaxed);
            if write_store.insert(make_entry(id)).await.is_err() {
                break;
            }
            // 让出执行权,允许 runtime 调度读任务,
            // 模拟真实并发场景而非单线程死循环独占线程
            tokio::task::yield_now().await;
        }
    });

    let read_store = Arc::clone(&store);
    let mut idx = 0usize;

    let mut group = c.benchmark_group("concurrent_read_during_write");
    // 显式保证样本数 ≥ 100,即使单次迭代较慢也不降级
    group.sample_size(100);
    group.bench_function("get_under_write_load", |b| {
        b.iter(|| {
            rt.block_on(async {
                let id = format!("bench-entry-{}", idx % PRE_FILL_COUNT);
                idx += 1;
                let entry = read_store.get(black_box(id)).await.expect("get 失败");
                assert!(entry.is_some(), "并发读必须命中预填充条目");
            });
        });
    });
    group.finish();

    // 清理后台写入任务,避免进程残留
    writer.abort();
}

criterion_group!(benches, read_only_latency, concurrent_read_during_write);
criterion_main!(benches);
