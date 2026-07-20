//! Task 15 集成测试 — 上下文预算模型 / 派生准入闸 / compression_threshold
//!
//! 覆盖 SubTask:
//! - 15.3: 1M 标准容量(L3 context_window=1_048_576, effective_capacity=131_072, sparse_factor=8)
//! - 15.4: 50 Agent 稳态分布聚合 ≤ 130MB
//! - 15.5: 准入闸失败(M_total + 单 Agent 驻留 > 130MB × 0.9 时拒绝)
//! - 15.6: compression_threshold(90% 触发压缩,Critical 不压缩,Optional 可丢弃)
//!
//! ## 测试策略
//!
//! - 纯计算测试(无 async),用 `#[test]` 而非 `#[tokio::test]`
//! - 命名遵循 block-named 风格(`fn budget_model_l3_xxx_is_yyy`)
//! - 覆盖边界场景(等号允许/超限拒绝)与稳态场景(50 Agent 分布)

use chimera_mas::context::budget_model::{
    should_compress_at, AdmissionGate, ContextTier, MemoryBudgetModel, COMPRESSION_THRESHOLD,
    SPARSE_FACTOR,
};
use chimera_mas::context::ContextPriority;
use chimera_mas::error::MasError;
use chimera_mas::invariants::{MEMORY_BUDGET_MB, MEMORY_BUDGET_UTILIZATION};

// ============================================================
// SubTask 15.3: 1M 标准容量测试
// ============================================================

#[test]
fn budget_model_l3_context_window_is_1m() {
    // 1M = 1_048_576 = 2^20
    assert_eq!(ContextTier::L3.context_window(), 1_048_576);
}

#[test]
fn budget_model_l3_effective_capacity_is_128k() {
    // 128K = 131_072 = 2^17(热工作集上限)
    assert_eq!(ContextTier::L3.effective_capacity(), 131_072);
}

#[test]
fn budget_model_sparse_factor_is_8() {
    // ADR-026 决策 7:1M / 128K = 8
    assert_eq!(SPARSE_FACTOR, 8);
}

#[test]
fn budget_model_l3_window_equals_8x_effective_capacity() {
    // 1M 上下文 = 128K 实际 + 8× 稀疏压缩(Ω-Compress 单一实现原则)
    let l3 = ContextTier::L3;
    assert_eq!(
        l3.context_window(),
        l3.effective_capacity() * SPARSE_FACTOR as usize
    );
}

#[test]
fn budget_model_l0_l1_l2_capacities_match_hcw_standard() {
    // HCW 四级窗口标准:4K / 32K / 128K / 1M
    assert_eq!(ContextTier::L0.context_window(), 4_096);
    assert_eq!(ContextTier::L1.context_window(), 32_768);
    assert_eq!(ContextTier::L2.context_window(), 131_072);
}

#[test]
fn budget_model_l0_l1_l2_no_sparse_effective_equals_window() {
    // L0-L2 不启用稀疏,有效容量 == 窗口(直接加载不会爆内存)
    assert_eq!(ContextTier::L0.effective_capacity(), 4_096);
    assert_eq!(ContextTier::L1.effective_capacity(), 32_768);
    assert_eq!(ContextTier::L2.effective_capacity(), 131_072);
}

#[test]
fn budget_model_only_l3_sparse_enabled() {
    // 仅 L3 启用稀疏压缩(L0-L2 容量 ≤ 128K,无需稀疏)
    assert!(!ContextTier::L0.sparse_enabled());
    assert!(!ContextTier::L1.sparse_enabled());
    assert!(!ContextTier::L2.sparse_enabled());
    assert!(ContextTier::L3.sparse_enabled());
}

#[test]
fn budget_model_agent_context_standard_constants_exposed() {
    // SubTask 15.8:AgentContext 显式声明 1M 标准 + 128K 热工作集 + 8× 稀疏
    use chimera_mas::context::AgentContext;
    assert_eq!(AgentContext::STANDARD_CONTEXT_WINDOW, 1_048_576);
    assert_eq!(AgentContext::STANDARD_EFFECTIVE_CAPACITY, 131_072);
    assert_eq!(AgentContext::SPARSE_FACTOR, 8);
    // 与 budget_model::SPARSE_FACTOR 复用同一真值源
    assert_eq!(AgentContext::SPARSE_FACTOR, SPARSE_FACTOR);
}

// ============================================================
// SubTask 15.4: 50 Agent 稳态分布聚合 ≤ 130MB
// ============================================================

#[test]
fn budget_model_50_agent_steady_state_total_under_130mb() {
    // ADR-026 决策 7:50 Agent 稳态分布(30×L0 + 12×L1 + 5×L2 + 3×L3)聚合 ≤ 130MB
    let model = MemoryBudgetModel::default_model(); // bytes_per_tok = 4
    let mut total_bytes = 0usize;
    for _ in 0..30 {
        total_bytes += model.estimate_resident(ContextTier::L0);
    }
    for _ in 0..12 {
        total_bytes += model.estimate_resident(ContextTier::L1);
    }
    for _ in 0..5 {
        total_bytes += model.estimate_resident(ContextTier::L2);
    }
    for _ in 0..3 {
        total_bytes += model.estimate_resident(ContextTier::L3);
    }
    let total_mb = total_bytes / (1024 * 1024);
    assert!(
        total_mb <= MEMORY_BUDGET_MB,
        "50 Agent 稳态分布 {total_mb}MB 超过预算 {MEMORY_BUDGET_MB}MB"
    );
}

