//! nexus-core 集成测试 — 覆盖 CLV、NexusState、领域类型的完整往返
//!
//! 测试矩阵:
//! - CLV:维度校验、零向量边界、余弦相似度(相同/正交/零向量)
//! - NexusState:注册/查询/重复检测/快照哈希/并发安全
//! - 领域类型:serde_json 序列化反序列化往返
//! - Checkpoint:created_at 自动生成

use nexus_core::{
    Checkpoint, MultimodalInput, NexusError, NexusState, Quest, Task, TaskStatus, ThinkingMode,
    UserIntent, CLV,
};

// ============================================================
// 辅助构造函数
// ============================================================

fn make_quest(id: &str) -> Quest {
    Quest {
        quest_id: id.to_string(),
        title: format!("Test Quest {id}"),
        tasks: vec![
            Task {
                task_id: format!("{id}-t1"),
                description: "first task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            },
            Task {
                task_id: format!("{id}-t2"),
                description: "second task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![format!("{id}-t1")],
            },
        ],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    }
}

fn make_user_intent(id: &str) -> UserIntent {
    UserIntent {
        intent_id: id.to_string(),
        raw_text: format!("hello world from {id}"),
        multimodal_inputs: vec![MultimodalInput::Text("text input".into())],
        risk_level: 30,
    }
}

fn make_checkpoint(quest_id: &str, cp_id: &str) -> Checkpoint {
    Checkpoint::new(
        quest_id,
        cp_id,
        "abc123def456",
        vec![0x01, 0x02, 0x03, 0x04],
    )
}

// ============================================================
// CLV 测试
// ============================================================

#[test]
fn test_clv_zero_dimension_correct() {
    let clv = CLV::zero();
    assert_eq!(CLV::dimension(), 512);
    assert_eq!(clv.as_slice().len(), 512);
    assert!(clv.as_slice().iter().all(|&v| v == 0.0));
}

#[test]
fn test_clv_from_vec_valid_dimension() {
    let v = vec![0.42_f32; 512];
    let clv = CLV::from_vec(v).unwrap();
    assert_eq!(clv.as_slice().len(), 512);
    assert!(clv.as_slice().iter().all(|&v| (v - 0.42).abs() < 1e-6));
}

#[test]
fn test_clv_from_vec_invalid_dimension() {
    let v = vec![0.0_f32; 256];
    let result = CLV::from_vec(v);
    assert!(matches!(
        result,
        Err(NexusError::InvalidClvDimension {
            expected: 512,
            actual: 256
        })
    ));
}

#[test]
fn test_clv_cosine_similarity_identical_vectors() {
    let mut v = vec![0.0_f32; 512];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = (i as f32) * 0.01;
    }
    let clv = CLV::from_vec(v).unwrap();
    let sim = clv.cosine_similarity(&clv);
    // 相同向量余弦相似度 = 1.0(容忍浮点误差)
    assert!(
        (sim - 1.0).abs() < 1e-5,
        "identical vectors should have similarity ~1.0, got {sim}"
    );
}

#[test]
fn test_clv_cosine_similarity_orthogonal_vectors() {
    // 前半非零 vs 后半非零 → 点积为 0 → 余弦相似度为 0
    let mut v1 = vec![0.0_f32; 512];
    let mut v2 = vec![0.0_f32; 512];
    for i in 0..256 {
        v1[i] = 1.0;
        v2[256 + i] = 1.0;
    }
    let clv1 = CLV::from_vec(v1).unwrap();
    let clv2 = CLV::from_vec(v2).unwrap();
    let sim = clv1.cosine_similarity(&clv2);
    assert!(
        sim.abs() < 1e-6,
        "orthogonal vectors should have similarity ~0.0, got {sim}"
    );
}

