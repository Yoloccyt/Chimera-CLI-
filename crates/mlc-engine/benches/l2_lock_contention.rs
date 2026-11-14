//! L2 读锁竞争基准 —— R12 W2-2:量出"读锁持有期间写者被阻塞"到底有多严重
//!
//! # 测什么
//!
//! `crates/mlc-engine/src/l2_semantic.rs:544` 的 `recall_by_clv` 在 `self.inner.read()`
//! 读锁内完成全部余弦计算(`recall_exact`,同步、全程无 `.await`)。ADR-297 §3 已驳回
//! "违反 `agents.md §4.4` 反模式 1"的定性(该反模式管跨 `.await` 持锁),但登记为开放项:
//! 读锁持有期间 `insert`/`remove` 被阻塞,严重性未知,判据是"写者获取写锁的 p95 是否随
//! 读者数显著劣化"。
//!
//! 本基准测**写者拿写锁的墙钟延迟分布**(p50/p95/p99/max)随读者数
//! `c ∈ {0,1,2,4,8}` 的变化,并同时记录读者侧吞吐、单次召回墙钟、以及"未受扰召回"的
//! 基线时长(H)作对照。
//!
//! # 关键设计取舍(为什么是这个形状)
//!
//! 1. **10k 条目 × 512 维、top_k=10、关闭近似层**(`with_hnsw_threshold(_, 0)`):
//!    被测对象就是"读锁内做全量余弦"这条路径。若开近似层,10k ≥ 阈值 4096 会走 HNSW,
//!    而本基准的写操作会持续给图打墓碑 ⇒ 中途 `stale` ⇒ 召回路径在写锁内重建图
//!    (ADR-192 §4:10k 档 3.56s),会把"读锁竞争"测成"建图暂停"。近似层未测,见报告
//!    "未决问题"(不推测数值)。
//! 2. **写操作 = `insert` 已存在 id**(稳态更新路径,条目数恒为 10k):
//!    若每轮都插新 id,条目数会随迭代数无界增长(每条 CLV 驻留 2KB),既撑爆内存
//!    又会在容量边界引入驱逐噪声(`evict_oldest_locked`)。
//!    注意:更新路径的写临界区是 `O(n)` 的 `retain` + 512 维内容哈希,**比新增更大**,
//!    故以它为分母算出的劣化倍数是**保守下界**(分母偏大 ⇒ 倍数偏小)。
//! 3. **criterion 用 `iter_custom` 而不是 `b.iter`**:
//!    - `b.iter(|| 一次写)`:criterion 测的是整个闭包(含每轮 `MemoryEntry` 深拷贝模板、
//!      含把样本推进 Vec 的记账),这些开销会进 criterion 报的"每次迭代耗时",
//!      把写者延迟虚高(尤其 c=0 档,记账占比可达 10%~50%)。
//!    - `iter_custom` 让我**只把写调用的墙钟时长计入返回值**,模板克隆与采样记账落在
//!      计时窗之外 ⇒ criterion 报的数与我自己算的分位数同源、可互相印证。
//!    - 线程仍在 `bench_function` 之外建好(每档一次),`iter_custom` 内层 = 一次写。
//! 4. **固定采样协议 `SamplingMode::Flat` + `sample_size(1000)`**(每档 1000 次写):
//!    Auto 模式会按**预热期均值**反推每样本迭代数,而本基准的分布是**重尾**的
//!    (首轮实测:c=1 档一次 903ms 的病态样本把估计值抬到 45s,该档最后只收到 51 个样本,
//!    c=0 档却有 167917 个 ⇒ 各档样本数差 4 个数量级,分位数不可横向比)。
//!    Flat 模式把"每档样本数"钉成常量,横向可比性是结构性的,不靠事后解释。
//! 5. **不 sleep**:Windows 定时器粒度约 15.6ms,`sleep` 会被吞成一跳,把负载量化成阶跃。
//!    读者用真实召回(10k×512 余弦)产生负载;唯一的时间窗(纯读吞吐探针)用
//!    `Instant` 忙等截止,也不用 sleep。
//!
//! # 阳性对照(必须有,否则结论不可信)
//!
//! `assert_positive_control` 断言三件事:①c=8 档读者真的跑过(召回次数 > 0);
//! ②`p95(c=8) / p95(c=0) ≥ MIN_CONTENTION_RATIO`;③c=0 档写者几乎从不被阻塞
//! (`frac_over_1ms < 0.02`,否则说明机器有外部噪声,读数不可用)。
//! 另设两个**混杂对照**:
//! - `cpu_only_c8`:8 个读者打**另一个同规模实例**(同代码、同 CPU 负载、不同锁)
//!   ⇒ 若它也出现同等劣化,c=8 的劣化就不能归因于锁;
//! - `lock_c1_repeat`:同一 c=1 档在**所有档之后**重跑一次 ⇒ 用来区分"c=1 的读数"
//!   是 c 的函数,还是"进程内第一次让写者睡在 futex 上"的一次性冷启动代价。
//!
//! # 这不是什么
//!
//! - 不是生产 SLO 取数:`cargo bench` 的 bench profile 继承 `[profile.release]` 的
//!   `opt-level="z"`(见根 `Cargo.toml`),绝对值不可与 release 二进制或其它机器比较;
//!   本轮**只比同轮同档的相对关系**。
//! - 不改生产代码:本波只取证。新增 bench 文件 + `Cargo.toml` 的 `[[bench]]` 注册是唯一改动。

