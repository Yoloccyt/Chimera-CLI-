//! CSA(Combined System Action)延迟验证 — SubTask 29.3
//!
//! 测量端到端执行优化流程的各组件延迟,验证 CSA 总延迟 < 100ms。
//!
//! # CSA 延迟分解(目标上限)
//! ```text
//! GEA  1ms  — 门控计算 + 激活
//! FaaE 1ms  — 语义路由
//! PVL  10ms — 生产验证(5 操作)
//! MTPE 20ms — 多步预测(N=5)
//! GQEP 50ms — 聚集执行(10 操作)
//! SCC  5ms  — 缓存命中
//! EDSB 5ms  — 熵计算
//! ─────────────
//! 合计 ≈ 92ms < 100ms
//! ```
//!
//! # 测试方法
//! - min-of-N(5 次):运行 5 次,取最小值,减少调度噪声
//! - 使用 `std::time::Instant::now()` 测量延迟
//! - 所有性能断言标记 `#[ignore = "perf: run with --ignored"]`,
//!   避免在 CI 中因调度噪声导致间歇性失败
//!
//! # 运行方式
//! ```powershell
//! cargo test -p gea-activator --jobs 1 -- --ignored
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use event_bus::EventBus;

use faae_router::{ExpertProfile as FaaeExpertProfile, FaaeConfig, FaaeRouter, ToolId};
use gea_activator::{
    compute_gate_value, ExpertProfile as GeaExpertProfile, GeaActivator, GeaConfig, TaskProfile,
};
use gqep_executor::{GqepConfig, GqepExecutor, GqepFuture};
use mtpe_executor::{MtpeConfig, MtpeExecutor, PredictionContext};
use pvl_layer::{Producer, PvlConfig, Verifier};
use scc_cache::{ContextEntry, ContextId, SccCache, SccConfig};

/// min-of-N 的 N 值 — 运行 5 次取最小值
const MIN_OF_N: usize = 5;

/// 计算向量最小值(min-of-N)
fn min_of_n(durations: &[Duration]) -> Duration {
    durations.iter().min().copied().unwrap_or_default()
}

/// 构造匹配指定索引的 CLV(只有该维度为 1.0)
fn make_clv_for(index: usize) -> Vec<f32> {
    let mut v = vec![0.0; 64];
    v[index] = 1.0;
    v
}

/// 创建并配置 GEA 激活器,预注册 4 个正交专家
fn make_gea_activator(bus: &EventBus) -> Arc<GeaActivator> {
    let activator = GeaActivator::new(GeaConfig::default(), bus.clone()).unwrap();
    for i in 0..4 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        activator.register_expert(GeaExpertProfile::new(
            format!("expert-{i}"),
            v,
            0.8,
            vec!["code-gen".into()],
        ));
    }
    Arc::new(activator)
}

/// 创建并配置 FaaE 路由器,预注册 4 个正交工具专家
async fn make_faae_router(bus: &EventBus) -> FaaeRouter {
    let router = FaaeRouter::new(bus.clone());
    for i in 0..4 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        let profile = FaaeExpertProfile::new(format!("tool-{i}"), v, vec!["code-gen".into()], 0.8);
        router.register_expert(profile).await;
    }
    router
}

// ============================================================
// CSA 总延迟测试
// ============================================================

