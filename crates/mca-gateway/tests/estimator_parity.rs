//! R12 W1-2：`mca-gateway` 注入装配内核后"同一把尺子"的证据
//!
//! 本波次要证明的**不是**"内核算出的数字等于 mca 的 `estimate_tokens(messages)`"
//! —— 那在结构上不成立（内核看到的是去重键用的扁平化文本，不是消息块原文，
//! 详见报告 `docs/reports/r12-w1-2-estimator.md` 的已知边界 §1）。要证明的是更强
//! 也更本质的一条：
//!
//! > 内核里的 **token 计量函数**就是 `mca_gateway::estimate_text`（UTF-8 字节宽/4
//! > floor），因此 `diagnostics.used_tokens` 的**单位**归 mca 所有。
//!
//! 判据（三条，互相独立）：
//! 1. [`kernel_reports_mca_units_when_injected`]：`used_tokens` 恰等于
//!    `Σ estimate_text(交给内核的那份 content)`；
//! 2. [`default_kernel_units_differ_on_cjk`]：同一批条目走内核**默认**估算器时读数
//!    不同（中文 3 倍差），证明"注入确实换了尺子"而不是巧合相等；
//! 3. [`injected_units_drive_eviction_count`]：**若**把真实预算喂给内核（下一波才
//!    允许的动作），驱逐条数由 mca 单位决定 —— 这是"启用驱逐的前置已就绪"的
//!    可执行证据，本波仍不启用。
//!
//! 说明：本文件只用纯函数面（`assemble_context_with` / `estimate_text`），不起
//! 完整 MCA 链路（无需通道/网络/spec），故不受 `CHIMERA_ASSEMBLY_WIRED` 影响。

use hcw_window::assemble::{
    assemble_context, assemble_context_with, ContextItem, ContextRole, ContextSource,
};
use mca_gateway::estimate_text;
use nexus_contracts::spec::harness_dimensions::ContextAssemblyContract;

/// 注入器 —— 与生产侧 `conversation_trim::assembly_token_estimate` 同实现同类型
/// （`u32 → usize` 扩宽在本文件里重复一次，因为它那边是 `pub(crate)`）
fn mca_estimator(content: &str) -> usize {
    estimate_text(content) as usize
}

fn item(source: ContextSource, content: &str, seq: u64) -> ContextItem {
    ContextItem {
        role: match source {
            ContextSource::Tool => ContextRole::Tool,
            ContextSource::User => ContextRole::User,
            _ => ContextRole::Assistant,
        },
        source,
        content: content.to_string(),
        pinned: false,
        seq,
    }
}

fn contract(max_tokens: usize) -> ContextAssemblyContract {
    let mut c = ContextAssemblyContract::default_contract();
    c.max_tokens = max_tokens;
    c
}

/// 4 条互不相同的中文条目（各 4 字符 = 12 字节）：mca 口径各 3 token
fn cjk_items() -> Vec<ContextItem> {
    vec![
        item(ContextSource::Tool, "一二三四", 0),
        item(ContextSource::Tool, "五六七八", 1),
        item(ContextSource::Tool, "甲乙丙丁", 2),
        item(ContextSource::Tool, "戊己庚辛", 3),
    ]
}

// ============================================================
// 1. used_tokens == Σ estimate_text(内核看到的 content)
// ============================================================

#[test]
fn kernel_reports_mca_units_when_injected() {
    let items = cjk_items();
    let expected: usize = items
        .iter()
        .map(|i| mca_estimator(&i.content)) // 内核看到的正是这份 content
        .sum();
    assert_eq!(expected, 12, "4 条 × (12 字节/4) = 12");

    let out = assemble_context_with(items, &contract(usize::MAX), 0, &mca_estimator);
    assert_eq!(
        out.diagnostics.used_tokens, expected,
        "注入后 used_tokens 必须是 mca 单位（字节宽/4 floor）"
    );
    assert_eq!(out.items.len(), 4, "预算非约束 ⇒ 不驱逐");
    assert_eq!(out.diagnostics.evicted, 0);
    assert_eq!(out.diagnostics.budget_tokens, usize::MAX);

    // 混合语料（中英 + emoji 4 字节宽）复核同一恒等式
    let mixed = vec![
        item(ContextSource::Tool, "tool ok", 0),
        item(ContextSource::Tool, "你好世界", 1),
        item(ContextSource::Tool, "🦀🦀", 2),
    ];
    let mixed_expected: usize = mixed.iter().map(|i| mca_estimator(&i.content)).sum();
    // "tool ok" 7/4=1、"你好世界" 12/4=3、"🦀🦀" 8/4=2 ⇒ 6
    assert_eq!(mixed_expected, 6);
    let mixed_out = assemble_context_with(mixed, &contract(usize::MAX), 0, &mca_estimator);
    assert_eq!(mixed_out.diagnostics.used_tokens, mixed_expected);
}

// ============================================================
// 2. 默认口径在同一语料上给出不同读数（注入真的换了尺子）
// ============================================================

#[test]
fn default_kernel_units_differ_on_cjk() {
    let items = cjk_items();
    let injected = assemble_context_with(items.clone(), &contract(usize::MAX), 0, &mca_estimator);
    let default = assemble_context(items.clone(), &contract(usize::MAX), 0);

    // 内核默认 = 字符数/4 向上取整、最小 1：4 字符 ⇒ 1 token/条
    assert_eq!(default.diagnostics.used_tokens, 4);
    // mca = 字节宽/4 floor：12 字节 ⇒ 3 token/条
    assert_eq!(injected.diagnostics.used_tokens, 12);
    assert_eq!(
        injected.diagnostics.used_tokens,
        default.diagnostics.used_tokens * 3,
        "中文上两套口径恰好 3 倍差（0.75 vs 0.25 token/字符）"
    );
    // 条目本身不受估算器影响（去重/顺序/内容与计量口径正交）
    assert_eq!(default.items, injected.items);
}

// ============================================================
// 3. 前置已就绪：注入口径决定驱逐条数（本波仍不启用）
// ============================================================

#[test]
fn injected_units_drive_eviction_count() {
    // 真实预算（= 下一波才允许的动作，此处只作"前置就绪"的证据）
    let items = cjk_items(); // mca 口径 12 token，内核默认口径 4 token

    // ① mca 单位 + 预算 6 ⇒ 只留 2 条（12 → 9 → 6 ≤ 6）
    let by_mca = assemble_context_with(items.clone(), &contract(6), 0, &mca_estimator);
    assert_eq!(by_mca.items.len(), 2, "mca 单位下预算 6 只能装 2 条");
    assert_eq!(by_mca.diagnostics.evicted, 2);
    assert_eq!(by_mca.diagnostics.used_tokens, 6);
    assert_eq!(
        by_mca.diagnostics.used_tokens,
        by_mca.items.len() * 3,
        "used_tokens 与保留条数在同一把尺子下自洽"
    );

    // ② 同一预算走内核默认单位 ⇒ 4 ≤ 6，一条都不驱逐
    let by_default = assemble_context(items, &contract(6), 0);
    assert_eq!(by_default.items.len(), 4, "默认单位下预算 6 装得下 4 条");
    assert_eq!(by_default.diagnostics.evicted, 0);

    // ③ 结论：换算口径后就**不再**是"双倍过度裁剪"——同一份预算在两种单位下
    //    给出不同（而非叠加）的驱逐决策。这正是 R12 W1-2 打通的那条路。
    assert_ne!(
        by_mca.items.len(),
        by_default.items.len(),
        "口径不同 ⇒ 驱逐条数不同（同一份预算，而不是两本账叠加）"
    );
}