#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use criterion::measurement::WallTime;
use criterion::{criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, SamplingMode};
use mlc_engine::{MemoryEntry, MemoryId, MemoryTier, SemanticMemory, SharedCLV};
use nexus_core::CLV;

/// 条目规模(题面固定 10k;≈ 10k × 512 × 4B = 20MB 向量驻留)
const ENTRIES: usize = 10_000;
/// 召回 top_k:题面固定 10(`recall_exact` 只对 Top-K 做 MemoryId clone)
const TOP_K: usize = 10;
/// 读者数档位(题面固定)
const READER_COUNTS: [usize; 5] = [0, 1, 2, 4, 8];
/// 每档固定写样本数(Flat 采样 ⇒ 恰为 1000 次写)
///
/// WHY 1000:足以稳定 p50/p95(p95 落在第 50 个样本),p99 只有 ~10 个样本支撑,
/// 故报告里 p99 只作尾部量级指示,不作判据。
const SAMPLES_PER_ARM: usize = 1000;
/// 每档保留的原始延迟样本上限(u32 纳秒 ⇒ 4.29s 满量程)
const SAMPLE_CAP: usize = 2_000_000;
/// 阳性对照下限:p95(c=8) / p95(c=0) 至少 3×
///
/// WHY 3× 而不是 2× 或 10×:这是**探针可见性**的下限,不是"值得改锁"的门槛。
/// 若锁竞争真实存在,c=8 的写者 p95 至少要等一个读者的持锁时长(ms 量级),
/// 而 c=0 的写者 p95 是它自己的临界区(几十 µs 量级),量级差本应 ≥ 10×;
/// 取 3× 只留出"探针失效之外"的单次调度抖动余量。
const MIN_CONTENTION_RATIO: f64 = 3.0;
/// c=0 档"写者被阻塞"的容忍上限(超过即认为机器有外部噪声)
const MAX_UNCONTENDED_BLOCK_FRAC: f64 = 0.02;
/// "被阻塞"的判定阈:1ms(写者自身临界区是几十 µs 量级,拉开一个数量级)
const BLOCKED_THRESHOLD_NS: u32 = 1_000_000;
/// 纯读吞吐探针的窗口(忙等截止,不用 sleep)
const READONLY_WINDOW: Duration = Duration::from_millis(600);
/// 未受扰召回基线的采样次数
const BASELINE_RECALLS: usize = 30;
/// 读者就绪等待上限(只等"每个读者至少完成 1 次召回",正常 10ms 量级)
const READER_READY_TIMEOUT: Duration = Duration::from_secs(60);

// ============================================================
// 语料:确定性 LCG(与 benches/hnsw_tiered.rs、tests/hnsw_tiered.rs 同形态)
// ============================================================

fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// [-1.0, 1.0) 均匀
fn lcg_f32(state: &mut u64) -> f32 {
    let raw = lcg_next(state) as f64;
    (raw / (1u64 << 31) as f64) as f32 - 1.0
}

fn clv_from(rng: &mut u64) -> CLV {
    let mut v = Vec::with_capacity(CLV::DIMENSION);
    for _ in 0..CLV::DIMENSION {
        v.push(lcg_f32(rng));
    }
    CLV::from_vec(v).expect("CLV 构造应成功(512 维非零)")
}

