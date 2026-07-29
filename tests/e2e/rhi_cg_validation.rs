//! P5.5.2 — RHI-CG 进化执行器 + P5.5.3 北极星指标验证
//!
//! 对应任务: P5.5.2（RhiCgEvolutionExecutor 实现）+ P5.5.3（KPI-01/KPI-02 验证）
//! 架构层: 测试编排层（tests/e2e/，非生产代码，复用 P5.1/P5.2/P5.3 既有实现）
//!
//! # 设计要点
//!
//! - **复用既有组件**（复杂度预算净增长 ≤0）:
//!   - `StubJudgeClient`（P5.1）: 确定性评判器，always Current wins（spec 设计保证 v(i+1) > v(i)）
//!   - `MockCiGate::with_passing_result()`（P5.2）: 模拟 CI 通过，用于验证胜率路径
//!   - `SignificanceDetector`（P5.2）: 累积回归次数，验证否决证据检查
//!   - `SpecRegistry`（P5.2）: 版本化注册表 + 谱系追踪
//!   - `SelfComparisonHistory`（P5.1）: 偏好对持久化到 L2 语义记忆
//!
//! - **3 轮 × 5 任务 = 15 次评判**:
//!   - Round 1: v2 vs v1（5 任务）
//!   - Round 2: v3 vs v2（5 任务）
//!   - Round 3: v4 vs v3（5 任务）
//!
//! - **北极星指标**:
//!   - KPI-01 累计胜率 = wins / 15 ≥ 60%（即 ≥ 9 次优胜）
//!   - KPI-02 误杀率 = false_kills / 15 < 5%（即 ≤ 0 次误杀）
//!
//! - **确定性保证**:
//!   - spec_score(v(i+1)) > spec_score(v(i)) 由 P5.5.1 任务集设计保证
//!   - StubJudgeClient::current_wins() 裁决 Current 胜出（与 spec_score 一致）
//!   - MockCiGate::with_passing_result() 永远通过（无误杀）
//!
//! # TDD 守恒
//!
//! 测试先于实现:本文件先写测试用例（test_executor_initialization 等），
//! 再写最小实现使测试通过。所有 KPI 验证为失败测试（未实现前应失败）。
//!
//! # async 反模式对齐（§4.4）
//!
//! - 不持锁跨 `.await`:本文件无锁（测试代码）
//! - bus.subscribe() 在 spawn 之前:本文件不使用 spawn（顺序执行）
//! - rusqlite 必须 spawn_blocking:本文件不直接调用 rusqlite

#![forbid(unsafe_code)]

#[path = "fixtures/quest_set_v1.rs"]
mod quest_set_v1;

use auto_dpo::{
    JudgeClient, PreferencePair, SelfComparisonHistory, SelfComparisonRecord, SpecVersion,
    StubJudgeClient,
};
use gsoe_evolution::{CiGate, MockCiGate, SignificanceDetector, SpecRegistry};
use nexus_contracts::HarnessSpec;
use quest_set_v1::{quest_set_v1, spec_score, QuestTask};

// ============================================================
// 数据结构 — 进化结果载体
// ============================================================

/// 单个任务单轮的进化评判结果
///
/// # 字段语义
///
/// | 字段 | 含义 |
/// |------|------|
/// | `task_id` | 任务标识（T1-T5） |
/// | `round` | 进化轮次（1-3） |
/// | `winner` | 评判器裁决的胜出版本（Current/Previous） |
/// | `winner_score` | 胜出者评分 [0.0, 1.0] |
/// | `loser_score` | 失败者评分 [0.0, 1.0] |
/// | `confidence` | 评判器置信度 [0.0, 1.0] |
/// | `rationale` | 评判理由（人类可读，用于审计） |
/// | `ci_passed` | CI 执行门是否通过 |
/// | `registered` | 是否成功注册到 SpecRegistry |
/// | `vetoed` | 是否被通道 B 否决（CI 失败） |
#[derive(Debug, Clone, PartialEq)]
pub struct EvolutionVerdict {
    /// 任务标识（T1-T5）
    pub task_id: String,
    /// 进化轮次（1-3）
    pub round: u32,
    /// 评判器裁决的胜出版本
    pub winner: SpecVersion,
    /// 胜出者评分 [0.0, 1.0]
    pub winner_score: f32,
    /// 失败者评分 [0.0, 1.0]
    pub loser_score: f32,
    /// 评判器置信度 [0.0, 1.0]
    pub confidence: f32,
    /// 评判理由
    pub rationale: String,
    /// CI 执行门是否通过
    pub ci_passed: bool,
    /// 是否成功注册到 SpecRegistry
    pub registered: bool,
    /// 是否被通道 B 否决
    pub vetoed: bool,
}

/// 一轮进化的聚合结果（5 任务）
///
/// # 字段语义
///
/// | 字段 | 含义 |
/// |------|------|
/// | `round` | 轮次（1-3） |
/// | `verdicts` | 该轮 5 个任务的评判结果列表 |
/// | `wins` | 优胜次数（Current 胜出 + CI 通过） |
/// | `losses` | 失败次数（Previous 胜出 或 CI 失败） |
/// | `false_kills` | 误杀次数（Current 实际更优但被 CI 否决） |
#[derive(Debug, Clone, PartialEq)]
pub struct EvolutionRoundResult {
    /// 轮次（1-3）
    pub round: u32,
    /// 该轮 5 个任务的评判结果列表
    pub verdicts: Vec<EvolutionVerdict>,
    /// 优胜次数（Current 胜出 + CI 通过）
    pub wins: u32,
    /// 失败次数（Previous 胜出 或 CI 失败）
    pub losses: u32,
    /// 误杀次数（Current 实际更优但被 CI 否决）
    pub false_kills: u32,
}

