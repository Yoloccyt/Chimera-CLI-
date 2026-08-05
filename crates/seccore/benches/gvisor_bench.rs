//! gVisor 执行 vs 降级路径延迟对比 — criterion 基准测试
//!
//! 对应 P3-6: gVisor benchmark 验证
//! 运行: `cargo bench -p seccore --jobs 1`
//!
//! ## 基准场景
//!
//! - `gvisor_execute`: 通过 gVisor runsc OCI bundle 执行命令的延迟
//!   (仅 Linux + runsc 可用时运行,标记 `#[ignore]` 需 `--ignored` 运行)
//! - `sandbox_execute`: 进程级降级路径(`tokio::process::Command`)的延迟
//! - `gvisor_vs_sandbox`: 两者对比(仅 Linux + runsc 可用)
//!
//! ## 跨平台策略
//!
//! - Windows/macOS: gVisor 基准跳过(标记 `#[ignore]`),降级路径基准始终可用
//! - `#[cfg(target_os = "linux")]` 条件编译保护 gVisor 相关代码

use criterion::{black_box, criterion_group, criterion_main, Criterion};

// WHY cfg 门控:Duration 与 GvisorRuntime 仅在 Linux + runsc 分支使用,
// Windows/macOS 编译时导入会产生 unused_imports 违规(-D warnings 拦截)
#[cfg(target_os = "linux")]
use seccore::gvisor::GvisorRuntime;
use seccore::sandbox::Sandbox;
use seccore::types::Command;
#[cfg(target_os = "linux")]
use seccore::types::{CommandSpec, RiskLevel};
#[cfg(target_os = "linux")]
use std::time::Duration;

/// 构造基准测试用 CommandSpec(echo 命令,低风险) — 仅 gVisor Linux 分支使用
#[cfg(target_os = "linux")]
fn make_echo_spec() -> CommandSpec {
    CommandSpec {
        program: "echo".to_string(),
        allowed_args: vec!["benchmark-test".to_string()],
        env_whitelist: [("PATH".into(), "/usr/bin:/bin".into())].into(),
        risk_level: RiskLevel::Low,
        risk_score: 10,
    }
}

/// 进程级降级路径延迟(始终可用) — Sandbox::audit_and_execute 全链路(含策略校验)
fn bench_sandbox_execute(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 应成功");

    c.bench_function("sandbox_execute", |b| {
        b.to_async(&rt).iter(|| async {
            let mut sandbox = Sandbox::with_default_policy();
            let command = Command::new("echo").arg("benchmark-test");
            black_box(sandbox.audit_and_execute(command).await)
        });
    });
}

/// gVisor execute 延迟(仅 Linux + runsc 可用,默认忽略)
#[cfg(target_os = "linux")]
fn bench_gvisor_execute(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 应成功");
    let spec = make_echo_spec();

    // 检测 runsc 是否可用,不可用时跳过基准
    let runtime = match GvisorRuntime::detect("/usr/local/bin/runsc") {
        Some(r) => r,
        None => {
            eprintln!("警告: runsc 不可用,跳过 gVisor 基准");
            return;
        }
    };

    c.bench_function("gvisor_execute", |b| {
        b.to_async(&rt).iter(|| async {
            let result = runtime.spawn(black_box(&spec)).await;
            black_box(result)
        });
    });
}

/// gVisor vs 降级路径对比(仅 Linux + runsc 可用,默认忽略)
#[cfg(target_os = "linux")]
fn bench_gvisor_vs_sandbox(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 应成功");
    let spec = make_echo_spec();

    let runtime = match GvisorRuntime::detect("/usr/local/bin/runsc") {
        Some(r) => r,
        None => {
            eprintln!("警告: runsc 不可用,跳过 gVisor vs 降级对比基准");
            return;
        }
    };

    let mut group = c.benchmark_group("gvisor_vs_sandbox");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    group.bench_function("gvisor", |b| {
        b.to_async(&rt).iter(|| async {
            let result = runtime.spawn(black_box(&spec)).await;
            black_box(result)
        });
    });

    group.bench_function("sandbox", |b| {
        b.to_async(&rt).iter(|| async {
            let mut sandbox = Sandbox::with_default_policy();
            let command = Command::new("echo").arg("benchmark-test");
            black_box(sandbox.audit_and_execute(command).await)
        });
    });

    group.finish();
}

#[cfg(target_os = "linux")]
criterion_group! {
    name = gvisor_benches;
    config = Criterion::default().significance_level(0.05).sample_size(10);
    targets = bench_gvisor_execute, bench_gvisor_vs_sandbox, bench_sandbox_execute
}

#[cfg(not(target_os = "linux"))]
criterion_group! {
    name = gvisor_benches;
    config = Criterion::default().significance_level(0.05).sample_size(10);
    targets = bench_sandbox_execute
}

criterion_main!(gvisor_benches);
