//! ShadowModeOrchestrator E2E — 编排器组合裁决三路径验证
//!
//! 对应 ADR: ADR-053-rev4(权威版)+ ADR-053 收敛备忘录 §五 B-4/B-5
//!
//! 覆盖三路径(仿 shadow_breaker_e2e 范式):
//! 1. **正常路径**:治理签署配置 + 14 批全门通过 + 前置全就绪 → Promote 建议
//!    (仅建议,解冻属 ADR-054 + 用户治理)
//! 2. **边界路径**:下界落扩展带 [0.45, 0.5] → ExtendTo25 预注册扩展;
//!    前置未就绪 → NotPromote(不伪装闭合);AHIRT 证据缺失 → 批非胜
//! 3. **异常路径**:RedTeamAudit Critical 事件经真实 EventBus 双通道被
//!    采集器捕获 → 批非胜;熔断器跳闸 → 摄入与终判全短路

use chimera_mas::shadow::{
    AhirtBatchEvidence, AhirtCategoryStats, AhirtEvidenceCollector, BatchEvidence, BatchVerdict,
    DimensionScores, GovernanceSignoff, PromotionAdvice, ShadowModeConfig, ShadowModeOrchestrator,
    Stage3Prerequisites, BASE_BATCHES,
};
use chimera_mas::MasError;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use gsoe_evolution::{RlUpdateTarget, UnfreezeScope};
use nexus_contracts::formal_props::VerificationResult;
use pvl_layer::Verification;

// ============================================================
// 测试夹具
// ============================================================

fn signed_config() -> ShadowModeConfig {
    let signoff = GovernanceSignoff::new(
        "user",
        "ADR-053-rev4 + audit/ADR-053-governance-signoff-2026-07-29.md",
        "2026-07-29",
    )
    .expect("合法签署凭证");
    ShadowModeConfig::anchor_profile(signoff).expect("锚点档配置合法")
}

fn orchestrator() -> ShadowModeOrchestrator {
    let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
    ShadowModeOrchestrator::new(
        signed_config(),
        scope,
        RlUpdateTarget::GsoeVariantSelection,
        42,
    )
}

fn all_ready() -> Stage3Prerequisites {
    Stage3Prerequisites {
        alpha_composite_calibrated: true,
        power_intra_batch_verified: true,
        binomial_sf_comments_corrected: true,
        payload_rotation_ready: true,
        coverage_instrumentation_ready: true,
        s_min_final_confirmed: true,
    }
}

fn full_ahirt() -> AhirtBatchEvidence {
    AhirtBatchEvidence {
        categories: [
            "prompt_injection",
            "command_injection",
            "privilege_escalation",
            "sandbox_escape",
        ]
        .iter()
        .map(|&c| AhirtCategoryStats {
            category: c.into(),
            total: 25,
            failed: 0,
        })
        .collect(),
        red_team_audit_seen: false,
    }
}

fn winning_evidence() -> BatchEvidence {
    BatchEvidence {
        candidate: DimensionScores {
            execution: 0.95,
            mutation: 0.7,
            held_out: 0.7,
        },
        baseline: DimensionScores {
            execution: 0.9,
            mutation: 0.6,
            held_out: 0.6,
        },
        candidate_verification: Verification {
            passed: true,
            pass_rate: 0.95,
            real_execution: true,
            errors: Vec::new(),
        },
        ahirt: Some(full_ahirt()),
    }
}

// ============================================================
// 路径 1:正常 — 满门通过产出 Promote 建议
// ============================================================

/// 14 批全胜 + 前置全就绪 → Promote(仅建议,不解冻)
#[test]
fn e2e_normal_full_promotion_advice() {
    let mut orch = orchestrator();
    orch.set_prerequisites(all_ready());

    for i in 0..BASE_BATCHES {
        let verdict = orch
            .ingest_batch(
                format!("batch-{i}"),
                format!("lineage-{i}"),
                &winning_evidence(),
            )
            .expect("全门通过的批次应正常入账");
        assert!(matches!(verdict, BatchVerdict::Win), "第 {i} 批应计胜");
    }

    match orch.checkpoint_advice().expect("14 批检查点终判应可执行") {
        PromotionAdvice::Promote {
            wins,
            batches,
            lower_bound,
        } => {
            assert_eq!((wins, batches), (BASE_BATCHES, BASE_BATCHES));
            // 14/14 全胜:Wilson 下界远超 0.5,且恒 ≤ Wilson(单调 fail-closed)
            assert!(lower_bound.value > 0.5);
            assert!(lower_bound.value <= lower_bound.wilson);
        }
        other => panic!("满门通过应产出 Promote 建议,实得 {other:?}"),
    }
}

// ============================================================
// 路径 2:边界 — 扩展带 / 前置门 / 证据缺失
// ============================================================

/// 无治理签署 → 配置不可构造(fail-closed 第一道防线)
#[test]
fn e2e_boundary_unsigned_config_cannot_exist() {
    let result = GovernanceSignoff::new("", "", "");
    assert!(
        matches!(result, Err(MasError::ShadowGovernanceConfigInvalid { .. })),
        "空签署必须拒绝构造"
    );
}

