//! Task 6: OSA 五维度并行计算一致性测试
//!
//! 验证 `compute_all_masks`(std::thread::scope 并行)与顺序调用 5 个
//! `compute_*_mask` 方法产生完全一致的结果。
//!
//! # 测试策略
//!
//! 1. **顺序基准**:直接调用 5 个 `compute_*_mask` 同步方法,得到五维度掩码
//! 2. **并行结果**:调用 `compute_all_masks` async 方法,得到 OmniSparseMasks
//! 3. **比较**:五维度掩码的 `active_ids` 与 `sparsity_ratio` 完全一致
//!
//! # 不变量
//!
//! - 相同 TaskProfile → 相同五维度掩码(纯函数确定性)
//! - 并行计算不改变结果顺序(active_ids 顺序确定,mask_hash 一致)
//! - 不同复杂度档位 / 风险等级 / 候选集规模均满足一致性

use std::sync::Arc;

use event_bus::EventBus;
use nexus_contracts::{MemoryStrategy, MemoryStrategyProvider, MemoryTaskPhase};
use osa_coordinator::{
    AffectedScope, FileId, MemoryId, OmniSparseCoordinator, OperationId, RiskLevel, TaskId,
    TaskProfile, TaskType, TimePressure, ToolId,
};

// ============================================================
// 辅助函数:构造 TaskProfile
// ============================================================

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
        // 评分字段默认 None:fallback 到 heuristic_scores(前 K 个)
        routing_scores: None,
        context_scores: None,
        memory_scores: None,
        // task_phase 默认 None:fallback 到 Initial
        task_phase: None,
    }
}

/// 比较五维度掩码:并行结果 vs 顺序基准
///
/// 断言五维度的 `active_ids` 与 `sparsity_ratio` 完全一致。
/// WHY 比较 active_ids 而非 is_active:active_ids 顺序确定(mask_hash 一致性),
/// 顺序错误会导致 mask_hash 不同,必须严格比较 Vec 相等。
fn assert_masks_match_sequential(
    coord: &OmniSparseCoordinator,
    profile: &TaskProfile,
    parallel_masks: &osa_coordinator::OmniSparseMasks,
) {
    // 顺序基准:直接调用 5 个 compute_*_mask 同步方法
    let seq_routing = coord.compute_routing_mask(profile);
    let seq_context = coord.compute_context_mask(profile);
    let seq_memory = coord.compute_memory_mask(profile);
    let seq_audit = coord.compute_audit_mask(profile);
    let seq_budget = coord.compute_budget_mask(profile);

    // 比较五维度:active_ids + sparsity_ratio 必须完全一致
    assert_eq!(
        parallel_masks.routing.active_ids, seq_routing.active_ids,
        "routing active_ids 不一致:并行 {:?} vs 顺序 {:?}",
        parallel_masks.routing.active_ids, seq_routing.active_ids
    );
    assert_eq!(
        parallel_masks.routing.sparsity_ratio, seq_routing.sparsity_ratio,
        "routing sparsity_ratio 不一致"
    );

    assert_eq!(
        parallel_masks.context.active_ids, seq_context.active_ids,
        "context active_ids 不一致"
    );
    assert_eq!(
        parallel_masks.context.sparsity_ratio, seq_context.sparsity_ratio,
        "context sparsity_ratio 不一致"
    );

    assert_eq!(
        parallel_masks.memory.active_ids, seq_memory.active_ids,
        "memory active_ids 不一致"
    );
    assert_eq!(
        parallel_masks.memory.sparsity_ratio, seq_memory.sparsity_ratio,
        "memory sparsity_ratio 不一致"
    );

    assert_eq!(
        parallel_masks.audit.active_ids, seq_audit.active_ids,
        "audit active_ids 不一致"
    );
    assert_eq!(
        parallel_masks.audit.sparsity_ratio, seq_audit.sparsity_ratio,
        "audit sparsity_ratio 不一致"
    );

    assert_eq!(
        parallel_masks.budget.active_ids, seq_budget.active_ids,
        "budget active_ids 不一致"
    );
    assert_eq!(
        parallel_masks.budget.sparsity_ratio, seq_budget.sparsity_ratio,
        "budget sparsity_ratio 不一致"
    );
}

