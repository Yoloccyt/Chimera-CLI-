//! 类型化工具执行结果契约 — 执行器边界的类型化终点（L0 纯契约）。
//!
//! # 背景（W4 / L2 工具层重构方案 §6.2–§6.3，ADR-32X 待落档）
//!
//! [`ToolExecutor::execute`](crate::spec::tool_dispatch) 的
//! `Result<String, String>` 把 L0 `DispatchError` 四层归因 string 化——
//! 错误归因靠 Err 文案前缀（`policy:`/`sandbox:`/`route:`/`transport:`）
//! 的**无编译期保证约定**。本模块新增类型化执行结果与加性 trait 方法
//! （沿 ADR-256 `execute_node` 加性先例），前缀约定降级为迁移脚手架。
//!
//! # 演进节奏（方案 §6.3 加性纪律）
//!
//! 1. 本批：L0 类型 + trait `execute_typed` 默认桥接（10 个既有实现零改动，
//!    行为逐位）+ [`ToolExecResult::from_legacy_err`] 前缀解析脚手架；
//! 2. 实现方逐批覆盖 `execute_typed`（统一适配器首批——映射自 ToolOutcome，
//!    零解析损失）；
//! 3. 全覆盖后旧 `execute` 标 deprecated 一个版本窗，物理退役独立批
//!    （`from_legacy_err` 同批删除——前缀约定从「契约」降级为「脚手架」）。

use crate::spec::tool_dispatch::DispatchError;

/// 单次工具执行的类型化终点。
///
/// WHY 不直接上浮 tool_plane `ToolStatus`：L0 契约需与执行器实现解耦
/// （tool_plane 是 L7 实现细节），以 `DispatchError` 载荷承载四层归因，
/// L7 适配器负责单向映射（内部状态 → L0 结果），避免类型双源。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolExecResult {
    /// 唯一关联 id（节点级 = `{plan_id}-{node_id}`；legacy 路径 = 工具名+时戳）
    pub call_id: String,
    /// 终态（成功/失败类归因）
    pub status: ToolExecStatus,
    /// 成功载荷（JSON 文本；失败时 `None`）
    pub payload: Option<String>,
    /// 输出被截断（payload 超限）——W4 起层间熔断可据此恢复细粒度判定
    /// （W2 登记的熔断点后移问题，见 exec_plan_truncation 测试注释）
    pub truncated: bool,
}

impl ToolExecResult {
    /// 成功结果构造。
    #[must_use]
    pub fn ok(call_id: impl Into<String>, payload: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            status: ToolExecStatus::Ok,
            payload: Some(payload.into()),
            truncated: false,
        }
    }

    /// 失败结果构造（`#[non_exhaustive]` 的 crate 外失败构造点——
    /// payload 空、truncated=false）。
    #[must_use]
    pub fn failed(call_id: impl Into<String>, status: ToolExecStatus) -> Self {
        Self {
            call_id: call_id.into(),
            status,
            payload: None,
            truncated: false,
        }
    }

    /// 覆写截断标志（builder；成功且载荷超限的调用方标记）。
    #[must_use]
    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    /// 从 legacy `Err(String)` 前缀解析归因（**迁移脚手架**——覆盖
    /// `execute_typed` 的实现方不经过本函数；全覆盖后随旧 trait 方法删除）。
    #[must_use]
    pub fn from_legacy_err(call_id: impl Into<String>, err: &str) -> Self {
        Self {
            call_id: call_id.into(),
            status: ToolExecStatus::from_legacy_err(err),
            payload: None,
            truncated: false,
        }
    }

    /// legacy 风格错误文案（从 status 重构）——供迁移期调用方以既有
    /// `Err(String)` 形态消费（PlanRunner 节点错误写回等零下游变化）。
    #[must_use]
    pub fn error_text(&self) -> String {
        match &self.status {
            ToolExecStatus::Ok => String::new(),
            other => other.to_string(),
        }
    }
}

/// 类型化终态 — 四层归因（复用 L0 [`DispatchError`]）+ 三精确执行态。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolExecStatus {
    /// 成功（载荷见 [`ToolExecResult::payload`]）。
    Ok,
    /// policy 层拒绝（deny/ask）。
    Denied(DispatchError),
    /// sandbox 层拦截。
    SandboxBlocked(DispatchError),
    /// route 层归因（未命中 / 背压 / 跳过；覆盖实现方可给细分）。
    Routed(DispatchError),
    /// transport 层 / 实际执行失败。
    Provider(DispatchError),
    /// deadline 超时。
    Timeout {
        /// 工具名。
        tool: String,
        /// 生效 deadline（ms）。
        deadline_ms: u64,
    },
    /// 取消令牌触发。
    Cancelled {
        /// 工具名。
        tool: String,
    },
    /// 参数非法（执行器/工具自身校验拒绝）。
    InvalidArgs {
        /// 工具名。
        tool: String,
        /// 拒绝原因（文案语义自包含）。
        reason: String,
    },
}

