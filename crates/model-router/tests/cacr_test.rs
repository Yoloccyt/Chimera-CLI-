//! CACR 大预算精度回归测试
//!
//! 验证 `CacrGuard::check` 在 remaining_budget 超过 2^24 后仍使用整数运算，
//! 避免因 f32 精度丢失导致阈值判定错误。

use model_router::{CacrConfig, CacrDecision, CacrGuard};

#[test]
fn test_large_budget_no_precision_loss() {
    // WHY: f32 只有 24 位有效尾数，无法精确表示所有大于 2^24(16,777,216) 的整数。
    // 当预算达到 2^25 量级时，如 2^25+1 的值会被 f32 四舍五入，导致
    // `remaining_budget as f32 * 0.8` 产生一美分级的阈值误差。
    // 本测试使用 2^25+1 的预算和略低于精确 warn_limit 的成本，验证整数运算
    // 不会错误地触发 Downgrade。
    let guard = CacrGuard::new(CacrConfig {
        budget_limit: 1 << 25,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    });

    let remaining_budget = (1 << 25) + 1; // 33,554,433
                                          // 精确 warn_limit = floor(33,554,433 * 0.8) = 26,843,546
    let warn_limit = remaining_budget * 80 / 100;
    let cost = warn_limit - 1; // 26,843,545，严格小于 warn_limit，应 Allow

    let decision = guard.check(cost, remaining_budget);
    assert_eq!(
        decision,
        CacrDecision::Allow,
        "预算 2^25+1、成本比精确 warn_limit 低 1 时应 Allow；\
         若使用 f32 精度丢失，会错误地触发 Downgrade"
    );
}
