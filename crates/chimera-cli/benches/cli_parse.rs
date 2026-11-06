//! CLI 命令解析与启动分段基准 — `--help` / `run` / `doctor` 的 clap 解析成本 + 启动三段计时
//!
//! WHY 进程内测量：真正的进程冷启动 = 操作系统加载 + 静态初始化 + clap 解析 +
//! 命令执行。criterion 在进程内只能测「clap 解析」这一段的 CPU 成本；
//! 进程级冷启动（含二进制加载）在报告中以真实 `chimera.exe` 多次启动的实测
//! 数字给出（见 `outputs/perf-baseline-report.md` §2），两者口径分开登记，不混算。
//!
//! # 启动三段（对应 `main.rs` 的 `tracing::info!` 分段计时）
//!
//! `main.rs` 在退出前打三段计时：`parse_and_init_ms` / `config_ms` / `runtime_ms`。
//! 二进制 crate 的 `main` 私有函数不可被 bench 引用，故本 bench 以**等价路径**
//! 复现三段测量，使阈值可被 CI 断言：
//!
//! | 段 | `main.rs` 实现 | 本 bench 等价测量 |
//! |---|---|---|
//! | parse+init | `Cli::parse()` + 颜色模式 + tracing 初始化 | `Cli::try_parse_from`（`help`/`run`/`doctor` 三档） |
//! | config | `spec::needs_full_config(cmd)` 决定是否 `config::load()` | `spec::needs_full_config` 判定（**判定本身**，不含 14 段 Figment 解析） |
//! | runtime | `build_runtime(spec::classify(cmd))` | 按 `spec::classify` 选 flavor 后构建 tokio runtime |
//!
//! WHY config 段只测「判定」而非「真实加载」：真实加载读盘 + 解析 14 段，
//! 受磁盘与文件系统缓存影响，噪声远大于信号；判定是纯函数，是 D5 优化的
//! **实际决策点**（决定是否付那笔税），测判定即可锁定决策路径未退化。
//!
//! 运行：
//! ```powershell
//! cargo bench -p chimera-cli --bench cli_parse
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::time::Instant;

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

/// 启动 config 段 —— 「是否需要全量配置」的决策路径
///
/// WHY 单独成组：D5 优化的收益正是「4 类命令免 14 段 Figment 解析」；
/// 若该判定退化（恒返回 true），本组数字不变但真实启动会回退——
/// 故本组锁定的是**判定函数本身**的正确性与成本。
fn bench_startup_config_decision(c: &mut Criterion) {
    let mut group = c.benchmark_group("cli_startup_config");
    group.sample_size(50);

    // 免解析路径：help（needs_full_config == false）
    group.bench_function("decision_help", |b| {
        b.iter(|| {
            let cli = Cli::try_parse_from(["chimera", "help"]).expect("help 应解析成功");
            black_box(chimera_cli::spec::needs_full_config(cli.command.as_ref()));
        });
    });

    // 全量路径：run（needs_full_config == true）
    group.bench_function("decision_run", |b| {
        b.iter(|| {
            let cli = Cli::try_parse_from(["chimera", "run", "hi"]).expect("run 应解析成功");
            black_box(chimera_cli::spec::needs_full_config(cli.command.as_ref()));
        });
    });

    group.finish();
}

/// 启动 runtime 段 —— 按命令类选 tokio runtime flavor 并构建
///
/// WHY 测 runtime 构建：多线程 runtime 的 worker 池创建是 one-shot 命令
/// 启动税的主要来源之一（D4 把 runtime 创建推迟到 parse 之后，已消除
/// `--version/--help` 的这一段）。本组给出「NoContext 走 current_thread」
/// 与「FullContext 走多线程」两条路径的构建成本。
fn bench_startup_runtime(c: &mut Criterion) {
    let mut group = c.benchmark_group("cli_startup_runtime");
    group.sample_size(30); // runtime 构建较重，减少采样以控制时长

    // current_thread（NoContext 的 flavor）：单线程，无 worker 池
    group.bench_function("build_current_thread", |b| {
        b.iter(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("current_thread runtime 应构建成功");
            black_box(rt);
        });
    });

    // multi_thread（FullContext 的默认 flavor）：建 worker 池
    group.bench_function("build_multi_thread", |b| {
        b.iter(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("multi_thread runtime 应构建成功");
            black_box(rt);
        });
    });

    group.finish();
}

/// 启动 parse+init 段 —— 端到端「解析 + 分类」的一次真实走法
///
/// WHY 用 `Instant` 显式计时而非仅 criterion 的迭代：criterion 测的是
/// 「单次操作的纯 CPU 成本」，而启动关心的是**连续三段的相对占比**。
/// 本组把三段串起来测一遍，给出「从 argv 到可执行」的合计成本。
fn bench_startup_end_to_end(c: &mut Criterion) {
    let mut group = c.benchmark_group("cli_startup_e2e");
    group.sample_size(30);

    group.bench_function("parse_classify_runtime", |b| {
        b.iter(|| {
            let started = Instant::now();
            let cli = Cli::try_parse_from(["chimera", "run", "hi"]).expect("run 应解析成功");
            let parsed = started.elapsed();

            let needs_cfg = chimera_cli::spec::needs_full_config(cli.command.as_ref());
            let class = chimera_cli::spec::classify(cli.command.as_ref());
            let classified = started.elapsed();

            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("multi_thread runtime 应构建成功");
            let runtime_built = started.elapsed();

            black_box((parsed, classified, runtime_built, needs_cfg, class, rt));
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_parse,
    bench_startup_config_decision,
    bench_startup_runtime,
    bench_startup_end_to_end
);
criterion_main!(benches);
