//! OmniSparseCoordinator 单元测试 — 验证 4 个复杂度档位产生不同稀疏度掩码
//!
//! 对应 SubTask 4.11:验证 routing/context/audit 各维度数值符合预期
//!
//! 复杂度联动稀疏化策略(架构手册):
//! - Simple(< 0.25):routing Top-8,context 1 文件,audit 10%
//! - Regular(0.25-0.5):routing Top-16,context 10 文件,audit 50%
//! - Complex(0.5-0.75):routing Top-24,context 100 文件,audit 100%
//! - UltraComplex(≥ 0.75):routing Top-32,context 1000 文件,audit 100%

use event_bus::EventBus;
use osa_coordinator::OsaConfig;
use osa_coordinator::{
    AffectedScope, FileId, MemoryId, OmniSparseCoordinator, OperationId, RiskLevel, TaskId,
    TaskProfile, TaskType, TimePressure, ToolId,
};

/// 构造测试用 TaskProfile
///
/// - `complexity`:复杂度分数 [0.0, 1.0]
/// - `risk`:风险等级
/// - 候选集:50 工具 / 2000 文件 / 50 记忆 / 100 操作 / 10 任务
fn make_profile(complexity: f32, risk: RiskLevel) -> TaskProfile {
    TaskProfile {
        task_id: TaskId::new(format!("task-{complexity}")),
        task_type: TaskType::Read,
        complexity_score: complexity,
        risk_level: risk,
        time_pressure: TimePressure::Low,
        affected_scope: AffectedScope::Local,
        available_tools: (0..50).map(|i| ToolId::new(format!("tool-{i}"))).collect(),
        available_files: (0..2000)
            .map(|i| FileId::new(format!("file-{i}")))
            .collect(),
        available_memories: (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect(),
        recent_operations: (0..100)
            .map(|i| OperationId::new(format!("op-{i}")))
            .collect(),
        active_tasks: (0..10).map(|i| TaskId::new(format!("task-{i}"))).collect(),
    }
}

#[test]
fn test_simple_band_routing_top_8() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.1, RiskLevel::Low);
    let mask = coord.compute_routing_mask(&profile);
    assert_eq!(
        mask.active_count(),
        8,
        "Simple 档位 routing 应保留 Top-8 工具"
    );
}

#[test]
fn test_simple_band_context_1_file() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.1, RiskLevel::Low);
    let mask = coord.compute_context_mask(&profile);
    assert_eq!(mask.active_count(), 1, "Simple 档位 context 应保留 1 文件");
}

#[test]
fn test_simple_band_audit_10_percent() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.1, RiskLevel::Low);
    let mask = coord.compute_audit_mask(&profile);
    // 100 操作 × 10% = 10 个
    assert_eq!(
        mask.active_count(),
        10,
        "Simple 档位 audit 应采样 10%(100×0.1=10)"
    );
}

#[test]
fn test_regular_band_routing_top_16() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.3, RiskLevel::Low);
    let mask = coord.compute_routing_mask(&profile);
    assert_eq!(
        mask.active_count(),
        16,
        "Regular 档位 routing 应保留 Top-16 工具"
    );
}

#[test]
fn test_regular_band_context_10_files() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.3, RiskLevel::Low);
    let mask = coord.compute_context_mask(&profile);
    assert_eq!(
        mask.active_count(),
        10,
        "Regular 档位 context 应保留 10 文件"
    );
}

#[test]
fn test_regular_band_audit_50_percent() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.3, RiskLevel::Low);
    let mask = coord.compute_audit_mask(&profile);
    // 100 操作 × 50% = 50 个
    assert_eq!(
        mask.active_count(),
        50,
        "Regular 档位 audit 应采样 50%(100×0.5=50)"
    );
}

#[test]
fn test_complex_band_routing_top_24() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.6, RiskLevel::Low);
    let mask = coord.compute_routing_mask(&profile);
    assert_eq!(
        mask.active_count(),
        24,
        "Complex 档位 routing 应保留 Top-24 工具"
    );
}

#[test]
fn test_complex_band_context_100_files() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.6, RiskLevel::Low);
    let mask = coord.compute_context_mask(&profile);
    assert_eq!(
        mask.active_count(),
        100,
        "Complex 档位 context 应保留 100 文件"
    );
}

