//! `chimera tool` — tool-plane discovery, single calls and ToolPlan execution.
//!
//! Default-off rollout: `CHIMERA_TOOL_PLANE=observe` previews without executing;
//! `apply` executes. `proc.exec` is a write tool and requires an explicit
//! approval (`--yes` / `--no-permission` or an interactive prompt).

use anyhow::Result;
use gqep_executor::tool_plane::{
    ToolInvocation, ToolOrigin, ToolOutcome, ToolPriority, ToolStatus,
};
use gqep_executor::{PlanRunError, PlanRunOptions};
use nexus_contracts::tool_plan::{guards, SideEffectDecl, ToolPlan};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::cli::ToolAction;
use crate::composition;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;
use crate::permission::{self, PermissionCtx};
use crate::tool_plane::{ToolPlaneConfig, ToolPlaneMode};

/// Execute `chimera tool ...`.
pub async fn execute(
    action: &ToolAction,
    cfg: &ChimeraConfig,
    json: bool,
    perm: &PermissionCtx,
    dry_run: bool,
) -> Result<()> {
    let plane = ToolPlaneConfig::from_env();
    match action {
        ToolAction::List => list(cfg, json, plane).await,
        ToolAction::Call {
            tool,
            args,
            timeout_ms,
        } => call(cfg, json, perm, dry_run, plane, tool, args, *timeout_ms).await,
        ToolAction::Plan { file, max_parallel } => {
            plan(cfg, json, perm, dry_run, plane, file, *max_parallel).await
        }
    }
}

#[derive(Debug, Serialize)]
struct ToolRow<'a> {
    name: &'a str,
    provider: &'a str,
    description: &'a str,
    side_effect: String,
    timeout_ms: u64,
}

#[derive(Debug, Serialize)]
struct ToolListPayload<'a> {
    count: usize,
    tools: &'a [ToolRow<'a>],
}

async fn list(cfg: &ChimeraConfig, json: bool, _plane: ToolPlaneConfig) -> Result<()> {
    let runtime = composition::build_tool_runtime(cfg)?;
    let descriptors = runtime.registry().descriptors();
    let rows: Vec<ToolRow<'_>> = descriptors
        .iter()
        .map(|descriptor| ToolRow {
            name: &descriptor.name,
            provider: &descriptor.provider,
            description: &descriptor.description,
            side_effect: format!("{:?}", descriptor.side_effect),
            timeout_ms: descriptor.timeout_ms,
        })
        .collect();
    let payload = ToolListPayload {
        count: rows.len(),
        tools: &rows,
    };
    output::emit(json, &payload, || {
        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                vec![
                    row.name.to_string(),
                    row.provider.to_string(),
                    row.side_effect.clone(),
                    row.timeout_ms.to_string(),
                    row.description.to_string(),
                ]
            })
            .collect();
        output::print_table(&["工具", "Provider", "副作用", "超时(ms)", "说明"], &table);
        Ok(())
    })
}

#[derive(Debug, Serialize)]
struct ObservePayload<'a> {
    status: &'a str,
    tool: &'a str,
    provider: &'a str,
    side_effect: String,
    would_execute: bool,
}

#[derive(Debug, Serialize)]
struct CallPayload<'a> {
    status: &'a str,
    tool: &'a str,
    provider: &'a str,
    payload: &'a str,
    duration_ms: u64,
    truncated: bool,
}

#[allow(clippy::too_many_arguments)]
async fn call(
    cfg: &ChimeraConfig,
    json: bool,
    perm: &PermissionCtx,
    dry_run: bool,
    plane: ToolPlaneConfig,
    tool: &str,
    args: &str,
    timeout_ms: Option<u64>,
) -> Result<()> {
    require_enabled(plane, "tool call")?;
    let runtime = composition::build_tool_runtime(cfg)?;
    let Some((_provider, descriptor)) = runtime.registry().resolve(tool) else {
        return Err(ChimeraCliError::EngineError(format!("tool not found: {tool}")).into());
    };

    if plane.mode == ToolPlaneMode::Observe || dry_run {
        let payload = ObservePayload {
            status: if dry_run { "dry_run" } else { "observe" },
            tool,
            provider: &descriptor.provider,
            side_effect: format!("{:?}", descriptor.side_effect),
            would_execute: false,
        };
        return output::emit(json, &payload, || {
            output::print_info(&format!(
                "{}: {tool} (provider={}, side_effect={:?}) — 未执行",
                payload.status, descriptor.provider, descriptor.side_effect
            ));
            Ok(())
        });
    }

    let approved = if descriptor.side_effect == SideEffectDecl::Write {
        permission::confirm(perm, "执行工具", &format!("{tool} {args}")).await?
    } else {
        true
    };
    if !approved {
        return Err(ChimeraCliError::PermissionDenied(format!("用户拒绝执行工具 {tool}")).into());
    }

    let invocation = ToolInvocation {
        call_id: format!("cli-{}", Uuid::now_v7()),
        tool: tool.to_string(),
        args_json: args.to_string(),
        timeout_ms,
        priority: ToolPriority::Normal,
        origin: ToolOrigin::Cli,
        quest_id: None,
        task_id: None,
        approved,
    };
    let outcome = runtime.invoke(invocation, &CancellationToken::new()).await;
    emit_outcome(json, &outcome)
}

