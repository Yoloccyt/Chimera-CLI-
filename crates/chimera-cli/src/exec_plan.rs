//! `exec --plan` 工具执行内核接线（E1′ 首批次）
//!
//! # 职责
//! 把声明式 [`ToolPlan`] 接进真实工具执行内核：
//! - [`BuiltinToolExecutor`]：进程内置工具（`text.echo` / `fs.read` / `fs.list`），
//!   路径受限（仅相对路径、拒绝 `..`），`fs.read` 有 256 KiB 上限；
//! - [`RuntimeToolExecutor`]：委托 [`ToolRuntime`] 四环链
//!   （policy → sandbox → route → transport），错误自带 `policy:` / `sandbox:` 归因前缀；
//! - [`OpTimeoutExecutor`]：单操作超时层；
//! - [`run_plan_with_failures`] / [`run_plan`]：计划级执行 + 三层超时防护 + SccCache 单点回写。
//!
//! # 三层超时防护（由内到外）
//! 1. **单操作**：[`OpTimeoutExecutor`]（复用既有 `GqepConfig::default_timeout_ms`）；
//! 2. **计划级**：[`PlanGuards::timeout_ms`]，`PlanRunner` 内层熔断并置 `truncated`；
//! 3. **进程级兜底**：`run_plan_with_failures` 外层 `tokio::time::timeout`
//!    （计划级 × 2），防止实现缺陷导致永久挂起。

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::tool_runtime::ToolRuntime;
use gqep_executor::{PlanGuards, PlanRunner, PlanSummary, ToolExecutor};
use nexus_contracts::spec::tool_plan::ToolPlan;
use nexus_contracts::tool_plan::{PlanError, SideEffectDecl};
use scc_cache::{ContextEntry, ContextId};

use crate::composition::AppContext;
use crate::error::ChimeraCliError;

/// `fs.read` 单文件上限（256 KiB）
const FS_READ_MAX_BYTES: u64 = 256 * 1024;

/// 进程内置工具执行器 — `text.echo` / `fs.read` / `fs.list`
///
/// 作为 [`ToolRuntime`] 的真实 transport delegate 使用；未知工具**恒失败**，
/// 绝不返回成功（避免把「未实现」伪装成「执行成功」）。
#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltinToolExecutor;

/// 内置只读工具名集合 —— 与 `execute` 的分派臂同源
///
/// WHY 三件工具全部声明 `ReadOnly`:它们只做「回显参数」与「读文件/列目录」,
/// 不写、不 fork 进程、不联网。声明后 `PlanRunner` 才会在计划**省略**
/// `side_effect` 字段时把同层节点放进有界并发（ADR-254）—— 此前这类计划
/// 整层串行，实测代价 ≈4.3×（`toolplan_layers/undeclared8_ms2/{1,8}`）。
///
/// 反向安全性:若将来给 `execute` 加了新的内置工具而忘记登记进本表，
/// 它的属性是「未知」⇒ 保守串行 + 不路由 ⇒ fail-closed，不会误并行。
const BUILTIN_READONLY_TOOLS: &[&str] = &["text.echo", "fs.read", "fs.list"];

#[async_trait]
impl ToolExecutor for BuiltinToolExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        match tool_name {
            // 原样回显参数串（先 parse 以拒绝非法 JSON）
            "text.echo" => {
                serde_json::from_str::<serde_json::Value>(args_json)
                    .map_err(|e| format!("text.echo invalid args JSON: {e}"))?;
                Ok(args_json.to_string())
            }
            "fs.read" => fs_read(args_json).await,
            "fs.list" => fs_list(args_json).await,
            other => Err(format!("unknown builtin tool: {other}")),
        }
    }

    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        BUILTIN_READONLY_TOOLS
            .contains(&tool_name)
            .then_some(SideEffectDecl::ReadOnly)
    }

    fn declared_tools(&self) -> Vec<String> {
        BUILTIN_READONLY_TOOLS
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }
}

