//! TUI ↔ 编排器协议握手服务端 — Concord W10 T10.2(ADR-082)
//!
//! 对应架构层:L10 Interface(chimera-cli)
//!
//! # 核心职责
//! - 订阅共享 `EventBus`,响应 TUI 启动时发布的 `TuiHello`
//! - 按协议版本协商兼容级别(`negotiate` 纯函数),发布 `TuiHelloAck`
//! - **SEC-4 一次性握手**:仅信道建立初期的首个 `TuiHello` 被应答;
//!   运行期到达的握手帧丢弃并留审计日志(防伪造 Refused 的 DoS 弱化攻击)
//!
//! # 设计决策(WHY)
//! - 防 Codex #37536 式版本偏移静默故障:陈旧后端存活时新 TUI 必须
//!   **显式**得知兼容状态,而非静默缺失功能。
//! - 协商纯函数化(`negotiate`):版本比较逻辑与事件发布解耦,单测全覆盖。
//! - 复用既有 EventBus 通道(零新进程外协议,方案 §7.1 裁决)。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use event_bus::{CompatLevel, EventBus, EventBusError, EventMetadata, NexusEvent};
use tokio::task::JoinHandle;

/// 事件来源标识
const SOURCE: &str = "chimera-cli";

/// 服务端声明的握手协议版本(semver)
pub const TUI_PROTO_VERSION: &str = "1.0.0";

/// 解析 semver 字符串为 (major, minor, patch);非法返回 None
pub fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // 多余段视为非法
    }
    Some((major, minor, patch))
}

/// 协议版本协商(纯函数,ADR-082 决策)
///
/// # 规则
/// - 任一侧版本不可解析 → `Refused`(无法证明兼容,显式拒绝优于静默);
/// - 主版本不同 → `Refused`(协议断裂);
/// - 客户端次版本高于服务端 → `Degraded`(服务端缺新能力,列出降级项);
/// - 其余(服务端 ≥ 客户端)→ `Full`。
pub fn negotiate(client_proto: &str, server_proto: &str) -> CompatLevel {
    let (Some(client), Some(server)) = (parse_semver(client_proto), parse_semver(server_proto))
    else {
        return CompatLevel::Refused;
    };
    if client.0 != server.0 {
        return CompatLevel::Refused;
    }
    if client.1 > server.1 {
        return CompatLevel::Degraded(vec![format!(
            "server protocol {}.{} older than client {}.{}",
            server.0, server.1, client.0, client.1
        )]);
    }
    CompatLevel::Full
}

/// 构造握手应答事件(纯函数,测试可直接断言载荷)
pub fn build_ack(client_proto: &str) -> NexusEvent {
    NexusEvent::TuiHelloAck {
        metadata: EventMetadata::new(SOURCE),
        proto: TUI_PROTO_VERSION.into(),
        compat: negotiate(client_proto, TUI_PROTO_VERSION),
        server_version: env!("CARGO_PKG_VERSION").into(),
    }
}

