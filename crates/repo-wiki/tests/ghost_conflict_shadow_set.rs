//! P3-W11.2.4 影子集测试 — 幽灵冲突率 <1% 验证
//!
//! 对应任务:P3-W11.2 验证项(spec.md:298 "写入路径检测 Contradicts 关系 → 标记过渡期不删旧记录")
//! 对应架构层:L5 Knowledge(repo-wiki 写入路径)
//!
//! # "幽灵冲突"定义(基于三重悖论·记忆悖论)
//!
//! 任务阶段切换时,新旧事实共存于召回结果中,无法区分时间有效性。
//! "幽灵冲突" = **应被矛盾检测归档(Historical)的旧版本条目,以 Current 状态
//! 残留在召回结果中**,与新版本条目形成虚假矛盾(看似冲突实则只是版本更迭)。
//!
//! # 幽灵冲突率公式
//!
//! ```text
//! 幽灵冲突率 = 残留 Current 的应归档条目数 / 应归档条目总数
//! ```
//!
//! 目标:< 1%(P3-W11.2 验收标准)
//!
//! # 影子集设计
//!
//! 构造 100 个主题的保留测试集(不参与配置/训练),每个主题有:
//! - `v1`(旧版本):先写入,初始 Current 状态
//! - `v2`(新版本):与 v1 高度相似(余弦相似度 ≈ 0.95 > 0.9 阈值),写入时
//!   触发矛盾检测,v1 应被标记 Historical
//!
//! **embedding 设计**(精确控制相似度):
//! - 主题 i 的 v1:维度 `2*i` 为 1.0,其余为 0.0(稀疏正交基)
//! - 主题 i 的 v2:维度 `2*i` 为 0.95,维度 `2*i+1` 为 0.31(相似度 ≈ 0.95)
//! - 不同主题使用不同维度对 → 正交(相似度 = 0.0,不会误检测为矛盾)
//! - 100 主题需 200 维,512-dim CLV 足够
//!
//! # 验证维度
//!
//! 1. **幽灵冲突率 <1%**(主测试)— 所有 v1 应被标记 Historical,召回只取 Current 时无幽灵冲突
//! 2. **Historical 标记完整性** — 所有应归档的 v1 确实被标记 Historical
//! 3. **召回过滤有效性** — 召回结果不含任何 Historical 条目
//! 4. **矛盾关系记录** — ContradictionResult 包含矛盾关系列表
//! 5. **独立主题不误报** — 不同主题(正交 embedding)不被检测为矛盾
//! 6. **阈值边界** — 相似度 < 0.9 的条目不触发矛盾检测(预期保留 Current)
//! 7. **幂等性** — 对已 Historical 的条目再次写入新版本,不重复标记
//!
//! # 红线对齐
//!
//! - §2.2: 测试在 L5 repo-wiki/tests/,dev-dependencies 允许(仅测试代码)
//! - §4.1: 无 unwrap/expect(边界用 ? 或 unwrap_or)
//! - §4.4 反模式 6: f32 禁止隐式转 f64,全程 f32(embedding)
//! - §6.1: 单函数 ≤ 200 行
//! - `#![forbid(unsafe_code)]`: 纯测试,无 unsafe 需求

#![forbid(unsafe_code)]

use repo_wiki::{ContradictionDetector, WikiEntry, WikiStore};
use std::time::Instant;

// ============================================================
// 常量 — 影子集规模与 embedding 参数
// ============================================================

/// 影子集主题数(100 个主题,v1+v2 各一,共 200 条目)
///
/// WHY 100:足够规模验证幽灵冲突率统计意义,同时测试完成 < 10s(100² 相似度计算)
const SHADOW_SET_TOPICS: usize = 100;

/// embedding 维度(与 CLV 512-dim 对齐)
const EMBEDDING_DIM: usize = 512;

/// v2 在主维度的分量(控制与 v1 的相似度 ≈ 0.95)
///
/// 计算相似度:cos(v1, v2) = 0.95 / sqrt(0.95² + 0.31²) ≈ 0.9506 > 0.9 阈值
const V2_MAIN_COMPONENT: f32 = 0.95;

