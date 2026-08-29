//! 统一错误层级契约 — NexusError 与 Recoverable（A0 / WI-01 §6.6）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts，纯类型 + thiserror）
//! 对应工作项: **A0 L0 契约三模块**（v4.0 统一执行总案 §6.6 + §8.1 A0 迁移步）
//! 对应设计源: 外部修订版统一错误层级（批判性吸收——库层结构化枚举 +
//!             应用层 anyhow 人类可读消息的既有分工不变）
//!
//! # 核心职责
//!
//! 承载跨层统一错误类型与恢复策略标记：
//! - [`NexusError`]: 协议/事件/工具/沙箱等**结构化错误枚举**（库层惯例）
//! - [`RecoveryStrategy`]: 错误恢复策略五档（调用方按策略执行降级）
//! - [`Recoverable`]: 错误可恢复性标记 trait（`NexusError` 自动实现）
//!
//! # 设计约束（ADR-033 例外扩展）
//!
//! - **thiserror 例外**: 本模块引入 `thiserror` 依赖（与 serde/chrono/uuid
//!   同级的基础库例外——库层错误类型标准 §4.1 要求 thiserror enum，
//!   37 个 error.rs 全部 thiserror 先例）；Cargo.toml 注释已登记
//! - **不替代既有 crate 错误**: 各 crate 的 `*Error` 保持（`EventBusError` /
//!   `MasError` 等），本模块为**协议面与跨层边界**的统一错误
//! - **人类可读消息**: 应用层 anyhow 包装本类型（`#[error]` 消息即
//!   人类可读形态，无需再映射）

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================
// 统一错误枚举
// ============================================================

/// 跨层统一错误 — 协议/事件/工具/沙箱/预算等边界错误（WI-01 §6.6）
///
/// # 使用约定
/// - **库层**: 结构化枚举错误（本类型）——不再包裹 `String` 错误
/// - **应用层（CLI/TUI）**: anyhow 包装本类型输出人类可读消息
/// - **协议面**: `AppEvent::Error { code, message }` 的 `code` 取本枚举
///   变体名（app.rs 契约），`message` 取 `#[error]` 文本
#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize)]
pub enum NexusError {
    /// 事件序列化失败
    #[error("event serialization failed: {0}")]
    SerializationError(String),
    /// 无效事件类型（动态事件注册表查无此类型）
    #[error("invalid event type: {0}")]
    InvalidEventType(String),
    /// 上下文预算超限（current > max）
    #[error("context budget exceeded: {current} > {max}")]
    ContextBudgetExceeded {
        /// 当前占用
        current: usize,
        /// 预算上限
        max: usize,
    },
    /// 工具执行超时
    #[error("tool execution timeout: {tool_name} after {duration_ms}ms")]
    ToolTimeout {
        /// 工具名
        tool_name: String,
        /// 超时时长（毫秒）
        duration_ms: u64,
    },
    /// 沙箱违规（零信任拦截）
    #[error("sandbox violation: {details}")]
    SandboxViolation {
        /// 违规详情
        details: String,
    },
    /// SubAgent 嵌套禁止（WI-25 编译期/运行期双断言）
    #[error("subagent nesting forbidden")]
    NestedSubAgentForbidden,
    /// MCP 服务器断连（WI-22 降级链触发）
    #[error("MCP server disconnected: {server_name}")]
    McpDisconnected {
        /// 服务器名
        server_name: String,
    },
    /// 审批拒绝（用户或策略拒否）
    #[error("approval denied: {operation}")]
    ApprovalDenied {
        /// 被拒操作
        operation: String,
    },
    /// 模型 API 错误（供应商返回错误）
    #[error("model API error: {status} - {message}")]
    ModelApiError {
        /// HTTP 状态码（0 = 非 HTTP 传输错误）
        status: u16,
        /// 错误消息
        message: String,
    },
    /// 动态事件注册超限（WI-21 命名空间配额）
    #[error("dynamic event namespace quota exceeded: {namespace} ({current}/{max})")]
    EventQuotaExceeded {
        /// 命名空间
        namespace: String,
        /// 当前注册数
        current: usize,
        /// 配额上限
        max: usize,
    },
}

// ============================================================
// 恢复策略
// ============================================================

/// 错误恢复策略 — 调用方按策略执行降级（WI-01 §6.6）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStrategy {
    /// 固定次数重试
    Retry {
        /// 最大重试次数
        max_attempts: u8,
    },
    /// 指数退避重试
    RetryWithBackoff,
    /// 降级到内置实现（如 MCP 断连 → 内置工具）
    FallbackToBuiltin,
    /// 压缩后重试（上下文预算超限 → 压缩 → 重试）
    CompressAndRetry,
    /// 快速失败（不重试，直接上抛）
    FailFast,
}

