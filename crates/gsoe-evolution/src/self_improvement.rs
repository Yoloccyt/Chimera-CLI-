//! SelfImprovementPipeline — 自我改进流水线(PenguinHarness 四步,文档 §10.3.4)
//!
//! 对应架构层:L5 Knowledge(gsoe-evolution 子模块)
//! 对应设计源:PenguinHarness「Agent Creation → Benchmark Design →
//! Agent Evaluation → Agent Optimization」完整流水线(文档 §10.3.4)
//!
//! # 降级实现(R2 冻结合规,ADR-042)
//!
//! 文档 §10.3.4 规划完整流水线;本实现为**规则/统计驱动的降级版本**
//! (与 AEGIS-lite 同风格),四步语义保留但每步为确定性规则:
//! - `TemplateCreator`:按仓库类型生成模板 HarnessSpec(不生成代码)
//! - `RuleBenchmarkDesigner`:默认场景集(测试通过/无回归/合约校验)
//! - `RuleBasedEvaluator`:合约覆盖场景比例评分(确定性)
//! - `AegisPipeline`(Optimizer):复用 L5 四阶段进化(已落地能力编排)
//!
//! 无梯度更新、无 RL 训练路径;R2 解冻后各步可升级为学习驱动(需新 ADR)。
//!
//! # 依赖方向(§2.2)
//!
//! 本模块仅依赖 crate 内能力(AegisPipeline/CiGate)与 L0 nexus-contracts
//! 类型;不向上依赖 L9/L10。

use crate::aegis::{AegisPipeline, TrajectoryOutcome};
use crate::ci_gate::CiGate;
use nexus_contracts::{HarnessMeta, HarnessSpec, RetryPolicy};

/// 改进需求 — 自我改进流水线的输入(文档 §10.3.4 `Requirements`)
#[derive(Debug, Clone, PartialEq)]
pub struct ImprovementRequirements {
    /// 目标仓库类型(如 "rust-workspace" / "python-package")
    pub target_repo_kind: String,
    /// 达标分数(评估分数 ≥ 此值即满足要求)
    pub min_score: f64,
    /// 最大迭代次数(文档对齐:循环继续测试直到满足要求,上限 5)
    pub max_iterations: u32,
}

impl ImprovementRequirements {
    /// 创建改进需求
    pub fn new(target_repo_kind: impl Into<String>, min_score: f64, max_iterations: u32) -> Self {
        Self {
            target_repo_kind: target_repo_kind.into(),
            min_score,
            max_iterations,
        }
    }
}

/// 评估场景 — Benchmark 中的单个验证场景
#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkScenario {
    /// 场景名(如 "tests_pass"),与 ContractSpec.name 对应
    pub name: String,
    /// 场景描述(人类可读)
    pub description: String,
}

/// 评估基准 — 场景集(文档 §10.3.4 `benchmark`)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Benchmark {
    /// 评估场景列表
    pub scenarios: Vec<BenchmarkScenario>,
}

/// 改进结果 — 流水线输出(文档 §10.3.4 `ImprovementResult`)
#[derive(Debug, Clone, PartialEq)]
pub struct ImprovementResult {
    /// 最终候选 spec(达标或迭代至上限时的最佳版本)
    pub final_spec: HarnessSpec,
    /// 实际迭代次数
    pub iterations: u32,
    /// 最终分数
    pub final_score: f64,
    /// 是否满足要求(最终分数 ≥ 需求的 min_score,由 run 计算携带)
    pub meets_requirements: bool,
}

/// Agent 创建器 — 流水线第一步(文档 §10.3.4 Step 1)
pub trait AgentCreator {
    /// 基于需求创建候选 HarnessSpec(模板式,不生成代码)
    fn create(&self, requirements: &ImprovementRequirements) -> HarnessSpec;
}

/// 规则模板创建器 — 按仓库类型映射任务类型,生成最小合法候选
#[derive(Debug, Default, Clone, Copy)]
pub struct TemplateCreator;

impl AgentCreator for TemplateCreator {
    fn create(&self, requirements: &ImprovementRequirements) -> HarnessSpec {
        // WHY 模板式:任务类型映射是规则表(R2 冻结下无学习路径),
        // 未知仓库类型回退 "code_fix" 通用类型。
        let task_type = match requirements.target_repo_kind.as_str() {
            "rust-workspace" => "rust_workspace",
            "python-package" => "python_package",
            _ => "code_fix",
        };
        HarnessSpec {
            meta: HarnessMeta {
                name: "self-improved-spec".to_string(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: Some(task_type.to_string()),
            },
            contracts: vec![],
            hops: vec![],
            retry: RetryPolicy::default(),
            auxiliary: None,
        }
    }
}

/// 基准设计器 — 流水线第二步(文档 §10.3.4 Step 2)
pub trait BenchmarkDesigner {
    /// 基于候选 spec 设计评估场景集
    fn design(&self, spec: &HarnessSpec) -> Benchmark;
}

/// 规则基准设计器 — 默认场景集 + 合约场景
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBenchmarkDesigner;

impl BenchmarkDesigner for RuleBenchmarkDesigner {
    fn design(&self, spec: &HarnessSpec) -> Benchmark {
        // WHY 默认场景集固定:规则式基准设计不依赖失败轨迹历史,
        // 保证空经验下流水线仍可评估(可测试、可复现)。
        let mut scenarios = vec![
            BenchmarkScenario {
                name: "no_panic".to_string(),
                description: "模糊目标不得 panic".to_string(),
            },
            BenchmarkScenario {
                name: "tests_pass".to_string(),
                description: "测试套件全绿".to_string(),
            },
            BenchmarkScenario {
                name: "bench_no_regression".to_string(),
                description: "性能基准无回退".to_string(),
            },
        ];
        // 已有合约 → 每个合约一个对应场景(评估覆盖语义)
        for contract in &spec.contracts {
            if !scenarios.iter().any(|s| s.name == contract.name) {
                scenarios.push(BenchmarkScenario {
                    name: contract.name.clone(),
                    description: contract
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("合约 {} 必须满足", contract.name)),
                });
            }
        }
        Benchmark { scenarios }
    }
}