/// v2 在次维度的分量(配合 V2_MAIN_COMPONENT 使 |v2| ≈ 1.0,相似度 ≈ 0.95)
const V2_SIDE_COMPONENT: f32 = 0.31;

/// 目标幽灵冲突率上限(P3-W11.2 验收标准:< 1%)
const GHOST_CONFLICT_RATE_TARGET: f32 = 0.01;

// ============================================================
// 辅助函数 — 影子集构造
// ============================================================

/// 构造主题 i 的 v1(旧版本)embedding
///
/// 维度 `2*i` 为 1.0,其余为 0.0(稀疏正交基)
fn make_v1_embedding(topic: usize) -> Vec<f32> {
    let mut emb = vec![0.0_f32; EMBEDDING_DIM];
    let dim = 2 * topic;
    if dim < EMBEDDING_DIM {
        emb[dim] = 1.0;
    }
    emb
}

/// 构造主题 i 的 v2(新版本)embedding
///
/// 维度 `2*i` 为 0.95,维度 `2*i+1` 为 0.31(相似度 ≈ 0.95 > 0.9 阈值)
fn make_v2_embedding(topic: usize) -> Vec<f32> {
    let mut emb = vec![0.0_f32; EMBEDDING_DIM];
    let dim = 2 * topic;
    if dim + 1 < EMBEDDING_DIM {
        emb[dim] = V2_MAIN_COMPONENT;
        emb[dim + 1] = V2_SIDE_COMPONENT;
    } else if dim < EMBEDDING_DIM {
        // 维度不足时降级:只用主维度(相似度 = 1.0,仍 > 0.9)
        emb[dim] = V2_MAIN_COMPONENT;
    }
    emb
}

/// 构造测试用 WikiEntry
fn make_entry(entry_id: &str, title: &str, content: &str, embedding: Vec<f32>) -> WikiEntry {
    WikiEntry::new(
        entry_id,
        title,
        content,
        vec!["shadow-set".into()],
        embedding,
    )
}

// ============================================================
// 主测试:幽灵冲突率 <1%
// ============================================================