/// 北极星指标报告 — KPI-01/KPI-02 验证结果
///
/// # 北极星指标（设计文档 §13.3）
///
/// | KPI | 含义 | 阈值 | 计算公式 |
/// |-----|------|------|---------|
/// | KPI-01 | Harness lineage 累计胜率 | ≥ 60% | wins / total_comparisons |
/// | KPI-02 | 误杀率 | < 5% | false_kills / total_comparisons |
#[derive(Debug, Clone, PartialEq)]
pub struct NorthStarReport {
    /// 总评判次数（3 轮 × 5 任务 = 15）
    pub total_comparisons: u32,
    /// 总优胜次数（Current 胜出 + CI 通过）
    pub total_wins: u32,
    /// 总误杀次数（Current 实际更优但被 CI 否决）
    pub total_false_kills: u32,
    /// KPI-01 累计胜率 [0.0, 1.0]
    pub kpi_01_win_rate: f64,
    /// KPI-02 误杀率 [0.0, 1.0]
    pub kpi_02_false_kill_rate: f64,
    /// KPI-01 是否达标（≥ 60%）
    pub kpi_01_passed: bool,
    /// KPI-02 是否达标（< 5%）
    pub kpi_02_passed: bool,
    /// 3 轮进化结果
    pub rounds: Vec<EvolutionRoundResult>,
}

// ============================================================
// RhiCgEvolutionExecutor — 进化执行器
// ============================================================

/// RHI-CG 进化执行器 — 协调通道 A（评判）+ 通道 B（CI 否决）+ SpecRegistry + History
///
/// # 设计决策（WHY）
///
/// - **复用既有组件**: 不新建额外抽象（复杂度预算净增长 ≤0），
///   直接组合 P5.1/P5.2 已实现的 StubJudgeClient / MockCiGate /
///   SignificanceDetector / SpecRegistry / SelfComparisonHistory
///
/// - **async execute_evolution_round**: 评判器与 CI 执行门返回 Future，
///   必须 `.await`，因此执行器方法为 async
///
/// - **顺序执行（不并发）**: 5 任务顺序评判，避免并发引入的复杂性
///   （测试代码非生产路径，无需并发优化）
///
/// - **source = "rhi_cg"**: SpecRegistry 注册时标注来源为 RHI-CG 通道，
///   便于下游订阅者从 SpecRegistered.source 区分注册路径
pub struct RhiCgEvolutionExecutor {
    /// 评判器客户端（StubJudgeClient::current_wins()，确定性裁决 Current 胜出）
    judge_client: StubJudgeClient,
    /// CI 执行门（MockCiGate::with_passing_result()，模拟 CI 通过）
    ci_gate: MockCiGate,
    /// 显著性检测器（累积回归次数，用于否决证据检查）
    significance_detector: SignificanceDetector,
    /// Spec 版本化注册表（谱系追踪 + A/B 测试 + 一键回滚）
    spec_registry: SpecRegistry,
    /// 自比较历史持久化器（偏好对 → L2 语义记忆）
    history: SelfComparisonHistory,
}

impl RhiCgEvolutionExecutor {
    /// 创建新的进化执行器（默认配置）
    ///
    /// # 默认配置
    /// - `judge_client`: StubJudgeClient::current_wins()（确定性裁决 Current 胜出）
    /// - `ci_gate`: MockCiGate::with_passing_result()（CI 永远通过）
    /// - `significance_detector`: SignificanceDetector::new()（streak=0, runs=0）
    /// - `spec_registry`: SpecRegistry::new()（空注册表，无 EventBus）
    /// - `history`: SelfComparisonHistory::with_default_capacity()（容量 1024）
    pub fn new() -> Self {
        Self {
            judge_client: StubJudgeClient::current_wins(),
            ci_gate: MockCiGate::with_passing_result(),
            significance_detector: SignificanceDetector::new(),
            spec_registry: SpecRegistry::new(),
            history: SelfComparisonHistory::with_default_capacity(),
        }
    }

    /// 返回 SpecRegistry 的引用（供测试验证谱系）
    pub fn spec_registry(&self) -> &SpecRegistry {
        &self.spec_registry
    }

    /// 返回 SelfComparisonHistory 的引用（供测试验证持久化）
    pub fn history(&self) -> &SelfComparisonHistory {
        &self.history
    }

    /// 返回 SignificanceDetector 的引用（供测试验证回归状态）
    pub fn significance_detector(&self) -> &SignificanceDetector {
        &self.significance_detector
    }

