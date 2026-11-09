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

use super::batch::{dispatch_batch_with_sink, BatchAttributionSink, BatchLimits, BatchOutcome};
use super::chain::{DispatcherBuilder, SandboxedDispatcher};
use super::dispatch::{DispatchError, DispatchFuture, DispatchRequest, ToolDispatcher};
use super::inproc::InProcDispatcher;
use super::sandbox_gate::{SandboxGate, SeccoreSandboxGate};
use crate::exec_tool_executor::ExecPolicyToolExecutor;
use crate::toolplan_runner::ToolExecutor;
use nexus_contracts::tool_plan::SideEffectDecl;

/// 准入前置环组 — 与四环链**共用同一 `Arc`**（单一策略/沙箱实例）
///
/// WHY 单独存一份而不是从链里反取：环是私有装饰器，反取需要下行转换；
/// 存同源 `Arc` 只是两次引用计数递增，却让 [`ToolRuntime::admit`] 与链内
/// 前两环共用同一判决体（`chain::evaluate_policy` / `chain::check_sandbox`）。
struct AdmissionRings {
    policy: Arc<ExecPolicy>,
    sandbox: Arc<dyn SandboxGate>,
}

/// 工具运行时 — 强制沙箱四环链的成品装配
pub struct ToolRuntime {
    /// 四环齐备的派发器（唯一构造入口为 [`DispatcherBuilder::build`]）
    dispatcher: SandboxedDispatcher,
    /// 原始 delegate 的能力视图 —— 仅供 `capability_of` / `declared_tools` 查询
    ///
    /// WHY 在构造点就地保留原 `Arc` 而不是让包装件逐层转发:派生链上跑的是
    /// `ExecPolicyToolExecutor` 等包装件，能力自述属于**被包装的原始执行器**。
    /// 转发要跨 4 个实现（Runtime/OpTimeout/Counting/ExecPolicy），漏任一层
    /// 能力表就在现网静默失效，而单测因直接用裸执行器而全绿 ——
    /// 「测试证明不了接线」的典型形态。就地保留只有 1 处可能漏。
    capabilities: Option<Arc<dyn ToolExecutor>>,
    /// 批量归因出口（W3/ADR-248）——`None` = 改造前行为（L1 等价，单点可回退）
    batch_sink: Option<Arc<dyn BatchAttributionSink>>,
    /// 准入前置环组（见 [`ToolRuntime::admit`]）
    admission: AdmissionRings,
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
        // 能力视图:包装前就地保留原 delegate（见结构体字段注释）
        let capabilities: Option<Arc<dyn ToolExecutor>> = Some(Arc::clone(&delegate));
        let guarded: Arc<dyn ToolExecutor> =
            Arc::new(ExecPolicyToolExecutor::new(Arc::clone(&policy), delegate));
        let inproc: Arc<dyn ToolDispatcher> = Arc::new(InProcDispatcher::new(guarded));
        // 路由表由执行器自报的工具名构成;未声明者仍落 fallback ⇒ 归因不变。
        let mut routes: HashMap<String, Arc<dyn ToolDispatcher>> = HashMap::new();
        if let Some(view) = capabilities.as_deref() {
            for name in view.declared_tools() {
                routes.insert(name, Arc::clone(&inproc));
            }
        }
        // 准入环组与链内同源：同一个 policy 与 sandbox Arc 既进链、也存给 admit()。
        let sandbox: Arc<dyn SandboxGate> = Arc::new(SeccoreSandboxGate::default_secure());
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(Arc::clone(&policy)))
            .with_sandbox(Some(Arc::clone(&sandbox)))
            .with_route(Some(routes))
            .with_fallback(Some(Arc::clone(&inproc)))
            .with_transport(Some(inproc))
            .build()?;
        Ok(Self {
            dispatcher,
            capabilities,
            batch_sink: None,
            admission: AdmissionRings { policy, sandbox },
        })
    }

    /// 派发一次请求（四环链入口）
    pub fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        self.dispatcher.dispatch(req)
    }

    /// 借用式派发一次请求 — 调用方无需构造 `DispatchRequest`
    ///
    /// WHY 提供这个入口：热路径上的批量编排往往已持有 `tool_name` 与
    /// `args_json` 的**借用**（例如从同一次响应解析出的多个工具调用）。
    /// 走 `dispatch` 需先构造拥有型 `DispatchRequest`（2 次堆分配），
    /// 而这份拥有型数据在派发结束后即被丢弃 —— 纯浪费。
    ///
    /// 契约：与 [`ToolRuntime::dispatch`] 的结果**完全一致**（含错误变体与
    /// 文案）—— 两者走同一套四环，只是入参形态不同。
    pub fn dispatch_borrowed<'a>(
        &'a self,
        req: &'a nexus_contracts::tool_dispatch::DispatchRequestRef<'a>,
    ) -> super::dispatch::BorrowedDispatchFuture<'a> {
        use super::dispatch::BorrowedToolDispatcher;
        self.dispatcher.dispatch_borrowed(req)
    }

    /// 准入前置：只穿越 policy → sandbox 两环，**不触碰 route/transport**（不执行工具）
    ///
    /// WHY 需要独立入口（ADR-273）：命令面（`chimera mcp call`）要在自己执行工具之前
    /// 复用同一个策略/沙箱实例做安全前置。走 [`Self::dispatch`] 会落到传输环**真的执行**
    /// 工具 —— 那会造成二次执行，且执行者是组合根装配的 MCP 执行器，而不是命令面自己的
    /// ephemeral mesh（两套生命周期，ADR-256 已登记）。
    ///
    /// 契约：对同一请求，判决与链内前两环**逐字一致**（同源 `Arc` + 同一判决体）。
    pub fn admit(&self, req: &DispatchRequest) -> Result<(), DispatchError> {
        super::chain::evaluate_admission(
            &self.admission.policy,
            self.admission.sandbox.as_ref(),
            req,
        )
    }

    /// 生产接线（组合根用）：策略可配置 + 委托执行器可缺省
    ///
    /// 与 [`ToolRuntime::new`] 的唯一差异：
    /// 1. 策略的「未命中默认动作」可指定（[`ToolRuntime::new`] 固定用
    ///    `ExecPolicy::new()` 的零信任默认 `Deny`）；
    /// 2. `delegate` 可为 `None` —— 此时传输环装一个**恒拒绝**执行器，
    ///    而非虚构一个能"成功"的执行器。
    ///
    /// # WHY 需要可缺省委托
    ///
    /// 本仓当前的工具执行面**尚无生产委托执行器**（`ToolExecutor` 的生产实现
    /// 只有 `ExecPolicyToolExecutor`，它自身包裹的 delegate 由上层注入）。
    /// 组合根若凭空造一个"总是成功"的 delegate，等于**虚构功能**——任何工具
    /// 都会返回成功，掩盖真实的未接线状态。
    ///
    /// 正确姿态是：四环链的**安全骨架**（policy → sandbox → route → transport）
    /// 先真实装配起来并可被检验，而传输环对「没有真实执行器」这一事实**如实
    /// fail-closed**（返回 `DispatchError::Transport`）。上层一旦注入真实
    /// delegate（`ToolRuntime::new`），行为立即切换为可执行。
    ///
    /// # 参数
    /// - `delegate`: 真实工具执行者；`None` 表示「本进程尚未接入真实执行器」
    /// - `rules`: execpolicy 规则（先命中先裁决；调用方按具体→通用排序）
    /// - `default_action`: 策略未命中规则时的默认动作
    ///
    /// # 安全不变量
    /// 可缺省委托**只影响传输环**。policy 与 sandbox 两环的行为与
    /// [`ToolRuntime::new`] **完全一致** —— 拒绝仍发生在任何副作用之前。
    pub fn with_optional_delegate(
        delegate: Option<Arc<dyn ToolExecutor>>,
        rules: Vec<seccore::execpolicy::ExecPolicyRule>,
        default_action: seccore::execpolicy::PolicyAction,
    ) -> Result<Self, DispatchError> {
        let mut policy = ExecPolicy::with_default_action(default_action);
        for rule in rules {
            policy = policy.add_rule(rule);
        }
        let policy = Arc::new(policy);
        // 能力视图:先取出原 delegate 的自述，再把 delegate 交给传输环。
        // `None` 分支（尚未接入真实执行器）⇒ 视图指向 `UnwiredToolExecutor` 本身,
        // 其 `declared_tools()` 为空 ⇒ 路由表空、能力未知 ⇒ 全量保守（与今日一致）。
        let capabilities: Option<Arc<dyn ToolExecutor>> = delegate.clone();
        let transport_executor: Arc<dyn ToolExecutor> = match delegate {
            Some(exec) => Arc::new(ExecPolicyToolExecutor::new(Arc::clone(&policy), exec)),
            None => Arc::new(UnwiredToolExecutor),
        };
        let inproc: Arc<dyn ToolDispatcher> = Arc::new(InProcDispatcher::new(transport_executor));
        // 路由表来源:执行器自报的工具名全集（ADR-255）。空集时与今日逐字一致
        // —— 全部流量走 fallback，`unknown builtin tool` 归因保持不变。
        let mut routes: HashMap<String, Arc<dyn ToolDispatcher>> = HashMap::new();
        if let Some(view) = capabilities.as_deref() {
            for name in view.declared_tools() {
                routes.insert(name, Arc::clone(&inproc));
            }
        }
        // 准入环组与链内同源（见 `new` 同注释）。
        let sandbox: Arc<dyn SandboxGate> = Arc::new(SeccoreSandboxGate::default_secure());
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(Arc::clone(&policy)))
            .with_sandbox(Some(Arc::clone(&sandbox)))
            .with_route(Some(routes))
            .with_fallback(Some(Arc::clone(&inproc)))
            .with_transport(Some(inproc))
            .build()?;
        Ok(Self {
            dispatcher,
            capabilities,
            batch_sink: None,
            admission: AdmissionRings { policy, sandbox },
        })
    }

    /// 注入批量归因出口（W3 / ADR-248；组合根唯一装配点调用）
    ///
    /// 消费 `self` 的 builder 形态，链在 `with_optional_delegate` 之后；
    /// `None`（默认构造）= 改造前行为，L1 等价。
    #[must_use]
    pub fn with_batch_sink(mut self, sink: Arc<dyn BatchAttributionSink>) -> Self {
        self.batch_sink = Some(sink);
        self
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
        dispatch_batch_with_sink(
            &self.dispatcher,
            requests,
            limits,
            self.batch_sink.as_deref(),
        )
        .await
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

    /// 能力自述直接回落到原 delegate —— 四环链本身不改变工具的副作用属性
    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        self.capabilities
            .as_deref()
            .and_then(|view| view.capability_of(tool_name))
    }

    /// 可服务工具名全集（供上层构造 `PlanRunner` 时同源取值）
    fn declared_tools(&self) -> Vec<String> {
        self.capabilities
            .as_deref()
            .map(|view| view.declared_tools())
            .unwrap_or_default()
    }
}

/// 未接线执行器 — 本进程尚未接入真实工具执行器时的**恒拒绝**传输环
///
/// # WHY 恒拒绝而非「总是成功」
///
/// 组合根在真实 delegate 缺席时若装一个"总是成功"的执行器，等于**虚构功能**：
/// 任何工具调用都会返回成功，把「这条路径没有真实执行器」这一事实掩盖成
/// 「工具执行成功」。恒拒绝让未接线状态**如实可见**：
/// 派发返回 `DispatchError::Transport("tool executor not wired: ...")`，
/// 上层可据此判定「四环链已装配但传输环未接真实执行器」。
///
/// # 安全姿态
///
/// 恒拒绝是 fail-closed 方向（拒绝 > 误放行）。策略层与沙箱层的行为与
/// 装了真实 delegate 时**完全一致** —— 它们仍先于传输环短路。
#[derive(Debug)]
struct UnwiredToolExecutor;

#[async_trait]
impl ToolExecutor for UnwiredToolExecutor {
    async fn execute(&self, tool_name: &str, _args_json: &str) -> Result<String, String> {
        Err(format!(
            "tool executor not wired: no production ToolExecutor is registered for tool '{tool_name}'"
        ))
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
