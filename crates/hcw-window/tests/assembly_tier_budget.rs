//! 分层配额与页预算的**运行行为**测试（本 Spec A3，ADR-301 D4）
//!
//! # 被测语义（`assemble_context_with_tier`）
//! 1. **页预算**（`TierBudget::page_budget`）：单条 token 估算超过上限即**拒绝**
//!    （不装载），`0` = 关闭该约束；
//! 2. **R/S/E/M 分层配额**：来源 → 层映射（System/User/Assistant→R、Tool→S、
//!    Memory→E、Retrieval→M），某层累计超配额时按**价值密度升序**（同密度按
//!    `seq` 降序）在该层内驱逐；
//! 3. **关态等价**：不提供 `TierBudget` 的旧入口 `assemble_context_with` 逐字段
//!    不变；提供「各项无限」的预算时必须与旧入口输出**逐字段相等**。
//!
//! # 为什么页预算选「拒绝」而不是「切分」
//! `ContextItem` 的 `seq` 是调用方回取原始消息的唯一句柄（`mca` 生产路径按
//! `seq` 回取原消息克隆）。切分会产出「同一 `seq` 的多条碎片」，破坏「输出是
//! 输入子序列」这一结构性保证，并让去重键与 `seq` 一一对应的前提失效。
//! 拒绝则是可观测的（`diagnostics.page_rejected`）、可回退的（返回空 ⇒ 调用方
//! fail-open）且不改变任何保留条目的字节。代价是超大单条内容不进入上下文——
//! 这是**显式限额**的语义，由 governor 的 `page_budget` 决策，内核只执行。

use hcw_window::assemble::{
    assemble_context_with, assemble_context_with_tier, estimate_tokens, ContextItem, ContextRole,
    ContextSource,
};
use hcw_window::governor::{AdaptiveGovernor, PlanContext, TierBudget, WindowGovernor};
use nexus_contracts::spec::harness_dimensions::ContextAssemblyContract;
use proptest::prelude::*;

/// 构造契约（只改预算，其余取缺省）
fn contract(max_tokens: usize) -> ContextAssemblyContract {
    let mut contract = ContextAssemblyContract::default_contract();
    contract.max_tokens = max_tokens;
    contract
}

/// 构造单条上下文
fn item(
    role: ContextRole,
    source: ContextSource,
    content: &str,
    seq: u64,
    pinned: bool,
) -> ContextItem {
    ContextItem {
        role,
        source,
        content: content.to_string(),
        pinned,
        seq,
    }
}

/// 宽松预算：四项配额取 `u32::MAX`（`total()` 饱和到 `u32::MAX`）、页预算关闭
fn generous_budget() -> TierBudget {
    TierBudget {
        resident: u32::MAX,
        structured: u32::MAX,
        episodic: u32::MAX,
        semantic: u32::MAX,
        page_budget: 0,
    }
}

/// 来源 → 层序号（R=0/S=1/E=2/M=3，供属性断言复算）
fn tier_index(source: ContextSource) -> usize {
    match source {
        ContextSource::System | ContextSource::User | ContextSource::Assistant => 0,
        ContextSource::Tool => 1,
        ContextSource::Memory => 2,
        ContextSource::Retrieval => 3,
    }
}

// ============================================================
// 1) 页预算：单条装载上限（拒绝语义）
// ============================================================

#[test]
fn page_budget_rejects_oversize_item() {
    // 默认估算器 = 字符/4 向上取整：100 字符 = 25 token、104 字符 = 26 token
    let items = vec![
        item(
            ContextRole::Tool,
            ContextSource::Tool,
            &"a".repeat(100),
            0,
            false,
        ),
        item(
            ContextRole::Tool,
            ContextSource::Tool,
            &"b".repeat(104),
            1,
            false,
        ),
        item(ContextRole::User, ContextSource::User, "hello", 2, false),
    ];
    let budget = TierBudget {
        resident: 10_000,
        structured: 10_000,
        episodic: 10_000,
        semantic: 10_000,
        page_budget: 25,
    };
    let out = assemble_context_with_tier(items, &contract(1_000_000), &budget, &estimate_tokens);
    let seqs: Vec<u64> = out.items.iter().map(|i| i.seq).collect();
    assert_eq!(
        seqs,
        vec![0, 2],
        "超过页上限的条目必须被拒绝；恰好等于上限的条目保留（边界含等号）"
    );
    assert_eq!(out.diagnostics.page_rejected, 1, "页拒绝须可观测");
    assert_eq!(
        out.diagnostics.evicted, 0,
        "页拒绝是单条规则，不计入预算驱逐"
    );
    assert_eq!(
        out.diagnostics.budget_tokens, 40_000,
        "全局预算 = min(contract.max_tokens, 四项配额和)"
    );
}