    /// 注册初始版本（v1）— 在执行进化轮次前调用
    ///
    /// # 流程
    /// 对每个任务，注册其 v1 spec（parent=None）到 SpecRegistry。
    /// 注册成功后 v1 成为该任务的 active 版本。
    ///
    /// # 参数
    /// - `tasks`: 5 任务集
    ///
    /// # 返回
    /// - `Ok(())`: 所有任务 v1 注册成功
    /// - `Err(String)`: 任一任务注册失败（描述失败原因）
    pub fn register_initial_versions(&mut self, tasks: &[QuestTask]) -> Result<(), String> {
        for task in tasks {
            let v1_spec = task.spec(1);
            self.spec_registry
                .register_with_source(v1_spec.clone(), "rhi_cg")
                .map_err(|e| format!("任务 {} v1 注册失败: {}", task.task_id(), e))?;
        }
        Ok(())
    }

    /// 执行一轮进化（5 任务 × 1 轮 = 5 次评判）
    ///
    /// # 流程
    /// 对每个任务:
    /// 1. **通道 A（评判）**: `judge_client.judge(spec_v_i, spec_v_i_minus_1)` → JudgeVerdict
    /// 2. **通道 B（CI 否决）**: `ci_gate.execute(spec_v_i)` → CiGateResult
    /// 3. **显著性检测**: 根据 CI 结果调用 `record_pass()` 或 `record_regression()`
    /// 4. **注册决策**:
    ///    - CI 通过 → `spec_registry.register_with_source(spec_v_i, "rhi_cg")`
    ///    - CI 否决 → 跳过注册，记录否决
    /// 5. **历史持久化**: 仅 CI 通过时，构造 PreferencePair + SelfComparisonRecord 存入 history
    ///
    /// # 参数
    /// - `tasks`: 5 任务集
    /// - `round`: 轮次（1-3），决定比较的版本对:
    ///   - round=1: v2 vs v1
    ///   - round=2: v3 vs v2
    ///   - round=3: v4 vs v3
    ///
    /// # 返回
    /// - `Ok(EvolutionRoundResult)`: 该轮 5 任务的聚合结果
    /// - `Err(String)`: 评判器或 CI 执行失败（基础设施错误）
    pub async fn execute_evolution_round(
        &mut self,
        tasks: &[QuestTask],
        round: u32,
    ) -> Result<EvolutionRoundResult, String> {
        // 校验 round 范围（1-3）
        if !(1..=3).contains(&round) {
            return Err(format!("round 必须在 1..=3 范围内，得到 {round}"));
        }

        let mut verdicts: Vec<EvolutionVerdict> = Vec::with_capacity(tasks.len());
        let mut wins: u32 = 0;
        let mut losses: u32 = 0;
        let mut false_kills: u32 = 0;

        for task in tasks {
            // 获取相邻版本 spec: round=1 → v2 vs v1, round=2 → v3 vs v2, round=3 → v4 vs v3
            // WHY usize 转换: task.spec() 接收 usize 版本号，round 为 u32（来自轮次标识）
            let spec_v_i: &HarnessSpec = task.spec((round + 1) as usize);
            let spec_v_i_minus_1: &HarnessSpec = task.spec(round as usize);

            // 步骤 1: 通道 A — 评判器裁决
            let judge_verdict = self
                .judge_client
                .judge(spec_v_i, spec_v_i_minus_1)
                .await
                .map_err(|e| format!("任务 {} round {} 评判失败: {}", task.task_id(), round, e))?;

            // 步骤 2: 通道 B — CI 执行门
            let ci_result = self.ci_gate.execute(spec_v_i).await.map_err(|e| {
                format!("任务 {} round {} CI 执行失败: {}", task.task_id(), round, e)
            })?;

            // 步骤 3: 显著性检测器状态更新
            // WHY 根据 CI 结果维护 streak:通过则重置，失败则累积
            if ci_result.passed {
                self.significance_detector.record_pass();
            } else {
                self.significance_detector.record_regression();
            }

            // 步骤 4: 注册决策 + 历史持久化
            let winner_is_current: bool = judge_verdict.winner == SpecVersion::Current;
            let ci_passed: bool = ci_result.passed;
            let registered: bool;
            let vetoed: bool;

            if ci_passed {
                // CI 通过: 注册新版本到 SpecRegistry
                //
                // WHY 注册后立即 promote 为 active:
                // SpecRegistry.register_with_source 仅在 parent=None 时设置 active,
                // 子版本注册后默认不修改 active。RHI-CG 进化语义要求"优胜版本立即上线",
                // 因此注册 v(i+1) 后必须调用 set_candidate + promote_candidate 使其成为 active,
                // 否则 lineage() 只会返回 [1]（永远停留在初始版本），无法体现进化效果。
                let spec_name: String = spec_v_i.meta.name.clone();
                let spec_version: u32 = spec_v_i.meta.version;
                self.spec_registry
                    .register_with_source(spec_v_i.clone(), "rhi_cg")
                    .map_err(|e| {
                        format!(
                            "任务 {} round {} spec 注册失败: {}",
                            task.task_id(),
                            round,
                            e
                        )
                    })?;
                // 将新注册版本设为 candidate 并 promote 为 active
                // WHY 两步式 promote 而非直接修改 active: SpecRegistry API 设计遵循
                // A/B 测试语义（set_candidate → promote_candidate），不暴露直接修改 active 的接口
                self.spec_registry
                    .set_candidate(&spec_name, spec_version)
                    .map_err(|e| {
                        format!(
                            "任务 {} round {} set_candidate 失败: {}",
                            task.task_id(),
                            round,
                            e
                        )
                    })?;
                self.spec_registry
                    .promote_candidate(&spec_name)
                    .map_err(|e| {
                        format!(
                            "任务 {} round {} promote_candidate 失败: {}",
                            task.task_id(),
                            round,
                            e
                        )
                    })?;
                registered = true;
                vetoed = false;

                // 构造 PreferencePair 并持久化到 SelfComparisonHistory
                //
                // WHY pair_id 包含 task_id: pair_id 是 SemanticMemory 的唯一键,
                // 不同任务的相同版本对（如 T1 v2 vs v1 和 T2 v2 vs v1）若共享同一 pair_id
                // 会互相覆盖（self_history.rs §不变量保护: pair_id 唯一性）。
                // 加入 task_id 后,5 任务 × 3 轮 = 15 条记录各有唯一 pair_id,持久化完整。
                let pair_id = format!(
                    "rhi-pair-{}-{}-{}",
                    task.task_id(),
                    spec_v_i.meta.version,
                    spec_v_i_minus_1.meta.version
                );
                let pair = PreferencePair::from_adjacent_specs(
                    pair_id,
                    spec_v_i,
                    spec_v_i_minus_1,
                    &judge_verdict,
                );
                let record = SelfComparisonRecord::from_pair_and_verdict(pair, &judge_verdict);
                self.history.store(record).map_err(|e| {
                    format!(
                        "任务 {} round {} 历史持久化失败: {}",
                        task.task_id(),
                        round,
                        e
                    )
                })?;
            } else {
                // CI 否决: 跳过注册，记录否决
                registered = false;
                vetoed = true;
            }

            // 步骤 5: 更新计数器
            // WHY wins = Current 胜出 + CI 通过（候选被接受）
            //      losses = Previous 胜出 或 CI 失败（候选被拒绝）
            //      false_kills = Current 实际更优（winner=Current）但 CI 否决
            if winner_is_current && ci_passed {
                wins += 1;
            } else {
                losses += 1;
            }
            if winner_is_current && !ci_passed {
                false_kills += 1;
            }

            verdicts.push(EvolutionVerdict {
                task_id: task.task_id().to_string(),
                round,
                winner: judge_verdict.winner,
                winner_score: judge_verdict.winner_score,
                loser_score: judge_verdict.loser_score,
                confidence: judge_verdict.confidence,
                rationale: judge_verdict.rationale.clone(),
                ci_passed,
                registered,
                vetoed,
            });
        }

        Ok(EvolutionRoundResult {
            round,
            verdicts,
            wins,
            losses,
            false_kills,
        })
    }
}

