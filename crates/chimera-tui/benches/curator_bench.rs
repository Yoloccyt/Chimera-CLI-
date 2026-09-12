//! curator(`/compact` 上下文策展)基准 — FC-2 接线验收(ADR-081)
//!
//! 测量 `RuleCurationPolicy::curate` 在真实会话规模下的端到端耗时
//! (五段分类 + 价值打分 + 0-1 背包 O(n·W) + 抽取式摘要):
//! - `msgs/500`:默认 `max_chat_messages` 上限(生产会话可达规模);
//! - `msgs/5000`:压力规模(预算 O(n·W) 线性段的拐点探查)。
//!
//! 运行:`cargo bench -p chimera-tui --bench curator_bench -- --measurement-time 1`

#![forbid(unsafe_code)]

use chimera_tui::data::curator::{
    CompactPolicy, CurationConfig, CurationPolicy, RuleCurationPolicy,
};
use chimera_tui::types::{ChatMessage, ChatRole};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// 构造交替轮次历史(n 轮 = 2n 条消息),assistant 回复带真实长度内容
fn history(turns: usize) -> Vec<ChatMessage> {
    (0..turns)
        .flat_map(|i| {
            [
                ChatMessage {
                    role: ChatRole::User,
                    content: format!("question {i}: 分析第 {i} 个模块的性能瓶颈并给出优化建议"),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    content: format!(
                        "answer {i}: 经过 profile 分析,热点集中在 {i} 号路径的序列化开销;\
                         建议引入零拷贝解码并复用缓冲区,预计可降低 40% 延迟。@benchmark"
                    ),
                },
            ]
        })
        .collect()
}

fn bench_curate(c: &mut Criterion) {
    let mut group = c.benchmark_group("curator_compact");
    for turns in [250usize, 2500] {
        let msgs = history(turns);
        group.bench_function(format!("msgs/{}", msgs.len()), |b| {
            b.iter(|| {
                let plan = RuleCurationPolicy.curate(
                    black_box(&msgs),
                    &CurationConfig::default(),
                    CompactPolicy::Balanced,
                );
                black_box(&plan.report);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_curate);
criterion_main!(benches);
