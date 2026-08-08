//! 评估-进化-协同三位一体闭环 E2E（Milestone D-3）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 D-3）：
//! RuntimeAuditor 发现 → AEGIS 变体 → GRPO 协同 → 再评估 的端到端闭环。
//!
//! 验收对齐：
//! - KPI-B1：证据纪律——仅运行时事件计为已验证
//!   （发现阶段 StaticOnly → 再评估阶段 RuntimeEvents 跃迁）
//! - KPI-B2/B3：变体经 Critic + Parliament 审议；Security 一票否决
//! - 九论文矩阵：评估（Qoder Runtime Auditor）/ 进化（小米 AEGIS）/
//!   协同（小米+快手 Cross-Harness GRPO）三新增项同链路贯通
//!
//! R2 冻结（ADR-042）：GRPO 为规则式 rollout 采样 + 优势计算（C-2 占位
//! 实现），不触训练面；本 E2E 仅消费既有公开 API，无新增依赖。

#![forbid(unsafe_code)]

use efficiency_monitor::auditor::{EvidenceKind, FindingSeverity, RuntimeAuditor};
use gsoe_evolution::policy::grpo::{compute_advantage, sample_rollouts};
use gsoe_evolution::types::EvolutionPolicy;
use nexus_contracts::variant::{VariantContract, VariantId};
use parliament::variant_pool::VariantPool;
use parliament::variant_review::VariantReview;

/// 闭环四步：发现 → 变体审议 → GRPO 协同 → 再评估（证据跃迁）
#[test]
fn trinity_loop_find_variant_evolve_reevaluate() {
    // ===== 1. 评估·发现：RuntimeAuditor 审计未登记能力 → EvidenceGap(High) =====
    let auditor = RuntimeAuditor::new();
    let finding = auditor.audit_capability("refactor");
    assert_eq!(
        finding.severity,
        FindingSeverity::High,
        "未登记 → High 证据缺口"
    );
    assert_eq!(finding.evidence, EvidenceKind::StaticOnly, "无运行时证据");

    // ===== 2. 进化·AEGIS 变体：审议通过 + Security 一票否决（KPI-B3） =====
    let contract = VariantContract::new(
        VariantId::new("trinity-refactor-v1", 1),
        vec!["refactor".into()],
        0.85, // 预期性能
        0.05, // 性能方差
    );
    let mut pool = VariantPool::new();
    pool.register(contract.clone());
    let review = VariantReview::new();
    let approved = review.review(&contract, &pool, &[]);
    assert!(approved.is_approved(), "无安全顾虑 → 变体审议通过");
    let vetoed = review.review(&contract, &pool, &["命令注入".into()]);
    assert!(!vetoed.is_approved(), "Security 一票否决 → 变体不可采纳");

    // ===== 3. 协同·Cross-Harness GRPO：群体相对优势计算 =====
    let policy = EvolutionPolicy::new(0.1, 1.5, 0.2, 4).expect("合法进化策略");
    let mut rollouts = sample_rollouts(&policy, 4);
    compute_advantage(&mut rollouts);
    assert_eq!(rollouts.len(), 4, "4 条 rollout 采样");
    assert!(
        rollouts.iter().all(|r| r.advantage.is_some()),
        "GRPO 优势全部计算（组内相对归一化）"
    );

    // ===== 4. 评估·再评估：采纳变体后登记 + 真实使用 → 证据跃迁 =====
    auditor.register_capability("refactor");
    for _ in 0..3 {
        auditor.record_capability_use("refactor");
    }
    let recheck = auditor.audit_capability("refactor");
    assert_eq!(
        recheck.severity,
        FindingSeverity::Info,
        "High → Info 闭环改善"
    );
    assert_eq!(
        recheck.evidence,
        EvidenceKind::RuntimeEvents(3),
        "静态证据 → 运行时事件证据（KPI-B1 证据纪律）"
    );
}

/// 闭环反向路径：未采纳变体（审议否决）→ 能力保持未验证状态
#[test]
fn trinity_loop_veto_keeps_capability_unverified() {
    let auditor = RuntimeAuditor::new();
    auditor.register_capability("sandbox-exec");
    // 变体被否决（安全顾虑）→ 不 record_capability_use → 仍 UnusedCapability
    let contract = VariantContract::new(
        VariantId::new("trinity-sandbox-v1", 1),
        vec!["sandbox-exec".into()],
        0.9,
        0.05,
    );
    let mut pool = VariantPool::new();
    pool.register(contract.clone());
    let vetoed = VariantReview::new().review(&contract, &pool, &["沙箱逃逸".into()]);
    assert!(!vetoed.is_approved(), "安全否决");
    let finding = auditor.audit_capability("sandbox-exec");
    assert_eq!(
        finding.severity,
        FindingSeverity::Medium,
        "未采纳 → 能力未验证（UnusedCapability）"
    );
}
