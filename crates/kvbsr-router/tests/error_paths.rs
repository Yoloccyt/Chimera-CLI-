//! KVBSR 路由器错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 20.5:补充错误路径测试
//!
//! # 测试覆盖
//! 1. 空状态:未调用 build_blocks 时 route 返回 EmptyBlocks
//! 2. 空输入:build_blocks 传入空工具列表返回 EmptyBlocks
//! 3. 维度不匹配:build_blocks 传入维度不一致的工具向量返回 InvalidConfig
//! 4. 无效配置:block_vector_dim=0 返回 InvalidConfig
//! 5. 无效配置:top_tools=0 返回 InvalidConfig

#![forbid(unsafe_code)]

use event_bus::EventBus;
use kvbsr_router::{
    CoOccurrenceMatrix, KVBlockSemanticRouter, KvbsrConfig, KvbsrError, ToolVector,
};
use nexus_core::CLV;

/// 空状态:未调用 build_blocks 时 route 返回 EmptyBlocks
///
/// WHY:空块状态下路由无意义,必须显式报错而非返回空结果,
/// 避免下游消费者误判空结果为"无匹配工具"。
#[tokio::test]
async fn test_route_without_build_blocks() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    let clv = CLV::zero();
    let result = router.route(&clv).await;
    assert!(result.is_err(), "未调用 build_blocks 时应返回错误");
    let err = result.unwrap_err();
    assert!(
        matches!(err, KvbsrError::EmptyBlocks),
        "应为 EmptyBlocks,实际: {err:?}"
    );
}

/// 空输入:build_blocks 传入空工具列表返回 EmptyBlocks
///
/// WHY:空工具列表无法构建语义块,应在系统边界拦截。
#[tokio::test]
async fn test_build_blocks_empty_tools() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    let tools: Vec<ToolVector> = Vec::new();
    let co = CoOccurrenceMatrix::new();
    let result = router.build_blocks(tools, co).await;
    assert!(result.is_err(), "空工具列表应返回错误");
    let err = result.unwrap_err();
    assert!(
        matches!(err, KvbsrError::EmptyBlocks),
        "应为 EmptyBlocks,实际: {err:?}"
    );
}

/// 维度不匹配:build_blocks 传入维度不一致的工具向量返回 InvalidConfig
///
/// WHY:工具向量维度必须与 config.block_vector_dim 一致,
/// 维度不匹配会导致块向量加权平均语义失真(用 min 静默截断)。
#[tokio::test]
async fn test_build_blocks_dimension_mismatch() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    // 默认 block_vector_dim=64,传入 32 维向量应失败
    let tools = vec![ToolVector::new("t1", vec![0.0; 32], 100)];
    let co = CoOccurrenceMatrix::new();
    let result = router.build_blocks(tools, co).await;
    assert!(result.is_err(), "维度不匹配应返回错误");
    let err = result.unwrap_err();
    assert!(
        matches!(err, KvbsrError::InvalidConfig(_)),
        "应为 InvalidConfig,实际: {err:?}"
    );
    assert!(
        err.to_string().contains("block_vector_dim"),
        "错误信息应包含 block_vector_dim"
    );
}

/// 无效配置:block_vector_dim=0 返回 InvalidConfig
///
/// WHY:block_vector_dim=0 会导致向量运算除零或空向量,属于配置失误。
#[test]
fn test_config_validate_zero_block_vector_dim() {
    let config = KvbsrConfig::new().with_block_vector_dim(0);
    let err = config.validate().unwrap_err();
    assert!(matches!(err, KvbsrError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("block_vector_dim"),
        "错误信息应包含字段名"
    );
}

/// 无效配置:top_tools=0 返回 InvalidConfig
///
/// WHY:top_tools=0 意味着路由不返回任何工具,属于配置失误。
#[test]
fn test_config_validate_zero_top_tools() {
    let config = KvbsrConfig::new().with_top_tools(0);
    let err = config.validate().unwrap_err();
    assert!(matches!(err, KvbsrError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("top_tools"),
        "错误信息应包含字段名"
    );
}