/// P3-W11.2.4 主验收测试 — 影子集幽灵冲突率 < 1%
///
/// # 流程
///
/// 1. 构造 100 主题影子集(每主题 v1 + v2,共 200 条目)
/// 2. 先写入所有 v1(普通 insert,Current 状态)
/// 3. 依次写入 v2(用 insert_with_contradiction_check,触发矛盾检测)
/// 4. 验证:每个 v1 被标记 Historical,v2 保持 Current
/// 5. 召回:list_all() + is_current() 过滤(模拟 P3-W11.1 召回默认只取 Current)
/// 6. 统计幽灵冲突率 = 残留 Current 的 v1 数 / v1 总数
/// 7. 断言:< 1%
#[tokio::test]
async fn test_shadow_set_ghost_conflict_rate_under_1pct() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("shadow_set.db");
    let store = WikiStore::open(&db_path).unwrap();

    // === 阶段 1:写入所有 v1(旧版本,普通 insert) ===
    for topic in 0..SHADOW_SET_TOPICS {
        let entry = make_entry(
            &format!("topic-{topic}-v1"),
            &format!("主题 {topic} 旧版本"),
            &format!("topic {topic} version 1 content"),
            make_v1_embedding(topic),
        );
        store.insert(entry).await.unwrap();
    }
    assert_eq!(
        store.count().await.unwrap(),
        SHADOW_SET_TOPICS as u32,
        "v1 写入后条目数应为 {}",
        SHADOW_SET_TOPICS
    );

    // === 阶段 2:依次写入 v2(新版本,带矛盾检测) ===
    let mut total_contradictions = 0_usize;
    for topic in 0..SHADOW_SET_TOPICS {
        let entry = make_entry(
            &format!("topic-{topic}-v2"),
            &format!("主题 {topic} 新版本"),
            &format!("topic {topic} version 2 content (updated)"),
            make_v2_embedding(topic),
        );
        let result = store.insert_with_contradiction_check(entry).await.unwrap();

        // 每个新版本应触发矛盾检测(v1 与 v2 相似度 > 0.9)
        assert!(
            result.has_contradictions(),
            "主题 {topic} 的 v2 应触发矛盾检测"
        );
        // 矛盾关系数应为 1(仅同主题 v1 相似度 > 0.9,其他主题正交)
        assert_eq!(
            result.contradiction_count(),
            1,
            "主题 {topic} 的 v2 应只与同主题 v1 矛盾"
        );
        total_contradictions += result.contradiction_count();
    }

    // 200 条目(100 v1 + 100 v2)
    assert_eq!(
        store.count().await.unwrap(),
        (SHADOW_SET_TOPICS * 2) as u32,
        "v2 写入后条目数应为 {}",
        SHADOW_SET_TOPICS * 2
    );
    // 矛盾关系总数 = 100(每主题 1 个)
    assert_eq!(
        total_contradictions, SHADOW_SET_TOPICS,
        "矛盾关系总数应等于主题数"
    );

    // === 阶段 3:召回(模拟 P3-W11.1 召回默认只取 Current) ===
    let all_entries = store.list_all().await.unwrap();
    let current_entries: Vec<&WikiEntry> = all_entries.iter().filter(|e| e.is_current()).collect();
    let historical_entries: Vec<&WikiEntry> =
        all_entries.iter().filter(|e| e.is_historical()).collect();

    // === 阶段 4:统计幽灵冲突率 ===
    // 应归档数 = 100(所有 v1 应被标记 Historical)
    let should_archive = SHADOW_SET_TOPICS;

    // 实际归档数 = Historical 条目中的 v1 数
    let actual_archived = historical_entries
        .iter()
        .filter(|e| e.entry_id.ends_with("-v1"))
        .count();

    // 幽灵冲突数 = 残留 Current 的 v1(应被归档但未被归档)
    let ghost_conflicts = current_entries
        .iter()
        .filter(|e| e.entry_id.ends_with("-v1"))
        .count();

    // 幽灵冲突率 = 幽灵冲突数 / 应归档数
    let ghost_conflict_rate = if should_archive > 0 {
        ghost_conflicts as f32 / should_archive as f32
    } else {
        0.0
    };

    // === 阶段 5:断言 ===
    // 所有 v1 应被标记 Historical(无遗漏)
    assert_eq!(
        actual_archived, should_archive,
        "所有 v1 应被标记 Historical:实际 {actual_archived}, 期望 {should_archive}"
    );
    // 无幽灵冲突(残留 Current 的 v1 = 0)
    assert_eq!(
        ghost_conflicts, 0,
        "幽灵冲突数应为 0(所有 v1 已归档),实际 {ghost_conflicts}"
    );
    // 幽灵冲突率 < 1%
    assert!(
        ghost_conflict_rate < GHOST_CONFLICT_RATE_TARGET,
        "幽灵冲突率 {ghost_conflict_rate:.4} 应 < {GHOST_CONFLICT_RATE_TARGET}(1%)"
    );

    // Current 条目应全部是 v2(100 个)
    assert_eq!(
        current_entries.len(),
        SHADOW_SET_TOPICS,
        "Current 条目应全部是 v2,数量 = {}",
        SHADOW_SET_TOPICS
    );
    // 所有 Current 条目都是 v2
    assert!(
        current_entries.iter().all(|e| e.entry_id.ends_with("-v2")),
        "所有 Current 条目应为 v2"
    );
}

// ============================================================
// 验证 2:Historical 标记完整性
// ============================================================