/// `fs.read` — `{"path":"relative/file"}` → `{"content": "...", "bytes": n}`
async fn fs_read(args_json: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(args_json).map_err(|e| format!("fs.read invalid args JSON: {e}"))?;
    let raw = value
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| "fs.read requires {\"path\": \"relative/file\"}".to_string())?;
    let path = safe_relative_path(raw, "fs.read")?;

    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("fs.read cannot stat {raw}: {e}"))?;
    if meta.len() > FS_READ_MAX_BYTES {
        return Err(format!(
            "fs.read file too large: {raw} ({} bytes > {FS_READ_MAX_BYTES} bytes limit)",
            meta.len()
        ));
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("fs.read cannot read {raw} as UTF-8 text: {e}"))?;
    Ok(serde_json::json!({ "content": content, "bytes": content.len() }).to_string())
}

/// `fs.list` — `{"path":"."}`（缺省 `.`）→ `{"entries": [名称...]}`（字典序）
async fn fs_list(args_json: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(args_json).map_err(|e| format!("fs.list invalid args JSON: {e}"))?;
    let raw = value.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let path = safe_relative_path(raw, "fs.list")?;

    let mut read_dir = tokio::fs::read_dir(&path)
        .await
        .map_err(|e| format!("fs.list cannot open directory {raw}: {e}"))?;
    let mut entries = Vec::new();
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|e| format!("fs.list cannot read directory {raw}: {e}"))?
    {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("fs.list non-UTF-8 entry name under {raw}"))?;
        entries.push(name);
    }
    entries.sort();
    Ok(serde_json::json!({ "entries": entries }).to_string())
}

/// 文件工具路径约束 — 仅相对路径，拒绝绝对路径、`..` 与根/盘符前缀
fn safe_relative_path(raw: &str, tool: &str) -> Result<PathBuf, String> {
    if raw.is_empty() {
        return Err(format!("{tool} rejected empty path"));
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(format!("{tool} rejected absolute path: {raw}"));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!(
            "{tool} rejected parent-directory component ('..'): {raw}"
        ));
    }
    if path
        .components()
        .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
    {
        return Err(format!("{tool} rejected rooted/prefix path: {raw}"));
    }
    Ok(path.to_path_buf())
}

/// 四环链执行器 — 委托 [`ToolRuntime::execute`]（policy → sandbox → route → transport）
///
/// WHY 不直接给 L7 的 `ToolRuntime` 加 blanket impl：L7 已持有 `ToolExecutor` 实现，
/// 本层只做「计划装配点的显式命名」，避免跨层实现耦合。
pub struct RuntimeToolExecutor {
    /// 四环链运行时（共享 Arc）
    pub inner: Arc<ToolRuntime>,
}

#[async_trait]
impl ToolExecutor for RuntimeToolExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        ToolExecutor::execute(self.inner.as_ref(), tool_name, args_json).await
    }

    /// 能力视图必须逐层转发到 [`ToolRuntime`]
    ///
    /// WHY 关键:`PlanRunner` 的 `layer_requires_serial` 拿到的是**装饰器链最外层**，
    /// 任何一跳省略本方法都会让能力表在现网静默失效（而单测直接用 ProbeExecutor
    /// 仍然全绿 —— 即「测试证明不了生产接线」的经典形态）。
    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        ToolExecutor::capability_of(self.inner.as_ref(), tool_name)
    }

    fn declared_tools(&self) -> Vec<String> {
        ToolExecutor::declared_tools(self.inner.as_ref())
    }
}

/// 单操作超时执行器 — 每个工具调用独立 `tokio::time::timeout`
pub struct OpTimeoutExecutor {
    /// 被装饰执行器
    pub inner: Arc<dyn ToolExecutor>,
    /// 单操作超时
    pub per_op_timeout: Duration,
}

#[async_trait]
impl ToolExecutor for OpTimeoutExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        match tokio::time::timeout(
            self.per_op_timeout,
            self.inner.execute(tool_name, args_json),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(format!(
                "tool op timeout after {}ms: {tool_name}",
                self.per_op_timeout.as_millis()
            )),
        }
    }

    // 能力视图逐层转发（见 RuntimeToolExecutor 的同名方法注释）
    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        self.inner.capability_of(tool_name)
    }

    fn declared_tools(&self) -> Vec<String> {
        self.inner.declared_tools()
    }
}

