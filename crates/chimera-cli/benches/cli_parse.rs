//! CLI 命令解析基准 — `--help` / `run` / `doctor` 的 clap 解析成本
//!
//! WHY 进程内测量：真正的进程冷启动 = 操作系统加载 + 静态初始化 + clap 解析 +
//! 命令执行。criterion 在进程内只能测「clap 解析」这一段的 CPU 成本；
//! 进程级冷启动（含二进制加载）在报告中以真实 `chimera.exe` 多次启动的实测
//! 数字给出（见 `outputs/perf-baseline-report.md` §2），两者口径分开登记，不混算。
//!
//! 运行：
//! ```powershell
//! cargo bench -p chimera-cli --bench cli_parse
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_cli::Cli;
use clap::Parser;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("cli_parse");
    group.sample_size(50);

    // `--help` 走 clap 的帮助渲染路径（构造完整命令树 + 生成帮助文本）
    group.bench_function("help", |b| {
        b.iter(|| black_box(Cli::try_parse_from(["chimera", "--help"])));
    });

    // 最常用子命令：run（默认只展示计划）
    group.bench_function("run", |b| {
        b.iter(|| black_box(Cli::try_parse_from(["chimera", "run", "hi"])));
    });

    // doctor（9 探针命令的解析面；不触发探针执行）
    group.bench_function("doctor", |b| {
        b.iter(|| black_box(Cli::try_parse_from(["chimera", "doctor"])));
    });

    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
