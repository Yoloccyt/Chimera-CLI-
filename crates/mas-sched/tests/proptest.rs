//! mas-sched 影子决策面属性测试 —— W16 门禁的"任意输入"验证
//!
//! 对应架构层: L9 Quest(mas-sched 控制面,ADR-145)
//!
//! ## 覆盖的不变量
//! 1. **日志 append-only**:`ShadowLog` 只能追加,历史不可变、顺序不可变(审计链根基)。
//! 2. **回放记账闭合**:`total == 日志长度` 且 `matched + mismatched == total`——
//!    不可回放的决策必须被**计入** mismatched,不得静默丢弃(否则"100% 可回放"可被虚报)。
//! 3. **回放确定性(Ω₂)**:同一日志重放两次报告逐位相同,且重放不改写日志。
//! 4. **良构序列 100% 可回放**:W16 门禁在其**真实前置条件**下成立。
//! 5. **影子只决策不执行**:对外恒 `Denied`、`lease_count` 恒 0、日志记录**内部真实决策**。
//! 6. **配额硬约束**:同 peer 并发租约超出 `Quota::max_concurrent` 的部分必须被拒。
//! 7. **审计持久化**:任意 `ShadowDecision` serde 往返无损。
//!
//! ## 语法约束(§4.1 规范)
//! proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`
//! 禁止 closure 形式(某些 pattern 解析失败)
//!
//! ## 为什么属性 4 要带前置条件
//! [`ShadowLog::replay`] 的文档自称"每条决策输出与原始日志逐位一致(门禁:100% 可回放)",
//! 但重放侧是**独立状态机**:claim 一律回 ShadowReject、handoff 一律 Ok、
//! should_run 只看"该任务在本次重放中是否出现过 claim"。因此以下决策在重放侧必然不一致:
//! 内部以非 ShadowReject 原因拒绝的 claim、重放侧查无此任务的 renew、
//! 内部失败的 handoff、以及租约被 HANDOFF 释放后的 should_run。
//! 故"100%"只对良构序列成立;无条件成立的只有属性 2(见两个 witness 测试)。

#![forbid(unsafe_code)]

use mas_sched::shadow::{ShadowClaimOutcome, ShadowLog};
use mas_sched::{
    ClaimOutcome, DenyReason, PeerScheduler, Priority, RenewOutcome, ShadowDecision,
    ShadowScheduler, ShouldRunVerdict, SimplePeerScheduler, TaskId, TodoClaim,
};
use proptest::prelude::*;

// ============================================================
// 辅助策略(Strategy)
// ============================================================

/// 生成任意 ID 字符串 —— 含空串,覆盖 task/peer 的边界取值
fn arb_id() -> impl Strategy<Value = String> {
    "[a-z0-9_\\-]{0,12}"
}

/// 生成任意 TaskId
fn arb_task_id() -> impl Strategy<Value = TaskId> {
    arb_id().prop_map(TaskId::from)
}

/// 生成任意预估时长 —— 卡在配额边界两侧(300_000 为 Quota::default 上限)
fn arb_est_ms() -> impl Strategy<Value = u64> {
    prop_oneof![
        Just(0u64),
        Just(1),
        Just(10_000),
        Just(299_999),
        Just(300_000),
        Just(300_001),
        Just(u64::MAX),
    ]
}

/// 生成任意拒绝原因 —— 覆盖全部 4 个变体(含 ShadowReject 本身)
fn arb_deny_reason() -> impl Strategy<Value = DenyReason> {
    prop_oneof![
        Just(DenyReason::QuotaExceeded),
        Just(DenyReason::TaskClaimed),
        Just(DenyReason::DurationExceedsQuota),
        Just(DenyReason::ShadowReject),
    ]
}

/// 生成任意 claim 决策输出 —— Granted 的 duration 覆盖 0 与 u64 上界
fn arb_claim_outcome() -> impl Strategy<Value = ShadowClaimOutcome> {
    prop_oneof![
        prop_oneof![Just(0u64), Just(1), Just(10_000), Just(u64::MAX)]
            .prop_map(|duration_ms| ShadowClaimOutcome::Granted { duration_ms }),
        arb_deny_reason().prop_map(ShadowClaimOutcome::Denied),
    ]
}

/// 生成任意续期结果 —— 覆盖全部 3 个变体
fn arb_renew_outcome() -> impl Strategy<Value = RenewOutcome> {
    prop_oneof![
        Just(RenewOutcome::Renewed),
        Just(RenewOutcome::NotRenewable),
        Just(RenewOutcome::Expired),
    ]
}

