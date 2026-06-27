//! 超时治理 — 单操作超时包装
//!
//! 对应尸检教训:5.4% 孤儿调用中,部分源于无超时控制导致永久挂起。
//! GQEP 通过 `tokio::time::timeout` 包装每个操作,超时返回 `OperationTimeout`。
//!
//! # 超时策略
//! - **单操作超时**:每个 future 用 `tokio::time::timeout` 包装
//! - **全局超时**:调用者传入 `GqepConfig.default_timeout_ms` 作为 `timeout_ms`
//! - **单操作覆盖全局**:调用者可为特定操作传入更短/更长的 `timeout_ms`
//!
//! # 事件发布
//! 超时触发时发布 `OperationTimedOut` 事件(携带 `operation_id`/`timeout_ms`),
//! 供订阅者(如 efficiency-monitor)记录超时指标。

use std::time::Duration;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::warn;

use crate::error::GqepError;
use crate::types::GqepFuture;

/// 为单个操作添加超时包装
///
/// 使用 `tokio::time::timeout` 包装,超时返回 `GqepError::OperationTimeout`。
/// 超时事件通过 Event Bus 发布 `OperationTimedOut` 事件
/// (携带 `operation_id`/`timeout_ms`)。
///
/// # 参数
/// - `future`:待包装的异步操作
/// - `timeout_ms`:超时阈值(毫秒)
///   - `0` 表示不超时(直接执行,用于明确不需要超时的场景)
///   - 其他值表示超过此毫秒数则触发超时
/// - `operation_id`:操作标识,用于事件追踪与日志关联
/// - `event_bus`:事件总线,用于发布超时事件
///
/// # 全局超时与单操作超时
/// - **全局超时**:调用者传入 `config.default_timeout_ms`
/// - **单操作覆盖全局**:调用者为特定操作传入不同的 `timeout_ms`
///   (如长操作传入更大值,短操作传入更小值)
///
/// # 返回
/// 包装后的 `GqepFuture<T>`:
/// - 正常完成:返回原 future 的结果
/// - 超时:返回 `GqepError::OperationTimeout`,并发布 `OperationTimedOut` 事件
///
/// # 示例
/// ```no_run
/// # use gqep_executor::{with_timeout, GqepFuture, GqepError};
/// # use event_bus::EventBus;
/// # async fn example() {
/// let bus = EventBus::new();
/// let future: GqepFuture<String> = Box::pin(async { Ok("done".to_string()) });
/// // 单操作超时 100ms,覆盖全局默认 5000ms
/// let wrapped = with_timeout(future, 100, "op-1", bus);
/// let result = wrapped.await;
/// # }
/// ```
pub fn with_timeout<T>(
    future: GqepFuture<T>,
    timeout_ms: u64,
    operation_id: &str,
    event_bus: EventBus,
) -> GqepFuture<T>
where
    T: Send + 'static,
{
    let operation_id = operation_id.to_string();
    let event_bus = event_bus.clone();
    Box::pin(async move {
        // 0 表示不超时,直接执行(用于明确不需要超时的场景)
        if timeout_ms == 0 {
            return future.await;
        }

        let timeout = Duration::from_millis(timeout_ms);
        match tokio::time::timeout(timeout, future).await {
            Ok(result) => result,
            Err(_) => {
                // 超时:发布 OperationTimedOut 事件
                // WHY 发布事件:供 efficiency-monitor 等订阅者记录超时指标,
                // 对应架构红线"所有异步操作必须有 GQEP 聚集/超时处理"
                let event = NexusEvent::OperationTimedOut {
                    metadata: EventMetadata::new("gqep-executor"),
                    operation_id: operation_id.clone(),
                    timeout_ms,
                };
                if let Err(e) = event_bus.publish(event).await {
                    warn!(error = %e, operation_id = %operation_id, "发布超时事件失败");
                }
                Err(GqepError::OperationTimeout {
                    operation_id,
                    timeout_ms,
                })
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_timeout_triggered() {
        // 验证超时触发:长操作 + 短超时 → OperationTimeout
        let bus = EventBus::new();
        let long_future: GqepFuture<String> = Box::pin(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("done".to_string())
        });

        let result = with_timeout(long_future, 50, "op-1", bus).await;

        assert!(
            matches!(result, Err(GqepError::OperationTimeout { ref operation_id, timeout_ms }) if operation_id == "op-1" && timeout_ms == 50),
            "应返回 OperationTimeout,实际: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_no_timeout_success() {
        // 验证未超时正常返回
        let bus = EventBus::new();
        let quick_future: GqepFuture<String> = Box::pin(async { Ok("done".to_string()) });

        let result = with_timeout(quick_future, 5000, "op-2", bus).await;

        assert_eq!(result.unwrap(), "done");
    }

    #[tokio::test]
    async fn test_per_op_timeout_overrides_global() {
        // 验证单操作超时覆盖全局:全局 5000ms,单操作 50ms → 超时
        let bus = EventBus::new();
        let long_future: GqepFuture<String> = Box::pin(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("done".to_string())
        });

        // 单操作超时 50ms(覆盖全局默认 5000ms)
        let result = with_timeout(long_future, 50, "op-3", bus).await;

        assert!(
            matches!(
                result,
                Err(GqepError::OperationTimeout { timeout_ms: 50, .. })
            ),
            "单操作超时 50ms 应覆盖全局,实际: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_timeout_zero_means_no_timeout() {
        // 验证 timeout_ms=0 表示不超时
        let bus = EventBus::new();
        let future: GqepFuture<String> = Box::pin(async { Ok("done".to_string()) });

        let result = with_timeout(future, 0, "op-4", bus).await;

        assert_eq!(result.unwrap(), "done");
    }

    #[tokio::test]
    async fn test_timeout_preserves_future_error() {
        // 验证原 future 的错误被保留(非超时错误)
        let bus = EventBus::new();
        let failing_future: GqepFuture<String> = Box::pin(async {
            Err(GqepError::OperationFailed {
                operation_id: "inner".into(),
                reason: "intentional".into(),
            })
        });

        let result = with_timeout(failing_future, 5000, "op-5", bus).await;

        assert!(
            matches!(result, Err(GqepError::OperationFailed { ref reason, .. }) if reason == "intentional"),
            "应保留原 future 错误,实际: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_timeout_publishes_event() {
        // 验证超时事件发布
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let long_future: GqepFuture<String> = Box::pin(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("done".to_string())
        });

        let _ = with_timeout(long_future, 50, "op-6", bus).await;

        let event = rx.recv_timeout(Duration::from_millis(100)).await;
        assert!(event.is_ok(), "应收到事件");
        assert!(
            matches!(event.unwrap(), NexusEvent::OperationTimedOut { ref operation_id, timeout_ms, .. } if operation_id == "op-6" && timeout_ms == 50),
            "应为 OperationTimedOut 事件"
        );
    }
}
