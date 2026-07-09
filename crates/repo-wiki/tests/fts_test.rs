//! FTS5 全文索引测试 — 验证 FTS5 检索、降级策略与索引同步
//!
//! 对应 Task 2 / N15: repo-wiki FTS5 全文索引
//!
//! # 测试覆盖
//! - `test_fts5_search_returns_relevant_docs`:相关文档召回 + 无关文档排除
//! - `test_fts5_fallback_handles_invalid_query`:FTS5 query 语法错误时降级到 LIKE 不 panic
//! - `test_fts5_fallback_to_like_when_unavailable`:FTS5 禁用时降级到 LIKE 且功能正常
//! - `test_fts5_index_document_synced`:insert/delete 同步 FTS5 索引
//! - `test_fts5_capability_detected`:运行时检测 FTS5 可用性
//! - `test_fts5_upsert_no_duplicate_index`:UPSERT 场景下索引不重复
//!
//! # 设计原则
//! 这些测试在 FTS5 可用与不可用两种环境下都应通过:
//! - FTS5 可用:走 MATCH 路径,精确分词匹配
//! - FTS5 不可用:降级 LIKE,子串匹配同样召回相关文档
//!
//! 从而验证降级策略的透明性(调用方不感知底层引擎切换)。

#![forbid(unsafe_code)]

use repo_wiki::{FtsCapability, WikiConfig, WikiEntry, WikiStore};

/// 构建测试条目(512-dim 占位向量,空标签)
fn make_entry(id: &str, title: &str, content: &str) -> WikiEntry {
    WikiEntry::new(id, title, content, vec![], vec![0.0; 512])
}

/// 验证全文检索返回相关文档,不返回无关文档。
///
/// 该测试在 FTS5 可用与不可用两种情况下都应通过:
/// - FTS5 可用:走 MATCH 路径,unicode61 分词精确匹配
/// - FTS5 不可用:降级 LIKE,子串匹配同样召回相关文档
#[tokio::test]
async fn test_fts5_search_returns_relevant_docs() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("fts.db")).unwrap();

    // 插入 3 个文档:两个含 "tokio",一个不含
    store
        .insert(make_entry(
            "e-1",
            "Tokio runtime",
            "Tokio is an async runtime for Rust",
        ))
        .await
        .unwrap();
    store
        .insert(make_entry(
            "e-2",
            "Async programming",
            "The tokio scheduler drives futures",
        ))
        .await
        .unwrap();
    store
        .insert(make_entry(
            "e-3",
            "Database design",
            "SQLite is an embedded database",
        ))
        .await
        .unwrap();

    // 搜索 "tokio":应返回 e-1 和 e-2,不返回 e-3
    let found = store.search_fulltext("tokio".to_string()).await.unwrap();
    let ids: Vec<String> = found.iter().map(|e| e.entry_id.clone()).collect();
    assert!(ids.contains(&"e-1".to_string()), "应召回 e-1");
    assert!(ids.contains(&"e-2".to_string()), "应召回 e-2");
    assert!(!ids.contains(&"e-3".to_string()), "不应召回无关文档 e-3");
}

/// 验证 FTS5 query 语法错误时降级到 LIKE,不 panic、不返回 Err。
///
/// WHY:FTS5 MATCH 对特殊字符(如不平衡的双引号)会报语法错误。
/// `search_fulltext` 应捕获该错误并降级到 LIKE 路径,保证调用方不感知错误。
/// 降级后 LIKE 用 `%query%` 匹配,即使无结果也应返回空 Vec 而非 Err。
#[tokio::test]
async fn test_fts5_fallback_handles_invalid_query() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("fts_fallback.db")).unwrap();

    store
        .insert(make_entry(
            "e-1",
            "Rust guide",
            "Rust programming language guide",
        ))
        .await
        .unwrap();

    // 不平衡的双引号:FTS5 会报语法错误,触发降级
    // 降级后 LIKE 查 `%"%`,文档无双引号,返回空 Vec(不 panic、不 Err)
    let result = store.search_fulltext("\"".to_string()).await;
    assert!(result.is_ok(), "FTS5 语法错误应降级而非返回 Err");
    assert!(
        result.unwrap().is_empty(),
        "降级 LIKE 后无双引号匹配,应返回空"
    );

    // 正常 query 仍能工作(降级不影响正常路径)
    let found = store.search_fulltext("Rust".to_string()).await.unwrap();
    assert!(!found.is_empty(), "正常 query 应返回结果");
}

