#![forbid(unsafe_code)]

//! Task 21(RED): INV-7 / INV-8 不变量检查器失败测试
//!
//! 对应设计文档 §21.2 + §15.4(INV-7)+ §17.5(INV-8)。
//! 覆盖 4 类场景(共 12 个测试):
//! 1. INV-7 单 Agent 预算界(4 个)— 驻留 ≤ / > effective_capacity 边界
//! 2. INV-7 全局内存预算(3 个)— M_total ≤ / > 130MB×0.9 边界
//! 3. INV-8 归档单调性(4 个)— Hot→Warm→Cold→Ice 单向降级 + 反向/同层拒绝
//! 4. ArchiveTier 辅助方法(1 个)— level() 返回 0/1/2/3
//!
//! TDD RED 阶段:本文件引用尚未实现的 API(`InvariantChecker::check_inv7_context_budget` /
//! `check_inv8_archive_monotonicity` / `ArchiveTier`),编译失败为预期。
//! GREEN 阶段实现 `src/invariants.rs` 后全部通过。
//!
//! ## 红线对齐
//!
//! - §4.1: u64 大数百分比用 f64 中间值(非 f32,避免精度膨胀)
//! - §4.4 反模式 6: f32 禁止隐式转 f64 比较,全程 f64
//! - §6.1: 单函数 ≤ 200 行
//! - §15.4: Token 超限复用 `MasError::TokenBudgetExceeded`
//! - §17.5: 反向膨胀返回 `MasError::ArchiveMonotonicityViolated`
//! - `#![forbid(unsafe_code)]`: 纯计算,无 unsafe 需求

use chimera_mas::invariants::{
    ArchiveTier, InvariantChecker, MEMORY_BUDGET_MB, MEMORY_BUDGET_UTILIZATION,
};
use chimera_mas::MasError;

// ============================================================
// 1. INV-7 单 Agent 预算界(4 个测试)
// ============================================================

/// INV-7 边界:agent_resident == effective_capacity 时应通过(等号允许)
///
/// 语义(§15.4):"实际驻留 Token ≤ effective_capacity" — 等号表示恰好满载,合法。
#[test]
fn inv7_agent_resident_equal_to_capacity_is_ok() {
    let result = InvariantChecker::check_inv7_context_budget(128_000, 128_000, 0, 130);
    assert!(result.is_ok(), "驻留 == 容量应通过(等号允许)");
}

/// INV-7 边界:agent_resident < effective_capacity 时应通过(尚未满载)
#[test]
fn inv7_agent_resident_below_capacity_is_ok() {
    let result = InvariantChecker::check_inv7_context_budget(64_000, 128_000, 0, 130);
    assert!(result.is_ok(), "驻留 < 容量应通过");
}

/// INV-7 超限:agent_resident > effective_capacity 时返回 TokenBudgetExceeded
///
/// 这是 INV-7 的核心断言:单 Agent 驻留不能超过 tier 容量上限。
#[test]
fn inv7_agent_resident_exceeds_capacity_returns_error() {
    let result = InvariantChecker::check_inv7_context_budget(200_000, 128_000, 0, 130);
    let err = result.expect_err("驻留 > 容量应返回错误");
    assert!(
        matches!(err, MasError::TokenBudgetExceeded { .. }),
        "INV-7 超限应返回 TokenBudgetExceeded,实际: {err:?}"
    );
}

/// INV-7 全局约束满足时单 Agent 超限仍报错(单 Agent 约束优先级独立)
///
/// 验证 INV-7 的两个约束是 AND 关系:任一不满足即拒绝。
#[test]
fn inv7_global_ok_but_agent_exceeds_still_rejects() {
    // 全局 M_total=50MB ≤ 130×0.9=117MB,但单 Agent 超限
    let result = InvariantChecker::check_inv7_context_budget(200_000, 128_000, 50, 130);
    assert!(
        matches!(result, Err(MasError::TokenBudgetExceeded { .. })),
        "单 Agent 超限时即使全局满足也应返回 TokenBudgetExceeded"
    );
}

