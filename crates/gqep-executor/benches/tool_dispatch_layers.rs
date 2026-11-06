//! 工具派发链**分层**基准 — 定位 Policy / Sandbox / Route / Transport 各环占比
//!
//! 既有 `tool_runtime_chain` 只测端到端（单点数字无法归因），本基准把链拆成
//! 可独立测量的层：
//! - `floor_echo_transport`：直接调用传输层（链的地板，含一次 boxed future 分配）
//! - `policy_evaluate_allow`：`ExecPolicy::evaluate` 纯同步规则匹配
//! - `sandbox_check_allowall` / `sandbox_check_seccore`：沙箱门两种实现
//! - `chain_*`：经 `DispatcherBuilder` 组装后的端到端路径（allow / deny / route miss）
//!
//! 归因口径（报告里按此公式写，不做拍脑袋拆分）：
//! - 沙箱层成本 ≈ `chain_seccore_route_hit` − `chain_allowall_route_hit`
//! - 策略层成本 ≈ `chain_allowall_route_hit` − `floor_echo_transport` − 沙箱−allowall−路由残差
//!   （残差由 `chain_allowall_route_hit` 与 `floor_echo_transport` 同轮对照给出）
//!
//! 运行：
//! ```powershell
//! cargo bench -p gqep-executor --bench tool_dispatch_layers
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use gqep_executor::tool_runtime::{
    AllowAllSandboxGate, DispatchError, DispatchFuture, DispatchOutcome, DispatchRequest,
    DispatcherBuilder, SandboxGate, SeccoreSandboxGate, ToolDispatcher,
};
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 回声 transport — args_json 原样回传（零 I/O，隔离链本身开销）
struct EchoTransport;

impl ToolDispatcher for EchoTransport {
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        Box::pin(async move {
            Ok(DispatchOutcome {
                content_json: req.args_json.clone(),
            })
        })
    }
}

/// 恒拒绝沙箱门 — 用于测量「沙箱拒绝即短路」路径
struct DenyGate;

/// 优化前的沙箱门实现（逐次 clone 策略 + 插入工具名）—— **同轮对照基准**
///
/// WHY 在这里内联复刻而不是引用 git 历史：criterion 的跨轮比较会被机器状态
/// （CPU 频率 / 后台负载）污染；只有同一进程同一轮内测得的对照差值才能作为
/// 因果证据（与 `panels_scale_bench` 的 `*_legacy_*` 控制组同法）。
struct LegacySeccoreGate {
    command_policy: seccore::CommandPolicy,
    env_policy: seccore::EnvPolicy,
}

impl SandboxGate for LegacySeccoreGate {
    fn check(&self, req: &DispatchRequest) -> Result<(), DispatchError> {
        let mut policy = self.command_policy.clone();
        policy.allowed_commands.insert(req.tool_name.to_lowercase());
        let cmd = seccore::Command {
            program: req.tool_name.clone(),
            args: vec![req.args_json.clone()],
            env: HashMap::new(),
        };
        seccore::validate_command(&cmd, &policy)
            .map_err(|e| DispatchError::Sandbox(format!("{e}")))?;
        let env: HashMap<String, String> = HashMap::new();
        seccore::validate_env(&env, &self.env_policy)
            .map_err(|e| DispatchError::Sandbox(format!("{e}")))?;
        Ok(())
    }
}

impl SandboxGate for DenyGate {
    fn check(&self, _req: &DispatchRequest) -> Result<(), DispatchError> {
        Err(DispatchError::Sandbox("denied: bench gate".to_string()))
    }
}

fn request() -> DispatchRequest {
    DispatchRequest {
        tool_name: "bench.echo".to_string(),
        args_json: "{\"q\":\"bench\"}".to_string(),
    }
}

fn allow_policy(tool: &str) -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new(tool, "*"),
        action: PolicyAction::Allow,
    }))
}

fn route_hit(transport: &Arc<dyn ToolDispatcher>) -> HashMap<String, Arc<dyn ToolDispatcher>> {
    let mut routes = HashMap::new();
    routes.insert("bench.echo".to_string(), Arc::clone(transport));
    routes
}

/// 单层/单点微基准 — 链的构成件各自成本
fn bench_layer_components(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    let transport: Arc<dyn ToolDispatcher> = Arc::new(EchoTransport);
    let gate = SeccoreSandboxGate::default_secure();
    let policy = allow_policy("bench.echo");
    let req = request();

    let mut group = c.benchmark_group("dispatch_layers");
    group.sample_size(50);

    group.bench_function("floor_echo_transport", |b| {
        b.to_async(&rt)
            .iter(|| async { transport.dispatch(&req).await.unwrap() });
    });

    group.bench_function("policy_evaluate_allow", |b| {
        b.iter(|| {
            let action = policy.evaluate(&req.tool_name, std::slice::from_ref(&req.args_json));
            criterion::black_box(action);
        });
    });

    group.bench_function("sandbox_check_allowall", |b| {
        let gate = AllowAllSandboxGate::new();
        b.iter(|| {
            gate.check(&req).unwrap();
        });
    });

    group.bench_function("sandbox_check_seccore", |b| {
        b.iter(|| {
            gate.check(&req).unwrap();
        });
    });

    // 同轮对照：优化前实现（逐次 clone 策略）
    let legacy_gate = LegacySeccoreGate {
        command_policy: seccore::CommandPolicy::default_secure(),
        env_policy: seccore::EnvPolicy::default_secure(),
    };
    group.bench_function("sandbox_check_seccore_legacy", |b| {
        b.iter(|| {
            legacy_gate.check(&req).unwrap();
        });
    });

    group.finish();
}

