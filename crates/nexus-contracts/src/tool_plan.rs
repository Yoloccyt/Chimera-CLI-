//! ToolPlan 契约 — 声明式工具计划批编排（P3-T8，v4.0 WI-16）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts，ADR-151 裁决：L0 schema 先行）
//! 对应任务: **P3-T8**（手册 W17，WI-16：ToolPlan DSL + PlanRunner + PlanGuards）
//!
//! # 设计（v4.0 WI-16 规格）
//! 不嵌 JS 引擎（V8/QuickJS 依赖重、安全面大，违 forbid(unsafe) 精神），
//! 改为**声明式 ToolPlan DSL（JSON）**：模型输出有界 DAG
//! （tool_call/map/filter/aggregate/limit/sort），PlanRunner 在 gqep-executor 内
//! 解释执行，中间结果驻留执行环境，仅聚合结果回填。
//!
//! # 安全不变量
//! - 计划内每个 tool_call 子节点仍走 execpolicy 审批/沙箱/超时/审计完整流水线
//!   （由 gqep PlanRunner 接入侧保证）;
//! - [`PlanGuards`] 硬约束（只读白名单/步数 ≤64/单计划超时/回填预算）;
//! - 副作用节点逐条确认（ToolCall 含 side_effect 声明）。
//!
//! # 退化路径
//! 计划校验失败即拒，自动退化逐次调用（调用方语义）。

use serde::{Deserialize, Serialize};

/// 工具计划 — 有界 DAG（节点 + 边）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolPlan {
    /// 计划 ID
    pub id: String,
    /// 节点（tool_call / map / filter / aggregate / limit / sort）
    pub nodes: Vec<ToolNode>,
    /// 依赖边（from → to 数据流）
    pub edges: Vec<PlanEdge>,
}

/// 计划节点
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolNode {
    /// 节点 ID（计划内唯一）
    pub id: String,
    /// 算子
    pub op: ToolOp,
    /// 工具名（ToolCall 必填）
    #[serde(default)]
    pub tool_name: Option<String>,
    /// 参数 JSON（ToolCall 必填）
    #[serde(default)]
    pub args_json: Option<String>,
    /// 字段路径（Map/Filter/Aggregate 操作字段;简化:点分路径）
    #[serde(default)]
    pub field: Option<String>,
    /// 过滤谓词（Filter;简化:字面量相等比较 `field==value`）
    #[serde(default)]
    pub predicate: Option<String>,
    /// 副作用声明（ToolCall;Write 节点需逐条确认）
    #[serde(default)]
    pub side_effect: Option<SideEffectDecl>,
}

/// 工具调用副作用声明 — 与 streaming_dispatch 分类对齐
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectDecl {
    /// 只读（查询/检索类）
    ReadOnly,
    /// 写/副作用（需逐条确认）
    Write,
}

/// 计划算子 — 有界 DAG 五种数据算子 + 工具调用
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOp {
    /// 工具调用（叶子/中间节点,经完整审批流水线）
    ToolCall,
    /// 逐元素映射（field 路径取值）
    Map,
    /// 过滤（predicate 谓词）
    Filter,
    /// 聚合（field 求和/计数;简化:sum/count 由 field 前缀表达）
    Aggregate,
    /// 截断（limit 数量）
    Limit,
    /// 排序（field 降序）
    Sort,
}

/// 计划边 — from → to 数据流
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEdge {
    /// 源节点
    pub from: String,
    /// 目标节点
    pub to: String,
}

