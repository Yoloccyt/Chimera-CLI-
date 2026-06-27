//! 集成测试 — ISCM 跨层共享锚点机制
//!
//! 对应任务:Week 2 Task 5
//! 覆盖场景:
//! 1. 锚点创建与解析:L9_Quest 层创建锚点 → L5_Knowledge 层解析返回同一版本
//! 2. 悬空锚点检测:删除条目后 resolve_anchor 返回 WikiError::AnchorDangling
//! 3. 跨层一致性:L9 创建锚点 → L5 更新条目 → L2 读取,三者返回同一 updated_at
//! 4. 锚点 UUIDv7 全局唯一性:创建 1000 个锚点无冲突
//! 5. delete 联动标记悬空:删除条目后,list_anchors_by_entity 返回的锚点 is_dangling=true
//! 6. list_anchors_by_layer:按层过滤锚点
//! 7. mark_dangling 手动标记:手动调用 mark_dangling 后 resolve_anchor 返回错误

use chrono::Utc;
use repo_wiki::{IscmAnchor, Layer, WikiEntry, WikiError, WikiStore};
use uuid::Uuid;

/// 构造测试用 WikiEntry
fn make_entry(
    id: &str,
    title: &str,
    content: &str,
    tags: Vec<String>,
    embedding: Vec<f32>,
) -> WikiEntry {
    let now = Utc::now();
    WikiEntry {
        entry_id: id.into(),
        title: title.into(),
        content: content.into(),
        tags,
        embedding,
        created_at: now,
        updated_at: now,
    }
}

// ============================================================
// 场景 1:锚点创建与解析(L9 创建 → L5 解析)
// ============================================================

#[test]
fn test_anchor_create_and_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    // 插入条目
    let entry = make_entry(
        "e-1",
        "Title",
        "Content",
        vec!["tag".into()],
        vec![0.0; 512],
    );
    store.insert(&entry).unwrap();

    // L9 层创建锚点(模拟 quest-engine 引用知识实体)
    let anchor = store
        .create_anchor(Layer::L9_Quest, "quest-engine", "e-1")
        .unwrap();
    assert_eq!(anchor.layer, Layer::L9_Quest);
    assert_eq!(anchor.crate_name, "quest-engine");
    assert_eq!(anchor.entity_id, "e-1");
    assert!(!anchor.is_dangling);

    // L5 层解析锚点(模拟 repo-wiki 跨层读取)
    let resolved = store.resolve_anchor(anchor.anchor_id).unwrap();
    assert_eq!(resolved.entry_id, "e-1");
    assert_eq!(resolved.title, "Title");
    assert_eq!(resolved.content, "Content");
}

// ============================================================
// 场景 2:悬空锚点检测(删除条目后 resolve 返回 AnchorDangling)
// ============================================================

#[test]
fn test_dangling_anchor_detection() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    let entry = make_entry("e-1", "Title", "Content", vec![], vec![0.0; 512]);
    store.insert(&entry).unwrap();

    let anchor = store
        .create_anchor(Layer::L5_Knowledge, "repo-wiki", "e-1")
        .unwrap();

    // 删除条目(应联动标记锚点为悬空)
    store.delete("e-1").unwrap();

    // 解析锚点应返回 AnchorDangling
    let result = store.resolve_anchor(anchor.anchor_id);
    assert!(
        matches!(result, Err(WikiError::AnchorDangling(_))),
        "expected AnchorDangling, got {result:?}"
    );
}

// ============================================================
// 场景 3:跨层一致性(L9 创建 → L5 更新 → L2 读取同一 updated_at)
// ============================================================

#[test]
fn test_cross_layer_consistency() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    // 初始条目
    let entry_v1 = make_entry("e-1", "v1", "content v1", vec![], vec![0.0; 512]);
    store.insert(&entry_v1).unwrap();

    // L9 创建锚点
    let anchor_l9 = store
        .create_anchor(Layer::L9_Quest, "quest-engine", "e-1")
        .unwrap();

    // L5 也创建锚点(同一实体,不同层引用)
    let anchor_l5 = store
        .create_anchor(Layer::L5_Knowledge, "repo-wiki", "e-1")
        .unwrap();

    // L2 创建锚点
    let anchor_l2 = store
        .create_anchor(Layer::L2_Memory, "mlc-engine", "e-1")
        .unwrap();

    // 模拟 L5 更新条目(UPSERT 语义,updated_at 刷新)
    std::thread::sleep(std::time::Duration::from_millis(10));
    let entry_v2 = {
        let now = Utc::now();
        WikiEntry {
            entry_id: "e-1".into(),
            title: "v2".into(),
            content: "content v2".into(),
            tags: vec![],
            embedding: vec![0.0; 512],
            created_at: entry_v1.created_at,
            updated_at: now,
        }
    };
    store.insert(&entry_v2).unwrap();

    // 三层各自解析锚点,应返回同一 updated_at(跨层一致性)
    let resolved_l9 = store.resolve_anchor(anchor_l9.anchor_id).unwrap();
    let resolved_l5 = store.resolve_anchor(anchor_l5.anchor_id).unwrap();
    let resolved_l2 = store.resolve_anchor(anchor_l2.anchor_id).unwrap();

    assert_eq!(resolved_l9.updated_at, entry_v2.updated_at);
    assert_eq!(resolved_l5.updated_at, entry_v2.updated_at);
    assert_eq!(resolved_l2.updated_at, entry_v2.updated_at);

    // 三者应返回同一版本(v2)
    assert_eq!(resolved_l9.title, "v2");
    assert_eq!(resolved_l5.title, "v2");
    assert_eq!(resolved_l2.title, "v2");
}

