//! 共享衰减公式 — 为 L3 cmt-tiering 与 L4 decay-engine 提供统一的数学基础
//!
//! 对应架构层:L1 Core(被 L3/L4 向下依赖)
//!
//! # 设计决策(WHY)
//!
//! ## 问题:两个 crate 各自实现衰减公式
//! - cmt-tiering(L3):`priority = access_count × exp(-Δt / τ)` — 指数衰减
//! - decay-engine(L4):`level -= elapsed × rate` — 线性衰减
//!
//! 两者语义不同(记忆层级迁移 vs 安全权限衰减),但**指数衰减公式本身**
//! 是纯数学函数,无副作用,可作为 L1 共享基础设施。
//!
//! ## 统一方案(P1-4)
//! 1. **本模块**:提供 `exponential_decay_factor` 纯函数(指数衰减因子计算)
//! 2. **cmt-tiering**:重构 `DecayCalculator` 使用本模块的 `exponential_decay_factor`
//! 3. **decay-engine**:新增 `use_exponential_decay` 配置开关,启用时用指数衰减替代线性
//!
//! ## 指数衰减 vs 线性衰减
//! - **指数衰减**:`value × exp(-Δt / τ)` — 自然衰减现象的标准模型
//!   - 优点:自动收敛到 0,不会为负;初始衰减快,符合"遗忘曲线"
//!   - 适用:cmt-tiering 记忆优先级(默认)
//! - **线性衰减**:`value - elapsed × rate` — 简单可预测
//!   - 优点:行为直观,安全审计友好(每秒钟减少固定量)
//!   - 适用:decay-engine 安全权限衰减(默认,向后兼容)
//!
//! # 性能
//! - 纯数学运算,无 I/O,无内存分配,< 100ns
//! - `exp()` 调用在现代 CPU 上约 50-100 个时钟周期

/// 计算指数衰减因子:exp(-Δt / τ)
///
/// 公式:`decay_factor = e^(-delta_seconds / tau_seconds)`
///
/// # 参数
/// - `delta_seconds`:距上次事件的时间(秒),必须 ≥ 0
/// - `tau_seconds`:衰减时间常数 τ(秒),必须 > 0
///
/// # 返回
/// 衰减因子 [0.0, 1.0]:
/// - Δt = 0 时返回 1.0(无衰减)
/// - Δt = τ 时返回 1/e ≈ 0.3679(衰减到 37%)
/// - Δt → ∞ 时趋向 0.0(完全衰减)
///
/// # 使用示例
/// ```
/// use nexus_core::decay::exponential_decay_factor;
///
/// // 24 小时后的衰减因子
/// let factor = exponential_decay_factor(86400.0, 86400.0);
/// assert!((factor - 0.36787944).abs() < 1e-6);
///
/// // 刚发生的事件(Δt=0)不衰减
/// assert!((exponential_decay_factor(0.0, 3600.0) - 1.0).abs() < 1e-6);
/// ```
///
/// # Panics
/// 不会 panic。`delta_seconds < 0` 时内部 clamp 为 0(容忍时钟漂移)。
/// `tau_seconds <= 0` 时返回 1.0(无衰减,降级处理)。
#[inline]
pub fn exponential_decay_factor(delta_seconds: f64, tau_seconds: f64) -> f64 {
    // 防御:Δt 为负(时钟漂移)按 0 处理
    let delta = delta_seconds.max(0.0);
    // 防御:τ ≤ 0 时降级为无衰减(避免除零/负指数)
    if tau_seconds <= 0.0 {
        return 1.0;
    }
    (-delta / tau_seconds).exp()
}

/// 应用指数衰减后的值:value × exp(-Δt / τ)
///
/// 适用于"初始值 × 衰减因子"的场景:
/// - cmt-tiering:`priority = access_count × decay_factor`
/// - decay-engine(指数模式):`level = level × decay_factor`
///
/// # 参数
/// - `value`:初始值
/// - `delta_seconds`:距上次事件的时间(秒)
/// - `tau_seconds`:衰减时间常数 τ(秒)
///
/// # 返回
/// 衰减后的值:value × exp(-Δt / τ)
#[inline]
pub fn apply_exponential_decay(value: f64, delta_seconds: f64, tau_seconds: f64) -> f64 {
    value * exponential_decay_factor(delta_seconds, tau_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // exponential_decay_factor 测试
    // ============================================================

    #[test]
    fn test_factor_zero_delta() {
        // Δt = 0 时因子为 1.0
        assert!((exponential_decay_factor(0.0, 86400.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_factor_one_tau() {
        // Δt = τ 时因子为 1/e ≈ 0.3679
        let factor = exponential_decay_factor(86400.0, 86400.0);
        assert!((factor - 0.36787944).abs() < 1e-4);
    }

    #[test]
    fn test_factor_three_tau() {
        // Δt = 3τ 时因子为 e^(-3) ≈ 0.0498
        let factor = exponential_decay_factor(259200.0, 86400.0);
        assert!((factor - 0.04978707).abs() < 1e-4);
    }

    #[test]
    fn test_factor_large_delta() {
        // Δt 很大时因子趋向 0
        let factor = exponential_decay_factor(1_000_000.0, 100.0);
        assert!(factor < 1e-10);
    }

    #[test]
    fn test_factor_negative_delta_clamped() {
        // 负 Δt 容忍为 0(时钟漂移)
        let factor = exponential_decay_factor(-100.0, 86400.0);
        assert!((factor - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_factor_zero_tau_fallback() {
        // τ = 0 时降级返回 1.0(无衰减)
        let factor = exponential_decay_factor(100.0, 0.0);
        assert!((factor - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_factor_negative_tau_fallback() {
        // τ < 0 时降级返回 1.0
        let factor = exponential_decay_factor(100.0, -1.0);
        assert!((factor - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_factor_small_tau_fast_decay() {
        // τ 小 → 衰减快
        let factor_small = exponential_decay_factor(100.0, 10.0);
        let factor_large = exponential_decay_factor(100.0, 1000.0);
        // τ=10 时 e^(-10) ≈ 0.000045 < τ=1000 时 e^(-0.1) ≈ 0.905
        assert!(factor_small < factor_large);
    }

    // ============================================================
    // apply_exponential_decay 测试
    // ============================================================

    #[test]
    fn test_apply_zero_delta() {
        let result = apply_exponential_decay(100.0, 0.0, 86400.0);
        assert!((result - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_apply_one_tau() {
        // 100 × 1/e ≈ 36.7879
        let result = apply_exponential_decay(100.0, 86400.0, 86400.0);
        assert!((result - 36.787944).abs() < 1e-2);
    }

    #[test]
    fn test_apply_zero_value() {
        let result = apply_exponential_decay(0.0, 100.0, 86400.0);
        assert!(result.abs() < 1e-6);
    }
}
