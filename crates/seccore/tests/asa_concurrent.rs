//! ASA 并发测试 — 验证多线程并发审计无数据竞争
//!
//! 对应 SubTask 32.5:10 线程并发 audit,无 panic、无数据竞争

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use seccore::{AsaAuditor, OperationAuditInput};

/// 构造测试用 OperationAuditInput。
fn make_input(content: &str, keywords: Vec<&str>, complexity: f32) -> OperationAuditInput {
    OperationAuditInput {
        operation_id: "concurrent-op".to_string(),
        content: content.to_string(),
        risk_keywords: keywords.iter().map(|s| s.to_string()).collect(),
        complexity_score: complexity,
        semantic_vector: None,
        reference_risk_vectors: Vec::new(),
    }
}

#[test]
fn test_concurrent_audit_no_panic() {
    // 10 线程并发 audit,每线程 100 次,共 1000 次
    let auditor = Arc::new(AsaAuditor::with_default_config());
    let success_count = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];

    for i in 0..10 {
        let auditor = Arc::clone(&auditor);
        let success_count = Arc::clone(&success_count);

        handles.push(thread::spawn(move || {
            for j in 0..100 {
                let input = make_input(&format!("op {i} {j}"), vec!["sudo"], 0.1);
                let result = auditor.audit(&input);
                // 验证评分在合理范围 [0.0, 1.0]
                assert!(result.safety_score >= 0.0 && result.safety_score <= 1.0);
                success_count.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    assert_eq!(success_count.load(Ordering::Relaxed), 1000);
}

#[test]
fn test_concurrent_audit_with_history_update() {
    // 混合读/写:5 线程 audit(读历史),5 线程 record_failure(写历史)
    // 验证 RwLock 在读多写少场景下无数据竞争
    let auditor = Arc::new(AsaAuditor::with_default_config());
    let mut handles = vec![];

    // 5 线程并发 audit(读历史)
    for i in 0..5 {
        let auditor = Arc::clone(&auditor);
        handles.push(thread::spawn(move || {
            for j in 0..100 {
                let input = make_input(&format!("audit {i} {j}"), vec!["sudo"], 0.1);
                let result = auditor.audit(&input);
                assert!(result.safety_score >= 0.0 && result.safety_score <= 1.0);
            }
        }));
    }

    // 5 线程并发 record_failure(写历史)
    for i in 0..5 {
        let auditor = Arc::clone(&auditor);
        handles.push(thread::spawn(move || {
            for j in 0..100 {
                auditor.record_failure(&format!("fail {i} {j}"));
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    // 验证历史统计正确:5 线程 × 100 次 = 500 次失败
    let (total, fail) = auditor.history_stats();
    assert_eq!(total, 500);
    assert_eq!(fail, 500);
}

#[test]
fn test_concurrent_audit_and_intervene() {
    // 并发 audit_and_intervene,验证 Block 级别正确返回 Err
    let auditor = Arc::new(AsaAuditor::with_default_config());
    let block_count = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];

    for _ in 0..10 {
        let auditor = Arc::clone(&auditor);
        let block_count = Arc::clone(&block_count);

        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                // 3 个关键字 → Block
                let input = make_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.0);
                let result = auditor.audit_and_intervene(&input);
                if result.is_err() {
                    block_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    // 1000 次 Block
    assert_eq!(block_count.load(Ordering::Relaxed), 1000);
}

#[test]
fn test_concurrent_mixed_interventions() {
    // 混合干预级别:Allow/Warn/Block 并发执行
    let auditor = Arc::new(AsaAuditor::with_default_config());
    let allow_count = Arc::new(AtomicUsize::new(0));
    let warn_count = Arc::new(AtomicUsize::new(0));
    let block_count = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];

    // Allow 线程:无关键字
    {
        let auditor = Arc::clone(&auditor);
        let allow_count = Arc::clone(&allow_count);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let input = make_input("echo hello", vec![], 0.0);
                if let Ok(r) = auditor.audit_and_intervene(&input) {
                    if r.intervention == seccore::InterventionAction::Allow {
                        allow_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    // Warn 线程:2 个关键字
    {
        let auditor = Arc::clone(&auditor);
        let warn_count = Arc::clone(&warn_count);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let input = make_input("sudo rm", vec!["sudo", "rm"], 0.0);
                if let Ok(r) = auditor.audit_and_intervene(&input) {
                    if r.intervention == seccore::InterventionAction::Warn {
                        warn_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    // Block 线程:3 个关键字
    {
        let auditor = Arc::clone(&auditor);
        let block_count = Arc::clone(&block_count);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let input = make_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.0);
                if auditor.audit_and_intervene(&input).is_err() {
                    block_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    assert_eq!(allow_count.load(Ordering::Relaxed), 100);
    assert_eq!(warn_count.load(Ordering::Relaxed), 100);
    assert_eq!(block_count.load(Ordering::Relaxed), 100);
}

#[test]
#[ignore = "perf: run with --ignored"]
fn test_audit_latency_under_5ms() {
    // 性能断言:审计延迟 < 5ms/操作
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.5);

    // 预热(消除冷启动噪声)
    for _ in 0..100 {
        let _ = auditor.audit(&input);
    }

    // 测量 1000 次审计的总延迟
    let start = Instant::now();
    for _ in 0..1000 {
        let _ = auditor.audit(&input);
    }
    let elapsed = start.elapsed();

    let avg_latency_us = elapsed.as_micros() / 1000;
    assert!(
        avg_latency_us < 5000,
        "审计延迟 {avg_latency_us}μs 超过 5000μs (5ms) 限制"
    );
}
