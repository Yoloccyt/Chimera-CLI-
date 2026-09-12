//! ShardedBus 分片吞吐门禁基准 — P1-T12(Phase 1 地基波次,手册 §8.5)
//!
//! 对应任务:P1-T12(Phase 1 地基波次)
//! 架构层:L1 Core(event-bus)
//!
//! # 门禁口径(手册 §8.5 / T12)
//! - 分片发布吞吐(`enable_sharding(64)` 后 `publish_blocking` 入片路径)
//!   **> 500K msg/s** —— 分片是「发布端并行化」:主线程只做入片(信用 CAS +
//!   Mutex + ArrayQueue push + Notify),broadcast send 由后台 worker 承担,
//!   发布端吞吐不再被 broadcast 复制/接收者数拖累;
//! - 对照:单流发布吞吐(`EventBus::new()` 默认关分片,与 v2.27.1 完全一致),
//!   报告分片相对单流的加速比(对照基准,非门禁)。
//!
//! # 门禁说明(诚实数据红线)
//! 复用 T8/T9 单行采样模式:iter_custom 内零打印(只计时),测量阶段结束后
//! 固定 n 单次采样,仅打印一次 P50/P99(msg/s)。基准**不做断言** —— 阈值
//! 判定在 CI 解析报告时进行(防快速模式采样抖动误报)。
//!
//! # worker 运行环境
//! `enable_sharding` 需要 tokio runtime 上下文:本基准创建 current_thread
//! runtime 并移入独立线程常驻 `block_on(pending)`,驱动 64 个 shard_worker
//! (攒批 64 汇入 broadcast + 批归还信用)。测量线程不参与 runtime 调度。
//!
//! # 慢消费者防 warn 洪泛
//! 分片回退事件(shed)与单流路径都经过 broadcast send + lag 检测,需一个
//! 后台消费线程持续 `try_recv` 排空广播缓冲,避免 warn! 日志洪泛失真。
//!
//! # 测量有效性检查
//! 分片基准结束后打印 `shadow_stats`(sharded/merged/shed):若 shed 大量,
//! 说明 worker 追不上发布端,测量退化为 broadcast 回退路径(需检查配置)。

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::{EventBus, EventMetadata, NexusEvent};

/// 门禁采样事件数 — 固定口径(500K msg/s 门禁的量级锚点)
const GATE_MSG: u64 = 100_000;
/// 门禁采样次数 — 单次采样排序取 P50/P99(固定 n 单次采样模式)
const GATE_SAMPLES: usize = 32;
/// 分片数(与 DEFAULT_SHARD_COUNT 一致,64 = 2^6)
const SHARDS: usize = 64;

/// 构造用于基准的 NexusEvent(QuestCreated 为最常见的高频 Unordered 事件)
///
/// WHY 每次 publish 构造新事件:EventMetadata 含 UUIDv7 event_id,与真实
/// 发布路径一致(事件生命周期内 event_id 唯一)。
fn make_event() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("bench-source"),
        quest_id: "q-bench".into(),
        title: "shard bench event".into(),
        task_count: 1,
    }
}

/// 启动后台消费线程 — 持续排空指定 bus 的广播缓冲
///
/// WHY 需要消费线程:发布端 lag 检测(`Ok(receivers) < expected`)在订阅者
/// 不消费导致缓冲溢出时每次发送都触发 warn!(100K 次发送 = 100K 条 warn,
/// 既失真又拖慢测量)。消费线程在独立核上 try_recv 排空,不进入被计时的
/// 主线程路径(测量诚实)。
fn spawn_drain_consumer(bus: &EventBus, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    let mut rx = bus.subscribe();
    std::thread::spawn(move || {
        // 消费线程循环排空;Lagged 错误被吞掉(预期内,不影响测量)
        while !stop.load(Ordering::Relaxed) {
            let _ = rx.try_recv();
        }
    })
}

