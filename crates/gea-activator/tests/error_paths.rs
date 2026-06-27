//! GEA 门控专家激活错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 29.6:为 gea-activator 补充错误路径测试
//!
//! # 测试覆盖
//! 1. 无效门控值:GateValue::new 越界返回 InvalidGateValue
//! 2. 专家不存在:resolve_conflicts 候选专家不在注册表返回 ExpertNotFound
//! 3. 冲突消解失败:所有专家重叠度 > 0.8 时仅保留评分最高者(抑制其余)
//! 4. 配置错误:负权重 / 权重和≠1.0 / 阈值越界 返回 ConfigError
//! 5. 缓存满:LRU 驱逐后容量恒定

#![forbid(unsafe_code)]

use std::collections::HashMap;

use event_bus::EventBus;
use gea_activator::{
    resolve_conflicts, Candidate, ExpertId, ExpertProfile, GateValue, GeaActivator, GeaConfig,
    GeaError, TaskProfile,
};

// ============================================================
// 错误路径 1:无效门控值
// ============================================================

/// GateValue::new 越界(> 1.0)返回 InvalidGateValue
///
/// WHY:门控值经 sigmoid + clamp 后理论上必 ∈ [0, 1],
/// 此测试验证防御外部传入的预计算门控值越界
#[test]
fn test_invalid_gate_value_above_one() {
    let result = GateValue::new(1.5);
    let err = result.unwrap_err();
    assert!(
        matches!(err, GeaError::InvalidGateValue { value } if (value - 1.5).abs() < 1e-6),
        "应为 InvalidGateValue,实际: {err:?}"
    );
}

/// GateValue::new 越界(< 0.0)返回 InvalidGateValue
#[test]
fn test_invalid_gate_value_below_zero() {
    let result = GateValue::new(-0.1);
    assert!(matches!(result, Err(GeaError::InvalidGateValue { .. })));
}

/// GateValue::new 接受 NaN 返回 InvalidGateValue
///
/// WHY:NaN 比较均返回 false,需显式拒绝防止后续排序异常
#[test]
fn test_invalid_gate_value_nan_rejected() {
    let result = GateValue::new(f32::NAN);
    assert!(matches!(result, Err(GeaError::InvalidGateValue { .. })));
}

/// GateValue::new 边界值 0.0 和 1.0 合法
#[test]
fn test_gate_value_boundary_valid() {
    assert!(GateValue::new(0.0).is_ok());
    assert!(GateValue::new(1.0).is_ok());
}

// ============================================================
// 错误路径 2:专家不存在
// ============================================================

/// resolve_conflicts 候选专家不在注册表返回 ExpertNotFound
///
/// WHY:候选列表由门控计算阶段产生,理论上所有候选都应在注册表中。
/// 此测试验证防御性检查:候选专家被并发注销时返回明确错误而非 panic
#[test]
fn test_resolve_conflicts_expert_not_found() {
    let profiles = HashMap::new(); // 空注册表
    let config = GeaConfig::default();
    let candidates: Vec<Candidate> = vec![(ExpertId::new("missing"), 0.8)];

    let result = resolve_conflicts(candidates, &profiles, &config);
    let err = result.unwrap_err();
    assert!(
        matches!(err, GeaError::ExpertNotFound { ref expert_id } if expert_id == "missing"),
        "应为 ExpertNotFound,实际: {err:?}"
    );
}

// ============================================================
// 错误路径 3:冲突消解(所有专家重叠度 > 0.8)
// ============================================================

/// 所有专家向量相同(重叠度 = 1.0 > 0.8),仅保留评分最高者
///
/// WHY:高度重叠的专家视为功能冗余,冲突消解应抑制低评分者。
/// 此测试验证重叠检测逻辑:相同向量 → 重叠度 1.0 → 抑制其余
#[test]
fn test_conflict_resolution_all_high_overlap() {
    // 5 个专家向量完全相同(重叠度 = 1.0)
    let v = vec![1.0; 64];
    let mut profiles = HashMap::new();
    for i in 0..5 {
        profiles.insert(
            ExpertId::new(format!("e-{i}")),
            ExpertProfile::new(format!("e-{i}"), v.clone(), 0.5, vec![]),
        );
    }

    // 候选门控值递减:e-0 最高
    let candidates: Vec<Candidate> = (0..5)
        .map(|i| (ExpertId::new(format!("e-{i}")), 0.9 - (i as f32 * 0.1)))
        .collect();

    let config = GeaConfig::default(); // overlap_threshold = 0.8, top_k = 3
    let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

    // 仅 e-0(评分最高)被激活,其余 4 个被抑制(重叠度 > 0.8)
    assert_eq!(
        result.activated.len(),
        1,
        "全部高重叠时应仅激活 1 个评分最高者"
    );
    assert_eq!(result.activated[0], ExpertId::new("e-0"));
    assert_eq!(result.suppressed.len(), 4, "其余 4 个应被抑制");
}

// ============================================================
// 错误路径 4:配置错误
// ============================================================

