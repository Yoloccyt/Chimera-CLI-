//! P1-3(计划 Task 6):自我改进流水线测试(PenguinHarness 四步,文档 §10.3.4)
//!
//! 覆盖:
//! - 初始 spec 达标时零迭代即停
//! - 不达标时迭代至上限并返回最终分数
//! - Creator 按仓库类型生成候选 spec
//! - BenchmarkDesigner 规则式生成评估场景
//! - Evaluator 按合约覆盖场景比例评分
//!
//! 降级实现说明(R2 冻结合规,ADR-042):全程规则/统计驱动,
//! 无梯度更新、无 RL 训练路径;四步复用 L5 已有能力编排。

use gsoe_evolution::ci_gate::MockCiGate;
use gsoe_evolution::self_improvement::{
    AgentCreator, AgentEvaluator, BenchmarkDesigner, ImprovementRequirements, RuleBasedEvaluator,
    RuleBenchmarkDesigner, SelfImprovementPipeline, TemplateCreator,
};

/// 初始 spec 达标(min_score=0)时零迭代即停
#[tokio::test]
async fn test_run_meets_requirements_immediately() {
    let requirements = ImprovementRequirements::new("rust-workspace", 0.0, 5);
    let mut pipeline = SelfImprovementPipeline::new();
    let gate = MockCiGate::with_passing_result();

    let result = pipeline.run(&requirements, &gate).await;
    assert_eq!(result.iterations, 0, "达标即停,不进入迭代");
    assert!(result.final_score >= 0.0);
    assert!(result.meets_requirements);
}

/// 不达标时迭代至 max_iterations 上限
#[tokio::test]
async fn test_run_iterates_until_max() {
    // min_score=1.0 且模板 spec 无合约 → 分数恒 0,必然迭代至上限
    let requirements = ImprovementRequirements::new("rust-workspace", 1.0, 3);
    let mut pipeline = SelfImprovementPipeline::new();
    let gate = MockCiGate::with_passing_result();

    let result = pipeline.run(&requirements, &gate).await;
    assert_eq!(result.iterations, 3, "应迭代至 max_iterations");
    assert!(!result.meets_requirements);
}

/// Creator 按仓库类型生成候选 spec(task_type 映射)
#[test]
fn test_creator_generates_spec_by_repo_kind() {
    let creator = TemplateCreator;
    let requirements = ImprovementRequirements::new("rust-workspace", 0.8, 5);

    let spec = creator.create(&requirements);
    assert_eq!(spec.meta.name, "self-improved-spec");
    assert_eq!(spec.meta.version, 1);
    assert_eq!(spec.meta.parent, None);
    assert!(spec.meta.task_type.is_some());
}

/// BenchmarkDesigner 规则式生成非空评估场景集
#[test]
fn test_designer_builds_scenarios() {
    let creator = TemplateCreator;
    let designer = RuleBenchmarkDesigner;
    let requirements = ImprovementRequirements::new("rust-workspace", 0.8, 5);

    let spec = creator.create(&requirements);
    let benchmark = designer.design(&spec);
    assert!(
        !benchmark.scenarios.is_empty(),
        "评估场景集不得为空(空场景无评估依据)"
    );
    // 默认场景至少包含测试通过/无回归两项
    let names: Vec<&str> = benchmark
        .scenarios
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(names.contains(&"tests_pass"));
    assert!(names.contains(&"bench_no_regression"));
}

/// Evaluator 按合约覆盖场景比例评分
#[test]
fn test_evaluator_scores_coverage() {
    let creator = TemplateCreator;
    let designer = RuleBenchmarkDesigner;
    let evaluator = RuleBasedEvaluator;
    let requirements = ImprovementRequirements::new("rust-workspace", 0.8, 5);

    // 无合约 spec → 覆盖 0 → 分数 0
    let bare_spec = creator.create(&requirements);
    let benchmark = designer.design(&bare_spec);
    let score = evaluator.evaluate(&bare_spec, &benchmark);
    assert_eq!(score, 0.0, "无合约覆盖场景集,分数为 0");

    // 覆盖全部默认场景的 spec → 分数 1.0
    let mut full_spec = bare_spec.clone();
    full_spec.contracts = benchmark
        .scenarios
        .iter()
        .map(|s| nexus_contracts::ContractSpec {
            name: s.name.clone(),
            property: format!("scenario_{}_must_pass", s.name),
            description: None,
            from: None,
            to: None,
            fields: vec![],
        })
        .collect();
    let full_score = evaluator.evaluate(&full_spec, &benchmark);
    assert!((full_score - 1.0).abs() < 1e-6, "全场景覆盖应得满分");
}
