//! FaaE 路由基准测试
//!
//! 对应 SubTask 28.6:criterion 基准测试
//!
//! 基准场景:
//! - 路由延迟:20 候选工具,精筛 Top-8
//! - 熵计算延迟:20 工具的香农熵计算
//!
//! WHY 使用 block_on:route 为 async fn,criterion 默认同步,
//! 通过 `Runtime::new().block_on()` 在同步上下文中调用 async 方法。

use std::collections::HashMap;
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use faae_router::{EdsbBalancer, ExpertProfile, FaaeConfig, FaaeRouter, ToolId};
use tokio::sync::RwLock;

/// 构造 20 个测试专家(每个在不同维度有高值)
fn make_test_profiles() -> Vec<ExpertProfile> {
    (0..20)
        .map(|i| {
            let mut v = vec![0.0; 64];
            v[i] = 1.0;
            ExpertProfile::new(format!("tool-{i}"), v, vec!["bench".into()], 1.0)
        })
        .collect()
}

/// 基准:FaaE 路由(20 候选工具,精筛 Top-8)
fn bench_route(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");

    let bus = EventBus::new();
    let router = FaaeRouter::new(bus);

    // 在基准循环外注册专家
    rt.block_on(async {
        for profile in make_test_profiles() {
            router.register_expert(profile).await;
        }
    });

    // 构造查询 CLV(匹配 tool-0)
    let clv: Vec<f32> = {
        let mut v = vec![0.0; 64];
        v[0] = 1.0;
        v
    };
    let candidates: Vec<ToolId> = (0..20).map(|i| ToolId::new(format!("tool-{i}"))).collect();

    c.bench_function("route_20_candidates", |b| {
        b.iter(|| {
            rt.block_on(router.route(&clv, &candidates))
                .expect("路由应成功");
        });
    });
}

/// 基准:EDSB 熵计算(20 工具)
fn bench_compute_entropy(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");

    let bus = EventBus::new();
    let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);

    // 构造 20 个带使用计数的 profiles
    let profiles: HashMap<ToolId, Arc<RwLock<ExpertProfile>>> = rt.block_on(async {
        let mut map = HashMap::new();
        for i in 0..20 {
            let mut v = vec![0.0; 64];
            v[i] = 1.0;
            let profile = ExpertProfile::with_usage_count(
                format!("tool-{i}"),
                v,
                vec!["bench".into()],
                1.0,
                (i + 1) as u64 * 10, // 不同使用次数
            );
            map.insert(
                ToolId::new(format!("tool-{i}")),
                Arc::new(RwLock::new(profile)),
            );
        }
        map
    });

    c.bench_function("compute_entropy_20_tools", |b| {
        b.iter(|| {
            rt.block_on(balancer.compute_entropy(&profiles))
                .expect("熵计算应成功");
        });
    });
}

/// 基准:FaaE 路由(100 候选工具,精筛 Top-8)— 规模基准
fn bench_route_100_candidates(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建应成功");

    let bus = EventBus::new();
    let router = FaaeRouter::new(bus);

    // 注册 100 个专家
    rt.block_on(async {
        for i in 0..100 {
            let mut v = vec![0.0; 64];
            v[i % 64] = 1.0;
            let profile = ExpertProfile::new(format!("tool-{i}"), v, vec!["bench".into()], 1.0);
            router.register_expert(profile).await;
        }
    });

    let clv: Vec<f32> = {
        let mut v = vec![0.0; 64];
        v[0] = 1.0;
        v
    };
    let candidates: Vec<ToolId> = (0..100).map(|i| ToolId::new(format!("tool-{i}"))).collect();

    c.bench_function("route_100_candidates", |b| {
        b.iter(|| {
            rt.block_on(router.route(&clv, &candidates))
                .expect("路由应成功");
        });
    });
}

criterion_group!(
    benches,
    bench_route,
    bench_compute_entropy,
    bench_route_100_candidates
);
criterion_main!(benches);
