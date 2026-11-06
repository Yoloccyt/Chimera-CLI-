//! 工具运行时强制沙箱派发链基准 — criterion
//!
//! 测点：allow 路径端到端 dispatch 延迟（policy 命中 → sandbox 放行 →
//! route 命中 → transport 回声）。含 execpolicy 决策统计锁、Arc 解引用与
//! 四次 boxed future 分配的固定开销。
//!
//! 运行方式:
//! ```powershell
//! cargo bench -p gqep-executor --bench tool_runtime_chain
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
use std::collections::HashMap;
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use gqep_executor::tool_runtime::{
    AllowAllSandboxGate, DispatchFuture, DispatchOutcome, DispatchRequest, DispatcherBuilder,
    ToolDispatcher,
};
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 回声 transport — content_json 原样回传 args_json（零 I/O，隔离链本身开销）
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

/// allow 路径端到端延迟
fn bench_allow_path(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    let transport: Arc<dyn ToolDispatcher> = Arc::new(EchoTransport);
    let mut routes: HashMap<String, Arc<dyn ToolDispatcher>> = HashMap::new();
    routes.insert("bench.echo".to_string(), Arc::clone(&transport));

    let dispatcher = DispatcherBuilder::new()
        .with_policy(Some(Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new("bench.echo", "*"),
            action: PolicyAction::Allow,
        }))))
        .with_sandbox(Some(Arc::new(AllowAllSandboxGate::new())))
        .with_route(Some(routes))
        .with_transport(Some(transport))
        .build()
        .expect("bench 链应构建成功");

    let request = DispatchRequest {
        tool_name: "bench.echo".to_string(),
        args_json: "{\"q\":\"bench\"}".to_string(),
    };

    let mut group = c.benchmark_group("tool_runtime_chain");
    group.sample_size(30); // 与 gather bench 一致，加速基准

    group.bench_function("allow_route_hit_echo", |b| {
        b.to_async(&rt)
            .iter(|| async { dispatcher.dispatch(&request).await.unwrap() });
    });

    group.finish();
}

criterion_group!(benches, bench_allow_path);
criterion_main!(benches);