/// 已填充 10k 条目的实例,**关闭近似层**(阈值 0 ⇒ 恒走 `recall_exact`)
///
/// WHY 阈值 0:见文件头 §设计取舍 1。容量给 +16 余量,保证只走更新路径时永不触发
/// `evict_oldest_locked`(否则写延迟里会混入驱逐与池清理)。
fn build_filled(entries: usize) -> SemanticMemory {
    let mem = SemanticMemory::with_hnsw_threshold(entries + 16, 0);
    let mut rng: u64 = 0x5eed_0012_0002_0f11;
    for i in 0..entries {
        let clv = clv_from(&mut rng);
        let entry = MemoryEntry::new(
            format!("m-{i}"),
            format!("content-{i}"),
            MemoryTier::L2Semantic,
        )
        .with_clv(clv);
        let _evicted = mem.insert(entry).expect("填充应成功");
    }
    mem
}

/// 查询向量:独立种子(不与任何条目同向),全体读者共用同一 query
fn query_clv() -> CLV {
    let mut rng: u64 = 0x0a11_ce00_0000_0b01;
    clv_from(&mut rng)
}

/// 写者模板 = 已存在条目 `m-0` 的原样副本
///
/// WHY 已存在 id:见文件头 §设计取舍 2(条目数恒定、无驱逐)。
/// WHY 保留同内容 CLV:更新路径 `SharedCLV::intern` 会命中 `clv_pool`(池中该内容仍在),
/// 故每轮**不新分配 2KB**,写临界区不受池增长干扰。
fn writer_template(mem: &SemanticMemory) -> MemoryEntry {
    let entry = mem.get("m-0").expect("模板条目 m-0 应存在");
    assert!(entry.clv.is_some(), "L2 条目必须携带 CLV");
    entry
}

// ============================================================
// 读者:持续循环调用 recall_by_clv(占空比 ≈ 100% 的最坏情况)
// ============================================================

/// 单个读者线程的自记账(热路上只做 u64 累加,不用原子操作)
#[derive(Debug, Default, Clone, Copy)]
struct ReaderStat {
    calls: u64,
    sum_ns: u64,
    max_ns: u64,
}

impl ReaderStat {
    fn merge(&mut self, other: Self) {
        self.calls += other.calls;
        self.sum_ns += other.sum_ns;
        self.max_ns = self.max_ns.max(other.max_ns);
    }

    fn mean_us(&self) -> f64 {
        if self.calls == 0 {
            return 0.0;
        }
        (self.sum_ns as f64 / self.calls as f64) / 1_000.0
    }
}

struct Readers {
    stop: Arc<AtomicBool>,
    /// 已完成至少 1 次召回的读者数(用于确认"读者真的在跑")
    ready: Arc<AtomicUsize>,
    handles: Vec<thread::JoinHandle<ReaderStat>>,
}

fn spawn_readers(target: Arc<SemanticMemory>, count: usize, query: CLV) -> Readers {
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::with_capacity(count);
    for _ in 0..count {
        let target = Arc::clone(&target);
        let stop = Arc::clone(&stop);
        let ready = Arc::clone(&ready);
        let query = query.clone();
        handles.push(thread::spawn(move || {
            let mut stat = ReaderStat::default();
            let mut announced = false;
            while !stop.load(Ordering::Relaxed) {
                // 真实负载:10k × 512 维余弦 + Top-K 选择(库内整段在读锁内完成)
                let t0 = Instant::now();
                let hits = target.recall_by_clv(&query, TOP_K).unwrap_or_default();
                let dt = t0.elapsed();
                black_box(hits.len());
                stat.calls += 1;
                stat.sum_ns += dt.as_nanos().min(u64::MAX as u128) as u64;
                stat.max_ns = stat.max_ns.max(dt.as_nanos().min(u64::MAX as u128) as u64);
                if !announced {
                    ready.fetch_add(1, Ordering::Release);
                    announced = true;
                }
            }
            stat
        }));
    }
    Readers {
        stop,
        ready,
        handles,
    }
}

/// 等到每个读者都完成过至少 1 次召回(否则 c>0 档可能测的是"没有读者的 c>0")
fn wait_until_ready(rs: &Readers, count: usize) {
    let deadline = Instant::now() + READER_READY_TIMEOUT;
    while rs.ready.load(Ordering::Acquire) < count {
        assert!(
            Instant::now() < deadline,
            "读者未在 {READER_READY_TIMEOUT:?} 内完成首次召回:就绪 {}/{count}",
            rs.ready.load(Ordering::Acquire)
        );
        std::hint::spin_loop();
    }
}

