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
/// 正常运行环境应为 `AvailableTrigram` 或 `AvailableUnicode61`(取决于 SQLite
/// 版本是否支持 trigram tokenizer,3.34+ 支持);但跨平台或非 bundled 场景
/// 可能 `Unavailable`。此处仅断言方法可调用且返回合法枚举值,不强依赖具体
/// 状态(防御性,在系统边界)。
///
/// v1.3.0 升级:`FtsCapability` 从二值(Available/Unavailable)扩展为三值
/// (AvailableTrigram/AvailableUnicode61/Unavailable),此测试同步更新匹配
/// 三值枚举,验证 v1.2.0 调用方语义在三值改造后仍正确(`is_available()`
/// 对 AvailableTrigram + AvailableUnicode61 返回 true)。
#[tokio::test]
async fn test_fts5_capability_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("fts_cap.db")).unwrap();

    let cap = store.fts_capability();
    assert!(
        matches!(
            cap,
            FtsCapability::AvailableTrigram
                | FtsCapability::AvailableUnicode61
                | FtsCapability::Unavailable
        ),
        "capability 应返回合法枚举值,实际 = {cap:?}"
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
            "FTS5 Available 时应能检索到文档 (cap={cap:?})"
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

// ============================================================================
// v1.3.0 Task S3: trigram tokenizer 升级测试
// ============================================================================
//
// 这些测试覆盖 trigram tokenizer 升级后的三值 FtsCapability 与三级降级链。
// 设计原则:测试不假设 trigram 实际可用性(SQLite 编译差异),通过
// `store.fts_capability()` 返回值分支判断,使测试在 trigram 可用与不可用
// 两种环境下都能通过。

/// 验证 trigram 能力下 CJK 三字以上子串可直接 MATCH 命中,无需降级 LIKE。
///
/// WHY:unicode61 tokenizer 将连续 CJK 字符视为单 token,"性能分析报告" 是
/// 一个整体 token,子串 "分析报告" MATCH 不命中(v1.2.0 依赖 LIKE 降级)。
/// trigram tokenizer 将文本按 3 字符滑窗分词,"性能分析报告" 会生成
/// "性能分"、"能分析"、"分析报"、"析报告" 等 trigram,"分析报告" 可直接
/// MATCH 命中。此测试验证 trigram 能力下 CJK 子串检索无需降级 LIKE。
///
/// 若运行环境 trigram 不可用,降级到 AvailableUnicode61,本测试仍通过
/// (unicode61 路径会空结果降级 LIKE,LIKE 子串匹配也能命中)。
#[tokio::test]
async fn test_trigram_cjk_substring_match() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("trigram_cjk.db")).unwrap();
    let cap = store.fts_capability();

    // 索引含 4 字 CJK 子串的文档
    store
        .insert(make_entry(
            "e-cjk",
            "性能分析报告",
            "本周性能分析报告已生成",
        ))
        .await
        .unwrap();
    store
        .insert(make_entry("e-other", "无关条目", "完全不同的内容"))
        .await
        .unwrap();

    // 搜索 4 字 CJK 子串 "分析报告"(>= 3 字符,trigram 应处理)
    let found = store.search_fulltext("分析报告".to_string()).await.unwrap();
    let ids: Vec<String> = found.iter().map(|e| e.entry_id.clone()).collect();
    assert!(
        ids.contains(&"e-cjk".to_string()),
        "CJK 子串 '分析报告' 应召回 e-cjk (cap={cap:?})"
    );
    assert!(
        !ids.contains(&"e-other".to_string()),
        "不应召回无关条目 e-other"
    );

    // 若 trigram 可用,验证未走 LIKE 路径(无法直接验证,但通过 3 字符 CJK
    // 子串 "性能分" 进一步验证 trigram 行为;unicode61 + LIKE 也能命中)
    if matches!(cap, FtsCapability::AvailableTrigram) {
        let found3 = store.search_fulltext("性能分".to_string()).await.unwrap();
        assert!(
            found3.iter().any(|e| e.entry_id == "e-cjk"),
            "trigram 可用时 3 字 CJK 子串 '性能分' 应命中"
        );
    }
}

