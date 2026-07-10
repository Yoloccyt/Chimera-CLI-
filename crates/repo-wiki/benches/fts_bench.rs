//! FTS5 vs LIKE 全文检索性能基准
//!
//! 对应 Task 2 / N15: repo-wiki FTS5 全文索引(v1.2.0)
//! 对应 Task S3: FTS5 trigram tokenizer 升级(v1.3.0)
//!
//! # v1.2.0 基准(英文稀有词查询)
//! - `fts5_match`:1000 文档规模下 FTS5 MATCH 查询延迟(O(log n))
//! - `like_scan`:1000 文档规模下 LIKE 全表扫描延迟(O(n))
//!
//! v1.3.0 升级后 `WikiStore` 默认走 trigram tokenizer(SQLite 3.34+),
//! `fts5_match` 实际测量 trigram MATCH 路径。
//!
//! # v1.3.0 新增基准(CJK 三引擎对比)
//! - `cjk_fulltext_search/trigram/{50,100,1000}`:trigram MATCH CJK 4 字子串
//! - `cjk_fulltext_search/unicode61/{50,100,1000}`:unicode61 MATCH CJK 4 字子串
//! - `cjk_fulltext_search/like/{50,100,1000}`:LIKE `%query%` CJK 4 字子串
//!
//! # 对比意义(v1.3.0)
//! 在 CJK 4 字子串查询场景下,对比三种引擎的延迟:
//! - **trigram**:应直接命中(O(log n) 倒排索引),CJK 三字以上子串无需降级 LIKE
//! - **unicode61**:CJK 子串不命中(整体 token),MATCH 返回空结果(O(log n) 但无召回)
//! - **LIKE**:子串匹配命中(O(n) 全表扫描),保证召回率但性能随文档数线性增长
//!
//! 预期结论:trigram 延迟最低 + 命中正确;LIKE 延迟随文档数线性增长;
//! unicode61 延迟与 trigram 相近(都是 FTS5 MATCH)但无命中(tokenizer 局限)。
//!
//! # WHY 用 raw rusqlite 而非 WikiStore
//! v1.3.0 需要对比 trigram vs unicode61,但 `WikiStore::open` 运行时检测后
//! 固定走 trigram(若可用),无法强制 unicode61。bench 用 raw `rusqlite::Connection`
//! 直接控制 tokenizer,消除 `WikiStore` 的 spawn_blocking + 连接池开销,纯测量
//! FTS5/LIKE 查询性能。v1.2.0 的 `fts5_match` / `like_scan` 保留 WikiStore
//! 端到端测量,反映真实使用场景。
//!
//! # 运行
//! ```bash
//! cargo bench -p repo-wiki --bench fts_bench
//! ```

#![forbid(unsafe_code)]

use std::path::Path;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use repo_wiki::{WikiConfig, WikiEntry, WikiStore};
use rusqlite::{params, Connection};
use tokio::runtime::Runtime;

// ============================================================================
// v1.2.0 基准:WikiStore 端到端(英文稀有词查询)
// ============================================================================

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

/// FTS5 MATCH 查询基准 — O(log n) 倒排索引(v1.3.0 默认 trigram tokenizer)
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

// ============================================================================
// v1.3.0 新增基准:CJK 三引擎对比(raw rusqlite,50/100/1000 文档规模)
// ============================================================================

/// 文档规模梯度 — 50(小)/ 100(中)/ 1000(大),覆盖典型规模
const CJK_DOC_COUNTS: [usize; 3] = [50, 100, 1000];

/// 构建测试 CJK 文档 — 每个文档含 "性能分析报告" 子串,便于 CJK 4 字子串查询
///
/// WHY 所有文档共享相同 CJK 子串:bench 测量的是查询延迟而非召回率,
/// 所有文档都应被命中(LIKE 全表扫描 + trigram MATCH 都命中),
/// 确保三引擎查询的工作量一致(都遍历/索引相同数据)。
fn make_cjk_entry(i: usize) -> (String, String, String) {
    let entry_id = format!("bench-cjk-{i}");
    let title = format!("性能分析报告 {i}");
    let content = format!("本周性能分析报告已生成,编号 {i},包含架构设计与优化建议");
    (entry_id, title, content)
}

