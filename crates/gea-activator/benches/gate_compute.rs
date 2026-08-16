//! GEA 门控计算性能基准 — criterion 基准测试
//!
//! 对应 SubTask 23.6
//!
//! # 基准配置
//! - warmup: 10 次迭代
//! - measurement: 100 次采样
//! - 测量 P50/P99 延迟

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::EventBus;
use gea_activator::{
    compute_gate_value, compute_gate_value_with_norms, resolve_conflicts, Candidate, ExpertId,
    ExpertProfile, GeaActivator, GeaConfig, TaskProfile,
};
use std::collections::HashMap;

fn bench_gate_compute(c: &mut Criterion) {
    let config = GeaConfig::default();
    let expert = ExpertProfile::new("e-1", vec![0.5; 64], 0.8, vec!["code-gen".into()]);
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    c.bench_with_input(
        BenchmarkId::new("gate_compute", "64dim"),
        &(&task, &expert, &config),
        |b, &(task, expert, config)| {
            b.iter(|| compute_gate_value(task, expert, config));
        },
    );
}

fn bench_gate_compute_512dim(c: &mut Criterion) {
    let config = GeaConfig::default();
    let expert = ExpertProfile::new("e-1", vec![0.5; 64], 0.8, vec!["code-gen".into()]);
    // 512 维 CLV(与 64 维专家向量不等长,测试维度差异下的性能)
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 512]);

    c.bench_with_input(
        BenchmarkId::new("gate_compute", "512dim_clv"),
        &(&task, &expert, &config),
        |b, &(task, expert, config)| {
            b.iter(|| compute_gate_value(task, expert, config));
        },
    );
}

fn bench_activate_with_cache(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let activator = GeaActivator::new(GeaConfig::default(), bus).unwrap();

    // 注册 5 个专家
    for i in 0..5 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        activator.register_expert(ExpertProfile::new(
            format!("e-{i}"),
            v,
            0.8,
            vec!["code-gen".into()],
        ));
    }

    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    // 预热缓存
    rt.block_on(activator.activate(&task)).unwrap();

    c.bench_function("activate_cached", |b| {
        b.iter(|| {
            rt.block_on(activator.activate(&task)).unwrap();
        });
    });
}

fn bench_activate_no_cache(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let activator = GeaActivator::new(GeaConfig::default(), bus).unwrap();

    // 注册 5 个专家
    for i in 0..5 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        activator.register_expert(ExpertProfile::new(
            format!("e-{i}"),
            v,
            0.8,
            vec!["code-gen".into()],
        ));
    }

    c.bench_function("activate_no_cache", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            // 每次用不同任务,避免缓存命中
            let task = TaskProfile::new(
                0.5 + (idx % 100) as f32 * 0.005,
                "code-gen",
                30,
                vec![0.5; 64],
            );
            idx += 1;
            rt.block_on(activator.activate(&task)).unwrap();
        });
    });
}

/// 满载驱逐热路径基准(L9 优化 2.2:evict_oldest 采样近似 LRU 证伪)
///
/// 缓存预填至容量上限(128),之后每次 activate 均用全新任务 →
/// 每次都未命中且触发 evict_oldest。旧版 O(n) 全遍历 vs 新版 O(sample):
/// 本组直接度量驱逐路径开销,接入 bench_check.yml 阀值守护。
fn bench_activate_eviction_saturated(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let activator = GeaActivator::new(GeaConfig::default(), bus).unwrap();

    for i in 0..5 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        activator.register_expert(ExpertProfile::new(
            format!("e-{i}"),
            v,
            0.8,
            vec!["code-gen".into()],
        ));
    }

    // 预填至容量上限(cache_capacity 默认 128),使后续每次 activate 必触发 evict
    let capacity = GeaConfig::default().cache_capacity;
    for i in 0..capacity {
        let task = TaskProfile::new(i as f32 * 0.001, "code-gen", 30, vec![0.5; 64]);
        rt.block_on(activator.activate(&task)).unwrap();
    }

    c.bench_function("activate_eviction_saturated", |b| {
        // 从 capacity 起递增 task,保证每次都是新 key + 容量满 → 触发 evict
        let mut idx = capacity as u64;
        b.iter(|| {
            let task = TaskProfile::new(idx as f32 * 0.001, "code-gen", 30, vec![0.5; 64]);
            idx += 1;
            rt.block_on(activator.activate(&task)).unwrap();
        });
    });
}

/// 冲突消解基准(L9 优化第二轮:早停 O(n²·d)→O(n·k·d) 证伪)
///
/// n=128 高重叠候选(向量相近 → 大量冲突,触发内层重叠检测热路径),
/// top_k 默认 3。早停后一旦 activated 集满 3 即停,剩余全部抑制。
fn bench_resolve_conflicts(c: &mut Criterion) {
    let config = GeaConfig::default();
    // 构造 128 个高重叠专家:基向量 + 微扰,使余弦普遍高于 overlap_threshold
    let mut profiles: HashMap<ExpertId, ExpertProfile> = HashMap::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for i in 0..128u32 {
        let id = ExpertId::new(format!("e-{i}"));
        // 高重叠:主分量 1.0 + 随 i 微变的次分量,余弦接近 1
        let mut v = vec![1.0_f32; 64];
        v[(i as usize) % 64] += 0.01 * (i as f32);
        profiles.insert(
            id.clone(),
            ExpertProfile::new(format!("e-{i}"), v, 0.8, vec!["code-gen".into()]),
        );
        // distinct gate,降序互异
        candidates.push((id, 0.5 + (i as f32) * 0.001));
    }

    c.bench_function("resolve_conflicts/128_high_overlap", |b| {
        b.iter(|| {
            let result =
                resolve_conflicts(candidates.clone(), &profiles, &config).expect("resolve ok");
            criterion::black_box(result);
        });
    });
}

