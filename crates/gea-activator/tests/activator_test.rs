//! GEA TaskProfile Hash 测试 — 验证直接 Hash 替代 serde_json 序列化
//!
//! 对应 Phase V Task V-4 [N17]
//!
//! # 测试目标
//! - TaskProfile impl Hash 的一致性(相同字段 → 相同 hash)
//! - TaskProfile impl Hash 的区分度(任一字段不同 → 不同 hash)
//! - NaN 安全(clv/complexity_score 含 NaN 时不 panic)
//! - 缓存命中回归(改造为直接 Hash 后缓存功能仍正常)
//!
//! # WHY 直接 Hash 替代 serde_json 序列化
//! 原实现 `hash_task_profile` 每次 `serde_json::to_string` 序列化整个 TaskProfile
//! 再 DefaultHasher,序列化开销 O(n)(n = clv 长度,通常 512),且分配 String。
//! 直接 impl Hash 用 `to_bits()` 逐字段哈希,零分配 O(n),省去序列化中间态。

#![forbid(unsafe_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use event_bus::EventBus;
use gea_activator::{ExpertProfile, GeaActivator, GeaConfig, TaskProfile};

/// 计算一个 TaskProfile 的 u64 哈希值(辅助测试断言)
fn hash_of(tp: &TaskProfile) -> u64 {
    let mut hasher = DefaultHasher::new();
    tp.hash(&mut hasher);
    hasher.finish()
}

// ============================================================
// 一致性:相同字段值必须产生相同 hash
// ============================================================

#[test]
fn test_task_profile_hash_consistency() {
    let t1 = TaskProfile::new(0.7, "code-gen", 30, vec![0.5; 64]);
    let t2 = TaskProfile::new(0.7, "code-gen", 30, vec![0.5; 64]);

    // 字段全等 → hash 必须相等(Hash 与 Eq 一致性的基础)
    assert_eq!(hash_of(&t1), hash_of(&t2));
}

// ============================================================
// 区分度:任一字段不同必须产生不同 hash
// ============================================================

#[test]
fn test_task_profile_hash_different_fields() {
    let base = TaskProfile::new(0.7, "code-gen", 30, vec![0.5; 64]);

    // 1. complexity_score 不同
    let diff_complexity = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);
    assert_ne!(
        hash_of(&base),
        hash_of(&diff_complexity),
        "complexity_score 不同应有不同 hash"
    );

    // 2. task_type 不同
    let diff_type = TaskProfile::new(0.7, "refactor", 30, vec![0.5; 64]);
    assert_ne!(
        hash_of(&base),
        hash_of(&diff_type),
        "task_type 不同应有不同 hash"
    );

    // 3. risk_level 不同
    let diff_risk = TaskProfile::new(0.7, "code-gen", 60, vec![0.5; 64]);
    assert_ne!(
        hash_of(&base),
        hash_of(&diff_risk),
        "risk_level 不同应有不同 hash"
    );

    // 4. clv 内容不同(同长度,避免长度差干扰)
    let diff_clv = TaskProfile::new(0.7, "code-gen", 30, vec![0.6; 64]);
    assert_ne!(
        hash_of(&base),
        hash_of(&diff_clv),
        "clv 内容不同应有不同 hash"
    );

    // 5. clv 长度不同也应有不同 hash(hash 必须包含长度,防碰撞)
    let diff_len = TaskProfile::new(0.7, "code-gen", 30, vec![0.5; 128]);
    assert_ne!(
        hash_of(&base),
        hash_of(&diff_len),
        "clv 长度不同应有不同 hash"
    );
}

// ============================================================
// NaN 安全:f32::NAN.to_bits() 有确定值,hash 不 panic
// ============================================================

#[test]
fn test_task_profile_hash_nan_safe() {
    // clv 含 NaN 不应 panic;to_bits() 对 NaN 返回确定 bit pattern
    let t_nan_clv = TaskProfile::new(0.7, "code-gen", 30, vec![f32::NAN; 64]);
    let _ = hash_of(&t_nan_clv);

    // complexity_score 为 NaN 也不应 panic
    let t_nan_score = TaskProfile::new(f32::NAN, "code-gen", 30, vec![0.5; 64]);
    let _ = hash_of(&t_nan_score);

    // 两个 NaN 字段值相同的 TaskProfile hash 必须一致(NaN bits 确定性)
    let t_nan_a = TaskProfile::new(f32::NAN, "x", 0, vec![f32::NAN]);
    let t_nan_b = TaskProfile::new(f32::NAN, "x", 0, vec![f32::NAN]);
    assert_eq!(hash_of(&t_nan_a), hash_of(&t_nan_b));
}

// ============================================================
// 缓存命中回归:改造为直接 Hash 后缓存功能仍正常
// ============================================================

#[tokio::test]
async fn test_cache_uses_task_profile_key() {
    let activator = GeaActivator::new(GeaConfig::default(), EventBus::new()).unwrap();
    activator.register_expert(ExpertProfile::new(
        "e-1",
        vec![0.5; 64],
        0.8,
        vec!["code-gen".into()],
    ));

    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    // 第一次激活:缓存未命中,hit_rate 应为 0.0
    let _ = activator.activate(&task).await.unwrap();
    assert_eq!(activator.cache_hit_rate(), 0.0);

    // 第二次激活相同 TaskProfile:应命中缓存,hit_rate > 0.0
    // 直接用 TaskProfile 作 key(而非 u64 序列化哈希),相同字段值即同 key
    let _ = activator.activate(&task).await.unwrap();
    assert!(
        activator.cache_hit_rate() > 0.0,
        "相同 TaskProfile 第二次应命中缓存"
    );

    // 不同 TaskProfile 应未命中(hit_rate 不应继续上升至 1.0 之外)
    let other = TaskProfile::new(0.1, "refactor", 10, vec![0.2; 64]);
    let before = activator.cache_hit_rate();
    let _ = activator.activate(&other).await.unwrap();
    // other 是新 key,本次为 miss;hit_rate 不变(total+1, hits 不变 → 比例下降)
    assert!(
        activator.cache_hit_rate() <= before,
        "不同 TaskProfile 不应命中"
    );
}
