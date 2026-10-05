//! 计算面遥测与池饱和判定 — ADR-197（Phase A 可观测闭环）
//!
//! 对应架构层:L1 Core
//!
//! # 职责（三件事,互不越界）
//! 1. **占用归因**:[`KindCounters`] 以定长原子数组按 [`TaskKind`] 记录 spawn /
//!    failure / 降级 / 拒绝次数,为「谁吃了池时间」提供可导出证据（消解审计 S-5:
//!    `kind` 曾仅作契约占位）。
//! 2. **饱和判定**:[`saturation_ratio_bp`] 与 [`parse_saturation_mode`] / [`parse_threshold_bp`] 均为
//!    **纯函数**;阈值经 [`threshold_bp`] 从 env 一次性读取（默认 900 = 90%）;
//!    [`saturation_mode`] 决定超阈后怎么做：`Observe`（默认，只观测不改分支）/
//!    `Degrade`（走 Inline）/ `Strict`（直接返回 `Saturated`）。
//! 3. **观测上报**:[`spawn_saturation_reporter`] 周期采样并在状态变化时发布
//!    `NexusEvent::ComputePoolSaturated`（Normal 级观测面）。
//!
//! # WHY 上报放在装配层周期器,而不是在 `spawn_*` 热路径里发事件
//! - 本桥**不持 `EventBus`**（`ComputeBridge` 只有池 + 阈值表 + 活跃计数）;
//!   若为发事件而给 L1 计算桥塞一个全局 bus 句柄,等于把 IO 侧依赖引入纳秒级热路径。
//! - 仓内既有先例是**owner crate 提供 `spawn_*_reporter`、装配层启动**:
//!   `event_bus::spawn_throughput_reporter` 与 `seccore::spawn_interception_reporter`
//!   （均被 `chimera-cli/src/experience_loop.rs` 的 §16.5 周期报告器段调用）。
//!   本模块沿用同一形态,故零新增跨层依赖边。
//!
//! # 数值与红线纪律
//! - 比率一律用**千分比 `u32`** 表示,不进浮点相等比较（避免 GPU 提案那类
//!   "微小差异可接受"的语义滑坡,也与 DetReduce 逐位一致取向一致）。
//! - u64 大数百分比按项目红线以 `f64` 中间值计算。
//! - `catch_unwind` / 锁纪律不在此模块;本模块**无锁、无 await、无 IO**（除 reporter 的
//!   sleep 与 `publish_blocking`）,`publish_blocking` 失败仅 `warn!` 不阻断。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use event_bus::EventBus;

use super::calibration;
use super::dispatch::TaskKind;
use super::hts::kind_index;
use super::utilization::UtilizationSampler;

/// 数组槽位数 —— 与 [`TaskKind::ALL`] 唯一对应
///
/// WHY 用 `ALL.len()` 而非硬编码:ADR-196 的 TaskKind 扩容（+四登记位）会改变变体数,
/// 此处自动跟随,不留需要同步的魔数。
const KIND_SLOTS: usize = TaskKind::ALL.len();

/// 默认饱和阈值（千分比：900 = 90%）
///
/// WHY 千分比而不是百分比/小数:与 [`saturation_ratio_bp`] 同量纲,整数比较无浮点误差;
/// 1000‰ = 池恰好满载,阈值取 900‰ 留出余量（避免临界抖动）。
const DEFAULT_SATURATION_BP: u32 = 900;

/// 合法阈值区间（千分比）：100‰ = 10% 到 1_000‰ = 池恰好满载
///
/// 第三轮 T2.3 收窄（原 100..=10_000）:占用比 `saturation_ratio_bp = active×1000/threads`，
/// 而 `active` 是并发守卫计数（rayon 每线程最多同时跑一个闭包，`enter()` 仅在
/// 闭包被调度时发生），故实际 `active ≤ threads` → 占用比恒 ≤ 1000‰。旧上限
/// 10_000‰ 隐含“排队积压也可超满载”的假设，但队列深度当前不可观测（F-U2），
/// 且 >1000‰ 阈值永不触发——与 fail-safe 拒绝“0=永不降级”同危险（静默失效）。
/// 队列深度真实可观测后属独立 ADR（本轮不做），故先把区间收紧到有语义上限。
const THRESHOLD_BP_RANGE: std::ops::RangeInclusive<u32> = 100..=1_000;

