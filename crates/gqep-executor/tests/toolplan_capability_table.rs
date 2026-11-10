//! X3 能力表派生 `side_effect` 的行为契约（ADR-254）。
//!
//! 要锁的语义只有三条，且每条都对应一个「今天做不到」的可观测事实：
//! 1. 计划**未声明** `side_effect` 时，若执行器自报该工具只读 ⇒ 层内**必须真并行**
//!    （今天恒整层串行 —— `toolplan_layers/undeclared8_ms2/{1,8}` 实测 69.8 vs 63.8 ms）;
//! 2. 计划**显式声明 `Write`** 时，执行器说是只读也不许并行
//!    （计划作者意图 > 实现自述 —— 否则「声明」这一契约被实现方悄悄推翻）;
//! 3. 执行器无法回答（`None`）时保守串行（fail-closed，未注册工具不被误判为可并行）。
//!
//! 判据用**同时在飞节点数峰值**（L2 运行时计数器），不用墙钟比值：
//! 墙钟受宿主抖动影响，峰值是结构性证据，且与
//! `tests/toolplan_concurrency_proptest.rs` 既有四条不变量同口径 ——
//! 那四条断言在本批必须原样绿，这是「不削减既有语义」的可执行证明。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::toolplan_runner::{PlanGuards, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{SideEffectDecl, ToolNode, ToolOp, ToolPlan};

/// 记录「同时在飞节点数峰值」的执行器
///
/// `declared` 决定它对自己所服务的工具名自报的副作用属性 —— 即 ADR-254 的
/// 「执行器能力表」。默认实现（`None`）路径由 `SilentExecutor` 覆盖。
struct ProbeExecutor {
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    /// 本执行器自报只读的工具名集合；不在集合内 ⇒ 自报 `None`（未知）
    readonly_names: Vec<String>,
    /// 是否覆写 capability_of（false ⇒ 走 trait 默认实现，行为须与今天逐字一致）
    overrides: bool,
}

impl ProbeExecutor {
    fn new(readonly_names: Vec<String>, overrides: bool) -> Self {
        Self {
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            readonly_names,
            overrides,
        }
    }
}

#[async_trait]
impl ToolExecutor for ProbeExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        // 让并发窗口足够长，使「峰值 > 1」不是调度噪声
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(args_json.to_string())
    }

    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        if !self.overrides {
            return None;
        }
        if self.readonly_names.iter().any(|n| n == tool_name) {
            Some(SideEffectDecl::ReadOnly)
        } else {
            None
        }
    }

    fn declared_tools(&self) -> Vec<String> {
        if self.overrides {
            self.readonly_names.clone()
        } else {
            Vec::new()
        }
    }
}

/// n 个同层 ToolCall 节点，`side_effect` 由调用方决定
fn plan(n: usize, side_effect: Option<SideEffectDecl>) -> ToolPlan {
    let nodes = (0..n)
        .map(|i| ToolNode {
            id: format!("t{i}"),
            op: ToolOp::ToolCall,
            tool_name: Some(format!("probe.tool.{i}")),
            args_json: Some(format!(r#"{{"i":{i}}}"#)),
            field: None,
            predicate: None,
            side_effect,
        })
        .collect();
    ToolPlan {
        id: format!("capability-probe-{n}-{side_effect:?}"),
        nodes,
        edges: Vec::new(),
    }
}

fn runner(exec: ProbeExecutor, concurrency: usize) -> (PlanRunner, Arc<AtomicUsize>) {
    let peak = Arc::clone(&exec.peak);
    (
        PlanRunner::new(Box::new(exec), PlanGuards::default()).with_concurrency(concurrency),
        peak,
    )
}

fn all_tool_names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("probe.tool.{i}")).collect()
}

/// 判据 1：未声明 + 执行器自报只读 ⇒ 真并行
#[tokio::test]
async fn capability_table_parallelizes_undeclared_readonly() {
    let n = 8;
    let (r, peak) = runner(ProbeExecutor::new(all_tool_names(n), true), 8);
    let summary = r.run(&plan(n, None)).await.expect("计划必须执行成功");
    assert_eq!(summary.tool_calls, n);
    assert_eq!(summary.nodes_executed, n);
    assert!(
        !summary.truncated,
        "20ms×8 在 30s 预算内不应触发截断: {:?}",
        summary.duration_ms
    );
    assert!(
        peak.load(Ordering::SeqCst) > 1,
        "能力表已声明只读，层内仍整层串行（峰值 {}）",
        peak.load(Ordering::SeqCst)
    );
}