/// 生成任意 should_run 裁决 —— 覆盖全部 4 个变体
fn arb_verdict() -> impl Strategy<Value = ShouldRunVerdict> {
    prop_oneof![
        Just(ShouldRunVerdict::Run),
        Just(ShouldRunVerdict::Defer),
        Just(ShouldRunVerdict::AlreadyConverged),
        Just(ShouldRunVerdict::NoActionableWork),
    ]
}

/// 生成任意影子决策条目 —— 4 个变体 × 随机字段(属性 1/7 的输入域)
fn arb_decision() -> impl Strategy<Value = ShadowDecision> {
    prop_oneof![
        (arb_task_id(), arb_id(), arb_claim_outcome()).prop_map(|(task_id, peer_id, outcome)| {
            ShadowDecision::Claim {
                task_id,
                peer_id,
                outcome,
            }
        }),
        (arb_task_id(), arb_renew_outcome())
            .prop_map(|(task_id, outcome)| ShadowDecision::Renew { task_id, outcome }),
        (arb_task_id(), arb_id(), any::<bool>()).prop_map(|(task_id, to_peer, ok)| {
            ShadowDecision::Handoff {
                task_id,
                to_peer,
                ok,
            }
        }),
        (arb_task_id(), arb_verdict())
            .prop_map(|(task_id, verdict)| ShadowDecision::ShouldRun { task_id, verdict }),
    ]
}

/// 一次控制面调用 —— 属性 2/3 的输入域:允许互相矛盾(未 claim 即 renew、重复 claim…)
#[derive(Debug, Clone)]
enum Op {
    Claim {
        task: String,
        peer: String,
        est_ms: u64,
    },
    Renew {
        task: String,
        peer: String,
    },
    Handoff {
        task: String,
        to: String,
    },
    ShouldRun {
        task: String,
    },
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (arb_id(), arb_id(), arb_est_ms()).prop_map(|(task, peer, est_ms)| Op::Claim {
            task,
            peer,
            est_ms
        }),
        (arb_id(), arb_id()).prop_map(|(task, peer)| Op::Renew { task, peer }),
        (arb_id(), arb_id()).prop_map(|(task, to)| Op::Handoff { task, to }),
        arb_id().prop_map(|task| Op::ShouldRun { task }),
    ]
}

/// 以调度器执行一段操作序列(返回值丢弃 —— 属性只关心日志与报告的闭合性)
fn apply_ops(sched: &ShadowScheduler<SimplePeerScheduler>, ops: &[Op]) {
    for op in ops {
        match op {
            Op::Claim { task, peer, est_ms } => {
                let claim = TodoClaim::new(task.clone(), peer.clone(), Priority::Medium, *est_ms);
                let _ = sched.claim(&claim);
            }
            Op::Renew { task, peer } => {
                let _ = sched.renew_lease(task, peer);
            }
            Op::Handoff { task, to } => {
                let _ = sched.handoff(task, to);
            }
            Op::ShouldRun { task } => {
                let _ = sched.should_run(task);
            }
        }
    }
}

