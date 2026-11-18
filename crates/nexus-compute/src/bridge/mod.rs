//! ComputeBridge — CPU 卸载统一入口（手册 §8.3 L-f / §11.1 契约 / v4.0 §7.5.2 L-a）
//!
//! 对应架构层:L1 Core
//!
//! # 职责
//! - **L-a 全局计算池**:独立 rayon `ThreadPool`（池 = num_cpus−2、栈 2MB、
//!   线程名 `chimera-compute-*`）,与 tokio worker 隔离防互饿（v4.0 §7.5.2）。
//! - **L-f 路由**:`route()` 纳秒级查表三态派发（Inline / Rayon / Async,ADR-127）。
//! - **panic 隔离**:`spawn_compute_batch` 逐任务 `catch_unwind` 回传,
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
//! - `ComputeError::Saturated`（池饱和,D15）**自 ADR-197 起有真实触发路径**:
//!   饱和处置为 env 三态（判定与计数见 [`super::telemetry`]）——默认 `observe` 仅
//!   记观测+发事件、任务仍走池（行为零变更）;`degrade` 才降级为 Inline 执行
//!   （变慢不变错）;`strict`（旧 `CHIMERA_COMPUTE_STRICT_POOL`）才以 `Saturated` 返回错误。
//! - 缝合点（Clock/Rng/Fs/Net,§10.8 Ω₇）见 [`super::seam`],T14 注入时才接线本桥。
//! - **hnsw_rs 全局池例外（第三轮 T2.5 · 纪律④ 桥接唯一的已知破口）**：`mlc-engine`
//!   语义建边（mlc-engine `memory_graph` >1K 路径）与 `repo-wiki` `HnswStore` 依赖
//!   `hnsw_rs` 索引,其 search/insert 内部自行派发 **rayon 全局池**任务,不经本桥
//!   专用 `self.pool`。本桥的 `pool.install`/`scope` **无法将 hnsw_rs 内部的全局池调度
//!   排除/归并**（它直接调 rayon 全局 spawn,不继承外层 scope）,因此实际存在两个线程池并存。
//!   接线前须显式决定：① 迁桥（包 hnsw_rs 调用入本桥任务——因上述 install 无法排除而不可行，
//!   或需上游改造），或② 接受为**长期有界例外**并固定其线程上限。未决定前不得默认它已合规。

use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use thiserror::Error;

use super::dispatch::{DispatchPlan, TaskKind};
use super::hts::HtsTable;
use super::telemetry::KindCounters;
use super::utilization::ActiveCounter;

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
    /// 批量入口槽位理论不可达时的兜底错误（保持库代码零 panic；oneshot 取消路径已随
    /// `spawn_compute` 退役,此变体仅由 [`spawn_compute_batch`] 的 None 兜底构造）。
    #[error("cancelled before completion")]
    Cancelled,
    /// 计算池饱和,可重试
    ///
    /// 触发条件（ADR-197）:池占用 ≥ [`threshold_bp`](super::telemetry::threshold_bp)
    /// **且** 饱和处置模式为 `strict`（旧 `CHIMERA_COMPUTE_STRICT_POOL`）。默认 `observe`
    /// 模式仅记观测不改分支（任务仍走池,不返回本错误）;`degrade` 模式降级为 Inline
    /// 执行,二者均不产生本错误,后者计入降级计数。
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
    /// 阈值表写者串行锁 — 消除 `update_threshold` 的 load→clone→store lost-update
    ///
    /// WHY 单独一把 Mutex 而非 arc-swap CAS:见 [`update_threshold`](Self::update_threshold)
    /// 文档。仅串行化低频写者,读路径 `route` 不碰此锁。
    hts_write_lock: std::sync::Mutex<()>,
    /// WI-34 利用率探针 — 池活跃任务计数（spawn +1 / 完成 −1,T1 接线）
    ///
    /// WHY 独立 Arc:任务闭包需 `'static`,守卫持 Arc 克隆入闭包,RAII Drop 归还。
    active: Arc<ActiveCounter>,
    /// 池线程数快照（rayon ThreadPool 无公开线程数访问器,构造时保存）
    thread_count: usize,
    /// A1/ADR-197:按 TaskKind 归因的卸载/失败/降级计数（零分配原子数组）
    counters: KindCounters,
    /// 饱和阈值千分比覆盖（0 = 沿用 env/默认值）
    ///
    /// WHY 字段而不直读 env:[`strict_pool_mode`](super::telemetry::strict_pool_mode)
    /// 等 env 缓存是进程级 `OnceLock`,在并行测试里改 env 会彼此干扰其他用例;
    /// 实例字段供**同模块测试**确定性地模拟饱和,对外行为零影响。
    sat_bp_override: std::sync::atomic::AtomicU32,
    /// 处置策略实例覆盖（`u8::MAX` = 沿用 env;0=Observe / 1=Degrade / 2=Strict）
    ///
    /// WHY 用 AtomicU8 而不就是两个 bool:三态策略加一个"未设置"哨兵共 4 个取值,
    /// 单字段无锁读写即可表达,避开“双字段手工配对”这种可漂移的真值源（本仓
    /// shard_bus 已由 ArcSwap 收敛过同类问题）。
    mode_override: std::sync::atomic::AtomicU8,
    /// B1/ADR-198:校准采样计数器（仅当 `CHIMERA_COMPUTE_CALIBRATION` ≠ off 时读写）
    cal_counter: std::sync::atomic::AtomicU64,
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

mod batch;
mod pool;
mod probe;
mod route;

/// 三态判定纯函数 — route 的核心逻辑,与 `is_io_bound` 判定/阈值查询解耦
///
/// WHY 拆分(Ω₇):`is_io_bound` 当前对十类登记恒为 `false`(手册 §8.4 全 CPU 型),
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
///
/// R3-2b(ADR-262):`CHIMERA_COMPUTE_THREADS` 显式覆盖（与 chimera-cli 的
/// `CHIMERA_RT_WORKERS` 同族旋钮）——合法正整数(≥`MIN_THREADS`)直接采用,
/// 未设/非法回退既有口径（零行为变更）。WHY:计算池与 tokio worker 的
/// 核数分配是 CPU 争用/能耗的第一调节面,容器/桌面共宿主场景需要显式划分。
#[must_use]
fn compute_threads() -> usize {
    if let Ok(raw) = std::env::var("CHIMERA_COMPUTE_THREADS") {
        match raw.trim().parse::<usize>() {
            Ok(n) if n >= MIN_THREADS => return n,
            _ => tracing::warn!(
                value = %raw,
                min = MIN_THREADS,
                "CHIMERA_COMPUTE_THREADS 非法,回退默认（num_cpus − 2,下限 MIN_THREADS）"
            ),
        }
    }
    match std::thread::available_parallelism() {
        Ok(n) => (n.get().saturating_sub(2)).max(MIN_THREADS),
        // 受限环境(如 cgroup 探测失败)无法获知核心数,取保守下限;
        // 注意:容器内 num_cpus 不可信的问题(ADR-103 三重来源③)由 T9 cgroup 校正接管
        Err(_) => MIN_THREADS,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
// test-module unwrap is the Rust idiom; E-5 targets production code
mod tests;