// ============================================================
// 2. INV-7 全局内存预算(3 个测试)
// ============================================================

/// INV-7 全局边界:M_total == 130×0.9=117MB 时应通过(等号允许)
#[test]
fn inv7_global_at_boundary_is_ok() {
    let m_budget = 130; // 130MB
                        // M_total = 130 × 0.9 = 117MB,恰好等于阈值
    let m_total = (m_budget as f64 * 0.9) as usize;
    let result = InvariantChecker::check_inv7_context_budget(0, 128_000, m_total, m_budget);
    assert!(result.is_ok(), "M_total == budget×0.9 应通过(等号允许)");
}

/// INV-7 全局超限:M_total > 130×0.9 时返回 TokenBudgetExceeded
///
/// 语义(§15.3):"派生准入闸 — 预计 M_total ≤ 130MB×0.9 方可派生新 Agent"
#[test]
fn inv7_global_exceeds_budget_returns_error() {
    // M_total = 118MB > 117MB(130×0.9)
    let result = InvariantChecker::check_inv7_context_budget(0, 128_000, 118, 130);
    let err = result.expect_err("M_total > budget×0.9 应返回错误");
    assert!(
        matches!(err, MasError::TokenBudgetExceeded { .. }),
        "INV-7 全局超限应返回 TokenBudgetExceeded,实际: {err:?}"
    );
}

/// INV-7 常量验证:MEMORY_BUDGET_MB=130, MEMORY_BUDGET_UTILIZATION=0.9
///
/// 防止常量被意外修改导致 INV-7 阈值漂移。
#[test]
fn inv7_memory_budget_constants_are_stable() {
    assert_eq!(MEMORY_BUDGET_MB, 130, "MEMORY_BUDGET_MB 应为 130(§15.3)");
    assert_eq!(
        MEMORY_BUDGET_UTILIZATION, 0.9,
        "MEMORY_BUDGET_UTILIZATION 应为 0.9(§15.3 派生准入闸)"
    );
    // 验证阈值计算:130 × 0.9 = 117MB
    let threshold = (MEMORY_BUDGET_MB as f64 * MEMORY_BUDGET_UTILIZATION) as usize;
    assert_eq!(threshold, 117, "130 × 0.9 应为 117MB");
}

// ============================================================
// 3. INV-8 归档单调性(4 个测试)
// ============================================================

/// INV-8 合法路径:Hot→Warm→Cold→Ice 全链路单向降级均通过
///
/// 验证四个连续的降级步骤都返回 Ok。
#[test]
fn inv8_monotonic_demotion_chain_hot_to_ice_all_ok() {
    assert!(
        InvariantChecker::check_inv8_archive_monotonicity(ArchiveTier::Hot, ArchiveTier::Warm)
            .is_ok()
    );
    assert!(InvariantChecker::check_inv8_archive_monotonicity(
        ArchiveTier::Warm,
        ArchiveTier::Cold
    )
    .is_ok());
    assert!(
        InvariantChecker::check_inv8_archive_monotonicity(ArchiveTier::Cold, ArchiveTier::Ice)
            .is_ok()
    );
    // 跨级降级(Hot→Ice)也应合法
    assert!(
        InvariantChecker::check_inv8_archive_monotonicity(ArchiveTier::Hot, ArchiveTier::Ice)
            .is_ok()
    );
}