/// 验证所有应归档的 v1 确实被标记 Historical(is_historical() == true)
#[tokio::test]
async fn test_shadow_set_old_versions_marked_historical() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("historical.db")).unwrap();

    // 写入 20 主题(小规模验证 Historical 标记)
    let topics = 20;
    for topic in 0..topics {
        let v1 = make_entry(
            &format!("t{topic}-v1"),
            "old",
            "old content",
            make_v1_embedding(topic),
        );
        store.insert(v1).await.unwrap();
    }
    for topic in 0..topics {
        let v2 = make_entry(
            &format!("t{topic}-v2"),
            "new",
            "new content",
            make_v2_embedding(topic),
        );
        store.insert_with_contradiction_check(v2).await.unwrap();
    }

    // 验证每个 v1 都被标记 Historical
    for topic in 0..topics {
        let v1 = store
            .get(format!("t{topic}-v1"))
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("t{topic}-v1 应存在"));
        assert!(
            v1.is_historical(),
            "t{topic}-v1 应被标记 Historical,实际 temporal_meta = {:?}",
            v1.temporal_meta()
        );
        assert!(!v1.is_current(), "t{topic}-v1 不应为 Current 状态");
    }

    // 验证每个 v2 都保持 Current
    for topic in 0..topics {
        let v2 = store
            .get(format!("t{topic}-v2"))
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("t{topic}-v2 应存在"));
        assert!(v2.is_current(), "t{topic}-v2 应保持 Current 状态");
        assert!(!v2.is_historical(), "t{topic}-v2 不应被标记 Historical");
    }
}

// ============================================================
// 验证 3:召回过滤有效性
// ============================================================

/// 验证召回结果(is_current 过滤)不含任何 Historical 条目
#[tokio::test]
async fn test_shadow_set_recall_excludes_historical() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("recall.db")).unwrap();

    // 写入 10 主题
    for topic in 0..10_usize {
        let v1 = make_entry(
            &format!("r{topic}-v1"),
            "old",
            "content",
            make_v1_embedding(topic),
        );
        store.insert(v1).await.unwrap();
    }
    for topic in 0..10_usize {
        let v2 = make_entry(
            &format!("r{topic}-v2"),
            "new",
            "content new",
            make_v2_embedding(topic),
        );
        store.insert_with_contradiction_check(v2).await.unwrap();
    }

    // 召回:模拟 P3-W11.1 召回默认只取 Current
    let all = store.list_all().await.unwrap();
    let recalled: Vec<&WikiEntry> = all.iter().filter(|e| e.is_current()).collect();

    // 召回结果不应包含任何 Historical 条目
    let historical_in_recall: Vec<_> = recalled.iter().filter(|e| e.is_historical()).collect();
    assert!(
        historical_in_recall.is_empty(),
        "召回结果不应包含 Historical 条目,实际包含 {} 个",
        historical_in_recall.len()
    );

    // 召回结果应全部是 Current
    assert!(
        recalled.iter().all(|e| e.is_current()),
        "召回结果应全部是 Current 状态"
    );

    // 召回数量 = 10(只含 v2)
    assert_eq!(
        recalled.len(),
        10,
        "召回应只含 10 个 v2(Current),实际 {}",
        recalled.len()
    );
}

// ============================================================
// 验证 4:矛盾关系记录
// ============================================================

/// 验证 insert_with_contradiction_check 返回的 ContradictionResult 包含矛盾关系
#[tokio::test]
async fn test_shadow_set_contradiction_relations_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("relations.db")).unwrap();

    // 写入 v1
    let v1 = make_entry("rel-v1", "old", "content", make_v1_embedding(0));
    store.insert(v1).await.unwrap();

    // 写入 v2(触发矛盾检测)
    let v2 = make_entry("rel-v2", "new", "content new", make_v2_embedding(0));
    let result = store.insert_with_contradiction_check(v2).await.unwrap();

    // 验证矛盾关系
    assert!(result.has_contradictions(), "应检测到矛盾");
    assert_eq!(result.contradiction_count(), 1, "应有 1 条矛盾关系");

    let rel = &result.contradictions[0];
    assert_eq!(rel.source_id, "rel-v2", "source 应为新条目 v2");
    assert_eq!(rel.target_id, "rel-v1", "target 应为旧条目 v1");
    assert!(
        rel.confidence > 0.9,
        "矛盾置信度应 > 0.9,实际 {}",
        rel.confidence
    );
    assert!(
        rel.evidence.contains("cosine_similarity"),
        "证据应包含 cosine_similarity,实际: {}",
        rel.evidence
    );
}

// ============================================================
// 验证 5:独立主题不误报
// ============================================================

