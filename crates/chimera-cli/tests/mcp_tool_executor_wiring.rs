//! P1 / ADR-256 集成验收 — `McpToolExecutor` 四环接入接线矩阵（deepdive §6.4）。
//!
//! 覆盖面：四环前置无旁路（policy/sandbox 对 `mcp.*` 同样生效）、默认档
//! InProcessClient 零网络、ADR-244 D2 回归（unknown builtin 文案不变）、
//! 组合根唯一装配反向断言、ADR-253 调和（零第二 JSON-RPC 栈）。
//!
//! 运行：`cargo test -p chimera-cli --test mcp_tool_executor_wiring -- --test-threads=1`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_cli::composition::build;
use chimera_cli::ChimeraConfig;
use gqep_executor::tool_runtime::{DispatchError, DispatchRequest};
use nexus_core::config::{
    McpConfig, McpServerConfig, ToolPolicyAction, ToolPolicyConfig, ToolPolicyRuleDef,
};

/// 带 mcp server 注册的配置（TEST-NET-1 文档段地址——SSRF 白名单放行，
/// 且 InProcessClient 默认传输下全程零 socket）。
fn config_with_mcp_server(id: &str, url: &str) -> ChimeraConfig {
    ChimeraConfig {
        mcp: McpConfig {
            servers: vec![McpServerConfig {
                id: id.to_string(),
                url: Some(url.to_string()),
            }],
            // R10 WP-T1:声明集缺省空集(fail-closed 语义不变)
            declared_side_effects: Vec::new(),
        },
        ..ChimeraConfig::default()
    }
}

/// 带 tool_policy deny 规则的配置（ADR-246 声明式源；Z-1 口径：policy 定向
/// 须写全名——program 匹配是精确小写，通配 `mcp.*` 不可行）。
fn config_with_deny_rule(pattern: &str) -> ChimeraConfig {
    ChimeraConfig {
        tool_policy: ToolPolicyConfig {
            rules: vec![ToolPolicyRuleDef {
                pattern: pattern.to_string(),
                action: ToolPolicyAction::Deny,
            }],
            ..Default::default()
        },
        ..ChimeraConfig::default()
    }
}

/// 集成 1（deepdive §6.4）：未注册 server → Err 含 `route: mcp server not
/// registered`。空 config ⇒ registry 空 ⇒ 零 mesh 注册也零网络（InProcessClient）。
#[tokio::test]
async fn unregistered_server_fails_with_route_text() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");
    let req = DispatchRequest {
        tool_name: "mcp.ghost.tool".to_string(),
        args_json: "{}".to_string(),
    };
    let result = ctx.tool_runtime.dispatch(&req).await;
    match &result {
        Err(DispatchError::Transport(msg)) => {
            assert!(
                msg.contains("route: mcp server not registered: ghost"),
                "错误文案须携带 route 语义段，实际: {msg}"
            );
        }
        other => panic!("未注册 server 应归因 Transport（内联 route: 文案），实际: {other:?}"),
    }
}

/// 集成 2：默认档（InProcessClient + TEST-NET-1 注册）→ 协议级 ACK 成功回执，
/// 全程零 socket。**ADR-256 D8 诚实性**：该成功回执是协议级 ACK（默认档
/// prepare/commit 恒 ACK），非业务级工具结果——断言回执含 success/transaction_id
/// 即止，不虚构业务语义。
#[tokio::test]
async fn inprocess_default_zero_network() {
    let cfg = config_with_mcp_server("docsrv", "http://192.0.2.55:8080");
    let ctx = build(&cfg).expect("组合根装配应成功");
    let req = DispatchRequest {
        tool_name: "mcp.docsrv.render".to_string(),
        args_json: "{}".to_string(),
    };
    let result = ctx.tool_runtime.dispatch(&req).await;
    match &result {
        Ok(outcome) => {
            let v: serde_json::Value =
                serde_json::from_str(&outcome.content_json).expect("回执应为单层 JSON");
            let payload = outcome.content_json.clone();
            assert!(
                v["success"].as_bool().unwrap_or(false),
                "协议级 ACK: {payload}"
            );
            assert!(
                v["transaction_id"].as_str().is_some(),
                "回执须携带事务 ID: {payload}"
            );
        }
        other => panic!("InProcessClient 默认档应协议级 ACK，实际: {other:?}"),
    }
}

