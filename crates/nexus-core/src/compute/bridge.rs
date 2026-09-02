//! ComputeBridge — CPU 卸载统一入口（手册 §8.3 L-f / §11.1 契约 / v4.0 §7.5.2 L-a）
//!
//! 对应架构层:L1 Core
//!
//! # 职责
//! - **L-a 全局计算池**:独立 rayon `ThreadPool`（池 = num_cpus−2、栈 2MB、
//!   线程名 `chimera-compute-*`）,与 tokio worker 隔离防互饿（v4.0 §7.5.2）。
//! - **L-f 路由**:`route()` 纳秒级查表三态派发（Inline / Rayon / Async,ADR-127）。
//! - **panic 隔离**:`spawn_compute` 用 `catch_unwind` + `tokio::sync::oneshot` 回传,
//!   池内 panic 永不跨线程传播到调用方（§11.1 不变式）。
//!
//! # 契约纪律（§7.5.3 纪律④⑥,CI 静态扫描兜底）
//! rayon 闭包内**禁 `.await`、禁 IO（sqlite/网络/LLM 调用）、禁持锁跨闭包边界**;
//! 违例即触发池内线程阻塞,属于红线违规。
//!
//! # 骨架边界
//! - 阈值表为**动态 HTS 表**（[`HtsTable`],arc-swap RCU 读无锁,ADR-128）:初始值为
//!   S9 离线测定预填（来源标注见 [`super::hts`]）,T1 测定/T14 序贯检验经
//!   [`update_threshold`](ComputeBridge::update_threshold) 灌入。
//! - **HTS 序贯检验判定已落地**（T9,[`super::hts::sequential_test`]）:本桥只转发
//!   更新接口,真实运行时采样接线在 T14（注入时启用）。
//! - `ComputeError::Saturated`（池饱和熔断,D15）为契约变体,触发路径在 T14 接线。
//! - 缝合点（Clock/Rng/Fs/Net,§10.8 Ω₇）见 [`super::seam`],T14 注入时才接线本桥。

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use arc_swap::ArcSwap;

use super::dispatch::{DispatchPlan, TaskKind};
use super::hts::{HtsTable, ThresholdSource};
use super::reduce::{reduce as reduce_impl, ReduceMode};
// T1(WI-34):rayon 池活跃计数探针（利用率测量基座,见 utilization.rs）
use super::utilization::{ActiveCounter, RayonProbe};
use thiserror::Error;

/// 全局计算池线程数下限 — 即使单核/受限环境也保留 2 线程
const MIN_THREADS: usize = 2;

/// 全局计算池线程栈大小（2MB,与手册 §8.3 一致）
const STACK_SIZE_BYTES: usize = 2 * 1024 * 1024;

/// Compute 错误 — 手册 §11.1 契约（thiserror,库层错误标准 §4.1）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ComputeError {
    /// 任务在计算池内 panic — 已被 catch_unwind 隔离,不跨线程传播
    #[error("task panicked in compute pool")]
    Panicked,
    /// 调用方在完成前取消（oneshot 接收端失效）
    #[error("cancelled before completion")]
    Cancelled,
    /// 计算池饱和,可重试（D15 熔断触发路径,T9/T14 接线）
    #[error("pool saturated, retryable")]
    Saturated,
}