/// 链级端到端 — 四种路径（allow 双沙箱实现 / policy deny / sandbox deny / route miss）
fn bench_chains(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    let transport: Arc<dyn ToolDispatcher> = Arc::new(EchoTransport);
    let req = request();

    // allow 路径：AllowAll 沙箱（隔离策略+路由+传输成本）
    let chain_allowall = DispatcherBuilder::new()
        .with_policy(Some(allow_policy("bench.echo")))
        .with_sandbox(Some(Arc::new(AllowAllSandboxGate::new())))
        .with_route(Some(route_hit(&transport)))
        .with_transport(Some(Arc::clone(&transport)))
        .build()
        .expect("allowall 链应构建成功");

    // allow 路径：真实安全沙箱（含 policy 克隆 + seccore 校验）
    let chain_seccore = DispatcherBuilder::new()
        .with_policy(Some(allow_policy("bench.echo")))
        .with_sandbox(Some(Arc::new(SeccoreSandboxGate::default_secure())))
        .with_route(Some(route_hit(&transport)))
        .with_transport(Some(Arc::clone(&transport)))
        .build()
        .expect("seccore 链应构建成功");

    // 同轮对照：链级使用优化前沙箱门（逐次 clone 策略）
    let chain_seccore_legacy = DispatcherBuilder::new()
        .with_policy(Some(allow_policy("bench.echo")))
        .with_sandbox(Some(Arc::new(LegacySeccoreGate {
            command_policy: seccore::CommandPolicy::default_secure(),
            env_policy: seccore::EnvPolicy::default_secure(),
        })))
        .with_route(Some(route_hit(&transport)))
        .with_transport(Some(Arc::clone(&transport)))
        .build()
        .expect("legacy 链应构建成功");

    // policy deny（零规则 → fail-closed 短路）
    let chain_policy_deny = DispatcherBuilder::new()
        .with_policy(Some(Arc::new(ExecPolicy::new())))
        .with_sandbox(Some(Arc::new(AllowAllSandboxGate::new())))
        .with_route(Some(route_hit(&transport)))
        .with_transport(Some(Arc::clone(&transport)))
        .build()
        .expect("deny 链应构建成功");

    // sandbox deny（策略放行，沙箱拒绝）
    let chain_sandbox_deny = DispatcherBuilder::new()
        .with_policy(Some(allow_policy("bench.echo")))
        .with_sandbox(Some(Arc::new(DenyGate)))
        .with_route(Some(route_hit(&transport)))
        .with_transport(Some(Arc::clone(&transport)))
        .build()
        .expect("deny gate 链应构建成功");

    // route miss（空路由表 + 无 fallback）
    let chain_route_miss = DispatcherBuilder::new()
        .with_policy(Some(allow_policy("bench.echo")))
        .with_sandbox(Some(Arc::new(AllowAllSandboxGate::new())))
        .with_route(Some(HashMap::new()))
        .with_transport(Some(Arc::clone(&transport)))
        .build()
        .expect("route miss 链应构建成功");

    let mut group = c.benchmark_group("dispatch_chain");
    group.sample_size(50);

    group.bench_function("chain_allowall_route_hit", |b| {
        b.to_async(&rt)
            .iter(|| async { chain_allowall.dispatch(&req).await.unwrap() });
    });
    group.bench_function("chain_seccore_route_hit", |b| {
        b.to_async(&rt)
            .iter(|| async { chain_seccore.dispatch(&req).await.unwrap() });
    });
    group.bench_function("chain_seccore_route_hit_legacy", |b| {
        b.to_async(&rt)
            .iter(|| async { chain_seccore_legacy.dispatch(&req).await.unwrap() });
    });
    group.bench_function("chain_policy_deny", |b| {
        b.to_async(&rt).iter(|| async {
            let err = chain_policy_deny.dispatch(&req).await.unwrap_err();
            assert!(matches!(err, DispatchError::Policy(_)));
        });
    });
    group.bench_function("chain_sandbox_deny", |b| {
        b.to_async(&rt).iter(|| async {
            let err = chain_sandbox_deny.dispatch(&req).await.unwrap_err();
            assert!(matches!(err, DispatchError::Sandbox(_)));
        });
    });
    group.bench_function("chain_route_miss", |b| {
        b.to_async(&rt).iter(|| async {
            let err = chain_route_miss.dispatch(&req).await.unwrap_err();
            assert!(matches!(err, DispatchError::Route(_)));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_layer_components, bench_chains);
criterion_main!(benches);
