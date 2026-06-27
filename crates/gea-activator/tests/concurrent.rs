//! GEA 并发测试 — 验证多线程并发激活无数据竞争
//!
//! 对应 SubTask 23.6
//!
//! # 测试目标
//! - 10 线程同时 activate,无 panic、无数据竞争
//! - 缓存并发读写正确性
//! - 专家注册表并发读正确性

use std::sync::Arc;
use std::time::Duration;

use event_bus::EventBus;
use gea_activator::{ExpertProfile, GeaActivator, GeaConfig, TaskProfile};

/// 构造测试用激活器,预注册若干专家
fn make_activator_with_experts() -> Arc<GeaActivator> {
    let config = GeaConfig::default();
    let bus = EventBus::new();
    let activator = Arc::new(GeaActivator::new(config, bus).unwrap());

    // 注册 5 个正交专家
    for i in 0..5 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        let expert = ExpertProfile::new(format!("e-{i}"), v, 0.8, vec!["code-gen".into()]);
        activator.register_expert(expert);
    }
    activator
}

#[tokio::test]
async fn test_concurrent_activate_no_panic() {
    let activator = make_activator_with_experts();
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    // 10 线程并发激活相同任务
    let mut handles = Vec::new();
    for _ in 0..10 {
        let activator = activator.clone();
        let task = task.clone();
        handles.push(tokio::spawn(async move {
            // 每个线程激活 10 次
            for _ in 0..10 {
                let result = activator.activate(&task).await.unwrap();
                // 结果应为激活或空(不 panic 即可)
                let _ = result.has_activated();
            }
        }));
    }

    // 等待所有线程完成,任一 panic 则测试失败
    for handle in handles {
        handle.await.expect("task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_activate_different_tasks() {
    let activator = make_activator_with_experts();

    // 10 线程并发激活不同任务(不同复杂度)
    let mut handles = Vec::new();
    for i in 0..10 {
        let activator = activator.clone();
        let task = TaskProfile::new(0.5 + i as f32 * 0.05, "code-gen", 30, vec![0.5; 64]);
        handles.push(tokio::spawn(async move {
            let result = activator.activate(&task).await.unwrap();
            assert!(result.activated.len() <= GeaConfig::default().top_k);
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_register_and_activate() {
    let config = GeaConfig::default();
    let bus = EventBus::new();
    let activator = Arc::new(GeaActivator::new(config, bus).unwrap());

    // 预注册部分专家
    for i in 0..3 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        activator.register_expert(ExpertProfile::new(
            format!("e-{i}"),
            v,
            0.8,
            vec!["code-gen".into()],
        ));
    }

    let mut handles = Vec::new();

    // 线程 1-3:并发激活
    for _ in 0..3 {
        let activator = activator.clone();
        let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);
        handles.push(tokio::spawn(async move {
            for _ in 0..20 {
                let _ = activator.activate(&task).await;
            }
        }));
    }

    // 线程 4-5:并发注册新专家
    for i in 3..5 {
        let activator = activator.clone();
        handles.push(tokio::spawn(async move {
            let mut v = vec![0.0; 64];
            v[i] = 1.0;
            activator.register_expert(ExpertProfile::new(
                format!("e-{i}"),
                v,
                0.8,
                vec!["code-gen".into()],
            ));
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }

    // 最终应有 5 个专家
    assert_eq!(activator.expert_count(), 5);
}

#[tokio::test]
async fn test_concurrent_unregister_and_activate() {
    let config = GeaConfig::default();
    let bus = EventBus::new();
    let activator = Arc::new(GeaActivator::new(config, bus).unwrap());

    // 预注册 5 个专家
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

    let mut handles = Vec::new();

    // 线程 1-3:并发激活
    for _ in 0..3 {
        let activator = activator.clone();
        let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);
        handles.push(tokio::spawn(async move {
            for _ in 0..20 {
                let _ = activator.activate(&task).await;
            }
        }));
    }

    // 线程 4:并发注销专家
    let activator_for_unregister = activator.clone();
    handles.push(tokio::spawn(async move {
        use gea_activator::ExpertId;
        activator_for_unregister.unregister_expert(&ExpertId::new("e-0"));
    }));

    for handle in handles {
        handle.await.expect("task panicked");
    }

    // 最终应有 4 个专家(注销了 e-0)
    assert_eq!(activator.expert_count(), 4);
}

#[tokio::test]
async fn test_concurrent_cache_consistency() {
    // 相同任务并发激活,缓存应一致(无重复写入导致的不一致)
    let activator = make_activator_with_experts();
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    let mut handles = Vec::new();
    for _ in 0..10 {
        let activator = activator.clone();
        let task = task.clone();
        handles.push(tokio::spawn(async move {
            activator.activate(&task).await.unwrap()
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("task panicked"));
    }

    // 所有结果应一致(相同任务,相同激活结果)
    let first = &results[0];
    for result in &results[1..] {
        assert_eq!(
            result.activated, first.activated,
            "inconsistent cache result"
        );
        assert_eq!(result.top_gate_value, first.top_gate_value);
    }
}

/// 性能断言测试:激活延迟应在合理范围内
///
/// 标记 `#[ignore]`,需用 `cargo test -- --ignored` 运行
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_activate_latency() {
    let activator = make_activator_with_experts();
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    // 预热缓存
    let _ = activator.activate(&task).await;

    // 测量 100 次激活延迟(含缓存命中)
    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _ = activator.activate(&task).await;
    }
    let elapsed = start.elapsed();

    // 100 次激活(含缓存命中)应在 1 秒内
    assert!(
        elapsed < Duration::from_secs(1),
        "100 activations took {elapsed:?}, expected < 1s"
    );
}

/// 性能断言测试:门控计算吞吐量
///
/// 标记 `#[ignore]`,需用 `cargo test -- --ignored` 运行
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_gate_compute_throughput() {
    use gea_activator::compute_gate_value;

    let config = GeaConfig::default();
    let expert = ExpertProfile::new("e-1", vec![0.5; 64], 0.8, vec!["code-gen".into()]);
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    let start = std::time::Instant::now();
    let iterations = 100_000;
    for _ in 0..iterations {
        let _ = compute_gate_value(&task, &expert, &config);
    }
    let elapsed = start.elapsed();

    // 10 万次门控计算应在 1 秒内
    assert!(
        elapsed < Duration::from_secs(1),
        "{iterations} gate computations took {elapsed:?}, expected < 1s"
    );
}
