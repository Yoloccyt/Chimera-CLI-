//! polish-v2.7 P3-7:变体治理闭环 E2E 测试
//!
//! 对应架构层:L5 Knowledge(gsoe-evolution AEGIS)+ L8 Parliament(变体池/审议)
//! 对应 ADR:ADR-050(AEGIS-lite)/ ADR-051(Variant Pool)
//! 对应 KPI:KPI-P3(100% 变体经 Critic + Parliament 审议)
//!
//! # 测试覆盖(评估-进化-协同闭环,方案 §15.2 的规则化落地)
//!
//! 1. **全链路通过路径**:轨迹 → AEGIS 四阶段 → 变体 → Parliament 审议通过
//!    → SpecRegistry 登记 → lineage 正确 → 一键回滚可用
//! 2. **审议否决路径**:Security 关切 → 一票否决,变体不入池不登记
//! 3. **奖励欺骗守护**:超限变体被 Critic 拒绝,不进入审议

use gsoe_evolution::ci_gate::MockCiGate;
use gsoe_evolution::spec_registry::SpecRegistry;
use gsoe_evolution::{AegisPipeline, TrajectoryOutcome};
use nexus_contracts::{HarnessMeta, HarnessSpec, RetryPolicy, VariantContract, VariantId};
use parliament::{ReviewDecision, VariantPool, VariantReview};

/// 构造基线 spec(版本 1)
fn base_spec() -> HarnessSpec {
    HarnessSpec {
        meta: HarnessMeta {
            name: "e2e-governance-spec".into(),
            version: 1,
            immutable: false,
            parent: None,
            task_type: Some("code_fix".into()),
        },
        contracts: vec![],
        hops: vec![],
        retry: RetryPolicy::default(),
        auxiliary: None,
    }
}

/// timeout 主导的失败轨迹批次(触发 RelaxRetries 进化)
fn timeout_heavy_trajectories() -> Vec<TrajectoryOutcome> {
    (0..10)
        .map(|i| {
            if i < 6 {
                TrajectoryOutcome::failed(format!("t{i}"), "timeout", "pvl-layer", 5_000)
            } else {
                TrajectoryOutcome::succeeded(format!("t{i}"), 1_000)
            }
        })
        .collect()
}

/// 全链路通过路径:AEGIS → 审议 → 登记 → 回滚
#[tokio::test]
async fn e2e_variant_governance_full_approval_path() {
    // === Stage A: AEGIS 产出变体(Critic 已含 CiGate 门)===
    let pipeline = AegisPipeline::new();
    let gate = MockCiGate::with_passing_result();
    let base = base_spec();
    let verdict = pipeline
        .run_once(&timeout_heavy_trajectories(), &base, &gate)
        .await
        .expect("流水线不应报错");
    let variant_spec = verdict.accepted.expect("timeout 主导失败应产出变体");
    assert_eq!(variant_spec.meta.version, 2);

    // === Stage B: Parliament 三角色审议 ===
    let pool = VariantPool::new();
    let review = VariantReview::new();
    let contract = VariantContract::new(
        VariantId::new(&variant_spec.meta.name, variant_spec.meta.version),
        vec!["code_fix".into()],
        0.8, // 预期成功率(基线失败率 60% 的改善承诺)
        0.1, // 回归容忍(≤0.2 上限)
    );
    let decision = review.review(&contract, &pool, &[]);
    assert!(decision.is_approved(), "合规变体应通过三角色审议");

    // === Stage C: 审议通过 → 入池 + SpecRegistry 登记谱系 ===
    let mut pool = pool;
    pool.register(contract.clone());
    assert!(pool.route("code_fix").is_some(), "入池后应可路由");

    let mut registry = SpecRegistry::new();
    registry.register(base.clone()).expect("基线登记失败");
    registry
        .register(variant_spec.clone())
        .expect("变体登记失败");
    // 登记后两个版本均在库(注册不自动激活,active 仍为 v1)
    assert_eq!(registry.list_versions(&variant_spec.meta.name), vec![1, 2]);

    // === Stage D: 候选灰度 + 一键回滚(ADR-050 决策 5,复用既有机制)===
    registry
        .set_candidate(&variant_spec.meta.name, 2)
        .expect("设置候选失败");
    registry
        .promote_candidate(&variant_spec.meta.name)
        .expect("晋升候选失败");
    assert_eq!(
        registry
            .get_active(&variant_spec.meta.name)
            .map(|s| s.meta.version),
        Some(2)
    );
    // 晋升后 lineage = 父版本链 [1, 2](ADR-044:lineage 按 active 的父链语义)
    let lineage = registry
        .lineage(&variant_spec.meta.name)
        .expect("lineage 查询失败");
    assert_eq!(lineage, vec![1, 2]);
    // 回滚:active 回到 v1
    let rolled_back_to = registry
        .rollback(&variant_spec.meta.name)
        .expect("回滚失败");
    assert_eq!(rolled_back_to, 1, "回滚应回到基线 v1");
}

/// 审议否决路径:Security 一票否决,变体不入池
#[tokio::test]
async fn e2e_variant_governance_security_veto_path() {
    let review = VariantReview::new();
    let pool = VariantPool::new();
    let contract = VariantContract::new(
        VariantId::new("suspicious-spec", 2),
        vec!["code_fix".into()],
        0.99,
        0.01,
    );
    // 调用方预检发现安全关切(如触碰 UNLEARNABLE_SECURITY_RULES 语义)
    let concerns = vec![seccore::UNLEARNABLE_SECURITY_RULES[0].to_string()];
    let decision = review.review(&contract, &pool, &concerns);

    match decision {
        ReviewDecision::Reject(reason) => {
            assert!(reason.contains("Security 一票否决"));
        }
        ReviewDecision::Approve => panic!("安全关切非空必须否决"),
    }
    // 否决的变体不入池:池保持为空
    assert!(pool.is_empty());
}

/// 奖励欺骗守护:超限变体在 Critic 阶段即被拒绝,不进入审议
#[tokio::test]
async fn e2e_reward_hacking_guard_rejects_extreme_variant() {
    use gsoe_evolution::{AegisCritic, SpecCandidate};

    let critic = AegisCritic::new();
    let gate = MockCiGate::with_passing_result();
    let base = base_spec();

    // 构造极端变体:max_attempts = 100(远超 3× 基线 = 15 与绝对上限 20)
    let mut extreme = base.clone();
    extreme.meta.version = 2;
    extreme.meta.parent = Some(1);
    extreme.retry.max_attempts = 100;
    let verdict = critic
        .select(
            vec![SpecCandidate {
                spec: extreme,
                rationale: "reward hacking attempt".into(),
            }],
            &base,
            &gate,
        )
        .await
        .expect("裁决不应报错");

    assert!(verdict.accepted.is_none(), "极端变体必须被拒绝");
    assert_eq!(verdict.rejected.len(), 1);
    assert!(verdict.rejected[0].reason.contains("奖励欺骗守护"));
}