/// 集成 3（回归，ADR-244 D2）：非 mcp 未知工具仍走 builtin 失败——复合分派
/// 不改变既有归因文案。
#[tokio::test]
async fn unknown_builtin_still_fails() {
    let cfg = config_with_mcp_server("docsrv", "http://192.0.2.55:8080");
    let ctx = build(&cfg).expect("组合根装配应成功");
    let req = DispatchRequest {
        tool_name: "text.nonexistent".to_string(),
        args_json: "{}".to_string(),
    };
    let result = ctx.tool_runtime.dispatch(&req).await;
    match &result {
        Err(DispatchError::Transport(msg)) => {
            assert!(
                msg.contains("unknown builtin tool"),
                "ADR-244 D2 文案须不变，实际: {msg}"
            );
        }
        other => panic!("unknown builtin 应归因 Transport，实际: {other:?}"),
    }
}

/// 集成 4（Z-1 口径）：policy 规则可定向 `mcp.<server>.<tool>` 全名并拦截——
/// 四环 policy 前置对 mcp 分支同样生效（复合器不绕过 PolicyLayer）。
#[tokio::test]
async fn policy_rule_blocks_mcp_tool() {
    let cfg = config_with_deny_rule("mcp.gh.deploy");
    let ctx = build(&cfg).expect("组合根装配应成功");
    let req = DispatchRequest {
        tool_name: "mcp.gh.deploy".to_string(),
        args_json: "{}".to_string(),
    };
    let result = ctx.tool_runtime.dispatch(&req).await;
    match &result {
        Err(DispatchError::Policy(msg)) => {
            assert!(
                msg.contains("mcp.gh.deploy"),
                "policy 拒绝文案须携带工具全名，实际: {msg}"
            );
        }
        other => panic!("deny 规则应归因 Policy 环，实际: {other:?}"),
    }
}

/// 集成 5：沙箱层对 `mcp.*` 参数同样拦截（ADR-238 D4 对 mcp 生效）——注入形态
/// 归因 Sandbox，且**先于** mesh 调用（零 2PC 事务创建）。
#[tokio::test]
async fn sandbox_still_guards_mcp_args() {
    let cfg = config_with_mcp_server("docsrv", "http://192.0.2.55:8080");
    let ctx = build(&cfg).expect("组合根装配应成功");
    let req = DispatchRequest {
        tool_name: "mcp.docsrv.render".to_string(),
        args_json: "$(rm -rf /)".to_string(),
    };
    let result = ctx.tool_runtime.dispatch(&req).await;
    assert!(
        matches!(result, Err(DispatchError::Sandbox(_))),
        "注入形态应被沙箱层拦截（归因 Sandbox），实际: {result:?}"
    );
}

/// 集成 6（反向，组合根唯一装配）：`McpToolExecutor::from_config` /
/// `CompositeToolExecutor::new` 全 src 命中仅 composition.rs 与自身模块。
#[test]
fn composition_root_single_assembly() {
    let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits: Vec<(String, usize)> = Vec::new();
    let mut stack = vec![src_root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("读 src 目录应成功") {
            let path = entry.expect("读目录项应成功").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let n = text.matches("McpToolExecutor::from_config").count()
                    + text.matches("CompositeToolExecutor::new").count();
                if n > 0 {
                    hits.push((path.display().to_string(), n));
                }
            }
        }
    }
    hits.sort();
    // 不变量：装配构造只允许出现在组合根（composition.rs，from_config ×1 +
    // Composite::new ×1）与定义模块自身（模块内单测合法使用自己的构造器）；
    // 其余任何 src 文件出现即第二装配点 = 门红。
    let others: Vec<_> = hits
        .iter()
        .filter(|(p, _)| !p.ends_with("composition.rs") && !p.ends_with("mcp_tool_executor.rs"))
        .collect();
    assert!(others.is_empty(), "发现第二装配点: {others:?}");
    let comp = hits.iter().find(|(p, _)| p.ends_with("composition.rs"));
    match comp {
        Some((_, n)) => assert_eq!(*n, 2, "组合根装配形态 = from_config ×1 + Composite::new ×1"),
        None => panic!("组合根未装配 CompositeToolExecutor"),
    }
}

/// 集成 7（ADR-253 调和）：零第二 JSON-RPC 栈——模块源无 wire 级 jsonrpc
/// 字面量，执行路径唯一经 `execute_transaction`。
#[test]
fn no_second_json_rpc_stack() {
    let src = include_str!("../src/mcp_tool_executor.rs");
    assert!(
        !src.to_lowercase().contains("jsonrpc"),
        "模块不得引入 JSON-RPC wire 字面量（ADR-253 边界）"
    );
    let n = src.matches(".execute_transaction(").count();
    assert_eq!(
        n, 1,
        "执行路径唯一经 execute_transaction 调用（实际 {n} 处）"
    );
}