/// 验证短查询(< 3 字符)降级路径 — trigram 对 < 3 字符无优势。
///
/// WHY:trigram tokenizer 按 3 字符滑窗分词,1-2 字符查询无法生成有效
/// trigram token,继续走 trigram MATCH 会空结果或报错。`search_fulltext`
/// 应识别短查询并降级 unicode61 或 LIKE 路径。本测试验证 1 字符 CJK
/// 查询仍能返回正确结果(经 LIKE 路径召回)。
#[tokio::test]
async fn test_trigram_short_query_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("trigram_short.db")).unwrap();

    store
        .insert(make_entry("e-short", "数据分析", "数据统计分析方法"))
        .await
        .unwrap();

    // 1 字符 CJK 查询:trigram 无优势,应降级到 LIKE 路径
    let found = store.search_fulltext("数".to_string()).await.unwrap();
    assert!(
        found.iter().any(|e| e.entry_id == "e-short"),
        "1 字符查询 '数' 应召回 e-short(经 LIKE 路径)"
    );

    // 2 字符 CJK 查询:同样应降级
    let found2 = store.search_fulltext("数据".to_string()).await.unwrap();
    assert!(
        found2.iter().any(|e| e.entry_id == "e-short"),
        "2 字符查询 '数据' 应召回 e-short"
    );
}

/// 验证 trigram 不可用时降级到 unicode61(运行时检测)。
///
/// WHY:trigram tokenizer 需要 SQLite 3.34+ 编译选项,非 bundled 或旧版
/// SQLite 可能不支持。`init_fts_table` 应优先尝试 trigram,创建失败则
/// 降级创建 unicode61 虚拟表,标记 `AvailableUnicode61`。
///
/// 本测试通过 `WikiConfig::with_path().fts_enabled(true)` 触发 FTS5 初始化,
/// 验证 `fts_capability()` 返回三值之一(AvailableTrigram / AvailableUnicode61 /
/// Unavailable 中的合法值),且 AvailableTrigram + AvailableUnicode61 都表示
/// FTS5 可用(`is_available() == true`)。
#[tokio::test]
async fn test_trigram_unavailable_falls_back_to_unicode61() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("trigram_fallback.db")).unwrap();
    let cap = store.fts_capability();

    // capability 必须是三值之一
    assert!(
        matches!(
            cap,
            FtsCapability::AvailableTrigram
                | FtsCapability::AvailableUnicode61
                | FtsCapability::Unavailable
        ),
        "capability 应为三值之一,实际 = {cap:?}"
    );

    // trigram 或 unicode61 可用时应 is_available == true
    match cap {
        FtsCapability::AvailableTrigram | FtsCapability::AvailableUnicode61 => {
            assert!(
                cap.is_available(),
                "trigram/unicode61 可用时 is_available 应为 true"
            );
        }
        FtsCapability::Unavailable => {
            assert!(
                !cap.is_available(),
                "Unavailable 时 is_available 应为 false"
            );
        }
    }
}

/// 验证 FTS5 完全不可用时(`fts_enabled = false`)降级到 LIKE。
///
/// WHY:v1.2.0 行为 — 用户显式禁用 FTS5 或 SQLite 未编译 FTS5 扩展时,
/// `WikiStore` 应标记 `Unavailable` 并走 LIKE 全表扫描,保证功能可用性。
/// 此测试确保 v1.3.0 三值枚举改造后 v1.2.0 的禁用降级路径仍正确。
#[tokio::test]
async fn test_unicode61_unavailable_falls_back_to_like() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("unicode61_unavailable.db");

    // 显式禁用 FTS5,模拟完全不可用环境
    let config = WikiConfig::with_path(&db_path).fts_enabled(false);
    let store = WikiStore::open_with_config(config).unwrap();

    // 应标记为 Unavailable
    assert_eq!(
        store.fts_capability(),
        FtsCapability::Unavailable,
        "禁用 FTS5 后 capability 应为 Unavailable"
    );

    // 插入 CJK 文档
    store
        .insert(make_entry(
            "e-like",
            "全文检索",
            "FTS5 不可用时走 LIKE 降级",
        ))
        .await
        .unwrap();

    // LIKE 路径应正常工作(子串匹配)
    let found = store.search_fulltext("全文".to_string()).await.unwrap();
    assert_eq!(found.len(), 1, "LIKE 降级应召回 1 条");
    assert_eq!(found[0].entry_id, "e-like");
}

