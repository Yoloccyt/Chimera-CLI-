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
use tokio_util::sync::CancellationToken;

/// 计划级执行上下文 —— 一次 `run`/`run_with` 内恒定不变的部分。
///
/// WHY 不是把 5 个参数逐个传给执行器:上下文属于**同一次运行**，逐参数透传会让
/// trait 签名随每个新需求增长；用一个借用结构体，新增字段不破坏既有实现。
#[derive(Debug, Clone, Copy)]
pub struct PlanExecCtx<'a> {
    /// 计划 ID（用于构造可追溯的 call_id）
    pub plan_id: &'a str,
    /// 运行选项里的显式写放行位（`PlanRunOptions::approve_writes`）
    pub approve_writes: bool,
    /// 调用方的取消令牌（`None` = 无外部取消，执行器自决）
    pub cancel: Option<&'a CancellationToken>,
}

/// 节点级执行上下文 —— 执行器要据此判断"该不该已获批准"。
#[derive(Debug, Clone, Copy)]
pub struct NodeExecCtx<'a> {
    /// 所属计划的上下文
    pub plan: PlanExecCtx<'a>,
    /// 节点 ID（计划内唯一）
    pub node_id: &'a str,
    /// 节点自己声明的副作用（`None` = 未声明，按不需要批准处理）
    pub side_effect: Option<SideEffectDecl>,
}

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

    /// 带节点上下文的执行入口。
    ///
    /// 默认实现**丢弃上下文**并退回 [`execute`](Self::execute)，因此 legacy 执行器
    /// （测试夹具与旧的无上下文调用方）行为逐位不变；只有需要把 `approved` /
    /// `call_id` / 取消令牌装配进真实调用的适配器才覆盖它。
    ///
    /// WHY 默认实现而不是直接改 `execute` 签名:`execute` 有 10 个实现（含全部
    /// 测试夹具），扩签名会把一次编排层修复扩散成 10 处无关改动。
    async fn execute_node(
        &self,
        _ctx: &NodeExecCtx<'_>,
        tool_name: &str,
        args_json: &str,
    ) -> Result<String, String> {
        self.execute(tool_name, args_json).await
    }
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
    /// 在飞节点上限（默认 8；1 = 串行）——`run_with` 的派发窗口，`run` 的层内窗口
    pub max_parallel: usize,
}

impl Default for PlanGuards {
    fn default() -> Self {
        Self {
            max_steps: guards::MAX_STEPS,
            timeout_ms: guards::MAX_TIMEOUT_MS,
            budget_bytes: guards::MAX_BUDGET_BYTES,
            readonly_only: false,
            max_parallel: 8,
        }
    }
}

/// 新执行入口的运行选项（与旧 `run` 解耦，便于渐进接线）。
#[derive(Debug, Clone, Default)]
pub struct PlanRunOptions {
    /// 在飞节点上限（0 = 使用 `PlanGuards::max_parallel`；最终钳到 ≥1）
    pub max_parallel: usize,
    /// 计划级绝对 deadline（ms；0 = 使用 `PlanGuards::timeout_ms`）
    pub timeout_ms: u64,
    /// true = 任一节点失败立即返回 `NodeFailed`；false = 错误作为节点结果继续（旧语义）
    pub fail_fast: bool,
    /// true = 放行 Write 节点（仅在 `PlanGuards::readonly_only=true` 时有意义：
    /// 只读投影下 Write 默认被拒，本旗是显式覆盖；`false` 保持 fail-closed）
    pub approve_writes: bool,
    /// 外部取消令牌
    pub cancel: Option<CancellationToken>,
}

/// `run_with` 的失败类型。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PlanRunError {
    /// 计划校验失败（环/超步数/悬空边/不完整调用）。
    #[error("invalid plan: {0}")]
    Invalid(PlanError),
    /// 计划级 deadline 到期。
    #[error("plan timed out after {completed} completed nodes")]
    TimedOut {
        /// 已完成节点数。
        completed: usize,
    },
    /// 外部取消。
    #[error("plan cancelled after {completed} completed nodes")]
    Cancelled {
        /// 已完成节点数。
        completed: usize,
    },
    /// `fail_fast=true` 时某节点失败。
    #[error("node {node_id} failed after {completed} completed nodes: {error}")]
    NodeFailed {
        /// 失败节点 ID。
        node_id: String,
        /// 失败原因（纯文本）。
        error: String,
        /// 失败前已完成的节点数。
        completed: usize,
    },
}