/// 失败计数装饰器 — `PlanSummary` 无逐节点状态，本层统计工具失败节点数
struct CountingExecutor {
    /// 被装饰执行器
    inner: Arc<dyn ToolExecutor>,
    /// 失败节点计数（共享给 `run_plan_with_failures`）
    failed: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for CountingExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        match self.inner.execute(tool_name, args_json).await {
            Ok(value) => Ok(value),
            Err(err) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
                Err(err)
            }
        }
    }

    // 能力视图逐层转发（见 RuntimeToolExecutor 的同名方法注释）
    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        self.inner.capability_of(tool_name)
    }

    fn declared_tools(&self) -> Vec<String> {
        self.inner.declared_tools()
    }
}

/// 装配计划级守卫 — `timeout_ms` 做 1ms 下限保护
///
/// WHY `max(1)`:计划级守卫取 0 会让 `PlanRunner` 的内层熔断**立刻**触发，
/// 任何计划都会被「零超时」截断成空摘要（看起来像执行成功但其实什么都没跑）。
/// 下限 1ms 保证调用方传 0 时仍走正常执行路径，截断只可能来自真实耗时。
pub(crate) fn plan_guards_for(plan_timeout_ms: u64) -> PlanGuards {
    PlanGuards {
        timeout_ms: plan_timeout_ms.max(1),
        ..PlanGuards::default()
    }
}

/// 单操作超时上限（10 分钟）— 超出显式拒绝，不静默截断用户意图
pub const MAX_OP_TIMEOUT_MS: u64 = 600_000;
/// 计划级超时上限（1 小时）— 外层兜底为其 ×2
pub const MAX_PLAN_TIMEOUT_MS: u64 = 3_600_000;

/// 解析单操作有效超时（三层优先级，由内到外收敛）
///
/// 优先级：`CLI 显式值 / GqepConfig::default_timeout_ms` → 计划级 → 全局 gather。
/// WHY 逐层取 min：`gather_deadline_ms`（>0 时）是 GQEP 全局硬上限，
/// CLI 只能**收紧**它、不能越过；计划级是单操作的外层预算，单操作不应长于计划。
/// `gather_deadline_ms = 0` 表示关闭全局截止，此时不参与收敛（既有语义）。
pub(crate) fn effective_op_timeout_ms(
    op_override_ms: Option<u64>,
    plan_timeout_ms: u64,
    config_default_ms: u64,
    gather_deadline_ms: u64,
) -> u64 {
    let mut effective = op_override_ms
        .unwrap_or(config_default_ms)
        .max(1)
        .min(plan_timeout_ms.max(1));
    if gather_deadline_ms > 0 {
        effective = effective.min(gather_deadline_ms);
    }
    effective.max(1)
}

/// 外层兜底超时 — 把 `PlanRunner::run` 的三态结果映射为 CLI 错误
///
/// WHY 独立成函数:外层兜底（计划级 × 2）是进程级最后一道防线，
/// 其分支语义直接决定退出码（校验失败 → `EngineError` = 3，
/// 兜底超时 → `Timeout` = 4），显式命名后可脱离 `AppContext` 装配单独验证，
/// 避免「映射写错但被端到端噪声掩盖」。
pub(crate) async fn await_outer_deadline<F>(
    outer: Duration,
    fut: F,
) -> Result<PlanSummary, ChimeraCliError>
where
    F: std::future::Future<Output = Result<PlanSummary, PlanError>>,
{
    match tokio::time::timeout(outer, fut).await {
        Ok(Ok(summary)) => Ok(summary),
        Ok(Err(plan_error)) => Err(ChimeraCliError::EngineError(format!(
            "计划校验失败: {plan_error}"
        ))),
        Err(_) => Err(ChimeraCliError::Timeout(format!(
            "计划执行超时（外层兜底 {}ms）",
            outer.as_millis()
        ))),
    }
}

/// 执行计划（兼容入口）— 只返回聚合摘要，失败节点数经 [`run_plan_with_failures`]
pub async fn run_plan(
    ctx: &AppContext,
    plan: &ToolPlan,
    concurrency: usize,
    plan_timeout_ms: u64,
) -> Result<PlanSummary, ChimeraCliError> {
    run_plan_with_failures(ctx, plan, concurrency, plan_timeout_ms)
        .await
        .map(|(summary, _failed)| summary)
}

