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

use tokio::sync::{mpsc, Semaphore};

use mcp_mesh::{
    McpMesh, MeshConfig, MeshServer, MockParticipantClient, MockPhase, ParticipantClient,
};

/// 跟踪 commit 阶段最大并发在飞数的测试客户端。
///
/// `commit` future 首帧即 `fetch_add` 并记录峰值，然后向测试侧报告到达，
/// 再阻塞到测试释放（`Semaphore` 预存 permit，无丢失唤醒）。
/// `FuturesUnordered` 会把所有 commit future 推进到各自 await 点，
/// 故峰值 == N（串行时只会到达 1 个）——判定与调度时序无关，确定性可断言。
///
/// WHY 不用 `sleep(25ms)`（旧版）：峰值依赖「一轮 poll 能推进全部 future」的
/// 实现细节，且 25ms 窗口在高负载下可能提前结束 → 偶发 `peak < N`。
/// 改为「收齐 N 个到达 → 记录峰值 → 释放」的显式同步。
struct InflightClient {
    inflight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    arrived: mpsc::UnboundedSender<()>,
    release: Arc<Semaphore>,
}

impl InflightClient {
    fn new() -> (Self, mpsc::UnboundedReceiver<()>, Arc<Semaphore>) {
        let (arrived_tx, arrived_rx) = mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));
        (
            Self {
                inflight: Arc::new(AtomicUsize::new(0)),
                peak: Arc::new(AtomicUsize::new(0)),
                arrived: arrived_tx,
                release: Arc::clone(&release),
            },
            arrived_rx,
            release,
        )
    }
}

impl ParticipantClient for InflightClient {
    fn prepare<'a>(
        &'a self,
        _s: &'a MeshServer,
        _t: &'a str,
        _op: &'a str,
        _args: &'a [String],
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
        let arrived = self.arrived.clone();
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(cur, Ordering::SeqCst);
            // 报告已到达（测试收齐 N 个后才释放）
            let _ = arrived.send(());
            let permit = release.acquire().await.expect("release semaphore closed");
            permit.forget();
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

/// 并发扇出证明：commit 峰值在飞数 == 参与者数（串行时只会到达 1 个）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_phase_fanout_is_concurrent() {
    // 环境隔离：开发者在本地 shell 设了串行开关时，本测试必然不满足并发语义，
    // 直接给出可诊断的信息，而不是一个难以定位的断言失败。
    assert!(
        std::env::var_os("CHIMERA_MCP_SERIAL_COMMIT").is_none(),
        "commit_concurrency 需要并发提交语义；请先 unset CHIMERA_MCP_SERIAL_COMMIT"
    );

    const N: usize = 5;
    // 宽松事务超时：等待测试释放期间不触发 TransactionTimeout（生产 200ms 语义不变）
    let cfg = MeshConfig {
        transaction_timeout_ms: 5000,
        ..MeshConfig::default()
    };
    let (client, mut arrived, release) = InflightClient::new();
    let client = Arc::new(client);
    let mesh = McpMesh::with_participant_client(
        cfg,
        None,
        Arc::clone(&client) as Arc<dyn ParticipantClient>,
    );
    let participants = register_servers(&mesh, N);

    let task = tokio::spawn(async move {
        mesh.execute_transaction(participants, "op".into(), Vec::new())
            .await
    });

    // 收齐 N 个到达（串行实现只会有 1 个 → 2s 超时直接失败，不挂死）
    for i in 0..N {
        tokio::time::timeout(Duration::from_secs(2), arrived.recv())
            .await
            .unwrap_or_else(|_| panic!("第 {i} 个 commit 未在 2s 内到达（疑似串行执行）"))
            .expect("arrival channel 不应关闭");
    }
    let peak = client.peak.load(Ordering::SeqCst);
    release.add_permits(N);

    let result = task.await.expect("事务任务不应 panic").expect("事务应成功");
    assert!(result.success, "全部 commit 成功，事务应成功");
    assert_eq!(
        peak, N,
        "commit 阶段并发峰值应等于参与者数 {N}（串行实现恒为 1）——实测 {peak}"
    );
}

/// 失败范围最小化：任一参与者 commit 失败，其余仍被尝试提交，事务整体判失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_phase_failure_keeps_minimal_scope() {
    const N: usize = 5;
    let cfg = MeshConfig {
        transaction_timeout_ms: 5000,
        ..MeshConfig::default()
    };
    // s-2 在 commit 阶段始终失败
    let client = Arc::new(MockParticipantClient::new().fail_commit("s-2"));
    let mesh = McpMesh::with_participant_client(
        cfg,
        None,
        Arc::clone(&client) as Arc<dyn ParticipantClient>,
    );
    let participants = register_servers(&mesh, N);

    let result = mesh
        .execute_transaction(participants, "op".into(), Vec::new())
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
