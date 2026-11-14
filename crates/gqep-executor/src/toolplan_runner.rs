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
//!
//! # panic 姿态（ADR-271，诚实声明）
//!
//! `[profile.release]` 开 `panic = "abort"`（体积 SLO 的刻意权衡，见根 `Cargo.toml`）
//! ⇒ **工具执行中的 panic 会中止整个进程**，且 `catch_unwind` 在 release 下**抓不住**
//! （abort 不走 unwind）。因此本层只承诺 **Err 级 fail-soft**（缺字段/执行失败转为
//! 节点错误文本），**不承诺** panic 隔离。
//!
//! 反向锁 `tests/panic_posture_lock.rs` 断言：本声明在场 + 工具路径**零 `catch_unwind`** ——
//! 防止有人加一个在 release 下无效、却制造「已隔离」错觉的包装。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use nexus_contracts::tool_plan::{guards, PlanError, SideEffectDecl, ToolNode, ToolOp, ToolPlan};
use tokio::sync::Semaphore;

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

    /// 自报该工具的副作用属性 —— 计划未写 `side_effect` 时的**派生真值源**
    ///
    /// WHY 默认实现返回 `None`（＝未知）而不是让每个实现方都必须声明:
    /// 加性是硬要求 —— 既有实现（含测试夹具）零改动即保持今日行为，
    /// `tests/toolplan_concurrency_proptest.rs` 的四条不变量因此原样成立。
    ///
    /// WHY 真值源放在执行器而不是工具名前缀表（`streaming_dispatch::SideEffect::classify`）:
    /// 前缀表只认 `read:` / `query:` / `search:`，对 `text.echo` / `fs.read` / `fs.list`
    /// 一律判 Write ⇒ 接了等于没接；而**要执行该工具的对象**才知道它是否只读。
    /// 计划里的显式声明优先级仍高于本方法（见 `effective_side_effect`）。
    fn capability_of(&self, _tool_name: &str) -> Option<SideEffectDecl> {
        None
    }

    /// 执行器的批量副作用声明（可选，默认空集 = 未知）
    ///
    /// WHY 与 `capability_of` 并存：本方法面向「配置驱动、一次返回全表」的执行器
    /// （如 MCP 服务器声明的只读工具清单）；默认空集保证既有实现零改动。
    /// 优先级：计划节点显式声明 > `capability_of` > 本方法 > 未知（按可能写）。
    fn declared_side_effects(&self) -> Vec<(String, SideEffectDecl)> {
        Vec::new()
    }

    /// 本执行器可服务的工具名全集 —— 路由环（`RouteLayer`）的注册表来源
    ///
    /// WHY 与 `capability_of` 并列而非另立注册表:同一个实现方既是「谁能执行」
    /// 也是「执行什么属性」的唯一知情人；两处真值分叉就会重现
    /// 「路由表恒空 ⇒ 环永不裁决」的现状。默认空集 ⇒ 路由环退回 fallback，
    /// 与今日行为逐字一致。
    fn declared_tools(&self) -> Vec<String> {
        Vec::new()
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

/// 截断原因 — 区分「计划级熔断」与「回填预算超限」（ADR-268）
///
/// WHY 由执行器返回枚举，而不是 CLI 侧用 `duration_ms >= plan_ms` 反推：
/// 两个截断源的真值只有 `PlanRunner` 知道；反推会在「预算超限恰好发生在
/// 计划级超时之前」时误判，把结构事实降级成启发式猜测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationReason {
    /// 计划级超时熔断（`PlanGuards::timeout_ms`）
    PlanTimeout,
    /// 回填预算超限（`PlanGuards::budget_bytes`）
    BudgetBytes,
}

impl TruncationReason {
    /// 稳定字符串标识（CLI 的 JSON 字段用它；勿随意改名——脚本可能依赖）
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlanTimeout => "plan_timeout",
            Self::BudgetBytes => "budget_bytes",
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
    /// 截断标记（计划级熔断**或**回填预算超限）
    pub truncated: bool,
    /// 截断原因（仅 `truncated == true` 时非 `None`）
    pub truncation: Option<TruncationReason>,
}

/// 计划运行器 — topo 序解释执行 DAG
///
/// 分层执行:只读/纯数据算子层有界并发（`Semaphore` 上界 = 默认 8 或
/// [`Self::with_concurrency`] 覆写值）;含「可能写」节点的层整层串行。
pub struct PlanRunner {
    /// 工具执行器（注入）
    executor: Box<dyn ToolExecutor>,
    /// 守卫
    guards: PlanGuards,
    /// 只读层并发上界（有界,恒 ≥ 1）
    concurrency: usize,
}