// ADR-159 决策 3 三态登记:dev-only(历史副本,新 bench 请用 nexus_contracts::util::percentile_sorted)
/// 计算吞吐量的 P50/P99(排序后分位,NaN 防御降级为 Equal 序)
fn percentile_throughput(samples: &mut [f64]) -> (f64, f64) {
    samples.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() as f64 * 0.99) as usize - 1];
    (p50, p99)
}

/// 固定 n 单次采样(门禁口径,T8/T9 模式) — 预热后采样 GATE_SAMPLES 次
///
/// 返回 (P50, P99) msg/s。
fn gate_samples<F>(publish: F) -> (f64, f64)
where
    F: Fn(),
{
    // 预热:分配器与分支预测器就绪后采样更接近稳态
    for _ in 0..512 {
        publish();
    }
    let mut msg_per_sec = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        let start = Instant::now();
        for _ in 0..GATE_MSG {
            publish();
        }
        let secs = start.elapsed().as_secs_f64();
        msg_per_sec.push(GATE_MSG as f64 / secs);
    }
    percentile_throughput(&mut msg_per_sec)
}

// ============================================================
// 基准 1:单流发布吞吐(对照;EventBus::new() 灰度默认关,零回归口径)
// ============================================================

fn single_flow_publish_throughput(c: &mut Criterion) {
    let bus = EventBus::new();
    let stop = Arc::new(AtomicBool::new(false));
    let consumer = spawn_drain_consumer(&bus, Arc::clone(&stop));

    c.bench_function("shard_single_flow_publish", |b| {
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

    let (p50, p99) = gate_samples(|| {
        criterion::black_box(bus.publish_blocking(make_event())).ok();
    });
    eprintln!(
        "[bus_shard] single_flow_publish n={} p50={:.0} msg/s p99={:.0} msg/s (对照基准)",
        GATE_MSG, p50, p99
    );

    stop.store(true, Ordering::Relaxed);
    let _ = consumer.join();
}

// ============================================================
// 基准 2:分片发布吞吐(门禁 > 500K msg/s)
// ============================================================

fn sharded_flow_publish_throughput(c: &mut Criterion) {
    // current_thread runtime 移入独立线程常驻驱动 worker(测量线程不调度)
    // WHY 独立线程:block_on(pending) 永久驱动 64 个 shard_worker,与测量
    // 线程物理隔离(测量为发布端视角,worker 汇入开销不进入被计时路径)。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime 构建失败");
    let bus = EventBus::new();
    {
        // rt.enter() 提供 runtime 上下文,enable_sharding 才能 spawn worker
        let _guard = rt.enter();
        bus.enable_sharding(SHARDS).expect("bench 启用分片失败");
    }
    let _rt_thread = std::thread::spawn(move || {
        // 常驻驱动 worker 任务(进程结束自然终止,bench 生命周期内持续运行)
        rt.block_on(std::future::pending::<()>());
    });
    let stop = Arc::new(AtomicBool::new(false));
    let consumer = spawn_drain_consumer(&bus, Arc::clone(&stop));

    c.bench_function("shard_sharded_flow_publish", |b| {
        b.iter_custom(|iters| {
            let n = iters;
            let start = Instant::now();
            for _ in 0..n {
                criterion::black_box(bus.publish_blocking(make_event())).ok();
            }
            start.elapsed()
        });
    });

    let (p50, p99) = gate_samples(|| {
        criterion::black_box(bus.publish_blocking(make_event())).ok();
    });
    // 测量有效性检查:sharded 大量低于发布数 / shed 大量 → worker 追不上,
    // 测量退化为 broadcast 回退路径(需排查);正常场景 sharded == merged。
    let stats = bus.shadow_stats();
    eprintln!(
        "[bus_shard] sharded_flow_publish n={} p50={:.0} msg/s p99={:.0} msg/s (门禁: >500K msg/s) | shadow: sharded={} merged={} shed={}",
        GATE_MSG, p50, p99, stats.sharded_total, stats.merged_total, stats.shed_total
    );

    stop.store(true, Ordering::Relaxed);
    let _ = consumer.join();
}

criterion_group!(
    benches,
    single_flow_publish_throughput,
    sharded_flow_publish_throughput,
);
criterion_main!(benches);