#[test]
fn test_complex_band_audit_full() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.6, RiskLevel::Low);
    let mask = coord.compute_audit_mask(&profile);
    // 100 操作 × 100% = 100 个(全审计)
    assert_eq!(
        mask.active_count(),
        100,
        "Complex 档位 audit 应全审计(100%)"
    );
}

#[test]
fn test_ultra_complex_band_routing_top_32() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.9, RiskLevel::Low);
    let mask = coord.compute_routing_mask(&profile);
    assert_eq!(
        mask.active_count(),
        32,
        "UltraComplex 档位 routing 应保留 Top-32 工具"
    );
}

#[test]
fn test_ultra_complex_band_context_1000_files() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.9, RiskLevel::Low);
    let mask = coord.compute_context_mask(&profile);
    assert_eq!(
        mask.active_count(),
        1000,
        "UltraComplex 档位 context 应保留 1000 文件"
    );
}

#[test]
fn test_ultra_complex_band_audit_full() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.9, RiskLevel::Low);
    let mask = coord.compute_audit_mask(&profile);
    assert_eq!(
        mask.active_count(),
        100,
        "UltraComplex 档位 audit 应全审计(100%)"
    );
}

#[test]
fn test_bands_produce_different_sparsity() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let complexities = [0.1_f32, 0.3, 0.6, 0.9];
    let mut sparsities = Vec::new();
    for &c in &complexities {
        let profile = make_profile(c, RiskLevel::Low);
        let mask = coord.compute_routing_mask(&profile);
        sparsities.push(mask.sparsity_ratio);
    }
    // 复杂度越高,稀疏度越低(保留更多活跃项)
    assert!(sparsities[0] > sparsities[1], "Simple 稀疏度应 > Regular");
    assert!(sparsities[1] > sparsities[2], "Regular 稀疏度应 > Complex");
    assert!(
        sparsities[2] > sparsities[3],
        "Complex 稀疏度应 > UltraComplex"
    );
}

#[test]
fn test_high_risk_increases_audit_rate() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    // Simple 档位 + Low 风险 → 10%
    let profile_low = make_profile(0.1, RiskLevel::Low);
    let mask_low = coord.compute_audit_mask(&profile_low);
    // Simple 档位 + Critical 风险 → max(0.1, 1.0) = 1.0(全审计)
    let profile_critical = make_profile(0.1, RiskLevel::Critical);
    let mask_critical = coord.compute_audit_mask(&profile_critical);
    assert!(
        mask_critical.active_count() > mask_low.active_count(),
        "高风险应提高 audit 采样率"
    );
    assert_eq!(
        mask_critical.active_count(),
        100,
        "Simple+Critical 应全审计"
    );
}

#[test]
fn test_budget_mask_decreases_with_complexity() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    // 简单任务:保护比例高(保留少)
    let profile_simple = make_profile(0.0, RiskLevel::Low);
    let mask_simple = coord.compute_budget_mask(&profile_simple);
    // 超复杂任务:保护比例低(保留多)
    let profile_ultra = make_profile(1.0, RiskLevel::Low);
    let mask_ultra = coord.compute_budget_mask(&profile_ultra);
    // 复杂度越高,保留越多任务(降低稀疏度)
    assert!(
        mask_ultra.active_count() >= mask_simple.active_count(),
        "超复杂任务应保留更多活跃任务"
    );
}

#[test]
fn test_empty_candidates_return_empty_mask() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let mut profile = make_profile(0.5, RiskLevel::Medium);
    profile.available_tools.clear();
    profile.available_files.clear();
    profile.recent_operations.clear();
    profile.active_tasks.clear();
    assert_eq!(coord.compute_routing_mask(&profile).active_count(), 0);
    assert_eq!(coord.compute_context_mask(&profile).active_count(), 0);
    assert_eq!(coord.compute_audit_mask(&profile).active_count(), 0);
    assert_eq!(coord.compute_budget_mask(&profile).active_count(), 0);
}

