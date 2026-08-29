//! Provider 热更延迟门禁基准（P2-T12，v4.0 WI-06）
//!
//! 门禁口径（T8/T9 单行采样模式）：`hot_reload_ms` < 1000ms（1s 门禁）——
//! ArcSwap RCU 热更（reload_from_specs）端到端延迟。数据诚实：只输出实测。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion};
use model_router::error::RouterError;
use model_router::provider::{
    CompletionReq, CompletionResult, Health, ModelProvider, ProviderRegistry, ProviderSpec,
};

/// 极简 provider（热更延迟测量用——不真正调用 LLM）
struct StubProvider {
    id: String,
}

#[async_trait::async_trait]
impl ModelProvider for StubProvider {
    async fn complete(&self, req: &CompletionReq) -> Result<CompletionResult, RouterError> {
        Ok(CompletionResult {
            provider_id: self.id.clone(),
            model: req.model.clone(),
            text: String::new(),
            done: true,
        })
    }
    async fn health(&self) -> Health {
        Health {
            provider_id: self.id.clone(),
            healthy: true,
            latency_ms: 0,
        }
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> model_router::provider::ProviderCaps {
        model_router::provider::ProviderCaps::default()
    }
}

fn specs(n: usize) -> Vec<ProviderSpec> {
    (0..n)
        .map(|i| ProviderSpec {
            provider_id: format!("provider-{i}"),
            endpoint: "https://stub.local".into(),
            model_map: HashMap::new(),
            caps: Default::default(),
        })
        .collect()
}

fn hot_reload_bench(c: &mut Criterion) {
    let registry = ProviderRegistry::new();
    let mut group = c.benchmark_group("provider_hot_reload");
    group.sample_size(30);
    group.bench_function("reload_100_specs", |b| {
        b.iter(|| {
            criterion::black_box(registry.reload_from_specs(specs(100), |spec| {
                Ok(Arc::new(StubProvider {
                    id: spec.provider_id.clone(),
                }) as Arc<dyn ModelProvider>)
            }))
        });
    });
    group.finish();

    // 门禁单行采样：固定 50 次热更，P50/P99（ms）
    let mut samples = Vec::with_capacity(50);
    for _ in 0..50 {
        let t0 = Instant::now();
        let _ = registry.reload_from_specs(specs(100), |spec| {
            Ok(Arc::new(StubProvider {
                id: spec.provider_id.clone(),
            }) as Arc<dyn ModelProvider>)
        });
        samples.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples[25];
    let p99 = samples[49];
    eprintln!(
        "[hot_reload_gate] n=100_specs p50_ms={p50:.3} p99_ms={p99:.3} gate_ms=1000",
    );
}

criterion_group!(benches, hot_reload_bench);
criterion_main!(benches);
