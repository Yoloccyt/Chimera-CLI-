//! C12 组合根装配延迟基准（dev-only，登记于 bench_inventory_freeze.txt）。
//!
//! WHY: 组合根（build / build_app_server）位于每条命令启动路径上；无基线则未来
//! "装配里悄悄塞 IO/锁"无人报警。dev-only 态：不入 perf_thresholds 四表阈值门
//! （避免未校准即红），仅作本地/趋势探针。
//!
//! build_app_server 内部 `tokio::spawn` Critical 旁路订阅者，故需 tokio runtime
//! 上下文 —— 用 `rt.enter()` 守卫令当前线程具备 runtime 语境。

use chimera_cli::composition::{build, build_app_server};
use chimera_cli::ChimeraConfig;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nexus_contracts::app::{AppEvent, AppOp, ThreadStartParams, UserInput};

fn bench_build_app_context(c: &mut Criterion) {
    let cfg = ChimeraConfig::default();
    c.bench_function("composition_build_app_context", |b| {
        b.iter(|| {
            let ctx = build(black_box(&cfg)).expect("装配应成功");
            black_box(&ctx.bus);
        })
    });
}

fn bench_build_app_server(c: &mut Criterion) {
    let cfg = ChimeraConfig::default();
    // build_app_server 会 tokio::spawn 后台订阅者；current_thread runtime + enter
    // 提供同步 spawn 所需的 runtime 语境（bench 进程结束后 detached 任务随进程释放）。
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("current-thread runtime");
    let _guard = rt.enter();
    c.bench_function("composition_build_app_server", |b| {
        b.iter(|| {
            let ctx = build(&cfg).expect("装配应成功");
            let server = build_app_server(ctx);
            black_box(&server);
        })
    });
}

/// C-2：协议面 handle_op 延迟基线（serve/acp 对外承诺"真实核心"，其延迟此前
/// 仅被 InMemory 桩测过）。ThreadStart 建线程一次，随后量 TurnSubmit 往返。
/// 与 build_app_server 同需 runtime → 统一由一个 multi-thread Runtime 驱动。
fn bench_handle_op_turn_submit(c: &mut Criterion) {
    let cfg = ChimeraConfig::default();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(async { build_app_server(build(&cfg).expect("build")) });
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
    bench_build_app_context,
    bench_build_app_server,
    bench_handle_op_turn_submit
);
criterion_main!(benches);