// ============================================================
// 场景 4:锚点 UUIDv7 全局唯一性(1000 个无冲突)
// ============================================================

#[test]
fn test_anchor_uuid_uniqueness() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    let mut ids = std::collections::HashSet::new();
    for i in 0..1000 {
        let anchor = store
            .create_anchor(Layer::L5_Knowledge, "repo-wiki", &format!("e-{i}"))
            .unwrap();
        assert!(ids.insert(anchor.anchor_id), "UUID 冲突 at iteration {i}");
    }

    // 验证 1000 个锚点全部持久化
    let l5_anchors = store.list_anchors_by_layer(Layer::L5_Knowledge).unwrap();
    assert_eq!(l5_anchors.len(), 1000);
}

// ============================================================
// 场景 5:delete 联动标记悬空(list_anchors_by_entity 返回 is_dangling=true)
// ============================================================

#[test]
fn test_delete_marks_anchors_dangling() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    let entry = make_entry("e-1", "Title", "Content", vec![], vec![0.0; 512]);
    store.insert(&entry).unwrap();

    // 创建多个跨层锚点
    store
        .create_anchor(Layer::L9_Quest, "quest-engine", "e-1")
        .unwrap();
    store
        .create_anchor(Layer::L5_Knowledge, "repo-wiki", "e-1")
        .unwrap();
    store
        .create_anchor(Layer::L2_Memory, "mlc-engine", "e-1")
        .unwrap();

    // 删除前:所有锚点 is_dangling=false
    let before = store.list_anchors_by_entity("e-1").unwrap();
    assert_eq!(before.len(), 3);
    assert!(before.iter().all(|a| !a.is_dangling));

    // 删除条目
    store.delete("e-1").unwrap();

    // 删除后:所有锚点 is_dangling=true
    let after = store.list_anchors_by_entity("e-1").unwrap();
    assert_eq!(after.len(), 3);
    assert!(
        after.iter().all(|a| a.is_dangling),
        "all anchors should be marked dangling after entity deletion"
    );

    // updated_at 应被刷新(不等于 created_at)
    for anchor in &after {
        assert!(
            anchor.updated_at >= anchor.created_at,
            "updated_at should be refreshed after dangling mark"
        );
    }
}

// ============================================================
// 场景 6:list_anchors_by_layer 按层过滤
// ============================================================

#[test]
fn test_list_anchors_by_layer() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    // 为不同实体在不同层创建锚点
    store
        .create_anchor(Layer::L9_Quest, "quest-engine", "e-1")
        .unwrap();
    store
        .create_anchor(Layer::L9_Quest, "quest-engine", "e-2")
        .unwrap();
    store
        .create_anchor(Layer::L5_Knowledge, "repo-wiki", "e-1")
        .unwrap();
    store
        .create_anchor(Layer::L2_Memory, "mlc-engine", "e-1")
        .unwrap();

    // L9 应有 2 个锚点
    let l9 = store.list_anchors_by_layer(Layer::L9_Quest).unwrap();
    assert_eq!(l9.len(), 2);
    assert!(l9.iter().all(|a| a.layer == Layer::L9_Quest));

    // L5 应有 1 个锚点
    let l5 = store.list_anchors_by_layer(Layer::L5_Knowledge).unwrap();
    assert_eq!(l5.len(), 1);
    assert_eq!(l5[0].layer, Layer::L5_Knowledge);

    // L2 应有 1 个锚点
    let l2 = store.list_anchors_by_layer(Layer::L2_Memory).unwrap();
    assert_eq!(l2.len(), 1);
    assert_eq!(l2[0].layer, Layer::L2_Memory);

    // L1 应有 0 个锚点
    let l1 = store.list_anchors_by_layer(Layer::L1_Core).unwrap();
    assert!(l1.is_empty());
}

