//! chimera-cli Quest 编排器高频 chunk 生产性能基准。
//!
//! WHY 此 bench:Quest 编排器把分解结果逐字符发 chunk,每字符一次 `String` 分配。
//! 本 bench 量化 `build_quest_reply` + `plan_chunks` 的 chunk 生产吞吐(chunks/sec),
//! 建立"高频 chunk 生产"基线,作为后续优化(如缓冲复用 / SmallString)的可证伪依据。
//!
//! 度量分工:本 bench 覆盖生产侧(编排器 chunk 生成);渲染侧(engine 单 token
//! diff)由 `chimera-tui/benches/streaming_bench.rs` 覆盖,二者合围高频 chunk 端到端性能。
//!
//! 架构层归属:L10 Interface(bench 不入架构层,仅 dev-artifact)。

use chimera_cli::orchestrator::{build_quest_reply, plan_chunks};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};

/// 构造含 `n` 个任务的样例 Quest(reply 长度随任务数增长)
fn quest_with_tasks(n: usize) -> Quest {
    let tasks = (0..n)
        .map(|i| Task {
            task_id: format!("t{i}"),
            description: format!("执行第 {i} 步子任务"),
            status: TaskStatus::Pending,
            dependencies: if i == 0 {
                vec![]
            } else {
                vec![format!("t{}", i - 1)]
            },
        })
        .collect();
    Quest {
        quest_id: "q-bench".into(),
        title: "性能基准需求".into(),
        tasks,
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

/// 逐字符 chunk 规划吞吐:对不同任务规模测量 `build_quest_reply` + `plan_chunks`
/// 的完整 chunk 生产成本(`Throughput::Elements` 报告 chunks/sec)。
///
/// 覆盖路径与 `stream_quest` 分块一致(构造回复 + 逐字符分块),故本基线直接反映
/// 编排器每轮分解回发的 chunk 生产开销。
fn bench_plan_chunks(c: &mut Criterion) {
    let mut group = c.benchmark_group("quest_plan_chunks");
    // 代表性分解规模:小 / 中 / 大(与 QuestConfig max_tasks_per_quest=16 对齐)
    for tasks in [2usize, 8, 16] {
        let quest = quest_with_tasks(tasks);
        let reply = build_quest_reply(&quest);
        let chunk_count = reply.chars().count() as u64;
        group.throughput(Throughput::Elements(chunk_count));
        group.bench_with_input(BenchmarkId::from_parameter(tasks), &quest, |b, q| {
            b.iter(|| {
                let reply = build_quest_reply(black_box(q));
                let chunks = plan_chunks(&reply);
                black_box(chunks);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_plan_chunks);
criterion_main!(benches);