/// 判据 2：显式 `Write` 压过执行器的只读自报
///
/// WHY 这条比判据 1 更重要:它锁住「计划作者意图 > 实现自述」。
/// 若反过来让实现说了算，接入方就能把写操作伪装成只读来换取并发 ——
/// 层内写序不可复现，`toolplan_runner.rs` 注释所述的审计契约即被击穿。
#[tokio::test]
async fn explicit_write_overrides_capability_table() {
    let n = 8;
    let (r, peak) = runner(ProbeExecutor::new(all_tool_names(n), true), 8);
    let summary = r
        .run(&plan(n, Some(SideEffectDecl::Write)))
        .await
        .expect("计划必须执行成功");
    assert_eq!(summary.tool_calls, n);
    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "显式声明 Write 却发生并发 ⇒ 写序不再由计划决定"
    );
}

/// 判据 3：执行器不回答（trait 默认实现）⇒ 保守串行，且行为与改造前逐字一致
#[tokio::test]
async fn unknown_capability_stays_serial_fail_closed() {
    let n = 8;
    let (r, peak) = runner(ProbeExecutor::new(Vec::new(), false), 8);
    r.run(&plan(n, None)).await.expect("计划必须执行成功");
    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "未注册工具必须按「可能写」保守串行"
    );
}

/// 判据 3b（ADR-281 修订）：只**被覆盖的只读段**并行；未覆盖节点是写屏障
///
/// 旧契约「层内存在未知节点 ⇒ 整层串行」已被 ADR-281 收窄为「未知节点是屏障」：
/// 屏障**之前/之间**的只读段并发，屏障节点单独串行，读写不并发。
/// 本夹具 5 个 covered（t0..t4）在前、3 个未知（t5..t7）在后 ⇒ 只读段 peak = 5。
/// 判别力：整层串行 ⇒ 1；无屏障整层并发 ⇒ 8；只有正确分段才会是 5。
/// 屏障的顺序/不重叠语义由 `toolplan_concurrency_proptest.rs` 的混层不变量 5/6 锁定。
#[tokio::test]
async fn partial_capability_only_parallelizes_covered_names() {
    let n = 8;
    // 只把 t0..t4 报为只读，t5..t7 未知 ⇒ 前 5 个构成只读段，后 3 个是屏障
    let covered = vec![
        "probe.tool.0".to_string(),
        "probe.tool.1".to_string(),
        "probe.tool.2".to_string(),
        "probe.tool.3".to_string(),
        "probe.tool.4".to_string(),
    ];
    let (r, peak) = runner(ProbeExecutor::new(covered, true), 8);
    r.run(&plan(n, None)).await.expect("计划必须执行成功");
    assert_eq!(
        peak.load(Ordering::SeqCst),
        5,
        "只读段（5 个 covered）应并发到 5；未知节点不得加入该段（整层串行=1，全并发=8）"
    );
}

/// 判据 4（F-A21）：只读投影必须认得「未声明但实为写」的工具
///
/// 今天 `readonly_only` 只拒 `Some(Write)`，省略字段即可绕过只读模式 ——
/// 与「Plan 模式投影拒绝写」的既定语义相悖。接入能力表后应堵住该绕过。
#[tokio::test]
async fn readonly_projection_uses_capability_table() {
    let n = 2;
    // 执行器对这些工具**不声明只读**（视为未知/可能写）
    let exec = ProbeExecutor::new(Vec::new(), true);
    let guards = PlanGuards {
        readonly_only: true,
        ..PlanGuards::default()
    };
    let r = PlanRunner::new(Box::new(exec), guards);
    let res = r.run(&plan(n, None)).await;
    assert!(
        res.is_err(),
        "只读模式必须拒绝「未声明且执行器未报只读」的节点，实得 {res:?}"
    );
}