/// 停读者并汇总其自记账
fn stop_readers(rs: Readers) -> ReaderStat {
    let Readers {
        stop, handles, ..
    } = rs;
    stop.store(true, Ordering::Release);
    let mut total = ReaderStat::default();
    for h in handles {
        total.merge(h.join().expect("读者线程不应 panic"));
    }
    total
}

// ============================================================
// 延迟样本 → 分位数
// ============================================================

/// 纳秒样本(饱和到 u32::MAX,避免 4.29s 以上样本回绕)
fn to_nanos_u32(d: Duration) -> u32 {
    d.as_nanos().min(u32::MAX as u128) as u32
}

/// 最近秩法(nearest-rank):第 `ceil(p·n)` 个升序样本,`p ∈ (0,1]`
fn nearest_rank(sorted: &[u32], p: f64) -> u32 {
    let n = sorted.len();
    if n == 0 {
        return 0;
    }
    let rank = ((p * n as f64).ceil() as usize).max(1).min(n);
    sorted[rank - 1]
}

#[derive(Debug, Clone, Copy)]
struct LatStats {
    samples: usize,
    mean_us: f64,
    p50_us: f64,
    p90_us: f64,
    p95_us: f64,
    p99_us: f64,
    max_us: f64,
    /// 超过 1ms 的样本占比(把"尾部"翻译成"撞上读者的概率",便于报告判读)
    frac_over_1ms: f64,
    /// 超过 100ms 的病态样本数与首次出现的样本序号(用于区分冷启动与稳态)
    over_100ms: usize,
    first_over_100ms_idx: Option<usize>,
}

fn summarize(mut v: Vec<u32>) -> LatStats {
    if v.is_empty() {
        return LatStats {
            samples: 0,
            mean_us: 0.0,
            p50_us: 0.0,
            p90_us: 0.0,
            p95_us: 0.0,
            p99_us: 0.0,
            max_us: 0.0,
            frac_over_1ms: 0.0,
            over_100ms: 0,
            first_over_100ms_idx: None,
        };
    }
    let n = v.len();
    let sum: u64 = v.iter().map(|&x| u64::from(x)).sum();
    let over_1ms = v.iter().filter(|&&x| x > BLOCKED_THRESHOLD_NS).count();
    let first_over_100ms_idx = v.iter().position(|&x| x > 100_000_000);
    let over_100ms = v.iter().filter(|&&x| x > 100_000_000).count();
    v.sort_unstable();
    LatStats {
        samples: n,
        mean_us: (sum as f64 / n as f64) / 1_000.0,
        p50_us: f64::from(nearest_rank(&v, 0.50)) / 1_000.0,
        p90_us: f64::from(nearest_rank(&v, 0.90)) / 1_000.0,
        p95_us: f64::from(nearest_rank(&v, 0.95)) / 1_000.0,
        p99_us: f64::from(nearest_rank(&v, 0.99)) / 1_000.0,
        max_us: f64::from(v[n - 1]) / 1_000.0,
        frac_over_1ms: over_1ms as f64 / n as f64,
        over_100ms,
        first_over_100ms_idx,
    }
}

// ============================================================
// 探针:未受扰召回的持锁时长 H + 纯读吞吐(无写者)
// ============================================================

#[derive(Debug, Clone, Copy)]
struct ReadProbe {
    /// 单线程、无读者无写者:一次 `recall_by_clv` 的墙钟(p50/p95),即"读锁持有时间"的基线 H
    hold_p50_us: f64,
    hold_p95_us: f64,
    hold_max_us: f64,
    /// 纯读(无写者)下 c=1 / c=8 的聚合吞吐
    ro_ops_c1: f64,
    ro_ops_c8: f64,
    /// 纯读下每读者单次召回墙钟均值
    ro_call_mean_c1_us: f64,
    ro_call_mean_c8_us: f64,
}