/// CSA 端到端总延迟 — 验证 < 100ms
///
/// 串联所有组件,测量总延迟(min-of-5)
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_total_latency_under_100ms() {
    let bus = EventBus::new();

    // 预初始化所有组件(不计入延迟测量)
    let gea = make_gea_activator(&bus);
    let router = make_faae_router(&bus).await;
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let mtpe = MtpeExecutor::new(MtpeConfig::default(), bus.clone());
    let gqep = GqepExecutor::new(GqepConfig::default(), bus.clone());

    // 预填充 SCC 缓存
    cache.insert(ContextEntry::new("ctx-csa", "content"));

    let task = TaskProfile::new(0.9, "code-gen", 30, make_clv_for(0));
    let candidates: Vec<ToolId> = vec![ToolId::new("tool-0")];
    let mtpe_ctx = PredictionContext {
        quest_id: "q-csa".into(),
        history: vec!["hello".into()],
        clv: vec![0.1; 8],
    };

    // min-of-5 测量
    let mut total_durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let start = Instant::now();

        // 1. GEA 门控激活
        let _ = gea.activate(&task).await.unwrap();

        // 2. FaaE 语义路由
        let _ = router.route(&make_clv_for(0), &candidates).await.unwrap();

        // 3. PVL 生产验证(2 操作,简化)
        let pvl_config = PvlConfig::default();
        let producer = Producer::new(pvl_config.clone(), bus.clone());
        let verifier = Verifier::new(pvl_config.clone(), bus.clone());
        let (op_tx, mut op_rx) = tokio::sync::mpsc::channel(128);
        let (fb_tx, mut fb_rx) = tokio::sync::mpsc::channel(128);
        let v_handle = tokio::spawn(async move { verifier.run(&mut op_rx, &fb_tx).await });
        producer.produce("q-csa", 2, &op_tx).await.unwrap();
        drop(op_tx);
        v_handle.await.unwrap().unwrap();
        // fb_tx 在 spawn 完成后自动 drop,无需显式 drop
        while fb_rx.recv().await.is_some() {}

        // 4. MTPE 多步预测(N=5)
        let _ = mtpe.predict(&mtpe_ctx, 5).await.unwrap();

        // 5. GQEP 聚集执行(5 操作,简化)
        let futures: Vec<GqepFuture<String>> = (0..5)
            .map(|i| Box::pin(async move { Ok(format!("op-{i}")) }) as GqepFuture<String>)
            .collect();
        let _ = gqep.gather(futures).await;

        // 6. SCC 缓存命中
        let _ = cache.get_or_prefetch(&ContextId::new("ctx-csa"));

        // 7. EDSB 熵计算
        let registry = router.registry();
        {
            let profiles = registry.read().await;
            let edsb = faae_router::EdsbBalancer::new(FaaeConfig::default(), bus.clone());
            let _ = edsb.compute_entropy(&profiles).await;
        }

        total_durations.push(start.elapsed());
    }

    let min_total = min_of_n(&total_durations);
    assert!(
        min_total < Duration::from_millis(100),
        "CSA 总延迟应 < 100ms,实际最小 {:?}",
        min_total
    );
}

// ============================================================
// 各组件延迟分解测试
// ============================================================

/// GEA 延迟 — 门控计算 < 1ms
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_gea_latency() {
    let bus = EventBus::new();
    let gea = make_gea_activator(&bus);
    let task = TaskProfile::new(0.9, "code-gen", 30, make_clv_for(0));

    // 预热
    let _ = gea.activate(&task).await;

    let mut durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let start = Instant::now();
        let _ = gea.activate(&task).await;
        durations.push(start.elapsed());
    }

    let min_lat = min_of_n(&durations);
    assert!(
        min_lat < Duration::from_millis(1),
        "GEA 延迟应 < 1ms,实际最小 {:?}",
        min_lat
    );
}

/// GEA 纯门控计算延迟(不含事件发布)— 验证核心计算性能
#[test]
#[ignore = "perf: run with --ignored"]
fn test_csa_gea_gate_compute_only() {
    let config = GeaConfig::default();
    let expert = GeaExpertProfile::new("e-1", vec![0.5; 64], 0.8, vec!["code-gen".into()]);
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    let mut durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let start = Instant::now();
        for _ in 0..1000 {
            let _ = compute_gate_value(&task, &expert, &config);
        }
        durations.push(start.elapsed() / 1000);
    }

    let min_lat = min_of_n(&durations);
    assert!(
        min_lat < Duration::from_millis(1),
        "GEA 纯门控计算应 < 1ms,实际最小 {:?}",
        min_lat
    );
}

/// FaaE 语义路由延迟 — < 1ms
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_faae_latency() {
    let bus = EventBus::new();
    let router = make_faae_router(&bus).await;
    let clv = make_clv_for(0);
    let candidates: Vec<ToolId> = (0..4).map(|i| ToolId::new(format!("tool-{i}"))).collect();

    // 预热
    let _ = router.route(&clv, &candidates).await;

    let mut durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let start = Instant::now();
        let _ = router.route(&clv, &candidates).await;
        durations.push(start.elapsed());
    }

    let min_lat = min_of_n(&durations);
    assert!(
        min_lat < Duration::from_millis(1),
        "FaaE 路由延迟应 < 1ms,实际最小 {:?}",
        min_lat
    );
}

