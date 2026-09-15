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
//! # Err 文案格式约定（2026-09-12 修正,双重编码缺陷）
//!
//! `Err` 文案**必须是纯文本,不得自行包裹 JSON**:
//! `PlanRunner::exec_node` 会把 `Err(e)` 包装为 `{"error":{e:?}}` 作为节点
//! 结果回填模型 —— 若本执行器的 Err 已是 JSON 字符串,将产生
//! `{"error":"{\"error\":\"denied\"...}"}` 的**双重编码**,模型侧无法直接解析。
//! 修正为纯文本（`denied: ...` / `ask_required: ...`）后,包装结果为单层
//! 合法 JSON 且 `error` 字段可直接读取。
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
            //
            // WHY 纯文本而非 JSON（双重编码修正,2026-09-12）:PlanRunner 会把
            // `Err(e)` 包装为 `{"error":{e:?}}` 回填模型 —— 本执行器若返回 JSON
            // 文案将产生嵌套转义(模型无法直接解析)。纯文本经包装后为单层合法
            // JSON 且 `error` 字段可直接读取。关键词 `denied`/`ask_required`
            // 保留供调用方审计/审批流路由。
            PolicyAction::Deny => Err(format!(
                "denied: execpolicy (tool={tool_name})"
            )),
            PolicyAction::Ask => Err(format!(
                "ask_required: execpolicy needs approval (tool={tool_name})"
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

    /// 双重编码回归守卫（2026-09-12 修正）:Deny/Ask 的 Err 文案**不得是 JSON**
    ///
    /// WHY:PlanRunner 把 `Err(e)` 包装为 `{"error":{e:?}}` 回填模型 —— 若 Err
    /// 文案本身是 JSON（旧实现），包装结果为嵌套转义 `{"error":"{\"error\":...}"}`,
    /// 模型无法直接解析。纯文本经包装后为单层合法 JSON。
    ///
    /// 两个子场景各自构造对应策略:Deny（零信任默认）与 Ask（显式 Ask 规则）。
    #[tokio::test]
    async fn deny_and_ask_errors_are_plain_text_not_json() {
        use seccore::execpolicy::{ExecPolicyRule, RulePattern};

        let delegate = Arc::new(RecordingExecutor::default());
        // 子场景 1:Deny(零信任默认全拒)
        let deny_ex = ExecPolicyToolExecutor::new(
            Arc::new(ExecPolicy::new()),
            delegate.clone(),
        );
        let err = deny_ex
            .execute("search:docs", r#"{"q":"x"}"#)
            .await
            .expect_err("Deny 路径必须 Err");
        assert!(
            !err.trim_start().starts_with('{'),
            "Err 文案不得是 JSON(会与 PlanRunner 包装双重编码): {err}"
        );
        assert!(err.contains("denied"), "Deny 文案须含 denied: {err}");
        assert!(err.contains("search:docs"), "文案须携带工具名供定位: {err}");

        // 子场景 2:Ask(显式 Ask 规则)
        let ask_policy = ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("bash", "*"),
            action: PolicyAction::Ask,
        });
        let ask_ex = ExecPolicyToolExecutor::new(Arc::new(ask_policy), delegate.clone());
        let err = ask_ex
            .execute("bash", r#"{"cmd":"git push origin"}"#)
            .await
            .expect_err("Ask 路径必须 Err");
        assert!(
            !err.trim_start().starts_with('{'),
            "Err 文案不得是 JSON: {err}"
        );
        assert!(err.contains("ask_required"), "Ask 文案须含 ask_required: {err}");
        assert!(
            delegate.calls.lock().unwrap().is_empty(),
            "Deny/Ask 均不得委托执行"
        );
    }

    /// 端到端:Deny 工具经 PlanRunner 后,summary 为**单层**合法 JSON 且
    /// `error` 字段为纯文本字符串(修复前:嵌套转义字符串,模型无法解析)
    #[tokio::test]
    async fn denied_tool_produces_single_layer_json_summary() {
        use crate::toolplan_runner::{PlanGuards, PlanRunner};
        use nexus_contracts::tool_plan::{ToolNode, ToolOp, ToolPlan};

        let policy = ExecPolicy::new(); // 零信任:全部 deny
        let ex = ExecPolicyToolExecutor::new(Arc::new(policy), Arc::new(RecordingExecutor::default()));
        let runner = PlanRunner::new(Box::new(ex), PlanGuards::default());

        let plan = ToolPlan {
            id: "plan-deny".into(),
            nodes: vec![ToolNode {
                id: "fetch".into(),
                op: ToolOp::ToolCall,
                tool_name: Some("search:docs".into()),
                args_json: Some(r#"{"q":"rust"}"#.into()),
                field: None,
                predicate: None,
                side_effect: Some(nexus_contracts::tool_plan::SideEffectDecl::ReadOnly),
            }],
            edges: vec![],
        };

        let summary = runner.run(&plan).await.expect("单节点合法计划必须执行");
        // summary 即末节点结果:PlanRunner 包装的 {"error":"..."}
        let parsed: serde_json::Value =
            serde_json::from_str(&summary.summary).expect("summary 必须是合法 JSON");
        let error_field = parsed.get("error").expect("必须含 error 字段");
        assert!(
            error_field.is_string(),
            "error 字段必须是纯文本字符串(单层编码),实际: {error_field}"
        );
        let text = error_field.as_str().expect("字符串类型");
        assert!(
            !text.trim_start().starts_with('{'),
            "error 文本不得再是嵌套 JSON(双重编码回归): {text}"
        );
        assert!(text.contains("denied"), "应含拒绝语义: {text}");
    }
}