/// 纯读吞吐:起 `readers` 个线程打到 `Instant` 截止,返回 (聚合 ops/s, 读者自记账)
fn measure_readonly(
    mem: &Arc<SemanticMemory>,
    readers: usize,
    query: &CLV,
    window: Duration,
) -> (f64, ReaderStat) {
    let deadline = Instant::now() + window;
    let mut handles = Vec::with_capacity(readers);
    for _ in 0..readers {
        let mem = Arc::clone(mem);
        let query = query.clone();
        handles.push(thread::spawn(move || {
            let mut stat = ReaderStat::default();
            while Instant::now() < deadline {
                let t0 = Instant::now();
                let hits = mem.recall_by_clv(&query, TOP_K).unwrap_or_default();
                let dt = t0.elapsed();
                black_box(hits.len());
                stat.calls += 1;
                stat.sum_ns += dt.as_nanos().min(u64::MAX as u128) as u64;
                stat.max_ns = stat.max_ns.max(dt.as_nanos().min(u64::MAX as u128) as u64);
            }
            stat
        }));
    }
    let mut total = ReaderStat::default();
    for h in handles {
        total.merge(h.join().expect("纯读探针线程不应 panic"));
    }
    (total.calls as f64 / window.as_secs_f64(), total)
}

fn probe_uncontended_reads() -> ReadProbe {
    let mem = Arc::new(build_filled(ENTRIES));
    let query = query_clv();

    // H:单线程、无并发,一次召回的墙钟(≈ 读锁持有时间)
    let mut hold = Vec::with_capacity(BASELINE_RECALLS);
    for _ in 0..BASELINE_RECALLS {
        let t0 = Instant::now();
        let hits = mem.recall_by_clv(&query, TOP_K).expect("基线召回应成功");
        hold.push(to_nanos_u32(t0.elapsed()));
        black_box(hits.len());
    }
    let hold_stats = summarize(hold);

    let (ro_c1, st1) = measure_readonly(&mem, 1, &query, READONLY_WINDOW);
    let (ro_c8, st8) = measure_readonly(&mem, 8, &query, READONLY_WINDOW);

    let probe = ReadProbe {
        hold_p50_us: hold_stats.p50_us,
        hold_p95_us: hold_stats.p95_us,
        hold_max_us: hold_stats.max_us,
        ro_ops_c1: ro_c1,
        ro_ops_c8: ro_c8,
        ro_call_mean_c1_us: st1.mean_us(),
        ro_call_mean_c8_us: st8.mean_us(),
    };
    eprintln!(
        "[l2_lock_contention] probe hold_uncontended_p50_us={:.1} p95_us={:.1} max_us={:.1} \
         | readonly_c1_ops_s={:.0} call_mean_us={:.1} | readonly_c8_ops_s={:.0} call_mean_us={:.1} \
         | 折算占空比 c1={:.2} c8={:.2}",
        probe.hold_p50_us,
        probe.hold_p95_us,
        probe.hold_max_us,
        probe.ro_ops_c1,
        probe.ro_call_mean_c1_us,
        probe.ro_ops_c8,
        probe.ro_call_mean_c8_us,
        probe.ro_ops_c1 * probe.hold_p50_us / 1e6,
        probe.ro_ops_c8 * probe.hold_p50_us / 1e6,
    );
    probe
}

// ============================================================
// 单档:一个读者数 × 一种读者目标
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReaderMode {
    /// 读者打**被测实例**(= 真实竞争:读者持读锁,写者等写锁)
    LockOnMeasured,
    /// 读者打**影子实例**(同代码、同 CPU 负载、不同锁 ⇒ 只留 CPU 争用,无锁排队)
    CpuOnlyShadow,
}

impl ReaderMode {
    /// 档位标签的公共前缀(与 `run_arm` 的 label 参数同源,便于交叉核对)
    fn label_prefix(self) -> &'static str {
        match self {
            Self::LockOnMeasured => "lock",
            Self::CpuOnlyShadow => "cpu_only",
        }
    }
}

struct ArmResult {
    label: String,
    readers: usize,
    mode: ReaderMode,
    write: LatStats,
    reader: ReaderStat,
    window_s: f64,
}

impl ArmResult {
    fn read_ops_per_s(&self) -> f64 {
        self.reader.calls as f64 / self.window_s.max(1e-9)
    }
}