// 合法 mcp 工具名策略：mcp.<server∈白名单>.<tool∈白名单>
// WHY 顶层 fn：proptest! 生成的是普通 fn（非闭包），不能捕获外层局部变量——
// 策略须以调用表达式内联或顶层函数提供。
fn valid_mcp_name_strategy() -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::prelude::*;
    let seg = prop::collection::vec(
        prop_oneof![Just('a'), Just('G'), Just('9'), Just('-'), Just('_')],
        1..12,
    )
    .prop_map(|v| v.into_iter().collect::<String>());
    (Just("mcp.".to_string()), seg.clone(), seg)
        .prop_map(|(prefix, server, tool)| format!("{prefix}{server}.{tool}"))
}

// 集成 proptest（deepdive §6.4）：任意合法工具名 + 空注册表 → 恒 Err（文案
// 携带 `route:`）且零 TCP——InProcessClient 构造 + registry 先于 client 校验。
// WHY proptest! 位于文件顶层：该宏生成自己的 #[test]，不能嵌套在 #[test] 内
// （「cannot test inner items」）。
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn no_network_on_default(name in valid_mcp_name_strategy(), args in ".*") {
        use chimera_cli::mcp_tool_executor::McpToolExecutor;
        use gqep_executor::ToolExecutor as _;
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("current_thread runtime 构造应成功");
        let exec = McpToolExecutor::from_config(&ChimeraConfig::default());
        let result = rt.block_on(exec.execute(&name, &args));
        let err = match result {
            Ok(_) => panic!("空注册表下任意调用不得成功: name={name:?}"),
            Err(e) => e,
        };
        prop_assert!(err.contains("route:"), "name={:?} err={}", name, err);
    }
}

// ===========================================================================
// R11 W1（ADR-295 D1）：声明表穿透 + MCP 并行解封
// ===========================================================================
//
// 修复前的事实链：McpToolExecutor.declared_side_effects() 已返回配置表
// （R10 WP-T1），但 ToolRuntime / exec_plan.rs 三装饰器只转发
// capability_of / declared_tools，批量声明表在装饰器链被静默丢弃 ⇒
// `effective_side_effect` 第三优先级查表恒空 ⇒ MCP 工具恒判「可能写」
// 整层串行（并行封顶 = 0）。以下测试锁住修复后的两条语义：
// ① 声明表穿透全链（build() 组合根 → 装饰器 → ToolRuntime → Composite → Mcp）；
// ② 声明只读的工具在计划层内真并行，未声明仍保守串行（fail-closed 不放宽）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chimera_cli::exec_plan::{OpTimeoutExecutor, RuntimeToolExecutor};
use gqep_executor::tool_runtime::ToolRuntime;
use gqep_executor::toolplan_runner::{PlanGuards, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{SideEffectDecl, ToolNode, ToolOp, ToolPlan};

/// 记录「同时在飞峰值」并按批量声明表自报只读的探针执行器——
/// 判据口径与 `gqep-executor/tests/toolplan_capability_table.rs` 一致
/// （峰值是结构性证据，不用受宿主抖动影响的墙钟比值）。
struct SideEffectProbeExecutor {
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    readonly_names: Vec<String>,
}

impl SideEffectProbeExecutor {
    fn new(readonly_names: Vec<String>) -> Self {
        Self {
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            readonly_names,
        }
    }
}

#[async_trait]
impl ToolExecutor for SideEffectProbeExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(args_json.to_string())
    }

    // 刻意不覆写 capability_of：走查表路径（声明表是本批的唯一供给面）
    fn declared_tools(&self) -> Vec<String> {
        self.readonly_names.clone()
    }

    fn declared_side_effects(&self) -> Vec<(String, SideEffectDecl)> {
        self.readonly_names
            .iter()
            .map(|n| (n.clone(), SideEffectDecl::ReadOnly))
            .collect()
    }
}