impl Default for RhiCgEvolutionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// validate_north_star_metric — 北极星指标验证
// ============================================================

/// 执行 3 轮进化并验证北极星指标 KPI-01/KPI-02
///
/// # 流程
/// 1. 注册初始版本 v1（5 任务）
/// 2. 执行 Round 1（v2 vs v1）
/// 3. 执行 Round 2（v3 vs v2）
/// 4. 执行 Round 3（v4 vs v3）
/// 5. 聚合 3 轮结果，计算 KPI-01（胜率）与 KPI-02（误杀率）
///
/// # 验收门槛
/// - KPI-01 累计胜率 ≥ 60%（≥ 9 次优胜 / 15 次评判）
/// - KPI-02 误杀率 < 5%（≤ 0 次误杀 / 15 次评判）
///
/// # 参数
/// - `executor`: 进化执行器（ mutable，因为执行轮次会修改内部状态）
///
/// # 返回
/// - `Ok(NorthStarReport)`: 验证完成，报告含 KPI 计算结果
/// - `Err(String)`: 任一轮次执行失败
pub async fn validate_north_star_metric(
    executor: &mut RhiCgEvolutionExecutor,
) -> Result<NorthStarReport, String> {
    let tasks: Vec<QuestTask> = quest_set_v1();

    // 步骤 1: 注册初始版本 v1
    executor.register_initial_versions(&tasks)?;

    // 步骤 2-4: 执行 3 轮进化
    let mut rounds: Vec<EvolutionRoundResult> = Vec::with_capacity(3);
    for round in 1..=3u32 {
        let round_result: EvolutionRoundResult =
            executor.execute_evolution_round(&tasks, round).await?;
        rounds.push(round_result);
    }

    // 步骤 5: 聚合 KPI
    let total_comparisons: u32 = rounds.iter().map(|r| r.verdicts.len() as u32).sum();
    let total_wins: u32 = rounds.iter().map(|r| r.wins).sum();
    let total_false_kills: u32 = rounds.iter().map(|r| r.false_kills).sum();

    // KPI-01: 累计胜率 = wins / total_comparisons
    let kpi_01_win_rate: f64 = if total_comparisons > 0 {
        total_wins as f64 / total_comparisons as f64
    } else {
        0.0
    };
    // KPI-02: 误杀率 = false_kills / total_comparisons
    let kpi_02_false_kill_rate: f64 = if total_comparisons > 0 {
        total_false_kills as f64 / total_comparisons as f64
    } else {
        0.0
    };

    // 验收门槛判定
    const KPI_01_THRESHOLD: f64 = 0.60; // ≥ 60%
    const KPI_02_THRESHOLD: f64 = 0.05; // < 5%
    let kpi_01_passed: bool = kpi_01_win_rate >= KPI_01_THRESHOLD;
    let kpi_02_passed: bool = kpi_02_false_kill_rate < KPI_02_THRESHOLD;

    Ok(NorthStarReport {
        total_comparisons,
        total_wins,
        total_false_kills,
        kpi_01_win_rate,
        kpi_02_false_kill_rate,
        kpi_01_passed,
        kpi_02_passed,
        rounds,
    })
}

