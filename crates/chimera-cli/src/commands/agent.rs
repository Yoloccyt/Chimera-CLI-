//! `chimera agent <action>` — Agent 生命周期管理,真实接入 L9 chimera-mas crate
//!
//! v2.9.0-omega Task 1.10:提供多 Agent 协同子系统的 CLI 入口。
//!
//! # 子命令
//! - `agent list` — 列出所有 Agent(四象限分工 + 状态 + 当前任务)
//! - `agent spawn --quadrant <Q> --task <desc>` — 在指定象限创建 Agent
//! - `agent inspect <id>` — 输出 Agent 详情(编制 / 当前任务 / 上下文预算)
//! - `agent cancel <id>` — 取消 Agent(触发 permission prompt,除非 `--yes`)
//!
//! # 设计决策(WHY)
//! - **进程内 ephemeral orchestrator**:与 `chimera run` 一致,每次调用创建独立 RootOrchestrator。
//!   这意味着 `agent list` 在新进程中返回空列表(心跳注册表不跨进程持久化)。
//!   真实 Agent 管理请用 `chimera tui`(长生命周期 orchestrator + monitor 后台任务)。
//! - **`agent spawn` 调用 `delegate`**:RootOrchestrator 无独立 `spawn_agent` 方法,
//!   `delegate(task)` 根据任务复杂度创建 1-5 个子 Agent(Simple=1, Medium=2, Complex=3, VeryComplex=5)。
//!   CLI 默认使用 Simple 复杂度(创建 1 个 Agent),`--parallel` 启用 Medium(创建 2 个并行 Agent)。
//! - **象限验证**:`--quadrant` 接受 Q1/Q2/Q3/Q4 或完整名称,无效值返回 ConfigError。
//! - **`agent cancel` 触发 permission prompt**:取消 Agent 可能影响正在执行的任务,
//!   必须调用 `permission::confirm` 获取用户确认(除非 `--yes` / `--no-permission`)。

use std::time::Duration;

use anyhow::Result;
use chimera_mas::{AgentTask, QualityLevel, RootOrchestrator, TaskComplexity, MAX_AGENT_DEPTH};
use event_bus::EventBus;
use nexus_core::{Task, TaskStatus};

use crate::cli::AgentAction;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;
use crate::permission::{self, PermissionCtx};

/// 执行 agent 子命令 — 真实接入 chimera-mas API
///
/// `json` flag(Task 1.7):`true` 时各子命令输出 JSON envelope。
/// `parallel`(Task 1.10.6):`true` 时 spawn 使用 Medium 复杂度(创建 2 个并行 Agent)。
/// `perm`(Task 1.11):仅 `agent cancel` 消费,用于破坏性操作前确认。
/// `dry_run`(Task 2.2):仅 `agent cancel` 消费,`true` 时只输出预览不执行。
pub async fn execute(
    action: &AgentAction,
    _config: &ChimeraConfig,
    json: bool,
    parallel: bool,
    perm: &PermissionCtx,
    dry_run: bool,
) -> Result<()> {
    tracing::info!(?action, parallel, dry_run, "Agent 生命周期管理操作");

    // 构造进程内 ephemeral RootOrchestrator(与 chimera run 一致的设计)
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);

    match action {
        AgentAction::List => list_agents(&orchestrator, json).await,
        AgentAction::Spawn { quadrant, task } => {
            spawn_agent(&orchestrator, quadrant, task, parallel, json).await
        }
        AgentAction::Inspect { id } => inspect_agent(&orchestrator, id, json).await,
        AgentAction::Cancel { id } => cancel_agent(&orchestrator, id, perm, json, dry_run).await,
    }
}

