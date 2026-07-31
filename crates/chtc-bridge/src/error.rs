//! CHTC 错误类型 — 库层 thiserror enum(§4.1)

use thiserror::Error;

/// CHTC 桥接器错误
#[derive(Debug, Error)]
pub enum ChtcError {
    /// 不支持的 IDE 来源
    #[error("不支持的 IDE: {ide}")]
    UnsupportedIde {
        /// IDE 标识
        ide: String,
    },

    /// 工具调用超时
    #[error("工具调用超时: call_id={call_id} timeout={timeout_ms}ms")]
    CallTimeout {
        /// 调用 ID
        call_id: String,
        /// 超时阈值(毫秒)
        timeout_ms: u64,
    },

    /// 协议错误 — 原生格式不符合预期(缺字段/类型错误)
    #[error("协议错误: {reason}")]
    ProtocolError {
        /// 错误原因
        reason: String,
    },

    /// 调用载荷过大 — 超过 receive 入口的 1MB 大小上限
    ///
    /// WHY:防止恶意 IDE 注入超大 JSON 耗尽内存(系统边界校验,§4.1)
    #[error("调用载荷过大: size={size} bytes, 上限={limit} bytes")]
    PayloadTooLarge {
        /// 实际字节数
        size: usize,
        /// 大小上限
        limit: usize,
    },

    /// JSON 嵌套深度超限 — 超过 receive 入口的 32 层深度上限
    ///
    /// WHY:防止恶意构造的深层嵌套 JSON 触发解析栈溢出(系统边界校验)
    #[error("JSON 嵌套深度超限: depth={depth}, 上限={limit}")]
    PayloadDepthExceeded {
        /// 实际深度
        depth: usize,
        /// 深度上限
        limit: usize,
    },
}

/// 从 nexus-core 错误转换
///
/// WHY:L10 可向下依赖 L1 的 nexus-core,桥接层在内部状态交互失败时
/// 统一归并为 ChtcError::ProtocolError,避免上层感知底层错误细节。
impl From<nexus_core::NexusError> for ChtcError {
    fn from(err: nexus_core::NexusError) -> Self {
        Self::ProtocolError {
            reason: format!("nexus: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let e = ChtcError::UnsupportedIde {
            ide: "sublime".into(),
        };
        assert!(e.to_string().contains("sublime"));
    }

    #[test]
    fn test_protocol_error_display() {
        let e = ChtcError::ProtocolError {
            reason: "missing field".into(),
        };
        assert!(e.to_string().contains("missing field"));
    }

    #[test]
    fn test_payload_too_large_display() {
        let e = ChtcError::PayloadTooLarge {
            size: 2_000_000,
            limit: 1_048_576,
        };
        let msg = e.to_string();
        assert!(msg.contains("2000000"));
        assert!(msg.contains("1048576"));
    }

    #[test]
    fn test_payload_depth_exceeded_display() {
        let e = ChtcError::PayloadDepthExceeded {
            depth: 64,
            limit: 32,
        };
        let msg = e.to_string();
        assert!(msg.contains("64"));
        assert!(msg.contains("32"));
    }
}