#[test]
fn page_budget_zero_disables_cap() {
    let items = vec![
        item(
            ContextRole::Tool,
            ContextSource::Tool,
            &"a".repeat(4_000),
            0,
            false,
        ),
        item(ContextRole::Tool, ContextSource::Tool, "small", 1, false),
    ];
    let budget = TierBudget {
        resident: 10_000,
        structured: 10_000,
        episodic: 10_000,
        semantic: 10_000,
        page_budget: 0,
    };
    let out = assemble_context_with_tier(items, &contract(1_000_000), &budget, &estimate_tokens);
    assert_eq!(out.items.len(), 2, "page_budget=0 = 关闭页约束");
    assert_eq!(out.diagnostics.page_rejected, 0);
}

// ============================================================
// 2) R/S/E/M 分层配额
// ============================================================

#[test]
fn tier_quota_evicts_lowest_density_tool_items_first() {
    // 3 条 Tool（各 16 字符 = 4 token），S 配额 10 > 8 且 < 12
    let items: Vec<ContextItem> = (0..3)
        .map(|i| {
            item(
                ContextRole::Tool,
                ContextSource::Tool,
                &format!("{i:016}"),
                i as u64,
                false,
            )
        })
        .collect();
    let budget = TierBudget {
        resident: 10_000,
        structured: 10,
        episodic: 10_000,
        semantic: 10_000,
        page_budget: 0,
    };
    let out = assemble_context_with_tier(items, &contract(1_000_000), &budget, &estimate_tokens);
    let seqs: Vec<u64> = out.items.iter().map(|i| i.seq).collect();
    assert_eq!(
        seqs,
        vec![1, 2],
        "S 层按价值密度升序驱逐：最早出现（时近性最低）的 seq=0 先出"
    );
    assert_eq!(out.diagnostics.evicted, 1, "分层驱逐计入 evicted");
    let used: usize = out.items.iter().map(|i| estimate_tokens(&i.content)).sum();
    assert!(used <= 10, "S 层用量必须回落到配额内，used={used}");
}

#[test]
fn tier_quota_maps_memory_and_retrieval_to_their_own_tiers() {
    let items = vec![
        item(
            ContextRole::Assistant,
            ContextSource::Memory,
            &"m".repeat(32),
            0,
            false,
        ), // 8 token → E
        item(
            ContextRole::User,
            ContextSource::Retrieval,
            &"r".repeat(32),
            1,
            false,
        ), // 8 token → M
        item(
            ContextRole::User,
            ContextSource::User,
            &"u".repeat(32),
            2,
            false,
        ), // 8 token → R
    ];

    // E 配额 4 ⇒ 只驱逐 Memory；M/R 配额充足
    let episodic_tight = TierBudget {
        resident: 1_000,
        structured: 1_000,
        episodic: 4,
        semantic: 1_000,
        page_budget: 0,
    };
    let out = assemble_context_with_tier(
        items.clone(),
        &contract(1_000_000),
        &episodic_tight,
        &estimate_tokens,
    );
    let seqs: Vec<u64> = out.items.iter().map(|i| i.seq).collect();
    assert_eq!(seqs, vec![1, 2], "Memory→E 层，超 E 配额即驱逐");
    assert_eq!(out.diagnostics.evicted, 1);

    // M 配额 4 ⇒ 只驱逐 Retrieval
    let semantic_tight = TierBudget {
        resident: 1_000,
        structured: 1_000,
        episodic: 1_000,
        semantic: 4,
        page_budget: 0,
    };
    let out = assemble_context_with_tier(
        items,
        &contract(1_000_000),
        &semantic_tight,
        &estimate_tokens,
    );
    let seqs: Vec<u64> = out.items.iter().map(|i| i.seq).collect();
    assert_eq!(seqs, vec![0, 2], "Retrieval→M 层，超 M 配额即驱逐");
    assert_eq!(out.diagnostics.evicted, 1);
}

#[test]
fn zero_capacity_budget_evicts_everything() {
    // 退化预算（四项全 0）：容量为零 ⇒ 任何条目都装不下。语义为「显式零配额」，
    // 不是「未配置」——未配置的形态是**不提供** TierBudget（走旧入口）。
    let items = vec![
        item(ContextRole::User, ContextSource::User, "first", 0, false),
        item(ContextRole::User, ContextSource::User, "second", 1, false),
    ];
    let zero = TierBudget {
        resident: 0,
        structured: 0,
        episodic: 0,
        semantic: 0,
        page_budget: 0,
    };
    let out = assemble_context_with_tier(items, &contract(1_000_000), &zero, &estimate_tokens);
    assert!(out.items.is_empty(), "零容量预算不得放行任何条目");
    assert_eq!(out.diagnostics.evicted, 2);
    assert_eq!(out.diagnostics.budget_tokens, 0);
    assert_eq!(out.diagnostics.used_tokens, 0);
}

// ============================================================
// 3) 关态等价：旧入口逐字段不变
// ============================================================