/// 取出日志中全部 claim 决策记录的**内部真实**输出
fn logged_claim_outcomes(log: &ShadowLog) -> Vec<ShadowClaimOutcome> {
    log.snapshot()
        .iter()
        .filter_map(|d| match d {
            ShadowDecision::Claim { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .collect()
}

proptest! {
    // ============================================================
    // 属性 1:日志 append-only 且顺序保真
    // ============================================================

    /// 任意决策序列下,日志只追加、历史不可变、顺序与追加序逐位一致
    #[test]
    fn log_is_append_only_and_order_preserving(
        decisions in proptest::collection::vec(arb_decision(), 0..24),
    ) {
        let log = ShadowLog::new();
        prop_assert_eq!(log.len(), 0);
        prop_assert!(log.is_empty());

        for (i, d) in decisions.iter().enumerate() {
            log.push(d.clone());
            prop_assert_eq!(log.len(), i + 1, "追加第 {} 条后长度应递增 1", i);
            prop_assert_eq!(
                log.snapshot(),
                decisions[..=i].to_vec(),
                "第 {} 次快照必须恰为前缀(历史不可变)",
                i
            );
        }

        prop_assert_eq!(log.snapshot(), decisions.clone());
        prop_assert_eq!(log.is_empty(), decisions.is_empty());
    }

    // ============================================================
    // 属性 2 + 3:回放记账闭合 + 确定性
    // ============================================================

    /// 任意(可矛盾的)操作序列下,回放报告必须闭合且可重复
    ///
    /// WHY 这条无条件成立而"100% 可回放"不是:记账只依赖 `matched + mismatched == total`,
    /// 与决策是否良构无关;它保证不可回放的条目被**计数**而非静默消失。
    #[test]
    fn replay_accounting_is_closed_and_deterministic(
        ops in proptest::collection::vec(arb_op(), 0..24),
    ) {
        let sched = ShadowScheduler::new(SimplePeerScheduler::new());
        apply_ops(&sched, &ops);
        let original = SimplePeerScheduler::new();

        let report = sched.log().replay(&original);
        prop_assert_eq!(report.total, sched.log().len(), "total 必须等于日志长度");
        prop_assert_eq!(report.matched + report.mismatched, report.total);

        if report.total == 0 {
            prop_assert!((report.replay_rate() - 1.0).abs() < 1e-9, "空日志按满分计");
        } else {
            let expected = report.matched as f64 / report.total as f64;
            prop_assert!((report.replay_rate() - expected).abs() < 1e-9);
        }
        prop_assert!((0.0..=1.0).contains(&report.replay_rate()));

        // Ω₂ 确定性:同一日志重放两次,报告逐位相同
        let again = sched.log().replay(&original);
        prop_assert_eq!(report, again);
        // 重放不得改写日志(独立状态机,不污染审计链)
        prop_assert_eq!(sched.log().len(), report.total);
    }

    // ============================================================
    // 属性 4:良构序列 100% 可回放(W16 门禁)
    // ============================================================

    /// 良构决策序列必须 100% 可回放
    ///
    /// 良构 = 与重放侧状态机可对上的最小充分条件:
    /// - 每轮 claim 用**全新任务 + 全新 peer** → 内部必授予(不触发 TaskClaimed/QuotaExceeded);
    /// - renew 由**原持有者**发起;handoff 移交到**具名 peer**(不用 HANDOFF 哨兵,否则租约释放);
    /// - est 远小于配额上限 → 不触发 DurationExceedsQuota;租约 5min 内不判定,不触发 Defer/Expired。
    #[test]
    fn well_formed_decision_sequences_replay_completely(n in 0usize..12) {
        let sched = ShadowScheduler::new(SimplePeerScheduler::new());
        let mut ops = Vec::new();
        for i in 0..n {
            let task = format!("t{i}");
            let peer = format!("p{i}");
            ops.push(Op::Claim { task: task.clone(), peer: peer.clone(), est_ms: 10_000 });
            ops.push(Op::Renew { task: task.clone(), peer });
            ops.push(Op::Handoff { task: task.clone(), to: "peer-next".to_string() });
            ops.push(Op::ShouldRun { task });
        }
        // 未 claim 过的任务:两侧都判 NoActionableWork,同样可回放
        ops.push(Op::ShouldRun { task: "never-claimed".to_string() });

        apply_ops(&sched, &ops);
        let report = sched.log().replay(&SimplePeerScheduler::new());

        prop_assert_eq!(report.total, ops.len());
        prop_assert_eq!(report.mismatched, 0, "良构序列必须零差异");
        prop_assert!((report.replay_rate() - 1.0).abs() < 1e-9);
    }

    // ============================================================
    // 属性 5 + 6:影子语义与配额硬约束
    // ============================================================

    /// 影子期对外永不授予,但日志必须留住输入身份与**内部真实**决策
    #[test]
    fn shadow_claim_never_grants_and_records_inputs(
        task in arb_id(),
        peer in arb_id(),
        est_ms in arb_est_ms(),
    ) {
        let sched = ShadowScheduler::new(SimplePeerScheduler::new());
        let claim = TodoClaim::new(task.clone(), peer.clone(), Priority::Medium, est_ms);
        let out = sched.claim(&claim);

        prop_assert!(
            matches!(out, ClaimOutcome::Denied(_)),
            "影子期对外必须恒拒绝,got {out:?}"
        );
        prop_assert_eq!(sched.lease_count(), 0, "影子期不产生租约");

        let snap = sched.log().snapshot();
        prop_assert_eq!(snap.len(), 1, "每次决策都要留痕");
        prop_assert!(
            matches!(snap[0], ShadowDecision::Claim { .. }),
            "应记录 Claim 决策,got {:?}",
            snap[0]
        );
        if let ShadowDecision::Claim { task_id, peer_id, outcome } = &snap[0] {
            prop_assert_eq!(task_id.as_str(), task.as_str());
            prop_assert_eq!(peer_id, &peer);
            // 内部真实决策入日志(不被影子的 ShadowReject 掩盖)
            if est_ms > 300_000 {
                prop_assert_eq!(
                    outcome,
                    &ShadowClaimOutcome::Denied(DenyReason::DurationExceedsQuota)
                );
            } else {
                prop_assert!(
                    matches!(outcome, ShadowClaimOutcome::Granted { .. }),
                    "配额内的新任务内部应授予,got {outcome:?}"
                );
            }
        }
    }

    /// 同 peer 并发租约受配额硬约束:超出部分内部即被拒,且拒绝原因入日志
    #[test]
    fn peer_concurrency_quota_bounds_grants(n in 1usize..12) {
        let sched = ShadowScheduler::new(SimplePeerScheduler::new());
        for i in 0..n {
            let claim = TodoClaim::new(format!("t{i}"), "solo-peer", Priority::Medium, 10_000);
            let _ = sched.claim(&claim);
        }
        let outcomes = logged_claim_outcomes(sched.log());
        prop_assert_eq!(outcomes.len(), n);
        // 默认配额 max_concurrent = 4(Quota::default)
        let granted = outcomes
            .iter()
            .filter(|o| matches!(o, ShadowClaimOutcome::Granted { .. }))
            .count();
        prop_assert_eq!(granted, n.min(4), "授予数必须被并发配额封顶");
        prop_assert_eq!(sched.lease_count(), 0, "影子期不产生租约");
    }

    // ============================================================
    // 属性 7:审计持久化
    // ============================================================

    /// 任意决策条目 JSON 往返无损(周度报告/审计落盘的前提)
    #[test]
    fn shadow_decision_serde_roundtrip_lossless(d in arb_decision()) {
        let json = serde_json::to_string(&d)
            .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("编码失败: {e}")))?;
        let back: ShadowDecision = serde_json::from_str(&json)
            .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("解码失败: {e}")))?;
        prop_assert_eq!(back, d);
    }
}