/// 验证 FTS5 被显式禁用(`fts_enabled = false`)时降级到 LIKE 且功能正常。
///
/// WHY:某些环境(嵌入式平台、旧版 SQLite)可能不支持 FTS5 扩展,
/// 或用户出于存储/兼容性考虑显式禁用。此时 `WikiStore` 应标记
/// `Unavailable` 并走 LIKE 全表扫描,保证功能可用性。
/// 此测试与 `test_fts5_fallback_handles_invalid_query` 互补:
/// 后者测试运行时 MATCH 语法错误降级,本测试测试配置级禁用降级。
#[tokio::test]
async fn test_fts5_fallback_to_like_when_unavailable() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("fts_disabled.db");

    // 显式禁用 FTS5,模拟不可用环境
    let config = WikiConfig::with_path(&db_path).fts_enabled(false);
    let store = WikiStore::open_with_config(config).unwrap();

    // 确认 FTS5 被标记为不可用
    assert_eq!(
        store.fts_capability(),
        FtsCapability::Unavailable,
        "禁用 FTS5 后 capability 应为 Unavailable"
    );

    // 插入条目
    store
        .insert(make_entry("e-1", "Rust 编程", "Rust 是一门系统级编程语言"))
        .await
        .unwrap();
    store
        .insert(make_entry("e-2", "Python 脚本", "Python 是动态语言"))
        .await
        .unwrap();

    // LIKE 降级搜索仍应正常工作(子串匹配)
    let found = store.search_fulltext("Rust".to_string()).await.unwrap();
    assert_eq!(found.len(), 1, "LIKE 降级应匹配 1 条含 'Rust' 的条目");
    assert_eq!(found[0].entry_id, "e-1");

    let found_py = store.search_fulltext("Python".to_string()).await.unwrap();
    assert_eq!(found_py.len(), 1, "LIKE 降级应匹配 1 条含 'Python' 的条目");
    assert_eq!(found_py[0].entry_id, "e-2");

    // 不存在的词应返回空
    let not_found = store
        .search_fulltext("nonexistent_xyz".to_string())
        .await
        .unwrap();
    assert!(not_found.is_empty(), "不存在的词应返回空");
}

/// 验证 insert 后 FTS5 索引同步可见,delete 后同步清除。
#[tokio::test]
async fn test_fts5_index_document_synced() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("fts_sync.db")).unwrap();

    // 初始无文档,搜索返回空
    let empty = store.search_fulltext("kernel".to_string()).await.unwrap();
    assert!(empty.is_empty(), "初始状态应无匹配");

    // insert 后立即可搜到(索引同步)
    store
        .insert(make_entry(
            "e-sync",
            "Kernel module",
            "The kernel scheduler dispatches tasks",
        ))
        .await
        .unwrap();
    let found = store.search_fulltext("kernel".to_string()).await.unwrap();
    assert_eq!(found.len(), 1, "insert 后应同步建立索引");
    assert_eq!(found[0].entry_id, "e-sync");

    // delete 后搜不到(索引清除)
    store.delete("e-sync".to_string()).await.unwrap();
    let gone = store.search_fulltext("kernel".to_string()).await.unwrap();
    assert!(gone.is_empty(), "delete 后应同步清除索引");
}

/// 验证运行时检测 FTS5 可用性,且 capability 方法可访问。
///
/// WHY:`libsqlite3-sys 0.30.1` bundled 默认硬编码 `-DSQLITE_ENABLE_FTS5`,
/// 正常运行环境应为 `Available`;但跨平台或非 bundled 场景可能 `Unavailable`。
/// 此处仅断言方法可调用且返回合法枚举值,不强依赖具体状态(防御性,在系统边界)。
#[tokio::test]
async fn test_fts5_capability_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("fts_cap.db")).unwrap();

    let cap = store.fts_capability();
    assert!(
        matches!(cap, FtsCapability::Available | FtsCapability::Unavailable),
        "capability 应返回合法枚举值"
    );

    // Available 时进一步验证:虚拟表确实可 MATCH 查询
    if cap.is_available() {
        store
            .insert(make_entry(
                "cap-1",
                "capability probe",
                "probe content here",
            ))
            .await
            .unwrap();
        let found = store.search_fulltext("probe".to_string()).await.unwrap();
        assert!(
            found.iter().any(|e| e.entry_id == "cap-1"),
            "FTS5 Available 时应能检索到文档"
        );
    }
}

/// 验证 UPSERT 场景下 FTS5 索引不产生重复行。
///
/// WHY:entries 表用 `INSERT OR REPLACE`(UPSERT),同一 entry_id 重复 insert。
/// FTS5 表无 PRIMARY KEY 约束,若不"先删后插"会累积重复行,
/// 导致 MATCH 查询返回重复结果。此测试验证同步策略正确。
#[tokio::test]
async fn test_fts5_upsert_no_duplicate_index() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("fts_upsert.db")).unwrap();

    // 同一 entry_id 插入 3 次(UPSERT),每次都含 "common" 关键词
    store
        .insert(make_entry("e-up", "v1", "common keyword alpha"))
        .await
        .unwrap();
    store
        .insert(make_entry("e-up", "v2", "common keyword beta"))
        .await
        .unwrap();
    store
        .insert(make_entry("e-up", "v3", "common keyword gamma"))
        .await
        .unwrap();

    // 搜索 "common":应只返回 1 条(最新版本 v3),无重复
    let found = store.search_fulltext("common".to_string()).await.unwrap();
    let matching: Vec<_> = found.iter().filter(|e| e.entry_id == "e-up").collect();
    assert_eq!(matching.len(), 1, "UPSERT 后 FTS5 索引不应有重复行");
    assert_eq!(matching[0].title, "v3", "应返回最新版本");
}