/// Agent 评估器 — 流水线第三步(文档 §10.3.4 Step 3)
pub trait AgentEvaluator {
    /// 评估候选 spec 在基准上的得分(0.0 ~ 1.0)
    fn evaluate(&self, spec: &HarnessSpec, benchmark: &Benchmark) -> f64;
}

/// 规则评估器 — 合约覆盖场景比例评分(确定性)
///
/// score = 被 spec.contracts 覆盖的场景数 / 场景总数
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedEvaluator;

impl AgentEvaluator for RuleBasedEvaluator {
    fn evaluate(&self, spec: &HarnessSpec, benchmark: &Benchmark) -> f64 {
        if benchmark.scenarios.is_empty() {
            return 0.0;
        }
        let covered = benchmark
            .scenarios
            .iter()
            .filter(|s| spec.contracts.iter().any(|c| c.name == s.name))
            .count();
        covered as f64 / benchmark.scenarios.len() as f64
    }
}

/// 自我改进流水线 — 四步编排(文档 §10.3.4 完整语义,降级实现)
///
/// 循环:创建 → 设计基准 → 评估 → 优化(复用 AEGIS 四阶段),
/// 直到满足要求或达到迭代上限。
pub struct SelfImprovementPipeline {
    /// Step 1: Target Agent 创建器
    creator: TemplateCreator,
    /// Step 2: 基准设计器
    benchmark_designer: RuleBenchmarkDesigner,
    /// Step 3: 评估器
    evaluator: RuleBasedEvaluator,
    /// Step 4: 优化器(复用 L5 AEGIS-lite 四阶段)
    optimizer: AegisPipeline,
}

impl Default for SelfImprovementPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfImprovementPipeline {
    /// 创建默认配置的自我改进流水线
    pub fn new() -> Self {
        Self {
            creator: TemplateCreator,
            benchmark_designer: RuleBenchmarkDesigner,
            evaluator: RuleBasedEvaluator,
            optimizer: AegisPipeline::new(),
        }
    }

    /// 完整流水线(文档 §10.3.4 `run` 语义)
    ///
    /// # 参数
    /// - `requirements`:改进需求(仓库类型/达标分/迭代上限)
    /// - `ci_gate`:CI 执行门(生产用 `CargoCiGate`,测试用 `MockCiGate`)
    ///
    /// # 流程
    /// 1. 创建候选 spec
    /// 2. 设计评估基准
    /// 3. 评估;达标或迭代至上限 → 返回结果
    /// 4. 低分转失败轨迹,喂 AEGIS 四阶段生成变体;接受变体则更新候选
    ///
    /// # 错误策略(WHY)
    /// CI 门执行失败(如 cargo 不可达)时记录 warning 并保持当前候选,
    /// 流水线不因单轮优化失败而中断(降级语义)。
    pub async fn run(
        &mut self,
        requirements: &ImprovementRequirements,
        ci_gate: &dyn CiGate,
    ) -> ImprovementResult {
        // Step 1: 创建 Target Agent(模板候选 spec)
        let mut current = self.creator.create(requirements);

        // Step 2: 设计测试任务和评价标准
        let benchmark = self.benchmark_designer.design(&current);

        // Step 3+4: 执行测试并循环优化(迭代上限对齐文档示例 5)
        let mut iterations = 0u32;
        loop {
            // Step 3: 执行评估
            let score = self.evaluator.evaluate(&current, &benchmark);
            let meets = score >= requirements.min_score;
            if meets || iterations >= requirements.max_iterations {
                return ImprovementResult {
                    final_spec: current,
                    iterations,
                    final_score: score,
                    meets_requirements: meets,
                };
            }

            // Step 4: 低分转失败轨迹 → AEGIS 四阶段优化
            let trajectory = TrajectoryOutcome::failed(
                format!("self-improvement-iter-{iterations}"),
                "low_score",
                "self-improvement-evaluator",
                100,
            );
            match self
                .optimizer
                .run_once(&[trajectory], &current, ci_gate)
                .await
            {
                Ok(verdict) => {
                    // accepted 是 Critic 直接裁决的 HarnessSpec(≤1 个)
                    if let Some(improved) = verdict.accepted {
                        current = improved;
                    }
                }
                Err(err) => {
                    // CI 门失败:保持当前候选继续下一轮(降级语义)
                    tracing::warn!(
                        error = %err,
                        iterations,
                        "自我改进流水线优化失败,保持当前候选"
                    );
                }
            }
            iterations += 1;
        }
    }
}
