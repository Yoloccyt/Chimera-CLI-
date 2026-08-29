//! CBF 信用流背压门禁基准 — P1-T11(Phase 1 地基波次,手册 §8.5 / T-06)
//!
//! 对应任务:P1-T11(Phase 1 地基波次)
//! 架构层:L1 Core(event-bus)
//!
//! # 门禁口径(手册 §8.5 / T-06)
//! - 单流吞吐:纯 broadcast 发布(`publish_blocking`,含 P1-T11 信用流
//!   try_acquire 接入后的完整热路径)> 100K msg/s;
//! - 信用流原语吞吐:acquire/release 配对(无锁 CAS 路径),供 T12 分片
//!   改造后对比单片 vs 分片吞吐基准。
//!
//! # 门禁说明(诚实数据红线)
//! 复用 T8/T9 单行采样模式:iter_custom 内零打印(只计时),测量阶段结束后
//! 固定 n 单次采样,仅打印一次 P50/P99(msg/s 或 op/s)。基准**不做断言**
//! —— 阈值判定在 CI 解析报告时进行(防快速模式采样抖动误报)。
//!
//! # 慢消费者防 warn 洪泛
//! 单流吞吐测量期间,一个后台消费线程持续 `try_recv` 排空广播缓冲,
//! 避免生产端 lag 检测(`Ok(receivers) < expected`)每次发送都触发 warn!
//! 日志(100K 次发送 = 100K 条 warn,既失真又拖慢测量)。
//! 消费线程在另一核上运行,不进入被计时的主线程路径(测量诚实)。

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::{CreditFlow, EventBus, EventMetadata, NexusEvent};

/// 门禁采样事件数 — 固定口径(100K msg/s 门禁的量级锚点)
const GATE_MSG: u64 = 100_000;
/// 门禁采样次数 — 单次采样排序取 P50/P99(固定 n 单次采样模式)
const GATE_SAMPLES: usize = 32;

/// 构造用于基准的 NexusEvent(QuestCreated 是最常见的高频事件)
///
/// WHY 每次 publish 构造新事件:EventMetadata 含 UUIDv7 event_id,与真实
/// 发布路径一致(事件生命周期内 event_id 唯一)。
fn make_event() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("bench-source"),
        quest_id: "q-bench".into(),
        title: "credit bench event".into(),
        task_count: 1,
    }
}

/// 启动后台消费线程 — 持续排空指定 bus 的广播缓冲
///
/// WHY 需要消费线程:单流吞吐测量以"保持一个活跃订阅者"为诚实口径
/// (零订阅者时 broadcast send 直接 Err 快速返回,非真实投递成本);
/// 但订阅者不消费会导致缓冲溢出 → 生产端 lag 检测 warn! 洪泛。
/// 消费线程在独立核上 try_recv 排空,不进入被计时的主线程路径。
fn spawn_drain_consumer(bus: &EventBus, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    let mut rx = bus.subscribe();
    std::thread::spawn(move || {
        // 消费线程循环排空;Lagged 错误被吞掉(预期内,不影响测量)
        while !stop.load(Ordering::Relaxed) {
            let _ = rx.try_recv();
        }
    })
}

/// 计算吞吐量的 P50/P99(排序后分位,NaN 防御降级为 Equal 序)
fn percentile_throughput(samples: &mut [f64]) -> (f64, f64) {
    samples.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() as f64 * 0.99) as usize - 1];
    (p50, p99)
}

// ============================================================
// 基准 1:单流发布吞吐(纯 broadcast,门禁 > 100K msg/s)
// ============================================================

fn single_flow_publish_throughput(c: &mut Criterion) {
    let bus = EventBus::new();
    // 消费线程保持缓冲不溢出,避免 lag 检测 warn! 洪泛(见模块文档)
    let stop = Arc::new(AtomicBool::new(false));
    let consumer = spawn_drain_consumer(&bus, Arc::clone(&stop));

    c.bench_function("single_flow_publish", |b| {
        // iter_custom 约定:返回本批迭代总耗时(criterion 用其计算均值)
        b.iter_custom(|iters| {
            let n = iters;
            let start = Instant::now();
            for _ in 0..n {
                criterion::black_box(bus.publish_blocking(make_event())).ok();
            }
            start.elapsed()
        });
    });

    // 门禁采样:固定 GATE_MSG × GATE_SAMPLES,测量结束后仅打印一次 P50/P99
    // 预热:分配器与分支预测器就绪后采样更接近稳态
    for _ in 0..512 {
        criterion::black_box(bus.publish_blocking(make_event())).ok();
    }
    let mut msg_per_sec = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        let start = Instant::now();
        for _ in 0..GATE_MSG {
            criterion::black_box(bus.publish_blocking(make_event())).ok();
        }
        let secs = start.elapsed().as_secs_f64();
        msg_per_sec.push(GATE_MSG as f64 / secs);
    }
    let (p50, p99) = percentile_throughput(&mut msg_per_sec);
    eprintln!(
        "[bus_credit] single_flow_publish n={} p50={:.0} msg/s p99={:.0} msg/s (门禁: >100K msg/s)",
        GATE_MSG, p50, p99
    );

    stop.store(true, Ordering::Relaxed);
    let _ = consumer.join();
}

// ============================================================
// 基准 2:信用流 acquire/release 配对吞吐(无锁 CAS 路径)
// ============================================================

fn credit_flow_throughput(c: &mut Criterion) {
    // 默认 256 信用池:配对测量中 release 即时归还,池不枯竭,
    // 全程走 CAS 成功路径(信用守恒)
    let cf = CreditFlow::new();

    c.bench_function("credit_flow_acquire_release_pair", |b| {
        // WHY 条件归还而非无条件 release:保证每次迭代池不枯竭,
        // 测量恒为 CAS 成功路径(信用不足路径由单测覆盖,非基准口径)
        b.iter(|| {
            if criterion::black_box(cf.acquire(1)) {
                cf.release(1);
                criterion::black_box(());
            }
        });
    });

    // 门禁采样:固定 n 单次采样,打印 P50/P99(op/s)
    for _ in 0..1024 {
        if cf.acquire(1) {
            cf.release(1);
        }
    }
    let mut op_per_sec = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        let start = Instant::now();
        for _ in 0..GATE_MSG {
            if criterion::black_box(cf.acquire(1)) {
                cf.release(1);
                criterion::black_box(());
            }
        }
        let secs = start.elapsed().as_secs_f64();
        op_per_sec.push(GATE_MSG as f64 / secs);
    }
    let (p50, p99) = percentile_throughput(&mut op_per_sec);
    eprintln!(
        "[bus_credit] credit_flow_acquire_release_pair n={} p50={:.0} op/s p99={:.0} op/s",
        GATE_MSG, p50, p99
    );
}

criterion_group!(
    benches,
    single_flow_publish_throughput,
    credit_flow_throughput,
);
criterion_main!(benches);