// ============================================================
// 前置条件的反例见证(固定输入:证明属性 4 的"良构"限定不可去掉)
// ============================================================

/// 非良构决策必须进入 `mismatched` 而不是被静默丢弃
///
/// WHY 保留这条:把"100% 可回放"写成无条件断言会在真实负载(配额打满、重复 claim)下
/// 突然变红;把它写成"必然不一致"又把缺陷当规格。这里只钉住可辩护的事实——
/// 不可回放的条目会被**计数**,即门禁失败是响亮的。
#[test]
fn ill_formed_inputs_are_counted_as_mismatch_not_dropped() {
    // 1) 从未 claim 过的任务直接 renew:内部 Err → 日志降级记为 NotRenewable
    let sched = ShadowScheduler::new(SimplePeerScheduler::new());
    let _ = sched.renew_lease("ghost", "p1");
    let report = sched.log().replay(&SimplePeerScheduler::new());
    assert_eq!(report.total, 1, "失败决策也必须留痕");
    assert_eq!(report.mismatched, 1, "该条目在重放侧不可复现");
    assert!(report.replay_rate() < 1.0, "不可回放率必须低于门禁");

    // 2) 同 peer 第 5 次 claim:内部 QuotaExceeded,重放侧只回 ShadowReject
    let burst = ShadowScheduler::new(SimplePeerScheduler::new());
    for i in 0..5 {
        let claim = TodoClaim::new(format!("t{i}"), "burst", Priority::Medium, 10_000);
        let _ = burst.claim(&claim);
    }
    let r2 = burst.log().replay(&SimplePeerScheduler::new());
    assert_eq!(r2.total, 5);
    assert_eq!(r2.mismatched, 1, "仅被配额拒绝的那条不可复现");
    assert_eq!(r2.matched, 4);
}

/// HANDOFF 释放租约后的 should_run:两侧裁决口径分叉(见证,非规格)
#[test]
fn handoff_release_diverges_from_replay_verdict() {
    let sched = ShadowScheduler::new(SimplePeerScheduler::new());
    let claim = TodoClaim::new("t1", "p1", Priority::Medium, 10_000);
    let _ = sched.claim(&claim);
    let _ = sched.handoff("t1", mas_sched::HANDOFF); // 回编排层:租约释放
    let _ = sched.should_run("t1");

    let report = sched.log().replay(&SimplePeerScheduler::new());
    assert_eq!(report.total, 3);
    assert_eq!(
        report.mismatched, 1,
        "重放侧不建模租约释放,故释放后的 should_run 不可复现"
    );
}
