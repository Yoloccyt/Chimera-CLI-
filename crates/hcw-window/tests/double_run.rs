//! 压缩前后任务成功率双跑零差异（P2-T4/T14 退出标准）
//!
//! 波次退出标准：压缩不得降低任务成功率（信息悬崖消除验证）。
//! 方法：合成"事实检索任务"（从上下文定位关键事实）——压缩前执行
//! 与压缩后执行的成功率必须一致（零差异）；关键事实（高分条目）必须
//! 在压缩后仍可检索（thinking/关键锚点保留语义）。

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use hcw_window::pipeline::CompressionPipeline;
use hcw_window::preserve::{ConversationContext, ThinkingBlock};
use hcw_window::types::ContextEntry;
use hcw_window::HcwConfig;

fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 25, 0, 0, 0).unwrap()
}

/// 构造会话：N 条条目（含 M 个"关键事实"——高频访问，评分高）+ thinking
fn synth_session(n: usize, key_facts: usize) -> ConversationContext {
    let mut body = Vec::new();
    for i in 0..n {
        let is_key = i < key_facts;
        // 关键事实：高频访问（access_count 大 → 评分高 → 压缩保留）
        let content = if is_key {
            format!("KEY-FACT-{i}: 关键决策锚点内容，压缩必须保留")
        } else {
            format!("noise-{i}: 冗余上下文填充内容，可压缩丢弃")
        };
        let tokens = content.chars().count() / 3 + 1;
        let mut e = ContextEntry::new(format!("e-{i}"), format!("f-{}", i % 5), content, tokens);
        if is_key {
            e.access_count = 100; // 高频 → 高评分
        }
        body.push(Arc::new(e));
    }
    let thinking = (0..8)
        .map(|i| ThinkingBlock::new(i as u64, format!("thinking-trace-{i}")))
        .collect();
    ConversationContext::new("static-prefix", body, thinking)
}

/// 事实检索任务：从上下文中定位关键事实（成功 = 找到且内容完整）
fn run_retrieval_tasks(ctx: &ConversationContext, key_facts: usize) -> (u32, u32) {
    let mut success = 0;
    let mut total = 0;
    for i in 0..key_facts {
        let needle = format!("KEY-FACT-{i}");
        total += 1;
        let found = ctx.body.iter().any(|e| e.content.contains(needle.as_str()));
        if found {
            success += 1;
        }
    }
    (success, total)
}

/// 端到端双跑：压缩前后任务成功率必须零差异（退出标准）
///
/// 预算设置：低预算（≤ 噪声总量）强制压缩到 Collapse/Autocompact 级，
/// 验证关键事实（高评分）在激进压缩下仍保留。
#[test]
fn double_run_zero_difference_compressed_vs_full() {
    let pipeline = CompressionPipeline::new(HcwConfig::default());
    let ctx = synth_session(100, 10); // 100 条 / 10 关键事实
                                      // 压缩前（全量上下文）任务
    let (full_ok, full_total) = run_retrieval_tasks(&ctx, 10);
    assert_eq!(full_total, 10);
    assert_eq!(full_ok, 10, "全量上下文必须全成功（测试前置）");

    // 压缩后（激进预算：仅容 10 条噪声 token）任务
    let out = pipeline.compress(&ctx, 400, None, fixed_now());
    let (comp_ok, comp_total) = run_retrieval_tasks(&out.context, 10);

    // 零差异：压缩后关键事实检索成功率必须 = 压缩前
    assert_eq!(
        comp_ok, full_ok,
        "压缩前后任务成功率必须零差异: full={full_ok}/{full_total} compressed={comp_ok}/{comp_total}"
    );
    assert_eq!(comp_total, full_total);
}

/// thinking 完整率在双跑中保持 100%（压缩链路不触碰）
#[test]
fn double_run_thinking_preserved_bytewise() {
    let pipeline = CompressionPipeline::new(HcwConfig::default());
    let ctx = synth_session(100, 10);
    let out = pipeline.compress(&ctx, 400, None, fixed_now());
    assert_eq!(out.context.thinking.len(), ctx.thinking.len());
    for (o, i) in out.context.thinking.iter().zip(ctx.thinking.iter()) {
        assert_eq!(
            o.content.as_bytes(),
            i.content.as_bytes(),
            "thinking 逐字节一致"
        );
    }
}

/// 前缀逐字节不变（缓存前缀不失效——from 模式）
#[test]
fn double_run_prefix_unchanged() {
    let pipeline = CompressionPipeline::new(HcwConfig::default());
    let ctx = synth_session(100, 10);
    let out = pipeline.compress(&ctx, 400, None, fixed_now());
    assert_eq!(
        out.context.prefix.as_bytes(),
        ctx.prefix.as_bytes(),
        "前缀必须逐字节不变"
    );
}

/// 压缩后 token 不增（proptest 不变量在端到端场景成立）
#[test]
fn double_run_tokens_non_increasing() {
    let pipeline = CompressionPipeline::new(HcwConfig::default());
    let ctx = synth_session(100, 10);
    let out = pipeline.compress(&ctx, 400, None, fixed_now());
    let before: usize = ctx.body.iter().map(|e| e.token_size).sum();
    let after: usize = out.context.body.iter().map(|e| e.token_size).sum();
    assert!(
        after <= before,
        "压缩后 token 必须 ≤ 压缩前: {after} > {before}"
    );
}