/// INV-8 反向操作:Ice→Hot / Cold→Warm / Warm→Hot 均返回 ArchiveMonotonicityViolated
///
/// 这是 INV-8 的核心断言:记忆不可反向膨胀。
#[test]
fn inv8_reverse_promotion_returns_error() {
    let cases = [
        (ArchiveTier::Ice, ArchiveTier::Hot, "Ice→Hot"),
        (ArchiveTier::Ice, ArchiveTier::Warm, "Ice→Warm"),
        (ArchiveTier::Ice, ArchiveTier::Cold, "Ice→Cold"),
        (ArchiveTier::Cold, ArchiveTier::Warm, "Cold→Warm"),
        (ArchiveTier::Cold, ArchiveTier::Hot, "Cold→Hot"),
        (ArchiveTier::Warm, ArchiveTier::Hot, "Warm→Hot"),
    ];
    for (from_tier, to_tier, label) in cases {
        let result = InvariantChecker::check_inv8_archive_monotonicity(from_tier, to_tier);
        match result {
            Err(MasError::ArchiveMonotonicityViolated {
                from_tier: ft,
                to_tier: tt,
            }) => {
                // 验证错误字段正确反映 tier 名
                let expected_from = format!("{from_tier:?}");
                let expected_to = format!("{to_tier:?}");
                assert_eq!(
                    ft, expected_from,
                    "{label}: from_tier 字段错误,期望 {expected_from},实际 {ft}"
                );
                assert_eq!(
                    tt, expected_to,
                    "{label}: to_tier 字段错误,期望 {expected_to},实际 {tt}"
                );
            }
            Err(other_err) => {
                panic!("{label}: 期望 ArchiveMonotonicityViolated,实际返回 {other_err:?}")
            }
            Ok(()) => panic!("{label}: 反向操作应被拒绝,实际通过"),
        }
    }
}

/// INV-8 同层操作:Hot→Hot / Warm→Warm / Cold→Cold / Ice→Ice 均返回错误
///
/// 语义(§17.5):"单向降级" — 同层不属于降级,应拒绝。
/// 防止无意义归档操作(如重复归档到同 tier)造成元数据抖动。
#[test]
fn inv8_same_tier_returns_error() {
    let tiers = [
        ArchiveTier::Hot,
        ArchiveTier::Warm,
        ArchiveTier::Cold,
        ArchiveTier::Ice,
    ];
    for tier in tiers {
        let result = InvariantChecker::check_inv8_archive_monotonicity(tier, tier);
        assert!(
            matches!(result, Err(MasError::ArchiveMonotonicityViolated { .. })),
            "{tier:?}→{tier:?}: 同层操作应返回 ArchiveMonotonicityViolated"
        );
    }
}

/// INV-8 边界:Hot→Warm(刚好合法)+ Ice→Hot(刚好非法,跨3级反向)
#[test]
fn inv8_boundary_cases() {
    // Hot→Warm:level 0→1,合法
    assert!(
        InvariantChecker::check_inv8_archive_monotonicity(ArchiveTier::Hot, ArchiveTier::Warm)
            .is_ok()
    );
    // Ice→Hot:level 3→0,非法(跨3级反向)
    assert!(matches!(
        InvariantChecker::check_inv8_archive_monotonicity(ArchiveTier::Ice, ArchiveTier::Hot),
        Err(MasError::ArchiveMonotonicityViolated { .. })
    ));
}

// ============================================================
// 4. ArchiveTier 辅助方法(1 个测试)
// ============================================================

/// ArchiveTier::level() 返回 0/1/2/3,严格 Hot<Warm<Cold<Ice
///
/// INV-8 的单调性判断依据 level() 比较,需确保 level 值严格递增。
#[test]
fn archive_tier_level_returns_strictly_increasing_values() {
    assert_eq!(ArchiveTier::Hot.level(), 0, "Hot.level() 应为 0");
    assert_eq!(ArchiveTier::Warm.level(), 1, "Warm.level() 应为 1");
    assert_eq!(ArchiveTier::Cold.level(), 2, "Cold.level() 应为 2");
    assert_eq!(ArchiveTier::Ice.level(), 3, "Ice.level() 应为 3");
    // 严格递增验证
    assert!(ArchiveTier::Hot.level() < ArchiveTier::Warm.level());
    assert!(ArchiveTier::Warm.level() < ArchiveTier::Cold.level());
    assert!(ArchiveTier::Cold.level() < ArchiveTier::Ice.level());
}
