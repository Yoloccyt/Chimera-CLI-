//! Parliament proptest — 不变量属性测试(SubTask 37.7)
//!
//! 验证 VoteCounter 加权赞成率、共识判定与 AHIRT 探测率的不变量。
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)

#![forbid(unsafe_code)]

use parliament::{
    AhirtRedTeam, Consensus, Opinion, ParliamentConfig, ProbeResult, ProbeType, Proposal, Role,
    VoteCounter,
};
use proptest::prelude::*;

/// 生成随机角色立场 ∈ {0.0, 0.5, 1.0}(反对/弃权/赞成)
///
/// WHY 枚举值而非连续 f32:Opinion 的语义立场仅此三值,
/// `is_approve/is_reject/is_abstain` 通过精确相等判定,连续值会破坏判定逻辑
fn position_strategy() -> impl Strategy<Value = f32> {
    prop_oneof![Just(0.0f32), Just(0.5f32), Just(1.0f32)]
}

/// 生成 5 角色的随机意见列表(立场 + 置信度)
///
/// WHY 固定 5 角色:对应 AHIRT 5 角色对抗性议会设计,
/// 保证权重总和为 1.0,加权赞成率计算有意义
fn opinions_strategy() -> impl Strategy<Value = Vec<Opinion>> {
    let roles = Role::all();
    proptest::collection::vec((position_strategy(), 0.0f32..=1.0f32), roles.len()).prop_map(
        move |positions| {
            roles
                .iter()
                .zip(positions.iter())
                .map(|(&role, &(position, confidence))| {
                    Opinion::new(role, position, confidence, "proptest 意见")
                })
                .collect()
        },
    )
}

// 不变量:加权赞成率 ∈ [0.0, 1.0]
//
// 生成随机角色立场与置信度,计算加权赞成率,验证结果始终在 [0,1] 区间。
// 此不变量是 AHIRT 加权投票的安全基础:越界值会导致共识判定异常。
#[test]
fn proptest_vote_weighted_rate_in_range() {
    // WHY total_roles >= 5:opinions_strategy 固定生成 5 角色意见,
    // 参与率 = opinions.len() / total_roles,需 total_roles >= 5 保证参与率 <= 1.0
    proptest!(|(opinions in opinions_strategy(), total_roles in 5usize..=10)| {
        let config = ParliamentConfig::default();
        let counter = VoteCounter::new(&config);
        let proposal = Proposal::new("p-prop", "q-prop", "属性测试提案", 0.3);

        let result = counter.count_votes(&opinions, total_roles, &proposal);

        prop_assert!(
            (0.0..=1.0).contains(&result.weighted_approval_rate),
            "加权赞成率 {} 应在 [0,1] 区间",
            result.weighted_approval_rate
        );
        prop_assert!(
            (0.0..=1.0).contains(&result.participation_rate),
            "参与率 {} 应在 [0,1] 区间",
            result.participation_rate
        );
    });
}

// 不变量:共识判定一致性
//
// 验证共识判定逻辑的内部一致性:
// - Skeptic 否决(position=0.0)→ Vetoed(红队防线优先于赞成率)
// - 法定人数不足 → Rejected
// - 赞成率 ≥ 阈值且无否决 → Reached
// - 赞成率 < 阈值 → Rejected
//
// WHY 此不变量:确保 5 角色对抗性议会的决策逻辑可预测,
// 防止否决被赞成率绕过(§6 架构红线:红队防线不可被多数票绕过)
#[test]
fn proptest_consensus_determination_consistency() {
    proptest!(|(opinions in opinions_strategy())| {
        let config = ParliamentConfig::default();
        let counter = VoteCounter::new(&config);
        let proposal = Proposal::new("p-prop", "q-prop", "属性测试提案", 0.3);

        // total_roles = 5(全参与,排除法定人数不足干扰)
        let result = counter.count_votes(&opinions, 5, &proposal);

        // 检查 Skeptic 否决优先(红队防线)
        let skeptic_vetoed = opinions
            .iter()
            .any(|o| o.role == Role::Skeptic && o.is_reject());

        if skeptic_vetoed {
            prop_assert!(
                result.consensus.is_vetoed(),
                "Skeptic 否决应导致 Vetoed,实际: {:?}",
                result.consensus
            );
        } else {
            // 无否决时,根据赞成率判定
            match &result.consensus {
                Consensus::Reached { .. } => {
                    prop_assert!(
                        result.weighted_approval_rate >= config.consensus_threshold,
                        "Reached 共识要求赞成率 ≥ {},实际: {}",
                        config.consensus_threshold,
                        result.weighted_approval_rate
                    );
                }
                Consensus::Rejected { reason: _ } => {
                    prop_assert!(
                        result.weighted_approval_rate < config.consensus_threshold,
                        "Rejected 共识应满足赞成率 < {},实际: {}",
                        config.consensus_threshold,
                        result.weighted_approval_rate
                    );
                }
                Consensus::Vetoed { .. } => {
                    prop_assert!(false, "无 Skeptic 否决不应产生 Vetoed");
                }
            }
        }
    });
}

// 不变量:AHIRT 探测率 ∈ [0.0, 1.0]
//
// 生成随机 total(1-1000)和 passed(0-total),计算探测率,
// 验证结果 ∈ [0,1] 且等于 passed/total。
//
// WHY 此不变量:探测率是 AHIRT 红队评估系统防御能力的核心指标,
// 越界值会导致漏洞误报或漏报(§6 架构红线:探测率 > 95%)
#[test]
fn proptest_detection_rate_in_range() {
    proptest!(|(total in 1u32..=1000, passed_ratio in 0.0f32..=1.0f32)| {
        let passed = ((total as f32) * passed_ratio).round() as u32;
        // 确保 passed ≤ total(浮点取整可能越界)
        let passed = passed.min(total);

        // 构造 ProbeResult 列表(passed 个通过,其余失败)
        let mut results = Vec::with_capacity(total as usize);
        for i in 0..total {
            results.push(ProbeResult {
                probe_type: ProbeType::CommandInjection,
                payload: format!("payload-{i}"),
                passed: i < passed,
                actual_result: if i < passed {
                    "blocked".to_string()
                } else {
                    "allowed".to_string()
                },
                expected_result: "blocked".to_string(),
            });
        }

        let red_team = AhirtRedTeam::default();
        let rate = red_team.compute_detection_rate(&results);

        prop_assert!(
            (0.0..=1.0).contains(&rate),
            "探测率 {} 应在 [0,1] 区间",
            rate
        );

        // 验证探测率 = passed / total
        let expected = passed as f32 / total as f32;
        prop_assert!(
            (rate - expected).abs() < 1e-6,
            "探测率 {} 应等于 passed/total = {}",
            rate,
            expected
        );
    });
}

// 不变量:空探测结果列表的探测率为 0.0
//
// WHY 此不变量:空列表时不应产生 NaN 或除零错误,
// 返回 0.0 表示"无探测数据"的最保守判定
#[test]
fn proptest_detection_rate_empty_list_is_zero() {
    let red_team = AhirtRedTeam::default();
    let rate = red_team.compute_detection_rate(&[]);
    assert!(rate.abs() < 1e-6, "空探测列表的探测率应为 0.0,实际: {rate}");
}