// ============================================================
// 一致性测试:四档复杂度
// ============================================================

/// Simple 档位(complexity=0.1)并行 vs 顺序一致性
#[tokio::test]
async fn test_parallel_matches_sequential_simple_band() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.1, RiskLevel::Low);

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

/// Regular 档位(complexity=0.3)并行 vs 顺序一致性
#[tokio::test]
async fn test_parallel_matches_sequential_regular_band() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.3, RiskLevel::Medium);

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

/// Complex 档位(complexity=0.6)并行 vs 顺序一致性
#[tokio::test]
async fn test_parallel_matches_sequential_complex_band() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.6, RiskLevel::High);

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

/// UltraComplex 档位(complexity=0.9)并行 vs 顺序一致性
#[tokio::test]
async fn test_parallel_matches_sequential_ultra_complex_band() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.9, RiskLevel::Critical);

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

// ============================================================
// 一致性测试:边界值
// ============================================================

/// complexity=0.0(Simple 下界)并行 vs 顺序一致性
#[tokio::test]
async fn test_parallel_matches_sequential_complexity_zero() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.0, RiskLevel::Low);

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

/// complexity=1.0(UltraComplex 上界)并行 vs 顺序一致性
#[tokio::test]
async fn test_parallel_matches_sequential_complexity_one() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(1.0, RiskLevel::Critical);

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

// ============================================================
// 一致性测试:不同风险等级
// ============================================================

/// 四档风险等级(Low/Medium/High/Critical)并行 vs 顺序一致性
#[tokio::test]
async fn test_parallel_matches_sequential_all_risk_levels() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    for risk in [
        RiskLevel::Low,
        RiskLevel::Medium,
        RiskLevel::High,
        RiskLevel::Critical,
    ] {
        let profile = make_profile(0.5, risk);
        let parallel_masks = coord
            .compute_all_masks(&profile)
            .await
            .expect("并行掩码计算应成功");
        assert_masks_match_sequential(&coord, &profile, &parallel_masks);
    }
}

// ============================================================
// 一致性测试:空候选集
// ============================================================

/// 所有候选集为空时并行 vs 顺序一致性(应返回空掩码)
#[tokio::test]
async fn test_parallel_matches_sequential_empty_candidates() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let mut profile = make_profile(0.5, RiskLevel::Medium);
    profile.available_tools.clear();
    profile.available_files.clear();
    profile.available_memories.clear();
    profile.recent_operations.clear();
    profile.active_tasks.clear();

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    // 五维度均应为空掩码
    assert_eq!(parallel_masks.routing.active_count(), 0);
    assert_eq!(parallel_masks.context.active_count(), 0);
    assert_eq!(parallel_masks.memory.active_count(), 0);
    assert_eq!(parallel_masks.audit.active_count(), 0);
    assert_eq!(parallel_masks.budget.active_count(), 0);

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

// ============================================================
// 一致性测试:携带真实评分
// ============================================================

/// profile 携带真实评分时并行 vs 顺序一致性
///
/// WHY 真实评分场景:compute_*_mask 用真实评分做 Top-K(基于相关性),
/// active_ids 顺序由评分决定,需验证并行计算不破坏评分排序。
#[tokio::test]
async fn test_parallel_matches_sequential_with_real_scores() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let mut profile = make_profile(0.6, RiskLevel::Medium);

    // 注入真实评分(随机分布,非单调,验证 Top-K 排序正确性)
    // WHY 非单调评分:确保 select_top_k 的 select_nth_unstable_by 产生正确的 Top-K,
    // 而非"前 K 个"。并行计算必须保持相同的排序结果。
    profile.routing_scores = Some((0..50).map(|i| (i as f32 * 0.013).fract()).collect());
    profile.context_scores = Some((0..2000).map(|i| (i as f32 * 0.0007).fract()).collect());
    profile.memory_scores = Some((0..50).map(|i| (i as f32 * 0.017).fract()).collect());

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