/// n 个同层 ToolCall 节点（side_effect 留空 ⇒ 判定全靠声明表）
fn probe_plan(n: usize) -> ToolPlan {
    let nodes = (0..n)
        .map(|i| ToolNode {
            id: format!("t{i}"),
            op: ToolOp::ToolCall,
            tool_name: Some(format!("probe.tool.{i}")),
            args_json: Some(format!(r#"{{"i":{i}}}"#)),
            field: None,
            predicate: None,
            side_effect: None,
        })
        .collect();
    ToolPlan {
        id: format!("side-effect-probe-{n}"),
        nodes,
        edges: Vec::new(),
    }
}

/// 把探针装进生产构造入口（with_optional_delegate）+ 生产装饰器链
/// （RuntimeToolExecutor → OpTimeoutExecutor），返回最外层执行器与峰值句柄。
fn probe_chain(readonly_names: Vec<String>) -> (OpTimeoutExecutor, Arc<AtomicUsize>) {
    let probe = Arc::new(SideEffectProbeExecutor::new(readonly_names));
    let peak = Arc::clone(&probe.peak);
    let runtime = ToolRuntime::with_optional_delegate(
        Some(probe as Arc<dyn ToolExecutor>),
        Vec::new(),
        seccore::execpolicy::PolicyAction::Allow,
    )
    .expect("四环链装配（policy/sandbox/route/transport 恒齐备）");
    let outermost = OpTimeoutExecutor {
        inner: Arc::new(RuntimeToolExecutor {
            inner: Arc::new(runtime),
        }),
        per_op_timeout: Duration::from_secs(5),
    };
    (outermost, peak)
}

/// 穿透契约：配置声明表必须经「build() 组合根 → RuntimeToolExecutor →
/// OpTimeoutExecutor」全链到达最外层；缺省配置恒空集（fail-closed 不放宽）。
#[tokio::test]
async fn declared_side_effects_flow_through_full_decorator_chain() {
    use nexus_core::config::DeclaredSideEffectEntry;

    let mut cfg = config_with_mcp_server("docsrv", "http://192.0.2.55:8080");
    cfg.mcp.declared_side_effects.push(DeclaredSideEffectEntry {
        tool: "mcp.docsrv.search".to_string(),
        effect: SideEffectDecl::ReadOnly,
    });
    let ctx = build(&cfg).expect("组合根装配应成功");
    let outermost = OpTimeoutExecutor {
        inner: Arc::new(RuntimeToolExecutor {
            inner: Arc::clone(&ctx.tool_runtime),
        }),
        per_op_timeout: Duration::from_secs(5),
    };
    let table = outermost.declared_side_effects();
    assert_eq!(
        table.len(),
        1,
        "声明表必须穿透装饰器链+ToolRuntime+CompositeToolExecutor 到达最外层，实得 {table:?}"
    );
    assert_eq!(table[0].0, "mcp.docsrv.search");
    assert_eq!(table[0].1, SideEffectDecl::ReadOnly);

    // 反向对照：缺省（无声明）配置 ⇒ 最外层查表恒空集
    let ctx_default = build(&config_with_mcp_server("docsrv", "http://192.0.2.55:8080"))
        .expect("组合根装配应成功");
    let outermost_default = OpTimeoutExecutor {
        inner: Arc::new(RuntimeToolExecutor {
            inner: Arc::clone(&ctx_default.tool_runtime),
        }),
        per_op_timeout: Duration::from_secs(5),
    };
    assert!(
        outermost_default.declared_side_effects().is_empty(),
        "缺省配置无声明 ⇒ 恒空集（未知 ⇒ 可能写 ⇒ 串行）"
    );
}

/// 行为契约（解封正向）：声明只读的工具经生产构造入口 + 装饰器链后层内真并行。
/// 修复前本测试必红——声明表在 ToolRuntime/装饰器层被丢弃，恒整层串行。
#[tokio::test]
async fn declared_readonly_tools_run_concurrent_through_production_chain() {
    let n = 4;
    let names = (0..n).map(|i| format!("probe.tool.{i}")).collect();
    let (outermost, peak) = probe_chain(names);
    let runner = PlanRunner::new(Box::new(outermost), PlanGuards::default()).with_concurrency(8);
    let summary = runner.run(&probe_plan(n)).await.expect("计划必须执行成功");
    assert_eq!(summary.tool_calls, n);
    assert_eq!(summary.nodes_executed, n);
    assert!(
        peak.load(Ordering::SeqCst) > 1,
        "声明只读 ⇒ 层内必须真并行（峰值 {}，整层串行=1）",
        peak.load(Ordering::SeqCst)
    );
}

/// 行为契约（fail-closed 反向）：无声明 ⇒ 保守串行，与修复前逐字同语义。
#[tokio::test]
async fn undeclared_tools_stay_serial_fail_closed() {
    let n = 4;
    let (outermost, peak) = probe_chain(Vec::new());
    let runner = PlanRunner::new(Box::new(outermost), PlanGuards::default()).with_concurrency(8);
    runner.run(&probe_plan(n)).await.expect("计划必须执行成功");
    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "未声明工具必须按「可能写」保守串行（峰值 1）"
    );
}
