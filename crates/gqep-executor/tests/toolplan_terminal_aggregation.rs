//! W11 / ADR-277 — 计划聚合必须取**拓扑终端**，不能取声明序末位。
//!
//! # 缺陷
//! 旧实现 `plan.nodes.last()` 取的是**声明序**末位。若末位声明节点有后继，
//! 它会先于后继执行 ⇒ 聚合把「中间节点」的输出当成计划结果（正确性缺陷）。
//! 注意：这与 W2 的截断语义是**不同问题** —— 那个是「截断不进失败」，这个是「聚合取错节点」。
//!
//! 运行：`cargo test -p gqep-executor --test toolplan_terminal_aggregation`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use async_trait::async_trait;
use gqep_executor::toolplan_runner::{PlanGuards, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{PlanEdge, SideEffectDecl, ToolNode, ToolOp, ToolPlan};

/// 回显执行器：输出带节点名，便于断言摘要来自哪个节点
struct EchoTag;

#[async_trait]
impl ToolExecutor for EchoTag {
    async fn execute(&self, tool_name: &str, _args: &str) -> Result<String, String> {
        Ok(format!("out-of:{tool_name}"))
    }
}

fn call(id: &str, tool: &str) -> ToolNode {
    ToolNode {
        id: id.into(),
        op: ToolOp::ToolCall,
        tool_name: Some(tool.into()),
        args_json: Some("{}".into()),
        field: None,
        predicate: None,
        side_effect: Some(SideEffectDecl::ReadOnly),
    }
}

/// 声明序末位（c）**有后继**（c→b）⇒ 旧实现会把 c 的输出当摘要，
/// 正确实现必须取拓扑终端 b 的输出。
#[tokio::test]
async fn plan_summary_uses_topological_terminal() {
    let plan = ToolPlan {
        id: "terminal-probe".into(),
        // 声明序：b, a, c —— 末位 c 有出边（c→b），故 c 不是终端
        nodes: vec![
            call("b", "tool.b"),
            call("a", "tool.a"),
            call("c", "tool.c"),
        ],
        edges: vec![
            PlanEdge {
                from: "a".into(),
                to: "b".into(),
            },
            PlanEdge {
                from: "c".into(),
                to: "b".into(),
            },
        ],
    };
    let runner = PlanRunner::new(Box::new(EchoTag), PlanGuards::default());
    let summary = runner.run(&plan).await.expect("计划应执行成功");
    assert_eq!(summary.nodes_executed, 3);
    assert_eq!(
        summary.summary, "out-of:tool.b",
        "聚合必须取拓扑终端 b（无出边）的输出；取到别的节点说明用的是声明序末位"
    );
}