#[test]
fn test_clv_cosine_similarity_zero_vector() {
    let zero = CLV::zero();
    let mut v = vec![1.0_f32; 512];
    v[0] = 2.0;
    let nonzero = CLV::from_vec(v).unwrap();

    // 零向量与任意向量:返回 0.0(非 NaN)
    assert_eq!(zero.cosine_similarity(&nonzero), 0.0);
    // 零向量与零向量:返回 0.0
    assert_eq!(zero.cosine_similarity(&zero), 0.0);
    // 任意向量与零向量:返回 0.0
    assert_eq!(nonzero.cosine_similarity(&zero), 0.0);
}

#[test]
fn test_clv_serde_roundtrip() {
    let mut v = vec![0.0_f32; 512];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = (i as f32) * 0.1;
    }
    let original = CLV::from_vec(v).unwrap();
    let json = serde_json::to_string(&original).unwrap();
    let restored: CLV = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// ============================================================
// NexusState 测试
// ============================================================

#[test]
fn test_nexus_state_new_empty() {
    let state = NexusState::new();
    assert!(state.list_quests().is_empty());
}

#[test]
fn test_nexus_state_register_and_get_quest() {
    let state = NexusState::new();
    let quest = make_quest("q1");
    state.register_quest(quest.clone()).unwrap();

    let retrieved = state.get_quest("q1").unwrap();
    assert_eq!(retrieved.quest_id, "q1");
    assert_eq!(retrieved.title, "Test Quest q1");
    assert_eq!(retrieved.tasks.len(), 2);
    assert_eq!(retrieved.thinking_mode, ThinkingMode::Standard);
}

#[test]
fn test_nexus_state_get_nonexistent_quest() {
    let state = NexusState::new();
    assert!(state.get_quest("nonexistent").is_none());
}

#[test]
fn test_nexus_state_register_duplicate_quest() {
    let state = NexusState::new();
    state.register_quest(make_quest("q1")).unwrap();

    let result = state.register_quest(make_quest("q1"));
    assert!(matches!(result, Err(NexusError::QuestAlreadyExists(_))));
}

#[test]
fn test_nexus_state_snapshot_hash_is_64_hex() {
    let state = NexusState::new();
    let hash = state.snapshot_hash();
    assert_eq!(hash.len(), 64, "SHA-256 hex should be 64 chars");
    assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "hash should be lowercase hex: {hash}"
    );
}

#[test]
fn test_nexus_state_snapshot_hash_changes() {
    let state = NexusState::new();
    let hash_before = state.snapshot_hash();

    state.register_quest(make_quest("q1")).unwrap();
    let hash_after = state.snapshot_hash();

    assert_ne!(
        hash_before, hash_after,
        "hash must change after state mutation"
    );
}

#[test]
fn test_nexus_state_snapshot_hash_deterministic() {
    // 两个独立状态,以不同顺序注册相同 Quest → 哈希应一致
    let state1 = NexusState::new();
    state1.register_quest(make_quest("q1")).unwrap();
    state1.register_quest(make_quest("q2")).unwrap();

    let state2 = NexusState::new();
    state2.register_quest(make_quest("q2")).unwrap();
    state2.register_quest(make_quest("q1")).unwrap();

    assert_eq!(
        state1.snapshot_hash(),
        state2.snapshot_hash(),
        "hash should be deterministic regardless of insertion order"
    );
}

#[test]
fn test_nexus_state_concurrent_register() {
    let state = NexusState::new();
    let state1 = state.clone();
    let state2 = state.clone();

    let quest_id = "concurrent-quest".to_string();
    let quest_id2 = quest_id.clone();

    let h1 = std::thread::spawn(move || state1.register_quest(make_quest(&quest_id)));
    let h2 = std::thread::spawn(move || state2.register_quest(make_quest(&quest_id2)));

    let r1 = h1.join().unwrap();
    let r2 = h2.join().unwrap();

    // 恰好一个成功,另一个返回 QuestAlreadyExists
    let ok_count = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
    assert_eq!(ok_count, 1, "exactly one thread should succeed");

    if let Err(e) = &r1 {
        assert!(
            matches!(e, NexusError::QuestAlreadyExists(_)),
            "expected QuestAlreadyExists, got {e:?}"
        );
    }
    if let Err(e) = &r2 {
        assert!(
            matches!(e, NexusError::QuestAlreadyExists(_)),
            "expected QuestAlreadyExists, got {e:?}"
        );
    }

    // 最终状态:恰好一个 Quest 注册成功
    assert_eq!(state.list_quests().len(), 1);
}

