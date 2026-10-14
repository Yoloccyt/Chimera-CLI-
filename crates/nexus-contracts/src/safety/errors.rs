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
//! - [`NexusError`][]: 协议/事件/工具/沙箱等**结构化错误枚举**（库层惯例）
//! - [`RecoveryStrategy`][]: 错误恢复策略五档（调用方按策略执行降级）
//! - [`Recoverable`][]: 错误可恢复性标记 trait（`NexusError` 自动实现）
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
//!
//! # ErrorLayer 分层分类（WI-A2 / v3.0 方案 :237 + L0-4 :150）
//!
//! [`ErrorLayer`] 及其子枚举 [`EventFailure`] / [`ToolFailure`]：各层 thiserror→
//! NexusError 显式映射枚举（`to_nexus_error` 纯类型穷举映射，不引入 trait object）。
//! 契约源文档受 `.gitignore` 的 `*.md` 政策排除、不受 git 跟踪（rg 默认 0 命中，
//! 核对须 grep 绝对路径直读），契约原文逐字内嵌如下防文档丢失后不可考：
//!
//! - 方案 :237（WI-A2 行）：『| WI-A2 | `nexus-contracts::errors.rs` | `enum ErrorLayer` + `fn to_nexus_error(self)->NexusError`（纯类型） | W3 | 否 |』
//! - 方案 :150（L0-4 行）：『| L0-4 | `NexusError` 层级映射表 | 当前 `errors.rs` 仅顶层枚举；建各层 thiserror→NexusError 显式映射枚举（纯类型，不引入 trait object） | `errors.rs` + 各 crate `error.rs` | `source()` 约束（`ADR-184`） | A \[推断\] |』
//!
//! **双 NexusError 陷阱**：映射目标是 L0 contracts 版 [`NexusError`]（本文件 10 变体
//! wire 枚举），**非** L1 `crates/nexus-core/src/error.rs` 的独立同名枚举
//! （`InvalidClvDimension` 等，两者不互通，勿接错）。
//!
//! **ADR-184 约束继承**：ErrorLayer / EventFailure / ToolFailure 同为 wire 纯类型——
//! 无 `source` 字段、无 trait object（ADR-184 原文只点名 NexusError，本组类型按同
//! 约束以注释继承落档；依据方案 :237『需 ADR? 否』）。

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
// 错误分层分类（WI-A2）
// ============================================================

/// 错误分层分类 — 各层错误按产生平面（plane）归类后映射到 [`NexusError`]
///
/// # 双 NexusError 陷阱
/// 映射目标是 **L0 contracts 版** [`NexusError`]（本文件 10 变体 wire 枚举），**非**
/// L1 `crates/nexus-core/src/error.rs` 的独立同名枚举（两者不互通，勿接错）。
///
/// # 形态（两级混合）
/// 顶层 7 变体带 plane 前缀（Event / Memory / Tool / Security / SubAgent / Approval /
/// Model），多变体平面的子枚举 kind 去 plane 前缀（见 [`EventFailure`] /
/// [`ToolFailure`]）；plane 划分依据 = [`NexusError`] 各变体 doc 注释的 WI 出处
/// （本文件）+ AGENTS.md §2.1 十层归属，非外部权威清单（方案 L0-4 自评 A \[推断\]）。
///
/// # ADR-184 约束继承
/// 与 [`NexusError`] 同为 wire 纯类型：无 `source` 字段、无 trait object。
#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize)]
pub enum ErrorLayer {
    /// 事件平面失败（序列化 / 无效类型 / 配额超限，见 [`EventFailure`]）
    #[error(transparent)]
    Event(EventFailure),
    /// 上下文预算超限（current > max）
    #[error("context budget exceeded: {current} > {max}")]
    MemoryBudgetExceeded {
        /// 当前占用
        current: usize,
        /// 预算上限
        max: usize,
    },
    /// 工具平面失败（超时 / MCP 断连，见 [`ToolFailure`]）
    #[error(transparent)]
    Tool(ToolFailure),
    /// 沙箱违规（零信任拦截）
    #[error("sandbox violation: {details}")]
    SecuritySandboxViolation {
        /// 违规详情
        details: String,
    },
    /// SubAgent 嵌套禁止（硬约束，不重试）
    #[error("subagent nesting forbidden")]
    SubAgentNestingForbidden,
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
}

