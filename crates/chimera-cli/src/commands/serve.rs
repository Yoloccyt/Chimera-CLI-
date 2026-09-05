//! `chimera serve` — 协议宿主服务（WI-01 serve 形态）
//!
//! # 形态
//! AppServer + StdinTransport 的事件循环：从 stdin 读取 JSON-RPC v1 请求帧
//! （`app.op`），经 AppServer 处理，事件以 `app.event` 推送帧写 stdout。
//!
//! # 客户端接入（WI-01 验收）
//! 任意宿主（TUI/CLI/ACP/未来 IDE）经协议接入：
//! ```text
//! 客户端 ──(app.op 请求帧)──▶ chimera serve ──▶ AppServer ──▶ 核心
//! 客户端 ◀──(app.event 推送帧)── chimera serve ◀── AppServer ◀── 核心
//! ```
//!
//! # stdout 纪律（WI-02 同源）
//! stdout 仅协议帧（NDJSON）；日志/进度全走 stderr（tracing 默认）。
//!
//! # C1（2026-09-04）：真实核心 + 优雅关闭
//! - 装配走 [`crate::composition`]（QuestBackend 真实引擎，替代 InMemory 桩）；
//! - 事件循环外套 `tokio::select!{ ctrl_c }`：宿主进程 Ctrl+C 时优雅退出
//!   （此前仅 EOF 退出，Ctrl+C 硬杀，AppServer 内存会话无清理路径 F-A2-6）。

use anyhow::Result;
use nexus_app_server::{AppServer, AppTransport, StdinTransport, TransportError};

use crate::config::ChimeraConfig;

/// 执行 serve 命令 — 事件循环直至 EOF（客户端断开）或 Ctrl+C（优雅关闭）
pub async fn execute(config: &ChimeraConfig) -> Result<()> {
    tracing::info!("chimera serve: 协议宿主启动（JSON-RPC v1 over stdio）");

    // C1(2026-09-04): 真实核心装配——组合根集中构造 AppContext，
    // QuestBackend 包装 L9 QuestEngine（此前 AppServer::new 默认 InMemoryBackend，
    // 协议宿主对外承诺与实际能力脱节，F-A2-4）。
    let ctx = crate::composition::build(config)?;
    tracing::info!(
        version = %config.nexus.version,
        "chimera serve: 真实核心装配完成（QuestBackend + critical 旁路订阅，C1/C12）"
    );
    let server = crate::composition::build_app_server(ctx);
    let transport = StdinTransport::new();

    // C1: 优雅关闭——正常 EOF 退出与 Ctrl+C 信号竞争；
    // AppServer 会话态为内存态，select 分支返回即随进程释放（无落盘清理需求）。
    tokio::select! {
        r = serve_loop(&server, &transport) => r,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("chimera serve: 收到 Ctrl+C，优雅退出（内存会话随进程释放）");
            Ok(())
        }
    }
}

/// 事件循环主体 — 逐帧接收/处理/推送（EOF 即正常结束）
///
/// # 参数
/// - `server`: 组合根装配的协议宿主（真实核心后端）
/// - `transport`: 传输层抽象（`&dyn AppTransport` 以便用 MockTransport 无 stdio 驱动测试）
async fn serve_loop(server: &AppServer, transport: &dyn AppTransport) -> Result<()> {
    loop {
        // 接收客户端操作（EOF = 客户端断开，正常退出）
        let op = match transport.recv_op().await {
            Ok(op) => op,
            Err(TransportError::Eof) => {
                tracing::info!("chimera serve: 客户端断开（EOF），正常退出");
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(error = %e, "chimera serve: 传输错误，继续等待下一帧");
                continue;
            }
        };

        // 处理操作 → 事件推送（内闭外开：AppOp/AppEvent 协议面）
        match server.handle_op(&op).await {
            Ok(events) => {
                for ev in &events {
                    if let Err(e) = transport.send_event(ev).await {
                        tracing::warn!(error = %e, "chimera serve: 事件推送失败");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "chimera serve: 操作处理失败");
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
    use crate::commands::testutil::MockTransport;
    use nexus_contracts::app::{AppOp, ThreadId, ThreadStartParams};

    #[test]
    fn serve_uses_stdlib_transport() {
        // 编译期断言: serve 依赖 StdinTransport 满足 AppTransport 约束
        fn assert_transport<T: nexus_app_server::AppTransport>() {}
        assert_transport::<StdinTransport>();
    }

    /// T-1：serve_loop 逐帧处理（ThreadStart→TurnSubmit）后遇 EOF 正常返回 Ok，
    /// 且把 ThreadStart 产出事件经 send_event 推送（覆盖新增循环体与 EOF 退出，G7）。
    /// TurnSubmit 用未知 thread 以顺带验证“处理失败不中断循环”的降级路径。
    #[tokio::test]
    async fn serve_loop_processes_frames_and_stops_on_eof() {
        let ctx = crate::composition::build(&ChimeraConfig::default()).expect("装配应成功");
        let server = crate::composition::build_app_server(ctx);
        let ops = vec![
            AppOp::ThreadStart(ThreadStartParams::new("g1", "r1")),
            AppOp::TurnSubmit {
                thread_id: ThreadId::new("g1"),
                input: nexus_contracts::app::UserInput {
                    text: "分析依赖".into(),
                    extras: None,
                },
            },
        ];
        let transport = MockTransport::new(ops);
        serve_loop(&server, &transport)
            .await
            .expect("EOF 应使循环正常退出 Ok");
        let kinds = transport.sent_kinds();
        assert!(
            kinds.iter().any(|k| k == "thread_started"),
            "serve_loop 应将 ThreadStart 事件推送到传输层；实际 kinds: {kinds:?}"
        );
    }
}
