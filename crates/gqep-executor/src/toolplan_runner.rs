//! PlanRunner — 声明式 ToolPlan 解释执行（P3-T8，v4.0 WI-16）
//!
//! 对应架构层: **L7 Execution**（gqep-executor，ADR-151 裁决：挂既有 crate 增强）
//! 对应任务: **P3-T8**（手册 W17，WI-16：ToolPlan DAG + 计划期冲突拒绝）
//!
//! # 设计（v4.0 WI-16 规格）
//! - 解释执行 L0 [`ToolPlan`]（有界 DAG:tool_call/map/filter/aggregate/limit/sort）;
//! - 中间结果驻留执行环境,仅聚合结果回填（批处理往返降一个量级）;
//! - [`PlanGuards`] 硬约束:步数 ≤64 / 单计划超时 / 回填预算 / 只读模式;
//! - **冲突拒绝**:非法计划（环/超步数/悬空边）100% 拒绝（校验即拒,退化逐次调用）;
//! - **安全不变量**:每个 ToolCall 子节点经注入的 [`ToolExecutor`] 执行——接入方
//!   在 executor 内挂 execpolicy 审批/沙箱/超时/审计完整流水线。
//!
//! # 数据流
//! ```text
//! ToolPlan(1 次模型往返) → topo 序执行(M 子调用 0 往返) → PlanSummary 回填(1 次)
//! ```

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use nexus_contracts::tool_plan::{guards, PlanError, SideEffectDecl, ToolNode, ToolOp, ToolPlan};

/// 工具执行器 — 接入方注入（execpolicy 审批/沙箱/超时/审计流水线挂载点）
///
/// WHY trait 注入而非内置执行:工具执行依赖 L4 审批与沙箱（seccore）,
/// gqep 保持执行编排层职责,安全流水线由接入方在实现内装配（WI-16 安全不变量）。
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// 执行单个工具调用
    ///
    /// # 参数
    /// - `tool_name`:工具名
    /// - `args_json`:参数 JSON（原样透传）
    ///
    /// # 返回
    /// 执行结果字符串（JSON 形态;失败返回 Err,由调用方按节点错误处理）
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String>;
}

/// 计划守卫 — WI-16 硬约束（可覆盖默认值）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanGuards {
    /// 步数上限（默认 64）
    pub max_steps: usize,
    /// 单计划超时（ms,默认 30s）
    pub timeout_ms: u64,
    /// 回填预算（字节,默认 64KB）
    pub budget_bytes: usize,
    /// 只读模式（true = 拒绝 Write 节点——Plan 模式投影）
    pub readonly_only: bool,
}

impl Default for PlanGuards {
    fn default() -> Self {
        Self {
            max_steps: guards::MAX_STEPS,
            timeout_ms: guards::MAX_TIMEOUT_MS,
            budget_bytes: guards::MAX_BUDGET_BYTES,
            readonly_only: false,
        }
    }
}

/// 计划执行结果 — 聚合回填（模型视图）
#[derive(Debug, Clone, PartialEq)]
pub struct PlanSummary {
    /// 计划 ID
    pub plan_id: String,
    /// 聚合结果（回填预算内截断）
    pub summary: String,
    /// 工具调用数（0 往返语义:子调用不占模型往返）
    pub tool_calls: usize,
    /// 节点执行数
    pub nodes_executed: usize,
    /// 总耗时（ms）
    pub duration_ms: u64,
    /// 截断标记（回填预算超限）
    pub truncated: bool,
}

/// 计划运行器 — topo 序解释执行 DAG
pub struct PlanRunner {
    /// 工具执行器（注入）
    executor: Box<dyn ToolExecutor>,
    /// 守卫
    guards: PlanGuards,
}

impl std::fmt::Debug for PlanRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Box<dyn ToolExecutor> 无 Debug:trait 对象边界（与 AppServer 同模式）
        f.debug_struct("PlanRunner")
            .field("guards", &self.guards)
            .finish_non_exhaustive()
    }
}