/// 事件平面失败 kind — [`ErrorLayer::Event`] 的载荷（variant 名去 plane 前缀）
#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize)]
pub enum EventFailure {
    /// 事件序列化失败（`#[error]` 文本逐字复制 `NexusError::SerializationError`）
    #[error("event serialization failed: {0}")]
    SerializationError(String),
    /// 无效事件类型（动态事件注册表查无此类型）
    #[error("invalid event type: {0}")]
    InvalidEventType(String),
    /// 动态事件注册超限（WI-21 命名空间配额）
    #[error("dynamic event namespace quota exceeded: {namespace} ({current}/{max})")]
    QuotaExceeded {
        /// 命名空间
        namespace: String,
        /// 当前注册数
        current: usize,
        /// 配额上限
        max: usize,
    },
}

/// 工具平面失败 kind — [`ErrorLayer::Tool`] 的载荷（variant 名去 plane 前缀）
#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize)]
pub enum ToolFailure {
    /// 工具执行超时
    #[error("tool execution timeout: {tool_name} after {duration_ms}ms")]
    Timeout {
        /// 工具名
        tool_name: String,
        /// 超时时长（毫秒）
        duration_ms: u64,
    },
    /// MCP 服务器断连（WI-22 降级链触发）
    #[error("MCP server disconnected: {server_name}")]
    McpDisconnected {
        /// 服务器名
        server_name: String,
    },
}

impl ErrorLayer {
    /// 穷举映射到 [`NexusError`] — 恰 10 臂、无兜底臂（新增 kind 时编译失败强制登记，
    /// 与 `recovery_strategy` 同款穷尽纪律；字段逐一移交，`#[error]` 消息逐字一致）
    pub fn to_nexus_error(self) -> NexusError {
        match self {
            Self::Event(EventFailure::SerializationError(msg)) => {
                NexusError::SerializationError(msg)
            }
            Self::Event(EventFailure::InvalidEventType(ty)) => NexusError::InvalidEventType(ty),
            Self::Event(EventFailure::QuotaExceeded {
                namespace,
                current,
                max,
            }) => NexusError::EventQuotaExceeded {
                namespace,
                current,
                max,
            },
            Self::MemoryBudgetExceeded { current, max } => {
                NexusError::ContextBudgetExceeded { current, max }
            }
            Self::Tool(ToolFailure::Timeout {
                tool_name,
                duration_ms,
            }) => NexusError::ToolTimeout {
                tool_name,
                duration_ms,
            },
            Self::Tool(ToolFailure::McpDisconnected { server_name }) => {
                NexusError::McpDisconnected { server_name }
            }
            Self::SecuritySandboxViolation { details } => NexusError::SandboxViolation { details },
            Self::SubAgentNestingForbidden => NexusError::NestedSubAgentForbidden,
            Self::ApprovalDenied { operation } => NexusError::ApprovalDenied { operation },
            Self::ModelApiError { status, message } => {
                NexusError::ModelApiError { status, message }
            }
        }
    }
}