impl From<PlanError> for PlanRunError {
    fn from(value: PlanError) -> Self {
        Self::Invalid(value)
    }
}

/// 内部节点失败分类（不暴露给调用方）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeFailure {
    Failed(String),
    Timeout,
    Cancelled,
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
                // 旧 `run` 没有运行选项：显式写成"不放行写、无外部取消"，
                // 与它此前的隐式行为逐位一致（那时执行器根本看不到这两个字段）。
                let plan_id = plan.id.clone();
                tasks.push(async move {
                    let plan_ctx = PlanExecCtx {
                        plan_id: &plan_id,
                        approve_writes: false,
                        cancel: None,
                    };
                    // 旧 run 保持 fail-soft：工具错误包装为 {"error":...} 节点结果。
                    let out = match self.exec_node(&node_ref, &input, plan_ctx).await {
                        Ok(value) => value,
                        Err(error) => format!(r#"{{"error":{error:?}}}"#),
                    };
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

    /// 新执行入口：依赖就绪即派发 + 绝对 deadline + 取消 + fail-fast + 增量预算。
    ///
    /// 与旧 [`run`](Self::run) 的差异：
    /// - 并发单位是**在飞节点窗口**（`max_parallel`），不是层：节点一旦全部前驱提交
    ///   即可派发，不再等同层无关慢节点（旧层屏障见 ADR-233 W1a 一节）；
    /// - 超时是**绝对 deadline**（`timeout_at`），不是每层重置；
    /// - 支持 `CancellationToken`；
    /// - 预算按增量累加（旧 `run` 仍是 O(n²) 全量求和，保持旧行为不变）；
    /// - 多终端节点返回 `{"terminals":{...}}`；单终端保持旧语义。
    ///
    /// 提交序列是 DAG 偏序的一条线性化（提交只发生在 [`commit`](Self::commit)），
    /// 因此线性化不变式可被 [`inputs_of_precomputed`] 的 debug 断言当场检查。
    pub async fn run_with(
        &self,
        plan: &ToolPlan,
        opts: &PlanRunOptions,
    ) -> Result<PlanSummary, PlanRunError> {
        let started = Instant::now();
        plan.validate(self.guards.max_steps)?;

        // Write 节点默认由 readonly_only 拒绝；approve_writes 是显式覆盖。
        if self.guards.readonly_only && !opts.approve_writes {
            for node in &plan.nodes {
                if node.side_effect == Some(SideEffectDecl::Write) {
                    return Err(PlanRunError::Invalid(PlanError::IncompleteToolCall(
                        node.id.clone(),
                    )));
                }
            }
        }

        // 一次性建图（O(V+E)）：调度只看依赖是否就绪，不再分批于"层"。
        let mut graph = build_graph(plan)?;
        let preds = predecessor_map(plan);
        let timeout_ms = if opts.timeout_ms == 0 {
            self.guards.timeout_ms
        } else {
            opts.timeout_ms
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let max_parallel = if opts.max_parallel == 0 {
            self.guards.max_parallel
        } else {
            opts.max_parallel
        }
        .max(1);
        let cancel = opts.cancel.as_ref();

        let mut env: HashMap<String, String> = HashMap::new();
        let mut counters = RunCounters::default();
        // 每次运行的计划级上下文：执行器据此构造可追溯、可取消、审批语义显式的调用。
        let plan_ctx = PlanExecCtx {
            plan_id: &plan.id,
            approve_writes: opts.approve_writes,
            cancel,
        };

        let mut in_flight: FuturesUnordered<_> = FuturesUnordered::new();
        loop {
            // 取消/deadline 的判定点从"层顶"移到"派发顶"：每提交一个节点就重新判一次，
            // 返回的 completed 计数语义不变（已提交数）。
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(PlanRunError::Cancelled {
                    completed: counters.nodes_executed,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(PlanRunError::TimedOut {
                    completed: counters.nodes_executed,
                });
            }

            // 窗口未满就把已就绪的节点发出去——不再等整层。
            while in_flight.len() < max_parallel {
                let Some(pos) = graph.ready.pop_front() else {
                    break;
                };
                let node = &plan.nodes[pos];
                let input = inputs_of_precomputed(&preds, &env, &node.id);
                in_flight.push(self.node_future(node, input, deadline, cancel, plan_ctx));
            }

            // 排空即提交：提交只发生在这个单所有者循环里，所以提交序列是 DAG 偏序的一条线性化。
            let Some((node, result)) = in_flight.next().await else {
                break;
            };
            self.commit(
                &mut graph,
                &mut env,
                &mut counters,
                node,
                result,
                opts.fail_fast,
            )?;
            if counters.halted {
                break;
            }
        }

        Ok(PlanSummary {
            plan_id: plan.id.clone(),
            summary: terminal_summary(plan, &env),
            tool_calls: counters.tool_calls,
            nodes_executed: counters.nodes_executed,
            duration_ms: started.elapsed().as_millis() as u64,
            truncated: counters.truncated,
        })
    }

    /// 提交一次节点完成 —— 计数、预算、环境、后继就绪四件事都只在这里发生。
    ///
    /// WHY 单点提交:提交序列因此是 DAG 偏序的**一条线性化**，于是"派发某节点时
    /// 其全部前驱已提交"可被 [`inputs_of_precomputed`] 的 debug 断言当场验证；
    /// 若把提交分散到多处（如并发 join 分支），该不变式就无从检查。
    ///
    /// 返回 `Err` 表示本次运行立即终止（fail-fast 节点失败 / 超时 / 取消）；
    /// 预算耗尽不是错误，置 `counters.halted` 由调用方收敛为 `Ok`。
    fn commit(
        &self,
        graph: &mut Graph,
        env: &mut HashMap<String, String>,
        counters: &mut RunCounters,
        node: &ToolNode,
        result: Result<String, NodeFailure>,
        fail_fast: bool,
    ) -> Result<(), PlanRunError> {
        counters.nodes_executed += 1;
        if node.op == ToolOp::ToolCall {
            counters.tool_calls += 1;
        }
        let value = match result {
            Ok(value) => value,
            Err(NodeFailure::Failed(error)) if fail_fast => {
                return Err(PlanRunError::NodeFailed {
                    node_id: node.id.clone(),
                    error,
                    completed: counters.nodes_executed - 1,
                });
            }
            // 非 fail-fast：错误作为节点结果，下游可见并自行处理。
            Err(NodeFailure::Failed(error)) => format!(r#"{{"error":{error:?}}}"#),
            Err(NodeFailure::Timeout) => {
                return Err(PlanRunError::TimedOut {
                    completed: counters.nodes_executed - 1,
                })
            }
            Err(NodeFailure::Cancelled) => {
                return Err(PlanRunError::Cancelled {
                    completed: counters.nodes_executed - 1,
                })
            }
        };

        let remaining = self.guards.budget_bytes.saturating_sub(counters.used_bytes);
        let (value, cut) = truncate_to_budget(value, remaining);
        if cut {
            counters.truncated = true;
        }
        counters.used_bytes += value.len();
        env.insert(node.id.clone(), value);
        if counters.used_bytes >= self.guards.budget_bytes {
            counters.truncated = true;
            counters.halted = true;
        }
        graph.advance(&node.id);
        Ok(())
    }

    /// 单节点 future：绝对 deadline + 取消，返回节点引用与结果。
    async fn node_future<'a>(
        &'a self,
        node: &'a ToolNode,
        input: String,
        deadline: tokio::time::Instant,
        cancel: Option<&'a CancellationToken>,
        plan_ctx: PlanExecCtx<'a>,
    ) -> (&'a ToolNode, Result<String, NodeFailure>) {
        let result = tokio::select! {
            result = tokio::time::timeout_at(deadline, self.exec_node(node, &input, plan_ctx)) => result,
            _ = wait_cancel(cancel) => return (node, Err(NodeFailure::Cancelled)),
        };
        match result {
            Ok(Ok(value)) => (node, Ok(value)),
            Ok(Err(error)) => (node, Err(NodeFailure::Failed(error))),
            Err(_) => (node, Err(NodeFailure::Timeout)),
        }
    }

    /// 执行单个节点（数据算子 / 工具调用）
    ///
    /// 返回 `Err` 表示工具调用失败；旧 `run` 会把它包装成
    /// `{"error":...}` 节点结果（保持 fail-soft），新 `run_with` 由
    /// `PlanRunOptions::fail_fast` 决定是终止还是继续。
    async fn exec_node(
        &self,
        node: &ToolNode,
        input: &str,
        plan_ctx: PlanExecCtx<'_>,
    ) -> Result<String, String> {
        match node.op {
            ToolOp::ToolCall => {
                // validate 已保证二者齐备;但缺字段不再靠 expect 不变量——
                // 与下方执行失败同型 fail-soft(错误文本进节点结果，可审计不 panic)。
                let Some(tool) = node.tool_name.as_deref() else {
                    return Err("missing tool_name".to_string());
                };
                let Some(args) = node.args_json.as_deref() else {
                    return Err("missing args_json".to_string());
                };
                let ctx = NodeExecCtx {
                    plan: plan_ctx,
                    node_id: &node.id,
                    side_effect: node.side_effect,
                };
                self.executor.execute_node(&ctx, tool, args).await
            }
            ToolOp::Map => {
                let field = node.field.as_deref().unwrap_or("");
                Ok(map_field(input, field))
            }
            ToolOp::Filter => {
                let pred = node.predicate.as_deref().unwrap_or("");
                Ok(filter_by_predicate(input, pred))
            }
            ToolOp::Aggregate => {
                let field = node.field.as_deref().unwrap_or("");
                Ok(aggregate_field(input, field))
            }
            ToolOp::Limit => Ok(limit_input(input)),
            ToolOp::Sort => Ok(sort_by_field(input, node.field.as_deref().unwrap_or(""))),
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

/// 一次运行的可变计数与预算状态（[`PlanRunner::commit`] 的唯一写入面）。
#[derive(Default)]
struct RunCounters {
    tool_calls: usize,
    nodes_executed: usize,
    used_bytes: usize,
    /// 有节点结果被预算截断（或预算已耗尽）——粘滞位，进 `PlanSummary`。
    truncated: bool,
    /// 预算耗尽：与 `truncated` 分开，因为截断只标记内容而仍继续执行。
    halted: bool,
}

/// 依赖就绪调度用的图 —— 节点以 `plan.nodes` 下标索引。
///
/// WHY 下标而非 ID:层屏障版每层每节点都要 `plan.nodes.iter().find`（O(V)）并
/// `clone` 整个 `ToolNode`；这里一次性建表，之后调度只碰 `usize`。
///
/// WHY `succ`/`remaining` 按**原始边表**建立（重复边计多次）:`predecessor_map`
/// 对重复边给出重复前驱，节点输入的字节因此含重复父值；`remaining` 必须同样
/// "每条边递减一次"才能在最后一次重复递减时归零 ⇒ 输入内容与今天逐位相同。
struct Graph<'p> {
    /// 节点 ID → `plan.nodes` 下标
    idx: HashMap<&'p str, usize>,
    /// 每条出边的目标下标（按原始边表序）
    succ: Vec<Vec<usize>>,
    /// 尚未满足的入边数；归零即就绪
    remaining: Vec<usize>,
    /// 就绪队列（播种按节点声明序，推进按边表序 ⇒ 同一计划可复现）
    ready: VecDeque<usize>,
}

impl<'p> Graph<'p> {
    /// 提交一个节点后推进它的每条出边；某后继首次归零即入队。
    ///
    /// WHY 判"从非零跨到零"而不是"当前为零":`remaining` 与 `succ` 同源于原始边表，
    /// 正常情况下严格同步；一旦不同步（重复边少计一次），`saturating_sub` 会让同一
    /// 后继在每条多余出边上再次满足"为零"而**二次入队**——副作用节点被执行两遍是最
    /// 坏的一类错误，比少执行贵得多，所以入队条件带上"此前非零"。
    fn advance(&mut self, from: &str) {
        let Some(&pos) = self.idx.get(from) else {
            return;
        };
        for edge in 0..self.succ[pos].len() {
            let to = self.succ[pos][edge];
            if let Some(left) = self.remaining.get_mut(to) {
                let before = *left;
                *left = before.saturating_sub(1);
                if before > 0 && *left == 0 {
                    self.ready.push_back(to);
                }
            }
        }
    }
}

/// 一次性建图 —— O(V+E)，取代每层重算的 `compute_layers`。
///
/// 依赖 [`ToolPlan::validate`] 已排除重复 ID 与悬空边；这里仍返回
/// [`PlanError`] 而不是 `expect`：调用顺序是运行期的事实，不该用 panic 兜。
fn build_graph(plan: &ToolPlan) -> Result<Graph<'_>, PlanError> {
    let mut idx: HashMap<&str, usize> = HashMap::with_capacity(plan.nodes.len());
    for (pos, node) in plan.nodes.iter().enumerate() {
        if idx.insert(node.id.as_str(), pos).is_some() {
            return Err(PlanError::DuplicateNodeId(node.id.clone()));
        }
    }
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); plan.nodes.len()];
    let mut remaining: Vec<usize> = vec![0; plan.nodes.len()];
    for edge in &plan.edges {
        let (Some(&from), Some(&to)) = (idx.get(edge.from.as_str()), idx.get(edge.to.as_str()))
        else {
            let unknown = if idx.contains_key(edge.from.as_str()) {
                edge.to.as_str()
            } else {
                edge.from.as_str()
            };
            return Err(PlanError::UnknownNode(unknown.to_string()));
        };
        succ[from].push(to);
        remaining[to] += 1;
    }
    let ready = (0..plan.nodes.len())
        .filter(|&pos| remaining[pos] == 0)
        .collect();
    Ok(Graph {
        idx,
        succ,
        remaining,
        ready,
    })
}

/// 前驱表 — 一次构建替代 `run_with` 每个节点全边扫描。
fn predecessor_map(plan: &ToolPlan) -> HashMap<&str, Vec<&str>> {
    let mut preds: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &plan.edges {
        preds
            .entry(edge.to.as_str())
            .or_default()
            .push(edge.from.as_str());
    }
    preds
}

/// 用前驱表拼节点输入（与 `PlanRunner::inputs_of` 同语义）。
///
/// 依赖就绪即派发 ⇒ 每个前驱在派发时都已提交，故 parts 与 parents **等长**；
/// 过去的静默 `filter_map` 会把缺失的前驱吃掉，节点于是拿到一个少了某段输入的
/// 数组却毫无征兆。debug 档当场中止，release 档仍走原路径（不新增 panic 面）。
fn inputs_of_precomputed(
    preds: &HashMap<&str, Vec<&str>>,
    env: &HashMap<String, String>,
    node_id: &str,
) -> String {
    let Some(parents) = preds.get(node_id) else {
        return "[]".to_string();
    };
    let parts: Vec<&str> = parents
        .iter()
        .filter_map(|parent| env.get(*parent).map(String::as_str))
        .collect();
    debug_assert_eq!(
        parts.len(),
        parents.len(),
        "node {node_id} dispatched before all its predecessors committed"
    );
    if parts.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", parts.join(","))
    }
}

/// 取消分支：无 token 时永不就绪。
async fn wait_cancel(cancel: Option<&CancellationToken>) {
    match cancel {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// 回填预算截断（UTF-8 字符边界）。
fn truncate_to_budget(mut value: String, remaining: usize) -> (String, bool) {
    if value.len() <= remaining {
        return (value, false);
    }
    let mut end = remaining;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    (value, true)
}

/// 终端节点汇总：单终端保持旧语义；多终端返回 `{"terminals":{...}}`。
fn terminal_summary(plan: &ToolPlan, env: &HashMap<String, String>) -> String {
    let terminals: Vec<&str> = plan
        .nodes
        .iter()
        .filter(|node| !plan.edges.iter().any(|edge| edge.from == node.id))
        .map(|node| node.id.as_str())
        .collect();
    match terminals.as_slice() {
        [] => "{}".to_string(),
        [only] => env.get(*only).cloned().unwrap_or_else(|| "{}".to_string()),
        many => {
            let mut entries: Vec<String> = Vec::with_capacity(many.len());
            for id in many {
                let value = env.get(*id).cloned().unwrap_or_else(|| "{}".to_string());
                let key = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_string());
                entries.push(format!("{key}:{value}"));
            }
            format!("{{\"terminals\":{{{}}}}}", entries.join(","))
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
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
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
