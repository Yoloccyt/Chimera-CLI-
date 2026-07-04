//! MTPE 失败回退机制 — 多步预测失败后回退到单步预测
//!
//! 对应架构层:L7 Execution
//!
//! # 回退策略
//! 失败步回退到单步预测(N=1),而非整体回滚。
//! WHY 部分回退:整体回滚会浪费已成功的步骤产出,单步回退仅重做失败部分,
//! 减少计算浪费。对应 spec.md 决策:"失败步回退到单步预测,而非整体回滚"
//!
//! # 事件
//! 回退操作发布 `PredictionRolledBack` 事件(携带 failed_step/rollback_to)
//!
//! # 依赖说明
//! MTPE(L7)→GQEP(L6) 向下依赖允许,但为简化实现,
//! 回退直接调用 `self.predict(context, 1)`,不经过 GQEP 聚集。
//! WHY 简化:回退是低频操作(仅在预测失败时触发),且单步预测本身可靠,
//! 无需 GQEP 的并行聚集开销。Week 6 接入真实模型后可重新评估

use event_bus::{EventMetadata, NexusEvent};
use tracing::debug;

use crate::error::MtpeError;
use crate::predictor::MtpeExecutor;
use crate::types::{PredictionContext, PredictionResult};

/// 回退目标步数 — 固定为 1(单步预测)
const ROLLBACK_TO: usize = 1;

impl MtpeExecutor {
    /// 回退到单步预测 — 多步预测失败后的降级策略
    ///
    /// # 参数
    /// - `context`:原预测上下文(复用,避免重新编码)
    /// - `failed_step`:失败步序号(0-based),用于事件追踪
    ///
    /// # 返回
    /// - `Ok(PredictionResult)`:单步预测结果(N=1)
    /// - `Err(RollbackFailed)`:单步预测也失败,调用方应中止 Quest
    ///
    /// # 事件
    /// 发布 `PredictionRolledBack` 事件(携带 failed_step/rollback_to)
    ///
    /// # 设计决策
    /// WHY 复用 context:回退是同一上下文的降级预测,重新编码会引入
    /// 不必要的 NMC 调用开销,且可能改变上下文语义
    pub async fn rollback_to_single_step(
        &self,
        context: &PredictionContext,
        failed_step: usize,
    ) -> Result<PredictionResult, MtpeError> {
        debug!(
            quest_id = %context.quest_id,
            failed_step,
            rollback_to = ROLLBACK_TO,
            "MTPE 回退到单步预测"
        );

        // 调用单步预测(N=1)
        // WHY 直接调用 self.predict:回退是低频操作,无需 GQEP 聚集开销
        let result =
            self.predict(context, ROLLBACK_TO)
                .await
                .map_err(|e| MtpeError::RollbackFailed {
                    reason: format!("single-step prediction failed: {}", e),
                })?;

        // 发布回退事件
        let event = NexusEvent::PredictionRolledBack {
            metadata: EventMetadata::new("mtpe-executor"),
            failed_step,
            rollback_to: ROLLBACK_TO,
        };
        // WHY 忽略 publish 错误:事件丢失不影响回退主流程,
        // 回退结果已通过返回值传递
        let _ = self.event_bus().publish(event).await;

        debug!(
            quest_id = %context.quest_id,
            failed_step,
            "MTPE 回退完成"
        );

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MtpeConfig;
    use event_bus::EventBus;

    fn make_context(quest_id: &str) -> PredictionContext {
        PredictionContext {
            quest_id: quest_id.into(),
            history: vec!["test context".into()],
            clv: vec![0.1; 8],
        }
    }

    #[tokio::test]
    async fn test_rollback_step3_failure() {
        // 模拟第 3 步失败,回退到单步预测
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-rollback-3");

        let result = executor.rollback_to_single_step(&ctx, 3).await.unwrap();

        // 回退后应为单步预测
        assert_eq!(result.n, 1);
        assert_eq!(result.predicted_tokens.len(), 1);
        // 单步预测置信度应为 1.0
        assert!((result.predicted_tokens[0].confidence - 1.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_rollback_publishes_event() {
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let mut rx = executor.event_bus().subscribe();
        let ctx = make_context("q-rollback-evt");

        executor.rollback_to_single_step(&ctx, 5).await.unwrap();

        // 回退会产生两个事件:PredictionMade(来自 predict) + PredictionRolledBack
        let mut found_rollback = false;
        loop {
            match rx.try_recv() {
                Ok(Some(NexusEvent::PredictionRolledBack {
                    failed_step,
                    rollback_to,
                    ..
                })) => {
                    assert_eq!(failed_step, 5);
                    assert_eq!(rollback_to, 1);
                    found_rollback = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => break,
            }
        }
        assert!(found_rollback, "应发布 PredictionRolledBack 事件");
    }

    #[tokio::test]
    async fn test_rollback_improves_success_rate() {
        // 回退后成功率应提升:单步预测成功率通常高于多步
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-rollback-improve");

        // 模拟 N=5 预测失败
        executor.record_verification(5, false).await;
        assert_eq!(executor.get_success_rate(5).await, 0.0);

        // 回退到单步预测并验证成功
        let result = executor.rollback_to_single_step(&ctx, 4).await.unwrap();
        assert_eq!(result.n, 1);

        // 记录单步预测成功
        executor.record_verification(1, true).await;
        assert!((executor.get_success_rate(1).await - 1.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_rollback_with_step0() {
        // 边界:第 0 步失败
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-rollback-0");

        let result = executor.rollback_to_single_step(&ctx, 0).await.unwrap();
        assert_eq!(result.n, 1);
    }

    #[tokio::test]
    async fn test_rollback_with_step9() {
        // 边界:第 9 步失败(N=10 的最后一步)
        let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
        let ctx = make_context("q-rollback-9");

        let result = executor.rollback_to_single_step(&ctx, 9).await.unwrap();
        assert_eq!(result.n, 1);
    }
}
