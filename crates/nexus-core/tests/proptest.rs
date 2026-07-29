//! nexus-core 不变量属性测试 - CLV / UserIntent / Quest 序列化与数学不变量
//!
//! 对应任务:Week 8 Task 18 SubTask 18.1
//! 架构层:L1 Core
//!
//! # 测试目标
//! - CLV 512-dim 余弦相似性不变量:自相似 ~ 1.0、值域 [-1, 1]、对称性
//! - UserIntent risk_level 边界:u8 (0-100) 序列化往返不变
//! - Quest rmp-serde 序列化往返:任意 Quest 经 MessagePack 序列化再反序列化应相等
//!
//! # 语法约束(§4.4 规则)
//! proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`
//!
//! # 序列化协议(ADR-004)
//! MessagePack(rmp-serde)为跨层通信与持久化协议

#![forbid(unsafe_code)]

use nexus_core::{
    cosine_similarity_slices, MultimodalInput, Quest, Task, TaskStatus, ThinkingMode, UserIntent,
    CLV,
};
use proptest::prelude::*;

proptest! {
    #[test]
    fn clv_dimension_always_512(vec in prop::collection::vec(-1.0f32..1.0f32, 512)) {
        let clv = CLV::from_vec(vec)?;
        prop_assert_eq!(clv.as_slice().len(), 512);
        prop_assert_eq!(CLV::DIMENSION, 512);
    }
}

proptest! {
    #[test]
    fn clv_self_similarity_approx_one(
        vec in prop::collection::vec(-1.0f32..1.0f32, 512)
            .prop_filter("non-zero", |v| v.iter().any(|&x| x != 0.0))
    ) {
        let clv = CLV::from_vec(vec)?;
        let sim = clv.cosine_similarity(&clv);
        prop_assert!((sim - 1.0).abs() < 1e-4, "self-similarity should be ~1.0, got {}", sim);
    }
}

proptest! {
    #[test]
    fn clv_similarity_in_range(
        a in prop::collection::vec(-1.0f32..1.0f32, 512)
            .prop_filter("a-non-zero", |v| v.iter().any(|&x| x != 0.0)),
        b in prop::collection::vec(-1.0f32..1.0f32, 512)
            .prop_filter("b-non-zero", |v| v.iter().any(|&x| x != 0.0))
    ) {
        let clv_a = CLV::from_vec(a)?;
        let clv_b = CLV::from_vec(b)?;
        let sim = clv_a.cosine_similarity(&clv_b);
        prop_assert!(sim.is_finite() && (-1.0 - 1e-4..=1.0 + 1e-4).contains(&sim), "similarity out of [-1,1]: {}", sim);
    }
}

proptest! {
    #[test]
    fn clv_similarity_symmetric(
        a in prop::collection::vec(-1.0f32..1.0f32, 512)
            .prop_filter("a-non-zero", |v| v.iter().any(|&x| x != 0.0)),
        b in prop::collection::vec(-1.0f32..1.0f32, 512)
            .prop_filter("b-non-zero", |v| v.iter().any(|&x| x != 0.0))
    ) {
        let clv_a = CLV::from_vec(a)?;
        let clv_b = CLV::from_vec(b)?;
        let sim_ab = clv_a.cosine_similarity(&clv_b);
        let sim_ba = clv_b.cosine_similarity(&clv_a);
        prop_assert!((sim_ab - sim_ba).abs() < 1e-4, "similarity should be symmetric");
    }
}

proptest! {
    #[test]
    fn clv_zero_vector_returns_zero_similarity(
        vec in prop::collection::vec(-1.0f32..1.0f32, 512)
            .prop_filter("non-zero", |v| v.iter().any(|&x| x != 0.0))
    ) {
        let zero = CLV::zero();
        let other = CLV::from_vec(vec)?;
        prop_assert_eq!(zero.cosine_similarity(&other), 0.0);
        prop_assert_eq!(other.cosine_similarity(&zero), 0.0);
    }
}

proptest! {
    #[test]
    fn slices_self_similarity_approx_one(
        vec in prop::collection::vec(-1.0f32..1.0f32, 100)
            .prop_filter("non-zero", |v| v.iter().any(|&x| x != 0.0))
    ) {
        let sim = cosine_similarity_slices(&vec, &vec);
        prop_assert!((sim - 1.0).abs() < 1e-4, "slices self-similarity should be ~1.0, got {}", sim);
    }
}

proptest! {
    #[test]
    fn slices_clamped_to_unit_range(
        a in prop::collection::vec(-1.0f32..1.0f32, 100)
            .prop_filter("a-non-zero", |v| v.iter().any(|&x| x != 0.0)),
        b in prop::collection::vec(-1.0f32..1.0f32, 100)
            .prop_filter("b-non-zero", |v| v.iter().any(|&x| x != 0.0))
    ) {
        let sim = cosine_similarity_slices(&a, &b);
        prop_assert!(sim.is_finite() && (-1.0..=1.0).contains(&sim), "slices out of [-1,1]: {}", sim);
    }
}

proptest! {
    #[test]
    fn clv_msgpack_roundtrip(vec in prop::collection::vec(-1.0f32..1.0f32, 512)) {
        let clv = CLV::from_vec(vec)?;
        let bytes = rmp_serde::to_vec(&clv).expect("msgpack serialize");
        let decoded: CLV = rmp_serde::from_slice(&bytes).expect("msgpack deserialize");
        prop_assert_eq!(clv, decoded, "CLV should roundtrip through MessagePack");
    }
}