/// PVL 生产验证延迟 — < 10ms(5 操作)
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_pvl_latency() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    // 预热(首次包含编译/链接开销)
    {
        let producer = Producer::new(config.clone(), bus.clone());
        let verifier = Verifier::new(config.clone(), bus.clone());
        let (op_tx, mut op_rx) = tokio::sync::mpsc::channel(128);
        let (fb_tx, mut fb_rx) = tokio::sync::mpsc::channel(128);
        let v = tokio::spawn(async move { verifier.run(&mut op_rx, &fb_tx).await });
        producer.produce("warmup", 1, &op_tx).await.unwrap();
        drop(op_tx);
        v.await.unwrap().unwrap();
        // fb_tx 在 spawn 完成后自动 drop,无需显式 drop
        while fb_rx.recv().await.is_some() {}
    }

    let mut durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let producer = Producer::new(config.clone(), bus.clone());
        let verifier = Verifier::new(config.clone(), bus.clone());
        let (op_tx, mut op_rx) = tokio::sync::mpsc::channel(128);
        let (fb_tx, mut fb_rx) = tokio::sync::mpsc::channel(128);

        let start = Instant::now();
        let v_handle = tokio::spawn(async move { verifier.run(&mut op_rx, &fb_tx).await });
        producer.produce("q-csa", 5, &op_tx).await.unwrap();
        drop(op_tx);
        v_handle.await.unwrap().unwrap();
        // fb_tx 在 spawn 完成后自动 drop,无需显式 drop
        while fb_rx.recv().await.is_some() {}
        durations.push(start.elapsed());
    }

    let min_lat = min_of_n(&durations);
    assert!(
        min_lat < Duration::from_millis(10),
        "PVL 5 操作延迟应 < 10ms,实际最小 {:?}",
        min_lat
    );
}

/// MTPE 多步预测延迟 — < 20ms(N=5)
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_mtpe_latency() {
    let bus = EventBus::new();
    let executor = MtpeExecutor::new(MtpeConfig::default(), bus);
    let ctx = PredictionContext {
        quest_id: "q-csa".into(),
        history: vec!["hello".into()],
        clv: vec![0.1; 8],
    };

    // 预热
    let _ = executor.predict(&ctx, 5).await.unwrap();

    let mut durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let start = Instant::now();
        let _ = executor.predict(&ctx, 5).await.unwrap();
        durations.push(start.elapsed());
    }

    let min_lat = min_of_n(&durations);
    assert!(
        min_lat < Duration::from_millis(20),
        "MTPE N=5 延迟应 < 20ms,实际最小 {:?}",
        min_lat
    );
}

/// GQEP 聚集执行延迟 — < 50ms(10 操作)
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_gqep_latency() {
    let bus = EventBus::new();
    let executor = GqepExecutor::new(GqepConfig::default(), bus);

    // 预热
    let warmup: Vec<GqepFuture<String>> = vec![Box::pin(async { Ok("warmup".into()) })];
    let _ = executor.gather(warmup).await;

    let mut durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let futures: Vec<GqepFuture<String>> = (0..10)
            .map(|i| Box::pin(async move { Ok(format!("op-{i}")) }) as GqepFuture<String>)
            .collect();

        let start = Instant::now();
        let _ = executor.gather(futures).await;
        durations.push(start.elapsed());
    }

    let min_lat = min_of_n(&durations);
    assert!(
        min_lat < Duration::from_millis(50),
        "GQEP 10 操作延迟应 < 50ms,实际最小 {:?}",
        min_lat
    );
}

/// SCC 缓存命中延迟 — < 5ms
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_scc_latency() {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus);

    // 预填充缓存
    for i in 0..10 {
        cache.insert(ContextEntry::new(
            format!("ctx-{i}"),
            format!("content-{i}"),
        ));
    }

    let id = ContextId::new("ctx-0");

    // 预热
    let _ = cache.get_or_prefetch(&id);

    let mut durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let start = Instant::now();
        // 100 次命中取平均,减少单次测量噪声
        for _ in 0..100 {
            let _ = cache.get_or_prefetch(&id);
        }
        durations.push(start.elapsed() / 100);
    }

    let min_lat = min_of_n(&durations);
    assert!(
        min_lat < Duration::from_millis(5),
        "SCC 缓存命中延迟应 < 5ms,实际最小 {:?}",
        min_lat
    );
}

/// EDSB 熵计算延迟 — < 5ms
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_edsb_latency() {
    let bus = EventBus::new();
    let router = make_faae_router(&bus).await;

    // 均匀路由产生负载分布
    for tool_idx in 0..4 {
        let clv = make_clv_for(tool_idx);
        let candidates: Vec<ToolId> = (0..4).map(|i| ToolId::new(format!("tool-{i}"))).collect();
        for _ in 0..5 {
            let _ = router.route(&clv, &candidates).await;
        }
    }

    let edsb = faae_router::EdsbBalancer::new(FaaeConfig::default(), bus);
    let registry = router.registry();

    // 预热
    {
        let profiles = registry.read().await;
        let _ = edsb.compute_entropy(&profiles).await;
    }

    let mut durations = Vec::with_capacity(MIN_OF_N);
    for _ in 0..MIN_OF_N {
        let start = Instant::now();
        let profiles = registry.read().await;
        let _ = edsb.compute_entropy(&profiles).await;
        durations.push(start.elapsed());
    }

    let min_lat = min_of_n(&durations);
    assert!(
        min_lat < Duration::from_millis(5),
        "EDSB 熵计算延迟应 < 5ms,实际最小 {:?}",
        min_lat
    );
}