impl std::fmt::Debug for PlanRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Box<dyn ToolExecutor> 无 Debug:trait 对象边界（与 AppServer 同模式）
        f.debug_struct("PlanRunner")
            .field("guards", &self.guards)
            .field("concurrency", &self.concurrency)
            .finish_non_exhaustive()
    }
}

impl PlanRunner {
    /// 默认只读层并发上界:默认 8 = 有界上界，语义不变（结果与顺序契约不受并发度影响）
    const DEFAULT_CONCURRENCY: usize = 8;

    /// 新建运行器
    ///
    /// 默认并发度 8 = 有界上界，语义不变（结果与顺序契约不受并发度影响，
    /// 并发度只影响只读层的墙钟并行度）。
    #[must_use]
    pub fn new(executor: Box<dyn ToolExecutor>, guards: PlanGuards) -> Self {
        Self {
            executor,
            guards,
            concurrency: Self::DEFAULT_CONCURRENCY,
        }
    }

    /// 覆写只读层并发上界
    ///
    /// `n == 0` 按 1 处理:信号量为 0 时首个 `acquire` 永久挂起 = 死锁。
    #[must_use]
    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }

    /// 执行计划 — 校验即拒（冲突计划 100% 拒绝）,分层解释执行
    ///
    /// # 执行模型（WI-16 门禁:无冲突层内并行）
    /// DAG 按节点深度分层（Kahn 最长路径）;同层节点互无依赖（DAG 性质）,
    /// 层间串行（数据依赖）。层内策略:只读/纯数据算子层经 `FuturesUnordered`
    /// + `Semaphore` 有界并发;含「可能写」节点的层整层串行（不可重排、不可并发）。
    ///
    /// # 返回
    /// - `Ok(PlanSummary)`:聚合结果回填（预算内截断）;
    /// - `Err(PlanError)`:校验失败（环/超步数/悬空边/不完整调用）——调用方
    ///   据此退化逐次调用（WI-16 回退路径）。
    pub async fn run(&self, plan: &ToolPlan) -> Result<PlanSummary, PlanError> {
        let started = Instant::now();
        plan.validate(self.guards.max_steps)?;
        // 只读模式:拒绝「非只读」节点（Plan 模式投影）
        //
        // 判据改用有效属性（声明 > 能力表 > 未知）而非只看 `Some(Write)`:
        // 旧判据下**省略 `side_effect` 字段即可绕过只读模式** —— 一个实际写文件的
        // 工具在 Plan 模式里照样被执行，与「投影拒绝写」的既定语义相悖（F-A21）。
        // 未知按可能写处理，与 `effective_side_effect` 同一保守口径。
        if self.guards.readonly_only {
            for n in &plan.nodes {
                if n.op == ToolOp::ToolCall
                    && effective_side_effect(
                        n,
                        self.executor.as_ref(),
                        n.tool_name.as_deref().unwrap_or_default(),
                    ) != Some(SideEffectDecl::ReadOnly)
                {
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
        let mut truncation: Option<TruncationReason> = None;

        for layer in &layers {
            // 单计划超时熔断（WI-16 守卫）
            if started.elapsed() > Duration::from_millis(self.guards.timeout_ms) {
                truncated = true;
                truncation = Some(TruncationReason::PlanTimeout);
                break;
            }
            // 层内策略:副作用层串行,只读/纯算子层有界并发
            let layer_truncated = self
                .exec_layer(plan, layer, &mut env, &mut tool_calls, &mut nodes_executed)
                .await;
            // 回填预算守卫:环境累积超限即截断（串行/并发两条路径同口径）
            if layer_truncated {
                truncated = true;
                truncation = Some(TruncationReason::BudgetBytes);
                break;
            }
        }

        // 聚合回填：取**拓扑终端**节点（无出边）中执行序最后的那一个（ADR-277）。
        //
        // WHY 不能用 `plan.nodes.last()`：那是**声明序**末位，不保证是拓扑终端 ——
        // 当末位声明节点有后继时，它会先于后继执行，于是旧实现把「中间节点」的
        // 结果当成了计划输出（正确性缺陷，不是性能问题）。反向锁：
        // `tests/toolplan_terminal_aggregation.rs::plan_summary_uses_topological_terminal`。
        // 多终端时按**声明序倒序**取「已执行」的第一个 —— 不能按层序取：
        // `compute_layers` 的 frontier 由 HashMap 迭代构造，层内顺序不确定
        // （本轮实测：同一计划在 c=1 与 c=2 下摘要有 50% 概率不同，
        //  打红了 `readonly_dag_summary_equivalent`）。声明序是唯一稳定的定序。
        let summary = plan
            .nodes
            .iter()
            .rev()
            .filter(|n| !plan.edges.iter().any(|e| e.from == n.id))
            .find_map(|n| env.get(n.id.as_str()).cloned())
            .unwrap_or_else(|| "{}".into());
        Ok(PlanSummary {
            plan_id: plan.id.clone(),
            summary,
            tool_calls,
            nodes_executed,
            duration_ms: started.elapsed().as_millis() as u64,
            truncated,
            truncation,
        })
    }

    /// 单层执行 — 全读并发 / 全写串行 / 混层按「写屏障」分段
    ///
    /// # 返回
    /// `true` = 回填预算超限（调用方置 `truncated` 并终止后续层）;
    /// `false` = 本层正常完成。
    async fn exec_layer(
        &self,
        plan: &ToolPlan,
        layer: &[ToolNode],
        env: &mut HashMap<String, String>,
        tool_calls: &mut usize,
        nodes_executed: &mut usize,
    ) -> bool {
        let read_flags: Vec<bool> = layer
            .iter()
            .map(|n| node_is_read_safe(n, self.executor.as_ref()))
            .collect();
        if read_flags.iter().all(|r| *r) {
            return self
                .exec_layer_concurrent(plan, layer, env, tool_calls, nodes_executed)
                .await;
        }
        if read_flags.iter().all(|r| !*r) {
            return self
                .exec_layer_serial(plan, layer, env, tool_calls, nodes_executed)
                .await;
        }
        self.exec_layer_segmented(plan, layer, &read_flags, env, tool_calls, nodes_executed)
            .await
    }

    /// 混层执行 — 以「可能写」节点为屏障切分为读段
    ///
    /// WHY 不是整层串行也不是全层并发：
    /// - 整层串行会让 1 个写节点拖住同层最多 64 个只读节点（ADR-281 实测的退化点）；
    /// - 全层并发会让写序由调度器决定（不可复现、审计链失序）。
    ///
    /// 读段内部并发、读段之间按声明序被写节点隔开、读写**不**并发 —— 既保住
    /// 「写节点严格按声明序执行、读节点不跨越写节点观察外部状态」的既有语义，
    /// 又让同段只读节点并行。
    ///
    /// 层内无边界（Kahn 分层保证），故读节点不依赖同层写节点的输出；跨段回填
    /// 仍走同一 `accumulate`，不引入第二套预算/取消真值源（ADR-277 教训）。
    async fn exec_layer_segmented(
        &self,
        plan: &ToolPlan,
        layer: &[ToolNode],
        read_flags: &[bool],
        env: &mut HashMap<String, String>,
        tool_calls: &mut usize,
        nodes_executed: &mut usize,
    ) -> bool {
        let mut seg_start = 0usize;
        for (idx, node) in layer.iter().enumerate() {
            if read_flags[idx] {
                continue;
            }
            if seg_start < idx
                && self
                    .exec_layer_concurrent(
                        plan,
                        &layer[seg_start..idx],
                        env,
                        tool_calls,
                        nodes_executed,
                    )
                    .await
            {
                return true;
            }
            // 单节点写：复用串行路径，保持与「整层串行」逐字同口径的累计/预算判定。
            if self
                .exec_layer_serial(
                    plan,
                    std::slice::from_ref(node),
                    env,
                    tool_calls,
                    nodes_executed,
                )
                .await
            {
                return true;
            }
            seg_start = idx + 1;
        }
        if seg_start < layer.len() {
            return self
                .exec_layer_concurrent(plan, &layer[seg_start..], env, tool_calls, nodes_executed)
                .await;
        }
        false
    }

    /// 串行层执行 — 按既有层序逐节点 `await`（不 spawn、不并发）
    ///
    /// WHY 整层串行:该层存在 `op == ToolCall && side_effect != Some(ReadOnly)` 的节点,
    /// 即有副作用（显式 `Write` 或未声明 `None` 保守按「可能写」处理）。有副作用节点
    /// 不可重排、不可并发:并发会让同层写入顺序由调度器决定 ⇒ 结果不可复现,
    /// 审计链也无法对齐「计划声明顺序」的可审计性契约。
    async fn exec_layer_serial(
        &self,
        plan: &ToolPlan,
        layer: &[ToolNode],
        env: &mut HashMap<String, String>,
        tool_calls: &mut usize,
        nodes_executed: &mut usize,
    ) -> bool {
        for node in layer {
            // 每个节点用「前序节点已回填」的 env 计算输入（数据依赖需逐节点可见）
            let input = self.inputs_of(plan, env, &node.id);
            let out = self.exec_node(node, &input).await;
            accumulate(node, out, env, tool_calls, nodes_executed);
            if env.values().map(String::len).sum::<usize>() > self.guards.budget_bytes {
                return true;
            }
        }
        false
    }

    /// 并发层执行 — 只读/纯数据算子层:`Semaphore` 限制同时进行的节点数
    ///
    /// WHY 有界而非无上限 spawn:无上限 spawn 会把整层一次性推入运行时就绪队列,
    /// 在飞任务数与峰值内存随层宽线性增长（层宽受 `max_steps` 限制但仍可达 64）;
    /// 信号量把在飞节点数钉死在 `concurrency`,限流只延后不拒绝。
    async fn exec_layer_concurrent(
        &self,
        plan: &ToolPlan,
        layer: &[ToolNode],
        env: &mut HashMap<String, String>,
        tool_calls: &mut usize,
        nodes_executed: &mut usize,
    ) -> bool {
        let permits = Arc::new(Semaphore::new(self.concurrency.max(1)));
        let mut tasks = FuturesUnordered::new();
        for node in layer {
            let input = self.inputs_of(plan, env, &node.id);
            let node_ref = node.clone();
            let sem = Arc::clone(&permits);
            tasks.push(async move {
                // 必须先取许可再执行:在飞节点数恒 ≤ concurrency（有界上界）
                let _permit = match sem.acquire_owned().await {
                    Ok(p) => p,
                    // 信号量在本运行器生命周期内不会被 close;防御性 fail-soft,不 panic
                    Err(_) => {
                        return (
                            node_ref,
                            r#"{"error":"concurrency gate closed"}"#.to_string(),
                        );
                    }
                };
                let out = self.exec_node(&node_ref, &input).await;
                (node_ref, out)
            });
        }
        while let Some((node, out)) = tasks.next().await {
            accumulate(&node, out, env, tool_calls, nodes_executed);
            // 回填预算守卫:环境累积超限即截断（与串行层完全同口径）
            if env.values().map(String::len).sum::<usize>() > self.guards.budget_bytes {
                return true;
            }
        }
        false
    }

    /// 执行单个节点（数据算子 / 工具调用）
    async fn exec_node(&self, node: &ToolNode, input: &str) -> String {
        match node.op {
            ToolOp::ToolCall => {
                // validate 已保证二者齐备;但缺字段不再靠 expect 不变量——
                // 与下方执行失败同型 fail-soft(错误文本进节点结果，可审计不 panic)。
                let Some(tool) = node.tool_name.as_deref() else {
                    return r#"{"error":"missing tool_name"}"#.to_string();
                };
                let Some(args) = node.args_json.as_deref() else {
                    return r#"{"error":"missing args_json"}"#.to_string();
                };
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

/// 节点结果回填 — `tool_calls`/`nodes_executed` 累加 + 环境写入
///
/// WHY 抽公共函数:串行层与并发层必须共用同一回填口径,否则「层策略」会悄悄
/// 改变统计计数与截断判定语义（等价性硬门槛:同计划不同并发度 `PlanSummary` 相等）。
fn accumulate(
    node: &ToolNode,
    out: String,
    env: &mut HashMap<String, String>,
    tool_calls: &mut usize,
    nodes_executed: &mut usize,
) {
    if node.op == ToolOp::ToolCall {
        *tool_calls += 1;
    }
    *nodes_executed += 1;
    env.insert(node.id.clone(), out);
}

/// 节点的有效副作用属性 —— 计划声明 > 执行器能力表 > 未知
///
/// WHY 这个优先级次序:显式声明是计划作者的意图，实现方不得推翻它
/// （否则接入方可把写操作自报为只读来换取并发，层内写序不再由计划决定，
/// 可复现性与审计链同时失效）；两者都不知道时返回 `None`，
/// 由调用方 `node_is_read_safe` 按「可能写」保守处理。
fn effective_side_effect(
    node: &ToolNode,
    executor: &dyn ToolExecutor,
    declared: &str,
) -> Option<SideEffectDecl> {
    if let Some(effect) = node
        .side_effect
        .or_else(|| executor.capability_of(declared))
    {
        return Some(effect);
    }
    // R9：配置驱动的批量声明（默认空集 ⇒ 既有行为逐字不变）
    executor
        .declared_side_effects()
        .into_iter()
        .find(|(name, _)| name == declared)
        .map(|(_, effect)| effect)
}

/// 单节点是否「读安全」——纯数据算子恒安全；ToolCall 需有效副作用 == ReadOnly。
///
/// 「有效副作用」按 `effective_side_effect` 取值：显式声明优先，其次执行器能力表，
/// 两者皆无 ⇒ 按「可能写」保守处理（未声明 ≠ 只读）。WHY 保守：并发会让同层写入
/// 顺序由调度器决定 ⇒ 结果不可复现、审计链失序（ADR-254 D1 / ADR-281）。
fn node_is_read_safe(node: &ToolNode, executor: &dyn ToolExecutor) -> bool {
    node.op != ToolOp::ToolCall
        || effective_side_effect(
            node,
            executor,
            node.tool_name.as_deref().unwrap_or_default(),
        ) == Some(SideEffectDecl::ReadOnly)
}

/// 分层 — 每节点深度 = 最长路径长度（Kahn 扩展）;同层节点互无依赖
///
/// 返回按深度升序的层列表；**层内按计划声明序**（稳定定序）。
/// 实现:按「入度清零轮次」分层（Kahn 每轮弹出入度=0 的节点 = 一层）;
/// 若放置节点数 < 总数 → 环（validate 已保证无环,防御性校验）。
///
/// WHY 层内必须显式按声明序排序：frontier 由 `HashMap` 迭代构造，层内顺序
/// 不确定（ADR-277 D3 实测：两个终端同层时摘要随并发度漂移）。只读层并发时
/// 顺序不影响结果；有写节点的层由 [`exec_layer_serial`] 逐节点执行，若沿用
/// 不确定的层序，「同层写节点的执行顺序」会跨运行漂移 —— 审计链无法对齐。
fn compute_layers(plan: &ToolPlan) -> Result<Vec<Vec<ToolNode>>, PlanError> {
    // 声明序索引：层内排序键。
    let decl: HashMap<&str, usize> = plan
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
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
    frontier
        .make_contiguous()
        .sort_by_key(|n| decl.get(*n).copied().unwrap_or(usize::MAX));
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
        next.make_contiguous()
            .sort_by_key(|n| decl.get(*n).copied().unwrap_or(usize::MAX));
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

    /// R9：声明式执行器 —— 仅 `declared_side_effects` 提供只读声明
    struct DeclaringExecutor;

    #[async_trait]
    impl ToolExecutor for DeclaringExecutor {
        async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
            Ok(args_json.to_string())
        }
        fn declared_side_effects(&self) -> Vec<(String, SideEffectDecl)> {
            vec![("mcp.srv.audit".to_string(), SideEffectDecl::ReadOnly)]
        }
    }

    fn tool_call_node(tool: &str, side_effect: Option<SideEffectDecl>) -> ToolNode {
        ToolNode {
            id: format!("call-{tool}"),
            op: ToolOp::ToolCall,
            tool_name: Some(tool.into()),
            args_json: Some("{}".into()),
            field: None,
            predicate: None,
            side_effect,
        }
    }

    #[test]
    fn declared_side_effects_make_tool_read_safe_without_node_declaration() {
        let executor = DeclaringExecutor;
        let declared = tool_call_node("mcp.srv.audit", None);
        assert!(
            node_is_read_safe(&declared, &executor),
            "执行器批量声明为 ReadOnly ⇒ 计划未声明也应进入并发段"
        );
        let unknown = tool_call_node("mcp.srv.write", None);
        assert!(
            !node_is_read_safe(&unknown, &executor),
            "未声明工具必须按可能写保守串行（fail-closed 不扩散）"
        );
        let explicit_write = tool_call_node("mcp.srv.audit", Some(SideEffectDecl::Write));
        assert!(
            !node_is_read_safe(&explicit_write, &executor),
            "计划显式声明优先于执行器声明"
        );
    }

    #[test]
    fn default_executor_declares_no_side_effects() {
        assert!(MemExecutor.declared_side_effects().is_empty());
        let unknown = tool_call_node("mcp.srv.audit", None);
        assert!(
            !node_is_read_safe(&unknown, &MemExecutor),
            "默认空集 ⇒ 行为与改造前逐字一致（保守串行）"
        );
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
        assert_eq!(
            summary.truncation,
            Some(TruncationReason::BudgetBytes),
            "回填预算截断必须归因 BudgetBytes（ADR-268）"
        );
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
        assert_eq!(
            summary.truncation,
            Some(TruncationReason::PlanTimeout),
            "计划级熔断必须归因 PlanTimeout（ADR-268）——与预算截断可区分"
        );
    }
}