/// 跑一档:建实例 → 起读者(带就绪确认)→ 固定 1000 次写并逐次记账 → 停读者 → 汇总
fn run_arm(
    group: &mut BenchmarkGroup<'_, WallTime>,
    label: &str,
    readers: usize,
    mode: ReaderMode,
) -> ArmResult {
    let mem = Arc::new(build_filled(ENTRIES));
    let template = writer_template(&mem);
    let query = query_clv();

    // cpu_only 档:影子实例同规模同语料形态,读者打它 ⇒ 写者面对的锁是"没人抢"的
    let shadow: Option<Arc<SemanticMemory>> = match mode {
        ReaderMode::LockOnMeasured => None,
        ReaderMode::CpuOnlyShadow => Some(Arc::new(build_filled(ENTRIES))),
    };
    let reader_target = match (&shadow, mode) {
        (Some(s), ReaderMode::CpuOnlyShadow) => Arc::clone(s),
        _ => Arc::clone(&mem),
    };

    let rs = spawn_readers(Arc::clone(&reader_target), readers, query);
    if readers > 0 {
        wait_until_ready(&rs, readers);
    }

    let mut samples: Vec<u32> = Vec::new();
    let errors = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let bench_id = BenchmarkId::new("writer_insert_update", label);
    {
        let mem_w = Arc::clone(&mem);
        let tpl = template.clone();
        let errors = Arc::clone(&errors);
        let samples_mut = &mut samples;
        group.bench_function(bench_id, |b| {
            // 每档只建一次线程(上面已建);内层 = 一次写
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    // 模板深拷贝(≈2KB CLV 分配)在计时窗外:它不是被测对象
                    let entry = tpl.clone();
                    let t0 = Instant::now();
                    let out = mem_w.insert(entry);
                    let dt = t0.elapsed();
                    if out.is_err() {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    total += dt;
                    if samples_mut.len() < SAMPLE_CAP {
                        samples_mut.push(to_nanos_u32(dt));
                    }
                }
                total
            });
        });
    }
    let window = started.elapsed();

    let reader = stop_readers(rs);
    drop(shadow);
    drop(reader_target);

    // 写者路径的健康检查:锁毒化会让 insert 立刻返回 Err(写者"不阻塞"的假读数)
    let write_errors = errors.load(Ordering::Relaxed);
    assert_eq!(
        write_errors, 0,
        "{label}:写者出现 {write_errors} 次 Err(锁毒化?),本档读数无效"
    );
    assert_eq!(
        mem.len().expect("len 应可读"),
        ENTRIES,
        "{label}:条目数应恒为 {ENTRIES}(只做更新,不新增不驱逐)"
    );

    let write = summarize(samples);
    // 标签前缀与模式必须一致(否则表格里"档位名"与"实际读者目标"会错位)
    assert!(
        label.starts_with(mode.label_prefix()),
        "档位标签 {label} 与读者目标模式 {:?} 不一致",
        mode
    );
    let result = ArmResult {
        label: label.to_string(),
        readers,
        mode,
        write,
        reader,
        window_s: window.as_secs_f64(),
    };
    eprintln!(
        "[l2_lock_contention] arm={} readers={} samples={} p50_us={:.1} p90_us={:.1} p95_us={:.1} \
         p99_us={:.1} max_us={:.1} mean_us={:.1} frac_gt_1ms={:.3} over_100ms={} at_idx={:?} \
         reader_calls={} reader_ops_per_s={:.0} reader_call_mean_us={:.1} reader_call_max_us={:.1} \
         window_s={:.2}",
        result.label,
        result.readers,
        result.write.samples,
        result.write.p50_us,
        result.write.p90_us,
        result.write.p95_us,
        result.write.p99_us,
        result.write.max_us,
        result.write.mean_us,
        result.write.frac_over_1ms,
        result.write.over_100ms,
        result.write.first_over_100ms_idx,
        result.reader.calls,
        result.read_ops_per_s(),
        result.reader.mean_us(),
        result.reader.max_ns as f64 / 1_000.0,
        result.window_s,
    );
    result
}

// ============================================================
// 阳性对照
// ============================================================

fn find_arm<'a>(results: &'a [ArmResult], label: &str) -> &'a ArmResult {
    results
        .iter()
        .find(|r| r.label == label)
        .unwrap_or_else(|| panic!("缺少档位 {label}"))
}

