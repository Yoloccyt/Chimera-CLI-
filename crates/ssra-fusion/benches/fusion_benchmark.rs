//! SSRA 融合引擎性能基准 — 测量 10/50/100 模板融合延迟
//!
//! 对应 SubTask 1.5:性能基准测试
//!
//! # 验收标准
//! - p95 ≤ 20ms(设计目标)
//! - 目标 ≤ 15ms(留 25% 余量)
//!
//! # 运行
//! ```bash
//! cargo bench -p ssra-fusion
//! ```

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use ssra_fusion::{FusionRequest, FusionStrategy, SlimeFusionEngine, SlimeTemplate, SsraConfig};
use tokio::runtime::Runtime;

/// 构建带 N 个模板的引擎(无 EventBus,纯融合测量)
fn make_engine(n: usize) -> SlimeFusionEngine {
    let config = SsraConfig::default();
    let engine = SlimeFusionEngine::new(config);
    for i in 0..n {
        let id = format!("cap-{i}");
        let weight = 0.1 + (i as f32 * 0.01).rem_euclid(0.9);
        let strategy = match i % 3 {
            0 => FusionStrategy::WeightedAverage,
            1 => FusionStrategy::TopK,
            _ => FusionStrategy::MeanField,
        };
        let template = SlimeTemplate::new(id, vec!["x".into()], strategy).with_weight(weight);
        let _ = engine.registry().register(template);
    }
    engine
}

/// 构建融合请求(源适配器为 cap-0..cap-(n-1),top_k=8)
fn make_request(n: usize) -> FusionRequest {
    let sources: Vec<String> = (0..n).map(|i| format!("cap-{i}")).collect();
    FusionRequest::new("q-bench", sources, "target", 20, 8)
}

/// 基准:测量不同模板数量下的融合延迟
fn bench_fusion(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let sizes: &[usize] = &[10, 50, 100];

    let mut group = c.benchmark_group("ssra_fusion");
    group.sample_size(10); // min-of-N:criterion 默认 100,降低到 10 加速
    group.measurement_time(std::time::Duration::from_secs(5));

    for &n in sizes {
        let engine = make_engine(n);
        let request = make_request(n);

        group.bench_with_input(BenchmarkId::new("fuse", n), &n, |b, &_| {
            b.iter(|| {
                rt.block_on(async {
                    engine
                        .fuse(black_box(request.clone()))
                        .await
                        .expect("融合失败")
                })
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_fusion);
criterion_main!(benches);