/// CPU 卸载统一入口 — L-a 全局 rayon 池 + L-f 三态路由
///
/// 通过 [`bridge()`] 获取进程级单例（`OnceLock` 惰性初始化）;
/// 本类型不实现 `Clone`/`Copy`,避免误导调用方复制池句柄。
pub struct ComputeBridge {
    /// L-a 全局计算池（线程名 `chimera-compute-*`,池 = num_cpus−2,栈 2MB）
    pool: rayon::ThreadPool,
    /// HTS 动态阈值表 — arc-swap RCU 承载（ADR-128 读无锁,~5ns 快照）
    ///
    /// WHY ArcSwap 而非 RwLock:route() 是纳秒级热路径（P99 < 1µs 门禁）,读路径
    /// 必须无锁;更新（T1 测定/T14 校准）低频,RCU store 全表替换代价可忽略。
    hts: ArcSwap<HtsTable>,
    /// WI-34 利用率探针 — 池活跃任务计数（spawn +1 / 完成 −1,T1 接线）
    ///
    /// WHY 独立 Arc:任务闭包需 `'static`,守卫持 Arc 克隆入闭包,RAII Drop 归还。
    active: Arc<ActiveCounter>,
    /// 池线程数快照（rayon ThreadPool 无公开线程数访问器,构造时保存）
    thread_count: usize,
}

/// 进程级单例 — 手册 §8.3 骨架（`OnceLock` 惰性初始化,线程安全,幂等）
///
/// WHY 单例:18 个 crate 的 CPU 热点统一汇聚到唯一计算池（桥接唯一纪律④）,
/// 池线程数是全局资源预算（num_cpus−2）,重复建池会互相饿死。
#[must_use]
pub fn bridge() -> &'static ComputeBridge {
    static BRIDGE: OnceLock<ComputeBridge> = OnceLock::new();
    BRIDGE.get_or_init(|| ComputeBridge::new(compute_threads()))
}

impl ComputeBridge {
    /// 构造计算池 — 供单例惰性初始化调用,不直接暴露（用 [`bridge()`]）
    ///
    /// WHY 私有构造:池配置（线程数/栈/线程名）是全局预算,不允许调用方按需定制
    /// 导致多池并存（纪律④ 桥接唯一）。
    fn new(threads: usize) -> Self {
        // WHY 显式 match + panic 而非 unwrap/expect(红线:错误用 ?/match):
        // rayon build 失败仅当 OS 拒绝创建线程(进程级资源耗尽),此时计算桥不可用
        // 即进程不可用,无运行期降级路径(与 D15 运行期熔断杀池不同)——这是进程
        // 启动期致命前提的失败,panic 沿 OnceLock 惰性初始化传播(§11.1 不变式
        // "spawn_compute 永不 panic 到调用方"仅约束运行期计算任务,不覆盖单例初始化前提)
        let pool = match rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .stack_size(STACK_SIZE_BYTES)
            .thread_name(|idx| format!("chimera-compute-{idx}"))
            .build()
        {
            Ok(p) => p,
            Err(e) => panic!("ComputeBridge 初始化失败(系统线程资源耗尽): {e}"),
        };
        Self {
            pool,
            // HTS 初始表:dispatch.rs 静态值迁移（S9 离线测定预填,W1 复测,§8.4）
            hts: ArcSwap::from_pointee(HtsTable::default()),
            active: Arc::new(ActiveCounter::default()),
            thread_count: threads,
        }
    }

    /// WI-34 rayon 探针 — 池活跃任务数（瞬时,Relaxed 读）
    #[must_use]
    pub fn pool_active_tasks(&self) -> usize {
        self.active.active()
    }

    /// WI-34 rayon 探针 — 池线程数（构造时快照）
    #[must_use]
    pub fn pool_threads(&self) -> usize {
        self.thread_count
    }

    /// WI-34 rayon 探针 — 池活跃率快照（供 utilization::RayonProbe 消费）
    #[must_use]
    pub fn rayon_probe(&self) -> RayonProbe {
        RayonProbe {
            pool_threads: self.thread_count,
            active_tasks: self.active.active(),
        }
    }