/// 计划校验错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// 节点 ID 重复
    DuplicateNodeId(String),
    /// 边引用了不存在的节点
    UnknownNode(String),
    /// 图含环（非 DAG）
    Cycle,
    /// 步数超限
    TooManySteps(usize),
    /// ToolCall 缺 tool_name/args_json
    IncompleteToolCall(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::DuplicateNodeId(id) => write!(f, "duplicate node id: {id}"),
            PlanError::UnknownNode(id) => write!(f, "unknown node in edge: {id}"),
            PlanError::Cycle => write!(f, "plan graph contains a cycle"),
            PlanError::TooManySteps(n) => write!(f, "plan exceeds max steps: {n}"),
            PlanError::IncompleteToolCall(id) => write!(f, "tool call node incomplete: {id}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// 计划守卫常量 — WI-16 硬约束
pub mod guards {
    /// 单计划最大步数（节点数）
    pub const MAX_STEPS: usize = 64;
    /// 单计划最大执行时长（ms）
    pub const MAX_TIMEOUT_MS: u64 = 30_000;
    /// 单计划回填预算（字节;超限截断聚合结果）
    pub const MAX_BUDGET_BYTES: usize = 64 * 1024;
}

impl ToolPlan {
    /// 校验计划 — DAG 无环 + 节点唯一 + 步数上限 + ToolCall 完整性
    ///
    /// # 参数
    /// - `max_steps`:步数上限（默认 [`guards::MAX_STEPS`];调用方可按场景收紧）
    pub fn validate(&self, max_steps: usize) -> Result<(), PlanError> {
        // 1. 节点唯一
        let mut seen = std::collections::HashSet::new();
        for n in &self.nodes {
            if !seen.insert(n.id.as_str()) {
                return Err(PlanError::DuplicateNodeId(n.id.clone()));
            }
        }
        // 2. 步数上限（有界 DAG）
        if self.nodes.len() > max_steps {
            return Err(PlanError::TooManySteps(self.nodes.len()));
        }
        // 3. 边引用完整性
        for e in &self.edges {
            if !seen.contains(e.from.as_str()) || !seen.contains(e.to.as_str()) {
                return Err(PlanError::UnknownNode(format!("{}->{}", e.from, e.to)));
            }
        }
        // 4. DAG 无环（Kahn 拓扑排序）
        if has_cycle(&self.nodes, &self.edges) {
            return Err(PlanError::Cycle);
        }
        // 5. ToolCall 完整性
        for n in &self.nodes {
            if n.op == ToolOp::ToolCall && (n.tool_name.is_none() || n.args_json.is_none()) {
                return Err(PlanError::IncompleteToolCall(n.id.clone()));
            }
        }
        Ok(())
    }
}

/// Kahn 拓扑排序判环（O(V+E);边表小,零依赖）
fn has_cycle(nodes: &[ToolNode], edges: &[PlanEdge]) -> bool {
    use std::collections::{HashMap, VecDeque};
    let mut indegree: HashMap<&str, usize> = nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in edges {
        *indegree.entry(e.to.as_str()).or_insert(0) += 1;
        adj.entry(e.from.as_str()).or_default().push(e.to.as_str());
    }
    let mut queue: VecDeque<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| *n)
        .collect();
    let mut visited = 0usize;
    while let Some(n) = queue.pop_front() {
        visited += 1;
        if let Some(nexts) = adj.get(n) {
            for m in nexts {
                let d = indegree.get_mut(m).expect("边引用已校验");
                *d -= 1;
                if *d == 0 {
                    queue.push_back(m);
                }
            }
        }
    }
    visited < nodes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call_node(id: &str, tool: &str, args: &str) -> ToolNode {
        ToolNode {
            id: id.into(),
            op: ToolOp::ToolCall,
            tool_name: Some(tool.into()),
            args_json: Some(args.into()),
            field: None,
            predicate: None,
            side_effect: Some(SideEffectDecl::ReadOnly),
        }
    }

    /// 合法 DAG — 通过校验
    #[test]
    fn valid_dag_passes() {
        let plan = ToolPlan {
            id: "p1".into(),
            nodes: vec![
                call_node("fetch", "search:docs", r#"{"q":"rust"}"#),
                ToolNode {
                    id: "map1".into(),
                    op: ToolOp::Map,
                    tool_name: None,
                    args_json: None,
                    field: Some("title".into()),
                    predicate: None,
                    side_effect: None,
                },
                ToolNode {
                    id: "limit1".into(),
                    op: ToolOp::Limit,
                    tool_name: None,
                    args_json: None,
                    field: None,
                    predicate: None,
                    side_effect: None,
                },
            ],
            edges: vec![
                PlanEdge {
                    from: "fetch".into(),
                    to: "map1".into(),
                },
                PlanEdge {
                    from: "map1".into(),
                    to: "limit1".into(),
                },
            ],
        };
        assert_eq!(plan.validate(guards::MAX_STEPS), Ok(()));
    }

    /// 环检测 — 自环与多节点环均拒绝
    #[test]
    fn cycle_rejected() {
        let plan = ToolPlan {
            id: "p2".into(),
            nodes: vec![call_node("a", "t", "{}"), call_node("b", "t", "{}")],
            edges: vec![
                PlanEdge {
                    from: "a".into(),
                    to: "b".into(),
                },
                PlanEdge {
                    from: "b".into(),
                    to: "a".into(),
                },
            ],
        };
        assert_eq!(plan.validate(guards::MAX_STEPS), Err(PlanError::Cycle));
        // 自环
        let self_loop = ToolPlan {
            id: "p3".into(),
            nodes: vec![call_node("a", "t", "{}")],
            edges: vec![PlanEdge {
                from: "a".into(),
                to: "a".into(),
            }],
        };
        assert_eq!(self_loop.validate(guards::MAX_STEPS), Err(PlanError::Cycle));
    }

    /// 步数上限 — 超限拒绝（有界 DAG）
    #[test]
    fn max_steps_enforced() {
        let nodes: Vec<ToolNode> = (0..10)
            .map(|i| call_node(&format!("n{i}"), "t", "{}"))
            .collect();
        let plan = ToolPlan {
            id: "p4".into(),
            nodes,
            edges: vec![],
        };
        assert_eq!(plan.validate(5), Err(PlanError::TooManySteps(10)));
        assert_eq!(plan.validate(10), Ok(()));
    }

    /// ToolCall 完整性 — 缺 tool_name/args 拒绝
    #[test]
    fn incomplete_tool_call_rejected() {
        let plan = ToolPlan {
            id: "p5".into(),
            nodes: vec![ToolNode {
                id: "x".into(),
                op: ToolOp::ToolCall,
                tool_name: None,
                args_json: None,
                field: None,
                predicate: None,
                side_effect: None,
            }],
            edges: vec![],
        };
        assert_eq!(
            plan.validate(guards::MAX_STEPS),
            Err(PlanError::IncompleteToolCall("x".into()))
        );
    }

    /// 重复节点 / 悬空边 — 拒绝
    #[test]
    fn duplicate_and_dangling_rejected() {
        let plan = ToolPlan {
            id: "p6".into(),
            nodes: vec![call_node("a", "t", "{}"), call_node("a", "t", "{}")],
            edges: vec![],
        };
        assert_eq!(
            plan.validate(guards::MAX_STEPS),
            Err(PlanError::DuplicateNodeId("a".into()))
        );
        let dangling = ToolPlan {
            id: "p7".into(),
            nodes: vec![call_node("a", "t", "{}")],
            edges: vec![PlanEdge {
                from: "a".into(),
                to: "ghost".into(),
            }],
        };
        assert_eq!(
            dangling.validate(guards::MAX_STEPS),
            Err(PlanError::UnknownNode("a->ghost".into()))
        );
    }

    /// 序列化往返 — ToolPlan serde JSON 可编解码（模型输出 DSL）
    #[test]
    fn serde_roundtrip() {
        let plan = ToolPlan {
            id: "p8".into(),
            nodes: vec![call_node("fetch", "search:docs", r#"{"q":"rust"}"#)],
            edges: vec![],
        };
        let json = serde_json::to_string(&plan).expect("编码成功");
        let back: ToolPlan = serde_json::from_str(&json).expect("解码成功");
        assert_eq!(back, plan);
    }
}
