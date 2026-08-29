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

use anyhow::Result;
use nexus_app_server::{AppServer, AppServerConfig, AppTransport, StdinTransport, TransportError};

use crate::config::ChimeraConfig;

/// 执行 serve 命令 — 事件循环直至 EOF（客户端断开）
pub async fn execute(_config: &ChimeraConfig) -> Result<()> {
    tracing::info!("chimera serve: 协议宿主启动（JSON-RPC v1 over stdio）");

    let server = AppServer::new(AppServerConfig::default());
    let transport = StdinTransport::new();

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

    #[test]
    fn serve_uses_stdlib_transport() {
        // 编译期断言: serve 依赖 StdinTransport 满足 AppTransport 约束
        fn assert_transport<T: nexus_app_server::AppTransport>() {}
        assert_transport::<StdinTransport>();
    }
}
