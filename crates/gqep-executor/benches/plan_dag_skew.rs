//! 偏斜 DAG 基准 — PlanRunner 在「层宽均匀」与「层内长尾」两种形状下的 makespan
//!
//! # 为什么需要这个基准（R12 W0）
//!
//! 既有 `toolplan_layers` 只测**单层**（`readonly8_ms2` / `undeclared8_ms2` / `mixed7r1w_ms2`），
//! 单层没有层间 barrier，因此它**结构上测不出**「一层一个慢节点会把后续所有层按住」这一损失。
//! 未合并分支 ADR-233 追加节一的实测也印证了这点：均匀 16 节点下改成「依赖就绪即派发」
//! 只从 31.39ms 降到 31.25ms（≈0.4%），收益全在偏斜场景，而偏斜场景此前无任何基准。
//!
//! # 形状定义
//!
//! 节点 `n{l}_{i}`（层 l ∈ [0, D)，列 i ∈ [0, W)），边 `n{l-1}_{i} → n{l}_{i}`。
//! 因此每节点深度 = l（最长路径），共 D 层、每层 W 个节点，同层互无依赖。
//! 偏斜 = 每层第 0 列节点的服务时间放大 `skew` 倍。
//!
//! 理论墙钟（`Layered` 语义，并发上界 c）：
//! - 均匀：`D × ceil(W / c) × delay`
//! - 偏斜：`D × max(skew × delay, ceil((W-1) / c) × delay)` —— 长尾节点主导时 = `D × skew × delay`
//!
//! 后续 W4 引入「依赖就绪即派发」后，本基准的同一形状应出现 `skew` 越大、降幅越大的单调关系；
//! 均匀形状则**允许 ≈0 收益**（如实报告，不美化）。
//!
//! # 运行
//!
//! ```powershell
//! cargo bench -p gqep-executor --bench plan_dag_skew
//! cargo bench -p gqep-executor --bench plan_dag_skew -- skew4x   # 只跑偏斜组
//! ```
//!
//! # 注册态
//!
//! 本基准当前为 **dev-only**（登记见 `scripts/bench_inventory_freeze.txt`，理由与 ADR 号同行）。
//! 不设 CI 阈值：R12 W0 阶段的目标是**取基线**，阈值须待 CI 首跑校准后再升格 `registered`
//! （与 ADR-239 D2「阈值一律先实测后登记」同口径）。
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use gqep_executor::toolplan_runner::{PlanGuards, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{PlanEdge, SideEffectDecl, ToolNode, ToolOp, ToolPlan};

/// 基础服务时间（ms）。
///
/// **为什么不是 2ms**（首跑实测的教训）：Windows 默认定时器粒度 ≈ **15.6ms**，
/// `sleep(2ms)` 实际落地为 ~15.6ms 一跳，于是 2ms / 4ms / 8ms 三种长尾**全落进同一跳**，
/// 首跑三组读数 62.236 / 62.305 / 62.439 ms 几乎无差（+0.3%）——量到的是定时器粒度，不是调度形状。
/// 取 20ms 让"均匀 20ms vs 长尾 80ms"跨出 5 跳，差值才归因于调度。
const BASE_MS: u64 = 20;

/// 固定延迟执行器；`slow_ids` 里的工具名按 `skew` 倍服务时间放大。
struct SkewExecutor {
    base_ms: u64,
    skew: u64,
    slow_ids: HashSet<String>,
}

#[async_trait]
impl ToolExecutor for SkewExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        let factor = if self.slow_ids.contains(tool_name) {
            self.skew
        } else {
            1
        };
        tokio::time::sleep(Duration::from_millis(self.base_ms * factor)).await;
        Ok(args_json.to_string())
    }

    // 与生产 BuiltinToolExecutor 同形态：自报只读 ⇒ 同层节点走并发分支
    // （否则被测的是「未知按写 ⇒ 整层串行」，量不出调度形状的差异）。
    fn capability_of(&self, _tool_name: &str) -> Option<SideEffectDecl> {
        Some(SideEffectDecl::ReadOnly)
    }

    fn declared_tools(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 构造 `depth × width` 的只读 DAG；`skew_col0 = true` 时每层第 0 列变长尾。
fn build_dag(
    prefix: &str,
    depth: usize,
    width: usize,
    skew_col0: bool,
) -> (ToolPlan, HashSet<String>) {
    let mut nodes = Vec::with_capacity(depth * width);
    let mut edges = Vec::with_capacity(depth.saturating_sub(1) * width);
    let mut slow = HashSet::new();

    for l in 0..depth {
        for i in 0..width {
            let id = format!("n{l}_{i}");
            if skew_col0 && i == 0 {
                slow.insert(id.clone());
            }
            nodes.push(ToolNode {
                id: id.clone(),
                op: ToolOp::ToolCall,
                tool_name: Some(id.clone()),
                args_json: Some(format!(r#"{{"id":"{id}"}}"#)),
                field: None,
                predicate: None,
                side_effect: Some(SideEffectDecl::ReadOnly),
            });
            if l > 0 {
                edges.push(PlanEdge {
                    from: format!("n{}_{i}", l - 1),
                    to: id,
                });
            }
        }
    }

    (
        ToolPlan {
            id: format!("bench-{prefix}"),
            nodes,
            edges,
        },
        slow,
    )
}

/// 单形状基准 — `Layered` 语义下的墙钟（并发上界取默认 8）
fn bench_shape(c: &mut Criterion, label: &str, depth: usize, width: usize, skew: u64) {
    let (plan, slow) = build_dag(label, depth, width, skew > 1);
    let executor = SkewExecutor {
        base_ms: BASE_MS,
        skew,
        slow_ids: slow,
    };
    // 步数上限必须放宽：本基准的 w32×d4 = 128 节点 > `guards::MAX_STEPS`(64)，
    // 用默认守卫会在 validate 阶段直接被 `TooManySteps` 拒（首跑实测踩中）。
    // 这不算"绕过守卫"——本基准量的是调度形状，守卫本身有独立测试。
    let guards = PlanGuards {
        max_steps: 256,
        ..PlanGuards::default()
    };
    let runner = PlanRunner::new(Box::new(executor), guards);
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("plan_dag_skew");
    group.sample_size(20);
    group.bench_function(BenchmarkId::from_parameter(label), |b| {
        b.to_async(&rt).iter(|| async {
            let summary = runner.run(&plan).await.expect("计划应校验通过");
            // 断言形状而非耗时：所有节点必须恰好执行一次
            assert_eq!(summary.nodes_executed, depth * width, "节点执行数不符");
            criterion::black_box(summary);
        });
    });
    group.finish();
}

/// 均匀 vs 偏斜对照 —— 同一 (depth, width) 下只改长尾倍数，
/// 差值即「层间 barrier 把长尾放大到整个后代」的代价。
fn bench_uniform_vs_skew(c: &mut Criterion) {
    // 宽 8 / 深 4：宽 = 并发上界，均匀时每层恰好一波，长尾会独占整层
    bench_shape(c, "uniform_w8_d4_ms20", 4, 8, 1);
    bench_shape(c, "skew2x_w8_d4_ms20", 4, 8, 2);
    bench_shape(c, "skew4x_w8_d4_ms20", 4, 8, 4);

    // 宽 32 / 深 4：层宽 > 上界，均匀时每层 4 波；偏斜时长尾与其余波次可部分重叠
    bench_shape(c, "uniform_w32_d4_ms20", 4, 32, 1);
    bench_shape(c, "skew4x_w32_d4_ms20", 4, 32, 4);
}

criterion_group!(benches, bench_uniform_vs_skew);
criterion_main!(benches);