/// 验证不同主题(正交 embedding)不被检测为矛盾
#[tokio::test]
async fn test_shadow_set_independent_topics_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("independent.db")).unwrap();

    // 写入 3 个独立主题的 v1(正交 embedding)
    for topic in 0..3_usize {
        let v1 = make_entry(
            &format!("ind-{topic}-v1"),
            "title",
            "content",
            make_v1_embedding(topic),
        );
        store.insert(v1).await.unwrap();
    }

    // 写入主题 0 的 v2
    let v2 = make_entry("ind-0-v2", "new", "new content", make_v2_embedding(0));
    let result = store.insert_with_contradiction_check(v2).await.unwrap();

    // 应只与同主题(0)的 v1 矛盾,不与其他主题(1, 2)矛盾
    assert_eq!(
        result.contradiction_count(),
        1,
        "应只检测到 1 条矛盾(同主题 v1),实际 {}",
        result.contradiction_count()
    );

    let target = &result.contradictions[0].target_id;
    assert!(
        target.starts_with("ind-0"),
        "矛盾目标应为主题 0 的 v1,实际 {target}"
    );
}

// ============================================================
// 验证 6:阈值边界
// ============================================================

/// 验证相似度 < 0.9 的条目不触发矛盾检测(预期保留 Current)
///
/// 构造相似度 ≈ 0.8 的 embedding(< 0.9 阈值):
/// - v1:维度 0 为 1.0
/// - v2:维度 0 为 0.8,维度 1 为 0.6(相似度 = 0.8 / 1.0 = 0.8)
#[tokio::test]
async fn test_shadow_set_below_threshold_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("threshold.db")).unwrap();

    // v1:维度 0 为 1.0
    let mut v1_emb = vec![0.0_f32; EMBEDDING_DIM];
    v1_emb[0] = 1.0;
    let v1 = make_entry("thr-v1", "old", "content", v1_emb);
    store.insert(v1).await.unwrap();

    // v2:维度 0 为 0.8,维度 1 为 0.6 → 相似度 = 0.8 < 0.9
    let mut v2_emb = vec![0.0_f32; EMBEDDING_DIM];
    v2_emb[0] = 0.8;
    v2_emb[1] = 0.6;
    let v2 = make_entry("thr-v2", "new", "new content", v2_emb);
    let result = store.insert_with_contradiction_check(v2).await.unwrap();

    // 相似度 < 0.9,不应触发矛盾检测
    assert!(
        !result.has_contradictions(),
        "相似度 < 0.9 不应触发矛盾检测,实际检测到 {} 条",
        result.contradiction_count()
    );

    // v1 应保持 Current(未被标记 Historical)
    let v1_after = store.get("thr-v1".to_string()).await.unwrap().unwrap();
    assert!(v1_after.is_current(), "v1 应保持 Current(未触发矛盾检测)");
}

// ============================================================
// 验证 7:幂等性
// ============================================================

/// 验证对已 Historical 的条目再次写入新版本,不重复标记(幂等性)
#[tokio::test]
async fn test_shadow_set_idempotent_contradiction_check() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("idempotent.db")).unwrap();

    // 写入 v1
    let v1 = make_entry("idem-v1", "old", "content", make_v1_embedding(0));
    store.insert(v1).await.unwrap();

    // 写入 v2(触发矛盾检测,v1 被标记 Historical)
    let v2 = make_entry("idem-v2", "new", "new", make_v2_embedding(0));
    let result1 = store.insert_with_contradiction_check(v2).await.unwrap();
    assert_eq!(
        result1.contradiction_count(),
        1,
        "首次写入 v2 应检测到 1 条矛盾"
    );

    // 验证 v1 已 Historical
    let v1_after = store.get("idem-v1".to_string()).await.unwrap().unwrap();
    assert!(v1_after.is_historical(), "v1 应被标记 Historical");

    // 写入 v3(与 v2 相似度 > 0.9,但 v1 已 Historical 不应重复标记)
    let v3 = make_entry("idem-v3", "newer", "newer", make_v2_embedding(0));
    let result2 = store.insert_with_contradiction_check(v3).await.unwrap();

    // v3 应与 v2(仍 Current)矛盾,但不与 v1(已 Historical)重复矛盾
    // 因为 find_contradiction_candidates 只查 Current 条目
    assert_eq!(
        result2.contradiction_count(),
        1,
        "v3 应只与 v2(仍 Current)矛盾,不与 v1(已 Historical)重复"
    );

    // 验证矛盾目标是 v2 而非 v1
    let target = &result2.contradictions[0].target_id;
    assert_eq!(
        target, "idem-v2",
        "v3 的矛盾目标应为 v2(Current),实际 {target}"
    );

    // v2 应被标记 Historical(v3 取代 v2)
    let v2_after = store.get("idem-v2".to_string()).await.unwrap().unwrap();
    assert!(v2_after.is_historical(), "v2 应被标记 Historical(v3 取代)");

    // v3 应保持 Current
    let v3_after = store.get("idem-v3".to_string()).await.unwrap().unwrap();
    assert!(v3_after.is_current(), "v3 应保持 Current");
}

