//! PlanRunner 层并发语义属性测试（性能增量 2026-11-06）
//!
//! # 验证的不变量（每条 32 cases）
//! 1. `readonly_dag_summary_equivalent`：只读 DAG 在 `concurrency ∈ {1,2,8,64}`
//!    下 `PlanSummary` 逐字段相等（`duration_ms` 除外）。
//! 2. `write_chain_executes_in_layer_order`：Write 链按 layer 顺序逐节点执行
//!    （串行、确定），与并发度无关。
//! 3. `write_same_layer_is_fully_serial`：同层多个「可能写」节点整层串行——
//!    在飞峰值恒 1，且全部执行恰好一次。
//! 4. `readonly_layer_respects_concurrency_bound`：只读同层在飞峰值 ≤ 并发上界，
//!    且实际用满上界（否则「一律串行」的实现也会通过）。
//!
//! WHY：并发化只允许改变**墙钟**，不允许改变结果、顺序契约与审计口径；
//! 有副作用节点一旦并发，写入顺序由调度器决定 ⇒ 不可复现、审计链无法对齐。
//!
//! 稳定性：mock executor 只 `tokio::time::sleep(5ms)`，零真实 IO。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::toolplan_runner::{PlanGuards, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{PlanEdge, SideEffectDecl, ToolNode, ToolOp, ToolPlan};
use proptest::prelude::*;

/// 观测型执行器 — 记录在飞峰值与（进入时的）执行顺序
struct ProbeExecutor {
    /// 固定延迟（ms）
    delay_ms: u64,
    /// 当前在飞数
    in_flight: Arc<AtomicUsize>,
    /// 历史在飞峰值
    peak: Arc<AtomicUsize>,
    /// 执行顺序（进入时刻记录;工具名 = 节点 ID）
    seq: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ToolExecutor for ProbeExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.seq.lock().unwrap().push(tool_name.to_string());
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(args_json.to_string())
    }
}

/// 观测计数器组
#[derive(Clone, Default)]
struct Probe {
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    seq: Arc<Mutex<Vec<String>>>,
}

impl Probe {
    fn new() -> Self {
        Self::default()
    }