#[test]
fn budget_model_50_agent_steady_state_each_admission_gate_ok() {
    // 50 Agent 稳态分布:每次派生都通过准入闸
    let model = MemoryBudgetModel::default_model();
    let mut m_total = 0usize;
    // 30×L0
    for _ in 0..30 {
        let r = AdmissionGate::check(m_total, ContextTier::L0, model.bytes_per_tok);
        assert!(r.is_ok(), "L0 派生失败 at m_total={m_total}MB: {r:?}");
        m_total += model.estimate_resident_mb(ContextTier::L0);
    }
    // 12×L1
    for _ in 0..12 {
        let r = AdmissionGate::check(m_total, ContextTier::L1, model.bytes_per_tok);
        assert!(r.is_ok(), "L1 派生失败 at m_total={m_total}MB: {r:?}");
        m_total += model.estimate_resident_mb(ContextTier::L1);
    }
    // 5×L2
    for _ in 0..5 {
        let r = AdmissionGate::check(m_total, ContextTier::L2, model.bytes_per_tok);
        assert!(r.is_ok(), "L2 派生失败 at m_total={m_total}MB: {r:?}");
        m_total += model.estimate_resident_mb(ContextTier::L2);
    }
    // 3×L3
    for _ in 0..3 {
        let r = AdmissionGate::check(m_total, ContextTier::L3, model.bytes_per_tok);
        assert!(r.is_ok(), "L3 派生失败 at m_total={m_total}MB: {r:?}");
        m_total += model.estimate_resident_mb(ContextTier::L3);
    }
    // 最终聚合内存应远低于 130MB(实际约 6MB)
    assert!(
        m_total <= MEMORY_BUDGET_MB,
        "50 Agent 最终聚合 {m_total}MB 超过预算 {MEMORY_BUDGET_MB}MB"
    );
}

#[test]
fn budget_model_default_bytes_per_tok_is_4() {
    // 默认 bytes_per_tok = 4(UTF-8 平均)
    let model = MemoryBudgetModel::default_model();
    assert_eq!(model.bytes_per_tok, 4);
}

#[test]
fn budget_model_estimate_resident_l3_is_512kb() {
    // L3 单 Agent 驻留 = 4 bytes × 128K = 512 KB
    let model = MemoryBudgetModel::default_model();
    let resident = model.estimate_resident(ContextTier::L3);
    assert_eq!(resident, 4 * 131_072);
    assert_eq!(resident, 524_288);
    // MB 转换:512KB / 1MB = 0(向下取整)
    assert_eq!(model.estimate_resident_mb(ContextTier::L3), 0);
}

// ============================================================
// SubTask 15.5: 准入闸失败测试
// ============================================================

#[test]
fn admission_gate_threshold_is_117mb() {
    // INV-7 阈值:130 × 0.9 = 117 MB(派生前预留 10% 余量)
    let threshold_mb = (MEMORY_BUDGET_MB as f64 * MEMORY_BUDGET_UTILIZATION) as usize;
    assert_eq!(threshold_mb, 117);
}

#[test]
fn admission_gate_passes_at_threshold_boundary() {
    // 边界:M_total = 117(等号允许,恰好达阈值)
    let result = AdmissionGate::check(117, ContextTier::L3, 4);
    assert!(result.is_ok(), "等号应允许: {result:?}");
}

#[test]
fn admission_gate_denies_when_m_total_exceeds_threshold() {
    // M_total = 118 > 117,应拒绝派生
    let result = AdmissionGate::check(118, ContextTier::L3, 4);
    assert!(
        matches!(result, Err(MasError::AdmissionGateDenied { .. })),
        "应返回 AdmissionGateDenied: {result:?}"
    );
}

#[test]
fn admission_gate_denies_with_correct_diagnostic_fields() {
    // 验证错误字段填充正确(m_total/m_budget/new_agent_tier)
    let result = AdmissionGate::check(120, ContextTier::L3, 4);
    let err = result.unwrap_err();
    match err {
        MasError::AdmissionGateDenied {
            m_total,
            m_budget,
            new_agent_tier,
            reason,
        } => {
            assert_eq!(m_total, 120);
            assert_eq!(m_budget, MEMORY_BUDGET_MB);
            assert_eq!(new_agent_tier, "L3");
            // reason 包含 INV-7 原始错误信息(诊断用)
            assert!(!reason.is_empty());
        }
        _ => panic!("expected AdmissionGateDenied, got {err:?}"),
    }
}