/// 执行计划并返回 `(聚合摘要, 失败节点数)`
///
/// - 计划非法 / 校验失败 → [`ChimeraCliError::EngineError`]（Exec 退出码 3）
/// - 外层兜底超时 → [`ChimeraCliError::Timeout`]（Exec 退出码 4）
/// - SccCache 回写为 **单点**（全库唯一 `scc.insert`），成功/截断/节点失败均回写
///
/// 单操作超时取 `GqepConfig::default_timeout_ms`（既有默认）；CLI 需显式覆盖时
/// 用 [`run_plan_with_failures_timed`]。
pub async fn run_plan_with_failures(
    ctx: &AppContext,
    plan: &ToolPlan,
    concurrency: usize,
    plan_timeout_ms: u64,
) -> Result<(PlanSummary, usize), ChimeraCliError> {
    run_plan_with_failures_timed(ctx, plan, concurrency, plan_timeout_ms, None).await
}

/// [`run_plan_with_failures`] 的可显式指定单操作超时版本（非破坏性扩展入口）
///
/// `op_timeout_ms = None` 时与既有语义**逐字一致**（取 `GqepConfig::default_timeout_ms`）；
/// `Some(ms)` 时仍受计划级与全局 `gather_deadline_ms` 收敛（见 [`effective_op_timeout_ms`]）。
pub async fn run_plan_with_failures_timed(
    ctx: &AppContext,
    plan: &ToolPlan,
    concurrency: usize,
    plan_timeout_ms: u64,
    op_timeout_ms: Option<u64>,
) -> Result<(PlanSummary, usize), ChimeraCliError> {
    let guards = plan_guards_for(plan_timeout_ms);

    // ADR-244 D1：传输环 delegate 在组合根**唯一装配**（`composition::build`）。
    // 这里复用 `ctx.tool_runtime`，不再局部再装配 —— 否则同一进程会出现两种
    // 传输语义（同名工具两种归因），且策略规则双源无编译期一致性保证。
    let runtime = Arc::clone(&ctx.tool_runtime);

    // 单操作超时：CLI 显式值 → 计划级 → 全局 gather，逐层取 min（CLI 只能收紧）。
    let per_op_ms = effective_op_timeout_ms(
        op_timeout_ms,
        guards.timeout_ms,
        ctx.gqep.config().default_timeout_ms,
        ctx.gqep.config().gather_deadline_ms,
    );

    let failed = Arc::new(AtomicUsize::new(0));
    // 装饰器顺序即**失败归因契约**（ADR-257），不可倒置：
    // 计数层必须在超时层**之外**。倒置时（本行原实现）超时由外层产生、
    // 内层 future 被直接 drop ⇒ `CountingExecutor` 的 Err 分支永不执行
    // ⇒ 超时节点不计入 `failed_nodes` ⇒ 整个 `exec --plan` 以一个失败于
    // 超时的计划返回 `status:"ok"` 与退出码 0（F-A1）。
    // 顺序修正不新增字段、不改 JSON 形状、不引入开关，revert 即回原状。
    let timed = Arc::new(OpTimeoutExecutor {
        inner: Arc::new(RuntimeToolExecutor { inner: runtime }),
        per_op_timeout: Duration::from_millis(per_op_ms),
    });
    let executor: Box<dyn ToolExecutor> = Box::new(CountingExecutor {
        inner: timed,
        failed: Arc::clone(&failed),
    });
    let runner = PlanRunner::new(executor, guards).with_concurrency(concurrency);

    // 外层兜底：计划级 × 2，防止实现缺陷导致永久挂起
    let outer_timeout = Duration::from_millis(plan_timeout_ms.saturating_mul(2).max(1));
    let summary = await_outer_deadline(outer_timeout, runner.run(plan)).await?;

    let failed_nodes = failed.load(Ordering::Relaxed);
    write_scc_entry(ctx, &summary, failed_nodes);

    Ok((summary, failed_nodes))
}

