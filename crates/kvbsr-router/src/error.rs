//! KVBSR 错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L6 Router
//!
//! # 设计决策(WHY)
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//! - 6 个变体覆盖两级路由的全生命周期:块查找 → 工具查找 → 空状态 → 重平衡 → 配置 → 事件
//! - `EventBusError` 包装 `event_bus::EventBusError`,跨层通信失败时向上传播
//! - 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow

use thiserror::Error;

/// KVBSR 路由器错误类型 — 覆盖两级路由的全生命周期失败场景
#[derive(Debug, Error)]
pub enum KvbsrError {
    /// 块未找到(指定的 block_id 不存在或已被重平衡移除)
    #[error("块未找到: {0}")]
    BlockNotFound(String),

    /// 工具未找到(指定的 tool_id 不在工具向量索引中)
    #[error("工具未找到: {0}")]
    ToolNotFound(String),

    /// 块列表为空(未调用 build_blocks 初始化或重平衡后无块)
    ///
    /// WHY:空块状态下路由无意义,必须显式报错而非返回空结果,
    /// 避免下游消费者误判空结果为"无匹配工具"
    #[error("块列表为空,请先调用 build_blocks 初始化")]
    EmptyBlocks,

    /// 重平衡失败(共现矩阵为空、工具列表为空或聚类异常)
    #[error("重平衡失败: {0}")]
    RebalanceFailed(String),

    /// 无效配置(block_vector_dim 为 0、top_blocks/top_tools 为 0 等)
    #[error("无效配置: {0}")]
    InvalidConfig(String),

    /// 事件总线错误(发布 ToolsRouted/BlocksRebalanced 事件失败)
    #[error("事件总线错误: {0}")]
    EventBusError(String),
}

impl From<event_bus::EventBusError> for KvbsrError {
    /// 将 EventBus 错误转换为 KvbsrError::EventBusError
    fn from(e: event_bus::EventBusError) -> Self {
        Self::EventBusError(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_not_found_display() {
        let err = KvbsrError::BlockNotFound("blk-999".into());
        assert!(err.to_string().contains("blk-999"));
    }

    #[test]
    fn test_tool_not_found_display() {
        let err = KvbsrError::ToolNotFound("tool-x".into());
        assert!(err.to_string().contains("tool-x"));
    }

    #[test]
    fn test_empty_blocks_display() {
        let err = KvbsrError::EmptyBlocks;
        assert!(err.to_string().contains("块列表为空"));
    }

    #[test]
    fn test_rebalance_failed_display() {
        let err = KvbsrError::RebalanceFailed("no tools".into());
        assert!(err.to_string().contains("no tools"));
    }

    #[test]
    fn test_invalid_config_display() {
        let err = KvbsrError::InvalidConfig("dim=0".into());
        assert!(err.to_string().contains("dim=0"));
    }

    #[test]
    fn test_event_bus_error_display() {
        let err = KvbsrError::EventBusError("channel closed".into());
        assert!(err.to_string().contains("channel closed"));
    }
}