// ============================================================
// 一致性测试:注入 memory_strategy_provider
// ============================================================

/// Mock provider:返回固定策略(用于测试 S2 集成下的并行一致性)
struct MockFixedStrategyProvider {
    strategy: MemoryStrategy,
}

impl MemoryStrategyProvider for MockFixedStrategyProvider {
    fn select_strategy(&self, _phase: MemoryTaskPhase) -> MemoryStrategy {
        self.strategy
    }
}

/// 注入 S2 provider(AggressivePruning)时并行 vs 顺序一致性
///
/// WHY S2 集成测试:compute_memory_mask 调用 provider.select_strategy(phase),
/// 需验证 Arc<dyn MemoryStrategyProvider> 在多线程间共享的正确性。
/// MemoryStrategyProvider trait 要求 Send + Sync,理论上线程安全,
/// 此测试实证验证并行调用不产生数据竞争。
#[tokio::test]
async fn test_parallel_matches_sequential_with_s2_provider() {
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(MockFixedStrategyProvider {
        strategy: MemoryStrategy::AggressivePruning,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);

    let mut profile = make_profile(0.4, RiskLevel::Medium);
    // 注入 task_phase,触发 S2 策略选择路径
    profile.task_phase = Some(MemoryTaskPhase::LongRun);

    let parallel_masks = coord
        .compute_all_masks(&profile)
        .await
        .expect("并行掩码计算应成功");

    assert_masks_match_sequential(&coord, &profile, &parallel_masks);
}

// ============================================================
// 一致性测试:多次调用确定性(纯函数不变量)
// ============================================================

/// 多次并行调用产生相同结果(纯函数确定性)
///
/// WHY 多次调用测试:并行计算的线程调度可能不同,但结果必须确定。
/// 此测试验证 std::thread::scope 在不同调度下仍产生相同的五维度掩码。
#[tokio::test]
async fn test_parallel_repeatable_results() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile(0.5, RiskLevel::Medium);

    // 第一次并行计算
    let masks_1 = coord
        .compute_all_masks(&profile)
        .await
        .expect("第一次并行计算应成功");
    let hash_1 = osa_coordinator::compute_omni_mask_hash(&masks_1).expect("hash 计算应成功");

    // 第二次并行计算(相同 profile)
    let masks_2 = coord
        .compute_all_masks(&profile)
        .await
        .expect("第二次并行计算应成功");
    let hash_2 = osa_coordinator::compute_omni_mask_hash(&masks_2).expect("hash 计算应成功");

    // 两次并行计算的 mask_hash 必须一致(纯函数确定性)
    assert_eq!(
        hash_1, hash_2,
        "两次并行计算的 mask_hash 不一致:并行计算非确定性"
    );

    // 五维度 active_ids 也必须一致
    assert_eq!(
        masks_1.routing.active_ids, masks_2.routing.active_ids,
        "两次并行计算的 routing active_ids 不一致"
    );
    assert_eq!(
        masks_1.context.active_ids, masks_2.context.active_ids,
        "两次并行计算的 context active_ids 不一致"
    );
    assert_eq!(
        masks_1.memory.active_ids, masks_2.memory.active_ids,
        "两次并行计算的 memory active_ids 不一致"
    );
    assert_eq!(
        masks_1.audit.active_ids, masks_2.audit.active_ids,
        "两次并行计算的 audit active_ids 不一致"
    );
    assert_eq!(
        masks_1.budget.active_ids, masks_2.budget.active_ids,
        "两次并行计算的 budget active_ids 不一致"
    );
}

// ============================================================
// 一致性测试:不同复杂度批量验证
// ============================================================

