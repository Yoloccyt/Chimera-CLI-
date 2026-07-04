//! MTPE 加速比验证测试 — 验证多步预测相比单步预测的加速效果
//!
//! 对应架构层:L7 Execution
//!
//! # 测试逻辑
//! - 方案 A:1000 次 N=5 预测 → 产出 5000 个 token
//! - 方案 B:5000 次 N=1 预测 → 产出 5000 个 token
//! - 加速比 = 方案 B 总延迟 / 方案 A 总延迟,应 > 3×
//!
//! # 设计依据
//! MTPE 一次推理预测 N 个 token,减少推理调用次数。
//! N=5 预测一次 vs 5 次单步预测,理论加速比 = 5/1 = 5×。
//! 考虑伪预测开销,实际加速比应 > 3×。
//!
//! # 运行方式
//! 性能断言测试标记 `#[ignore]`,避免在日常 `cargo test` 中运行。
//! 手动运行:`cargo test -p mtpe-executor --test speedup -- --ignored --nocapture`

#![allow(clippy::unwrap_used)]

use event_bus::EventBus;
use mtpe_executor::{MtpeConfig, MtpeExecutor, PredictionContext};

/// 构造测试上下文
fn make_context(quest_id: &str) -> PredictionContext {
    PredictionContext {
        quest_id: quest_id.into(),
        history: vec!["test context for speedup".into()],
        clv: vec![0.1; 8],
    }
}

/// 加速比测试:1000×N=5 vs 5000×N=1,加速比应 > 3×
///
/// WHY 标记 #[ignore]:性能测试受系统负载影响,结果不稳定,
/// 不应在日常 `cargo test` 中运行。手动运行时加 `--ignored` 标志
#[tokio::test]
#[ignore]
async fn test_speedup_n5_vs_single_step() {
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = make_context("q-speedup");

    // 方案 A:1000 次 N=5 预测(产出 5000 个 token)
    let start_a = std::time::Instant::now();
    for _ in 0..1000 {
        let _result = executor.predict(&ctx, 5).await.unwrap();
    }
    let elapsed_a = start_a.elapsed();

    // 方案 B:5000 次 N=1 预测(产出 5000 个 token)
    let start_b = std::time::Instant::now();
    for _ in 0..5000 {
        let _result = executor.predict(&ctx, 1).await.unwrap();
    }
    let elapsed_b = start_b.elapsed();

    let speedup = elapsed_b.as_secs_f64() / elapsed_a.as_secs_f64();

    println!("方案 A (1000×N=5): {:?}", elapsed_a);
    println!("方案 B (5000×N=1): {:?}", elapsed_b);
    println!("加速比: {:.2}×", speedup);

    // 加速比应 > 3×(理论值 5×,考虑伪预测开销留余量)
    assert!(
        speedup > 3.0,
        "加速比 {:.2}× 低于阈值 3×,方案 A: {:?}, 方案 B: {:?}",
        speedup,
        elapsed_a,
        elapsed_b
    );
}

/// 加速比测试:N=10 vs 10×N=1,加速比应 > 3×
#[tokio::test]
#[ignore]
async fn test_speedup_n10_vs_single_step() {
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = make_context("q-speedup-10");

    // 方案 A:500 次 N=10 预测(产出 5000 个 token)
    let start_a = std::time::Instant::now();
    for _ in 0..500 {
        let _result = executor.predict(&ctx, 10).await.unwrap();
    }
    let elapsed_a = start_a.elapsed();

    // 方案 B:5000 次 N=1 预测(产出 5000 个 token)
    let start_b = std::time::Instant::now();
    for _ in 0..5000 {
        let _result = executor.predict(&ctx, 1).await.unwrap();
    }
    let elapsed_b = start_b.elapsed();

    let speedup = elapsed_b.as_secs_f64() / elapsed_a.as_secs_f64();

    println!("方案 A (500×N=10): {:?}", elapsed_a);
    println!("方案 B (5000×N=1): {:?}", elapsed_b);
    println!("加速比: {:.2}×", speedup);

    assert!(speedup > 3.0, "加速比 {:.2}× 低于阈值 3×", speedup);
}

/// 验证 N 值越大,单次预测延迟增长缓慢(主要开销在推理启动,与 N 无关)
#[tokio::test]
#[ignore]
async fn test_latency_grows_slowly_with_n() {
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = make_context("q-latency");

    // 测量 N=1 的平均延迟
    let mut total_n1 = std::time::Duration::ZERO;
    for _ in 0..100 {
        let start = std::time::Instant::now();
        let _ = executor.predict(&ctx, 1).await.unwrap();
        total_n1 += start.elapsed();
    }
    let avg_n1 = total_n1 / 100;

    // 测量 N=10 的平均延迟
    let mut total_n10 = std::time::Duration::ZERO;
    for _ in 0..100 {
        let start = std::time::Instant::now();
        let _ = executor.predict(&ctx, 10).await.unwrap();
        total_n10 += start.elapsed();
    }
    let avg_n10 = total_n10 / 100;

    println!("N=1 平均延迟: {:?}", avg_n1);
    println!("N=10 平均延迟: {:?}", avg_n10);
    println!(
        "延迟比 (N=10/N=1): {:.2}×",
        avg_n10.as_secs_f64() / avg_n1.as_secs_f64()
    );

    // N=10 的延迟不应超过 N=1 的 2 倍(主要开销在推理启动,与 N 无关)
    assert!(
        avg_n10 < avg_n1 * 2,
        "N=10 延迟 {:?} 超过 N=1 延迟 {:?} 的 2 倍",
        avg_n10,
        avg_n1
    );
}
