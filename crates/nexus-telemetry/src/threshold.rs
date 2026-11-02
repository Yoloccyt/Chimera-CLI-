//! env 阈值与策略的纯解析 + 进程级缓存（ADR-197）
//!
//! 时间/环境读取在 L0-B 是允许的（ADR-222 D3 的诚实边界）；本模块仍是
//! “进程生命周期只读一次”的形态，不在热路径 getenv。

use std::sync::OnceLock;

use crate::mode::{parse_saturation_mode, SaturationMode};

/// 默认饱和阈值（千分比：900 = 90%）
///
/// WHY 千分比而不是百分比/小数:与 [`crate::saturation_ratio_bp`] 同量纲,
/// 整数比较无浮点误差;1000‰ = 池恰好满载,阈值取 900‰ 留出余量
/// （避免临界抖动）。
pub const DEFAULT_SATURATION_BP: u32 = 900;

/// 合法阈值区间（千分比）：100‰ = 10% 到 1_000‰ = 池恰好满载
///
/// 第三轮 T2.3 收窄（原 100..=10_000）:占用比 `saturation_ratio_bp = active×1000/threads`，
/// 而 `active` 是并发守卫计数（rayon 每线程最多同时跑一个闭包，`enter()` 仅在
/// 闭包被调度时发生），故实际 `active ≤ threads` → 占用比恒 ≤ 1000‰。旧上限
/// 10_000‰ 隐含“排队积压也可超满载”的假设，但队列深度当前不可观测（F-U2），
/// 且 >1000‰ 阈值永不触发——与 fail-safe 拒绝“0=永不降级”同危险（静默失效）。
/// 队列深度真实可观测后属独立 ADR（本轮不做），故先把区间收紧到有语义上限。
pub const THRESHOLD_BP_RANGE: std::ops::RangeInclusive<u32> = 100..=1_000;

/// 饱和阈值 env 变量名（降级路径走配置/env,禁 feature 标志）
const ENV_SATURATION_BP: &str = "CHIMERA_COMPUTE_SATURATION_BP";

/// 饱和处置策略 env 变量名
const ENV_SATURATION_MODE: &str = "CHIMERA_COMPUTE_SATURATION_MODE";

/// 严格模式 env 变量名（历史名，现仅作为 `=strict` 的简写保留）
const ENV_STRICT_POOL: &str = "CHIMERA_COMPUTE_STRICT_POOL";

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
