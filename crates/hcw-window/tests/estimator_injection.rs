//! R12 W1-2：装配内核 token 估算**可注入**的契约测试
//!
//! 本文件锁定的不是"注入功能存在"，而是三条更难的性质：
//!
//! 1. [`default_estimator_equals_legacy`]：`assemble_context`（旧签名/旧语义）与
//!    `assemble_context_with(..., &estimate_tokens)` **逐字段相等** —— 委托重构
//!    零漂移，含 `diagnostics.used_tokens` 这种"只在驱逐时才分化"的字段。
//! 2. [`custom_estimator_changes_budget_not_order`]：注入别的尺子只改变**驱逐
//!    条数**，不改变条目的相对顺序与内容（装配内核的两条不变量与计量无关）。
//! 3. [`all_estimations_use_injected`]（关键）：预算累计与 `used_tokens` **两处**
//!    都必须反映注入值。用恒定值估算器断言 `used_tokens == 保留条数 × 值`：
//!    任一处漏改回默认估算器，这条断言都会红（漏改"累计" ⇒ 不驱逐 ⇒ used 是
//!    字符口径；漏改"扣减" ⇒ used 只降了字符口径的那点值）。
//!
//! 外加 proptest：任意条目序列 + 任意恒定估算值 ⇒ `used_tokens == 保留数 × 值`。
//!
//! # 口径固化（防止有人"顺手"改 `estimate_tokens`）
//! [`legacy_estimator_value_is_frozen`] 把默认估算器的**值**（字符数/4 向上取整、
//! 最小 1）钉成常量断言。它与既有 `assembly_contract.rs` 的 17 处按数字断言的
//! 用例同向：改公式会同时红在这里，从而不能借用"重构"之名重定标。

use hcw_window::assemble::{
    assemble_context, assemble_context_with, estimate_tokens, AssembledContext, ContextItem,
    ContextRole, ContextSource,
};
use nexus_contracts::spec::harness_dimensions::ContextAssemblyContract;
use proptest::prelude::*;

// ============================================================
// 夹具
// ============================================================

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

/// 恒定值估算器 —— 每条固定 `value` token，与内容无关
///
/// WHY 用它做"注入是否生效"的探针：恒定值的输出可**反解**（保留数 × 值），
/// 而任何真实估算器（字符/4、字节/4）算出的数字无法与"条数 × 常量"混淆
/// —— 一旦实现里有某一处仍走默认估算器，反解式立刻不成立。
fn constant_estimator(value: usize) -> impl Fn(&str) -> usize {
    move |_content: &str| value
}

/// 6 条角色/来源各异的条目，内容全部 4 字符（默认估算器恰好各 1 token）
///
/// WHY 选 4 字符：默认口径下"每条 1 token"，故 `total_tokens == 条数`，
/// 使"默认口径的结果"与"恒定值 1 的结果"重合 —— 想区分两把尺子必须靠
/// 恒定值 ≠ 1，这也让漏改导致的读数差异不会被巧合掩盖。
fn six_items() -> Vec<ContextItem> {
    vec![
        item(ContextRole::User, ContextSource::User, "aaa0", false, 0),
        item(ContextRole::User, ContextSource::User, "bbb1", false, 1),
        item(ContextRole::User, ContextSource::User, "ccc2", false, 2),
        item(ContextRole::User, ContextSource::User, "ddd3", false, 3),
        item(ContextRole::User, ContextSource::User, "eee4", false, 4),
        item(ContextRole::User, ContextSource::User, "fff5", false, 5),
    ]
}

/// 保留条目的 `seq` 序列
fn seqs(out: &AssembledContext) -> Vec<u64> {
    out.items.iter().map(|i| i.seq).collect()
}

// ============================================================
// 1. 默认入口 == 显式注入默认估算器（零漂移）
// ============================================================

#[test]
fn default_estimator_equals_legacy() {
    // 三种输入形态都要逐字段相等：① 空输入（走 early-return 分支）
    // ② 无重复且预算充足（不触摸驱逐）③ 超预算（走驱逐 + used 扣减）
    let cases: Vec<(Vec<ContextItem>, usize, usize)> = vec![
        (Vec::new(), 128_000, 4_096),
        (six_items(), 128_000, 4_096),
        (six_items(), 3, 0),
        (six_items(), 0, 3),
        // 去重 + 驱逐同时发生（两条重复项先被折叠，再按预算驱逐）
        (
            {
                let mut items = six_items();
                items.push(item(
                    ContextRole::User,
                    ContextSource::User,
                    "aaa0",
                    false,
                    6,
                ));
                items
            },
            2,
            0,
        ),
    ];

    for (items, max_tokens, tier_capacity) in cases {
        let contract = contract(max_tokens);
        let legacy = assemble_context(items.clone(), &contract, tier_capacity);
        let injected = assemble_context_with(items, &contract, tier_capacity, &estimate_tokens);

        // 逐字段相等（AssembledContext/AssemblyDiagnostics 均 derive PartialEq：
        // 一次 `assert_eq!` 覆盖 items + 全部 6 个诊断字段，含 used_tokens）
        assert_eq!(
            legacy, injected,
            "assemble_context 必须逐字段等于 assemble_context_with(..., &estimate_tokens); \
             max_tokens={max_tokens}, tier_capacity={tier_capacity}"
        );
        // 单独再钉一次 used_tokens：它是唯一"只在驱逐发生时才分化"的字段，
        // 上面那条整体相等若因未来某字段被移出 PartialEq 而失效，这条仍成立。
        assert_eq!(
            legacy.diagnostics.used_tokens, injected.diagnostics.used_tokens,
            "used_tokens 必须逐值相等"
        );
    }
}