/// 断言探针**确实能看见锁竞争**;看不见就红(结论不可信时宁可失败也不出报告)
fn assert_positive_control(results: &[ArmResult]) {
    let c0 = find_arm(results, "lock_c0");
    let c8 = find_arm(results, "lock_c8");

    assert!(
        c8.reader.calls > 0,
        "阳性对照失败:lock_c8 档读者一次召回都没完成(读者根本没在跑),本档数据无意义"
    );
    assert!(
        c0.write.samples >= SAMPLES_PER_ARM / 2 && c8.write.samples >= SAMPLES_PER_ARM / 2,
        "阳性对照失败:样本过少(c0={} c8={}),分位数不可信",
        c0.write.samples,
        c8.write.samples
    );
    assert!(
        c0.write.frac_over_1ms < MAX_UNCONTENDED_BLOCK_FRAC,
        "阳性对照失败:lock_c0 档已有 {:.1}% 的写被阻塞 >1ms(上限 {:.0}%),\
         说明机器有外部噪声,本轮读数不可用",
        c0.write.frac_over_1ms * 100.0,
        MAX_UNCONTENDED_BLOCK_FRAC * 100.0
    );

    let ratio = c8.write.p95_us / c0.write.p95_us.max(f64::EPSILON);
    eprintln!(
        "[l2_lock_contention] 阳性对照:p95(c=8)/p95(c=0) = {:.2}× (阈值 {MIN_CONTENTION_RATIO:.1}×);\
         p50(c=8)/p50(c=0) = {:.2}×",
        ratio,
        c8.write.p50_us / c0.write.p50_us.max(f64::EPSILON)
    );
    assert!(
        ratio >= MIN_CONTENTION_RATIO,
        "阳性对照失败:p95(c=8)/p95(c=0) 仅 {ratio:.2}× (< {MIN_CONTENTION_RATIO:.1}×)。\
         可能原因:写者路径没取写锁 / 读者没持读锁 / 读者被调度饿死。\
         此情形下「未观察到锁竞争」不可信,须先修探针而非下结论"
    );
}

// ============================================================
// 主基准
// ============================================================

fn bench_write_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("l2_lock_contention");
    // 固定采样协议(理由见文件头 §设计取舍 4):每档恰 SAMPLES_PER_ARM 个样本
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(SAMPLES_PER_ARM);
    group.warm_up_time(Duration::from_millis(150));

    // 0) 先取基线:未受扰召回时长 H + 纯读吞吐(无写者)
    let probe = probe_uncontended_reads();

    // 1) 主矩阵:c 升序
    let mut results: Vec<ArmResult> = Vec::new();
    for &readers in &READER_COUNTS {
        results.push(run_arm(
            &mut group,
            &format!("lock_c{readers}"),
            readers,
            ReaderMode::LockOnMeasured,
        ));
    }
    // 2) 顺序/冷启动对照:同一 c=1 在所有档之后重跑
    results.push(run_arm(
        &mut group,
        "lock_c1_repeat",
        1,
        ReaderMode::LockOnMeasured,
    ));
    // 3) 混杂对照:同 8 个读者的 CPU 负载,但读者打的不是被测实例
    results.push(run_arm(
        &mut group,
        "cpu_only_c8",
        8,
        ReaderMode::CpuOnlyShadow,
    ));
    group.finish();

    // ---------- 表格 ----------
    eprintln!("\n[l2_lock_contention] 写者 `insert`(已存在 id)墙钟延迟,单位 µs:");
    eprintln!(
        "| 档位 | 读者目标 | 读者数 | 样本 | p50 | p90 | p95 | p99 | max | >1ms 占比 | >100ms 次数@序号 | 读者 ops/s | 读者单次均值 |"
    );
    eprintln!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for r in &results {
        eprintln!(
            "| {} | {} | {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.3} | {}@{:?} | {:.0} | {:.1} |",
            r.label,
            r.mode.label_prefix(),
            r.readers,
            r.write.samples,
            r.write.p50_us,
            r.write.p90_us,
            r.write.p95_us,
            r.write.p99_us,
            r.write.max_us,
            r.write.frac_over_1ms,
            r.write.over_100ms,
            r.write.first_over_100ms_idx,
            r.read_ops_per_s(),
            r.reader.mean_us(),
        );
    }

    // ---------- Q1 / Q2 的原始量 ----------
    let base95 = find_arm(&results, "lock_c0").write.p95_us.max(f64::EPSILON);
    let base50 = find_arm(&results, "lock_c0").write.p50_us.max(f64::EPSILON);
    eprintln!("\n[l2_lock_contention] 相对 c=0 的写者延迟倍数(p95 / p50):");
    let mut prev95: Option<f64> = None;
    for &c_ in &READER_COUNTS {
        let arm = find_arm(&results, &format!("lock_c{c_}"));
        let r95 = arm.write.p95_us / base95;
        let trend = match prev95 {
            None => "—".to_string(),
            Some(p) if r95 >= p => format!("↑ {:.2}×", r95 / p.max(f64::EPSILON)),
            Some(p) => format!("↓ {:.2}×", r95 / p.max(f64::EPSILON)),
        };
        eprintln!(
            "[l2_lock_contention] c={} p95={:.1}µs 比值={:.2}× p50={:.1}µs 比值={:.2}× 相邻档变化={}",
            c_,
            arm.write.p95_us,
            r95,
            arm.write.p50_us,
            arm.write.p50_us / base50,
            trend
        );
        prev95 = Some(r95);
    }
    // 顺序对照:c=1 首跑 vs 重跑
    let c1 = find_arm(&results, "lock_c1");
    let c1r = find_arm(&results, "lock_c1_repeat");
    let cpu = find_arm(&results, "cpu_only_c8");
    eprintln!(
        "\n[l2_lock_contention] 对照:lock_c1 p50={:.1}µs p95={:.1}µs | lock_c1_repeat p50={:.1}µs \
         p95={:.1}µs | cpu_only_c8 p50={:.1}µs p95={:.1}µs | c=0 p50={:.1}µs p95={:.1}µs",
        c1.write.p50_us,
        c1.write.p95_us,
        c1r.write.p50_us,
        c1r.write.p95_us,
        cpu.write.p50_us,
        cpu.write.p95_us,
        base50,
        base95
    );
    eprintln!(
        "[l2_lock_contention] 探针复核:H(未受扰召回 p50)={:.1}µs;c=8 写者 p50/H = {:.2}×",
        probe.hold_p50_us,
        find_arm(&results, "lock_c8").write.p50_us / probe.hold_p50_us.max(f64::EPSILON)
    );

    assert_positive_control(&results);
}

