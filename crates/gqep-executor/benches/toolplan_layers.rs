//! PlanRunner 只读层「有界并发」基准 — criterion
//!
//! 场景:8 个只读 ToolCall 同层（无依赖）,mock executor 每次 `sleep(2ms)`;
//! 对比 `concurrency = 1`（等价串行）与 `concurrency = 8`（有界并行）。
//!
//! 理论墙钟:串行 ≈ 8 × 2ms = 16ms;有界并行 ≈ 2ms + 调度开销。
//!
//! 两组值组的分工(ADR-252):
//! - `readonly8_ms2/*` — 计划**显式声明** `side_effect: "read_only"`,走并发分支,
//!   是「上限」对照组。
//! - `undeclared8_ms2/*` — 计划**不声明** `side_effect`(字段缺省 `None`)。
//!   `effective_side_effect` 把「未声明」按「可能写」保守处理 ⇒ 今天 c1 与 c8
//!   读数应几乎相等(整层串行)。这组是 F-A5 的**代价测量锚**:它把
//!   「缺省语义导致并行分支在生产形同不存在」变成一个可对比的数字,
//!   而不是停在争论上。接入执行器能力表(ADR-254)后 c8 才应向
//!   `readonly8_ms2/8` 靠拢 —— 阈值收紧即该改动的验收证据。
//!
//! ADR-281 新增 `mixed7r1w_ms2/*`：7 个只读 + 末尾 1 个显式 Write ⇒ 写屏障分段。
//! c=1 ≈ 8×延迟；c=8 ≈ 读段 + 写 = 2×延迟；实测 c8/c1 = 5.82×（CI 保守 4.59×）。
//! 该组只量收益；写屏障的顺序/不重叠语义由 `toolplan_concurrency_proptest.rs`
//! 的混层不变量 5/6 锁定。
//!
//! 运行:
//! ```powershell
//! cargo bench -p gqep-executor --bench toolplan_layers
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::time::Duration;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use gqep_executor::toolplan_runner::{PlanGuards, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{SideEffectDecl, ToolNode, ToolOp, ToolPlan};

/// 固定延迟执行器 — 模拟真实只读工具 I/O（零副作用）
struct SleepExecutor {
    delay_ms: u64,
}

#[async_trait]
impl ToolExecutor for SleepExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(args_json.to_string())
    }

    // X2-N1 前置（ADR-254 D1）：执行器自报只读能力（与生产 BuiltinToolExecutor
    // 同形态，对齐 tests/toolplan_capability_table.rs:60 夹具先例）。WHY：生产
    // 执行器自报只读而 bench 的 SleepExecutor 一直沉默 ⇒ undeclared8_ms2 组
    // 此前测的是「执行器也不声明」的双缺省形态，非生产默认路径；补声明后
    // undeclared8_ms2/8 才是 ADR-252 D3 预言的「≈readonly8/8」读数，×2.5
    // 收紧才有依据（deepdive §5.3 原文规格）。
    fn capability_of(&self, _tool_name: &str) -> Option<SideEffectDecl> {
        Some(SideEffectDecl::ReadOnly)
    }

    // ADR-254 D4：declared_tools 供路由表建表；bench 不经 RouteLayer，
    // 但实现它保持「能执行 ⇔ 已声明」的契约一致性（默认空集 = 撒谎）。
    fn declared_tools(&self) -> Vec<String> {
        (0..8).map(|i| format!("bench.readonly.{i}")).collect()
    }
}

