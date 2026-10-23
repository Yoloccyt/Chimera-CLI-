//! commit_phase 并发扇出验收（性能重构 P1-1）。
//!
//! WHY 独立文件：旧 `commit_phase` 文档称「FuturesUnordered 并发」但实为串行 `for`，
//! 本文件锁死"真并发"这一新契约——用**峰值在飞数**（确定性）而非墙钟（易 flaky）证明扇出，
//! 并验证失败范围最小化与全参与者被尝试（与旧串行可观测语义一致）。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mcp_mesh::{
    McpMesh, MeshConfig, MeshServer, MockParticipantClient, MockPhase, ParticipantClient,
};

/// 跟踪 commit 阶段最大并发在飞数的测试客户端。
///
/// `commit` future 首帧即 `fetch_add` 并记录峰值，随后 sleep（模拟网络往返）。
/// `FuturesUnordered` 会把所有 commit future poll 到各自首个 await 点，故并发时
/// 峰值 == N（串行时 == 1）——该判定与调度时序无关，确定性可断言。
#[derive(Default)]
struct InflightClient {
    inflight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl ParticipantClient for InflightClient {
    fn prepare<'a>(
        &'a self,
        _s: &'a MeshServer,
        _t: &'a str,
        _op: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), mcp_mesh::McpError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn commit<'a>(
        &'a self,
        _s: &'a MeshServer,
        _t: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), mcp_mesh::McpError>> + Send + 'a>> {
        let inflight = Arc::clone(&self.inflight);
        let peak = Arc::clone(&self.peak);
        Box::pin(async move {
            let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(cur, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(25)).await;
            inflight.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn rollback<'a>(
        &'a self,
        _s: &'a MeshServer,
        _t: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), mcp_mesh::McpError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn query<'a>(
        &'a self,
        _s: &'a MeshServer,
        _q: &'a str,
        _t: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, mcp_mesh::McpError>> + Send + 'a>> {
        Box::pin(async { Ok("ok".to_string()) })
    }
}

fn register_servers(mesh: &McpMesh, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let id = format!("s-{i}");
            // RFC 5737 TEST-NET-3 地址绕过 SSRF 校验（与既有集成测试同法）
            mesh.register_server(MeshServer::new(
                &id,
                format!("203.0.113.1:8{i}"),
                vec!["cap-1".into()],
            ))
            .expect("注册应成功");
            id
        })
        .collect()
}

/// 并发扇出证明：commit 峰值在飞数 == 参与者数（串行时只可能 == 1）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_phase_fanout_is_concurrent() {
    const N: usize = 5;
    let mut cfg = MeshConfig::default();
    // 宽松事务超时，避免并发下 5×25ms 触发 TransactionTimeout flaky（生产 200ms 语义不变）
    cfg.transaction_timeout_ms = 5000;
    let client = Arc::new(InflightClient::default());
    let mesh = McpMesh::with_participant_client(
        cfg,
        None,
        Arc::clone(&client) as Arc<dyn ParticipantClient>,
    );
    let participants = register_servers(&mesh, N);

    let result = mesh
        .execute_transaction(participants, "op".into())
        .await
        .expect("事务应成功");
    assert!(result.success, "全部 commit 成功，事务应成功");

    let peak = client.peak.load(Ordering::SeqCst);
    assert_eq!(
        peak, N,
        "commit 阶段并发峰值应等于参与者数 {N}（串行实现恒为 1）——实测 {peak}"
    );
}

/// 失败范围最小化：任一参与者 commit 失败，其余仍被尝试提交，事务整体判失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_phase_failure_keeps_minimal_scope() {
    const N: usize = 5;
    let mut cfg = MeshConfig::default();
    cfg.transaction_timeout_ms = 5000;
    // s-2 在 commit 阶段始终失败
    let client = Arc::new(MockParticipantClient::new().fail_commit("s-2"));
    let mesh = McpMesh::with_participant_client(
        cfg,
        None,
        Arc::clone(&client) as Arc<dyn ParticipantClient>,
    );
    let participants = register_servers(&mesh, N);

    let result = mesh
        .execute_transaction(participants, "op".into())
        .await
        .expect("execute_transaction 吞掉 commit 错误并返回 failed 结果");
    assert!(
        !result.success,
        "有参与者 commit 最终失败 → 事务应判失败（与旧串行一致）"
    );

    // 失败范围最小化：所有 5 个参与者都被 commit 尝试过（distinct 去重后 == N）
    let mut distinct: Vec<String> = client
        .call_log()
        .iter()
        .filter(|c| c.phase == MockPhase::Commit)
        .map(|c| c.server_id.clone())
        .collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        N,
        "所有参与者都应被 commit 尝试（失败范围最小化），实测 {distinct:?}"
    );
}