#[test]
fn test_nexus_state_list_quests() {
    let state = NexusState::new();
    state.register_quest(make_quest("q1")).unwrap();
    state.register_quest(make_quest("q2")).unwrap();
    state.register_quest(make_quest("q3")).unwrap();

    let mut ids: Vec<_> = state
        .list_quests()
        .into_iter()
        .map(|q| q.quest_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["q1", "q2", "q3"]);
}

#[test]
fn test_nexus_state_update_progress_nonexistent() {
    let state = NexusState::new();
    let result = state.update_quest_progress("nonexistent", 0, 10);
    assert!(matches!(result, Err(NexusError::QuestNotFound(_))));
}

#[test]
fn test_nexus_state_update_progress_existing() {
    let state = NexusState::new();
    state.register_quest(make_quest("q1")).unwrap();
    let result = state.update_quest_progress("q1", 5, 10);
    assert!(result.is_ok());
}

#[test]
fn test_nexus_state_clone_shares_state() {
    let state = NexusState::new();
    let cloned = state.clone();
    // 克隆共享底层 Arc,注册在克隆上做,原 state 也能看到
    cloned.register_quest(make_quest("q1")).unwrap();
    assert_eq!(state.list_quests().len(), 1);
}

// ============================================================
// 领域类型序列化往返测试
// ============================================================

#[test]
fn test_user_intent_serde_roundtrip() {
    let original = make_user_intent("intent-1");
    let json = serde_json::to_string(&original).unwrap();
    let restored: UserIntent = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn test_quest_serde_roundtrip() {
    let original = make_quest("q-serde");
    let json = serde_json::to_string(&original).unwrap();
    let restored: Quest = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn test_task_serde_roundtrip() {
    let original = Task {
        task_id: "t-1".into(),
        description: "test task".into(),
        status: TaskStatus::Running,
        dependencies: vec!["t-0".into()],
    };
    let json = serde_json::to_string(&original).unwrap();
    let restored: Task = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn test_checkpoint_serde_roundtrip() {
    let original = make_checkpoint("q-1", "cp-1");
    let json = serde_json::to_string(&original).unwrap();
    let restored: Checkpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn test_checkpoint_created_at_auto_generated() {
    let before = chrono::Utc::now();
    let cp = make_checkpoint("q-1", "cp-1");
    let after = chrono::Utc::now();

    // created_at 应在构造前后之间
    assert!(cp.created_at >= before, "created_at should be >= before");
    assert!(cp.created_at <= after, "created_at should be <= after");
}

#[test]
fn test_multimodal_input_serde_roundtrip() {
    let original = MultimodalInput::Text("hello world".into());
    let json = serde_json::to_string(&original).unwrap();
    let restored: MultimodalInput = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn test_task_status_serde_roundtrip() {
    for status in [
        TaskStatus::Pending,
        TaskStatus::Running,
        TaskStatus::Completed,
        TaskStatus::Failed,
    ] {
        let json = serde_json::to_string(&status).unwrap();
        let restored: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, restored);
    }
}

#[test]
fn test_thinking_mode_serde_roundtrip() {
    for mode in [
        ThinkingMode::Fast,
        ThinkingMode::Standard,
        ThinkingMode::Deep,
    ] {
        let json = serde_json::to_string(&mode).unwrap();
        let restored: ThinkingMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, restored);
    }
}
