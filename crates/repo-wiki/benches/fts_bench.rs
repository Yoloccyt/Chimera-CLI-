//! FTS5 vs LIKE 全文检索性能基准
//!
//! 对应 Task 2 / N15: repo-wiki FTS5 全文索引
//!
//! # 基准项
//! - `fts5_match`:1000 文档规模下 FTS5 MATCH 查询延迟(O(log n))
//! - `like_scan`:1000 文档规模下 LIKE 全表扫描延迟(O(n))
//!
//! # 对比意义
//! 在 1000+ 文档规模下,FTS5 倒排索引显著优于 LIKE 全表扫描。本 bench 通过
//! `WikiConfig.fts_enabled` 切换引擎,端到端测量 `WikiStore::search_fulltext`
//! 的延迟差异(含 spawn_blocking + 连接池开销,反映真实使用场景)。
//!
//! # WHY 稀有词查询
//! 每个文档含唯一关键词 `uniq-{i}`,搜索 `uniq-500` 只匹配 entry 500。
//! LIKE 必须扫描全表 1000 行才能定位 1 条,FTS5 通过倒排索引直接定位,
//! 索引优势在小结果集场景最明显。
//!
//! # 运行
//! ```bash
//! cargo bench -p repo-wiki --bench fts_bench
//! ```

#![forbid(unsafe_code)]

use std::path::Path;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use repo_wiki::{WikiConfig, WikiEntry, WikiStore};
use tokio::runtime::Runtime;

/// 预填充文档数 — 模拟大规模文档库(1000+)
const DOC_COUNT: usize = 1000;

/// 构建测试条目 — 每个文档含唯一关键词 `uniq-{i}`,便于精确匹配单条
fn make_entry(i: usize) -> WikiEntry {
    WikiEntry::new(
        format!("bench-doc-{i}"),
        format!("Title {i} uniq-{i}"),
        format!(
            "Content body for entry {i}. Contains unique token uniq-{i} and \
             some filler text to add realistic payload for full-text search \
             benchmarking at scale."
        ),
        vec![format!("tag-{}", i % 10)],
        vec![0.0; 512],
    )
}

/// 构建 WikiStore(指定 FTS5 启用状态)
fn setup_store(db_path: &Path, fts_enabled: bool) -> WikiStore {
    let config = WikiConfig {
        db_path: db_path.to_path_buf(),
        vector_dim: 512,
        wal_enabled: true,
        read_pool_size: 2,
        fts_enabled,
    };
    WikiStore::open_with_config(config).expect("打开 WikiStore 失败")
}

/// 预填充 1000 文档到 store(setup 阶段,不计入 bench 时间)
fn prefill(rt: &Runtime, store: &WikiStore) {
    rt.block_on(async {
        for i in 0..DOC_COUNT {
            store.insert(make_entry(i)).await.expect("预填充失败");
        }
    });
}

/// FTS5 MATCH 查询基准 — O(log n) 倒排索引
fn bench_fts5_match_search(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let tmp = tempfile::tempdir().expect("创建临时目录失败");
    let store = setup_store(&tmp.path().join("fts5.db"), true);
    prefill(&rt, &store);

    // 搜索 uniq-500:只匹配 entry 500(FTS5 索引直接定位)
    let mut group = c.benchmark_group("fulltext_search");
    group.throughput(Throughput::Elements(DOC_COUNT as u64));
    // WHY:sample_size=10 是 criterion 最小值(要求 >=10),保证统计显著性
    group.sample_size(10);
    group.bench_function("fts5_match", |b| {
        b.iter(|| {
            rt.block_on(async {
                let found = store
                    .search_fulltext(black_box("uniq-500".to_string()))
                    .await
                    .expect("FTS5 搜索失败");
                assert!(!found.is_empty(), "FTS5 应返回结果");
            });
        });
    });
    group.finish();
}

/// LIKE 全表扫描基准 — O(n),作为 FTS5 的对照
fn bench_like_search(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let tmp = tempfile::tempdir().expect("创建临时目录失败");
    let store = setup_store(&tmp.path().join("like.db"), false);
    prefill(&rt, &store);

    let mut group = c.benchmark_group("fulltext_search");
    group.throughput(Throughput::Elements(DOC_COUNT as u64));
    group.sample_size(10);
    group.bench_function("like_scan", |b| {
        b.iter(|| {
            rt.block_on(async {
                let found = store
                    .search_fulltext(black_box("uniq-500".to_string()))
                    .await
                    .expect("LIKE 搜索失败");
                assert!(!found.is_empty(), "LIKE 应返回结果");
            });
        });
    });
    group.finish();
}

criterion_group!(benches, bench_fts5_match_search, bench_like_search);
criterion_main!(benches);