/// 饱和阈值 env 变量名（降级路径走配置/env,禁 feature 标志）
const ENV_SATURATION_BP: &str = "CHIMERA_COMPUTE_SATURATION_BP";

/// 严格模式 env 变量名（历史名，现仅作为 `=strict` 的简写保留）
const ENV_STRICT_POOL: &str = "CHIMERA_COMPUTE_STRICT_POOL";

/// 饱和处置策略 env 变量名
const ENV_SATURATION_MODE: &str = "CHIMERA_COMPUTE_SATURATION_MODE";

/// 饱和处置策略（ADR-197 D2）
///
/// WHY 三态而不是一个布尔:计划的风险缓解要求“先以不改变分支的观测模式上线,
/// 拐点数据充足后再开降级”;单一布尔只能表达“降级 / 报错”两种行为变更,
/// 无法表达“只看不改分支”这个最安全的默认态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaturationMode {
    /// 默认:只记计数与发事件，任务**仍走池**（行为零改变，先测拐点）
    Observe,
    /// 超阈时改走 Inline 执行（变慢不变错，结果逐元素一致）
    Degrade,
    /// 超阈时不执行闭包，直接返回 `ComputeError::Saturated`（调用方自行重试）
    Strict,
}

/// 解析处置策略 —— 纯函数，便于表驱动单测
///
/// 非法值回落 [`SaturationMode::Observe`]（WHY:行为变更必须显式声明才能发生）。
/// 向后兼容:旧 `CHIMERA_COMPUTE_STRICT_POOL=1/true/on/yes` 等价于 `strict`。
#[must_use]
pub fn parse_saturation_mode(raw: Option<&str>, legacy_strict: Option<&str>) -> SaturationMode {
    if let Some(text) = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
    {
        return match text.as_str() {
            "observe" => SaturationMode::Observe,
            "degrade" | "inline" => SaturationMode::Degrade,
            "strict" | "error" => SaturationMode::Strict,
            // 空/非法值不猜意图:回到零行为变更的默认态
            _ => SaturationMode::Observe,
        };
    }
    // 兼容旧开关（只认肯定值,其他一律当未设置）
    match legacy_strict
        .map(str::trim)
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("1") | Some("true") | Some("on") | Some("yes") => SaturationMode::Strict,
        _ => SaturationMode::Observe,
    }
}

/// 单个 `TaskKind` 的累计计数快照（供导出与测试断言）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindStat {
    /// 任务类型
    pub kind: TaskKind,
    /// 卸载次数（含降级执行的那些）
    pub spawns: u64,
    /// 池内 panic 次数
    pub failures: u64,
}

/// 按 `TaskKind` 归因的计数器 —— 定长原子数组,零分配、零锁
///
/// WHY 不用 DashMap/Vec:热路径每次卸载只做 1 次 `fetch_add`,索引由
/// [`kind_index`] 在编译期确定的数组上取,避免任何堆分配与哈希开销。
#[derive(Debug)]
pub struct KindCounters {
    spawns: [AtomicU64; KIND_SLOTS],
    failures: [AtomicU64; KIND_SLOTS],
    /// 累计"因池饱和而降级为 Inline 执行"的卸载次数
    ///
    /// WHY 与 `saturations` 分开：默认 Observe 策略下“超阈”发生了但“降级”没发生，
    /// 合用一个计数器会让事件谎报行为变更次数。
    degradations: AtomicU64,
    /// 累计降级为 Inline 的**项数**(第三轮 T2.2):`degradations` 是批粒度
    /// (一批只 +1),本计数是项粒度(一批降级 n 项则 +n)。二者口径不同不可互推。
    /// WHY 独立计数而非逐项 `active.enter`:逐项 enter 会把降级本身计入并发
    /// 占用→高负载下触发饱和自屏蔽(降级越多越饱和)的行为变更(T2.2 明确拒绝);
    /// 纯遥测增量,不改变 `active` 语义。
    degraded_items: AtomicU64,
    /// 累计超阈观测次数（三种策略下均递增，上报去抖基线）
    saturations: AtomicU64,
    /// 累计"严格模式下直接返回 `Saturated`"的拒绝次数
    rejections: AtomicU64,
}

impl Default for KindCounters {
    fn default() -> Self {
        Self {
            spawns: std::array::from_fn(|_| AtomicU64::new(0)),
            failures: std::array::from_fn(|_| AtomicU64::new(0)),
            degradations: AtomicU64::new(0),
            degraded_items: AtomicU64::new(0),
            saturations: AtomicU64::new(0),
            rejections: AtomicU64::new(0),
        }
    }
}