impl PlanRunner {
    /// 新建运行器
    #[must_use]
    pub fn new(executor: Box<dyn ToolExecutor>, guards: PlanGuards) -> Self {
        Self { executor, guards }
    }

    /// 执行计划 — 校验即拒（冲突计划 100% 拒绝）,分层并行解释执行
    ///
    /// # 执行模型（WI-16 门禁:无冲突层内全并行）
    /// DAG 按节点深度分层（Kahn 最长路径）;同层节点互无依赖（DAG 性质）,
    /// 经 `FuturesUnordered` 并行执行;层间串行（数据依赖）。
    ///
    /// # 返回
    /// - `Ok(PlanSummary)`:聚合结果回填（预算内截断）;
    /// - `Err(PlanError)`:校验失败（环/超步数/悬空边/不完整调用）——调用方
    ///   据此退化逐次调用（WI-16 回退路径）。
    pub async fn run(&self, plan: &ToolPlan) -> Result<PlanSummary, PlanError> {
        let started = Instant::now();
        plan.validate(self.guards.max_steps)?;
        // 只读模式:拒绝 Write 节点（Plan 模式投影）
        if self.guards.readonly_only {
            for n in &plan.nodes {
                if n.side_effect == Some(SideEffectDecl::Write) {
                    return Err(PlanError::IncompleteToolCall(n.id.clone()));
                }
            }
        }
        // 分层（深度 = 最长路径;同层无依赖 → 可并行）
        let layers = compute_layers(plan)?;
        // 执行环境:节点 ID → 结果（中间结果驻留,不占模型上下文）
        let mut env: HashMap<String, String> = HashMap::new();
        let mut tool_calls = 0usize;
        let mut nodes_executed = 0usize;
        let mut truncated = false;

        for layer in &layers {
            // 单计划超时熔断（WI-16 守卫）
            if started.elapsed() > Duration::from_millis(self.guards.timeout_ms) {
                truncated = true;
                break;
            }
            // 同层并行（FuturesUnordered,§4.1 规范）
            let mut tasks = FuturesUnordered::new();
            for node in layer {
                let input = self.inputs_of(plan, &env, &node.id);
                let node_ref = node.clone();
                tasks.push(async move {
                    let out = self.exec_node(&node_ref, &input).await;
                    (node_ref, out)
                });
            }
            while let Some((node, out)) = tasks.next().await {
                if node.op == ToolOp::ToolCall {
                    tool_calls += 1;
                }
                nodes_executed += 1;
                env.insert(node.id.clone(), out);
                // 回填预算守卫:环境累积超限即截断
                if env.values().map(String::len).sum::<usize>() > self.guards.budget_bytes {
                    truncated = true;
                    break;
                }
            }
            if truncated {
                break;
            }
        }

        // 聚合回填:末节点结果（或 join 所有输出;简化:取最后一个执行节点）
        let summary = match plan.nodes.last() {
            Some(last) => env.get(&last.id).cloned().unwrap_or_else(|| "{}".into()),
            None => "{}".into(),
        };
        Ok(PlanSummary {
            plan_id: plan.id.clone(),
            summary,
            tool_calls,
            nodes_executed,
            duration_ms: started.elapsed().as_millis() as u64,
            truncated,
        })
    }

