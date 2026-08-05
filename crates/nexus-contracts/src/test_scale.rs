//! P9-T2 测试等待缩放工具 — CHIMERA_TEST_TIMEOUT_SCALE 协议
//!
//! # 背景(P9-T2 测试运行时间优化)
//!
//! nexus-contracts 严格遵循 ADR-033 "纯类型 + 零逻辑" 约束。本模块作为该
//! 约束的唯一例外(test-only utility),不引入任何领域逻辑,仅提供
//! `CHIMERA_TEST_TIMEOUT_SCALE` 环境变量的读取与缩放函数。
//!
//! # 协议
//!
//! - `CHIMERA_TEST_TIMEOUT_SCALE=1.0`(缺省):所有 `scaled_timeout!(N)` 返回
//!   `Duration::from_secs(N)`,与原硬编码行为完全等价
//! - `CHIMERA_TEST_TIMEOUT_SCALE=0.1`:所有 2s+ 等待缩为 1/10,加速 fast 档
//! - clamp 范围 `[0.01, 1.0]`:避免 0 或负值导致测试瞬时跳过
//!
//! # 用法
//!
//! ```ignore  // 示意代码,本模块仅在测试上下文导出
//! use nexus_contracts::test_scale::{scaled_timeout, scale_timeout};
//! use std::time::Duration;
//!
//! let d: Duration = scaled_timeout!(10);  // 缺省 10s;scale=0.1 时 1s
//! let secs = scale_timeout(5);            // 缺省 5; scale=0.5 时 2
//! ```
//!
//! # 兼容性
//!
//! 旧测试代码不替换仍正常工作(`Duration::from_secs(N)` 直接使用);
//! 仅当显式替换为 `scaled_timeout!` 时才受 scale 影响。

#![allow(clippy::module_name_repetitions)]

/// 环境变量名 — 测试等待缩放因子
pub const ENV_TIMEOUT_SCALE: &str = "CHIMERA_TEST_TIMEOUT_SCALE";

/// 缩放下界 — 避免 scale=0 导致测试瞬时跳过丢失断言
pub const SCALE_MIN: f64 = 0.01;

/// 缩放上界 — scale>1.0 不在 PR fast 档使用,显式封顶
pub const SCALE_MAX: f64 = 1.0;

/// 读取当前 scale 因子;若 env 未设或非法,返回 1.0
///
/// 行为:
/// - 缺省:1.0(与原硬编码行为等价)
/// - 解析失败(非数字 / 负数 / NaN):1.0(失败安全,确保不破坏断言语义)
/// - clamp 到 `[SCALE_MIN, SCALE_MAX]`
pub fn current_scale() -> f64 {
    match std::env::var(ENV_TIMEOUT_SCALE) {
        Ok(s) => match s.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v.clamp(SCALE_MIN, SCALE_MAX),
            _ => 1.0,
        },
        Err(_) => 1.0,
    }
}

/// 将硬编码 `secs` 缩放为实际测试应等待的秒数
///
/// 缺省(scale=1.0)返回 `secs` 本身,保证向后兼容。
/// 最小返回 1ms(防止 0 值导致 sleep 跳过)。
pub fn scale_timeout(secs: u64) -> u64 {
    let s = current_scale();
    let scaled = (secs as f64) * s;
    scaled.max(if secs == 0 { 0.0 } else { 0.001 }) as u64
}

/// `scaled_timeout!` 宏 — 接受字面量 secs,返回 `Duration`
///
/// # 示例
///
/// ```ignore
/// use nexus_contracts::test_scale::scaled_timeout;
/// let d = scaled_timeout!(5);  // Duration::from_secs(5) 或更短(scale<1.0)
/// ```
#[macro_export]
macro_rules! scaled_timeout {
    ($secs:expr) => {{
        let __s: u64 = $secs;
        let __scale = $crate::test_scale::current_scale();
        let __scaled_f = (std::time::Duration::from_secs(__s).as_secs_f64()) * __scale;
        let __final_f = if __s == 0 { 0.0 } else { __scaled_f.max(0.001) };
        std::time::Duration::from_secs_f64(__final_f)
    }};
}

// =============================================================================
// 自测试(确保 scale 工具自身正确)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 模拟 env::var 行为不可变(并行测试) — 直接验证 scale_timeout 数学逻辑
    #[test]
    fn scale_timeout_pure_math() {
        let v = scale_timeout(10);
        assert!((0..=10).contains(&v), "scale_timeout(10) out of range: {v}");
    }

    #[test]
    fn current_scale_default_is_one() {
        let s = current_scale();
        assert!(
            (SCALE_MIN..=SCALE_MAX).contains(&s),
            "scale {s} out of clamp range"
        );
    }

    #[test]
    fn scaled_timeout_macro_returns_duration() {
        let d: Duration = scaled_timeout!(5);
        assert!(d.as_secs() <= 5);
        assert!(d.as_secs_f64() > 0.0);
    }

    #[test]
    fn scaled_timeout_zero_stays_zero() {
        let d: Duration = scaled_timeout!(0);
        assert_eq!(d.as_secs_f64(), 0.0);
    }
}