impl KindCounters {
    /// 记录一次卸载提交（降级路径同样计入 —— 语义是"任务被受理"）
    pub fn record_spawn(&self, kind: TaskKind) {
        self.spawns[kind_index(kind)].fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次池内 panic（`ComputeError::Panicked`）
    pub fn record_failure(&self, kind: TaskKind) {
        self.failures[kind_index(kind)].fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次因饱和触发的 Inline 降级
    pub fn record_degradation(&self) {
        self.degradations.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一批降级为 Inline 的**项数**(第三轮 T2.2 独立项粒度遥测)
    ///
    /// WHY 传 `n` 而非逐项调用:降级项数由调用方(`spawn_compute_batch`)一次传入,
    /// 与 `active.enter`(并发占用)解耦——避免降级被计入饱和判据导致自屏蔽行为变更。
    pub fn record_degraded_items(&self, n: u64) {
        self.degraded_items.fetch_add(n, Ordering::Relaxed);
    }

    /// 记录一次超阈观测（三种策略下均记，用于事件去抖基线）
    pub fn record_saturation(&self) {
        self.saturations.fetch_add(1, Ordering::Relaxed);
    }

    /// 累计超阈观测次数
    #[must_use]
    pub fn saturations(&self) -> u64 {
        self.saturations.load(Ordering::Relaxed)
    }

    /// 记录一次严格模式下的 `Saturated` 拒绝
    pub fn record_rejection(&self) {
        self.rejections.fetch_add(1, Ordering::Relaxed);
    }

    /// 累计降级次数（单调递增,供上报器去抖判据）
    #[must_use]
    pub fn degradations(&self) -> u64 {
        self.degradations.load(Ordering::Relaxed)
    }

    /// 累计降级为 Inline 的项数(第三轮 T2.2;项粒度,≠ `degradations` 批粒度)
    #[must_use]
    pub fn degraded_items_total(&self) -> u64 {
        self.degraded_items.load(Ordering::Relaxed)
    }

    /// 累计严格拒绝次数
    #[must_use]
    pub fn rejections(&self) -> u64 {
        self.rejections.load(Ordering::Relaxed)
    }

    /// 导出各类型计数快照（按 [`TaskKind::ALL`] 顺序,确定性输出）
    #[must_use]
    pub fn snapshot(&self) -> Vec<KindStat> {
        TaskKind::ALL
            .iter()
            .enumerate()
            .map(|(idx, kind)| KindStat {
                kind: *kind,
                spawns: self.spawns[idx].load(Ordering::Relaxed),
                failures: self.failures[idx].load(Ordering::Relaxed),
            })
            .collect()
    }

    /// 卸载计数最高的类型名（用于事件归因字段）
    ///
    /// WHY 决胜规则确定化:计数相同时取 [`TaskKind::ALL`] 中靠前者 —— 与
    /// `variant_pool::best_by_perf` 同类"确定性决胜"纪律,避免归因抖动。
    #[must_use]
    pub fn hottest_kind(&self) -> String {
        let snap = self.snapshot();
        let mut best: Option<KindStat> = None;
        for stat in snap {
            match best {
                Some(cur) if stat.spawns > cur.spawns => best = Some(stat),
                None => best = Some(stat),
                _ => {}
            }
        }
        // 全零（尚无采样）时输出 "Unknown",不伪造归因
        match best {
            Some(stat) if stat.spawns > 0 => format!("{:?}", stat.kind),
            _ => "Unknown".to_string(),
        }
    }

    /// 某一类型的卸载次数（避开 `HashMap<TaskKind, _>` —— `TaskKind` 未 derive Hash）
    #[must_use]
    pub fn spawns_of(&self, kind: TaskKind) -> u64 {
        self.spawns[kind_index(kind)].load(Ordering::Relaxed)
    }

    /// 某一类型的池内 panic 次数
    #[must_use]
    pub fn failures_of(&self, kind: TaskKind) -> u64 {
        self.failures[kind_index(kind)].load(Ordering::Relaxed)
    }
}

/// 占用率千分比 —— `active / threads × 1000`,以 `f64` 作中间值（u64 大数百分比红线）
///
/// # 契约
/// - `threads` 取 `max(1)` 兜底:池构造期已有 `MIN_THREADS = 2` 下限,此处只为让
///   纯函数在测试中可对任意输入求值,不掩盖运行期异常。
/// - 返回 `u32`,可超过 1000（active 大于线程数即排队积压,正是我们关心的信号）。
#[must_use]
pub fn saturation_ratio_bp(active: usize, threads: usize) -> u32 {
    let threads = threads.max(1) as f64;
    let ratio_bp = (active as f64 / threads) * 1_000.0;
    // 饱和只关心"大于等于阈值"方向,溢出 u32 的极端值钳到上限即可,不引入回绕
    ratio_bp.clamp(0.0, u32::MAX as f64) as u32
}

/// 解析 env 阈值为千分比 —— 纯函数,便于属性测试覆盖非法输入
///
/// 非法/越界（不在 [`THRESHOLD_BP_RANGE`] 内或非数字）一律回落默认值:
/// WHY fail-safe:阈值配错不该让池"永不降级"（如 0）或"永远降级"。
#[must_use]
pub fn parse_threshold_bp(raw: Option<&str>) -> u32 {
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return DEFAULT_SATURATION_BP;
    };
    match text.parse::<u32>() {
        Ok(bp) if THRESHOLD_BP_RANGE.contains(&bp) => bp,
        _ => DEFAULT_SATURATION_BP,
    }
}

/// 生效阈值千分比（进程生命周期内读取 env 一次,不在热路径 getenv）
#[must_use]
pub fn threshold_bp() -> u32 {
    static CACHED: OnceLock<u32> = OnceLock::new();
    *CACHED.get_or_init(|| parse_threshold_bp(std::env::var(ENV_SATURATION_BP).ok().as_deref()))
}

/// 生效的饱和处置策略（进程生命周期内读 env 一次,不在热路径 getenv）
///
/// WHY 默认 Observe：“是否改走 Inline/报错”是行为变更，必须由运维显式开启；
/// 先观测再拿拐点数据，才能判断降级到底有没有收益（ADR-197 D2 与 A1 风险缓解）。
#[must_use]
pub fn saturation_mode() -> SaturationMode {
    static CACHED: OnceLock<SaturationMode> = OnceLock::new();
    *CACHED.get_or_init(|| {
        parse_saturation_mode(
            std::env::var(ENV_SATURATION_MODE).ok().as_deref(),
            std::env::var(ENV_STRICT_POOL).ok().as_deref(),
        )
    })
}

/// 上报判定（纯函数）—— 饱和且降级计数有变化才发事件
///
/// WHY 抽成函数而不是内联在循环里:去抖逻辑是本模块唯一有"状态错乱风险"的
/// 地方（漏报 = 不可观测，风暴 = 刷屏），必须能被确定性地单测覆盖，
/// 而不是靠真池占用 + 定时等待（慢机上必飘）。
///
/// # 参数
/// - `ratio_bp` / `threshold_bp`:当前占用与生效阈值（千分比）；
/// - `degraded`:当前累计降级数（L1 真值源）；
/// - `previous`:上次已上报的降级数；`u64::MAX` 表示尚未报过（复位哨兵）。
#[must_use]
pub fn report_needed(ratio_bp: u32, threshold_bp: u32, degraded: u64, previous: u64) -> bool {
    if ratio_bp < threshold_bp {
        return false;
    }
    // 首次超阈（哨兵）或计数变化 → 上报；同一超阈期内计数未变 → 静默
    previous == u64::MAX || previous != degraded
}

/// 饱和观测上报器句柄（由装配层 `experience_loop` 启动,与吞吐/拦截率上报器同族）
///
/// # 为何本函数只驱动周期
/// 采样、双口径累计与"该不该发"全部收敛在
/// [`UtilizationSampler::sample_and_report`](super::utilization::UtilizationSampler::sample_and_report)
/// —— 同一判定两处实现是漏修的头号入口（本仓曾因 publish/publish_blocking 双份实现
/// 踩过一次，见 `bus.rs` 的宏形式注释）。
pub fn spawn_saturation_reporter(bus: EventBus, period_secs: u64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let period = Duration::from_secs(period_secs.max(1));
        static SAMPLER: OnceLock<UtilizationSampler> = OnceLock::new();
        let sampler = SAMPLER.get_or_init(UtilizationSampler::default);
        loop {
            tokio::time::sleep(period).await;
            // 去抖与发布在采样器内完成;返回值仅用于日志与后续指标导出
            sampler.sample_and_report(&bus);
            // B1/ADR-198:写表也只在低频周期发生（非 apply 模式时为 no-op，
            // 不把“改变运行期阈值”这种治理动作放进卸载热路径）
            let applied = calibration::flush(super::bridge::bridge());
            if applied > 0 {
                tracing::info!(applied, "HTS 阈值表由 SPRT 校准写入（apply 模式）");
            }
            // 校准证据输出：每 kind 一行（阈值校准是否已发生必须可审计，而不是
            // 只能靠“事后抽库”发现——本仓 Phase A 的教训就是静默无输出等于无守护）
            for stat in calibration::snapshot() {
                tracing::debug!(
                    kind = ?stat.kind,
                    inline_samples = stat.inline_samples,
                    pool_samples = stat.pool_samples,
                    decided = ?stat.decided,
                    candidate_threshold = stat.candidate_threshold,
                    applied = stat.applied,
                    mode = ?calibration::mode(),
                    "compute calibration evidence"
                );
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    #[test]
    fn ratio_uses_f64_and_never_wraps() {
        assert_eq!(saturation_ratio_bp(1, 2), 500);
        assert_eq!(saturation_ratio_bp(2, 2), 1000);
        // 积压方向可超 1000,正是饱和信号
        assert_eq!(saturation_ratio_bp(3, 2), 1500);
        // threads=0 不 panic(兜底 max(1))
        assert_eq!(saturation_ratio_bp(5, 0), 5000);
        // usize::MAX 钳制到 u32 上限,不回绕
        assert_eq!(saturation_ratio_bp(usize::MAX, 1), u32::MAX);
    }

    #[test]
    fn parse_threshold_fail_safe_on_illegal_input() {
        assert_eq!(parse_threshold_bp(None), DEFAULT_SATURATION_BP);
        assert_eq!(parse_threshold_bp(Some("")), DEFAULT_SATURATION_BP);
        assert_eq!(parse_threshold_bp(Some("  ")), DEFAULT_SATURATION_BP);
        assert_eq!(parse_threshold_bp(Some("abc")), DEFAULT_SATURATION_BP);
        // 0 = "永不降级" —— 必须回落默认值（fail-safe 核心用例）
        assert_eq!(parse_threshold_bp(Some("0")), DEFAULT_SATURATION_BP);
        assert_eq!(parse_threshold_bp(Some("99")), DEFAULT_SATURATION_BP);
        // 第三轮 T2.3 收窄区间至 100..=1000:>1000 与占用比不可能超过它等价
        // （与 0=永不降级 同危险的静默失效），新上限外一律回落默认。
        assert_eq!(parse_threshold_bp(Some("1001")), DEFAULT_SATURATION_BP);
        assert_eq!(parse_threshold_bp(Some("1500")), DEFAULT_SATURATION_BP);
        assert_eq!(parse_threshold_bp(Some("10001")), DEFAULT_SATURATION_BP);
        assert_eq!(parse_threshold_bp(Some("10000")), DEFAULT_SATURATION_BP);
        // 合法边界与典型值
        assert_eq!(parse_threshold_bp(Some("100")), 100);
        assert_eq!(parse_threshold_bp(Some("900")), 900);
        assert_eq!(parse_threshold_bp(Some(" 1000 ")), 1_000); // 新上边界
    }

    /// 去抖判定：首次超阈报、期内无变化不报、计数变化再报、回落不报
    #[test]
    fn report_needed_debounce_matrix() {
        // 首次超阈（哨兵）
        assert!(report_needed(1_000, 900, 3, u64::MAX));
        // 同一超阈期内计数未变 → 不重复发
        assert!(!report_needed(1_000, 900, 3, 3));
        // 计数变化 → 再发
        assert!(report_needed(1_000, 900, 4, 3));
        // 未达阈值 → 一律不发（即使计数在动）
        assert!(!report_needed(800, 900, 9, u64::MAX));
        // 等号边界：恰好等于阈值算饱和（与 is_saturated 的 >= 语义一致）
        assert!(report_needed(900, 900, 0, u64::MAX));
    }

    /// 处置策略解析表：三态合法值、非法值与旧严格开关兼容
    #[test]
    fn parse_saturation_mode_table() {
        use SaturationMode::{Degrade, Observe, Strict};
        assert_eq!(
            parse_saturation_mode(None, None),
            Observe,
            "默认必须是不改分支的 Observe"
        );
        assert_eq!(parse_saturation_mode(Some("observe"), None), Observe);
        assert_eq!(
            parse_saturation_mode(Some("DEGRADE"), None),
            Degrade,
            "大小写不敏感"
        );
        assert_eq!(parse_saturation_mode(Some(" inline "), None), Degrade);
        assert_eq!(parse_saturation_mode(Some("strict"), None), Strict);
        assert_eq!(parse_saturation_mode(Some("error"), None), Strict);
        // 非法值不猜意图:回到零行为变更默认态
        assert_eq!(parse_saturation_mode(Some("yes-please"), None), Observe);
        assert_eq!(parse_saturation_mode(Some(""), None), Observe);
        // 旧开关仍生效,且仅当新变量未设置时
        assert_eq!(parse_saturation_mode(None, Some("1")), Strict);
        assert_eq!(parse_saturation_mode(None, Some("true")), Strict);
        assert_eq!(parse_saturation_mode(None, Some("0")), Observe);
        assert_eq!(
            parse_saturation_mode(Some("degrade"), Some("1")),
            Degrade,
            "新变量优先于旧变量"
        );
    }

    #[test]
    fn counters_track_per_kind_and_hottest_is_deterministic() {
        let counters = KindCounters::default();
        counters.record_spawn(TaskKind::Generic);
        counters.record_spawn(TaskKind::Generic);
        counters.record_spawn(TaskKind::OsaMask);
        counters.record_failure(TaskKind::OsaMask);
        counters.record_degradation();
        counters.record_saturation();
        counters.record_rejection();

        let map_ok = (
            counters.spawns_of(TaskKind::Generic),
            counters.spawns_of(TaskKind::OsaMask),
            counters.spawns_of(TaskKind::ClvSimilarity),
        );
        assert_eq!(map_ok, (2, 1, 0));
        assert_eq!(counters.failures_of(TaskKind::OsaMask), 1);
        assert_eq!(counters.degradations(), 1);
        assert_eq!(counters.rejections(), 1);
        // Generic 计数更高 → 归因给它
        assert_eq!(counters.hottest_kind(), "Generic");

        let snap = counters.snapshot();
        assert_eq!(snap.len(), KIND_SLOTS);
        let osa = snap.iter().find(|s| s.kind == TaskKind::OsaMask).unwrap();
        assert_eq!(osa.failures, 1);
    }

    #[test]
    fn hottest_kind_unknown_when_no_samples() {
        let counters = KindCounters::default();
        assert_eq!(counters.hottest_kind(), "Unknown");
    }

    #[test]
    fn snapshot_covers_every_kind_exactly_once() {
        let counters = KindCounters::default();
        let snap = counters.snapshot();
        assert_eq!(snap.len(), KIND_SLOTS);
        for kind in TaskKind::ALL {
            assert_eq!(
                snap.iter().filter(|s| s.kind == kind).count(),
                1,
                "{kind:?} 必须且只能出现一次"
            );
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// 占用率对 active 单调不减(固定 threads)
        ///
        /// WHY 先 `let` 再断言:`prop_assert!` 会把表达式文本化并进格式串,
        /// 含 `{}` 的 if/else 会被误读为占位符（invalid format string）。
        #[test]
        fn ratio_monotone_in_active(a in 0usize..1_000_000, b in 0usize..1_000_000, t in 1usize..64) {
            let ra = saturation_ratio_bp(a, t);
            let rb = saturation_ratio_bp(b, t);
            let ok = if a <= b { ra <= rb } else { ra >= rb };
            prop_assert!(ok);
        }

        /// 占用率对 threads 单调不增(active 固定,且 threads>=1)
        #[test]
        fn ratio_antitone_in_threads(a in 0usize..100_000, t1 in 1usize..64, t2 in 1usize..64) {
            let lo = saturation_ratio_bp(a, t1.min(t2));
            let hi = saturation_ratio_bp(a, t1.max(t2));
            prop_assert!(lo >= hi);
        }

        /// 零 active 恒为 0,零 threads 不 panic
        #[test]
        fn zero_active_is_zero(threads in 0usize..256) {
            prop_assert_eq!(saturation_ratio_bp(0, threads), 0);
        }

        /// 阈值解析永远落在安全区间或回落默认值,绝不接受"0=永不降级"
        #[test]
        fn threshold_never_zero_or_huge(raw in proptest::option::of(0u32..20_001)) {
            let text = raw.map(|v| v.to_string());
            let bp = parse_threshold_bp(text.as_deref());
            prop_assert!(bp == DEFAULT_SATURATION_BP || THRESHOLD_BP_RANGE.contains(&bp));
        }
    }
}