/// 高密度冲突消解基准(专家 Agent 优化 2026-08-11:范数预计算 + 点积剪枝证伪)
///
/// n=512 高重叠:剪枝路径主战场,验证点积早停在高密度池的收益;
/// n=128 混合(50% 正交 + 50% 高重叠):同时覆盖剪枝与精确回退双路径。
fn bench_resolve_conflicts_high_density(c: &mut Criterion) {
    let config = GeaConfig::default();

    // 512 高重叠:范数预计算 + 剪枝路径
    let mut profiles: HashMap<ExpertId, ExpertProfile> = HashMap::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for i in 0..512u32 {
        let id = ExpertId::new(format!("d-{i}"));
        let mut v = vec![1.0_f32; 64];
        v[(i as usize) % 64] += 0.005 * (i as f32);
        profiles.insert(
            id.clone(),
            ExpertProfile::new(format!("d-{i}"), v, 0.8, vec!["code-gen".into()]),
        );
        candidates.push((id, 0.5 + (i as f32) * 0.0002));
    }
    c.bench_function("resolve_conflicts/512_high_overlap", |b| {
        b.iter(|| {
            let result =
                resolve_conflicts(candidates.clone(), &profiles, &config).expect("resolve ok");
            criterion::black_box(result);
        });
    });

    // 128 混合:前半正交(精确回退),后半高重叠(剪枝)
    let mut profiles_mix: HashMap<ExpertId, ExpertProfile> = HashMap::new();
    let mut candidates_mix: Vec<Candidate> = Vec::new();
    for i in 0..128u32 {
        let id = ExpertId::new(format!("m-{i}"));
        let mut v = vec![0.0_f32; 64];
        if i < 64 {
            v[i as usize] = 1.0; // 正交:无冲突,走精确回退 + 早停
        } else {
            v[0] = 1.0;
            v[(i as usize) % 64] += 0.01; // 高重叠:触发剪枝
        }
        profiles_mix.insert(
            id.clone(),
            ExpertProfile::new(format!("m-{i}"), v, 0.8, vec!["code-gen".into()]),
        );
        candidates_mix.push((id, 0.5 + (i as f32) * 0.002));
    }
    c.bench_function("resolve_conflicts/128_mixed", |b| {
        b.iter(|| {
            let result = resolve_conflicts(candidates_mix.clone(), &profiles_mix, &config)
                .expect("resolve ok");
            criterion::black_box(result);
        });
    });
}

/// 门控范数预计算路径基准(专家 Agent 优化 2026-08-11)
///
/// 对比 `compute_gate_value`(精确,每次重算范数)与
/// `compute_gate_value_with_norms`(预计算范数,内层仅点积)。
fn bench_gate_compute_with_norms(c: &mut Criterion) {
    let config = GeaConfig::default();
    let expert = ExpertProfile::new("e-1", vec![0.5; 64], 0.8, vec!["code-gen".into()]);
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 512]);
    // bench 侧计算范数输入(与 crate 内部 prefix_l2_norm 数学等价,无需逐位一致)
    let norm_of =
        |v: &[f32], len: usize| -> f32 { v.iter().take(len).map(|x| x * x).sum::<f32>().sqrt() };
    let task_norm = norm_of(&task.clv, expert.expert_vector.len());
    let expert_norm = norm_of(&expert.expert_vector, expert.expert_vector.len());

    c.bench_function("gate_compute_with_norms/512d_clv", |b| {
        b.iter(|| {
            let gate =
                compute_gate_value_with_norms(&task, task_norm, &expert, expert_norm, &config);
            criterion::black_box(gate);
        });
    });
}

/// 高密度专家池全链路激活基准(专家 Agent 优化 2026-08-11)
///
/// 128 专家 + 512 维 CLV:门控循环(范数缓存)+ 冲突消解(剪枝)全热路径,
/// 量化激活延迟随专家池规模的增长(激活效率维度 P0 基准)。
fn bench_activate_dense_pool(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let activator = GeaActivator::new(GeaConfig::default(), bus).unwrap();

    for i in 0..128u32 {
        let mut v = vec![0.0_f32; 64];
        v[(i as usize) % 64] = 1.0;
        v[((i as usize) + 1) % 64] = 0.5;
        activator.register_expert(ExpertProfile::new(
            format!("e-{i}"),
            v,
            0.8,
            vec!["code-gen".into()],
        ));
    }

    c.bench_function("activate_dense/128_experts_512d", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            // 每次新任务避免缓存命中,测全链路(门控 + 冲突 + 驱逐)
            let task = TaskProfile::new(
                0.5 + (idx % 100) as f32 * 0.005,
                "code-gen",
                30,
                vec![0.5; 512],
            );
            idx += 1;
            rt.block_on(activator.activate(&task)).unwrap();
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(std::time::Duration::from_millis(500));
    targets = bench_gate_compute, bench_gate_compute_512dim, bench_activate_with_cache, bench_activate_no_cache, bench_activate_eviction_saturated, bench_resolve_conflicts, bench_resolve_conflicts_high_density, bench_gate_compute_with_norms, bench_activate_dense_pool
}

criterion_main!(benches);
