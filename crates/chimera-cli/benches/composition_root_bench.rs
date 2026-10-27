//! C12 组合根装配延迟基准（dev-only，登记于 bench_inventory_freeze.txt）。
//!
//! WHY: 组合根（build / build_app_server）位于每条命令启动路径上；无基线则未来
//! "装配里悄悄塞 IO/锁"无人报警。dev-only 态：不入 perf_thresholds 四表阈值门
//! （避免未校准即红），仅作本地/趋势探针。
//!
//! build() / build_app_server() 内部 `tokio::spawn` Critical 旁路订阅者与分片 worker，
//! 故需 tokio runtime 语境。性能重构 D2 后：每组在每轮 `rt.block_on` 内完成
//! build → 优雅关闭(shutdown_sharding) → drop → yield 驱动 detached 任务退出，
//! 避免旧 `rt.enter()` 不驱动导致的 worker/订阅者跨 iter 累积（旧 OOM 根因）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
use chimera_cli::composition::{build, build_app_server, build_with_sharding};
use chimera_cli::spec::ShardingPolicy;
use chimera_cli::ChimeraConfig;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nexus_contracts::app::{AppEvent, AppOp, ThreadStartParams, UserInput};

// 性能重构 D2 基准准则：`build()` 现会 `tokio::spawn` 64 个分片 worker + 1 个
// Critical 旁路订阅者。二者都是**不随闭包同步结束而退出**的 detached 任务——旧 bench
// 用 `rt.enter()` + 同步 `b.iter` 从不驱动 runtime，导致 worker 与订阅者跨 iter 累积，
// 数百次迭代后 OOM。现改为每轮 `rt.block_on` 内
// `build → shutdown_sharding（优雅排空 worker）→ drop（关闭 Critical 通道）→ 若干 yield
// （驱动订阅者/被 abort 的 worker 退出）`，使每轮起始内存基线一致。
//
// 主基线 `composition_build_off` 对应 one-shot CLI 路径（不 spawn 任何 worker，零残留）；
// 对照基线 `composition_build_auto` / `composition_build_app_server` 对应长驻宿主。

/// one-shot 路径（ShardingPolicy::Off）：无 worker spawn，最贴近 run/exec/quest/… 启动。
fn bench_build_app_context_off(c: &mut Criterion) {
    let cfg = ChimeraConfig::default();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    c.bench_function("composition_build_off", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ctx =
                    build_with_sharding(black_box(&cfg), ShardingPolicy::Off).expect("装配应成功");
                black_box(&ctx.bus);
                ctx.bus.shutdown_sharding().await; // Off：无分片，no-op
                drop(ctx);
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
            });
        });
    });
}

/// 长驻宿主路径（ShardingPolicy::Auto）：spawn 64 worker，每轮优雅关闭避免累积。
fn bench_build_app_context_auto(c: &mut Criterion) {
    let cfg = ChimeraConfig::default();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    c.bench_function("composition_build_auto", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ctx =
                    build_with_sharding(black_box(&cfg), ShardingPolicy::Auto).expect("装配应成功");
                black_box(&ctx.bus);
                ctx.bus.shutdown_sharding().await; // 优雅排空 64 worker 并 await 退出
                drop(ctx);
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
            });
        });
    });
}

/// build_app_server 延迟基线（协议宿主构造点）：同样每轮 block_on 驱动，drop server
/// 后 yield 让 detached 任务退出，避免跨 iter 累积。
fn bench_build_app_server(c: &mut Criterion) {
    let cfg = ChimeraConfig::default();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    c.bench_function("composition_build_app_server", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ctx =
                    build_with_sharding(black_box(&cfg), ShardingPolicy::Auto).expect("装配应成功");
                let server = build_app_server(ctx).await;
                black_box(&server);
                drop(server); // server 持 bus；drop → Critical Sender 关闭 + guard abort worker
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
            });
        });
    });
}

/// C-2：协议面 handle_op 延迟基线（serve/acp 对外承诺"真实核心"，其延迟此前
/// 仅被 InMemory 桩测过）。ThreadStart 建线程一次，随后量 TurnSubmit 往返。
/// 与 build_app_server 同需 runtime → 统一由一个 multi-thread Runtime 驱动。
fn bench_handle_op_turn_submit(c: &mut Criterion) {
    let cfg = ChimeraConfig::default();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(async { build_app_server(build(&cfg).expect("build")).await });
    let thread_id = rt
        .block_on(server.handle_op(&AppOp::ThreadStart(ThreadStartParams::new("g", "r"))))
        .expect("ThreadStart")
        .into_iter()
        .find_map(|ev| match ev {
            AppEvent::ThreadStarted { thread } => Some(thread.thread_id),
            _ => None,
        })
        .expect("ThreadStarted 事件");
    let mut seq = 0u64;
    c.bench_function("composition_handle_op_turn_submit", |b| {
        b.iter(|| {
            seq += 1;
            let evs = rt
                .block_on(server.handle_op(&AppOp::TurnSubmit {
                    thread_id: thread_id.clone(),
                    input: UserInput {
                        text: format!("q{seq}").into(),
                        extras: None,
                    },
                }))
                .expect("TurnSubmit");
            black_box(evs.len())
        })
    });
}

criterion_group!(
    benches,
    bench_build_app_context_off,
    bench_build_app_context_auto,
    bench_build_app_server,
    bench_handle_op_turn_submit
);
criterion_main!(benches);
