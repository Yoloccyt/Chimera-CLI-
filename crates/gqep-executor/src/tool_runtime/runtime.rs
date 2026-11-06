//! 工具运行时 — 生产接线的唯一组装点（T1-2）
//!
//! 把四环链（policy → sandbox → route → transport）与真实执行者装配成一个
//! 可直接使用的运行时：链头做准入前置，链尾落到进程内执行。
//!
//! # WHY 双策略评估（刻意保留两处 `ExecPolicy` 判定）
//!
//! 一次允许的派发会经过**两次**策略评估：
//! 1. `chain::PolicyLayer`（链头）—— 准入前置，保证 Deny/Ask 在任何
//!    后续环（含沙箱与执行）之前短路；
//! 2. [`ExecPolicyToolExecutor`]（嵌在 [`InProcDispatcher`] 之内）—— 保证该执行者
//!    被**单独**作为 `ToolExecutor` 复用时（`PlanRunner::new(Box::new(ex))` 等既有
//!    接线）仍受同一策略约束，不会因为绕过 `ToolRuntime` 而失去审批。
//!
//! 两处使用**同源同策略**（`Arc::clone(&policy)`），因此判定结果一致；代价是
//! `DecisionStats` 会计两次决策。这是预期的防御性重复：安全前置宁可多算一次，
//! 也不能让「换一条调用路径就绕过策略」成为可能。实测开销见 bench
//! `tool_runtime_chain`（策略评估为内存规则匹配，无 IO）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use seccore::execpolicy::ExecPolicy;

use super::batch::{dispatch_batch, BatchLimits, BatchOutcome};
use super::chain::{DispatcherBuilder, SandboxedDispatcher};
use super::dispatch::{DispatchError, DispatchFuture, DispatchRequest, ToolDispatcher};
use super::inproc::InProcDispatcher;
use super::sandbox_gate::SeccoreSandboxGate;
use crate::exec_tool_executor::ExecPolicyToolExecutor;
use crate::toolplan_runner::ToolExecutor;

/// 工具运行时 — 强制沙箱四环链的成品装配
pub struct ToolRuntime {
    /// 四环齐备的派发器（唯一构造入口为 [`DispatcherBuilder::build`]）
    dispatcher: SandboxedDispatcher,
}

impl std::fmt::Debug for ToolRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRuntime")
            .field("dispatcher", &self.dispatcher)
            .finish()
    }
}

impl ToolRuntime {
    /// 生产接线：构造 `ExecPolicyToolExecutor` 并组装四环链
    ///
    /// 这是**唯一**在非测试代码中构造 [`ExecPolicyToolExecutor`] 的位置。
    ///
    /// # 参数
    /// - `delegate`: 真实工具执行者（进程调用 / MCP 调用 / 内部函数）
    /// - `policy`: execpolicy 规则引擎（链头与执行器共享同一 `Arc`）
    ///
    /// # 返回
    /// 缺环时返回 [`DispatchError::Route`]（组装期错误，消息含 `missing <环名>`）；
    /// 本函数提供的五环参数恒齐备，故实际只会在 builder 契约变更时才报错。
    pub fn new(
        delegate: Arc<dyn ToolExecutor>,
        policy: Arc<ExecPolicy>,
    ) -> Result<Self, DispatchError> {
        let guarded: Arc<dyn ToolExecutor> =
            Arc::new(ExecPolicyToolExecutor::new(Arc::clone(&policy), delegate));
        let inproc: Arc<dyn ToolDispatcher> = Arc::new(InProcDispatcher::new(guarded));
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(Arc::clone(&policy)))
            .with_sandbox(Some(Arc::new(SeccoreSandboxGate::default_secure())))
            // 显式空路由表：全部流量走 fallback（单进程内没有按工具名的分流需求）
            .with_route(Some(HashMap::new()))
            .with_fallback(Some(Arc::clone(&inproc)))
            .with_transport(Some(inproc))
            .build()?;
        Ok(Self { dispatcher })
    }

    /// 派发一次请求（四环链入口）
    pub fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        self.dispatcher.dispatch(req)
    }

    /// 批量派发 — 有界并发 + 输入序结果（性能增量 2026-11-06）
    ///
    /// # 语义契约
    /// `outcome.results[i]` 恒对应 `requests[i]`；在 `deadline_ms = 0` 且不触发
    /// 单操作超时时，逐位结果与「按顺序逐条 `dispatch`」完全一致（由
    /// `tests/tool_runtime_batch_proptest.rs` 的等价性 property 锁定）。
    ///
    /// # 为什么能并行
    /// 请求之间无数据依赖、无共享可变状态；每个请求仍独立经完整的
    /// policy → sandbox → route → transport 四环，安全前置**不因批量而减少**。
    ///
    /// # 为什么放在 L7
    /// 并发编排（上界 / 双层超时 / 顺序回填）属执行层职责；CLI 等消费方
    /// 只按输入序取结果，不自己拼并发。
    pub async fn execute_batch(
        &self,
        requests: &[DispatchRequest],
        limits: BatchLimits,
    ) -> BatchOutcome {
        dispatch_batch(&self.dispatcher, requests, limits).await
    }
}

