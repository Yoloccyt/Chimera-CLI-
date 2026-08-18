//! 冲突仲裁集成测试 — L0 契约消费 + 四决策闭环（v3.4.0 §13.2）
//!
//! 覆盖: 顶层 API 可达性 / L0 AtomicMemoryCard 消费 / 四决策闭环 /
//! trait 注入 mock judge / 召回子集不变量 proptest

#![forbid(unsafe_code)]

use async_trait::async_trait;
use nexus_contracts::memory_pyramid::{AtomicCardType, AtomicMemoryCard};
use parliament::{
    ArbitrationResult, ConflictArbitrator, ModelDecision, ModelJudge, RuleBasedRetriever,
};
use proptest::prelude::*;

fn card(id: &str, scene: &str, content: &str) -> AtomicMemoryCard {
    AtomicMemoryCard::new(
        id,
        AtomicCardType::Policy,
        100,
        scene,
        content,
        None,
        None,
        None,
        None,
        1_700_000_000_000,
    )
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[tokio::test]
async fn top_level_api_accessible() {
    use parliament::prelude::*;
    let arbitrator = ConflictArbitrator::default();
    let result = arbitrator
        .arbitrate(&card("n", "s", "fresh content"), &[])
        .await;
    assert_eq!(result, ArbitrationResult::AddNew);
}

// ----------------------------------------------------------
// 四决策闭环（L0 AtomicMemoryCard 消费）
// ----------------------------------------------------------

#[tokio::test]
async fn four_decisions_closure() {
    let arbitrator = ConflictArbitrator::default();
    // AddNew: 场景不匹配 → 召回空
    let add = arbitrator
        .arbitrate(
            &card("n", "s1", "alpha beta gamma"),
            &[card("e", "s2", "alpha beta gamma")],
        )
        .await;
    assert_eq!(add, ArbitrationResult::AddNew);
    // Skip: 内容完全一致
    let skip = arbitrator
        .arbitrate(
            &card("n", "s1", "alpha beta gamma delta"),
            &[card("e", "s1", "alpha beta gamma delta")],
        )
        .await;
    assert_eq!(skip, ArbitrationResult::Skip);
    // Update: 高度重叠（4/5 词交集 = 0.8 ≥ 0.7）
    let update = arbitrator
        .arbitrate(
            &card("n", "s1", "alpha beta gamma delta one"),
            &[card("e", "s1", "alpha beta gamma delta two")],
        )
        .await;
    assert_eq!(update, ArbitrationResult::Update(Box::from("e")));
    // Merge: 双候选实质重叠（各 0.5 ≥ 0.3）
    let merge = arbitrator
        .arbitrate(
            &card("n", "s1", "alpha beta eta theta"),
            &[
                card("c1", "s1", "alpha beta gamma delta"),
                card("c2", "s1", "alpha beta epsilon zeta"),
            ],
        )
        .await;
    assert_eq!(
        merge,
        ArbitrationResult::Merge(vec![Box::from("c1"), Box::from("c2")])
    );
}

// ----------------------------------------------------------
// trait 注入 mock judge（D-4 注入点）
// ----------------------------------------------------------

#[tokio::test]
async fn mock_judge_injection() {
    struct FixedJudge(ModelDecision);
    #[async_trait]
    impl ModelJudge for FixedJudge {
        async fn judge(
            &self,
            _new_card: &AtomicMemoryCard,
            _candidates: &[AtomicMemoryCard],
        ) -> ModelDecision {
            self.0.clone()
        }
    }
    // 注入 Update 决策 judge
    let arbitrator = ConflictArbitrator::new(
        Box::new(RuleBasedRetriever::default()),
        Box::new(FixedJudge(ModelDecision::Update(Box::from("target-id")))),
    );
    let result = arbitrator
        .arbitrate(
            &card("n", "s1", "shared terms here"),
            &[card("e", "s1", "shared terms here")],
        )
        .await;
    assert_eq!(result, ArbitrationResult::Update(Box::from("target-id")));
}

// ----------------------------------------------------------
// 召回阈值门控
// ----------------------------------------------------------

#[tokio::test]
async fn recall_threshold_gating() {
    // min_shared_terms=3：仅 2 词交集（alpha/beta）→ 召回空 → AddNew
    let arbitrator = ConflictArbitrator::new(
        Box::new(RuleBasedRetriever {
            min_shared_terms: 3,
        }),
        Box::new(parliament::RuleBasedJudge),
    );
    let result = arbitrator
        .arbitrate(
            &card("n", "s1", "alpha beta unique specialized"),
            &[card("e", "s1", "alpha beta other different")],
        )
        .await;
    assert_eq!(result, ArbitrationResult::AddNew, "未达召回阈值应 AddNew");
}

// ----------------------------------------------------------
// proptest：召回子集不变量
// ----------------------------------------------------------

proptest! {
    /// 仲裁结果恒为四枚举之一；空既有集恒 AddNew
    #[test]
    fn arbitration_result_closed(
        n_existing in 0usize..5,
        content_seed in 0u32..100,
    ) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        // async block 返回 Result 以使 prop_assert! 的 early-return 生效
        let outcome: Result<(), proptest::test_runner::TestCaseError> = rt.block_on(async {
            let arbitrator = ConflictArbitrator::default();
            let existing: Vec<AtomicMemoryCard> = (0..n_existing)
                .map(|i| card(&format!("e{i}"), "s1", &format!("term{content_seed} common shared data")))
                .collect();
            let new_card = card("n", "s1", &format!("term{content_seed} common shared extra"));
            let result = arbitrator.arbitrate(&new_card, &existing).await;
            prop_assert!(matches!(
                result,
                ArbitrationResult::AddNew
                    | ArbitrationResult::Skip
                    | ArbitrationResult::Update(_)
                    | ArbitrationResult::Merge(_)
            ));
            if n_existing == 0 {
                prop_assert_eq!(result, ArbitrationResult::AddNew, "空既有集恒 AddNew");
            }
            Ok(())
        });
        outcome?;
    }
}
