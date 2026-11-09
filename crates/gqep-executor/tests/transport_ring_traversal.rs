//! W1 / ADR-267 — 第四环（TransportLayer）真实进入热路径的回归锁。
//!
//! # 为什么需要「结构锁」而不是调用计数锁
//!
//! `TransportLayer::dispatch` 是纯直转发（`self.inner.dispatch(req)`），
//! 因此「第四环是否被穿越」在**行为上不可观测**：修与不修两种形态下，
//! 任何 spy 的调用计数都完全相同。本仓既有测试
//! （`chain.rs` 的 5 处与 `tool_runtime_chain_proptest.rs`）把**同一个**
//! `CountingTransport` 同时喂给 route 与 transport，正是这个盲区的体现。
//!
//! 所以判别力由两部分提供：
//! 1. **结构锁 + 负对照**（`fourth_ring_wrapping_is_structural`）：断言 `build()`
//!    确实把路由命中臂与 fallback 各自包进 `TransportLayer`，且**旧形态必须消失**。
//!    修复前该测试必红 —— 这才叫有判别力。
//! 2. **行为锁**：短路不变量（policy/sandbox 拒绝时传输零触达）与
//!    命中/fallback 两臂各自可达。这些是可观测的，且修复不得破坏它们。
//!
//! 运行：`cargo test -p gqep-executor --test transport_ring_traversal`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use gqep_executor::tool_runtime::{
    DispatchError, DispatchFuture, DispatchOutcome, DispatchRequest, DispatcherBuilder,
    SandboxGate, SandboxedDispatcher, ToolDispatcher,
};
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 回显型 transport + 调用计数
struct CountingTransport {
    calls: Arc<AtomicUsize>,
}

impl ToolDispatcher for CountingTransport {
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(DispatchOutcome {
                content_json: req.args_json.clone(),
            })
        })
    }
}

/// 计数型沙箱门（可配置拒绝）
struct CountingGate {
    calls: Arc<AtomicUsize>,
    deny: bool,
}

impl SandboxGate for CountingGate {
    fn check(&self, req: &DispatchRequest) -> Result<(), DispatchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.deny {
            Err(DispatchError::Sandbox(format!("denied: {}", req.tool_name)))
        } else {
            Ok(())
        }
    }
}

fn counting(calls: &Arc<AtomicUsize>) -> Arc<dyn ToolDispatcher> {
    Arc::new(CountingTransport {
        calls: Arc::clone(calls),
    })
}

fn allow_policy(tool: &str) -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new(tool, "*"),
        action: PolicyAction::Allow,
    }))
}

fn deny_policy(tool: &str) -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new(tool, "*"),
        action: PolicyAction::Deny,
    }))
}

fn request(tool: &str) -> DispatchRequest {
    DispatchRequest {
        tool_name: tool.to_string(),
        args_json: r#"{"k":"v"}"#.to_string(),
    }
}

fn routes_hit(
    tool: &str,
    target: &Arc<dyn ToolDispatcher>,
) -> HashMap<String, Arc<dyn ToolDispatcher>> {
    let mut routes = HashMap::new();
    routes.insert(tool.to_string(), Arc::clone(target));
    routes
}

/// 用**两个不同** spy 组装：route 目标 = A，builder transport = B。
/// 生产不变量是 A 与 B 同源，但测试刻意分开，以便断言「哪一条臂被走到」。
fn build_two_spy(
    policy: Arc<ExecPolicy>,
    deny_sandbox: bool,
    route_calls: &Arc<AtomicUsize>,
    transport_calls: &Arc<AtomicUsize>,
    fallback: Option<Arc<dyn ToolDispatcher>>,
) -> SandboxedDispatcher {
    let gate = Arc::new(CountingGate {
        calls: Arc::new(AtomicUsize::new(0)),
        deny: deny_sandbox,
    });
    DispatcherBuilder::new()
        .with_policy(Some(policy))
        .with_sandbox(Some(gate))
        .with_route(Some(routes_hit("echo", &counting(route_calls))))
        .with_fallback(fallback)
        .with_transport(Some(counting(transport_calls)))
        .build()
        .expect("四环齐备应构建成功")
}