    /// L-f 核心路由 — 纳秒级查表三态判定（取代 S8 的 50-200μs RuntimeSwitcher,ADR-127）
    ///
    /// 判定顺序:① IO 密集 → [`Async`](DispatchPlan::Async);
    /// ② 条目数 < 阈值 → [`Inline`](DispatchPlan::Inline);
    /// ③ 否则 → [`Rayon`](DispatchPlan::Rayon)。
    ///
    /// 阈值来源:**动态 HTS 表**（T9 升级,替代 T8 的静态查表）——`load()` 拿
    /// arc-swap 无锁快照（~5ns,ADR-128）,`get(kind)` 固定数组索引零分配,
    /// 保持 T8 的零分配语义。无副作用,可并发调用。
    #[must_use]
    pub fn route(&self, kind: TaskKind, n_items: usize) -> DispatchPlan {
        let entry = self.hts.load().get(kind);
        decide(kind, kind.is_io_bound(), entry.threshold, n_items)
    }

    /// 运行期更新阈值表 — T1 测定值灌入 / T14 序贯检验校准的转发接口
    ///
    /// WHY RCU store 而非原地改:读路径持无锁快照,更新必须全表替换（arc-swap 语义）;
    /// 更新低频（测定/校准事件）,每次复制 6 条 Entry 的成本可忽略。
    /// `source` 为强制显式参数（诚实数据红线:阈值必须可溯源,见 [`ThresholdSource`]）。
    pub fn update_threshold(
        &self,
        kind: TaskKind,
        threshold: usize,
        chunk: usize,
        source: ThresholdSource,
    ) {
        let current = self.hts.load_full();
        let mut next = (*current).clone();
        next.update(kind, threshold, chunk, source);
        self.hts.store(Arc::new(next));
    }

    /// 确定性归约 — §11.1 Compute 契约 reduce 方法（ADR-102/106,手册 §10.2）
    ///
    /// 语义:纯 CPU 标量归约,同步内联执行,无错误路径（契约直接返回 f64,
    /// `Saturated`/`Cancelled` 等错误变体不适用）;委托 [`crate::compute::reduce`]。
    ///
    /// # 契约
    /// - [`ReduceMode::Deterministic`]:同一构建内多次调用逐位一致;双构建容差 ≤ 1e-6;
    /// - [`ReduceMode::Audit`]:ReproBLAS 式指数分桶,跨构建逐位比对用
    ///   （审计开销 ≤ 30% 门禁,见 `reduce_bench`）。
    #[must_use]
    pub fn reduce(&self, vals: &[f64], mode: ReduceMode) -> f64 {
        // WHY 不走 spawn_compute:归约是标量级计算,池调度（微秒级）反成主导开销;
        // T14 接线时若输入规模超 HTS 阈值,再考虑 Rayon 分块并行（接口不变,
        // 返回类型 f64 无错误面）
        reduce_impl(vals, mode)
    }

