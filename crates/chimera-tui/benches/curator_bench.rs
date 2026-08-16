//! 策展器性能基准(Concord W9 T9.2,ADR-081)
//!
//! 对应架构层:L10 Interface(`chimera_tui::data::curator`)
//!
//! # 基准项与目标(方案 §5.3)
//! - `curate_500_msgs_budget_4096`:n=500、W=4096 的完整策展(分类+打分+
//!   背包+摘要组装),目标 <10ms(O(n·W) 上界 ≈ 2M 单元操作);
//! - `classify_500_msgs`:纯分类+打分吞吐(O(n));
//! - `knapsack_tight_budget`:紧预算下候选全参与背包的最坏形态。
//!
//! # 设计理由(WHY)
//! - 策展是触发式操作(/compact),非每帧热路径,但同步执行于管道任务,
//!   必须保证单次 <10ms 以不占满 250ms tick 预算(方案 §5.3 实测约束)。
//! - 消息内容确定性构造(固定种子长度序列),排除随机噪声。

#![forbid(unsafe_code)]

use chimera_tui::data::curator::{
    classify, CompactPolicy, CurationConfig, CurationPolicy, RuleCurationPolicy,
};
use chimera_tui::types::{ChatMessage, ChatRole};
use criterion::{criterion_group, criterion_main, Criterion};

/// 构造 n 轮交替历史(user 短问 + assistant 长答),确定性长度
fn history(n: usize) -> Vec<ChatMessage> {
    let mut v = Vec::with_capacity(n * 2);
    for i in 0..n {
        v.push(ChatMessage {
            role: ChatRole::User,
            content: format!("question {i} about topic {}", i % 7),
        });
        // 长度 80-180 字符循环(≈20-45 token),模拟真实回答分布
        let filler = "x".repeat(80 + (i % 5) * 20);
        v.push(ChatMessage {
            role: ChatRole::Assistant,
            content: format!("answer {i}: {filler} @reviewer"),
        });
    }
    v
}

fn curator_perf(c: &mut Criterion) {
    let msgs_500 = history(250); // 250 轮 = 500 条
    let cfg = CurationConfig {
        budget_tokens: 4096,
        recent_turns: 4,
        ..Default::default()
    };
    let policy = RuleCurationPolicy;

    let mut group = c.benchmark_group("curator");
    group.bench_function("curate_500_msgs_budget_4096", |b| {
        b.iter(|| {
            let plan = policy.curate(
                criterion::black_box(&msgs_500),
                criterion::black_box(&cfg),
                CompactPolicy::Conservative,
            );
            criterion::black_box(plan);
        });
    });
    group.bench_function("classify_500_msgs", |b| {
        b.iter(|| {
            let scored = classify(criterion::black_box(&msgs_500), criterion::black_box(&cfg));
            criterion::black_box(scored);
        });
    });
    group.bench_function("curate_tight_budget", |b| {
        let tight = CurationConfig {
            budget_tokens: 200,
            recent_turns: 1,
            ..Default::default()
        };
        b.iter(|| {
            let plan = policy.curate(
                criterion::black_box(&msgs_500),
                criterion::black_box(&tight),
                CompactPolicy::Aggressive,
            );
            criterion::black_box(plan);
        });
    });
    group.finish();
}

criterion_group!(benches, curator_perf);
criterion_main!(benches);