/// 下界达标但 6 项前置未就绪 → NotPromote(rev4 诚实二分:不伪装闭合)
#[test]
fn e2e_boundary_prerequisites_block_promotion() {
    let mut orch = orchestrator(); // 前置默认全未就绪
    for i in 0..BASE_BATCHES {
        orch.ingest_batch(format!("batch-{i}"), "lineage", &winning_evidence())
            .expect("批次应入账");
    }
    match orch.checkpoint_advice().expect("终判应可执行") {
        PromotionAdvice::NotPromote { reason, .. } => {
            for item in ["R3-E06-2", "R3-E02-3", "s_min"] {
                assert!(reason.contains(item), "拒绝原因应列出缺项 {item}:{reason}");
            }
        }
        other => panic!("前置未就绪不应产出其他建议:{other:?}"),
    }
}

/// AHIRT 证据缺失批计非胜;检查点外终判被拒(防 optional stopping)
#[test]
fn e2e_boundary_missing_ahirt_and_no_peeking() {
    let mut orch = orchestrator();
    let mut evidence = winning_evidence();
    evidence.ahirt = None;
    let verdict = orch
        .ingest_batch("batch-0", "lineage", &evidence)
        .expect("证据缺失是业务非胜,非摄入错误");
    match verdict {
        BatchVerdict::NonWin { reasons } => {
            assert!(reasons.iter().any(|r| r.contains("AHIRT 证据缺失")));
        }
        BatchVerdict::Win => panic!("AHIRT 缺失不应计胜"),
    }
    // 1 批不是预注册检查点,终判必须被拒
    assert!(matches!(
        orch.checkpoint_advice(),
        Err(MasError::ShadowGateRejected { .. })
    ));
}

// ============================================================
// 路径 3:异常 — RedTeamAudit 事件接线 / 熔断短路
// ============================================================

/// 真实 EventBus 双通道接线:AhirtProbeCompleted 聚合 + RedTeamAudit 必达标记
///
/// WHY multi_thread:采集器后台任务需要与测试主体并发运行,
/// current_thread 下 publish 后无 await 点会饿死采集循环。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_abnormal_red_team_audit_marks_batch_non_win() {
    let bus = EventBus::new();
    // 采集器 spawn 前同步订阅(§4.4 反模式 3 由 start 内部保证)
    let collector = AhirtEvidenceCollector::start(&bus);

    // 发布 4 类探测完成事件(每类 25 探测,0 失败)
    for probe_type in [
        "prompt_injection",
        "command_injection",
        "privilege_escalation",
        "sandbox_escape",
    ] {
        bus.publish(NexusEvent::AhirtProbeCompleted {
            metadata: EventMetadata::new("e2e-test"),
            probe_type: probe_type.into(),
            total: 25,
            passed: 25,
            failed: 0,
            detection_rate: 1.0,
        })
        .await
        .expect("发布探测完成事件应成功");
    }
    // 发布 RedTeamAudit Critical(走 mpsc 旁路,必达)
    bus.publish(NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("e2e-test"),
        vulnerability_type: "prompt_injection".into(),
        failed_probes: 3,
        total_probes: 25,
        detection_rate: 0.88,
        remediation_suggestion: "加固 prompt 过滤规则".into(),
    })
    .await
    .expect("发布 RedTeamAudit 应成功");

    // 等待采集循环消费(事件经 tokio 通道异步投递)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let ahirt = collector.take_window().expect("窗口应聚合到 AHIRT 证据");
    assert_eq!(ahirt.categories.len(), 4, "应聚合 4 类探测统计");
    assert!(ahirt.red_team_audit_seen, "RedTeamAudit 必达标记应置位");

    // 携带告警标记的证据 → 批非胜
    let mut evidence = winning_evidence();
    evidence.ahirt = Some(ahirt);
    let mut orch = orchestrator();
    let verdict = orch
        .ingest_batch("batch-0", "lineage", &evidence)
        .expect("摄入应成功");
    assert!(
        !matches!(verdict, BatchVerdict::Win),
        "RedTeamAudit 观测批不应计胜"
    );

    // 窗口取走后再取为空(每批窗口独立)
    assert!(collector.take_window().is_none(), "窗口应已清空");
}

/// 熔断器跳闸(形式化属性 Violated)→ 批次摄入与终判全部短路拒绝
#[test]
fn e2e_abnormal_tripped_breaker_short_circuits_everything() {
    let mut orch = orchestrator();
    orch.ingest_batch("batch-0", "lineage", &winning_evidence())
        .expect("跳闸前批次应可入账");

    let verdict = orch.observe_verifications(&[VerificationResult::Violated {
        counterexample: "属性 #6 decay 单调性反例".into(),
        samples_tested: 128,
    }]);
    assert!(!verdict.is_permitted(), "Violated 观测应拒绝 RL 更新");
    assert!(orch.breaker().is_tripped(), "熔断器应永久跳闸");

    // 跳闸后一切短路(fail-closed 不可逆,直至授权复位)
    assert!(matches!(
        orch.ingest_batch("batch-1", "lineage", &winning_evidence()),
        Err(MasError::ShadowGateRejected { .. })
    ));
    assert!(matches!(
        orch.checkpoint_advice(),
        Err(MasError::ShadowGateRejected { .. })
    ));

    // S-2.1 授权复位后恢复摄入(问责凭证强制)
    let auth = decay_engine::ResetAuthorization::new("E01+E02", "根因已排查:测试注入反例")
        .expect("合法复位凭证");
    orch.reset_breaker(auth);
    assert!(!orch.breaker().is_tripped());
    orch.ingest_batch("batch-1", "lineage", &winning_evidence())
        .expect("复位后批次应可入账");
}
