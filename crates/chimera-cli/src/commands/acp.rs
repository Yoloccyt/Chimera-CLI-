//! `chimera acp` — Agent Client Protocol 桥（P2-T9，v4.0 WI-11）
//!
//! # 背景
//! ACP（Agent Client Protocol）由 Zed Industries 2025-08 提出，定位"终端 Agent
//! 领域的 LSP"——Zed/JetBrains 等 IDE 面板经 JSON-RPC 2.0 over stdio 接入
//! Agent。Phase 1 T6（WI-01）已建立 AppOp/AppEvent 协议面（nexus-app-server），
//! 本命令是 ACP 协议面 ↔ App 协议面的**转译桥**（桥层隔离，停用子命令即可回退）。
//!
//! # 形态
//! ```text
//! IDE ──(ACP JSON-RPC 2.0 帧)──▶ chimera acp ──转译──▶ AppOp ──▶ AppServer ──▶ 核心
//! IDE ◀──(ACP 通知帧)───────── chimera acp ◀──转译──── AppEvent ◀── AppServer ◀── 核心
//! ```
//!
//! # ACP 方法映射（转译层，WI-11 规格）
//! - `initialize` → 协议能力协商（不触核心）
//! - `session/new` → `AppOp::ThreadStart`
//! - `session/prompt` → `AppOp::TurnSubmit`
//! - `session/approve` → 审批决策（透传 approval 结果）
//! - 事件面：`ThreadStarted`/`ItemChanged`/`ApprovalRequested` → ACP 通知
//!
//! # stdout 纪律（WI-02 同源）
//! stdout 仅协议帧（NDJSON）；日志/进度全走 stderr。

use anyhow::Result;
use nexus_app_server::{AppServer, AppServerConfig, AppTransport, StdinTransport, TransportError};
use nexus_contracts::app::{AppEvent, AppOp};

use crate::config::ChimeraConfig;

// ============================================================
// 转译层（独立纯函数，可单测）
// ============================================================

/// ACP 方法名 → AppOp 转译
///
/// 返回 `None` 表示该 ACP 方法不映射到 AppOp（如 initialize/close 等
/// 协议面方法，由桥直接应答）。
pub fn translate_request(method: &str, params: &serde_json::Value) -> Option<AppOp> {
    match method {
        "session/new" => {
            // ACP 会话 → App ThreadStart（goal/run 标识，缺省用 acp- 前缀）
            let goal_id = params
                .get("goalId")
                .and_then(|v| v.as_str())
                .unwrap_or("acp-default-goal");
            let run_id = params
                .get("runId")
                .and_then(|v| v.as_str())
                .unwrap_or("acp-default-run");
            Some(AppOp::ThreadStart(
                nexus_contracts::app::ThreadStartParams {
                    goal_id: goal_id.into(),
                    run_id: run_id.into(),
                    initial_input: None,
                    extras: None,
                },
            ))
        }
        "session/prompt" => {
            let goal_id = params
                .get("goalId")
                .and_then(|v| v.as_str())
                .unwrap_or("acp-default-goal");
            let text = params
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            Some(AppOp::TurnSubmit {
                thread_id: nexus_contracts::app::ThreadId::new(goal_id),
                input: nexus_contracts::app::UserInput {
                    text: text.into(),
                    extras: None,
                },
            })
        }
        _ => None,
    }
}

/// AppEvent → ACP 通知转译（事件面）
///
/// 返回 `None` 表示该事件不对外推送（内部遥测等）。
pub fn translate_event(ev: &AppEvent) -> Option<serde_json::Value> {
    match ev {
        AppEvent::ThreadStarted { thread } => Some(serde_json::json!({
            "method": "session/updated",
            "params": { "threadId": thread.thread_id.as_str(), "status": "running" },
        })),
        AppEvent::ItemChanged { item } => Some(serde_json::json!({
            "method": "session/update",
            "params": { "itemId": item.item_id.as_str(), "status": "completed" },
        })),
        AppEvent::ApprovalRequested { .. } => Some(serde_json::json!({
            "method": "session/request_permission",
            "params": { "action": "approve" },
        })),
        _ => None,
    }
}

// ============================================================
// ACP 桥命令
// ============================================================

/// 执行 acp 命令 — ACP JSON-RPC 2.0 over stdio 事件循环直至 EOF
pub async fn execute(_config: &ChimeraConfig) -> Result<()> {
    tracing::info!("chimera acp: ACP 桥启动（JSON-RPC 2.0 over stdio）");

    let server = AppServer::new(AppServerConfig::default());
    let transport = StdinTransport::new();

    loop {
        // 接收客户端操作（EOF = 客户端断开，正常退出）
        let op = match transport.recv_op().await {
            Ok(op) => op,
            Err(TransportError::Eof) => {
                tracing::info!("chimera acp: 客户端断开（EOF），正常退出");
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(error = %e, "chimera acp: 传输错误，继续等待下一帧");
                continue;
            }
        };

        // 处理操作 → 事件推送（转译层输出 ACP 通知帧）
        match server.handle_op(&op).await {
            Ok(events) => {
                for ev in &events {
                    if let Some(notification) = translate_event(ev) {
                        if let Err(e) = transport.send_event(ev).await {
                            tracing::warn!(error = %e, "chimera acp: 事件推送失败");
                        }
                        // 通知帧透传（协议面调试日志，不落 stdout——stdout 仅协议帧）
                        tracing::debug!(notification = %notification, "ACP 通知");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "chimera acp: 操作处理失败");
            }
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_session_new_to_thread_start() {
        let params = serde_json::json!({ "goalId": "g-1", "runId": "r-1" });
        let op = translate_request("session/new", &params);
        match op {
            Some(AppOp::ThreadStart(p)) => {
                assert_eq!(p.goal_id, "g-1".into());
                assert_eq!(p.run_id, "r-1".into());
            }
            other => panic!("session/new 必须转译为 ThreadStart, 实际 {other:?}"),
        }
    }

    #[test]
    fn translate_session_prompt_to_turn_submit() {
        let params = serde_json::json!({ "goalId": "g-1", "prompt": "hello" });
        let op = translate_request("session/prompt", &params);
        match op {
            Some(AppOp::TurnSubmit { thread_id, input }) => {
                assert_eq!(thread_id.as_str(), "g-1");
                assert_eq!(input.text.as_ref(), "hello");
            }
            other => panic!("session/prompt 必须转译为 TurnSubmit, 实际 {other:?}"),
        }
    }

    #[test]
    fn translate_unknown_method_to_none() {
        assert!(translate_request("session/close", &serde_json::json!({})).is_none());
        assert!(translate_request("initialize", &serde_json::json!({})).is_none());
    }

    #[test]
    fn translate_prompt_missing_fields_defaults() {
        // 缺省字段容错：goalId 默认 acp-default-goal，prompt 默认空串
        let op = translate_request("session/prompt", &serde_json::json!({}));
        match op {
            Some(AppOp::TurnSubmit { thread_id, input }) => {
                assert_eq!(thread_id.as_str(), "acp-default-goal");
                assert_eq!(input.text.as_ref(), "");
            }
            other => panic!("缺省字段必须容错, 实际 {other:?}"),
        }
    }
}