#[test]
fn generous_budget_equals_legacy_entry_point() {
    // 含精确重复（seq=10 是 seq=3 的副本）与预算压力（max_tokens=30）的固定语料
    let mut items: Vec<ContextItem> = (0..10)
        .map(|i| {
            item(
                ContextRole::User,
                ContextSource::User,
                &format!("msg {i} {}", "x".repeat(20)),
                i,
                false,
            )
        })
        .collect();
    let duplicate = items[3].clone();
    items.push(ContextItem {
        seq: 10,
        ..duplicate
    });

    let legacy = assemble_context_with(items.clone(), &contract(30), 0, &estimate_tokens);
    let tiered =
        assemble_context_with_tier(items, &contract(30), &generous_budget(), &estimate_tokens);
    assert_eq!(
        tiered, legacy,
        "宽松预算下分层入口必须与旧入口逐字段相等（含诊断字段）"
    );
    assert!(
        tiered.diagnostics.evicted > 0,
        "本用例必须真正触发预算驱逐，否则证明不了等价路径"
    );
}

// ============================================================
// 4) 与 WindowPlan 的顺序契约（配额和 = resident_cap）
// ============================================================

#[test]
fn plan_tier_budget_bounds_assembly() {
    let plan = AdaptiveGovernor::new().plan(&PlanContext {
        effective_window: 1_000_000,
        budget_utilization: 0.0,
        thinking_mode: "Standard".into(),
    });
    let budget = plan.tier_budget();
    assert_eq!(
        budget.total(),
        plan.resident_cap,
        "四项配额和必须等于 resident_cap（plan 派生契约）"
    );

    // 200 条工具消息 × 1000 token = 200K > S 配额 160K；再加一条超页条目
    let mut items: Vec<ContextItem> = (0..200)
        .map(|i| {
            item(
                ContextRole::Tool,
                ContextSource::Tool,
                &format!("{i:04}{}", "x".repeat(3_996)),
                i as u64,
                false,
            )
        })
        .collect();
    items.push(item(
        ContextRole::Tool,
        ContextSource::Tool,
        &format!("big{}", "y".repeat(16_000)),
        200,
        false,
    ));

    let out = assemble_context_with_tier(items, &contract(10_000_000), &budget, &estimate_tokens);
    let used: usize = out.items.iter().map(|i| estimate_tokens(&i.content)).sum();
    assert!(
        used <= budget.structured as usize,
        "工具条目用量不得超过 S 配额：{used} > {}",
        budget.structured
    );
    assert!(
        used <= budget.assembly_capacity() as usize,
        "装配面总量不得超过 S+E+M"
    );
    assert_eq!(
        out.diagnostics.page_rejected, 1,
        "4001 token 的超页条目被拒绝"
    );
    assert!(out.diagnostics.evicted > 0, "S 配额必须触发实际驱逐");
}

// ============================================================
// 5) 属性测试：子序列 + 页上限 + 分层配额 + 账目守恒
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn tier_budget_invariants(
        contents in prop::collection::vec("[a-z]{1,40}", 1..10),
        page in 1u32..40,
        quota in 0u32..80,
    ) {
        let items: Vec<ContextItem> = contents
            .iter()
            .enumerate()
            .map(|(i, content)| {
                let (role, source) = match i % 4 {
                    0 => (ContextRole::System, ContextSource::System),
                    1 => (ContextRole::Tool, ContextSource::Tool),
                    2 => (ContextRole::Assistant, ContextSource::Memory),
                    _ => (ContextRole::User, ContextSource::Retrieval),
                };
                item(role, source, content, i as u64, false)
            })
            .collect();
        let budget = TierBudget {
            resident: quota,
            structured: quota,
            episodic: quota,
            semantic: quota,
            page_budget: page,
        };
        let input_len = items.len();
        let out = assemble_context_with_tier(items, &contract(1_000_000), &budget, &estimate_tokens);

        // (a) 输出是输入的子序列：seq 严格递增
        let seqs: Vec<u64> = out.items.iter().map(|i| i.seq).collect();
        prop_assert!(seqs.windows(2).all(|w| w[0] < w[1]), "seq 必须严格递增: {seqs:?}");

        // (b) 页上限：保留条目逐条 ≤ page_budget
        for kept in &out.items {
            prop_assert!(
                estimate_tokens(&kept.content) <= page as usize,
                "保留条目超页: {:?} = {}", kept.content, estimate_tokens(&kept.content)
            );
        }

        // (c) 分层配额：每层用量 ≤ 该层配额
        let mut used = [0usize; 4];
        for kept in &out.items {
            used[tier_index(kept.source)] += estimate_tokens(&kept.content);
        }
        for (tier, value) in used.iter().enumerate() {
            prop_assert!(*value <= quota as usize, "层 {tier} 超配额: {value} > {quota}");
        }

        // (d) 账目守恒：输入 = 保留 + 去重 + 页拒绝 + 预算驱逐
        prop_assert_eq!(
            input_len,
            out.items.len() + out.diagnostics.deduped + out.diagnostics.page_rejected
                + out.diagnostics.evicted,
            "每条输入必须且只能落入一个去向"
        );
    }
}