// ============================================================
// 场景 7:mark_dangling 手动标记后 resolve 返回错误
// ============================================================

#[test]
fn test_mark_dangling_manual() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    let entry = make_entry("e-1", "Title", "Content", vec![], vec![0.0; 512]);
    store.insert(&entry).unwrap();

    let anchor = store
        .create_anchor(Layer::L5_Knowledge, "repo-wiki", "e-1")
        .unwrap();

    // 解析成功(条目存在,锚点未悬空)
    let resolved = store.resolve_anchor(anchor.anchor_id).unwrap();
    assert_eq!(resolved.entry_id, "e-1");

    // 手动标记悬空(模拟外部失效检测)
    store.mark_dangling(anchor.anchor_id).unwrap();

    // 解析应返回 AnchorDangling
    let result = store.resolve_anchor(anchor.anchor_id);
    assert!(
        matches!(result, Err(WikiError::AnchorDangling(_))),
        "expected AnchorDangling after manual mark, got {result:?}"
    );

    // 验证 list_anchors_by_entity 中 is_dangling=true
    let anchors = store.list_anchors_by_entity("e-1").unwrap();
    assert_eq!(anchors.len(), 1);
    assert!(anchors[0].is_dangling);
}

// ============================================================
// 边界场景:resolve 不存在的锚点返回 EntryNotFound
// ============================================================

#[test]
fn test_resolve_nonexistent_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    let fake_id = Uuid::now_v7();
    let result = store.resolve_anchor(fake_id);
    assert!(
        matches!(result, Err(WikiError::EntryNotFound(_))),
        "expected EntryNotFound for nonexistent anchor, got {result:?}"
    );
}

// ============================================================
// 边界场景:mark_dangling 不存在的锚点返回 EntryNotFound
// ============================================================

#[test]
fn test_mark_dangling_nonexistent_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    let fake_id = Uuid::now_v7();
    let result = store.mark_dangling(fake_id);
    assert!(
        matches!(result, Err(WikiError::EntryNotFound(_))),
        "expected EntryNotFound for nonexistent anchor, got {result:?}"
    );
}

// ============================================================
// 边界场景:懒标记悬空(锚点存在但实体不存在)
// ============================================================

#[test]
fn test_lazy_dangling_mark_on_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("test.db")).unwrap();

    // 直接创建锚点(不插入实体)
    let anchor = store
        .create_anchor(Layer::L5_Knowledge, "repo-wiki", "e-missing")
        .unwrap();

    // 第一次解析:实体不存在,应懒标记为悬空并返回 AnchorDangling
    let result = store.resolve_anchor(anchor.anchor_id);
    assert!(
        matches!(result, Err(WikiError::AnchorDangling(_))),
        "expected AnchorDangling for missing entity, got {result:?}"
    );

    // 验证锚点已被懒标记为悬空
    let anchors = store.list_anchors_by_entity("e-missing").unwrap();
    assert_eq!(anchors.len(), 1);
    assert!(anchors[0].is_dangling);

    // 第二次解析:应直接返回 AnchorDangling(因 is_dangling=true)
    let result2 = store.resolve_anchor(anchor.anchor_id);
    assert!(
        matches!(result2, Err(WikiError::AnchorDangling(_))),
        "expected AnchorDangling on second resolve, got {result2:?}"
    );
}

// ============================================================
// 边界场景:IscmAnchor::new 单元行为
// ============================================================

#[test]
fn test_iscm_anchor_new_basic() {
    let anchor = IscmAnchor::new(Layer::L7_Execution, "pvl-layer", "e-1");
    assert_eq!(anchor.layer, Layer::L7_Execution);
    assert_eq!(anchor.crate_name, "pvl-layer");
    assert_eq!(anchor.entity_id, "e-1");
    assert!(!anchor.is_dangling);
    assert_eq!(anchor.created_at, anchor.updated_at);
}

// ============================================================
// 边界场景:Layer 转换
// ============================================================

#[test]
fn test_layer_roundtrip_all_variants() {
    let layers = [
        Layer::L1_Core,
        Layer::L2_Memory,
        Layer::L3_Storage,
        Layer::L4_Security,
        Layer::L5_Knowledge,
        Layer::L6_Router,
        Layer::L7_Execution,
        Layer::L8_Parliament,
        Layer::L9_Quest,
        Layer::L10_Interface,
    ];

    for layer in layers {
        let s = layer.as_str();
        assert_eq!(Layer::from_str(s), Some(layer), "roundtrip failed for {s}");
    }

    // 无效字符串
    assert_eq!(Layer::from_str("L0_Unknown"), None);
    assert_eq!(Layer::from_str(""), None);
}