/// SubTask 10.5:验证 OSA 10 任务并发 compute_all_masks 的 mask_hash 一致性
///
/// OmniSparseCoordinator 的掩码计算为纯函数(相同 TaskProfile 产生相同掩码),
/// 10 个 tokio::spawn 并发 compute_all_masks(相同 TaskProfile)应返回相同的 mask_hash。
/// 验证无 panic、无数据竞争、mask_hash 一致。
#[tokio::test]
async fn test_concurrent_compute_masks() {
    use std::sync::Arc;

    let bus = EventBus::new();
    let coord = Arc::new(OmniSparseCoordinator::new(bus));
    // WHY Arc:profile 需在多个 spawn 闭包中共享引用,
    // Arc clone 为廉价的引用计数递增,避免每次 clone 整个 TaskProfile
    let profile = Arc::new(make_profile(0.5, RiskLevel::Medium));

    // 10 任务并发 compute_all_masks(相同 TaskProfile)
    let mut handles = Vec::with_capacity(10);
    for _ in 0..10 {
        let coord_clone = coord.clone();
        let profile_clone = profile.clone();
        handles.push(tokio::spawn(async move {
            coord_clone.compute_all_masks(&profile_clone).await
        }));
    }

    // 等待所有计算完成,收集 mask_hash
    let mut mask_hashes = Vec::with_capacity(10);
    for handle in handles {
        let masks = handle.await.unwrap().unwrap();
        mask_hashes.push(masks.mask_hash().to_string());
    }

    // 验证所有 mask_hash 一致(纯函数,相同输入应产生相同输出)
    let first_hash = &mask_hashes[0];
    for (i, hash) in mask_hashes.iter().enumerate() {
        assert_eq!(
            hash, first_hash,
            "任务 {} 的 mask_hash 不一致:期望 {},实际 {}",
            i, first_hash, hash
        );
    }
}

// ============================================================
// SubTask 14.4:复杂度阈值可配置化测试
// ============================================================

/// SubTask 14.4:验证自定义阈值 [0.3, 0.6, 0.9] 产生不同分档
///
/// 默认阈值 (0.25, 0.5, 0.75) 与自定义阈值 (0.3, 0.6, 0.9) 在以下分数产生不同档位:
/// - 0.26:默认 Regular(Top-16),自定义 Simple(Top-8)
/// - 0.55:默认 Complex(Top-24),自定义 Regular(Top-16)
/// - 0.8:默认 UltraComplex(Top-32),自定义 Complex(Top-24)
#[test]
fn test_custom_complexity_thresholds_produce_different_bands() {
    // 默认配置协调器
    let default_bus = EventBus::new();
    let default_coord = OmniSparseCoordinator::new(default_bus);

    // 自定义阈值配置协调器
    let custom_config = OsaConfig::default().with_complexity_thresholds(0.3, 0.6, 0.9);
    let custom_bus = EventBus::new();
    let custom_coord = OmniSparseCoordinator::with_config(custom_bus, custom_config);

    // 0.26:默认 Regular(Top-16),自定义 Simple(Top-8)
    let profile = make_profile(0.26, RiskLevel::Low);
    let default_mask = default_coord.compute_routing_mask(&profile);
    let custom_mask = custom_coord.compute_routing_mask(&profile);
    assert_eq!(
        default_mask.active_count(),
        16,
        "默认阈值 0.26 应为 Regular(Top-16)"
    );
    assert_eq!(
        custom_mask.active_count(),
        8,
        "自定义阈值 0.26 < 0.3 应为 Simple(Top-8)"
    );

    // 0.55:默认 Complex(Top-24),自定义 Regular(Top-16)
    let profile = make_profile(0.55, RiskLevel::Low);
    let default_mask = default_coord.compute_routing_mask(&profile);
    let custom_mask = custom_coord.compute_routing_mask(&profile);
    assert_eq!(
        default_mask.active_count(),
        24,
        "默认阈值 0.55 应为 Complex(Top-24)"
    );
    assert_eq!(
        custom_mask.active_count(),
        16,
        "自定义阈值 0.55 ∈ [0.3, 0.6) 应为 Regular(Top-16)"
    );

    // 0.8:默认 UltraComplex(Top-32),自定义 Complex(Top-24)
    let profile = make_profile(0.8, RiskLevel::Low);
    let default_mask = default_coord.compute_routing_mask(&profile);
    let custom_mask = custom_coord.compute_routing_mask(&profile);
    assert_eq!(
        default_mask.active_count(),
        32,
        "默认阈值 0.8 应为 UltraComplex(Top-32)"
    );
    assert_eq!(
        custom_mask.active_count(),
        24,
        "自定义阈值 0.8 ∈ [0.6, 0.9) 应为 Complex(Top-24)"
    );
}

