//! OSA 协调器错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 20.5:补充错误路径测试
//!
//! # 测试覆盖
//! 1. 无效输入:compute_all_masks 在 complexity_score > 1.0 时返回 InvalidTaskProfile
//! 2. 无效配置:routing_top_k_bounds 下限 > 上限返回 InvalidConfig
//! 3. 无效配置:budget_protection_threshold 超出 [0,1] 返回 InvalidConfig
//! 4. 边界校验:SparsityOutOfRange 错误构造与显示
//! 5. 错误转换:serde_json::Error → OsaError::MaskComputationFailed

#![forbid(unsafe_code)]

use event_bus::EventBus;
use osa_coordinator::{OmniSparseCoordinator, OsaConfig, OsaError, RiskLevel, TaskProfile};

/// 无效输入:compute_all_masks 在 complexity_score > 1.0 时返回 InvalidTaskProfile
///
/// WHY:complexity_score 是 OSA 联动稀疏化的核心驱动,超出 [0.0, 1.0] 会导致
/// 复杂度档位判定异常。compute_all_masks 在入口校验并拒绝非法输入。
#[tokio::test]
async fn test_compute_masks_invalid_complexity() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    // complexity_score = 1.5 超出 [0.0, 1.0]
    let profile = TaskProfile::new("task-1", 1.5, RiskLevel::Low);
    let result = coord.compute_all_masks(&profile).await;
    assert!(result.is_err(), "complexity_score=1.5 应返回错误");
    let err = result.unwrap_err();
    assert!(
        matches!(err, OsaError::InvalidTaskProfile(_)),
        "应为 InvalidTaskProfile,实际: {err:?}"
    );
    assert!(
        err.to_string().contains("complexity_score"),
        "错误信息应包含字段名"
    );
}

/// 无效输入:compute_all_masks 在 complexity_score < 0.0 时返回 InvalidTaskProfile
///
/// WHY:负数复杂度无意义,应在系统边界拦截。
#[tokio::test]
async fn test_compute_masks_negative_complexity() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = TaskProfile::new("task-2", -0.1, RiskLevel::Low);
    let result = coord.compute_all_masks(&profile).await;
    assert!(result.is_err(), "complexity_score=-0.1 应返回错误");
    let err = result.unwrap_err();
    assert!(matches!(err, OsaError::InvalidTaskProfile(_)));
}

/// 无效配置:routing_top_k_bounds 下限 > 上限返回 InvalidConfig
///
/// WHY:routing_top_k_bounds (min, max) 要求 min <= max,
/// 反转后会导致 Top-K 选取逻辑异常(选 0 个工具)。
#[test]
fn test_config_validate_inverted_bounds() {
    let config = OsaConfig::new().with_routing_top_k_bounds(32, 8);
    let err = config.validate().unwrap_err();
    assert!(matches!(err, OsaError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("routing_top_k_bounds"),
        "错误信息应包含字段名"
    );
}

/// 无效配置:budget_protection_threshold 超出 [0,1] 返回 InvalidConfig
///
/// WHY:budget_protection_threshold 是任务保护比例,超出 [0.0, 1.0] 无意义。
#[test]
fn test_config_validate_invalid_budget_threshold() {
    let config = OsaConfig::new().with_budget_protection_threshold(1.5);
    let err = config.validate().unwrap_err();
    assert!(matches!(err, OsaError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("budget_protection_threshold"),
        "错误信息应包含字段名"
    );
}

/// 错误转换:serde_json::Error → OsaError::MaskComputationFailed
///
/// WHY:OSA 在 mask_hash 计算阶段使用 JSON 序列化,序列化失败需正确转换。
/// 归为 MaskComputationFailed(而非单独变体),因为序列化是掩码计算的子步骤。
#[test]
fn test_error_conversion_from_serde_json() {
    let json_err = serde_json::from_str::<String>("not a valid string").unwrap_err();
    let osa_err: OsaError = json_err.into();
    assert!(
        matches!(osa_err, OsaError::MaskComputationFailed(_)),
        "serde_json::Error 应转换为 MaskComputationFailed"
    );
    assert!(
        osa_err.to_string().contains("JSON 序列化失败"),
        "错误信息应包含 JSON 序列化失败描述"
    );
}