#[test]
fn legacy_estimator_value_is_frozen() {
    // 默认估算器的**值**是冻结面（R12 W1-2 硬约束"不得改变 estimate_tokens 的值"）。
    // 既有 17 处 assembly_contract 断言按具体数字锁定同一件事，本断言只是把它
    // 显式写成一张小表，使"改公式"在最近的地方红。
    assert_eq!(estimate_tokens("abcd"), 1, "4 字符/4 = 1");
    assert_eq!(estimate_tokens("abcde"), 2, "5 字符/4 向上取整 = 2");
    assert_eq!(estimate_tokens("abc"), 1, "3 字符/4 向上取整 = 1，且最小 1");
    assert_eq!(estimate_tokens(""), 1, "空串按最小 1（内核历史语义）");
    assert_eq!(
        estimate_tokens("你好世界"),
        1,
        "中文按**字符数**/4 = 1（内核口径；mca 的字节宽口径为 3）—— \
         任何把中文口径改成字节宽的改动都会红在这里"
    );
}

// ============================================================
// 2. 定制估算器只改预算，不改顺序
// ============================================================

#[test]
fn custom_estimator_changes_budget_not_order() {
    let items = six_items();
    let reference = items.clone();

    // 默认口径:6 条 × 1 token = 6 ≤ 预算 25 ⇒ 不驱逐、全部保留
    let default_out = assemble_context(items.clone(), &contract(25), 0);
    assert_eq!(default_out.diagnostics.used_tokens, 6);
    assert_eq!(default_out.diagnostics.evicted, 0, "默认口径下预算充足");
    assert_eq!(seqs(&default_out), vec![0, 1, 2, 3, 4, 5]);

    // 恒定 10:60 > 25 ⇒ 驱逐到 used ≤ 25,即 4 条被淘汰、保留 2 条(used=20)
    let custom_out = assemble_context_with(items, &contract(25), 0, &constant_estimator(10));
    assert_eq!(
        custom_out.diagnostics.evicted, 4,
        "恒定 10 token ⇒ 必须驱逐 4 条(60→20)"
    );
    assert_eq!(custom_out.diagnostics.used_tokens, 20);
    assert_eq!(custom_out.items.len(), 2);

    // ① 条数变了（这是注入生效的正面证据）
    assert_ne!(
        custom_out.items.len(),
        default_out.items.len(),
        "注入的估算器必须改变**驱逐条数**"
    );

    // ② 顺序没变:保留项在输入中的相对次序严格递增
    let kept = seqs(&custom_out);
    assert!(
        kept.windows(2).all(|w| w[0] < w[1]),
        "保留项必须保持输入相对顺序, got {kept:?}"
    );
    // ③ 内容没被改写（装配只决定去留，不重写条目）
    for kept_item in &custom_out.items {
        let original = &reference[kept_item.seq as usize];
        assert_eq!(
            kept_item.content, original.content,
            "装配不得改写条目内容(seq={})",
            kept_item.seq
        );
        assert_eq!(kept_item.role, original.role);
        assert_eq!(kept_item.source, original.source);
        assert_eq!(kept_item.pinned, original.pinned);
    }
    // ④ 保留集 == 从输入按序过滤保留者的结果（顺序等价的可执行判据）
    let expected: Vec<String> = reference
        .iter()
        .filter(|i| kept.contains(&i.seq))
        .map(|i| i.content.clone())
        .collect();
    let actual: Vec<String> = custom_out.items.iter().map(|i| i.content.clone()).collect();
    assert_eq!(actual, expected, "保留集必须是输入的相对顺序子序列");
}

// ============================================================
// 3. 关键:所有估算点都走注入器
// ============================================================

