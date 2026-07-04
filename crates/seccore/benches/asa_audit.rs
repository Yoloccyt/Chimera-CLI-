//! ASA 审计延迟基准 — criterion 基准测试
//!
//! 对应 SubTask 32.5:审计延迟基准
//! 运行:`cargo bench -p seccore --jobs 1`

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use seccore::{AsaAuditor, OperationAuditInput};

/// 构造基准测试用 OperationAuditInput。
fn make_input(content: &str, keywords: Vec<&str>, complexity: f32) -> OperationAuditInput {
    OperationAuditInput {
        operation_id: "bench-op".to_string(),
        content: content.to_string(),
        risk_keywords: keywords.iter().map(|s| s.to_string()).collect(),
        complexity_score: complexity,
    }
}

/// Allow 级别审计基准(无风险关键字)。
fn bench_audit_allow(c: &mut Criterion) {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("echo hello", vec![], 0.1);

    c.bench_function("audit_allow", |b| {
        b.iter(|| {
            black_box(auditor.audit(black_box(&input)));
        });
    });
}

/// Warn 级别审计基准(2 个风险关键字)。
fn bench_audit_warn(c: &mut Criterion) {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo rm", vec!["sudo", "rm"], 0.1);

    c.bench_function("audit_warn", |b| {
        b.iter(|| {
            black_box(auditor.audit(black_box(&input)));
        });
    });
}

/// Block 级别审计基准(3 个风险关键字)。
fn bench_audit_block(c: &mut Criterion) {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.1);

    c.bench_function("audit_block", |b| {
        b.iter(|| {
            black_box(auditor.audit(black_box(&input)));
        });
    });
}

/// 带历史记录的审计基准(预填充 100 条失败记录)。
fn bench_audit_with_history(c: &mut Criterion) {
    let auditor = AsaAuditor::with_default_config();
    // 预填充历史(100 次失败)
    for _ in 0..100 {
        auditor.record_failure("hist-fail");
    }
    let input = make_input("sudo test", vec!["sudo"], 0.1);

    c.bench_function("audit_with_history", |b| {
        b.iter(|| {
            black_box(auditor.audit(black_box(&input)));
        });
    });
}

/// audit_and_intervene 基准(含干预决策)。
fn bench_audit_and_intervene(c: &mut Criterion) {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo rm", vec!["sudo", "rm"], 0.1);

    c.bench_function("audit_and_intervene", |b| {
        b.iter(|| {
            // Warn 级别,返回 Ok
            let _ = black_box(auditor.audit_and_intervene(black_box(&input)));
        });
    });
}

criterion_group!(
    benches,
    bench_audit_allow,
    bench_audit_warn,
    bench_audit_block,
    bench_audit_with_history,
    bench_audit_and_intervene
);
criterion_main!(benches);
