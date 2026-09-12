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

use chimera_cli::orchestrator::{build_quest_reply, plan_chunks, plan_chunks_batched};
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

/// H-a 批聚合对照:同一回复在大/中/小批下的 chunk 生产成本与**chunk 数**。
///
/// WHY 此对照:批聚合(H-a 事件率治理)的收益不是"分块更快",而是**事件数下降**
/// —— 下游每条 `TuiChatResponseChunk` 都要经 `broadcast` 对每个订阅者深拷贝一份
/// 事件体,再各自过 TUI 侧同步器链。故本组用 `Throughput::Elements(字符数)` 统一
/// 分母(同一回复的字符数固定),使各组耗时可直接横向比较"每条回复的总分块成本";
/// `BenchmarkId` 同时打印该批大小下的 chunk 数,作为事件数下降倍数的可证伪证据。
/// 生产默认批大小为 [`DEFAULT_CHUNK_BATCH_CHARS`](chimera_cli::orchestrator::DEFAULT_CHUNK_BATCH_CHARS)=8。
fn bench_chunk_batching(c: &mut Criterion) {
    let mut group = c.benchmark_group("quest_chunk_batching");
    let quest = quest_with_tasks(16); // 最大代表性规模(max_tasks_per_quest=16)
    let reply = build_quest_reply(&quest);
    let char_count = reply.chars().count() as u64;
    for batch in [1usize, 4, 8, 12] {
        let chunks = plan_chunks_batched(&reply, batch).len();
        let id = BenchmarkId::new(format!("batch{batch}_chunks{chunks}"), char_count);
        group.throughput(Throughput::Elements(char_count));
        group.bench_with_input(id, &reply, |b, r| {
            b.iter(|| {
                let batched = plan_chunks_batched(black_box(r), batch);
                black_box(batched);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_plan_chunks, bench_chunk_batching);
criterion_main!(benches);