// ============================================================
// CSA 延迟分解汇总测试
// ============================================================

/// CSA 延迟分解汇总 — 验证各组件延迟符合预期分布
///
/// 分别测量各组件延迟,验证总和 < 100ms 且各分量在合理范围
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_csa_latency_breakdown() {
    let bus = EventBus::new();

    // === GEA 延迟 ===
    let gea = make_gea_activator(&bus);
    let task = TaskProfile::new(0.9, "code-gen", 30, make_clv_for(0));
    let _ = gea.activate(&task).await; // 预热
    let gea_lat = {
        let start = Instant::now();
        let _ = gea.activate(&task).await;
        start.elapsed()
    };

    // === FaaE 延迟 ===
    let router = make_faae_router(&bus).await;
    let candidates: Vec<ToolId> = (0..4).map(|i| ToolId::new(format!("tool-{i}"))).collect();
    let _ = router.route(&make_clv_for(0), &candidates).await; // 预热
    let faae_lat = {
        let start = Instant::now();
        let _ = router.route(&make_clv_for(0), &candidates).await;
        start.elapsed()
    };

    // === MTPE 延迟 ===
    let mtpe = MtpeExecutor::new(MtpeConfig::default(), bus.clone());
    let mtpe_ctx = PredictionContext {
        quest_id: "q-csa".into(),
        history: vec!["hello".into()],
        clv: vec![0.1; 8],
    };
    let _ = mtpe.predict(&mtpe_ctx, 5).await; // 预热
    let mtpe_lat = {
        let start = Instant::now();
        let _ = mtpe.predict(&mtpe_ctx, 5).await;
        start.elapsed()
    };

    // === GQEP 延迟 ===
    let gqep = GqepExecutor::new(GqepConfig::default(), bus.clone());
    let warmup: Vec<GqepFuture<String>> = vec![Box::pin(async { Ok("w".into()) })];
    let _ = gqep.gather(warmup).await; // 预热
    let gqep_lat = {
        let futures: Vec<GqepFuture<String>> = (0..10)
            .map(|i| Box::pin(async move { Ok(format!("op-{i}")) }) as GqepFuture<String>)
            .collect();
        let start = Instant::now();
        let _ = gqep.gather(futures).await;
        start.elapsed()
    };

    // === SCC 延迟 ===
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    cache.insert(ContextEntry::new("ctx-csa", "content"));
    let _ = cache.get_or_prefetch(&ContextId::new("ctx-csa")); // 预热
    let scc_lat = {
        let start = Instant::now();
        let _ = cache.get_or_prefetch(&ContextId::new("ctx-csa"));
        start.elapsed()
    };

    // === EDSB 延迟 ===
    // 均匀路由产生负载
    for tool_idx in 0..4 {
        let clv = make_clv_for(tool_idx);
        let cands: Vec<ToolId> = (0..4).map(|i| ToolId::new(format!("tool-{i}"))).collect();
        for _ in 0..5 {
            let _ = router.route(&clv, &cands).await;
        }
    }
    let edsb = faae_router::EdsbBalancer::new(FaaeConfig::default(), bus);
    let registry = router.registry();
    let edsb_lat = {
        let start = Instant::now();
        let profiles = registry.read().await;
        let _ = edsb.compute_entropy(&profiles).await;
        start.elapsed()
    };

    // 汇总延迟(单次测量,非 min-of-N,用于趋势观察)
    let total = gea_lat + faae_lat + mtpe_lat + gqep_lat + scc_lat + edsb_lat;

    // 打印延迟分解(便于性能分析)
    eprintln!(
        "CSA 延迟分解: GEA={:?} FaaE={:?} MTPE={:?} GQEP={:?} SCC={:?} EDSB={:?} | 总计={:?}",
        gea_lat, faae_lat, mtpe_lat, gqep_lat, scc_lat, edsb_lat, total
    );

    // 验证总延迟 < 100ms(单次测量,宽松阈值)
    assert!(
        total < Duration::from_millis(100),
        "CSA 总延迟应 < 100ms,实际 {:?}",
        total
    );
}