#[async_trait]
impl ToolExecutor for ToolRuntime {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        let req = DispatchRequest {
            tool_name: tool_name.to_string(),
            args_json: args_json.to_string(),
        };
        // 错误统一走 Display（`policy:` / `sandbox:` / `route:` / `transport:` 前缀），
        // 供 PlanRunner 包装为单层 JSON 的 `error` 文本（勿在此自行包裹 JSON）。
        self.dispatch(&req)
            .await
            .map(|outcome| outcome.content_json)
            .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use seccore::execpolicy::{ExecPolicyRule, RulePattern};
    use seccore::PolicyAction;

    /// 回显型委托执行器 + 调用计数（验证「拒绝时零委托」）
    struct CountingExecutor {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ToolExecutor for CountingExecutor {
        async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(args_json.to_string())
        }
    }

    fn delegate(calls: &Arc<AtomicUsize>) -> Arc<dyn ToolExecutor> {
        Arc::new(CountingExecutor {
            calls: Arc::clone(calls),
        })
    }

    fn request(tool_name: &str, args_json: &str) -> DispatchRequest {
        DispatchRequest {
            tool_name: tool_name.to_string(),
            args_json: args_json.to_string(),
        }
    }

    fn allow_policy(tool: &str) -> Arc<ExecPolicy> {
        Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new(tool, "*"),
            action: PolicyAction::Allow,
        }))
    }

    /// 允许规则：派发成功、内容回显、委托恰好 1 次
    #[tokio::test]
    async fn allow_rule_dispatches_and_echoes_content() {
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime =
            ToolRuntime::new(delegate(&calls), allow_policy("echo")).expect("五环齐备应成功");

        let outcome = runtime
            .dispatch(&request("echo", r#"{"k":"v"}"#))
            .await
            .expect("允许策略应成功");
        assert_eq!(outcome.content_json, r#"{"k":"v"}"#);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "委托应被调用 1 次");
    }

    /// 零信任（无规则）：链头即拒，委托 0 次
    #[tokio::test]
    async fn zero_trust_policy_denies_before_delegate() {
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = ToolRuntime::new(delegate(&calls), Arc::new(ExecPolicy::new()))
            .expect("零信任策略同样是合法装配");

        let result = runtime.dispatch(&request("echo", "{}")).await;
        assert!(
            matches!(result, Err(DispatchError::Policy(_))),
            "无规则应归因为 Policy，实际: {result:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "拒绝后零委托调用");
    }

    /// Ask 规则：文案含 `ask_required`，委托 0 次
    #[tokio::test]
    async fn ask_rule_reports_ask_required() {
        let calls = Arc::new(AtomicUsize::new(0));
        let policy = Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("bash", "*"),
            action: PolicyAction::Ask,
        }));
        let runtime = ToolRuntime::new(delegate(&calls), policy).expect("装配应成功");

        match runtime
            .dispatch(&request("bash", r#"{"cmd":"git push"}"#))
            .await
        {
            Err(DispatchError::Policy(msg)) => {
                assert!(msg.contains("ask_required"), "ask 应显式标记: {msg}")
            }
            other => panic!("期望 Policy ask_required，实际: {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0, "Ask 不直接委托");
    }

    /// 注入形态参数：策略允许后仍被沙箱层拦截，归因 Sandbox
    #[tokio::test]
    async fn injection_args_blocked_by_sandbox() {
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime =
            ToolRuntime::new(delegate(&calls), allow_policy("repo.search")).expect("装配应成功");

        let result = runtime
            .dispatch(&request("repo.search", "$(rm -rf /)"))
            .await;
        assert!(
            matches!(result, Err(DispatchError::Sandbox(_))),
            "注入形态应归因为 Sandbox，实际: {result:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "沙箱拒绝后零委托调用");
    }

    /// `ToolExecutor` 路径：错误字符串带 `policy:` 前缀（供上层按前缀路由）
    #[tokio::test]
    async fn execute_error_carries_policy_prefix() {
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime =
            ToolRuntime::new(delegate(&calls), Arc::new(ExecPolicy::new())).expect("装配应成功");

        let err = runtime
            .execute("bash", r#"{"cmd":"rm -rf /"}"#)
            .await
            .expect_err("零信任策略必须 Err");
        assert!(err.starts_with("policy:"), "错误应带 policy: 前缀: {err}");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "拒绝后零委托调用");
    }

    /// `ToolExecutor` 路径：允许策略下返回内容
    #[tokio::test]
    async fn execute_returns_content_on_allow() {
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = ToolRuntime::new(delegate(&calls), allow_policy("echo")).expect("装配应成功");

        let out = runtime
            .execute("echo", r#"{"k":"v"}"#)
            .await
            .expect("允许策略应成功");
        assert_eq!(out, r#"{"k":"v"}"#);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "委托应被调用 1 次");
    }

    /// 全链批量：与逐条串行**逐位一致**，且拒绝项不触达委托
    #[tokio::test]
    async fn execute_batch_matches_serial_through_full_chain() {
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = ToolRuntime::new(delegate(&calls), allow_policy("echo")).expect("装配应成功");
        let requests = vec![
            request("echo", r#"{"i":0}"#),
            request("blocked", r#"{"i":1}"#),
            request("echo", r#"{"i":2}"#),
        ];

        let mut expected = Vec::with_capacity(requests.len());
        for req in &requests {
            expected.push(runtime.dispatch(req).await);
        }
        let calls_after_serial = calls.load(Ordering::SeqCst);

        let outcome = runtime
            .execute_batch(
                &requests,
                BatchLimits {
                    concurrency: 2,
                    per_op_timeout_ms: 0,
                    deadline_ms: 0,
                },
            )
            .await;

        assert_eq!(outcome.results.len(), requests.len());
        for (i, (got, want)) in outcome.results.iter().zip(expected.iter()).enumerate() {
            assert_eq!(got, want, "第 {i} 条批量结果必须与串行逐位一致");
        }
        assert_eq!(outcome.succeeded, 2);
        assert_eq!(outcome.failed, 1);
        // 只看批量阶段的增量：串行对照阶段已调用 2 次（两个允许项）
        let batch_calls = calls.load(Ordering::SeqCst) - calls_after_serial;
        assert_eq!(batch_calls, 2, "批量阶段仅允许项触达委托（被拒绝项 0 次）");
        assert!(outcome.is_conserved(), "四段守恒必须成立");
    }
}