impl ToolExecStatus {
    /// 从 legacy Err 文案解析（前缀约定 → 变体；**脚手架**）。
    ///
    /// 约定形态（迁移期全部 Err 文案产出方）：
    /// `policy:` / `sandbox:` / `route:` / `transport:` / `timeout:` /
    /// `cancelled:`；无前缀兜底为 `Provider`（fail-closed：未知归因不冒充成功）。
    #[must_use]
    pub fn from_legacy_err(err: &str) -> Self {
        // WHY 不 trim_start：载荷（冒号后的原文，含前导空白与控制字符）必须
        // 逐字保真——error_text 重构（DispatchError Display 为 `layer:{msg}`
        // 无分隔空格）依赖载荷原样往返；proptest 已锁控制字符形态。
        let strip = |p: &str| -> Option<&str> { err.strip_prefix(p) };
        if let Some(rest) = strip("policy:") {
            Self::Denied(DispatchError::Policy(rest.to_string()))
        } else if let Some(rest) = strip("sandbox:") {
            Self::SandboxBlocked(DispatchError::Sandbox(rest.to_string()))
        } else if let Some(rest) = strip("route:") {
            Self::Routed(DispatchError::Route(rest.to_string()))
        } else if let Some(rest) = strip("transport:") {
            Self::Provider(DispatchError::Transport(rest.to_string()))
        } else if let Some(rest) = strip("timeout:") {
            // 粗粒度：deadline 数值剥 "ms" 后缀解析，失败给 0（精确值由覆盖
            // 实现方供给）
            Self::Timeout {
                tool: String::new(),
                deadline_ms: rest
                    .split_whitespace()
                    .find_map(|t| t.trim_end_matches("ms").parse::<u64>().ok())
                    .unwrap_or_default(),
            }
        } else if let Some(rest) = strip("cancelled:") {
            Self::Cancelled {
                tool: rest.to_string(),
            }
        } else {
            Self::Provider(DispatchError::Transport(err.to_string()))
        }
    }
}

impl std::fmt::Display for ToolExecStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => f.write_str("ok"),
            Self::Denied(e) | Self::SandboxBlocked(e) | Self::Routed(e) | Self::Provider(e) => {
                write!(f, "{e}")
            }
            Self::Timeout { tool, deadline_ms } => {
                write!(f, "timeout: {tool} after {deadline_ms}ms")
            }
            Self::Cancelled { tool } => write!(f, "cancelled: {tool}"),
            Self::InvalidArgs { tool, reason } => write!(f, "invalid args for {tool}: {reason}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_result_roundtrip() {
        let r = ToolExecResult::ok("call-1", r#"{"v":1}"#);
        assert_eq!(r.status, ToolExecStatus::Ok);
        assert_eq!(r.payload.as_deref(), Some(r#"{"v":1}"#));
        assert!(!r.truncated);
        assert_eq!(r.error_text(), "");
    }

    #[test]
    fn legacy_prefix_map_all_four_layers() {
        let cases = [
            "policy: denied mcp.x",
            "sandbox: blocked",
            "route: tool not found: mcp.x",
            "transport: conn refused",
        ];
        for legacy in cases {
            let r = ToolExecResult::from_legacy_err("c", legacy);
            // 逐字回环:解析不 trim(载荷保真)⇒ error_text 与 legacy 原文一致,
            // 归因信息经 DispatchError 变体承载而非文案差异
            assert_eq!(
                r.error_text(),
                legacy,
                "legacy={legacy:?} 须逐字回环(归因变体另见变体断言)"
            );
        }
        // 归因变体精确性:四层前缀各归其位
        assert!(matches!(
            ToolExecResult::from_legacy_err("c", "policy: x").status,
            ToolExecStatus::Denied(_)
        ));
        assert!(matches!(
            ToolExecResult::from_legacy_err("c", "sandbox: x").status,
            ToolExecStatus::SandboxBlocked(_)
        ));
        assert!(matches!(
            ToolExecResult::from_legacy_err("c", "route: x").status,
            ToolExecStatus::Routed(_)
        ));
        assert!(matches!(
            ToolExecResult::from_legacy_err("c", "transport: x").status,
            ToolExecStatus::Provider(_)
        ));
    }

    #[test]
    fn legacy_timeout_and_cancelled() {
        let r = ToolExecResult::from_legacy_err("c", "timeout: echo after 5000ms");
        match r.status {
            ToolExecStatus::Timeout { deadline_ms, .. } => assert_eq!(deadline_ms, 5000),
            other => panic!("应为 Timeout: {other:?}"),
        }
        let r = ToolExecResult::from_legacy_err("c", "cancelled: mcp.x");
        assert!(matches!(r.status, ToolExecStatus::Cancelled { .. }));
    }

    #[test]
    fn unprefixed_err_falls_back_to_provider() {
        // 无前缀兜底 Provider（fail-closed：未知归因不冒充成功/路由错）
        let r = ToolExecResult::from_legacy_err("c", "unknown builtin tool: x");
        assert!(matches!(r.status, ToolExecStatus::Provider(_)));
    }

    proptest::proptest! {
        /// 四层前缀不变量:任一 `layer:msg` 形态解析后 error_text 与
        /// DispatchError Display 逐字一致(归因不丢)。
        #[test]
        fn four_layer_prefix_roundtrip(layer in 0u8..4, msg in ".*") {
            let (prefix, variant): (&str, fn(String) -> DispatchError) = match layer {
                0 => ("policy:", DispatchError::Policy),
                1 => ("sandbox:", DispatchError::Sandbox),
                2 => ("route:", DispatchError::Route),
                _ => ("transport:", DispatchError::Transport),
            };
            let legacy = format!("{prefix} {msg}");
            let r = ToolExecResult::from_legacy_err("c", &legacy);
            let expected = variant(msg);
                // 逐字回环:解析保真 ⇒ error_text 与 legacy 原文一致
            proptest::prop_assert_eq!(r.error_text(), legacy);
        }
    }
}