/// 用 raw rusqlite 创建指定 tokenizer 的 FTS5 表 + entries 表 + 预填充 CJK 文档
///
/// WHY raw rusqlite 而非 WikiStore:WikiStore 运行时检测后固定走 trigram
/// (若可用),无法强制 unicode61。bench 需要对比 trigram vs unicode61,
/// 必须直接控制 tokenizer。raw rusqlite 消除 WikiStore 的 spawn_blocking +
/// 连接池开销,纯测量 FTS5/LIKE 查询性能。
///
/// schema 简化:仅 entry_id / title / content 三列(bench 不需要 tags/embedding/
/// 时间戳,这些是 WikiStore 的存储关注点,不影响 FTS5/LIKE 查询性能)。
fn setup_raw_cjk_store(db_path: &Path, tokenizer: &str, doc_count: usize) -> Connection {
    let conn = Connection::open(db_path).expect("打开 raw rusqlite 失败");

    conn.execute_batch(
        "CREATE TABLE entries (
            entry_id TEXT PRIMARY KEY,
            title    TEXT NOT NULL,
            content  TEXT NOT NULL
        );",
    )
    .expect("创建 entries 表失败");

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE entries_fts USING fts5(\
         entry_id UNINDEXED, title, content, tokenize='{tokenizer}');"
    ))
    .expect("创建 FTS5 虚拟表失败");

    // 预填充 doc_count 个 CJK 文档
    for i in 0..doc_count {
        let (entry_id, title, content) = make_cjk_entry(i);
        conn.execute(
            "INSERT INTO entries(entry_id, title, content) VALUES (?1, ?2, ?3);",
            params![entry_id, title, content],
        )
        .expect("插入 entries 失败");
        conn.execute(
            "INSERT INTO entries_fts(entry_id, title, content) VALUES (?1, ?2, ?3);",
            params![entry_id, title, content],
        )
        .expect("插入 entries_fts 失败");
    }

    conn
}

/// CJK 4 字子串查询基准 — 对比 trigram / unicode61 / LIKE 三引擎
///
/// 查询:"分析报告"(4 字 CJK 子串,所有文档的 title/content 均含此子串)
///
/// 预期行为:
/// - trigram:直接命中(O(log n) 倒排索引,trigram 分词 "分析报" + "析报告")
/// - unicode61:空结果(整体 token,"性能分析报告" 是单 token,"分析报告" 不匹配)
/// - LIKE:命中(O(n) 全表扫描,%分析报告% 子串匹配)
fn bench_cjk_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("cjk_fulltext_search");
    group.throughput(Throughput::Elements(CJK_DOC_COUNTS[2] as u64));
    // WHY sample_size=10:与 v1.2.0 bench 一致,criterion 最小值,保证统计显著性
    group.sample_size(10);

    for &doc_count in &CJK_DOC_COUNTS {
        // trigram MATCH
        let tmp_t = tempfile::tempdir().expect("创建 trigram 临时目录失败");
        let conn_t = setup_raw_cjk_store(&tmp_t.path().join("trigram.db"), "trigram", doc_count);
        group.bench_function(format!("trigram/{doc_count}"), |b| {
            b.iter(|| {
                let mut stmt = conn_t
                    .prepare(
                        "SELECT e.entry_id FROM entries e
                         JOIN entries_fts f ON e.entry_id = f.entry_id
                         WHERE entries_fts MATCH ?1
                         ORDER BY rank;",
                    )
                    .expect("prepare trigram 失败");
                let found: Vec<String> = stmt
                    .query_map(params!["分析报告"], |row| row.get::<_, String>(0))
                    .expect("query_map trigram 失败")
                    .filter_map(|r| r.ok())
                    .collect();
                black_box(found);
            });
        });

        // unicode61 MATCH(预期空结果 — CJK 整体 token 不命中)
        let tmp_u = tempfile::tempdir().expect("创建 unicode61 临时目录失败");
        let conn_u =
            setup_raw_cjk_store(&tmp_u.path().join("unicode61.db"), "unicode61", doc_count);
        group.bench_function(format!("unicode61/{doc_count}"), |b| {
            b.iter(|| {
                let mut stmt = conn_u
                    .prepare(
                        "SELECT e.entry_id FROM entries e
                         JOIN entries_fts f ON e.entry_id = f.entry_id
                         WHERE entries_fts MATCH ?1
                         ORDER BY rank;",
                    )
                    .expect("prepare unicode61 失败");
                let found: Vec<String> = stmt
                    .query_map(params!["分析报告"], |row| row.get::<_, String>(0))
                    .expect("query_map unicode61 失败")
                    .filter_map(|r| r.ok())
                    .collect();
                black_box(found);
            });
        });

        // LIKE 全表扫描(预期命中 — 子串匹配)
        let tmp_l = tempfile::tempdir().expect("创建 like 临时目录失败");
        let conn_l = setup_raw_cjk_store(&tmp_l.path().join("like.db"), "trigram", doc_count);
        group.bench_function(format!("like/{doc_count}"), |b| {
            b.iter(|| {
                let mut stmt = conn_l
                    .prepare(
                        "SELECT entry_id FROM entries
                         WHERE title LIKE ?1 OR content LIKE ?1;",
                    )
                    .expect("prepare LIKE 失败");
                let found: Vec<String> = stmt
                    .query_map(params!["%分析报告%"], |row| row.get::<_, String>(0))
                    .expect("query_map LIKE 失败")
                    .filter_map(|r| r.ok())
                    .collect();
                black_box(found);
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_fts5_match_search,
    bench_like_search,
    bench_cjk_search
);
criterion_main!(benches);