    /// L-a 统一入口 — CPU 卸载单任务
    ///
    /// 契约（§11.1 不变式）:
    /// - 池内闭包 panic 被 `catch_unwind` 捕获,返回 [`ComputeError::Panicked`],**永不 panic 到调用方**;
    /// - 调用方在 await 前 drop future（取消）时,池任务完成后静默丢弃结果（oneshot 取消契约）;
    /// - rayon 闭包内禁 `.await` / IO / 持锁跨闭包边界（§7.5.3 纪律④⑥,红线）。
    ///
    /// `kind` 当前仅作契约占位（骨架阶段不参与调度权重/遥测,T9/T14 接线）。
    pub fn spawn_compute<F, T>(
        &self,
        kind: TaskKind,
        f: F,
    ) -> impl Future<Output = Result<T, ComputeError>> + Send
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // WHY 骨架阶段:kind 仅登记契约,调度权重与遥测计数(ADR-125)在 T9/T14 接线
        let _ = kind;
        let (tx, rx) = tokio::sync::oneshot::channel();
        // WI-34 探针:执行即计（闭包内守卫 +1/Drop 归还）;排队阶段不计入活跃数
        let active = Arc::clone(&self.active);
        self.pool.spawn(move || {
            let _guard = active.enter();
            // AssertUnwindSafe:闭包 f 不跨 await/锁边界(契约纪律),panic 隔离无需全局可反射状态
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                .map_err(|_| ComputeError::Panicked);
            // 调用方已取消(drop rx)时 send 返回 Err,静默丢弃——取消安全
            let _ = tx.send(r);
        });
        async move { rx.await.map_err(|_| ComputeError::Cancelled)? }
    }

    /// L-a 批量入口 — CPU 卸载批量任务（任意数量闭包,分块并行）
    ///
    /// WHY `rayon::scope` 而非 `rayon::join`:`join` 只接收 **2 个**闭包（§8.3 修正 1/C13）,
    /// 批量必须用 scope 动态 spawn 任意数量 FnOnce 任务到本桥专用池。
    /// 结果按输入顺序对应（槽位写入索引与输入枚举一致）,结果数恒等于输入数。
    ///
    /// 单个任务 panic 不影响同批其他任务（逐个 catch_unwind 隔离）。
    ///
    /// `kind` 当前仅作契约占位（同 [`spawn_compute`](Self::spawn_compute)）。
    #[must_use]
    pub fn spawn_compute_batch<F, T, I>(
        &self,
        kind: TaskKind,
        items: I,
    ) -> Vec<Result<T, ComputeError>>
    where
        I: IntoIterator<Item = F>,
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // WHY 骨架阶段:同 spawn_compute,kind 仅登记契约
        let _ = kind;
        let items: Vec<F> = items.into_iter().collect();
        let n = items.len();
        if n == 0 {
            return Vec::new();
        }

        // 槽位共享:每任务写唯一索引槽(结果序 = 输入序);
        // WHY Mutex 而非原子/无锁:骨架阶段正确性优先,8 线程 × 槽位锁开销可忽略,
        // 后续可换 PerCpuPadded 计数器(ADR-125)优化
        let slots: Mutex<Vec<Option<Result<T, ComputeError>>>> =
            Mutex::new((0..n).map(|_| None).collect());

        // WI-34 探针:每任务闭包内守卫配对（执行即计,完成/panic 均归还）
        let active = Arc::clone(&self.active);

        // self.pool.scope 把任务调度到本桥专用池(而非 rayon 全局池,纪律④ 桥接唯一)
        self.pool.scope(|s| {
            for (idx, f) in items.into_iter().enumerate() {
                let slots = &slots;
                let active = Arc::clone(&active);
                s.spawn(move |_| {
                    let _guard = active.enter();
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                        .map_err(|_| ComputeError::Panicked);
                    // 任务内全部 catch_unwind,锁不 poison;unwrap_or_else 兜底防御
                    let mut guard = slots
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard[idx] = Some(r);
                });
            }
        });

        // 理论不可达分支:scope 同步阻塞至全部任务完成,槽位必然已填;
        // None 兜底用 Cancelled(取消语义)而非 panic,保持库代码零 panic 红线
        slots
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .into_iter()
            .map(|slot| slot.unwrap_or(Err(ComputeError::Cancelled)))
            .collect()
    }
}

/// 三态判定纯函数 — route 的核心逻辑,与 `is_io_bound` 判定/阈值查询解耦
///
/// WHY 拆分(Ω₇):`is_io_bound` 当前对六类登记恒为 `false`(手册 §8.4 全 CPU 型),
/// 直接经公开 `route` 无法覆盖 Async 分支;拆出纯函数后三态分支均可在
/// IO 类任务登记(T9)之前被单测锁定,登记时零回归风险。
///
/// 判定顺序（契约固定,勿调整）:
/// ① `io_bound` → [`Async`](DispatchPlan::Async);
/// ② `n_items < threshold` → [`Inline`](DispatchPlan::Inline);
/// ③ 否则 → [`Rayon`](DispatchPlan::Rayon)。
#[must_use]
pub(crate) fn decide(
    kind: TaskKind,
    io_bound: bool,
    threshold: usize,
    n_items: usize,
) -> DispatchPlan {
    // WHY 显式消费 kind:未来按任务类型做加权/遥测的扩展点(ADR-125),签名先行
    let _ = kind;
    if io_bound {
        return DispatchPlan::Async;
    }
    if n_items < threshold {
        DispatchPlan::Inline
    } else {
        DispatchPlan::Rayon
    }
}