/// n 个只读 ToolCall 同层（无依赖 → 同一层）
///
/// `declared` 控制每个节点是否写 `side_effect: Some(ReadOnly)`：
/// `false` 时留 `None`，即「未声明」形态，供 `undeclared8_ms2/*` 值组使用。
fn readonly_layer_plan(n: usize, declared: bool) -> ToolPlan {
    let nodes: Vec<ToolNode> = (0..n)
        .map(|i| ToolNode {
            id: format!("t{i}"),
            op: ToolOp::ToolCall,
            tool_name: Some(format!("bench.readonly.{i}")),
            args_json: Some(format!(r#"{{"i":{i}}}"#)),
            field: None,
            predicate: None,
            side_effect: if declared {
                Some(SideEffectDecl::ReadOnly)
            } else {
                None
            },
        })
        .collect();
    ToolPlan {
        id: format!("bench-readonly-layer-{n}-{declared}"),
        nodes,
        edges: Vec::new(),
    }
}

/// 7 个只读 + 1 个显式 Write（写节点放在声明序末位）同层
///
/// WHY 写放末位：只读段 `[r0..r6]` 一次性并发，随后写节点单独执行
/// ⇒ c=8 理论墙钟 = 2ms（读段）+ 2ms（写）= 4ms，c=1 = 16ms，比值 4×。
/// 写放中间会把层切成 3+4 两段读段，理论比值只有 ~2.7×，无法验证 ≥4× 门。
fn mixed_layer_plan(readonly_n: usize) -> ToolPlan {
    let mut nodes: Vec<ToolNode> = (0..readonly_n)
        .map(|i| ToolNode {
            id: format!("r{i}"),
            op: ToolOp::ToolCall,
            tool_name: Some(format!("bench.readonly.{i}")),
            args_json: Some(format!(r#"{{"i":{i}}}"#)),
            field: None,
            predicate: None,
            side_effect: Some(SideEffectDecl::ReadOnly),
        })
        .collect();
    nodes.push(ToolNode {
        id: "w".to_string(),
        op: ToolOp::ToolCall,
        tool_name: Some("bench.write".to_string()),
        args_json: Some(r#"{"i":99}"#.to_string()),
        field: None,
        predicate: None,
        side_effect: Some(SideEffectDecl::Write),
    });
    ToolPlan {
        id: format!("bench-mixed-layer-{readonly_n}r1w"),
        nodes,
        edges: Vec::new(),
    }
}

fn bench_toolplan_layers(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    // (值组前缀, 是否显式声明 read_only)。两组共用同一 executor 与同一形状，
    // 唯一差异是 `side_effect` 字段 ⇒ 差值即「缺省保守串行」的代价。
    let groups = [("readonly8_ms2", true), ("undeclared8_ms2", false)];

    let mut group = c.benchmark_group("toolplan_layers");
    group.sample_size(20);

    for (label, declared) in groups {
        let plan = readonly_layer_plan(8, declared);
        for concurrency in [1usize, 8] {
            let runner = PlanRunner::new(
                Box::new(SleepExecutor { delay_ms: 2 }),
                PlanGuards::default(),
            )
            .with_concurrency(concurrency);
            group.bench_with_input(
                BenchmarkId::new(label, concurrency),
                &concurrency,
                |b, _| {
                    b.to_async(&rt).iter(|| async {
                        let summary = runner.run(&plan).await.expect("基准计划必须执行成功");
                        assert_eq!(summary.tool_calls, 8);
                        assert!(!summary.truncated);
                    });
                },
            );
        }
    }

    // ADR-281：混层写屏障。7 个只读 + 末尾 1 个显式 Write。
    // 理论墙钟：c=1 ≈ 8×2ms=16ms；c=8 ≈ 只读段 2ms + 写 2ms = 4ms ⇒ 4×。
    // 若实现退化为整层串行，两组读数相等；若读写并发，写屏障语义被破坏
    // （由 toolplan_concurrency_proptest 的混层不变量锁定，本组只量收益）。
    let mixed_plan = mixed_layer_plan(7);
    for concurrency in [1usize, 8] {
        let runner = PlanRunner::new(
            Box::new(SleepExecutor { delay_ms: 2 }),
            PlanGuards::default(),
        )
        .with_concurrency(concurrency);
        group.bench_with_input(
            BenchmarkId::new("mixed7r1w_ms2", concurrency),
            &concurrency,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let summary = runner
                        .run(&mixed_plan)
                        .await
                        .expect("混层基准计划必须执行成功");
                    assert_eq!(summary.tool_calls, 8);
                    assert!(!summary.truncated);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_toolplan_layers);
criterion_main!(benches);