/// SubTask 14.4:验证自定义阈值影响 context 维度
#[test]
fn test_custom_complexity_thresholds_affect_context() {
    let custom_config = OsaConfig::default().with_complexity_thresholds(0.3, 0.6, 0.9);
    let bus = EventBus::new();
    let custom_coord = OmniSparseCoordinator::with_config(bus, custom_config);

    // 0.26:自定义阈值 → Simple → context 1 文件
    let profile = make_profile(0.26, RiskLevel::Low);
    let mask = custom_coord.compute_context_mask(&profile);
    assert_eq!(
        mask.active_count(),
        1,
        "自定义阈值 0.26 < 0.3 → Simple → context 1 文件"
    );

    // 0.4:自定义阈值 → Regular → context 10 文件
    let profile = make_profile(0.4, RiskLevel::Low);
    let mask = custom_coord.compute_context_mask(&profile);
    assert_eq!(
        mask.active_count(),
        10,
        "自定义阈值 0.4 ∈ [0.3, 0.6) → Regular → context 10 文件"
    );

    // 0.7:自定义阈值 → Complex → context 100 文件
    let profile = make_profile(0.7, RiskLevel::Low);
    let mask = custom_coord.compute_context_mask(&profile);
    assert_eq!(
        mask.active_count(),
        100,
        "自定义阈值 0.7 ∈ [0.6, 0.9) → Complex → context 100 文件"
    );
}

/// SubTask 14.4:验证默认阈值行为不变(向后兼容)
#[test]
fn test_default_complexity_thresholds_unchanged() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    // 默认阈值边界值验证
    // 0.24 < 0.25 → Simple
    let profile = make_profile(0.24, RiskLevel::Low);
    assert_eq!(coord.compute_routing_mask(&profile).active_count(), 8);

    // 0.25 ∈ [0.25, 0.5) → Regular
    let profile = make_profile(0.25, RiskLevel::Low);
    assert_eq!(coord.compute_routing_mask(&profile).active_count(), 16);

    // 0.5 ∈ [0.5, 0.75) → Complex
    let profile = make_profile(0.5, RiskLevel::Low);
    assert_eq!(coord.compute_routing_mask(&profile).active_count(), 24);

    // 0.75 ≥ 0.75 → UltraComplex
    let profile = make_profile(0.75, RiskLevel::Low);
    assert_eq!(coord.compute_routing_mask(&profile).active_count(), 32);
}

// ============================================================
// SubTask 15.8:边界测试(空、满、极端复杂度)
// ============================================================

/// SubTask 15.8:complexity_score = 0.0(Simple 档位下界)的全维度掩码验证
///
/// complexity = 0.0 对应 Simple 档位(< 0.25),应产生最高稀疏度:
/// - routing:Top-8 工具(50 候选 → sparsity = 1 - 8/50 = 0.84)
/// - context:1 文件(2000 候选 → sparsity = 1 - 1/2000 ≈ 0.9995)
/// - audit:10% 采样(100 操作 → 10 个,sparsity = 0.9)
/// - budget:保护比例最低(complexity=0 → protection = 0.8×0.5 = 0.4 → 4 个任务)
#[test]
fn test_complexity_zero_boundary_masks() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.0, RiskLevel::Low);

    // routing:Simple 档位 → Top-8
    let routing = coord.compute_routing_mask(&profile);
    assert_eq!(
        routing.active_count(),
        8,
        "complexity=0.0 routing 应保留 Top-8"
    );
    assert!(
        routing.sparsity() > 0.8,
        "complexity=0.0 routing 稀疏度应高(>0.8)"
    );

    // context:Simple 档位 → 1 文件
    let context = coord.compute_context_mask(&profile);
    assert_eq!(
        context.active_count(),
        1,
        "complexity=0.0 context 应保留 1 文件"
    );
    assert!(
        context.sparsity() > 0.99,
        "complexity=0.0 context 稀疏度应极高(>0.99)"
    );

    // audit:Simple 档位 + Low 风险 → 10% 采样 = 10 个
    let audit = coord.compute_audit_mask(&profile);
    assert_eq!(
        audit.active_count(),
        10,
        "complexity=0.0 audit 应采样 10%(100×0.1=10)"
    );

    // budget:complexity=0 → protection = 0.8×0.5 = 0.4 → ceil(10×0.4) = 4
    let budget = coord.compute_budget_mask(&profile);
    assert_eq!(
        budget.active_count(),
        4,
        "complexity=0.0 budget 应保留 4 个任务(40%)"
    );
}