    /// 执行单个节点（数据算子 / 工具调用）
    async fn exec_node(&self, node: &ToolNode, input: &str) -> String {
        match node.op {
            ToolOp::ToolCall => {
                let tool = node.tool_name.as_deref().expect("validate 已保证");
                let args = node.args_json.as_deref().expect("validate 已保证");
                // 工具失败不 panic:错误文本作为节点结果（调用方审计）
                match self.executor.execute(tool, args).await {
                    Ok(s) => s,
                    Err(e) => format!(r#"{{"error":{e:?}}}"#),
                }
            }
            ToolOp::Map => {
                let field = node.field.as_deref().unwrap_or("");
                map_field(input, field)
            }
            ToolOp::Filter => {
                let pred = node.predicate.as_deref().unwrap_or("");
                filter_by_predicate(input, pred)
            }
            ToolOp::Aggregate => {
                let field = node.field.as_deref().unwrap_or("");
                aggregate_field(input, field)
            }
            ToolOp::Limit => limit_input(input),
            ToolOp::Sort => sort_by_field(input, node.field.as_deref().unwrap_or("")),
        }
    }

    /// 节点输入 — 首节点用 "[]" 空数组,后续节点取前驱结果 join
    fn inputs_of(&self, plan: &ToolPlan, env: &HashMap<String, String>, node_id: &str) -> String {
        let preds: Vec<&str> = plan
            .edges
            .iter()
            .filter(|e| e.to == node_id)
            .map(|e| e.from.as_str())
            .collect();
        if preds.is_empty() {
            return "[]".to_string();
        }
        let parts: Vec<String> = preds.iter().filter_map(|p| env.get(*p).cloned()).collect();
        if parts.is_empty() {
            "[]".to_string()
        } else {
            format!("[{}]", parts.join(","))
        }
    }
}

/// 分层 — 每节点深度 = 最长路径长度（Kahn 扩展）;同层节点互无依赖
///
/// 返回按深度升序的层列表（层内顺序任意——并行执行无依赖）。
/// 实现:按「入度清零轮次」分层（Kahn 每轮弹出入度=0 的节点 = 一层）;
/// 若放置节点数 < 总数 → 环（validate 已保证无环,防御性校验）。
fn compute_layers(plan: &ToolPlan) -> Result<Vec<Vec<ToolNode>>, PlanError> {
    let mut indegree: HashMap<&str, usize> =
        plan.nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &plan.edges {
        *indegree.entry(e.to.as_str()).or_insert(0) += 1;
        adj.entry(e.from.as_str()).or_default().push(e.to.as_str());
    }
    let mut layer_nodes: Vec<Vec<ToolNode>> = Vec::new();
    let mut frontier: VecDeque<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| *n)
        .collect();
    let mut placed = 0usize;
    while !frontier.is_empty() {
        let mut next = VecDeque::new();
        let mut layer = Vec::new();
        while let Some(n) = frontier.pop_front() {
            let node = plan
                .nodes
                .iter()
                .find(|x| x.id == n)
                .ok_or_else(|| PlanError::UnknownNode(n.into()))?
                .clone();
            layer.push(node);
            placed += 1;
            if let Some(nexts) = adj.get(n) {
                for m in nexts {
                    let d = indegree
                        .get_mut(m)
                        .ok_or_else(|| PlanError::UnknownNode((*m).into()))?;
                    *d -= 1;
                    if *d == 0 {
                        next.push_back(*m);
                    }
                }
            }
        }
        layer_nodes.push(layer);
        frontier = next;
    }
    if placed != plan.nodes.len() {
        return Err(PlanError::Cycle);
    }
    Ok(layer_nodes)
}

/// Map — 提取 field 字段（简化:对 JSON 数组元素做字段抽取;非 JSON 原样透传）
fn map_field(input: &str, field: &str) -> String {
    if field.is_empty() {
        return input.to_string();
    }
    // 简化实现:按逗号分隔顶层元素,提取 "field":value 模式（DSL 有界性保证）
    let items: Vec<&str> = input
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let extracted: Vec<String> = items
        .iter()
        .filter_map(|item| extract_field(item, field))
        .collect();
    format!("[{}]", extracted.join(","))
}

