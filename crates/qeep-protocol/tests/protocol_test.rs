//! QEEP 三元组(Request-Ack-Receipt)与状态机契约测试
//!
//! 覆盖 Phase II correctness bug regression 要求:
//! - 完整三元组生命周期:Pending → Acknowledged → Completed
//! - 缺少 Ack 则无法到达 Receipt
//! - 终结状态不可再次转移
//! - 超时设置 Timeout 状态

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use qeep_protocol::{CallState, QeepError, QeepProtocol};

/// 测试完整 QEEP 三元组生命周期
///
/// 验证一个纠缠调用会依次经历:
/// 1. Request 注册(Pending/Acknowledged 阶段可观测)
/// 2. Ack 生成,状态进入 Acknowledged
/// 3. future 完成后 Receipt 返回给调用者
/// 4. 记录清理,pending 归零,completed_count 递增
///
/// 注:协议在调用终结后会立即移除内部记录,因此 Completed 状态本身
/// 通过 completed_count/pending_count 间接验证,而非 `call_state`。
#[tokio::test]
async fn test_full_triplet_request_ack_receipt() {
    let protocol = QeepProtocol::new(Duration::from_secs(5));
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let worker_barrier = barrier.clone();

    let handle = tokio::spawn({
        let p = protocol.clone();
        async move {
            p.entangle(async move {
                worker_barrier.wait().await;
                Ok(42)
            })
            .await
        }
    });

    // 等待注册 + Ack 创建完成
    tokio::time::sleep(Duration::from_millis(20)).await;

    let ids = protocol.pending_call_ids();
    assert_eq!(ids.len(), 1, "应存在且仅存在一个进行中的调用");
    let id = ids[0];

    // Request 已注册且已收到 Ack
    assert_eq!(
        protocol.call_state(id),
        Some(CallState::Acknowledged),
        "注册后应立刻进入 Acknowledged 状态"
    );
    let ack = protocol
        .call_ack(id)
        .expect("Acknowledged 状态的调用必须存在 Ack");
    assert_eq!(ack.id, id, "Ack 的 id 应与调用 id 一致");

    // 放行 future 完成,调用者收到 Receipt(result)
    barrier.wait().await;
    let receipt = handle.await.expect("JoinHandle 不应 panic");
    assert_eq!(receipt.unwrap(), 42, "应收到正确的 Receipt(result)");

    // 完成后记录被清理,状态不可再查询
    assert!(
        protocol.call_state(id).is_none(),
        "Completed 后内部记录应被移除"
    );
    assert!(protocol.call_ack(id).is_none(), "Completed 后 Ack 不再保留");
    assert_eq!(protocol.pending_count(), 0, "完成后 pending 应为 0");
    assert_eq!(
        protocol.completed_count(),
        1,
        "完成后 completed_count 应递增"
    );
    assert_eq!(protocol.orphan_count(), 0, "正常完成不产生孤儿");
}

/// 测试状态机不变量:Ack 是到达 Receipt 的必要条件
///
/// 验证 entangle 在执行 future 前必定产生 Ack,因此不存在
/// Pending → Completed 的绕过路径。
#[tokio::test]
async fn test_ack_missing_blocks_receipt() {
    let protocol = QeepProtocol::new(Duration::from_secs(5));

    let handle = protocol.entangle_spawn(async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok("done")
    });

    // 轮询直到发现 Ack;若始终无 Ack 则未来也不可能到达 Completed。
    let mut ack_observed = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        if protocol
            .pending_call_ids()
            .iter()
            .any(|id| protocol.call_ack(*id).is_some())
        {
            ack_observed = true;
            break;
        }
    }
    assert!(ack_observed, "调用在到达 Completed 之前必须先产生 Ack");

    let result = handle.await.expect("JoinHandle 不应 panic");
    assert_eq!(result.unwrap(), "done");
}

/// 测试终结状态的不可转移性
///
/// Completed/Timeout/Failed 三种终结状态在 entangle 返回后,
/// 内部记录会立即从 pending_calls 移除,因此不存在再次状态转移的可能。
#[tokio::test]
async fn test_call_state_terminal_cannot_transition() {
    // Completed
    let protocol = QeepProtocol::new(Duration::from_secs(5));
    let result = protocol.entangle(async { Ok(42) }).await;
    assert_eq!(result.unwrap(), 42);
    assert!(
        protocol.pending_call_ids().is_empty(),
        "Completed 后记录应被移除,无法再次转移"
    );

    // Timeout
    let protocol = QeepProtocol::new(Duration::from_millis(1));
    let result: Result<i32, QeepError> = protocol
        .entangle(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(42)
        })
        .await;
    assert!(matches!(result, Err(QeepError::Timeout)));
    assert!(
        protocol.pending_call_ids().is_empty(),
        "Timeout 后记录应被移除,无法再次转移"
    );

    // Failed
    let protocol = QeepProtocol::new(Duration::from_secs(5));
    let result: Result<(), QeepError> =
        protocol.entangle(async { Err(QeepError::Cancelled) }).await;
    assert!(matches!(result, Err(QeepError::Cancelled)));
    assert!(
        protocol.pending_call_ids().is_empty(),
        "Failed 后记录应被移除,无法再次转移"
    );
}

/// 测试超时场景下状态最终进入 Timeout
///
/// 验证 future 超过超时窗口后,entangle 返回 Timeout 错误,
/// 且协议计数器正确清理。
#[tokio::test]
async fn test_entangle_timeout_sets_state_timeout() {
    let protocol = QeepProtocol::new(Duration::from_millis(50));

    let handle = protocol.entangle_spawn(async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok(42)
    });

    // 等待任务注册并 Ack
    tokio::time::sleep(Duration::from_millis(10)).await;
    let ids = protocol.pending_call_ids();
    assert_eq!(ids.len(), 1, "超时前应存在进行中的调用");
    let id = ids[0];
    assert_eq!(
        protocol.call_state(id),
        Some(CallState::Acknowledged),
        "超时前状态应为 Acknowledged"
    );

    // 等待超时发生
    let result = handle.await.expect("JoinHandle 不应 panic");
    assert!(
        matches!(result, Err(QeepError::Timeout)),
        "future 超时应返回 QeepError::Timeout"
    );

    // Timeout 也是终结状态,记录被清理
    assert!(protocol.call_state(id).is_none(), "Timeout 后记录应被移除");
    assert_eq!(protocol.pending_count(), 0, "Timeout 后 pending 应为 0");
    assert_eq!(
        protocol.completed_count(),
        1,
        "Timeout 计入 completed_count"
    );
    assert_eq!(protocol.orphan_count(), 0, "Timeout 不属于孤儿");
}