#[test]
fn admission_gate_denies_with_correct_tier_name_in_error() {
    // 不同 tier 名称应正确填充到错误字段
    let l0_err = AdmissionGate::check(200, ContextTier::L0, 4).unwrap_err();
    match l0_err {
        MasError::AdmissionGateDenied { new_agent_tier, .. } => {
            assert_eq!(new_agent_tier, "L0");
        }
        _ => panic!("expected AdmissionGateDenied"),
    }

    let l2_err = AdmissionGate::check(200, ContextTier::L2, 4).unwrap_err();
    match l2_err {
        MasError::AdmissionGateDenied { new_agent_tier, .. } => {
            assert_eq!(new_agent_tier, "L2");
        }
        _ => panic!("expected AdmissionGateDenied"),
    }
}

#[test]
fn admission_gate_zero_bytes_per_tok_always_passes_single_agent_constraint() {
    // bytes_per_tok = 0 时,agent_resident = 0,单 Agent 约束恒满足
    // 但全局约束仍生效(M_total > 117 仍拒绝)
    let result = AdmissionGate::check(50, ContextTier::L3, 0);
    assert!(
        result.is_ok(),
        "bytes_per_tok=0, m_total=50 应通过: {result:?}"
    );

    // m_total 超限仍拒绝(全局约束)
    let result = AdmissionGate::check(120, ContextTier::L3, 0);
    assert!(matches!(result, Err(MasError::AdmissionGateDenied { .. })));
}

#[test]
fn admission_gate_far_below_threshold_passes() {
    // 远低于阈值,任何 tier 都应通过
    for tier in [
        ContextTier::L0,
        ContextTier::L1,
        ContextTier::L2,
        ContextTier::L3,
    ] {
        let result = AdmissionGate::check(0, tier, 4);
        assert!(result.is_ok(), "tier={tier:?} m_total=0 应通过: {result:?}");
    }
}

// ============================================================
// SubTask 15.6: compression_threshold 测试
// ============================================================

#[test]
fn compression_threshold_is_0_9() {
    // 90% 触发压缩阈值
    assert!((COMPRESSION_THRESHOLD - 0.9).abs() < f64::EPSILON);
}

#[test]
fn compression_critical_never_compresses() {
    // ADR-026 红线:Critical 块永不被压缩
    // 任何利用率下都返回 false(包括 100%)
    assert!(!should_compress_at(ContextPriority::Critical, 0.0));
    assert!(!should_compress_at(ContextPriority::Critical, 0.5));
    assert!(!should_compress_at(ContextPriority::Critical, 0.89));
    assert!(!should_compress_at(ContextPriority::Critical, 0.9));
    assert!(!should_compress_at(ContextPriority::Critical, 0.95));
    assert!(!should_compress_at(ContextPriority::Critical, 1.0));
}

#[test]
fn compression_optional_always_compressible() {
    // Optional 块在任何利用率下都可丢弃(可完全丢弃)
    assert!(should_compress_at(ContextPriority::Optional, 0.0));
    assert!(should_compress_at(ContextPriority::Optional, 0.1));
    assert!(should_compress_at(ContextPriority::Optional, 0.5));
    assert!(should_compress_at(ContextPriority::Optional, 0.89));
    assert!(should_compress_at(ContextPriority::Optional, 0.9));
    assert!(should_compress_at(ContextPriority::Optional, 1.0));
}

#[test]
fn compression_normal_triggers_at_90_percent() {
    // Normal 块:利用率 < 0.9 时不压缩,≥ 0.9 时压缩
    assert!(!should_compress_at(ContextPriority::Normal, 0.0));
    assert!(!should_compress_at(ContextPriority::Normal, 0.5));
    assert!(!should_compress_at(ContextPriority::Normal, 0.89));
    // 边界:恰好 0.9,触发压缩(等号允许,与 INV-7 风格一致)
    assert!(should_compress_at(ContextPriority::Normal, 0.9));
    assert!(should_compress_at(ContextPriority::Normal, 0.95));
    assert!(should_compress_at(ContextPriority::Normal, 1.0));
}

#[test]
fn compression_high_and_low_behave_like_normal_at_threshold() {
    // High/Low 与 Normal 行为一致:90% 触发压缩
    // WHY 一致:三者均为非 Critical/Optional 的中间优先级,
    // 压缩策略相同(只有保留顺序不同,由 build_prompt 中的 sort_by_key 处理)
    for priority in [ContextPriority::High, ContextPriority::Low] {
        assert!(
            !should_compress_at(priority, 0.89),
            "priority={priority:?} 0.89 不应压缩"
        );
        assert!(
            should_compress_at(priority, 0.9),
            "priority={priority:?} 0.9 应压缩"
        );
    }
}

#[test]
fn compression_priority_ordering_preserved() {
    // 验证 ContextPriority 排序保持不变(防止引入 compression 逻辑破坏 Ord)
    assert!(ContextPriority::Critical > ContextPriority::High);
    assert!(ContextPriority::High > ContextPriority::Normal);
    assert!(ContextPriority::Normal > ContextPriority::Low);
    assert!(ContextPriority::Low > ContextPriority::Optional);
}
