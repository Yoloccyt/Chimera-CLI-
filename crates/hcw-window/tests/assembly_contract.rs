//! 统一上下文装配内核契约测试（R9 / W1-T1~T2）
//!
//! 锁定五条硬契约：
//! 1. 空输入不 panic；
//! 2. 精确重复去重（保留最先出现者）；
//! 3. 无重复且预算充足 ⇒ 输出与输入逐字段等价；
//! 4. 预算 = `min(contract.max_tokens, tier_capacity)`；
//! 5. 驱逐保护顺序：可驱逐 → 近期 → 钉选。

use hcw_window::{assemble_context, ContextItem, ContextRole, ContextSource};
use nexus_contracts::spec::harness_dimensions::ContextAssemblyContract;
use proptest::prelude::*;

fn item(
    role: ContextRole,
    source: ContextSource,
    content: &str,
    pinned: bool,
    seq: u64,
) -> ContextItem {
    ContextItem {
        role,
        source,
        content: content.to_string(),
        pinned,
        seq,
    }
}

fn contract(max_tokens: usize) -> ContextAssemblyContract {
    let mut c = ContextAssemblyContract::default_contract();
    c.max_tokens = max_tokens;
    c
}

#[test]
fn empty_input_returns_empty_without_panic() {
    let out = assemble_context(Vec::new(), &contract(128_000), 4_096);
    assert!(out.items.is_empty());
    assert_eq!(out.diagnostics.deduped, 0);
    assert_eq!(out.diagnostics.evicted, 0);
    assert_eq!(out.diagnostics.used_tokens, 0);
    assert_eq!(out.diagnostics.budget_tokens, 4_096);
}

#[test]
fn exact_duplicates_are_removed_keeping_first_occurrence() {
    let items = vec![
        item(ContextRole::User, ContextSource::User, "abcdefgh", false, 0),
        item(
            ContextRole::Assistant,
            ContextSource::Assistant,
            "12345678",
            false,
            1,
        ),
        item(ContextRole::User, ContextSource::User, "abcdefgh", false, 2),
    ];
    let out = assemble_context(items, &contract(1_000), 1_000);
    assert_eq!(out.diagnostics.deduped, 1);
    assert_eq!(out.items.len(), 2);
    assert_eq!(out.items[0].content, "abcdefgh");
    assert_eq!(out.items[0].seq, 0);
    assert_eq!(out.items[1].content, "12345678");
}

#[test]
fn same_content_different_role_is_not_deduplicated() {
    let items = vec![
        item(ContextRole::User, ContextSource::User, "same", false, 0),
        item(
            ContextRole::Assistant,
            ContextSource::Assistant,
            "same",
            false,
            1,
        ),
    ];
    let out = assemble_context(items, &contract(1_000), 1_000);
    assert_eq!(out.diagnostics.deduped, 0);
    assert_eq!(out.items.len(), 2);
}

#[test]
fn same_content_different_source_is_not_deduplicated() {
    let items = vec![
        item(ContextRole::User, ContextSource::User, "same", false, 0),
        item(ContextRole::User, ContextSource::Memory, "same", false, 1),
    ];
    let out = assemble_context(items, &contract(1_000), 1_000);
    assert_eq!(out.diagnostics.deduped, 0);
    assert_eq!(out.items.len(), 2);
}

#[test]
fn no_duplicates_and_ample_budget_is_fieldwise_identical() {
    let items = vec![
        item(ContextRole::System, ContextSource::System, "sys", false, 0),
        item(ContextRole::User, ContextSource::User, "question", false, 1),
        item(
            ContextRole::Assistant,
            ContextSource::Assistant,
            "answer",
            false,
            2,
        ),
    ];
    let out = assemble_context(items.clone(), &contract(128_000), 128_000);
    assert_eq!(out.items, items);
    assert_eq!(out.diagnostics.deduped, 0);
    assert_eq!(out.diagnostics.evicted, 0);
}

#[test]
fn budget_uses_contract_when_contract_is_smaller() {
    // 两条各 2 token；contract=2 < tier=1000 ⇒ 只保留 1 条
    let items = vec![
        item(ContextRole::User, ContextSource::User, "abcdefgh", false, 0),
        item(ContextRole::User, ContextSource::User, "ijklmnop", false, 1),
    ];
    let out = assemble_context(items, &contract(2), 1_000);
    assert_eq!(out.diagnostics.budget_tokens, 2);
    assert_eq!(out.items.len(), 1);
    assert_eq!(out.diagnostics.evicted, 1);
}