impl From<ErrorLayer> for NexusError {
    /// 委托 [`ErrorLayer::to_nexus_error`]（`?` 传播兼容面）
    fn from(layer: ErrorLayer) -> Self {
        layer.to_nexus_error()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
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

    /// Ch11.6 错误可恢复性总契约不变式:recovery_strategy() 覆盖全部变体且确定性
    /// (一错一策:每个错误声明恢复策略,同错误多次声明恒同,可被 AERA 按 effort 可重放调度)
    #[test]
    fn recovery_strategy_total_and_deterministic() {
        // 构造全部 10 个变体的代表值(总数随变体新增而同增 —— 穷尽覆盖)
        let all: Vec<NexusError> = vec![
            NexusError::SerializationError("s".into()),
            NexusError::InvalidEventType("t".into()),
            NexusError::ContextBudgetExceeded { current: 1, max: 2 },
            NexusError::ToolTimeout {
                tool_name: "bash".into(),
                duration_ms: 1_000,
            },
            NexusError::SandboxViolation {
                details: "d".into(),
            },
            NexusError::NestedSubAgentForbidden,
            NexusError::McpDisconnected {
                server_name: "github".into(),
            },
            NexusError::ApprovalDenied {
                operation: "op".into(),
            },
            NexusError::ModelApiError {
                status: 500,
                message: "m".into(),
            },
            NexusError::EventQuotaExceeded {
                namespace: "ns".into(),
                current: 1,
                max: 10,
            },
        ];
        assert_eq!(
            all.len(),
            10,
            "新增 NexusError 变体必须在此登记并声明恢复策略(穷尽覆盖顺势增长)"
        );
        for err in &all {
            // 不变式 1:每个变体必须声明恢复策略(全函数,匹配无默认兜底漂移)
            let s1 = err.recovery_strategy();
            // 不变式 2:确定性 — 同一错误多次声明的策略恒定(可重放调度)
            assert_eq!(
                s1,
                err.recovery_strategy(),
                "{err:?} 的恢复策略必须确定性一致"
            );
            // 不变式 3:策略落在五档合法值域内
            match s1 {
                RecoveryStrategy::Retry { .. }
                | RecoveryStrategy::RetryWithBackoff
                | RecoveryStrategy::FallbackToBuiltin
                | RecoveryStrategy::CompressAndRetry
                | RecoveryStrategy::FailFast => {}
            }
        }
    }

    // ============================================================
    // ErrorLayer 分层分类测试(WI-A2 / v3.0 方案 :237 + L0-4 :150)
    // ============================================================

    /// 全 10 kind 代表值(总数随 kind 新增而同增 —— 穷尽覆盖纪律,镜像上方 10 变体 vec 守卫)
    fn all_error_layer_kinds() -> Vec<ErrorLayer> {
        vec![
            ErrorLayer::Event(EventFailure::SerializationError("s".into())),
            ErrorLayer::Event(EventFailure::InvalidEventType("t".into())),
            ErrorLayer::Event(EventFailure::QuotaExceeded {
                namespace: "ns".into(),
                current: 1,
                max: 10,
            }),
            ErrorLayer::MemoryBudgetExceeded {
                current: 120_000,
                max: 100_000,
            },
            ErrorLayer::Tool(ToolFailure::Timeout {
                tool_name: "bash".into(),
                duration_ms: 30_000,
            }),
            ErrorLayer::Tool(ToolFailure::McpDisconnected {
                server_name: "github".into(),
            }),
            ErrorLayer::SecuritySandboxViolation {
                details: "d".into(),
            },
            ErrorLayer::SubAgentNestingForbidden,
            ErrorLayer::ApprovalDenied {
                operation: "op".into(),
            },
            ErrorLayer::ModelApiError {
                status: 500,
                message: "m".into(),
            },
        ]
    }

    /// 10 kind 穷举映射:字面值构造 → 字面值目标 + kind(10)/plane(7) 计数守卫
    /// (镜像 recovery_strategy_total_and_deterministic 的登记纪律)
    #[test]
    fn error_layer_maps_every_kind_to_expected_target() {
        assert_eq!(
            all_error_layer_kinds().len(),
            10,
            "新增 ErrorLayer kind 必须在此登记并补映射断言(穷尽覆盖顺势增长)"
        );
        // plane 计数守卫:7 顶层变体(Event/Memory/Tool/Security/SubAgent/Approval/Model)
        let planes: Vec<ErrorLayer> = vec![
            ErrorLayer::Event(EventFailure::SerializationError("s".into())),
            ErrorLayer::MemoryBudgetExceeded { current: 1, max: 2 },
            ErrorLayer::Tool(ToolFailure::Timeout {
                tool_name: "bash".into(),
                duration_ms: 1_000,
            }),
            ErrorLayer::SecuritySandboxViolation {
                details: "d".into(),
            },
            ErrorLayer::SubAgentNestingForbidden,
            ErrorLayer::ApprovalDenied {
                operation: "op".into(),
            },
            ErrorLayer::ModelApiError {
                status: 500,
                message: "m".into(),
            },
        ];
        assert_eq!(
            planes.len(),
            7,
            "新增 ErrorLayer 顶层 plane 变体必须在此登记(穷尽覆盖顺势增长)"
        );
        assert_eq!(
            ErrorLayer::Event(EventFailure::SerializationError("s".into())).to_nexus_error(),
            NexusError::SerializationError("s".into())
        );
        assert_eq!(
            ErrorLayer::Event(EventFailure::InvalidEventType("t".into())).to_nexus_error(),
            NexusError::InvalidEventType("t".into())
        );
        assert_eq!(
            ErrorLayer::Event(EventFailure::QuotaExceeded {
                namespace: "ns".into(),
                current: 1,
                max: 10,
            })
            .to_nexus_error(),
            NexusError::EventQuotaExceeded {
                namespace: "ns".into(),
                current: 1,
                max: 10,
            }
        );
        assert_eq!(
            ErrorLayer::MemoryBudgetExceeded {
                current: 120_000,
                max: 100_000,
            }
            .to_nexus_error(),
            NexusError::ContextBudgetExceeded {
                current: 120_000,
                max: 100_000,
            }
        );
        assert_eq!(
            ErrorLayer::Tool(ToolFailure::Timeout {
                tool_name: "bash".into(),
                duration_ms: 30_000,
            })
            .to_nexus_error(),
            NexusError::ToolTimeout {
                tool_name: "bash".into(),
                duration_ms: 30_000,
            }
        );
        assert_eq!(
            ErrorLayer::Tool(ToolFailure::McpDisconnected {
                server_name: "github".into(),
            })
            .to_nexus_error(),
            NexusError::McpDisconnected {
                server_name: "github".into(),
            }
        );
        assert_eq!(
            ErrorLayer::SecuritySandboxViolation {
                details: "d".into(),
            }
            .to_nexus_error(),
            NexusError::SandboxViolation {
                details: "d".into(),
            }
        );
        assert_eq!(
            ErrorLayer::SubAgentNestingForbidden.to_nexus_error(),
            NexusError::NestedSubAgentForbidden
        );
        assert_eq!(
            ErrorLayer::ApprovalDenied {
                operation: "op".into(),
            }
            .to_nexus_error(),
            NexusError::ApprovalDenied {
                operation: "op".into(),
            }
        );
        assert_eq!(
            ErrorLayer::ModelApiError {
                status: 500,
                message: "m".into(),
            }
            .to_nexus_error(),
            NexusError::ModelApiError {
                status: 500,
                message: "m".into(),
            }
        );
    }

    /// `#[error(transparent)]` 委托 + 逐字消息复制无漂移:Display 恒等(全 10 kind)
    #[test]
    fn error_layer_display_matches_target_display() {
        for layer in all_error_layer_kinds() {
            // to_nexus_error 消费 self,先 clone 再映射保持原值可读 Display
            let target = layer.clone().to_nexus_error();
            assert_eq!(
                layer.to_string(),
                target.to_string(),
                "ErrorLayer Display 必须与映射目标 NexusError Display 逐字一致"
            );
        }
    }

    /// serde JSON roundtrip 恒等(全 10 kind 代表值;NexusError 同款先例)
    #[test]
    fn error_layer_serde_json_roundtrip() {
        for layer in all_error_layer_kinds() {
            let json = serde_json::to_string(&layer).expect("JSON 序列化失败");
            let decoded: ErrorLayer = serde_json::from_str(&json).expect("JSON 反序列化失败");
            assert_eq!(decoded, layer, "serde JSON roundtrip 必须恒等");
        }
    }

    /// `From<ErrorLayer>` 委托 to_nexus_error:newtype/struct/unit 三形态各取代表
    #[test]
    fn from_impl_delegates_to_to_nexus_error() {
        // newtype 形态(Event 子枚举包裹)
        let layered = ErrorLayer::Event(EventFailure::InvalidEventType("t".into()));
        assert_eq!(NexusError::from(layered.clone()), layered.to_nexus_error());
        // struct 形态(命名字段)
        let layered = ErrorLayer::MemoryBudgetExceeded {
            current: 101,
            max: 100,
        };
        assert_eq!(NexusError::from(layered.clone()), layered.to_nexus_error());
        // unit 形态
        let layered = ErrorLayer::SubAgentNestingForbidden;
        assert_eq!(NexusError::from(layered.clone()), layered.to_nexus_error());
    }

    /// 映射后恢复策略透传不变(全 10 kind,按 recovery_strategy 表硬编码;
    /// 顺序 = Event×3 / Memory / Tool×2 / Security / SubAgent / Approval / Model)
    #[test]
    fn mapped_recovery_strategy_equals_direct_construction() {
        let expectations: Vec<(ErrorLayer, RecoveryStrategy)> = vec![
            (
                ErrorLayer::Event(EventFailure::SerializationError("s".into())),
                RecoveryStrategy::Retry { max_attempts: 2 },
            ),
            (
                ErrorLayer::Event(EventFailure::InvalidEventType("t".into())),
                RecoveryStrategy::Retry { max_attempts: 2 },
            ),
            (
                ErrorLayer::Event(EventFailure::QuotaExceeded {
                    namespace: "ns".into(),
                    current: 1,
                    max: 10,
                }),
                RecoveryStrategy::FailFast,
            ),
            (
                ErrorLayer::MemoryBudgetExceeded {
                    current: 120_000,
                    max: 100_000,
                },
                RecoveryStrategy::CompressAndRetry,
            ),
            (
                ErrorLayer::Tool(ToolFailure::Timeout {
                    tool_name: "bash".into(),
                    duration_ms: 30_000,
                }),
                RecoveryStrategy::RetryWithBackoff,
            ),
            (
                ErrorLayer::Tool(ToolFailure::McpDisconnected {
                    server_name: "github".into(),
                }),
                RecoveryStrategy::FallbackToBuiltin,
            ),
            (
                ErrorLayer::SecuritySandboxViolation {
                    details: "d".into(),
                },
                RecoveryStrategy::FailFast,
            ),
            (
                ErrorLayer::SubAgentNestingForbidden,
                RecoveryStrategy::FailFast,
            ),
            (
                ErrorLayer::ApprovalDenied {
                    operation: "op".into(),
                },
                RecoveryStrategy::FailFast,
            ),
            (
                ErrorLayer::ModelApiError {
                    status: 500,
                    message: "m".into(),
                },
                RecoveryStrategy::RetryWithBackoff,
            ),
        ];
        assert_eq!(
            expectations.len(),
            10,
            "kind 新增必须同步登记恢复策略预期(穷尽覆盖顺势增长)"
        );
        for (layer, expected) in expectations {
            assert_eq!(
                layer.to_nexus_error().recovery_strategy(),
                expected,
                "映射后恢复策略必须与直接构造一致"
            );
        }
    }

    // ============================================================
    // proptest 属性(任意 payload 下的全称不变量)
    // ============================================================
    //
    // 遵循项目 proptest 纪律(§4.1 + archive_monotonicity.rs 同款):
    // - block-named 语法;prop_assert! 全部显式位置参数(不支持 {var} 内联捕获)
    // - 宏体内用普通注释禁 doc comment(unused_doc_comments 陷阱)
    // - ProptestConfig::with_cases(256)

    /// 10 分支等权 prop_oneof:各 kind 由任意 payload 生成器 prop_map 构造
    fn arb_error_layer() -> impl proptest::strategy::Strategy<Value = ErrorLayer> {
        use proptest::prelude::*;
        prop_oneof![
            any::<String>().prop_map(|s| ErrorLayer::Event(EventFailure::SerializationError(s))),
            any::<String>().prop_map(|t| ErrorLayer::Event(EventFailure::InvalidEventType(t))),
            (any::<String>(), any::<usize>(), any::<usize>()).prop_map(
                |(namespace, current, max)| ErrorLayer::Event(EventFailure::QuotaExceeded {
                    namespace,
                    current,
                    max,
                })
            ),
            (any::<usize>(), any::<usize>())
                .prop_map(|(current, max)| ErrorLayer::MemoryBudgetExceeded { current, max }),
            (any::<String>(), any::<u64>()).prop_map(|(tool_name, duration_ms)| {
                ErrorLayer::Tool(ToolFailure::Timeout {
                    tool_name,
                    duration_ms,
                })
            }),
            any::<String>().prop_map(|server_name| ErrorLayer::Tool(
                ToolFailure::McpDisconnected { server_name }
            )),
            any::<String>().prop_map(|details| ErrorLayer::SecuritySandboxViolation { details }),
            Just(ErrorLayer::SubAgentNestingForbidden),
            any::<String>().prop_map(|operation| ErrorLayer::ApprovalDenied { operation }),
            (any::<u16>(), any::<String>())
                .prop_map(|(status, message)| { ErrorLayer::ModelApiError { status, message } }),
        ]
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        // 任意 payload(空串/unicode/usize 极值)下 serde JSON roundtrip 恒等(全称性质)
        #[test]
        fn error_layer_json_roundtrip_is_identity(layer in arb_error_layer()) {
            let json = serde_json::to_string(&layer).expect("JSON 序列化失败");
            let decoded: ErrorLayer = serde_json::from_str(&json).expect("JSON 反序列化失败");
            proptest::prop_assert_eq!(decoded, layer);
        }

        // 任意 payload 下 Display 映射无漂移:layer.to_string() == 映射目标 to_string()
        #[test]
        fn error_layer_mapping_preserves_display(layer in arb_error_layer()) {
            let target = layer.clone().to_nexus_error();
            proptest::prop_assert_eq!(layer.to_string(), target.to_string());
        }
    }
}