/// 启动握手应答器,返回可 abort 的任务句柄
///
/// # 生命周期(§4.4 反模式 #3 / #7)
/// - **subscribe-before-spawn**:在 `tokio::spawn` 前同步订阅,不错过启动瞬间的握手;
/// - **SEC-4 一次性**:`AtomicBool` 记录已应答状态,后续 `TuiHello` 仅留
///   warn 审计日志后丢弃(不再应答,防运行期伪造握手帧篡改兼容状态);
/// - 调用方负责退出时 `abort()` 句柄,避免 orphan task。
pub fn spawn_handshake_responder(bus: EventBus) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        let answered = Arc::new(AtomicBool::new(false));
        loop {
            match rx.recv().await {
                Ok(NexusEvent::TuiHello { proto, caps, .. }) => {
                    if answered.swap(true, Ordering::SeqCst) {
                        // SEC-4:运行期重复握手帧 — 丢弃并留审计痕迹
                        tracing::warn!(
                            proto = %proto,
                            caps_count = caps.len(),
                            "重复 TuiHello 握手帧被丢弃(SEC-4:仅信道建立初期接受一次)"
                        );
                        continue;
                    }
                    let ack = build_ack(&proto);
                    if let Err(e) = bus.publish(ack).await {
                        tracing::warn!(error = %e, "TuiHelloAck 发布失败,TUI 将按未知兼容降级");
                    } else {
                        tracing::info!(client_proto = %proto, "TUI 协议握手完成");
                    }
                }
                Ok(_) => {}
                Err(EventBusError::SlowConsumerDropped { lag, .. }) => {
                    tracing::warn!(lag, "握手应答器接收滞后,丢弃部分事件后继续");
                }
                Err(_) => break, // ChannelClosed:退出
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_semver_valid_and_invalid() {
        assert_eq!(parse_semver("1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_semver("2.26.3"), Some((2, 26, 3)));
        assert_eq!(parse_semver("1.0"), None);
        assert_eq!(parse_semver("1.0.0.0"), None);
        assert_eq!(parse_semver("a.b.c"), None);
        assert_eq!(parse_semver(""), None);
    }

    #[test]
    fn negotiate_full_when_server_at_least_client() {
        assert_eq!(negotiate("1.0.0", "1.0.0"), CompatLevel::Full);
        assert_eq!(negotiate("1.0.0", "1.2.0"), CompatLevel::Full);
    }

    #[test]
    fn negotiate_degraded_when_client_newer_minor() {
        match negotiate("1.3.0", "1.0.0") {
            CompatLevel::Degraded(items) => {
                assert_eq!(items.len(), 1, "应携带一条降级项");
            }
            other => panic!("应为 Degraded,实际 {other:?}"),
        }
    }

    #[test]
    fn negotiate_refused_on_major_mismatch_or_invalid() {
        assert_eq!(negotiate("2.0.0", "1.0.0"), CompatLevel::Refused);
        assert_eq!(negotiate("1.0.0", "2.0.0"), CompatLevel::Refused);
        assert_eq!(negotiate("broken", "1.0.0"), CompatLevel::Refused);
        assert_eq!(negotiate("1.0.0", ""), CompatLevel::Refused);
    }

    #[test]
    fn build_ack_carries_compat_and_versions() {
        let ack = build_ack("1.0.0");
        match ack {
            NexusEvent::TuiHelloAck {
                proto,
                compat,
                server_version,
                ..
            } => {
                assert_eq!(proto, TUI_PROTO_VERSION);
                assert_eq!(compat, CompatLevel::Full);
                assert!(!server_version.is_empty());
            }
            other => panic!("应为 TuiHelloAck,实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn responder_answers_once_and_drops_repeats() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let handle = spawn_handshake_responder(bus.clone());

        // 首次握手 → 应答
        bus.publish(NexusEvent::TuiHello {
            metadata: EventMetadata::new("chimera-tui"),
            proto: "1.0.0".into(),
            tui_version: "2.26.0".into(),
            caps: vec!["orchestrated-commands".into()],
        })
        .await
        .expect("publish hello");
        let ack = wait_for_ack(&mut rx).await;
        assert!(matches!(
            ack,
            NexusEvent::TuiHelloAck {
                compat: CompatLevel::Full,
                ..
            }
        ));

        // 重复握手 → 不再应答(SEC-4):再发一次,只应收到自身发布的 TuiHello
        bus.publish(NexusEvent::TuiHello {
            metadata: EventMetadata::new("attacker"),
            proto: "9.9.9".into(),
            tui_version: "0.0.1".into(),
            caps: vec![],
        })
        .await
        .expect("publish repeat hello");
        // 排空信道:应收到第二个 TuiHello(broadcast 回环)而无第二个 Ack
        let mut saw_second_ack = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(300);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(NexusEvent::TuiHelloAck { .. })) => saw_second_ack = true,
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
        assert!(!saw_second_ack, "SEC-4:重复握手帧不得被应答");
        handle.abort();
    }

    /// 从接收端等待首个 TuiHelloAck(跳过其它广播事件)
    async fn wait_for_ack(rx: &mut event_bus::EventReceiver) -> NexusEvent {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let remaining = deadline - tokio::time::Instant::now();
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(ev @ NexusEvent::TuiHelloAck { .. })) => return ev,
                Ok(Ok(_)) => continue,
                _ => panic!("等待 TuiHelloAck 超时"),
            }
        }
    }
}