#[test]
fn all_estimations_use_injected() {
    const VALUE: usize = 10;

    // 情形 A:预算充足(不驱逐)——只有"预算累计"这一处起作用。
    // 若累计仍走默认估算器(每条 1),used 会是 6；注入生效则必须是 60。
    let ample = assemble_context_with(six_items(), &contract(1_000), 0, &constant_estimator(VALUE));
    assert_eq!(ample.diagnostics.evicted, 0);
    assert_eq!(
        ample.diagnostics.used_tokens,
        VALUE * ample.items.len(),
        "预算累计必须反映注入值(不是默认的字符/4)"
    );
    assert_eq!(ample.diagnostics.used_tokens, 60);

    // 情形 B:预算紧张(发生驱逐)——"累计"与"扣减"两处同时起作用。
    // 6 条 × 10 = 60 > 30 ⇒ 驱逐 3 条 ⇒ 保留 3 条、used = 30。
    // 漏改扣减点(仍减默认的 1)会让 used = 60 - 3 = 57 ≠ 30 ⇒ 本断言红。
    let tight = assemble_context_with(six_items(), &contract(30), 0, &constant_estimator(VALUE));
    assert_eq!(
        tight.diagnostics.used_tokens,
        VALUE * tight.items.len(),
        "used_tokens 必须恰等于 保留条数 × 注入值(任何一处漏改都会破坏它)"
    );
    assert_eq!(tight.items.len(), 3, "预算 30 / 每条 10 ⇒ 恰保留 3 条");
    assert_eq!(tight.diagnostics.evicted, 3);
    assert_eq!(tight.diagnostics.used_tokens, 30);

    // 情形 C:预算为 0 —— 驱逐必须把 used 一路减到 0(恒定 10 时可减尽)，
    // 而不是停在"默认口径的残值"上。这条同时排除 saturating_sub 掩盖漏改。
    let zero = assemble_context_with(six_items(), &contract(0), 0, &constant_estimator(VALUE));
    assert_eq!(zero.items.len(), 0, "预算 0 ⇒ 全部驱逐");
    assert_eq!(zero.diagnostics.evicted, 6);
    assert_eq!(
        zero.diagnostics.used_tokens, 0,
        "全部驱逐后 used_tokens 必须归零(扣减走的是同一个注入值)"
    );

    // 情形 D:预算取小(tier 更小)仍按注入口径计量 —— 预算来源与估算口径正交
    let tier_bound = assemble_context_with(
        six_items(),
        &contract(1_000),
        20,
        &constant_estimator(VALUE),
    );
    assert_eq!(
        tier_bound.diagnostics.budget_tokens, 20,
        "预算 = min(1000, 20)"
    );
    assert_eq!(tier_bound.items.len(), 2);
    assert_eq!(tier_bound.diagnostics.used_tokens, 20);
}

// ============================================================
// 4. proptest:恒定估算器 ⇒ used_tokens == 保留数 × 值
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// 任意条目序列 + 任意恒定估算值 + 任意预算 ⇒ `used_tokens == 保留数 × 值`
    ///
    /// WHY 用恒定值:它使 `used_tokens` 可反解。若实现里有任何一处估算点仍走默认
    /// 估算器(字符/4),这条恒等式在"驱逐发生"的样本上必然崩 —— 而随机预算
    /// 保证这类样本一定出现。
    #[test]
    fn prop_used_tokens_equals_kept_times_constant(
        raw in proptest::collection::vec((0u8..4, 0u8..6, "[a-z]{0,12}", any::<bool>()), 0..24),
        value in 1usize..50,
        budget in 0usize..200,
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
                // 序号前缀:生成集内无精确重复(去重会随机折叠会稀释"预算累计"的
                // 可观测性 —— 本属性要的是计量恒等式,不是去重语义)
                ContextItem {
                    role,
                    source,
                    content: format!("{i}:{content}"),
                    pinned,
                    seq: i as u64,
                }
            })
            .collect();

        let input_len = items.len();
        let out = assemble_context_with(items, &contract(budget), 0, &constant_estimator(value));

        prop_assert_eq!(
            out.diagnostics.used_tokens,
            out.items.len() * value,
            "used_tokens 必须恰等于 保留条数 × 注入值 (kept={}, value={}, used={})",
            out.items.len(),
            value,
            out.diagnostics.used_tokens
        );
        // 附带:条数守恒 —— 装配不得凭空空降或吞掉条目
        prop_assert_eq!(
            out.items.len() + out.diagnostics.evicted + out.diagnostics.deduped,
            input_len,
            "保留 + 驱逐 + 去重 必须等于输入条数"
        );
    }

    /// 恒定估算器的另一面:预算充足(>= 条数 × 值)时**不得**驱逐任何条目
    ///
    /// 这条把"累计用了哪个口径"变成可判定的:若累计仍走默认口径(值 ≥ 1 但通常
    /// 远小于注入值),预算"看起来充足"而实际按注入值算已超限 —— 反过来,若累计
    /// 走了注入值而扣减没走,本属性不会红(由上面那条负责)。两条属性合起来把
    /// 累计/扣减两个点各自钉死。
    #[test]
    fn prop_generous_budget_never_evicts(
        raw in proptest::collection::vec("[a-z]{1,8}", 0..16),
        value in 1usize..20,
    ) {
        let items: Vec<ContextItem> = raw
            .into_iter()
            .enumerate()
            .map(|(i, content)| ContextItem {
                role: ContextRole::User,
                source: ContextSource::User,
                content: format!("{i}:{content}"),
                pinned: false,
                seq: i as u64,
            })
            .collect();
        // 预算 = 上界(去重后条数 ≤ 输入条数) × 值 ⇒ 按注入口径必然充足
        let budget = items.len() * value;
        let out = assemble_context_with(items, &contract(budget), 0, &constant_estimator(value));
        prop_assert_eq!(out.diagnostics.evicted, 0, "预算充足时不得驱逐");
        prop_assert_eq!(out.diagnostics.used_tokens, out.items.len() * value);
    }
}