proptest! {
    #[test]
    fn user_intent_risk_level_roundtrip(
        risk_level in 0u8..=100u8,
        text in ".{0,100}"
    ) {
        let intent = UserIntent {
            intent_id: format!("intent-{}", risk_level),
            raw_text: text.clone(),
            multimodal_inputs: vec![MultimodalInput::Text(text)],
            risk_level,
        };
        let bytes = rmp_serde::to_vec(&intent).expect("msgpack serialize");
        let decoded: UserIntent = rmp_serde::from_slice(&bytes).expect("msgpack deserialize");
        prop_assert_eq!(intent, decoded.clone(), "UserIntent should roundtrip through MessagePack");
        prop_assert!(decoded.risk_level <= 100, "risk_level should be <= 100");
    }
}

proptest! {
    #[test]
    fn user_intent_risk_level_boundary_values(risk_level in 0u8..=100u8) {
        let intent = UserIntent {
            intent_id: "boundary-test".to_string(),
            raw_text: String::new(),
            multimodal_inputs: vec![],
            risk_level,
        };
        let json = serde_json::to_string(&intent).expect("json serialize");
        let decoded: UserIntent = serde_json::from_str(&json).expect("json deserialize");
        prop_assert_eq!(intent.risk_level, decoded.risk_level);
        prop_assert_eq!(intent, decoded.clone(), "UserIntent should roundtrip through JSON");
    }
}

proptest! {
    #[test]
    fn quest_msgpack_roundtrip(
        quest_id in "[a-z0-9]{1,20}",
        title in ".{0,50}",
        task_count in 0u8..=5u8,
        thinking_mode_idx in 0u8..=2u8,
        has_checkpoint in proptest::bool::ANY
    ) {
        let thinking_mode = match thinking_mode_idx {
            0 => ThinkingMode::Fast,
            1 => ThinkingMode::Standard,
            _ => ThinkingMode::Deep,
        };
        let tasks: Vec<Task> = (0..task_count)
            .map(|i| Task {
                task_id: format!("task-{i}"),
                description: format!("desc-{i}"),
                status: TaskStatus::Pending,
                dependencies: vec![],
            })
            .collect();
        let quest = Quest {
            quest_id: quest_id.clone(),
            title,
            tasks,
            thinking_mode,
            checkpoint_id: if has_checkpoint { Some(format!("cp-{quest_id}")) } else { None },
            priority: 128,
        };
        let bytes = rmp_serde::to_vec(&quest).expect("msgpack serialize quest");
        let decoded: Quest = rmp_serde::from_slice(&bytes).expect("msgpack deserialize quest");
        prop_assert_eq!(quest, decoded, "Quest should roundtrip through MessagePack");
    }
}

// ================================================================
// proptest: cosine_similarity_slices 补充不变量(T6-6)
//
// 已有测试覆盖了 CLV 层面的对称性/自相似性/值域,
// 此处补充 slices API 层面的边界性质:
// 1. 空切片返回 0.0(避免除零 panic)
// 2. 不等长切片取 min 长度(兼容行为)
// 3. slices 层面的对称性(独立于 CLV 方法)
// ================================================================

proptest! {
    /// 不变量: 空切片输入返回 0.0(不 panic,不返回 NaN)
    ///
    /// WHY: cosine_similarity_slices 对空输入返回 0.0 是防御性设计,
    /// 避免除零 panic。proptest 验证任意长度 a + 空 b 均返回 0.0。
    #[test]
    fn slices_empty_input_returns_zero(
        a in prop::collection::vec(-1.0f32..1.0f32, 0..50)
    ) {
        let empty: Vec<f32> = vec![];
        prop_assert_eq!(cosine_similarity_slices(&a, &empty), 0.0);
        prop_assert_eq!(cosine_similarity_slices(&empty, &a), 0.0);
        prop_assert_eq!(cosine_similarity_slices(&empty, &empty), 0.0);
    }
}

proptest! {
    /// 不变量: 不等长切片取 min 长度 — cosine_similarity_slices(a, b)
    /// 等价于 cosine_similarity_slices(a[..min_len], b[..min_len])
    ///
    /// WHY: 不等长输入是 API 文档明确声明的兼容行为(取最小长度),
    /// proptest 验证此语义在任意长度组合下一致。
    #[test]
    fn slices_unequal_length_uses_min(
        len_a in 1usize..20,
        len_b in 1usize..20,
    ) {
        let a: Vec<f32> = (0..len_a).map(|i| (i as f32) * 0.1).collect();
        let b: Vec<f32> = (0..len_b).map(|i| (i as f32) * 0.05 + 0.1).collect();
        let min_len = len_a.min(len_b);

        let sim_full = cosine_similarity_slices(&a, &b);
        let sim_truncated = cosine_similarity_slices(&a[..min_len], &b[..min_len]);
        prop_assert!(
            (sim_full - sim_truncated).abs() < 1e-6,
            "unequal length: full={} should equal truncated={} (len_a={}, len_b={})",
            sim_full, sim_truncated, len_a, len_b
        );
    }
}

proptest! {
    /// 不变量: cosine_similarity_slices 对称性 — cos(a,b) == cos(b,a)
    ///
    /// 独立于 CLV 层面的对称性测试,直接验证 slices API。
    #[test]
    fn slices_symmetry(
        a in prop::collection::vec(-1.0f32..1.0f32, 1..50)
            .prop_filter("a-non-zero", |v| v.iter().any(|&x| x != 0.0)),
        b in prop::collection::vec(-1.0f32..1.0f32, 1..50)
            .prop_filter("b-non-zero", |v| v.iter().any(|&x| x != 0.0))
    ) {
        let sim_ab = cosine_similarity_slices(&a, &b);
        let sim_ba = cosine_similarity_slices(&b, &a);
        prop_assert!(
            (sim_ab - sim_ba).abs() < 1e-6,
            "slices symmetry: cos(a,b)={} != cos(b,a)={}",
            sim_ab, sim_ba
        );
    }
}
