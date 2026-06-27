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

    /// 功能未实现 — 本周仅 VSCode 适配器完整实现 execute
    #[error("功能未实现: ide={ide} feature={feature}")]
    NotImplemented {
        /// IDE 标识
        ide: String,
        /// 未实现的功能名
        feature: String,
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
    fn test_not_implemented_display() {
        let e = ChtcError::NotImplemented {
            ide: "vim".into(),
            feature: "execute".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("vim"));
        assert!(msg.contains("execute"));
    }
}