    /// 组装指定并发度的运行器（延迟 5ms）
    fn runner(&self, concurrency: usize) -> PlanRunner {
        PlanRunner::new(
            Box::new(ProbeExecutor {
                delay_ms: 5,
                in_flight: Arc::clone(&self.in_flight),
                peak: Arc::clone(&self.peak),
                seq: Arc::clone(&self.seq),
            }),
            PlanGuards::default(),
        )
        .with_concurrency(concurrency)
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn seq(&self) -> Vec<String> {
        self.seq.lock().unwrap().clone()
    }

    fn reset(&self) {
        self.in_flight.store(0, Ordering::SeqCst);
        self.peak.store(0, Ordering::SeqCst);
        self.seq.lock().unwrap().clear();
    }
}

/// 只读 ToolCall 节点（工具名 = 节点 ID，回显 args）
fn readonly_node(id: &str) -> ToolNode {
    ToolNode {
        id: id.to_string(),
        op: ToolOp::ToolCall,
        tool_name: Some(id.to_string()),
        args_json: Some(format!(r#"{{"id":"{id}"}}"#)),
        field: None,
        predicate: None,
        side_effect: Some(SideEffectDecl::ReadOnly),
    }
}

/// 「可能写」ToolCall 节点（`declared_write` 控制显式 Write / 未声明 None）
fn maybe_write_node(id: &str, declared_write: bool) -> ToolNode {
    ToolNode {
        id: id.to_string(),
        op: ToolOp::ToolCall,
        tool_name: Some(id.to_string()),
        args_json: Some(format!(r#"{{"id":"{id}"}}"#)),
        field: None,
        predicate: None,
        side_effect: if declared_write {
            Some(SideEffectDecl::Write)
        } else {
            None
        },
    }
}

/// 只读 DAG：`deps[i]` 为真表示节点 t{i} 依赖 t{i-1}
fn readonly_dag(n: usize, deps: &[bool]) -> ToolPlan {
    let nodes = (0..n).map(|i| readonly_node(&format!("t{i}"))).collect();
    let edges = (1..n)
        .filter(|i| deps.get(*i).copied().unwrap_or(false))
        .map(|i| PlanEdge {
            from: format!("t{}", i - 1),
            to: format!("t{i}"),
        })
        .collect();
    ToolPlan {
        id: format!("ro-dag-{n}"),
        nodes,
        edges,
    }
}

/// 「可能写」线性链：t0 → t1 → ... → t{n-1}（每层一个节点）
fn write_chain(n: usize, declared_write: bool) -> ToolPlan {
    let nodes = (0..n)
        .map(|i| maybe_write_node(&format!("t{i}"), declared_write))
        .collect();
    let edges = (1..n)
        .map(|i| PlanEdge {
            from: format!("t{}", i - 1),
            to: format!("t{i}"),
        })
        .collect();
    ToolPlan {
        id: format!("write-chain-{n}"),
        nodes,
        edges,
    }
}

/// 「可能写」同层节点（无依赖 → 同一层）
fn write_same_layer(n: usize, declared_write: bool) -> ToolPlan {
    let nodes = (0..n)
        .map(|i| maybe_write_node(&format!("t{i}"), declared_write))
        .collect();
    ToolPlan {
        id: format!("write-flat-{n}"),
        nodes,
        edges: Vec::new(),
    }
}

fn new_runtime() -> Result<tokio::runtime::Runtime, TestCaseError> {
    tokio::runtime::Runtime::new().map_err(|e| TestCaseError::fail(e.to_string()))
}

fn fail_on_plan_error(e: nexus_contracts::tool_plan::PlanError) -> TestCaseError {
    TestCaseError::fail(format!("计划必须执行成功: {e}"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// 不变量 1：只读 DAG 的 PlanSummary 与并发度无关（duration_ms 除外）
    #[test]
    fn readonly_dag_summary_equivalent(
        n in 1usize..=6usize,
        deps in proptest::collection::vec(any::<bool>(), 6),
    ) {
        let plan = readonly_dag(n, &deps);
        let rt = new_runtime()?;

        let probe = Probe::new();
        let base = rt
            .block_on(probe.runner(1).run(&plan))
            .map_err(fail_on_plan_error)?;

        for concurrency in [2usize, 8, 64] {
            probe.reset();
            let got = rt
                .block_on(probe.runner(concurrency).run(&plan))
                .map_err(fail_on_plan_error)?;
            prop_assert_eq!(got.plan_id.as_str(), base.plan_id.as_str());
            prop_assert_eq!(got.summary.as_str(), base.summary.as_str());
            prop_assert_eq!(got.tool_calls, base.tool_calls);
            prop_assert_eq!(got.nodes_executed, base.nodes_executed);
            prop_assert_eq!(got.truncated, base.truncated);
        }

        // 语义不变：n 个 ToolCall 恰好执行 n 次，未触发预算截断
        prop_assert_eq!(base.tool_calls, n);
        prop_assert_eq!(base.nodes_executed, n);
        prop_assert!(!base.truncated);
    }

    /// 不变量 2：Write 链按 layer 顺序执行（串行、确定），与并发度无关
    #[test]
    fn write_chain_executes_in_layer_order(
        n in 1usize..=6usize,
        declared_write in any::<bool>(),
    ) {
        let plan = write_chain(n, declared_write);
        let rt = new_runtime()?;
        let expected: Vec<String> = (0..n).map(|i| format!("t{i}")).collect();

        for concurrency in [1usize, 8, 64] {
            let probe = Probe::new();
            let summary = rt
                .block_on(probe.runner(concurrency).run(&plan))
                .map_err(fail_on_plan_error)?;
            prop_assert_eq!(summary.tool_calls, n);
            prop_assert_eq!(summary.nodes_executed, n);
            prop_assert_eq!(
                probe.seq(),
                expected.clone(),
                "Write 链必须按 layer 顺序逐节点执行（并发度 = {}）",
                concurrency
            );
            prop_assert_eq!(
                probe.peak(),
                1,
                "Write 层在飞峰值必须恒为 1（并发度 = {}）",
                concurrency
            );
        }
    }

    /// 不变量 3：同层多个「可能写」节点整层串行（显式 Write 与未声明 None 同待遇）
    #[test]
    fn write_same_layer_is_fully_serial(
        n in 1usize..=6usize,
        declared_write in any::<bool>(),
    ) {
        let plan = write_same_layer(n, declared_write);
        let rt = new_runtime()?;

        for concurrency in [2usize, 8, 64] {
            let probe = Probe::new();
            let summary = rt
                .block_on(probe.runner(concurrency).run(&plan))
                .map_err(fail_on_plan_error)?;
            prop_assert_eq!(summary.tool_calls, n);
            prop_assert_eq!(summary.nodes_executed, n);
            prop_assert_eq!(
                probe.peak(),
                1,
                "有副作用层整层串行：在飞峰值必须为 1（显式 Write = {}）",
                declared_write
            );
            let mut observed = probe.seq();
            observed.sort();
            let mut expected: Vec<String> = (0..n).map(|i| format!("t{i}")).collect();
            expected.sort();
            prop_assert_eq!(observed, expected, "每个节点必须执行且仅执行一次");
        }
    }

    /// 不变量 4：只读同层的在飞峰值 ≤ 并发上界，且实际用满上界
    #[test]
    fn readonly_layer_respects_concurrency_bound(
        n in 1usize..=8usize,
        concurrency in prop_oneof![Just(1usize), Just(2usize), Just(8usize), Just(64usize)],
    ) {
        let plan = readonly_dag(n, &[false; 6]);
        let rt = new_runtime()?;
        let probe = Probe::new();

        let summary = rt
            .block_on(probe.runner(concurrency).run(&plan))
            .map_err(fail_on_plan_error)?;
        prop_assert_eq!(summary.tool_calls, n);

        let peak = probe.peak();
        prop_assert!(
            peak <= concurrency,
            "在飞峰值 {} 不得突破并发上界 {}",
            peak,
            concurrency
        );
        let expected_peak = n.min(concurrency);
        prop_assert_eq!(
            peak, expected_peak,
            "只读层应实际用满上界（n = {}, concurrency = {}）",
            n, concurrency
        );
        prop_assert_eq!(probe.seq().len(), n, "每个节点必须执行一次");
    }
}

// ============================================================
// 混层写屏障（ADR-281）— 确定性 #[tokio::test]（非 proptest）
// ============================================================

/// 扁平混层计划：(id, 是否只读)，无 edges ⇒ 全部落在同一层
fn flat_mixed_plan(spec: &[(&str, bool)]) -> ToolPlan {
    let nodes = spec
        .iter()
        .map(|(id, read)| {
            if *read {
                readonly_node(id)
            } else {
                maybe_write_node(id, true)
            }
        })
        .collect();
    ToolPlan {
        id: "mixed-flat".into(),
        nodes,
        edges: Vec::new(),
    }
}

/// 不变量 5：写节点是读段的屏障 —— 写之前的读段并发完成，写之后才跑下一段
#[tokio::test]
async fn mixed_layer_write_is_a_barrier_for_read_segments() {
    let plan = flat_mixed_plan(&[
        ("r0", true),
        ("r1", true),
        ("w", false),
        ("r2", true),
        ("r3", true),
    ]);
    let probe = Probe::new();
    let summary = probe.runner(4).run(&plan).await.expect("混层计划应成功");

    assert_eq!(summary.tool_calls, 5);
    assert_eq!(summary.nodes_executed, 5);
    let seq = probe.seq();
    assert_eq!(seq.len(), 5, "每个节点必须执行一次");
    assert_eq!(seq[2], "w", "写节点必须在两段只读之间，实际 {seq:?}");
    let mut before = seq[..2].to_vec();
    before.sort();
    assert_eq!(before, vec!["r0".to_string(), "r1".to_string()]);
    let mut after = seq[3..].to_vec();
    after.sort();
    assert_eq!(after, vec!["r2".to_string(), "r3".to_string()]);
    assert!(probe.peak() >= 2, "读段内应并发，peak={}", probe.peak());
    assert!(probe.peak() <= 4, "不得突破并发上界，peak={}", probe.peak());
}

/// 不变量 6：同层多个写节点按声明序执行，且被读段隔开（读写不并发）
#[tokio::test]
async fn mixed_layer_two_writes_keep_declaration_order() {
    let plan = flat_mixed_plan(&[("w0", false), ("r0", true), ("w1", false)]);
    let probe = Probe::new();
    probe.runner(8).run(&plan).await.expect("混层计划应成功");
    assert_eq!(
        probe.seq(),
        vec!["w0".to_string(), "r0".to_string(), "w1".to_string()],
        "写节点必须按声明序、且被读段隔开"
    );
    assert_eq!(probe.peak(), 1, "读写不得并发");
}

/// 不变量 7：混层计划在 c=1 与 c=8 下 PlanSummary 逐字段相等（duration_ms 除外）
#[tokio::test]
async fn mixed_layer_summary_equivalent_across_concurrency() {
    let plan = flat_mixed_plan(&[
        ("r0", true),
        ("r1", true),
        ("w", false),
        ("r2", true),
        ("r3", true),
    ]);
    let base = Probe::new().runner(1).run(&plan).await.expect("c=1 应成功");
    let conc = Probe::new().runner(8).run(&plan).await.expect("c=8 应成功");
    assert_eq!(base.plan_id, conc.plan_id);
    assert_eq!(base.summary, conc.summary);
    assert_eq!(base.tool_calls, conc.tool_calls);
    assert_eq!(base.nodes_executed, conc.nodes_executed);
    assert_eq!(base.truncated, conc.truncated);
}

/// 不变量 8：同层多写跨运行稳定（防 `HashMap` 迭代序回潮）
#[tokio::test]
async fn same_layer_writes_are_stable_across_runs() {
    let plan = flat_mixed_plan(&[("w2", false), ("w0", false), ("w1", false)]);
    let expected = vec!["w2".to_string(), "w0".to_string(), "w1".to_string()];
    for _ in 0..5 {
        let probe = Probe::new();
        probe.runner(8).run(&plan).await.expect("全写层应成功");
        assert_eq!(probe.seq(), expected, "同层写序必须按声明序稳定");
    }
}