/// 验证完整降级链:trigram > unicode61 > LIKE,各路径行为正确。
///
/// WHY:三级降级链是 v1.3.0 的核心设计 — 每级保证可用性,不可假设
/// "创建成功即可用"(trigram 创建可能成功但 MATCH 不工作,需 test_match
/// 验证)。本测试验证不同 FtsCapability 下 `search_fulltext` 行为一致
/// (返回正确结果,调用方不感知底层引擎切换)。
#[tokio::test]
async fn test_search_fulltext_priority_chain() {
    let tmp = tempfile::tempdir().unwrap();

    // 路径 1:启用 FTS5(可能 trigram 或 unicode61,取决于 SQLite 编译)
    let store_fts = WikiStore::open(&tmp.path().join("chain_fts.db")).unwrap();
    store_fts
        .insert(make_entry(
            "e-chain",
            "架构设计文档",
            "微服务架构设计原则与实践",
        ))
        .await
        .unwrap();
    let cap = store_fts.fts_capability();
    // 启用 FTS5 后应至少 AvailableTrigram 或 AvailableUnicode61(不应 Unavailable,
    // 除非 SQLite 未编译 FTS5)
    let found = store_fts
        .search_fulltext("架构设计".to_string())
        .await
        .unwrap();
    assert!(
        found.iter().any(|e| e.entry_id == "e-chain"),
        "FTS5 路径(无论 trigram 或 unicode61)应召回 e-chain (cap={cap:?})"
    );

    // 路径 2:禁用 FTS5(Unavailable → LIKE)
    // WHY 用 open_with_config 而非 open:WikiStore::open 用 default 配置
    // (fts_enabled=true),会忽略 builder 设置的 fts_enabled(false)。必须用
    // open_with_config 保留完整的配置对象,使 fts_enabled=false 生效。
    let like_config = WikiConfig::with_path(tmp.path().join("chain_like.db")).fts_enabled(false);
    let store_like = WikiStore::open_with_config(like_config).unwrap();
    store_like
        .insert(make_entry(
            "e-chain",
            "架构设计文档",
            "微服务架构设计原则与实践",
        ))
        .await
        .unwrap();
    assert_eq!(store_like.fts_capability(), FtsCapability::Unavailable);
    let found_like = store_like
        .search_fulltext("架构设计".to_string())
        .await
        .unwrap();
    assert!(
        found_like.iter().any(|e| e.entry_id == "e-chain"),
        "LIKE 路径应召回 e-chain"
    );
}

/// 验证英文查询在 trigram 与 unicode61 下结果一致。
///
/// WHY:trigram 与 unicode61 对英文(空格分词)查询行为应一致 — 两者都
/// 按空格分词,trigram 仅对每个 token 多生成 3 字符滑窗子串。英文查询
/// "architecture" 在 trigram 和 unicode61 下应返回相同结果集。
#[tokio::test]
async fn test_trigram_english_search() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("trigram_en.db")).unwrap();
    let cap = store.fts_capability();

    store
        .insert(make_entry(
            "e-arch",
            "System Architecture",
            "The architecture of distributed systems",
        ))
        .await
        .unwrap();
    store
        .insert(make_entry(
            "e-unrelated",
            "Cooking Guide",
            "How to bake bread",
        ))
        .await
        .unwrap();

    // 英文长查询(>= 3 字符,trigram 与 unicode61 应一致)
    let found = store
        .search_fulltext("architecture".to_string())
        .await
        .unwrap();
    let ids: Vec<String> = found.iter().map(|e| e.entry_id.clone()).collect();
    assert!(
        ids.contains(&"e-arch".to_string()),
        "英文查询 'architecture' 应召回 e-arch (cap={cap:?})"
    );
    assert!(
        !ids.contains(&"e-unrelated".to_string()),
        "不应召回无关英文文档"
    );

    // 短英文查询(< 3 字符)也应工作(降级 LIKE)
    let found_short = store.search_fulltext("ar".to_string()).await.unwrap();
    // "ar" 是 "architecture" 的子串,LIKE 应命中
    assert!(
        found_short.iter().any(|e| e.entry_id == "e-arch"),
        "短英文查询 'ar' 应经 LIKE 召回 e-arch"
    );
}
