//! AHIRT 并发测试 — 验证多线程并发探测无 panic、无数据竞争
//!
//! 对应 SubTask 33.5
//!
//! # 测试目标
//! - 10 线程并发 probe / probe_all / verify_security,无 panic
//! - AhirtRedTeam 满足 Send + Sync 约束(可跨线程共享)
//! - 周期探测不阻塞主流程

use std::sync::Arc;
use std::time::Duration;

use parliament::{AhirtRedTeam, ProbePayloadLibrary, ProbeResult, ProbeType};

/// 静态断言:AhirtRedTeam 满足 Send + Sync(编译期检查)
fn _assert_send_sync<T: Send + Sync>() {}
fn _static_assertions() {
    _assert_send_sync::<AhirtRedTeam>();
    _assert_send_sync::<ProbePayloadLibrary>();
    _assert_send_sync::<ProbeResult>();
}

#[tokio::test]
async fn test_concurrent_probe_no_panic() {
    let red_team = Arc::new(AhirtRedTeam::default());

    // 10 线程并发执行不同类型的探测
    let mut handles = Vec::new();
    for i in 0..10 {
        let red_team = red_team.clone();
        let probe_type = match i % 4 {
            0 => ProbeType::PromptInjection,
            1 => ProbeType::CommandInjection,
            2 => ProbeType::PrivilegeEscalation,
            _ => ProbeType::SandboxEscape,
        };
        handles.push(tokio::spawn(async move {
            let results = red_team.probe(probe_type);
            assert_eq!(results.len(), 25, "线程 {i} 探测结果应为 25 个");
            // 所有结果应通过(系统正确拦截)
            for r in &results {
                assert!(r.passed, "线程 {i} 载荷 '{}' 未被拦截", r.payload);
            }
        }));
    }

    for handle in handles {
        handle.await.expect("并发探测 task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_probe_all_no_panic() {
    let red_team = Arc::new(AhirtRedTeam::default());

    // 10 线程并发执行全量探测
    let mut handles = Vec::new();
    for i in 0..10 {
        let red_team = red_team.clone();
        handles.push(tokio::spawn(async move {
            let stats = red_team.probe_all();
            assert_eq!(stats.total, 100, "线程 {i} 探测总数应为 100");
            assert!(
                stats.detection_rate > 0.95,
                "线程 {i} 探测率 {} 应 > 95%",
                stats.detection_rate
            );
        }));
    }

    for handle in handles {
        handle.await.expect("并发 probe_all task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_verify_security_no_panic() {
    let red_team = Arc::new(AhirtRedTeam::default());

    // 10 线程并发执行安全验证
    let mut handles = Vec::new();
    for i in 0..10 {
        let red_team = red_team.clone();
        handles.push(tokio::spawn(async move {
            let report = red_team.verify_security();
            assert_eq!(report.stats.total, 100, "线程 {i} 探测总数应为 100");
            assert!(
                report.stats.detection_rate > 0.95,
                "线程 {i} 探测率 {} 应 > 95%",
                report.stats.detection_rate
            );
        }));
    }

    for handle in handles {
        handle.await.expect("并发 verify_security task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_probe_single_no_panic() {
    let red_team = Arc::new(AhirtRedTeam::default());
    let library = ProbePayloadLibrary::new();

    // 10 线程并发执行单个载荷探测
    let mut handles = Vec::new();
    for i in 0..10 {
        let red_team = red_team.clone();
        let payload = library.all()[i % 100].clone();
        handles.push(tokio::spawn(async move {
            let result = red_team.probe_single(&payload);
            assert!(result.passed, "线程 {i} 载荷 '{}' 未被拦截", result.payload);
        }));
    }

    for handle in handles {
        handle.await.expect("并发 probe_single task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_trigger_probe_no_panic() {
    let red_team = Arc::new(AhirtRedTeam::default());

    // 10 线程并发触发不同类型探测
    let mut handles = Vec::new();
    for i in 0..10 {
        let red_team = red_team.clone();
        let probe_type = match i % 4 {
            0 => ProbeType::PromptInjection,
            1 => ProbeType::CommandInjection,
            2 => ProbeType::PrivilegeEscalation,
            _ => ProbeType::SandboxEscape,
        };
        handles.push(tokio::spawn(async move {
            red_team.trigger_probe(probe_type);
        }));
    }

    for handle in handles {
        handle.await.expect("并发 trigger_probe task panicked");
    }
}

#[tokio::test]
async fn test_periodic_probe_does_not_block() {
    let red_team = AhirtRedTeam::default();

    // 启动周期探测(间隔 1 小时,实际不会触发)
    let handle = red_team.spawn_periodic_probe(Duration::from_secs(3600));

    // 主流程应立即继续,不被阻塞
    let start = std::time::Instant::now();
    let _stats = red_team.probe_all();
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 500,
        "周期探测不应阻塞主流程,probe_all 耗时 {}ms",
        elapsed.as_millis()
    );

    // 清理周期探测 task
    handle.abort();
}

#[tokio::test]
async fn test_concurrent_mixed_operations_no_panic() {
    let red_team = Arc::new(AhirtRedTeam::default());

    // 10 线程混合执行不同操作(probe / probe_all / verify_security / trigger_probe)
    let mut handles = Vec::new();
    for i in 0..10 {
        let red_team = red_team.clone();
        handles.push(tokio::spawn(async move {
            match i % 4 {
                0 => {
                    let results = red_team.probe(ProbeType::PromptInjection);
                    assert_eq!(results.len(), 25);
                }
                1 => {
                    let stats = red_team.probe_all();
                    assert_eq!(stats.total, 100);
                }
                2 => {
                    let report = red_team.verify_security();
                    assert!(report.stats.detection_rate > 0.95);
                }
                _ => {
                    red_team.trigger_probe(ProbeType::SandboxEscape);
                }
            }
        }));
    }

    for handle in handles {
        handle.await.expect("混合并发操作 task panicked");
    }
}