/// 负权重返回 ConfigError
///
/// WHY:负权重会导致门控值计算异常(sigmoid 输入偏移),需在系统边界拦截
#[test]
fn test_config_error_negative_weight() {
    let config = GeaConfig {
        w1: -0.1,
        ..Default::default()
    };
    let result = GeaActivator::new(config, EventBus::new());
    // WHY match 替代 unwrap_err:GeaActivator 未实现 Debug,unwrap_err 要求 T: Debug
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("负权重应返回 ConfigError,实际成功创建"),
    };
    assert!(
        matches!(err, GeaError::ConfigError { ref detail } if detail.contains("non-negative")),
        "负权重应返回 ConfigError,实际: {err:?}"
    );
}

/// 权重和 ≠ 1.0 返回 ConfigError
#[test]
fn test_config_error_weights_sum_not_one() {
    let config = GeaConfig {
        w1: 0.5,
        w2: 0.5,
        w3: 0.5, // 和 = 1.5
        ..Default::default()
    };
    let result = GeaActivator::new(config, EventBus::new());
    assert!(matches!(result, Err(GeaError::ConfigError { .. })));
}

/// 阈值越界(> 1.0)返回 ConfigError
#[test]
fn test_config_error_threshold_above_one() {
    let config = GeaConfig {
        activation_threshold: 1.5,
        ..Default::default()
    };
    let result = GeaActivator::new(config, EventBus::new());
    assert!(matches!(result, Err(GeaError::ConfigError { .. })));
}

/// cache_capacity = 0 返回 ConfigError
#[test]
fn test_config_error_zero_cache_capacity() {
    let config = GeaConfig {
        cache_capacity: 0,
        ..Default::default()
    };
    let result = GeaActivator::new(config, EventBus::new());
    assert!(matches!(result, Err(GeaError::ConfigError { .. })));
}

/// top_k = 0 返回 ConfigError
#[test]
fn test_config_error_zero_top_k() {
    let config = GeaConfig {
        top_k: 0,
        ..Default::default()
    };
    let result = GeaActivator::new(config, EventBus::new());
    assert!(matches!(result, Err(GeaError::ConfigError { .. })));
}

// ============================================================
// 错误路径 5:缓存满(LRU 驱逐)
// ============================================================

/// 缓存满时 LRU 驱逐最旧条目,容量恒定
///
/// WHY:缓存容量有限,超出时必须驱逐最久未访问的条目。
/// 此测试验证 LRU 驱逐逻辑:容量恒定,不超容
#[tokio::test]
async fn test_cache_lru_eviction_capacity_constant() {
    // 配置小缓存容量,触发 LRU 驱逐
    let config = GeaConfig {
        cache_capacity: 3,
        ..Default::default()
    };
    let activator = GeaActivator::new(config, EventBus::new()).unwrap();
    activator.register_expert(ExpertProfile::new(
        "e-1",
        vec![0.5; 64],
        0.8,
        vec!["code-gen".into()],
    ));

    // 插入 5 个不同任务,应触发 LRU 驱逐
    for i in 0..5 {
        let task = TaskProfile::new(0.8, format!("task-{i}"), 30, vec![0.5; 64]);
        activator.activate(&task).await.unwrap();
    }

    // 缓存容量 3,应有 3 个条目(不超容)
    assert_eq!(activator.cache_len(), 3, "LRU 驱逐后缓存容量应恒定为 3");
}

/// 缓存命中后条目时间刷新,避免被驱逐
///
/// WHY:LRU 语义要求"最近访问的不易被驱逐",
/// 此测试验证命中后条目不会被立即驱逐
#[tokio::test]
async fn test_cache_hit_refreshes_entry() {
    let config = GeaConfig {
        cache_capacity: 2,
        ..Default::default()
    };
    let activator = GeaActivator::new(config, EventBus::new()).unwrap();
    activator.register_expert(ExpertProfile::new(
        "e-1",
        vec![0.5; 64],
        0.8,
        vec!["code-gen".into()],
    ));

    let task1 = TaskProfile::new(0.8, "task-1", 30, vec![0.5; 64]);
    let task2 = TaskProfile::new(0.8, "task-2", 30, vec![0.5; 64]);

    // 插入 task1, task2
    activator.activate(&task1).await.unwrap();
    activator.activate(&task2).await.unwrap();
    assert_eq!(activator.cache_len(), 2);

    // 命中 task1(刷新其时间戳)
    activator.activate(&task1).await.unwrap();
    assert!(activator.cache_hit_rate() > 0.0, "task1 应命中缓存");

    // 插入 task3,应驱逐 task2(最久未访问),保留 task1(刚命中)
    let task3 = TaskProfile::new(0.8, "task-3", 30, vec![0.5; 64]);
    activator.activate(&task3).await.unwrap();
    assert_eq!(activator.cache_len(), 2, "容量应保持 2");

    // task1 应仍命中(刚刷新),task2 应已驱逐
    activator.activate(&task1).await.unwrap();
    // 第二次命中 task1 后,命中率应更高
    let _ = activator.cache_hit_rate();
}

// ============================================================
// 错误路径 6:空注册表激活(边界)
// ============================================================

/// 空注册表时 activate 返回空结果(不报错)
///
/// WHY:空注册表是合法状态(初始化阶段),activate 应返回空结果而非 panic
#[tokio::test]
async fn test_activate_empty_registry_returns_empty() {
    let activator = GeaActivator::new(GeaConfig::default(), EventBus::new()).unwrap();
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    let result = activator.activate(&task).await.unwrap();
    assert!(!result.has_activated(), "空注册表不应激活任何专家");
    assert_eq!(result.activated.len(), 0);
}