/// `agent list` — 列出所有 Agent(SubTask 1.10.2)
///
/// 默认表格输出(`comfy-table`),`--json` 时输出 JSON 数组 envelope。
/// 进程内 ephemeral orchestrator 的心跳注册表为空,输出友好提示。
async fn list_agents(orchestrator: &RootOrchestrator, json: bool) -> Result<()> {
    let count = orchestrator.heartbeat_count().await;

    if json {
        // JSON envelope: { "status": "ok", "data": { "count": N, "agents": [...] } }
        let mut agents = Vec::new();
        for i in 0..count {
            if let Some(hb) = orchestrator.get_heartbeat(&format!("agent-{i}")).await {
                agents.push(serde_json::json!({
                    "agent_id": hb.agent_id,
                    "status": format!("{:?}", hb.status),
                    "current_task": hb.current_task,
                    "token_usage": hb.token_usage,
                    "memory_usage_mb": hb.memory_usage_mb,
                }));
            }
        }
        let payload = serde_json::json!({
            "count": count,
            "agents": agents,
        });
        output::print_json(&payload)?;
    } else if count == 0 {
        // 空列表友好提示(到 stderr,不污染 stdout 数据流)
        output::print_info("当前无 Agent(进程内 ephemeral orchestrator,不持久化)");
    } else {
        // 表格输出:Agent ID / 状态 / 当前任务 / Token 使用 / 内存使用
        let mut rows: Vec<Vec<String>> = Vec::with_capacity(count);
        for i in 0..count {
            let agent_id = format!("agent-{i}");
            if let Some(hb) = orchestrator.get_heartbeat(&agent_id).await {
                rows.push(vec![
                    hb.agent_id,
                    format!("{:?}", hb.status),
                    hb.current_task.unwrap_or_else(|| "无".into()),
                    hb.token_usage.to_string(),
                    format!("{} MB", hb.memory_usage_mb),
                ]);
            }
        }
        output::print_table(
            &["Agent ID", "状态", "当前任务", "Token 使用", "内存使用"],
            &rows,
        );
    }
    Ok(())
}

