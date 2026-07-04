//! HCW 错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L2 Memory
//!
//! # 设计决策(WHY)
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//! - 6 个变体覆盖 HCW 全生命周期:窗口溢出 → 压缩失败 → 层级校验 →
//!   条目查找 → 事件总线 → 配置校验
//! - `EventBusError` 包装 `event_bus::EventBusError`,跨层通信失败时向上传播

use thiserror::Error;

/// HCW 错误类型 — 覆盖分层上下文窗口的全生命周期失败场景
#[derive(Debug, Error)]
pub enum HcwError {
    /// 窗口溢出(条目总大小超过当前层级容量且无法压缩或升级)
    ///
    /// WHY:L3 已是最高层级,若 L3 实际加载容量(128K)仍溢出且压缩失败,
    /// 返回此错误,调用方需决定丢弃低重要性条目或扩大 L3 容量
    #[error("窗口溢出: {0}")]
    WindowOverflow(String),

    /// 压缩失败(重要性评分计算异常、目标大小为 0 等)
    #[error("压缩失败: {0}")]
    CompressionFailed(String),

    /// 无效层级(字符串解析失败、层级升降越界等)
    #[error("无效层级: {0}")]
    InvalidTier(String),

    /// 条目未找到(按 ID 查找/移除时不存在)
    #[error("条目未找到: {0}")]
    EntryNotFound(String),

    /// 事件总线错误(发布 ContextWindowSwitched/ContextCompressed 事件失败)
    #[error("事件总线错误: {0}")]
    EventBusError(String),

    /// 无效配置(l0/l1/l2/l3 容量非递增、compression_threshold 超出 (0, 1] 等)
    #[error("无效配置: {0}")]
    InvalidConfig(String),
}

impl From<event_bus::EventBusError> for HcwError {
    /// 将 EventBus 错误转换为 HcwError::EventBusError
    ///
    /// WHY:EventBus 发布/订阅失败时向上传播,调用方按 HcwError 统一处理
    fn from(e: event_bus::EventBusError) -> Self {
        Self::EventBusError(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_overflow_display() {
        let err = HcwError::WindowOverflow("L3 capacity exceeded".into());
        assert!(err.to_string().contains("L3 capacity exceeded"));
    }

    #[test]
    fn test_compression_failed_display() {
        let err = HcwError::CompressionFailed("target_size is 0".into());
        assert!(err.to_string().contains("target_size is 0"));
    }

    #[test]
    fn test_invalid_tier_display() {
        let err = HcwError::InvalidTier("unknown tier: L4".into());
        assert!(err.to_string().contains("unknown tier: L4"));
    }

    #[test]
    fn test_entry_not_found_display() {
        let err = HcwError::EntryNotFound("e-1".into());
        assert!(err.to_string().contains("e-1"));
    }

    #[test]
    fn test_invalid_config_display() {
        let err = HcwError::InvalidConfig("l0_capacity must be > 0".into());
        assert!(err.to_string().contains("l0_capacity must be > 0"));
    }

    #[test]
    fn test_event_bus_error_display() {
        let err = HcwError::EventBusError("channel closed".into());
        assert!(err.to_string().contains("channel closed"));
    }
}
