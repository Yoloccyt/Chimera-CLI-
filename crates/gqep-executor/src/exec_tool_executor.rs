//! exec_tool_executor — ToolExecutor × execpolicy 审批流水线接线（P4-T3①，WI-16 安全不变量）
//!
//! 对应架构层: **L7 Execution**（gqep-executor）
//! 对应任务: **P4-T3**（W20 集成周:Phase 3 遗留接线①）
//!
//! # 安全不变量（WI-16 原文兑现）
//! "计划内每个 tool_call 子节点仍走 execpolicy 审批/沙箱/超时/审计完整流水线"——
//! 本执行器将 [`seccore::execpolicy::ExecPolicy`] 规则引擎挂载到
//! [`ToolExecutor`] 前置检查,决策全量计入 DecisionStats 审计:
//! - Allow → 放行执行
//! - Ask → 拒绝执行并返回 `ask_required` 错误（审批流由上层接入）
//! - Deny → 拒绝执行并返回 `denied` 错误
//!
//! # 组合根
//! 组合根装配:真实工具执行闭包（delegate）+ ExecPolicy 规则。委托执行与
//! 策略检查解耦——delegate 可为任意宿主实现（进程调用/MCP 调用/内部函数）。

use std::sync::Arc;

use async_trait::async_trait;

use crate::toolplan_runner::ToolExecutor;
use seccore::execpolicy::{ExecPolicy, PolicyAction};

/// execpolicy 挂载的工具执行器 — 策略前置检查 + 委托执行 + 全量审计
pub struct ExecPolicyToolExecutor {
    /// execpolicy 规则引擎（Arc 共享,决策统计跨调用累计）
    policy: Arc<ExecPolicy>,
    /// 委托执行器（策略放行后的真实执行;可为宿主任意实现）
    delegate: Arc<dyn ToolExecutor>,
}

impl std::fmt::Debug for ExecPolicyToolExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Arc<dyn ToolExecutor> 无 Debug（trait 对象边界）
        f.debug_struct("ExecPolicyToolExecutor")
            .finish_non_exhaustive()
    }
}

impl ExecPolicyToolExecutor {
    /// 新建 — 策略引擎 + 委托执行器
    #[must_use]
    pub fn new(policy: Arc<ExecPolicy>, delegate: Arc<dyn ToolExecutor>) -> Self {
        Self { policy, delegate }
    }

    /// 策略引擎引用（审计导出:DecisionStats 快照）
    #[must_use]
    pub fn policy(&self) -> &ExecPolicy {
        &self.policy
    }
}

#[async_trait]
impl ToolExecutor for ExecPolicyToolExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        // execpolicy 前置检查（program=工具名,args=[args_json];决策计入审计）
        let action = self.policy.evaluate(tool_name, &[args_json.to_string()]);
        match action {
            // Allow/Ask 之外的 Deny:拒绝执行（Ask 由上层审批流处置,此处同样不放行）
            PolicyAction::Deny => Err(format!(
                r#"{{"error":"denied","tool":"{tool_name}","reason":"execpolicy deny"}}"#
            )),
            PolicyAction::Ask => Err(format!(
                r#"{{"error":"ask_required","tool":"{tool_name}","reason":"execpolicy ask: needs approval"}}"#
            )),
            PolicyAction::Allow => self.delegate.execute(tool_name, args_json).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seccore::execpolicy::{ExecPolicyRule, RulePattern};

    /// 委托执行器 — 记录调用（验证 Deny/Ask 时零调用）
    #[derive(Debug, Default)]
    struct RecordingExecutor {
        calls: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ToolExecutor for RecordingExecutor {
        async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!("{tool_name}:{args_json}"));
            Ok(format!("ok:{tool_name}"))
        }
    }

    /// Allow 放行 — 委托执行,结果透传
    #[tokio::test]
    async fn allow_passes_through() {
        let policy = ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("search:docs", "*"),
            action: PolicyAction::Allow,
        });
        let delegate = Arc::new(RecordingExecutor::default());
        let ex = ExecPolicyToolExecutor::new(Arc::new(policy), delegate.clone());
        let out = ex.execute("search:docs", r#"{"q":"rust"}"#).await;
        assert_eq!(out.expect("放行必须成功"), "ok:search:docs");
        assert_eq!(delegate.calls.lock().unwrap().len(), 1, "委托必须被调用");
    }

    /// Deny 拒绝 — 零委托调用,错误含 denied
    #[tokio::test]
    async fn deny_blocks_execution() {
        let policy = ExecPolicy::new(); // 零信任默认全拒
        let delegate = Arc::new(RecordingExecutor::default());
        let ex = ExecPolicyToolExecutor::new(Arc::new(policy), delegate.clone());
        let out = ex.execute("search:docs", r#"{"q":"x"}"#).await;
        assert!(out.is_err());
        assert!(out.unwrap_err().contains("denied"), "错误必须含 denied");
        assert!(
            delegate.calls.lock().unwrap().is_empty(),
            "拒绝后零委托调用"
        );
    }

    /// Ask 路径 — 返回 ask_required（审批流由上层接入）
    #[tokio::test]
    async fn ask_returns_approval_error() {
        let policy = ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("bash", "*"),
            action: PolicyAction::Ask,
        });
        let delegate = Arc::new(RecordingExecutor::default());
        let ex = ExecPolicyToolExecutor::new(Arc::new(policy), delegate.clone());
        let out = ex.execute("bash", r#"{"cmd":"git push origin"}"#).await;
        assert!(out.is_err());
        assert!(out.unwrap_err().contains("ask_required"));
        assert!(delegate.calls.lock().unwrap().is_empty(), "Ask 不直接委托");
    }

    /// 决策审计 — 全量留痕（allow/deny 分桶）
    #[tokio::test]
    async fn decision_audit_accumulates() {
        let policy = ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("search:docs", "*"),
            action: PolicyAction::Allow,
        });
        let delegate = Arc::new(RecordingExecutor::default());
        let ex = ExecPolicyToolExecutor::new(Arc::new(policy), delegate.clone());
        let _ = ex.execute("search:docs", "{}").await; // allow
        let _ = ex.execute("other:tool", "{}").await; // deny
        let stats = ex.policy().decision_stats();
        assert_eq!(stats.allow_count(), 1);
        assert_eq!(stats.deny_count(), 1);
        assert_eq!(stats.total(), 2, "全量决策留痕");
    }
}