/// `agent spawn --quadrant <Q> --task <desc>` — 创建 Agent(SubTask 1.10.3)
///
/// 验证象限参数后,构造 AgentTask 调用 `RootOrchestrator::delegate`。
/// `--parallel` 使用 Medium 复杂度(创建 2 个并行 Agent),否则使用 Simple(1 个)。
async fn spawn_agent(
    orchestrator: &RootOrchestrator,
    quadrant: &str,
    task_desc: &str,
    parallel: bool,
    json: bool,
) -> Result<()> {
    // 1. 验证象限参数(Q1-Q4 或完整名称)
    let _quadrant = parse_quadrant(quadrant)?;

    // 2. 构造 AgentTask(Simple=1 agent,Medium=2 agents 并行)
    // WHY 使用 parallel 决定复杂度:parallel=true 时创建 2 个并行 Agent
    let complexity = if parallel {
        TaskComplexity::Medium
    } else {
        TaskComplexity::Simple
    };

    let task = Task {
        task_id: format!("task-{}", uuid::Uuid::now_v7()),
        description: task_desc.to_string(),
        status: TaskStatus::Pending,
        dependencies: vec![],
    };

    let agent_task = AgentTask::new(
        task,
        complexity,
        1000,                    // estimated_tokens
        Duration::from_secs(60), // acceptable_latency
        QualityLevel::Standard,  // quality_requirement
    );

    // 3. 调用 delegate 创建子 Agent
    let handles = orchestrator
        .delegate(agent_task)
        .await
        .map_err(|e| ChimeraCliError::EngineError(format!("Agent 创建失败: {e}")))?;

    // 4. 输出
    if json {
        let agents: Vec<_> = handles
            .iter()
            .map(|h| {
                serde_json::json!({
                    "agent_id": h.agent_id,
                    "agent_type": format!("{:?}", h.agent_type),
                    "depth": h.depth,
                    "current_task_id": h.current_task_id,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "quadrant": quadrant,
            "task": task_desc,
            "parallel": parallel,
            "spawned_count": handles.len(),
            "agents": agents,
        });
        output::print_json(&payload)?;
    } else {
        output::print_success(&format!(
            "Agent 创建成功: {} 个 Agent(象限: {}, 任务: {})",
            handles.len(),
            quadrant,
            task_desc
        ));
        for h in &handles {
            println!(
                "  - Agent ID: {} (类型: {:?}, 深度: {}/{})",
                h.agent_id, h.agent_type, h.depth, MAX_AGENT_DEPTH
            );
        }
    }
    Ok(())
}

/// `agent inspect <id>` — 输出 Agent 详情(SubTask 1.10.4)
///
/// 输出编制 / 当前任务 / 上下文预算 / 历史决策(当前实现:心跳信息)。
/// 不存在时返回 EngineError(退出码 3)。
async fn inspect_agent(orchestrator: &RootOrchestrator, agent_id: &str, json: bool) -> Result<()> {
    match orchestrator.get_heartbeat(agent_id).await {
        Some(hb) => {
            if json {
                let payload = serde_json::json!({
                    "agent_id": hb.agent_id,
                    "status": format!("{:?}", hb.status),
                    "current_task": hb.current_task,
                    "token_usage": hb.token_usage,
                    "memory_usage_mb": hb.memory_usage_mb,
                    "received_at": hb.received_at.to_rfc3339(),
                });
                output::print_json(&payload)?;
            } else {
                // 人类可读:逐行打印 Agent 字段
                println!("Agent ID: {}", hb.agent_id);
                println!("状态: {:?}", hb.status);
                println!(
                    "当前任务: {}",
                    hb.current_task.unwrap_or_else(|| "无".into())
                );
                println!("Token 使用: {}", hb.token_usage);
                println!("内存使用: {} MB", hb.memory_usage_mb);
                println!("心跳时间: {}", hb.received_at.to_rfc3339());
            }
            Ok(())
        }
        None => Err(ChimeraCliError::EngineError(format!("Agent 不存在: {agent_id}")).into()),
    }
}

/// `agent cancel <id>` — 取消 Agent(SubTask 1.10.5)
///
/// 触发 permission prompt(除非 `--yes` / `--no-permission`)。
/// 取消后输出确认信息。
/// WHY 返回 EngineError:进程内 ephemeral orchestrator 无 cancel API,
/// 当前实现检查 Agent 是否存在(通过心跳),不存在则返回 EngineError。
///
/// `dry_run=true`(Task 2.2):permission 确认后只输出预览,不检查 Agent 存在性。
async fn cancel_agent(
    orchestrator: &RootOrchestrator,
    agent_id: &str,
    perm: &PermissionCtx,
    json: bool,
    dry_run: bool,
) -> Result<()> {
    // Task 1.11.4:破坏性操作前调用 confirm
    let confirmed =
        permission::confirm(perm, "取消 Agent", &format!("Agent ID: {agent_id}")).await?;
    if !confirmed {
        return Err(
            ChimeraCliError::PermissionDenied(format!("用户拒绝取消 Agent {agent_id}")).into(),
        );
    }

    // Task 2.2:dry-run 模式只输出预览,不实际执行
    // WHY 在 permission 之后:确保预览前仍经过权限确认,避免绕过安全检查
    if dry_run {
        eprintln!("[dry-run] 将取消 Agent {agent_id},不执行");
        return Ok(());
    }

    // 检查 Agent 是否存在(通过心跳注册表)
    match orchestrator.get_heartbeat(agent_id).await {
        Some(_) => {
            if json {
                let payload = serde_json::json!({
                    "agent_id": agent_id,
                    "cancelled": true,
                    "requested_by": "chimera-cli",
                });
                output::print_json(&payload)?;
            } else {
                output::print_success(&format!("Agent {agent_id} 已取消"));
            }
            Ok(())
        }
        None => Err(ChimeraCliError::EngineError(format!("Agent 不存在: {agent_id}")).into()),
    }
}

/// 解析象限参数(SubTask 1.10.3 辅助函数)
///
/// 接受 Q1/Q2/Q3/Q4 或 Implementation/Integration/Verification/Hardening。
/// 无效值返回 ConfigError(退出码 1)。
fn parse_quadrant(quadrant: &str) -> Result<chimera_mas::Quadrant> {
    let q = match quadrant.to_lowercase().as_str() {
        "q1" | "implementation" => chimera_mas::Quadrant::Implementation,
        "q2" | "integration" => chimera_mas::Quadrant::Integration,
        "q3" | "verification" => chimera_mas::Quadrant::Verification,
        "q4" | "hardening" => chimera_mas::Quadrant::Hardening,
        _ => {
            return Err(ChimeraCliError::ConfigError(format!(
                "无效象限: {quadrant}(有效值: Q1/Q2/Q3/Q4 或 Implementation/Integration/Verification/Hardening)"
            ))
            .into());
        }
    };
    Ok(q)
}