/// 「锁外快照」方案的代价探针:写者在每次 insert 上要付的整表克隆
///
/// WHY 单独测:ADR-297 §3 提的修法是把 `vectors` 移进 `Arc<[...]>` 做 COW,
/// 代价落在**每次插入**的 `make_mut` 克隆上。判断"值不值得改锁结构"必须把这个
/// **无条件成本**与主表测到的**间歇性收益**放在同一张表里比。
///
/// 口径说明:
/// - 克隆的元素类型就是 `vectors: VecDeque<(SharedCLV, MemoryId)>` 的元素
///   (`SharedCLV` 是 `Arc<[f32]>`,克隆它是引用计数 +1,不复制 2KB 向量数据;
///   `MemoryId` 的 `String` 才是堆分配来源)——故这不是 20MB 级拷贝,别按字节数猜;
/// - 释放(旧快照 drop)在真实方案里由**最后一个读者**承担,不在写者路径上,
///   故这里把它放在计时窗外,只计克隆。
fn bench_cow_clone(c: &mut Criterion) {
    let mut pool: HashMap<u64, Arc<[f32]>> = HashMap::new();
    let mut rng: u64 = 0xc0ff_ee00_0012_0002;
    let mut snapshot: Vec<(SharedCLV, MemoryId)> = Vec::with_capacity(ENTRIES);
    for i in 0..ENTRIES {
        let clv = clv_from(&mut rng);
        snapshot.push((
            SharedCLV::intern(&clv, &mut pool),
            MemoryId::new(format!("m-{i}")),
        ));
    }
    let live = Arc::new(snapshot);
    // 模拟"读者持旧快照":引用计数 > 1 ⇒ 真实方案里 make_mut 必然克隆
    let reader_holds = Arc::clone(&live);
    black_box(&reader_holds);

    let mut samples: Vec<u32> = Vec::new();
    c.bench_function("cow_snapshot_clone_10k", |b| {
        let live = Arc::clone(&live);
        let samples_mut = &mut samples;
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let t0 = Instant::now();
                let next = black_box((*live).clone());
                let dt = t0.elapsed();
                total += dt;
                if samples_mut.len() < SAMPLE_CAP {
                    samples_mut.push(to_nanos_u32(dt));
                }
                drop(next); // 释放落在计时窗外(真实方案由最后一个读者释放)
            }
            total
        });
    });
    let stats = summarize(samples);
    eprintln!(
        "[l2_lock_contention] arm=cow_snapshot_clone_10k samples={} p50_us={:.1} p95_us={:.1} \
         p99_us={:.1} max_us={:.1} mean_us={:.1}",
        stats.samples, stats.p50_us, stats.p95_us, stats.p99_us, stats.max_us, stats.mean_us
    );
}

criterion_group!(benches, bench_write_latency, bench_cow_clone);
criterion_main!(benches);