/// 行为锁 1：allow + 路由命中 ⇒ 命中臂的目标被触达一次
#[tokio::test]
async fn allow_path_reaches_route_target() {
    let route_calls = Arc::new(AtomicUsize::new(0));
    let transport_calls = Arc::new(AtomicUsize::new(0));
    let d = build_two_spy(
        allow_policy("echo"),
        false,
        &route_calls,
        &transport_calls,
        None,
    );
    let out = d
        .dispatch(&request("echo"))
        .await
        .expect("allow 路径应成功");
    assert_eq!(out.content_json, r#"{"k":"v"}"#);
    assert_eq!(
        route_calls.load(Ordering::SeqCst),
        1,
        "命中臂目标应被触达一次"
    );
}

/// 行为锁 2：路由未命中 + 有 fallback ⇒ fallback 臂被触达
#[tokio::test]
async fn fallback_path_reaches_fallback_target() {
    let route_calls = Arc::new(AtomicUsize::new(0));
    let transport_calls = Arc::new(AtomicUsize::new(0));
    let fb_calls = Arc::new(AtomicUsize::new(0));
    let d = build_two_spy(
        // 策略必须放行**被请求的那个名字**，否则会在 policy 环就被默认拒绝，
        // 根本走不到路由/fallback（本测试首跑即栽在此）。
        allow_policy("not_registered"),
        false,
        &route_calls,
        &transport_calls,
        Some(counting(&fb_calls)),
    );
    // 请求一个不在路由表里的工具名 ⇒ 走 fallback
    d.dispatch(&request("not_registered"))
        .await
        .expect("fallback 应承接未命中");
    assert_eq!(
        route_calls.load(Ordering::SeqCst),
        0,
        "未命中不得触达命中臂"
    );
    assert_eq!(
        fb_calls.load(Ordering::SeqCst),
        1,
        "fallback 臂应被触达一次"
    );
}

/// 行为锁 3：policy 拒绝 ⇒ 沙箱与传输均零触达（短路不变量，修复不得破坏）
#[tokio::test]
async fn policy_deny_short_circuits_before_transport() {
    let route_calls = Arc::new(AtomicUsize::new(0));
    let transport_calls = Arc::new(AtomicUsize::new(0));
    let d = build_two_spy(
        deny_policy("echo"),
        false,
        &route_calls,
        &transport_calls,
        None,
    );
    let err = d
        .dispatch(&request("echo"))
        .await
        .expect_err("应被策略拒绝");
    assert!(matches!(err, DispatchError::Policy(_)), "实得 {err:?}");
    assert_eq!(route_calls.load(Ordering::SeqCst), 0);
    assert_eq!(transport_calls.load(Ordering::SeqCst), 0);
}

/// 行为锁 4：sandbox 拒绝 ⇒ 传输零触达
#[tokio::test]
async fn sandbox_deny_short_circuits_before_transport() {
    let route_calls = Arc::new(AtomicUsize::new(0));
    let transport_calls = Arc::new(AtomicUsize::new(0));
    let d = build_two_spy(
        allow_policy("echo"),
        true,
        &route_calls,
        &transport_calls,
        None,
    );
    let err = d
        .dispatch(&request("echo"))
        .await
        .expect_err("应被沙箱拒绝");
    assert!(matches!(err, DispatchError::Sandbox(_)), "实得 {err:?}");
    assert_eq!(route_calls.load(Ordering::SeqCst), 0);
    assert_eq!(transport_calls.load(Ordering::SeqCst), 0);
}

/// 行为锁 5：`transport()` 句柄的契约 —— 它确实是第四环的可用实例，
/// 经它派发能到达 builder 传入的 transport 内核。
///
/// 这同时给 `SandboxedDispatcher::transport()` 一个真实消费者：
/// 该访问器此前全库零调用（死 API 面）。其文档目的即「供诊断与上层复用
/// 同一传输实例」，本测试正是对该目的的契约化。
#[tokio::test]
async fn transport_handle_reaches_builder_transport_inner() {
    let route_calls = Arc::new(AtomicUsize::new(0));
    let transport_calls = Arc::new(AtomicUsize::new(0));
    let d = build_two_spy(
        allow_policy("echo"),
        false,
        &route_calls,
        &transport_calls,
        None,
    );
    let handle = d.transport();
    handle
        .dispatch(&request("echo"))
        .await
        .expect("经第四环句柄派发应成功");
    assert_eq!(
        transport_calls.load(Ordering::SeqCst),
        1,
        "transport() 句柄应到达 builder 传入的传输内核"
    );
    assert_eq!(route_calls.load(Ordering::SeqCst), 0, "句柄派发不经路由臂");
}

/// 结构锁（含负对照）：第四环的包装必须发生在 `build()` 里。
///
/// WHY 结构锁：见模块头 —— `TransportLayer` 行为透明，计数无法区分修与不修。
/// 负对照断言旧形态 `RouteLayer::new(routes, self.fallback)` 必须消失，
/// 因此本测试在修复前**必红**，具备判别力。
#[test]
fn fourth_ring_wrapping_is_structural() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tool_runtime/chain.rs");
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 chain.rs 应成功: {e}"));

    // 正断言：命中臂与 fallback 各自经 TransportLayer 包装
    assert!(
        text.contains("(name, wrap_transport(target))"),
        "build() 必须把路由命中臂目标包进 TransportLayer"
    );
    assert!(
        text.contains("self.fallback.map(wrap_transport)"),
        "build() 必须把 fallback 包进 TransportLayer"
    );

    // 负对照：旧形态（route 两臂直挂裸 dispatcher）必须已消失
    assert!(
        !text.contains("RouteLayer::new(routes, self.fallback)"),
        "旧形态仍在 ⇒ 第四环依旧不在热路径上（本锁在修复前应红）"
    );

    // 环序不得被改动：policy → sandbox → route，单头派发
    assert!(
        text.contains("SandboxLayer::new(sandbox, route)"),
        "沙箱环必须仍指向路由环（环序不变）"
    );
    assert!(
        text.contains("PolicyLayer::new(policy, sandbox)"),
        "策略环必须仍是链头（环序不变）"
    );
}