/// SubTask 15.8:complexity_score = 1.0(UltraComplex 档位上界)的全维度掩码验证
///
/// complexity = 1.0 对应 UltraComplex 档位(≥ 0.75),应产生最低稀疏度:
/// - routing:Top-32 工具(50 候选 → sparsity = 1 - 32/50 = 0.36)
/// - context:1000 文件(2000 候选 → sparsity = 1 - 1000/2000 = 0.5)
/// - audit:100% 采样(全审计,100 操作 → 100 个,sparsity = 0.0)
/// - budget:保护比例最高(complexity=1 → protection = 0.8×1.0 = 0.8 → 8 个任务)
#[test]
fn test_complexity_one_boundary_masks() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(1.0, RiskLevel::Low);

    // routing:UltraComplex 档位 → Top-32
    let routing = coord.compute_routing_mask(&profile);
    assert_eq!(
        routing.active_count(),
        32,
        "complexity=1.0 routing 应保留 Top-32"
    );
    assert!(
        routing.sparsity() < 0.5,
        "complexity=1.0 routing 稀疏度应低(<0.5)"
    );

    // context:UltraComplex 档位 → 1000 文件
    let context = coord.compute_context_mask(&profile);
    assert_eq!(
        context.active_count(),
        1000,
        "complexity=1.0 context 应保留 1000 文件"
    );
    assert!(
        (context.sparsity() - 0.5).abs() < 1e-6,
        "complexity=1.0 context 稀疏度应为 0.5"
    );

    // audit:UltraComplex 档位 → 100% 全审计
    let audit = coord.compute_audit_mask(&profile);
    assert_eq!(
        audit.active_count(),
        100,
        "complexity=1.0 audit 应全审计(100%)"
    );
    assert!(
        (audit.sparsity() - 0.0).abs() < 1e-6,
        "complexity=1.0 audit 稀疏度应为 0.0(全审计)"
    );

    // budget:complexity=1 → protection = 0.8×1.0 = 0.8 → ceil(10×0.8) = 8
    let budget = coord.compute_budget_mask(&profile);
    assert_eq!(
        budget.active_count(),
        8,
        "complexity=1.0 budget 应保留 8 个任务(80%)"
    );
}

/// SubTask 15.8:complexity 0.0 vs 1.0 稀疏度对比
///
/// 验证复杂度边界值产生单调递减的稀疏度(复杂度越高,稀疏度越低)。
/// 这是 OSA 联动稀疏化的核心不变量。
#[test]
fn test_complexity_zero_vs_one_sparsity_decreasing() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let profile_zero = make_profile(0.0, RiskLevel::Low);
    let profile_one = make_profile(1.0, RiskLevel::Low);

    // routing 维度:complexity=0.0 稀疏度 > complexity=1.0 稀疏度
    let routing_zero = coord.compute_routing_mask(&profile_zero);
    let routing_one = coord.compute_routing_mask(&profile_one);
    assert!(
        routing_zero.sparsity() > routing_one.sparsity(),
        "routing 维度:complexity=0.0 稀疏度 {} 应 > complexity=1.0 稀疏度 {}",
        routing_zero.sparsity(),
        routing_one.sparsity()
    );

    // context 维度
    let context_zero = coord.compute_context_mask(&profile_zero);
    let context_one = coord.compute_context_mask(&profile_one);
    assert!(
        context_zero.sparsity() > context_one.sparsity(),
        "context 维度:complexity=0.0 稀疏度 {} 应 > complexity=1.0 稀疏度 {}",
        context_zero.sparsity(),
        context_one.sparsity()
    );

    // budget 维度:complexity=0.0 保留少(稀疏度高) > complexity=1.0 保留多(稀疏度低)
    let budget_zero = coord.compute_budget_mask(&profile_zero);
    let budget_one = coord.compute_budget_mask(&profile_one);
    assert!(
        budget_zero.sparsity() > budget_one.sparsity(),
        "budget 维度:complexity=0.0 稀疏度 {} 应 > complexity=1.0 稀疏度 {}",
        budget_zero.sparsity(),
        budget_one.sparsity()
    );
}