/// 可恢复性标记 — 错误携带恢复策略（`NexusError` 自动实现）
pub trait Recoverable {
    /// 获取恢复策略
    fn recovery_strategy(&self) -> RecoveryStrategy;
}

impl Recoverable for NexusError {
    fn recovery_strategy(&self) -> RecoveryStrategy {
        match self {
            // 序列化/无效类型为协议面错误: 调用方重试（固定 2 次）
            Self::SerializationError(_) | Self::InvalidEventType(_) => {
                RecoveryStrategy::Retry { max_attempts: 2 }
            }
            // 预算超限: 压缩后重试（WI-12 CSC 四级压缩链触发点）
            Self::ContextBudgetExceeded { .. } => RecoveryStrategy::CompressAndRetry,
            // 工具超时: 指数退避重试（幂等工具可重试）
            Self::ToolTimeout { .. } => RecoveryStrategy::RetryWithBackoff,
            // 沙箱违规: 快速失败（零信任红线——不重试违规操作）
            Self::SandboxViolation { .. } => RecoveryStrategy::FailFast,
            // 嵌套禁止: 快速失败（硬约束）
            Self::NestedSubAgentForbidden => RecoveryStrategy::FailFast,
            // MCP 断连: 降级内置（WI-22 降级链）
            Self::McpDisconnected { .. } => RecoveryStrategy::FallbackToBuiltin,
            // 审批拒绝: 快速失败（等待用户新决策，不自动重试）
            Self::ApprovalDenied { .. } => RecoveryStrategy::FailFast,
            // 模型 API 错误: 指数退避重试
            Self::ModelApiError { .. } => RecoveryStrategy::RetryWithBackoff,
            // 注册超限: 快速失败（配额是硬边界，扩配需治理）
            Self::EventQuotaExceeded { .. } => RecoveryStrategy::FailFast,
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nexus_error_display_human_readable() {
        let err = NexusError::SandboxViolation {
            details: "attempted write to /etc/passwd".into(),
        };
        assert_eq!(
            err.to_string(),
            "sandbox violation: attempted write to /etc/passwd"
        );
    }

    #[test]
    fn nexus_error_json_roundtrip() {
        let err = NexusError::ContextBudgetExceeded {
            current: 120_000,
            max: 100_000,
        };
        let json = serde_json::to_string(&err).expect("JSON 序列化失败");
        let decoded: NexusError = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, err);
    }

    #[test]
    fn recovery_strategy_mapping() {
        // 零信任红线: 沙箱违规/嵌套禁止/审批拒绝 → FailFast
        assert_eq!(
            NexusError::SandboxViolation {
                details: "x".into()
            }
            .recovery_strategy(),
            RecoveryStrategy::FailFast
        );
        assert_eq!(
            NexusError::NestedSubAgentForbidden.recovery_strategy(),
            RecoveryStrategy::FailFast
        );
        // 预算超限 → 压缩重试（WI-12 触发点）
        assert_eq!(
            NexusError::ContextBudgetExceeded {
                current: 101,
                max: 100
            }
            .recovery_strategy(),
            RecoveryStrategy::CompressAndRetry
        );
        // MCP 断连 → 内置降级（WI-22）
        assert_eq!(
            NexusError::McpDisconnected {
                server_name: "github".into()
            }
            .recovery_strategy(),
            RecoveryStrategy::FallbackToBuiltin
        );
        // 工具超时/模型错误 → 指数退避
        assert_eq!(
            NexusError::ToolTimeout {
                tool_name: "bash".into(),
                duration_ms: 30_000
            }
            .recovery_strategy(),
            RecoveryStrategy::RetryWithBackoff
        );
    }

    #[test]
    fn recovery_strategy_serde_roundtrip() {
        let strategy = RecoveryStrategy::RetryWithBackoff;
        let json = serde_json::to_string(&strategy).expect("JSON 序列化失败");
        assert_eq!(json, "\"retry_with_backoff\"");
        let decoded: RecoveryStrategy = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, strategy);
    }

    #[test]
    fn error_code_matches_app_event_contract() {
        // 协议面约定: AppEvent::Error.code 取 NexusError 变体名
        let code = "SandboxViolation";
        let err = NexusError::SandboxViolation {
            details: "d".into(),
        };
        assert!(format!("{err:?}").contains(code));
    }
}