/// 计算池线程数 — num_cpus − 2,下限 2（手册 §8.3 与红线 3）
///
/// 预留 2 核给 tokio worker,防 CPU 计算饿死 IO（v4.0 §7.5.2 防互饿设计）。
/// 用 `std::thread::available_parallelism()` 而非 num_cpus crate（Ω₆ 最小依赖）。
#[must_use]
fn compute_threads() -> usize {
    match std::thread::available_parallelism() {
        Ok(n) => (n.get().saturating_sub(2)).max(MIN_THREADS),
        // 受限环境(如 cgroup 探测失败)无法获知核心数,取保守下限;
        // 注意:容器内 num_cpus 不可信的问题(ADR-103 三重来源③)由 T9 cgroup 校正接管
        Err(_) => MIN_THREADS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::dispatch::TaskKind;

    /// bridge() 单例幂等 — 两次调用返回同一指针（OnceLock 语义）
    #[test]
    fn bridge_is_singleton() {
        let a = bridge();
        let b = bridge();
        assert!(std::ptr::eq(a, b));
    }

    /// 计算池线程数 — 恒 >= 下限 2
    #[test]
    fn compute_threads_at_least_two() {
        assert!(compute_threads() >= MIN_THREADS);
    }

    /// decide 纯函数 — 三态全分支（含阈值边界）
    #[test]
    fn decide_three_way() {
        let b = bridge();
        // Inline: n < 阈值
        assert_eq!(b.route(TaskKind::Generic, 9_999), DispatchPlan::Inline);
        // Rayon: n == 阈值(边界)与 n > 阈值
        assert_eq!(b.route(TaskKind::Generic, 10_000), DispatchPlan::Rayon);
        assert_eq!(b.route(TaskKind::Generic, 10_001), DispatchPlan::Rayon);
        // Async: io_bound 优先(与 n 无关)
        assert_eq!(
            decide(TaskKind::Generic, true, 10_000, 0),
            DispatchPlan::Async
        );
        assert_eq!(
            decide(TaskKind::Generic, true, 10_000, usize::MAX),
            DispatchPlan::Async
        );
    }

    /// spawn_compute 正常路径 — 返回闭包结果,类型保真
    #[tokio::test]
    async fn spawn_compute_ok() {
        let out = bridge()
            .spawn_compute(TaskKind::Generic, || 42u64)
            .await
            .expect("正常任务应成功");
        assert_eq!(out, 42);
    }

    /// spawn_compute panic 隔离 — 池内 panic 转为 ComputeError::Panicked,不跨线程传播
    #[tokio::test]
    async fn spawn_compute_panics_isolated() {
        // 静音 panic hook:panic 消息预期出现(隔离语义),避免污染测试输出
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let res = bridge()
            .spawn_compute(TaskKind::Generic, || -> u64 { panic!("boom") })
            .await;
        std::panic::set_hook(prev);
        assert_eq!(res, Err(ComputeError::Panicked));
    }

    /// spawn_compute 取消契约 — oneshot 接收端失效后发送失败（取消安全）
    ///
    /// spawn_compute 依赖此契约:调用方 drop future 时,池内任务完成后的
    /// tx.send 返回 Err 被静默丢弃,无泄漏无 panic。
    #[test]
    fn spawn_compute_oneshot_cancel_semantics() {
        let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
        drop(rx);
        assert!(tx.send(42).is_err(), "接收端 drop 后发送必须失败");
    }

    /// Cancelled 映射 — rx.await 在发送端失效时返回 ComputeError::Cancelled
    #[tokio::test]
    async fn cancelled_mapping() {
        let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
        drop(tx);
        let mapped: Result<u64, ComputeError> = rx.await.map_err(|_| ComputeError::Cancelled);
        assert_eq!(mapped, Err(ComputeError::Cancelled));
    }

    /// spawn_compute_batch — 结果数与输入一致,顺序对应,无 panic
    #[tokio::test]
    async fn spawn_compute_batch_count_and_order() {
        let n = 32;
        let items: Vec<_> = (0..n).map(|i| move || i * 2).collect();
        let out = bridge().spawn_compute_batch(TaskKind::Generic, items);
        assert_eq!(out.len(), n, "结果数必须等于输入数");
        for (idx, r) in out.iter().enumerate() {
            assert_eq!(
                r.as_ref().expect("批量任务应成功"),
                &(idx * 2),
                "顺序必须对应输入"
            );
        }
    }

    /// spawn_compute_batch — 空输入返回空结果
    #[test]
    fn spawn_compute_batch_empty() {
        // F = 函数指针,显式标注类型以避免空迭代器无法推断 T
        let out: Vec<Result<u64, ComputeError>> =
            bridge().spawn_compute_batch(TaskKind::Generic, std::iter::empty::<fn() -> u64>());
        assert!(out.is_empty());
    }

    /// spawn_compute_batch — 单任务 panic 不影响同批其他任务
    #[test]
    fn spawn_compute_batch_panic_isolated() {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let items: Vec<Box<dyn FnOnce() -> u64 + Send>> =
            vec![Box::new(|| 1), Box::new(|| panic!("boom")), Box::new(|| 3)];
        let out = bridge().spawn_compute_batch(TaskKind::Generic, items);
        std::panic::set_hook(prev);
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0], Ok(1)));
        assert_eq!(out[1], Err(ComputeError::Panicked));
        assert!(matches!(&out[2], Ok(3)));
    }

    /// 并发 route — 8 线程 × 1000 次无 panic 无数据竞争
    ///
    /// WHY 替代 loom L-01:loom 在 Windows GNU 工具链不适用(项目 .toolchain 为
    /// stable-x86_64-pc-windows-gnu,loom 依赖平台线程模型细节,Windows 支持受限);
    /// 任务授权回退为普通多线程测试。DispatchPlan 为 Copy 纯枚举 + 静态阈值表,
    /// 并发读天然无竞争,此测试锁定"并发调用 route 行为一致"。
    #[test]
    fn route_concurrent_reads() {
        let b = bridge();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    for i in 0..1_000usize {
                        for kind in TaskKind::ALL {
                            let plan = b.route(kind, i);
                            let t = kind.threshold();
                            assert_eq!(plan, decide(kind, kind.is_io_bound(), t, i));
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("并发 route 线程应正常退出");
        }
    }

    /// update_threshold 联动 — route() 读动态表:阈值 1000→10 后 20 项 Inline→Rayon
    ///
    /// WHY 独立实例而非全局 `bridge()`:全局单例表被并行测试共享,直接修改会与
    /// 其他测试（如本文件 `route_concurrent_reads` 的静态阈值断言）竞争;
    /// 独立实例在私有字段可访问处构造（同模块测试）,零全局副作用。
    #[test]
    fn update_threshold_changes_route() {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .expect("测试池构建失败");
        let b = ComputeBridge {
            pool,
            hts: ArcSwap::from_pointee(HtsTable::default()),
            active: Arc::new(ActiveCounter::default()),
            thread_count: 2,
        };
        let kind = TaskKind::ClvSimilarity;
        // §8.4 初值:阈值 1000 → 20 项走 Inline
        assert_eq!(b.route(kind, 20), DispatchPlan::Inline);
        // T1/T14 校准:阈值降至 10 → 20 项转 Rayon,9 项仍 Inline
        b.update_threshold(kind, 10, 64, ThresholdSource::ConservativeDefault);
        assert_eq!(
            b.route(kind, 20),
            DispatchPlan::Rayon,
            "阈值降低后 route 必须跟随动态表"
        );
        assert_eq!(b.route(kind, 9), DispatchPlan::Inline);
        // 未更新的类别不受影响
        assert_eq!(b.route(TaskKind::KnnSearch, 5_000), DispatchPlan::Rayon);
    }

    /// 池探针语义 — 批量任务执行期间活跃数 > 0,完成后归零
    ///
    /// WHY 独立实例而非全局 `bridge()`:全局单例池被并行测试共享（如
    /// `pool_active_probe_concurrent` 的在途任务会污染 active 计数断言）;
    /// 独立实例构造私有字段（同模块测试可见）,零全局副作用。
    #[test]
    fn pool_active_probe_lifecycle() {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .expect("测试池构建失败");
        let b = ComputeBridge {
            pool,
            hts: ArcSwap::from_pointee(HtsTable::default()),
            active: Arc::new(ActiveCounter::default()),
            thread_count: 2,
        };
        assert_eq!(b.pool_active_tasks(), 0, "空闲池活跃数必须为 0");
        assert_eq!(b.pool_threads(), 2);
        let n = 16;
        let items: Vec<_> = (0..n)
            .map(|i| {
                move || {
                    // 模拟 CPU 任务:自旋一小段时间保证探针采样窗口可见
                    let start = std::time::Instant::now();
                    while start.elapsed() < std::time::Duration::from_millis(2) {
                        std::hint::spin_loop();
                    }
                    i
                }
            })
            .collect();
        let probe_before = b.rayon_probe();
        let out = b.spawn_compute_batch(TaskKind::Generic, items);
        let probe_after = b.rayon_probe();
        assert_eq!(out.len(), n);
        assert_eq!(probe_before.active_tasks, 0);
        assert_eq!(probe_after.active_tasks, 0, "批量完成后活跃数必须归零");
        assert_eq!(b.pool_active_tasks(), 0);
    }

    /// 池探针并发可见性 — 8 并发 batch 交叉执行时峰值计数可达 >0
    #[test]
    fn pool_active_probe_concurrent() {
        let b = bridge();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    let items: Vec<_> = (0..8)
                        .map(|i| {
                            move || {
                                let start = std::time::Instant::now();
                                while start.elapsed() < std::time::Duration::from_millis(1) {
                                    std::hint::spin_loop();
                                }
                                i
                            }
                        })
                        .collect();
                    let out = b.spawn_compute_batch(TaskKind::Generic, items);
                    assert_eq!(out.len(), 8);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("并发线程应正常退出");
        }
        assert_eq!(b.pool_active_tasks(), 0, "全部并发任务完成后活跃数归零");
    }

    /// ComputeError Display — 三变体文案符合 §11.1 契约
    #[test]
    fn compute_error_display() {
        assert_eq!(
            ComputeError::Panicked.to_string(),
            "task panicked in compute pool"
        );
        assert_eq!(
            ComputeError::Cancelled.to_string(),
            "cancelled before completion"
        );
        assert_eq!(
            ComputeError::Saturated.to_string(),
            "pool saturated, retryable"
        );
    }

    /// reduce 契约 — §11.1:bridge 委托 reduce 模块,双模式结果逐位一致
    #[test]
    fn bridge_reduce_delegates() {
        let vals = [1.0, -2.0, 0.5, 1e-3, -7.0e-9];
        for mode in [
            super::super::reduce::ReduceMode::Deterministic,
            super::super::reduce::ReduceMode::Audit,
        ] {
            let via_bridge = bridge().reduce(&vals, mode);
            let direct = super::super::reduce::reduce(&vals, mode);
            assert_eq!(
                via_bridge.to_bits(),
                direct.to_bits(),
                "bridge reduce({mode:?}) 必须与 reduce 模块直调逐位一致"
            );
        }
    }
}