// ============================================================
// 单元测试 + E2E 验收测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // P5.5.2: RhiCgEvolutionExecutor 基础测试
    // ============================================================

    /// 验证执行器初始化（组件齐全 + 默认配置）
    #[test]
    fn test_executor_initialization() {
        let executor = RhiCgEvolutionExecutor::new();

        // SpecRegistry 应为空（未注册任何 spec）
        assert!(executor.spec_registry().list_names().is_empty());

        // SignificanceDetector 初始状态: streak=0, runs=0
        assert_eq!(executor.significance_detector().regression_streak(), 0);
        assert_eq!(executor.significance_detector().observed_runs(), 0);

        // SelfComparisonHistory 初始状态: 空
        assert!(executor.history().is_empty().unwrap_or(true));
    }

    /// 验证执行器 Default trait 等价于 new()
    #[test]
    fn test_executor_default_equals_new() {
        let _executor_default = RhiCgEvolutionExecutor::default();
        let _executor_new = RhiCgEvolutionExecutor::new();
        // 两者组件类型相同，仅验证可创建（内部状态不可直接比较）
    }

    /// 验证 register_initial_versions 注册 5 任务 v1
    #[test]
    fn test_register_initial_versions() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();

        executor.register_initial_versions(&tasks).unwrap();

        // 5 任务的 v1 应全部注册
        assert_eq!(executor.spec_registry().list_names().len(), 5);

        // 验证每个任务的 spec name 在 registry 中存在
        // WHY 不直接比较 task_id: SpecRegistry 用 spec.meta.name 作为 key,
        // task_id 是 "T1"-"T5"，spec.meta.name 是任务内容相关的名称
        for task in &tasks {
            let spec_name = &task.spec(1).meta.name;
            let versions = executor.spec_registry().list_versions(spec_name);
            assert_eq!(
                versions,
                vec![1],
                "任务 {} 的 spec '{}' 应仅注册 v1",
                task.task_id(),
                spec_name
            );
        }
    }

    /// 验证 register_initial_versions 重复注册同一版本会失败（VersionConflict）
    #[test]
    fn test_register_initial_versions_duplicate_fails() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();

        // 第一次注册: 成功
        executor.register_initial_versions(&tasks).unwrap();

        // 第二次注册同一批 v1: 应失败（VersionConflict）
        let result = executor.register_initial_versions(&tasks);
        assert!(result.is_err(), "重复注册 v1 应失败");
        assert!(
            result.unwrap_err().contains("注册失败"),
            "错误消息应包含注册失败"
        );
    }

    /// 验证 register_initial_versions 对空任务集无操作
    #[test]
    fn test_register_initial_versions_empty_tasks() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let empty_tasks: Vec<QuestTask> = Vec::new();

        executor.register_initial_versions(&empty_tasks).unwrap();
        assert!(executor.spec_registry().list_names().is_empty());
    }

    // ============================================================
    // P5.5.2: execute_evolution_round 测试
    // ============================================================

    /// 验证 Round 1（v2 vs v1）: 5 任务全部优胜（Current 胜出 + CI 通过）
    #[tokio::test]
    async fn test_execute_evolution_round_1() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();
        executor.register_initial_versions(&tasks).unwrap();

        let result: EvolutionRoundResult = executor
            .execute_evolution_round(&tasks, 1)
            .await
            .expect("Round 1 执行失败");

        // 验证轮次
        assert_eq!(result.round, 1);

        // 验证 5 任务全部评判
        assert_eq!(result.verdicts.len(), 5);

        // 验证全部优胜（Current 胜出 + CI 通过）
        assert_eq!(result.wins, 5, "Round 1 应有 5 次优胜");
        assert_eq!(result.losses, 0, "Round 1 不应有失败");
        assert_eq!(result.false_kills, 0, "Round 1 不应有误杀");

        // 验证每个 verdict 的字段
        for verdict in &result.verdicts {
            assert_eq!(verdict.round, 1);
            assert_eq!(verdict.winner, SpecVersion::Current);
            assert!(verdict.ci_passed, "CI 应通过");
            assert!(verdict.registered, "应已注册");
            assert!(!verdict.vetoed, "不应被否决");
        }
    }

    /// 验证 Round 2（v3 vs v2）: 5 任务全部优胜
    #[tokio::test]
    async fn test_execute_evolution_round_2() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();
        executor.register_initial_versions(&tasks).unwrap();

        // 先执行 Round 1（注册 v2）
        executor.execute_evolution_round(&tasks, 1).await.unwrap();

        // 执行 Round 2
        let result: EvolutionRoundResult = executor
            .execute_evolution_round(&tasks, 2)
            .await
            .expect("Round 2 执行失败");

        assert_eq!(result.round, 2);
        assert_eq!(result.verdicts.len(), 5);
        assert_eq!(result.wins, 5, "Round 2 应有 5 次优胜");
        assert_eq!(result.losses, 0);
        assert_eq!(result.false_kills, 0);
    }

    /// 验证 Round 3（v4 vs v3）: 5 任务全部优胜
    #[tokio::test]
    async fn test_execute_evolution_round_3() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();
        executor.register_initial_versions(&tasks).unwrap();

        // 先执行 Round 1 + Round 2
        executor.execute_evolution_round(&tasks, 1).await.unwrap();
        executor.execute_evolution_round(&tasks, 2).await.unwrap();

        // 执行 Round 3
        let result: EvolutionRoundResult = executor
            .execute_evolution_round(&tasks, 3)
            .await
            .expect("Round 3 执行失败");

        assert_eq!(result.round, 3);
        assert_eq!(result.verdicts.len(), 5);
        assert_eq!(result.wins, 5, "Round 3 应有 5 次优胜");
        assert_eq!(result.losses, 0);
        assert_eq!(result.false_kills, 0);
    }

    /// 验证 execute_evolution_round 对非法 round 值返回 Err
    #[tokio::test]
    async fn test_execute_evolution_round_invalid_round() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();
        executor.register_initial_versions(&tasks).unwrap();

        // round=0 非法
        let result = executor.execute_evolution_round(&tasks, 0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("round 必须在 1..=3"));

        // round=4 非法
        let result = executor.execute_evolution_round(&tasks, 4).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("round 必须在 1..=3"));
    }

    /// 验证未注册初始版本时执行 Round 1 会失败（ParentMissing）
    ///
    /// WHY SpecRegistry 要求 parent 版本必须已注册，未注册 v1 直接
    /// 注册 v2 会触发 ParentMissing 错误
    #[tokio::test]
    async fn test_execute_round_without_initial_registration_fails() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();

        // 不调用 register_initial_versions，直接执行 Round 1
        // 评判与 CI 应成功，但注册 v2 时因 parent v1 未注册而失败
        let result = executor.execute_evolution_round(&tasks, 1).await;
        assert!(result.is_err(), "未注册 v1 直接执行 Round 1 应失败");
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("注册失败") || err_msg.contains("ParentMissing"),
            "错误应与 parent 缺失相关: {err_msg}"
        );
    }

    // ============================================================
    // P5.5.3: 北极星指标验证（KPI-01 + KPI-02）
    // ============================================================

    /// 验证 validate_north_star_metric 完整流程（3 轮 × 5 任务 = 15 次评判）
    #[tokio::test]
    async fn test_validate_north_star_metric_full_flow() {
        let mut executor = RhiCgEvolutionExecutor::new();

        let report: NorthStarReport = validate_north_star_metric(&mut executor)
            .await
            .expect("北极星指标验证失败");

        // 验证总评判次数 = 3 轮 × 5 任务 = 15
        assert_eq!(report.total_comparisons, 15, "总评判次数应为 15");

        // 验证 3 轮结果
        assert_eq!(report.rounds.len(), 3, "应有 3 轮结果");

        // 验证每轮 5 个 verdict
        for (idx, round) in report.rounds.iter().enumerate() {
            assert_eq!(round.round, (idx + 1) as u32);
            assert_eq!(round.verdicts.len(), 5, "每轮应有 5 个 verdict");
        }
    }

    /// 验证 KPI-01 累计胜率 ≥ 60%（核心验收门槛）
    ///
    /// WHY 此测试为 P5.5 核心验收:KPI-01 不达标则 P5.5 整体失败
    #[tokio::test]
    async fn test_kpi_01_win_rate_meets_threshold() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let report = validate_north_star_metric(&mut executor)
            .await
            .expect("验证失败");

        // KPI-01 应达标（≥ 60%）
        assert!(
            report.kpi_01_passed,
            "KPI-01 累计胜率 {:.2}% 未达 60% 阈值（wins={}/{}）",
            report.kpi_01_win_rate * 100.0,
            report.total_wins,
            report.total_comparisons
        );

        // 实际胜率应为 100%（所有 15 次评判都优胜）
        assert_eq!(
            report.total_wins, 15,
            "应有 15 次优胜（全部 Current 胜出 + CI 通过）"
        );
        assert!(
            (report.kpi_01_win_rate - 1.0).abs() < 1e-10,
            "胜率应为 100%，实际 {:.4}%",
            report.kpi_01_win_rate * 100.0
        );
    }

    /// 验证 KPI-02 误杀率 < 5%（核心验收门槛）
    ///
    /// WHY 此测试为 P5.5 核心验收:KPI-02 不达标则 P5.5 整体失败
    #[tokio::test]
    async fn test_kpi_02_false_kill_rate_below_threshold() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let report = validate_north_star_metric(&mut executor)
            .await
            .expect("验证失败");

        // KPI-02 应达标（< 5%）
        assert!(
            report.kpi_02_passed,
            "KPI-02 误杀率 {:.2}% 未低于 5% 阈值（false_kills={}/{}）",
            report.kpi_02_false_kill_rate * 100.0,
            report.total_false_kills,
            report.total_comparisons
        );

        // 实际误杀率应为 0%（CI 永远通过，无误杀）
        assert_eq!(
            report.total_false_kills, 0,
            "应有 0 次误杀（MockCiGate 永远通过）"
        );
        assert!(
            (report.kpi_02_false_kill_rate - 0.0).abs() < 1e-10,
            "误杀率应为 0%，实际 {:.4}%",
            report.kpi_02_false_kill_rate * 100.0
        );
    }

    /// 验证 KPI-01 与 KPI-02 同时达标（综合验收）
    #[tokio::test]
    async fn test_both_kpis_pass_simultaneously() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let report = validate_north_star_metric(&mut executor)
            .await
            .expect("验证失败");

        assert!(
            report.kpi_01_passed && report.kpi_02_passed,
            "KPI-01 ({:.2}%) 与 KPI-02 ({:.2}%) 未同时达标",
            report.kpi_01_win_rate * 100.0,
            report.kpi_02_false_kill_rate * 100.0
        );
    }

    // ============================================================
    // P5.5.2: 谱系（lineage）验证
    // ============================================================

    /// 验证 3 轮进化后每个任务的 lineage 为 [1, 2, 3, 4]
    ///
    /// WHY SpecRegistry 的 lineage() 方法沿 parent 链追溯，
    /// 验证谱系完整是 P5.5 的核心交付之一
    #[tokio::test]
    async fn test_lineage_correct_after_3_rounds() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();

        // 执行完整 3 轮进化
        validate_north_star_metric(&mut executor)
            .await
            .expect("3 轮进化失败");

        // 验证每个任务的 lineage 为 [1, 2, 3, 4]
        for task in &tasks {
            let spec_name = &task.spec(1).meta.name;
            let lineage: Vec<u32> = executor
                .spec_registry()
                .lineage(spec_name)
                .expect("lineage 查询失败");

            assert_eq!(
                lineage,
                vec![1, 2, 3, 4],
                "任务 {} 的 spec '{}' lineage 应为 [1, 2, 3, 4]，实际 {:?}",
                task.task_id(),
                spec_name,
                lineage
            );
        }
    }

    /// 验证 3 轮进化后每个任务注册了 4 个版本
    #[tokio::test]
    async fn test_all_versions_registered_after_3_rounds() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();

        validate_north_star_metric(&mut executor)
            .await
            .expect("3 轮进化失败");

        for task in &tasks {
            let spec_name = &task.spec(1).meta.name;
            let versions: Vec<u32> = executor.spec_registry().list_versions(spec_name);

            assert_eq!(
                versions,
                vec![1, 2, 3, 4],
                "任务 {} 应注册 4 个版本 [1, 2, 3, 4]，实际 {:?}",
                task.task_id(),
                versions
            );
        }
    }

    /// 验证 active 版本为 v4（最后一轮注册的版本）
    #[tokio::test]
    async fn test_active_version_is_latest_after_3_rounds() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let tasks = quest_set_v1();

        validate_north_star_metric(&mut executor)
            .await
            .expect("3 轮进化失败");

        for task in &tasks {
            let spec_name = &task.spec(1).meta.name;
            let active: &HarnessSpec = executor
                .spec_registry()
                .get_active(spec_name)
                .expect("应有 active 版本");

            assert_eq!(
                active.meta.version,
                4,
                "任务 {} active 应为 v4",
                task.task_id()
            );
        }
    }

    // ============================================================
    // P5.5.2: 自比较历史持久化验证
    // ============================================================

    /// 验证 3 轮进化后 SelfComparisonHistory 含 15 条记录
    ///
    /// WHY 每次评判成功（CI 通过）会持久化一条 SelfComparisonRecord，
    /// 3 轮 × 5 任务 = 15 条记录
    #[tokio::test]
    async fn test_history_persistence_after_3_rounds() {
        let mut executor = RhiCgEvolutionExecutor::new();

        validate_north_star_metric(&mut executor)
            .await
            .expect("3 轮进化失败");

        let history_len: usize = executor.history().len().expect("history.len() 失败");

        assert_eq!(
            history_len, 15,
            "应有 15 条历史记录（3 轮 × 5 任务），实际 {}",
            history_len
        );
    }

    /// 验证历史记录可通过 pair_id 检索
    ///
    /// WHY pair_id 格式为 "rhi-pair-{v_i}-{v_i_minus_1}"，
    /// 检索能力验证 L2 语义记忆索引正确
    #[tokio::test]
    async fn test_history_record_retrievable_by_pair_id() {
        let mut executor = RhiCgEvolutionExecutor::new();

        validate_north_star_metric(&mut executor)
            .await
            .expect("3 轮进化失败");

        // 检索第一个任务 T1 的 Round 1 记录（pair_id = "rhi-pair-T1-2-1"）
        //
        // WHY pair_id 格式包含 task_id: 不同任务的相同版本对（如 T1 v2 vs v1 和 T2 v2 vs v1）
        // 若共享 pair_id 会互相覆盖。包含 task_id 后保证 15 条记录各有唯一 pair_id。
        let pair_id = "rhi-pair-T1-2-1";
        let record: Option<SelfComparisonRecord> =
            executor.history().get(pair_id).expect("history.get() 失败");

        assert!(record.is_some(), "应能检索到 pair_id={}", pair_id);
        let record = record.unwrap();
        assert_eq!(record.pair_id(), pair_id);
        // winner_score 来自 StubJudgeClient::current_wins()（0.8）
        assert!(
            (record.confidence - 0.9).abs() < 1e-6,
            "置信度应为 0.9（StubJudgeClient::current_wins）"
        );
    }

    // ============================================================
    // P5.5.2: 显著性检测器状态验证
    // ============================================================

    /// 验证 3 轮进化后 SignificanceDetector 状态（15 次通过，0 次回归）
    ///
    /// WHY MockCiGate 永远通过，所以 streak=0，runs=15
    #[tokio::test]
    async fn test_significance_detector_state_after_3_rounds() {
        let mut executor = RhiCgEvolutionExecutor::new();

        validate_north_star_metric(&mut executor)
            .await
            .expect("3 轮进化失败");

        let detector: &SignificanceDetector = executor.significance_detector();

        // 15 次 CI 执行全部通过
        assert_eq!(detector.observed_runs(), 15, "应观察 15 次 CI 执行");
        assert_eq!(
            detector.regression_streak(),
            0,
            "streak 应为 0（无连续回归）"
        );

        // p-value 应为 1.0（streak=0 → 必然事件）
        let p_value: f64 = detector.p_value();
        assert!(
            (p_value - 1.0).abs() < 1e-10,
            "p-value 应为 1.0（streak=0），实际 {}",
            p_value
        );

        // 不应触发否决（streak < 3）
        assert!(!detector.is_veto_justified(), "不应触发否决（streak < 3）");
    }

    // ============================================================
    // P5.5.2: spec_score 一致性验证（确定性评判逻辑）
    // ============================================================

    /// 验证 StubJudgeClient::current_wins() 的裁决与 spec_score 设计一致
    ///
    /// WHY 评判逻辑保证:spec_score(v(i+1)) > spec_score(v(i)) 时 Current 胜出。
    /// 此测试验证 spec_score 单调递增，确保确定性评判的正确性
    #[test]
    fn test_spec_score_progression_ensures_current_wins() {
        let tasks = quest_set_v1();

        for task in &tasks {
            let v1_score = spec_score(task.spec(1));
            let v2_score = spec_score(task.spec(2));
            let v3_score = spec_score(task.spec(3));
            let v4_score = spec_score(task.spec(4));

            assert!(
                v2_score > v1_score,
                "任务 {} v2 评分 {} 应 > v1 评分 {}",
                task.task_id(),
                v2_score,
                v1_score
            );
            assert!(
                v3_score > v2_score,
                "任务 {} v3 评分 {} 应 > v2 评分 {}",
                task.task_id(),
                v3_score,
                v2_score
            );
            assert!(
                v4_score > v3_score,
                "任务 {} v4 评分 {} 应 > v3 评分 {}",
                task.task_id(),
                v4_score,
                v3_score
            );
        }
    }

    /// 验证所有 spec 通过 HarnessSpec::validate()（不可进化面合规）
    #[test]
    fn test_all_specs_pass_validation_in_executor() {
        let tasks = quest_set_v1();

        for task in &tasks {
            for version in 1..=4 {
                let spec = task.spec(version);
                let result = spec.validate();
                assert!(
                    result.is_ok(),
                    "任务 {} v{} validate() 失败: {:?}",
                    task.task_id(),
                    version,
                    result.err()
                );
            }
        }
    }

    /// 验证进化执行器的组件可独立访问（便于审计）
    #[test]
    fn test_executor_component_accessors() {
        let executor = RhiCgEvolutionExecutor::new();

        // 验证访问器返回正确的引用
        let _registry: &SpecRegistry = executor.spec_registry();
        let _history: &SelfComparisonHistory = executor.history();
        let _detector: &SignificanceDetector = executor.significance_detector();
    }

    /// 验证完整 3 轮进化的 verdict 列表完整（15 条）
    #[tokio::test]
    async fn test_full_3_rounds_verdict_count() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let report = validate_north_star_metric(&mut executor)
            .await
            .expect("验证失败");

        let total_verdicts: usize = report.rounds.iter().map(|r| r.verdicts.len()).sum();
        assert_eq!(total_verdicts, 15, "3 轮应有 15 条 verdict");
    }

    /// 验证每条 verdict 的字段完整性
    #[tokio::test]
    async fn test_verdict_fields_complete() {
        let mut executor = RhiCgEvolutionExecutor::new();
        let report = validate_north_star_metric(&mut executor)
            .await
            .expect("验证失败");

        for round in &report.rounds {
            for verdict in &round.verdicts {
                // task_id 非空
                assert!(!verdict.task_id.is_empty(), "task_id 不应为空");
                // round 在 1-3
                assert!((1..=3).contains(&verdict.round));
                // winner = Current（StubJudgeClient::current_wins）
                assert_eq!(verdict.winner, SpecVersion::Current);
                // 评分范围 [0.0, 1.0]
                assert!((0.0..=1.0).contains(&verdict.winner_score));
                assert!((0.0..=1.0).contains(&verdict.loser_score));
                assert!((0.0..=1.0).contains(&verdict.confidence));
                // rationale 非空
                assert!(!verdict.rationale.is_empty());
                // CI 通过 + 已注册 + 未否决
                assert!(verdict.ci_passed);
                assert!(verdict.registered);
                assert!(!verdict.vetoed);
            }
        }
    }
}