fn emit_outcome(json: bool, outcome: &ToolOutcome) -> Result<()> {
    if outcome.status == ToolStatus::Ok {
        let payload = outcome.payload.as_deref().unwrap_or("");
        let output_payload = CallPayload {
            status: "ok",
            tool: &outcome.tool,
            provider: &outcome.provider,
            payload,
            duration_ms: outcome.duration_ms,
            truncated: outcome.truncated,
        };
        return output::emit(json, &output_payload, || {
            output::print_success(&format!(
                "{} -> {} ({}ms{})",
                outcome.tool,
                payload,
                outcome.duration_ms,
                if outcome.truncated { ", truncated" } else { "" }
            ));
            Ok(())
        });
    }
    Err(map_tool_error(outcome))
}

fn map_tool_error(outcome: &ToolOutcome) -> anyhow::Error {
    let message = outcome
        .error
        .clone()
        .unwrap_or_else(|| format!("tool failed: {:?}", outcome.status));
    match outcome.status {
        ToolStatus::Denied | ToolStatus::AskRequired => {
            ChimeraCliError::PermissionDenied(message).into()
        }
        ToolStatus::Timeout => ChimeraCliError::Timeout(message).into(),
        ToolStatus::Cancelled => ChimeraCliError::UserCancelled.into(),
        _ => ChimeraCliError::EngineError(message).into(),
    }
}

#[derive(Debug, Serialize)]
struct PlanObservePayload<'a> {
    status: &'a str,
    plan_id: &'a str,
    nodes: usize,
    edges: usize,
    valid: bool,
}

#[derive(Debug, Serialize)]
struct PlanOutput<'a> {
    plan_id: &'a str,
    summary: &'a str,
    tool_calls: usize,
    nodes_executed: usize,
    duration_ms: u64,
    truncated: bool,
}

async fn plan(
    cfg: &ChimeraConfig,
    json: bool,
    perm: &PermissionCtx,
    dry_run: bool,
    plane: ToolPlaneConfig,
    file: &str,
    max_parallel: Option<usize>,
) -> Result<()> {
    require_enabled(plane, "tool plan")?;
    let text = std::fs::read_to_string(file).map_err(|error| {
        ChimeraCliError::ConfigError(format!("读取计划文件失败 {file}: {error}"))
    })?;
    let plan: ToolPlan = serde_json::from_str(&text)
        .map_err(|error| ChimeraCliError::ConfigError(format!("计划 JSON 解析失败: {error}")))?;

    if plane.mode == ToolPlaneMode::Observe || dry_run {
        plan.validate(guards::MAX_STEPS)
            .map_err(|error| ChimeraCliError::ConfigError(format!("计划校验失败: {error}")))?;
        let payload = PlanObservePayload {
            status: if dry_run { "dry_run" } else { "observe" },
            plan_id: &plan.id,
            nodes: plan.nodes.len(),
            edges: plan.edges.len(),
            valid: true,
        };
        return output::emit(json, &payload, || {
            output::print_info(&format!(
                "{}: plan={} nodes={} edges={} valid=true — 未执行",
                payload.status, plan.id, payload.nodes, payload.edges
            ));
            Ok(())
        });
    }

    let runtime = composition::build_tool_runtime(cfg)?;
    // WHY 用 should_skip_prompt:计划路径逐节点问人尚未接线（ADR-232 D6），所以
    // `--yes`/`--no-permission` 是这里唯一的显式放行口；不带旗标时 Write 节点
    // 保持 fail-closed（provider 因 `approved=false` 而拒），与 `tool call` 一致。
    let options = PlanRunOptions {
        max_parallel: max_parallel.unwrap_or(plane.max_parallel_plan),
        timeout_ms: 0,
        fail_fast: false,
        approve_writes: perm.should_skip_prompt(),
        cancel: None,
    };
    let summary = runtime
        .run_plan(&plan, options)
        .await
        .map_err(map_plan_error)?;
    let payload = PlanOutput {
        plan_id: &summary.plan_id,
        summary: &summary.summary,
        tool_calls: summary.tool_calls,
        nodes_executed: summary.nodes_executed,
        duration_ms: summary.duration_ms,
        truncated: summary.truncated,
    };
    output::emit(json, &payload, || {
        output::print_success(&format!(
            "plan {}: {} nodes, {} tool calls, {}ms{}",
            payload.plan_id,
            payload.nodes_executed,
            payload.tool_calls,
            payload.duration_ms,
            if payload.truncated { ", truncated" } else { "" }
        ));
        Ok(())
    })
}

fn map_plan_error(error: PlanRunError) -> anyhow::Error {
    let message = error.to_string();
    match error {
        PlanRunError::Invalid(_) => ChimeraCliError::ConfigError(message).into(),
        PlanRunError::TimedOut { .. } => ChimeraCliError::Timeout(message).into(),
        PlanRunError::Cancelled { .. } => ChimeraCliError::UserCancelled.into(),
        PlanRunError::NodeFailed { .. } => ChimeraCliError::EngineError(message).into(),
    }
}

fn require_enabled(plane: ToolPlaneConfig, command: &str) -> Result<()> {
    if plane.mode == ToolPlaneMode::Off {
        return Err(ChimeraCliError::ConfigError(format!(
            "{command} 未启用：设置 CHIMERA_TOOL_PLANE=observe 预演，或 =apply 执行"
        ))
        .into());
    }
    Ok(())
}