/// 提取 JSON 对象的字段值（简化:查找 `"field":value` 模式）
fn extract_field(item: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\":");
    let pos = item.find(&needle)?;
    let rest = &item[pos + needle.len()..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// Filter — 谓词过滤（简化:`field==value` 字面量相等）
fn filter_by_predicate(input: &str, predicate: &str) -> String {
    if predicate.is_empty() {
        return input.to_string();
    }
    let (field, value) = predicate
        .split_once("==")
        .map(|(f, v)| (f.trim(), v.trim()))
        .unwrap_or(("", ""));
    let items: Vec<&str> = input
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let kept: Vec<&str> = items
        .iter()
        .filter(|item| {
            extract_field(item, field)
                .map(|v| v.trim_matches('"') == value.trim_matches('"'))
                .unwrap_or(false)
        })
        .copied()
        .collect();
    format!("[{}]", kept.join(","))
}

/// Aggregate — 字段求和/计数（field 前缀 `sum:` = 求和,`count:` = 计数,默认求和）
fn aggregate_field(input: &str, field: &str) -> String {
    let (op, field_name) = field.split_once(':').unwrap_or(("sum", field));
    let items: Vec<&str> = input
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    match op {
        "count" => format!("{}", items.len()),
        _ => {
            // 求和:提取数值字段
            let total: f64 = items
                .iter()
                .filter_map(|item| extract_field(item, field_name))
                .filter_map(|v| v.parse::<f64>().ok())
                .sum();
            format!("{total}")
        }
    }
}

/// Limit — 截断（简化:取前 10 项;有界 DSL）
fn limit_input(input: &str) -> String {
    let items: Vec<&str> = input
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(10)
        .collect();
    format!("[{}]", items.join(","))
}

/// Sort — 按字段降序（简化:数值/字典序,稳定）
fn sort_by_field(input: &str, field: &str) -> String {
    let mut items: Vec<&str> = input
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    items.sort_by(|a, b| {
        let va = extract_field(a, field).unwrap_or_default();
        let vb = extract_field(b, field).unwrap_or_default();
        // 数值优先,否则字典序（降序）
        match (va.parse::<f64>(), vb.parse::<f64>()) {
            (Ok(x), Ok(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
            _ => vb.cmp(&va),
        }
    });
    format!("[{}]", items.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::tool_plan::PlanEdge;

    /// 内存工具执行器（测试用;零 IO,无沙箱依赖）
    struct MemExecutor;

    #[async_trait]
    impl ToolExecutor for MemExecutor {
        async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
            // 回显工具:产出 JSON 数组元素（模拟检索结果）
            match tool_name {
                "search:docs" => Ok(
                    r#"[{"title":"a","score":3},{"title":"b","score":5},{"title":"c","score":1}]"#
                        .to_string(),
                ),
                "echo" => Ok(args_json.to_string()),
                _ => Err(format!("unknown tool: {tool_name}")),
            }
        }
    }

    fn runner() -> PlanRunner {
        PlanRunner::new(Box::new(MemExecutor), PlanGuards::default())
    }

    fn node(id: &str, op: ToolOp) -> ToolNode {
        ToolNode {
            id: id.into(),
            op,
            tool_name: None,
            args_json: None,
            field: None,
            predicate: None,
            side_effect: None,
        }
    }

    /// 端到端 — ToolCall → Sort → Limit → Map 链式执行
    #[tokio::test]
    async fn full_pipeline_executes() {
        let mut map = node("map1", ToolOp::Map);
        map.field = Some("title".into());
        let plan = ToolPlan {
            id: "plan-1".into(),
            nodes: vec![
                ToolNode {
                    id: "fetch".into(),
                    op: ToolOp::ToolCall,
                    tool_name: Some("search:docs".into()),
                    args_json: Some(r#"{"q":"rust"}"#.into()),
                    field: None,
                    predicate: None,
                    side_effect: Some(SideEffectDecl::ReadOnly),
                },
                node("sort1", ToolOp::Sort),
                node("limit1", ToolOp::Limit),
                map,
            ],
            edges: vec![
                PlanEdge {
                    from: "fetch".into(),
                    to: "sort1".into(),
                },
                PlanEdge {
                    from: "sort1".into(),
                    to: "limit1".into(),
                },
                PlanEdge {
                    from: "limit1".into(),
                    to: "map1".into(),
                },
            ],
        };
        let summary = runner().run(&plan).await.expect("计划必须执行成功");
        assert_eq!(summary.tool_calls, 1, "1 次工具调用（0 模型往返）");
        assert_eq!(summary.nodes_executed, 4);
        // sort 降序:score 5,3,1 → limit 前 10 → map title
        assert!(
            summary.summary.contains("b"),
            "最高分 b 必须保留: {}",
            summary.summary
        );
        assert!(summary.summary.contains("a"));
        assert!(!summary.truncated);
    }

    /// 冲突拒绝 — 环计划 100% 拒绝（W17 门禁）
    #[tokio::test]
    async fn cycle_rejected() {
        let plan = ToolPlan {
            id: "plan-cycle".into(),
            nodes: vec![
                ToolNode {
                    id: "a".into(),
                    op: ToolOp::ToolCall,
                    tool_name: Some("echo".into()),
                    args_json: Some("{}".into()),
                    field: None,
                    predicate: None,
                    side_effect: Some(SideEffectDecl::ReadOnly),
                },
                node("b", ToolOp::Map),
            ],
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
        assert_eq!(
            runner().run(&plan).await,
            Err(PlanError::Cycle),
            "环必须 100% 拒绝"
        );
    }

    /// 冲突拒绝 — 超步数计划拒绝
    #[tokio::test]
    async fn max_steps_rejected() {
        let nodes: Vec<ToolNode> = (0..10)
            .map(|i| node(&format!("n{i}"), ToolOp::Map))
            .collect();
        let plan = ToolPlan {
            id: "plan-steps".into(),
            nodes,
            edges: vec![],
        };
        let g = PlanGuards {
            max_steps: 5,
            ..PlanGuards::default()
        };
        let r = PlanRunner::new(Box::new(MemExecutor), g);
        assert_eq!(r.run(&plan).await, Err(PlanError::TooManySteps(10)));
    }

    /// 只读模式 — Write 节点拒绝（Plan 模式投影）
    #[tokio::test]
    async fn readonly_mode_rejects_write() {
        let plan = ToolPlan {
            id: "plan-write".into(),
            nodes: vec![ToolNode {
                id: "w".into(),
                op: ToolOp::ToolCall,
                tool_name: Some("edit:file".into()),
                args_json: Some(r#"{"f":"a.txt"}"#.into()),
                field: None,
                predicate: None,
                side_effect: Some(SideEffectDecl::Write),
            }],
            edges: vec![],
        };
        let g = PlanGuards {
            readonly_only: true,
            ..PlanGuards::default()
        };
        let r = PlanRunner::new(Box::new(MemExecutor), g);
        assert!(r.run(&plan).await.is_err(), "只读模式必须拒绝 Write 节点");
    }

    /// 工具失败 — 错误结果入环境,不 panic（调用方审计）
    #[tokio::test]
    async fn tool_failure_is_graceful() {
        let plan = ToolPlan {
            id: "plan-fail".into(),
            nodes: vec![ToolNode {
                id: "f".into(),
                op: ToolOp::ToolCall,
                tool_name: Some("ghost:tool".into()),
                args_json: Some("{}".into()),
                field: None,
                predicate: None,
                side_effect: Some(SideEffectDecl::ReadOnly),
            }],
            edges: vec![],
        };
        let summary = runner().run(&plan).await.expect("计划不因工具失败而失败");
        assert_eq!(summary.tool_calls, 1);
        assert!(
            summary.summary.contains("error"),
            "错误结果必须可见: {}",
            summary.summary
        );
    }

    /// 数据算子 — Filter / Aggregate 独立行为
    #[tokio::test]
    async fn filter_and_aggregate_ops() {
        // Filter
        let filtered = filter_by_predicate(r#"[{"t":"a","v":1},{"t":"b","v":2}]"#, "t==b");
        assert!(filtered.contains("b"));
        assert!(!filtered.contains("\"a\""));
        // Aggregate sum
        let summed = aggregate_field(r#"[{"v":1},{"v":2},{"v":3}]"#, "v");
        assert_eq!(summed, "6");
        // Aggregate count
        let counted = aggregate_field(r#"[{"v":1},{"v":2}]"#, "count:");
        assert_eq!(counted, "2");
    }

    /// 回填预算 — 超限截断标记
    #[tokio::test]
    async fn budget_truncation() {
        let plan = ToolPlan {
            id: "plan-budget".into(),
            nodes: vec![ToolNode {
                id: "big".into(),
                op: ToolOp::ToolCall,
                tool_name: Some("echo".into()),
                args_json: Some(r#"{"big":"x"}"#.into()),
                field: None,
                predicate: None,
                side_effect: Some(SideEffectDecl::ReadOnly),
            }],
            edges: vec![],
        };
        let g = PlanGuards {
            budget_bytes: 8,
            ..PlanGuards::default()
        };
        let r = PlanRunner::new(Box::new(MemExecutor), g);
        let summary = r.run(&plan).await.expect("执行成功");
        assert!(summary.truncated, "超预算必须截断标记");
    }

    /// 同层并行 — 无依赖节点并行执行（WI-16 门禁:无冲突层内全并行）
    ///
    /// 两个 ToolCall 各 sleep 100ms,并行下总耗时 < 180ms（串行需 200ms）
    #[tokio::test]
    async fn same_layer_parallel() {
        struct SlowExec;
        #[async_trait]
        impl ToolExecutor for SlowExec {
            async fn execute(&self, _tool: &str, _args: &str) -> Result<String, String> {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok("{}".into())
            }
        }
        // 两个无依赖的 ToolCall（同层）→ 并行
        let plan = ToolPlan {
            id: "plan-parallel".into(),
            nodes: vec![
                ToolNode {
                    id: "t1".into(),
                    op: ToolOp::ToolCall,
                    tool_name: Some("a".into()),
                    args_json: Some("{}".into()),
                    field: None,
                    predicate: None,
                    side_effect: Some(SideEffectDecl::ReadOnly),
                },
                ToolNode {
                    id: "t2".into(),
                    op: ToolOp::ToolCall,
                    tool_name: Some("b".into()),
                    args_json: Some("{}".into()),
                    field: None,
                    predicate: None,
                    side_effect: Some(SideEffectDecl::ReadOnly),
                },
            ],
            edges: vec![],
        };
        let r = PlanRunner::new(Box::new(SlowExec), PlanGuards::default());
        let started = std::time::Instant::now();
        let summary = r.run(&plan).await.expect("执行成功");
        let elapsed = started.elapsed();
        assert_eq!(summary.tool_calls, 2);
        assert!(
            elapsed < Duration::from_millis(180),
            "同层必须并行(串行需 200ms,实测 {}ms)",
            elapsed.as_millis()
        );
    }

    /// 超时熔断 — 慢计划截断返回
    #[tokio::test]
    async fn timeout_fuse() {
        struct SlowExecutor;
        #[async_trait]
        impl ToolExecutor for SlowExecutor {
            async fn execute(&self, _tool: &str, _args: &str) -> Result<String, String> {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok("{}".into())
            }
        }
        let plan = ToolPlan {
            id: "plan-slow".into(),
            nodes: vec![
                ToolNode {
                    id: "s1".into(),
                    op: ToolOp::ToolCall,
                    tool_name: Some("t".into()),
                    args_json: Some("{}".into()),
                    field: None,
                    predicate: None,
                    side_effect: Some(SideEffectDecl::ReadOnly),
                },
                node("s2", ToolOp::Map),
            ],
            edges: vec![PlanEdge {
                from: "s1".into(),
                to: "s2".into(),
            }],
        };
        let g = PlanGuards {
            timeout_ms: 50,
            ..PlanGuards::default()
        };
        let r = PlanRunner::new(Box::new(SlowExecutor), g);
        let summary = r.run(&plan).await.expect("执行成功");
        assert!(summary.truncated, "超时必须截断返回");
    }
}