/// SccCache 单点回写 — `insert` 为无失败签名（这里无需 catch），
/// 字段不足时以既有 `ContextEntry{id, content}` 承载 plan_id/摘要/调用数/截断标记。
fn write_scc_entry(ctx: &AppContext, summary: &PlanSummary, failed_nodes: usize) {
    let entry_id = ContextId::new(format!("exec-plan:{}", summary.plan_id));
    let content = serde_json::json!({
        "plan_id": summary.plan_id,
        "summary": summary.summary,
        "tool_calls": summary.tool_calls,
        "nodes_executed": summary.nodes_executed,
        "duration_ms": summary.duration_ms,
        "truncated": summary.truncated,
        "failed_nodes": failed_nodes,
    })
    .to_string();
    ctx.scc.insert(ContextEntry::new(entry_id, content));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// 超时必须计入失败节点 —— 装饰器顺序即失败归因契约（F-A1 / ADR-257）
    ///
    /// 含**反向对照**:同一个慢执行器在「超时层在外、计数层在内」的旧顺序下
    /// 必须重现「超时不计数」缺陷。没有这半边，正例可能只是恰好通过，
    /// 本条测试就失去对「顺序」这一契约的约束力。
    #[tokio::test]
    async fn op_timeout_is_counted_as_failed_node() {
        struct SlowExecutor;

        #[async_trait]
        impl ToolExecutor for SlowExecutor {
            async fn execute(&self, tool_name: &str, _args: &str) -> Result<String, String> {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(format!("{tool_name}:done"))
            }
        }

        // 正例：契约顺序（计数层在超时层之外）
        let failed = Arc::new(AtomicUsize::new(0));
        let timed = Arc::new(OpTimeoutExecutor {
            inner: Arc::new(SlowExecutor) as Arc<dyn ToolExecutor>,
            per_op_timeout: Duration::from_millis(10),
        });
        let counting: Arc<dyn ToolExecutor> = Arc::new(CountingExecutor {
            inner: timed,
            failed: Arc::clone(&failed),
        });
        let err = counting
            .execute("bench.slow", "{}")
            .await
            .expect_err("200ms 工具在 10ms 预算下必须超时");
        assert!(
            err.contains("timeout"),
            "错误文案应含 timeout，实得 {err:?}"
        );
        assert_eq!(
            failed.load(Ordering::Relaxed),
            1,
            "超时节点必须计入 failed_nodes，否则 exec --plan 会以退出码 0 宣告成功"
        );

        // 反向对照：旧顺序必须重现缺陷
        let failed_old = Arc::new(AtomicUsize::new(0));
        let counting_old = Arc::new(CountingExecutor {
            inner: Arc::new(SlowExecutor) as Arc<dyn ToolExecutor>,
            failed: Arc::clone(&failed_old),
        });
        let inverted: Arc<dyn ToolExecutor> = Arc::new(OpTimeoutExecutor {
            inner: counting_old,
            per_op_timeout: Duration::from_millis(10),
        });
        inverted
            .execute("bench.slow", "{}")
            .await
            .expect_err("倒置顺序同样应超时");
        assert_eq!(
            failed_old.load(Ordering::Relaxed),
            0,
            "反向对照未能重现「超时不计数」⇒ 本测试没有真正锁住顺序契约"
        );
    }

    /// 单操作超时三层收敛：CLI/配置默认 → 计划级 → 全局 gather（>0 才参与）
    #[test]
    fn effective_op_timeout_layers() {
        // 缺省：取 GqepConfig 默认 5000
        assert_eq!(effective_op_timeout_ms(None, 30_000, 5_000, 5_000), 5_000);
        // 显式值可收紧
        assert_eq!(
            effective_op_timeout_ms(Some(1_000), 30_000, 5_000, 5_000),
            1_000
        );
        // 不得越过计划级
        assert_eq!(
            effective_op_timeout_ms(Some(10_000), 1_000, 5_000, 0),
            1_000
        );
        // gather > 0 时再对单操作收紧（CLI 不能越过全局上限）
        assert_eq!(
            effective_op_timeout_ms(Some(10_000), 30_000, 5_000, 2_000),
            2_000
        );
        // gather = 0 表示关闭全局截止，不参与收敛
        assert_eq!(
            effective_op_timeout_ms(Some(10_000), 30_000, 5_000, 0),
            10_000
        );
        // 任何输入都不得产 0（否则立即超时）
        assert_eq!(effective_op_timeout_ms(Some(0), 0, 0, 0), 1);
    }

    proptest! {
        /// 不变量：结果恒 ≥1；≤ 基值；≤ 计划级；gather>0 时 ≤ gather。
        ///
        /// WHY 覆盖整个输入域：CLI 值、默认值、计划级、gather 四个来源组合，
        /// 任何一层漏取 min 都会让「用户收紧的超时」被更宽的上层掩盖。
        #[test]
        fn effective_op_timeout_invariants(
            op in proptest::option::of(0u64..=MAX_OP_TIMEOUT_MS),
            plan in 0u64..=MAX_PLAN_TIMEOUT_MS,
            default in 0u64..=MAX_OP_TIMEOUT_MS,
            gather in proptest::option::of(1u64..=MAX_PLAN_TIMEOUT_MS),
        ) {
            let gather_ms = gather.unwrap_or(0);
            let effective = effective_op_timeout_ms(op, plan, default, gather_ms);
            prop_assert!(effective >= 1, "effective must be >=1, got {effective}");
            prop_assert!(effective <= plan.max(1), "must not exceed plan scope");
            prop_assert!(effective <= op.unwrap_or(default).max(1), "must not exceed base");
            if gather_ms > 0 {
                prop_assert!(effective <= gather_ms, "must not exceed gather deadline");
            }
        }
    }

    /// 计划级守卫必须原样转发调用方传入的计划超时，且对 0 做 1ms 下限保护
    #[test]
    fn plan_guards_forward_plan_timeout() {
        assert_eq!(plan_guards_for(7).timeout_ms, 7);
        // WHY 断言 0→1:零超时会让计划级熔断立刻触发，所有计划都被截断成空结果
        assert_eq!(plan_guards_for(0).timeout_ms, 1);
    }

    /// 永不完成的执行必须由外层兜底截断，并映射为 Timeout（退出码 4）
    #[tokio::test]
    async fn outer_deadline_fires_on_hung_execution() {
        let err = await_outer_deadline(
            Duration::from_millis(5),
            std::future::pending::<Result<PlanSummary, PlanError>>(),
        )
        .await
        .expect_err("永不完成的 future 必须被外层兜底截断");
        assert!(
            matches!(&err, ChimeraCliError::Timeout(m) if m.contains("外层兜底")),
            "应映射为外层兜底 Timeout，实际: {err:?}"
        );
    }

    /// 正常完成时必须零改写透传，不得因包装丢失字段
    #[tokio::test]
    async fn outer_deadline_is_transparent_on_success() {
        let expected = PlanSummary {
            plan_id: "transparent-plan".into(),
            summary: "ok".into(),
            tool_calls: 1,
            nodes_executed: 1,
            duration_ms: 3,
            truncated: false,
        };
        let got = await_outer_deadline(Duration::from_secs(5), async {
            Ok::<PlanSummary, PlanError>(expected.clone())
        })
        .await
        .expect("成功分支应原样透传");
        assert_eq!(got, expected, "外层兜底不得改写成功结果");
    }

    /// 单操作慢执行必须归因到单操作超时层，而不是外层兜底或计划级熔断
    #[tokio::test]
    async fn op_timeout_attributed_to_single_op_layer() {
        struct SlowExecutor;

        #[async_trait]
        impl ToolExecutor for SlowExecutor {
            async fn execute(&self, _tool: &str, _args: &str) -> Result<String, String> {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok("{}".into())
            }
        }

        let executor = OpTimeoutExecutor {
            inner: Arc::new(SlowExecutor),
            per_op_timeout: Duration::from_millis(10),
        };
        let err = executor
            .execute("text.echo", "{}")
            .await
            .expect_err("慢执行器必须被单操作超时截断");
        assert!(
            err.starts_with("tool op timeout after 10ms"),
            "应归因单操作超时层，实际: {err}"
        );
    }
}