/// 11 个复杂度值(0.0 ~ 1.0,步长 0.1)批量验证并行 vs 顺序一致性
///
/// WHY 批量测试:覆盖整个 complexity_score 范围,确保四档分级的所有边界
/// (0.25/0.5/0.75)在并行计算下均与顺序一致。
#[tokio::test]
async fn test_parallel_matches_sequential_all_complexities() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    for i in 0..=10 {
        let complexity = i as f32 / 10.0;
        let profile = make_profile(complexity, RiskLevel::Medium);
        let parallel_masks = coord
            .compute_all_masks(&profile)
            .await
            .expect("并行掩码计算应成功");
        assert_masks_match_sequential(&coord, &profile, &parallel_masks);
    }
}

// ============================================================
// 一致性测试:不同候选集规模
// ============================================================

/// 不同候选集规模(小/中/大)并行 vs 顺序一致性
///
/// WHY 多规模测试:验证并行计算在不同数据量下的正确性。
/// 小规模(10 项)验证无 panic,大规模(10000 项)验证无数据竞争。
#[tokio::test]
async fn test_parallel_matches_sequential_various_scales() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    // 小规模:10 工具 + 10 文件 + 10 记忆 + 5 操作 + 3 任务
    let mut small_profile = make_profile(0.5, RiskLevel::Medium);
    small_profile.available_tools = (0..10).map(|i| ToolId::new(format!("t-{i}"))).collect();
    small_profile.available_files = (0..10).map(|i| FileId::new(format!("f-{i}"))).collect();
    small_profile.available_memories = (0..10).map(|i| MemoryId::new(format!("m-{i}"))).collect();
    small_profile.recent_operations = (0..5).map(|i| OperationId::new(format!("o-{i}"))).collect();
    small_profile.active_tasks = (0..3).map(|i| TaskId::new(format!("tk-{i}"))).collect();

    let small_masks = coord
        .compute_all_masks(&small_profile)
        .await
        .expect("小规模并行计算应成功");
    assert_masks_match_sequential(&coord, &small_profile, &small_masks);

    // 大规模:500 工具 + 10000 文件 + 500 记忆 + 1000 操作 + 100 任务
    let mut large_profile = make_profile(0.8, RiskLevel::High);
    large_profile.available_tools = (0..500).map(|i| ToolId::new(format!("t-{i}"))).collect();
    large_profile.available_files = (0..10000).map(|i| FileId::new(format!("f-{i}"))).collect();
    large_profile.available_memories = (0..500).map(|i| MemoryId::new(format!("m-{i}"))).collect();
    large_profile.recent_operations = (0..1000)
        .map(|i| OperationId::new(format!("o-{i}")))
        .collect();
    large_profile.active_tasks = (0..100).map(|i| TaskId::new(format!("tk-{i}"))).collect();

    let large_masks = coord
        .compute_all_masks(&large_profile)
        .await
        .expect("大规模并行计算应成功");
    assert_masks_match_sequential(&coord, &large_profile, &large_masks);
}

// ============================================================
// 一致性测试:并发调用 compute_all_masks(无数据竞争)
// ============================================================

/// 10 个并发 compute_all_masks 调用,验证并行计算无数据竞争
///
/// WHY 并发测试:多个 compute_all_masks 同时调用时,每个调用内部
/// 又派生 5 个线程并行计算。10 × 5 = 50 个线程同时运行,
/// 验证 OmniSparseCoordinator 的 &self 共享不产生数据竞争。
#[tokio::test]
async fn test_concurrent_compute_all_masks_no_data_race() {
    use std::sync::Arc;

    let bus = EventBus::new();
    let coord = Arc::new(OmniSparseCoordinator::new(bus));
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
        let masks = handle.await.expect("task join 应成功").expect("计算应成功");
        mask_hashes.push(osa_coordinator::compute_omni_mask_hash(&masks).expect("hash 应成功"));
    }

    // 所有 mask_hash 必须一致(纯函数 + 并行计算无数据竞争)
    let first_hash = &mask_hashes[0];
    for (i, hash) in mask_hashes.iter().enumerate() {
        assert_eq!(
            hash, first_hash,
            "任务 {} 的 mask_hash 不一致:并行计算可能存在数据竞争",
            i
        );
    }
}