#[test]
fn budget_uses_tier_when_tier_is_smaller() {
    let items = vec![
        item(ContextRole::User, ContextSource::User, "abcdefgh", false, 0),
        item(ContextRole::User, ContextSource::User, "ijklmnop", false, 1),
    ];
    let out = assemble_context(items, &contract(128_000), 2);
    assert_eq!(out.diagnostics.budget_tokens, 2);
    assert_eq!(out.items.len(), 1);
}

#[test]
fn tier_zero_means_unconfigured_and_falls_back_to_contract() {
    let items = vec![item(
        ContextRole::User,
        ContextSource::User,
        "abcdefgh",
        false,
        0,
    )];
    let out = assemble_context(items, &contract(128_000), 0);
    assert_eq!(out.diagnostics.budget_tokens, 128_000);
    assert_eq!(out.items.len(), 1);
}

#[test]
fn pinned_item_is_evicted_last() {
    // 5 条各 2 token，预算 2 ⇒ 仅保留 1 条；index 0 为 pinned
    let mut items = vec![item(
        ContextRole::User,
        ContextSource::Memory,
        "pinned00",
        true,
        0,
    )];
    for i in 1..5u64 {
        items.push(item(
            ContextRole::Assistant,
            ContextSource::Assistant,
            &format!("msg{i:0>5}"),
            false,
            i,
        ));
    }
    let out = assemble_context(items, &contract(2), 1_000);
    assert_eq!(out.items.len(), 1);
    assert!(out.items[0].pinned, "钉选条目必须最后被淘汰");
    assert_eq!(out.items[0].seq, 0);
}

#[test]
fn evictable_is_dropped_before_recent_window() {
    // 6 条各 2 token，预算 6（3 条）；末 4 条为近期段，前 2 条先被淘汰
    let mut items = Vec::new();
    for i in 0..6u64 {
        items.push(item(
            ContextRole::Assistant,
            ContextSource::Assistant,
            &format!("msg{i:0>5}"),
            false,
            i,
        ));
    }
    let out = assemble_context(items, &contract(6), 1_000);
    assert_eq!(out.items.len(), 3);
    let kept: Vec<u64> = out.items.iter().map(|i| i.seq).collect();
    assert_eq!(kept, vec![3, 4, 5]);
}

#[test]
fn output_preserves_input_relative_order_and_seq() {
    let items = vec![
        item(ContextRole::System, ContextSource::System, "a", false, 10),
        item(ContextRole::User, ContextSource::User, "b", false, 20),
        item(
            ContextRole::Assistant,
            ContextSource::Assistant,
            "c",
            false,
            30,
        ),
    ];
    let out = assemble_context(items, &contract(128_000), 128_000);
    let seqs: Vec<u64> = out.items.iter().map(|i| i.seq).collect();
    assert_eq!(seqs, vec![10, 20, 30]);
}

#[test]
fn synthesis_requested_reflects_contract_without_calling_anything() {
    let mut c = contract(128_000);
    c.on_demand_synthesis = false;
    let out = assemble_context(
        vec![item(ContextRole::User, ContextSource::User, "x", false, 0)],
        &c,
        1_000,
    );
    assert!(!out.diagnostics.synthesis_requested);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// 无重复输入 + 充足预算 ⇒ 输出与输入逐字段等价
    #[test]
    fn prop_no_duplicates_implies_identity(
        raw in proptest::collection::vec((0u8..4, 0u8..6, "[a-z]{0,12}", any::<bool>()), 0..24)
    ) {
        let items: Vec<ContextItem> = raw
            .into_iter()
            .enumerate()
            .map(|(i, (r, s, content, pinned))| {
                let role = match r {
                    0 => ContextRole::System,
                    1 => ContextRole::User,
                    2 => ContextRole::Assistant,
                    _ => ContextRole::Tool,
                };
                let source = match s {
                    0 => ContextSource::System,
                    1 => ContextSource::User,
                    2 => ContextSource::Assistant,
                    3 => ContextSource::Memory,
                    4 => ContextSource::Tool,
                    _ => ContextSource::Retrieval,
                };
                // 内容带序号，保证生成集中无精确重复
                ContextItem {
                    role,
                    source,
                    content: format!("{i}:{content}"),
                    pinned,
                    seq: i as u64,
                }
            })
            .collect();
        let input = items.clone();
        let out = assemble_context(items, &contract(1_000_000), 1_000_000);
        prop_assert_eq!(out.items, input);
        prop_assert_eq!(out.diagnostics.deduped, 0);
        prop_assert_eq!(out.diagnostics.evicted, 0);
    }
}