/// SubTask 15.8:affected_scope 无文件时 context_mask 为 empty()
///
/// 当 TaskProfile 的 available_files 为空(即受影响范围内无文件)时,
/// compute_context_mask 应返回 empty 掩码(sparsity_ratio = 1.0, active_count = 0)。
/// 验证边界条件:空候选集 → 空掩码,而非 panic 或错误。
#[test]
fn test_empty_affected_scope_returns_empty_context_mask() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let mut profile = make_profile(0.5, RiskLevel::Medium);
    // 清空 available_files,模拟受影响范围内无文件
    profile.available_files.clear();

    let context_mask = coord.compute_context_mask(&profile);
    assert_eq!(
        context_mask.active_count(),
        0,
        "available_files 为空时 context_mask active_count 应为 0"
    );
    assert!(
        (context_mask.sparsity() - 1.0).abs() < 1e-6,
        "available_files 为空时 context_mask sparsity 应为 1.0(empty)"
    );
    assert!(
        context_mask.active_ids.is_empty(),
        "available_files 为空时 context_mask active_ids 应为空"
    );

    // 验证其他维度掩码不受影响(仍有候选集)
    assert!(
        coord.compute_routing_mask(&profile).active_count() > 0,
        "routing 维度不应受 available_files 为空影响"
    );
    assert!(
        coord.compute_audit_mask(&profile).active_count() > 0,
        "audit 维度不应受 available_files 为空影响"
    );
}

/// SubTask 15.8:risk_level = Critical 时 audit_mask 为 full()
///
/// Critical 风险等级的 audit 采样率为 1.0(全审计),
/// 无论复杂度档位如何,实际采样率取 max(complexity_rate, risk_rate) = 1.0。
/// 验证:audit_mask 的 sparsity = 0.0(无稀疏),active_count = 总操作数。
#[test]
fn test_critical_risk_audit_mask_full() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    // 用 Simple 档位(complexity=0.1)+ Critical 风险
    // 复杂度默认采样率 0.1,风险采样率 1.0,取 max = 1.0(全审计)
    let profile = make_profile(0.1, RiskLevel::Critical);
    let audit_mask = coord.compute_audit_mask(&profile);

    // 全审计:active_count 等于总操作数(100)
    let total_ops = profile.recent_operations.len();
    assert_eq!(
        audit_mask.active_count(),
        total_ops,
        "Critical 风险应全审计:active_count 应 = 总操作数 {total_ops}"
    );
    // full 掩码的稀疏度为 0.0(无稀疏)
    assert!(
        (audit_mask.sparsity() - 0.0).abs() < 1e-6,
        "Critical 风险 audit_mask sparsity 应为 0.0(full),实际 {}",
        audit_mask.sparsity()
    );
    // 验证所有操作都在 active_set 中(全审计)
    for op in &profile.recent_operations {
        assert!(
            audit_mask.is_active(op),
            "Critical 风险全审计:操作 {} 应在 audit_mask 中",
            op
        );
    }
}

/// SubTask 15.8:risk_level = Critical 在各复杂度档位下均触发全审计
///
/// 验证 Critical 风险的全审计行为不随复杂度变化:
/// - Simple + Critical → full audit
/// - Regular + Critical → full audit
/// - Complex + Critical → full audit
/// - UltraComplex + Critical → full audit
#[test]
fn test_critical_risk_full_audit_across_all_bands() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let total_ops = 100; // make_profile 中 recent_operations 固定 100 个

    for &complexity in &[0.1_f32, 0.3, 0.6, 0.9] {
        let profile = make_profile(complexity, RiskLevel::Critical);
        let audit_mask = coord.compute_audit_mask(&profile);
        assert_eq!(
            audit_mask.active_count(),
            total_ops,
            "complexity={complexity} + Critical 应全审计({total_ops} 个),实际 {}",
            audit_mask.active_count()
        );
        assert!(
            (audit_mask.sparsity() - 0.0).abs() < 1e-6,
            "complexity={complexity} + Critical audit sparsity 应为 0.0"
        );
    }
}
