//! GEA 门控专家激活属性测试 — 验证门控值与 Top-K 不变量
//!
//! WHY 文件名 property_tests(非 proptest):Cargo 会为 tests/<name>.rs 生成同名测试二进制,
//! 若文件名为 proptest.rs,则 `use proptest::prelude::*;` 会解析到测试二进制自身而非外部 crate

#![forbid(unsafe_code)]

use std::collections::HashMap;

use gea_activator::{
    compute_gate_value, resolve_conflicts, Candidate, ExpertId, ExpertProfile, GeaConfig,
    TaskProfile,
};
use proptest::prelude::*;

fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

fn prop_unit_f32() -> impl Strategy<Value = f32> {
    any::<f32>().prop_map(|v| {
        if v.is_nan() || v.is_infinite() {
            0.5
        } else {
            v.abs().rem_euclid(1.0)
        }
    })
}

fn prop_vector_64() -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(prop_unit_f32(), 64)
}

fn prop_valid_config() -> impl Strategy<Value = GeaConfig> {
    (prop_unit_f32(), prop_unit_f32(), prop_unit_f32()).prop_map(|(w1_raw, w2_raw, w3_raw)| {
        let sum = w1_raw + w2_raw + w3_raw;
        let (w1, w2, w3) = if sum > 0.0 {
            (w1_raw / sum, w2_raw / sum, w3_raw / sum)
        } else {
            (0.4, 0.3, 0.3)
        };
        GeaConfig {
            w1,
            w2,
            w3,
            bias: 0.5,
            activation_threshold: 0.5,
            cache_capacity: 128,
            overlap_threshold: 0.8,
            top_k: 3,
            cache_ttl_secs: 5,
        }
    })
}

fn make_orthogonal(idx: usize) -> Vec<f32> {
    let mut v = vec![0.0; 64];
    if idx < 64 {
        v[idx] = 1.0;
    }
    v
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn test_gate_value_in_unit_interval(complexity in prop_unit_f32(), priority in prop_unit_f32(), config in prop_valid_config()) {
        let task = TaskProfile::new(complexity, "code-gen", 30, vec![0.5; 64]);
        let expert = ExpertProfile::new("e-1", vec![0.5; 64], priority, vec!["code-gen".into()]);
        let gate = compute_gate_value(&task, &expert, &config);
        prop_assert!((0.0..=1.0).contains(&gate), "gate {} out of [0,1]", gate);
        prop_assert!(gate.is_finite(), "gate must be finite");
    }

    #[test]
    fn test_gate_value_zero_vectors_finite(complexity in prop_unit_f32(), config in prop_valid_config()) {
        let task = TaskProfile::new(complexity, "test", 30, vec![0.0; 64]);
        let expert = ExpertProfile::new("e-1", vec![0.0; 64], 0.5, vec![]);
        let gate = compute_gate_value(&task, &expert, &config);
        prop_assert!(gate.is_finite(), "zero-vector gate must be finite");
        prop_assert!((0.0..=1.0).contains(&gate), "zero-vector gate {} out of [0,1]", gate);
    }

    #[test]
    fn test_gate_value_dim_mismatch_in_range(complexity in prop_unit_f32(), task_clv_512 in prop::collection::vec(prop_unit_f32(), 512), expert_vector_64 in prop_vector_64(), config in prop_valid_config()) {
        let task = TaskProfile::new(complexity, "test", 30, task_clv_512);
        let expert = ExpertProfile::new("e-1", expert_vector_64, 0.5, vec![]);
        let gate = compute_gate_value(&task, &expert, &config);
        prop_assert!(gate.is_finite());
        prop_assert!((0.0..=1.0).contains(&gate), "dim mismatch gate {} out of [0,1]", gate);
    }

    #[test]
    fn test_top_k_never_exceeds_k(n_candidates in 1usize..=20, top_k in 1usize..=10) {
        let mut profiles = HashMap::new();
        for i in 0..n_candidates {
            let id = ExpertId::new(format!("e-{i}"));
            profiles.insert(id, ExpertProfile::new(format!("e-{i}"), make_orthogonal(i % 64), 0.5, vec![]));
        }
        let candidates: Vec<Candidate> = (0..n_candidates).map(|i| (ExpertId::new(format!("e-{i}")), 0.9 - (i as f32 * 0.01))).collect();
        let config = GeaConfig { top_k, ..Default::default() };
        let result = resolve_conflicts(candidates, &profiles, &config).map_err(fail)?;
        prop_assert!(result.activated.len() <= top_k, "activated {} exceeds top_k={}", result.activated.len(), top_k);
    }

    #[test]
    fn test_top_k_one_activates_single(n_candidates in 1usize..=15) {
        let mut profiles = HashMap::new();
        for i in 0..n_candidates {
            profiles.insert(ExpertId::new(format!("e-{i}")), ExpertProfile::new(format!("e-{i}"), make_orthogonal(i % 64), 0.5, vec![]));
        }
        let candidates: Vec<Candidate> = (0..n_candidates).map(|i| (ExpertId::new(format!("e-{i}")), 0.8)).collect();
        let config = GeaConfig { top_k: 1, ..Default::default() };
        let result = resolve_conflicts(candidates, &profiles, &config).map_err(fail)?;
        prop_assert_eq!(result.activated.len(), 1, "top_k=1 should activate 1");
    }

    #[test]
    fn test_fewer_candidates_than_top_k_all_activated(n_candidates in 1usize..=5, top_k in 6usize..=20) {
        let mut profiles = HashMap::new();
        for i in 0..n_candidates {
            profiles.insert(ExpertId::new(format!("e-{i}")), ExpertProfile::new(format!("e-{i}"), make_orthogonal(i % 64), 0.5, vec![]));
        }
        let candidates: Vec<Candidate> = (0..n_candidates).map(|i| (ExpertId::new(format!("e-{i}")), 0.8)).collect();
        let config = GeaConfig { top_k, ..Default::default() };
        let result = resolve_conflicts(candidates, &profiles, &config).map_err(fail)?;
        prop_assert_eq!(result.activated.len(), n_candidates, "fewer candidates should activate all");
    }

    #[test]
    fn test_activated_plus_suppressed_equals_total(n_candidates in 1usize..=20, top_k in 1usize..=10) {
        let mut profiles = HashMap::new();
        for i in 0..n_candidates {
            profiles.insert(ExpertId::new(format!("e-{i}")), ExpertProfile::new(format!("e-{i}"), make_orthogonal(i % 64), 0.5, vec![]));
        }
        let candidates: Vec<Candidate> = (0..n_candidates).map(|i| (ExpertId::new(format!("e-{i}")), 0.8)).collect();
        let config = GeaConfig { top_k, ..Default::default() };
        let result = resolve_conflicts(candidates, &profiles, &config).map_err(fail)?;
        let total = result.activated.len() + result.suppressed.len();
        prop_assert_eq!(total, n_candidates, "activated + suppressed != total");
    }
}

// WHY 空参数测试放在 proptest! 宏外:proptest! 宏要求至少 1 个 `parm in strategy` 参数,
// 零参数函数无法匹配宏模式,因此作为普通 #[test] 编写
#[test]
fn test_empty_candidates_returns_empty_result() {
    let profiles = HashMap::new();
    let config = GeaConfig::default();
    let result =
        resolve_conflicts(vec![], &profiles, &config).expect("empty candidates should not error");
    assert!(
        !result.has_activated(),
        "empty candidates should not activate"
    );
    assert_eq!(result.activated.len(), 0);
    assert_eq!(result.suppressed.len(), 0);
}