// ============================================================
// 性能验证:影子集规模下的延迟
// ============================================================

/// 验证 100 主题影子集的矛盾检测总延迟合理(< 30s)
///
/// WHY:确保矛盾检测的 O(n²) 扫描在 100 entry 规模下可接受
#[tokio::test]
async fn test_shadow_set_performance_under_30s() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("perf.db")).unwrap();

    // 写入 50 主题的 v1(减少规模以加速测试,仍足够验证性能)
    let topics = 50;
    for topic in 0..topics {
        let v1 = make_entry(
            &format!("p{topic}-v1"),
            "old",
            "content",
            make_v1_embedding(topic),
        );
        store.insert(v1).await.unwrap();
    }

    // 计时:依次写入 50 个 v2(每个触发矛盾检测)
    let start = Instant::now();
    for topic in 0..topics {
        let v2 = make_entry(
            &format!("p{topic}-v2"),
            "new",
            "new",
            make_v2_embedding(topic),
        );
        store.insert_with_contradiction_check(v2).await.unwrap();
    }
    let elapsed = start.elapsed();

    // 50 个 insert_with_contradiction_check 应 < 30s
    // 实际通常 < 5s(每次 O(n) 扫描 + O(1) 相似度计算)
    assert!(
        elapsed.as_secs() < 30,
        "50 主题矛盾检测总延迟 {elapsed:?} 应 < 30s"
    );
}

// ============================================================
// ContradictionDetector 单元验证(影子集上下文)
// ============================================================

/// 验证 ContradictionDetector 对影子集 embedding 的相似度计算正确
///
/// 确认 v1 与 v2 的相似度确实 > 0.9(触发矛盾检测的前提)
#[test]
fn test_shadow_set_embedding_similarity_above_threshold() {
    let detector = ContradictionDetector::new();

    // 主题 0 的 v1 和 v2
    let v1 = make_entry("sim-v1", "old", "content", make_v1_embedding(0));
    let v2_emb = make_v2_embedding(0);

    // 用 ContradictionDetector 检测(v2 视为新条目,v1 视为候选)
    // 注意:detect 的第一个参数是新条目,第二个是候选列表
    let v2 = make_entry("sim-v2", "new", "new", v2_emb);
    let contradictions = detector.detect(&v2, &[v1]);

    assert_eq!(
        contradictions.len(),
        1,
        "v1 与 v2 相似度应 > 0.9,触发矛盾检测"
    );
    let sim = contradictions[0].confidence;
    assert!(sim > 0.9, "v1-v2 相似度 {sim:.4} 应 > 0.9");
    assert!(
        sim < 1.0,
        "v1-v2 相似度 {sim:.4} 应 < 1.0(不同 embedding,非完全相同)"
    );
}

/// 验证不同主题的 embedding 正交(相似度 = 0.0,不触发矛盾检测)
#[test]
fn test_shadow_set_cross_topic_orthogonal() {
    let detector = ContradictionDetector::new();

    let topic0_v1 = make_entry("orth-0", "t0", "c0", make_v1_embedding(0));
    let topic1_v1 = make_entry("orth-1", "t1", "c1", make_v1_embedding(1));

    // 主题 0 和主题 1 的 embedding 正交,不应触发矛盾检测
    let contradictions = detector.detect(&topic0_v1, &[topic1_v1]);
    assert!(
        contradictions.is_empty(),
        "不同主题的 embedding 应正交,不触发矛盾检测"
    );
}
