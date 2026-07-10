//! PVL 验证者 — 流式验证操作并发送反馈
//!
//! 对应架构:L7 Execution,Producer-Verifier Loop 的验证端
//!
//! # 设计决策
//! - **同步 verify**:验证逻辑为纯 CPU 计算(无 IO),同步实现更简单,
//!   避免不必要的 async 开销
//! - **异步 run**:run 需要等待通道接收(rx.recv().await)与发送
//!   (feedback_tx.send().await),必须 async
//! - **AtomicU64 计数**:无锁统计验证/拒绝数,避免锁竞争
//! - **逐个验证**:对应流式处理设计,完成一个处理一个,内存占用低

use std::sync::atomic::{AtomicU64, Ordering};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::sync::mpsc;
use tracing::warn;

use crate::config::PvlConfig;
use crate::error::PvlError;
use crate::types::{FeedbackMessage, Operation, VerificationResult};

/// 危险关键词列表 — 安全检查用
///
/// WHY 静态列表:占位实现,实际场景应从 SecCore 获取动态黑名单。
/// 这些关键词覆盖常见危险命令模式
const DANGEROUS_KEYWORDS: &[&str] = &[
    "rm -rf",
    "sudo rm",
    "chmod 777",
    "mkfs",
    "dd if=/dev/zero",
    ":(){:|:&};:",
    "fork bomb",
];

/// PVL 验证者 — 流式验证操作并发送反馈
///
/// # 线程安全
/// `Verifier` 内部所有字段均为线程安全。`verify` 方法为 `&self` 同步方法,
/// `run` 方法为 `&self` 异步方法,均允许多任务并发调用。
///
/// # 验证流程
/// 1. 语法检查:内容非空
/// 2. 安全检查:内容不含危险关键词(如 "rm -rf")
/// 3. 依赖检查:内容不含未定义引用(占位:检查 "$undefined")
pub struct Verifier {
    /// 执行配置
    pub(crate) config: PvlConfig,
    /// 事件总线,用于发布 PredictionVerified 事件
    pub(crate) event_bus: EventBus,
    /// 已验证通过的操作数(无锁统计)
    pub(crate) verified_count: AtomicU64,
    /// 已拒绝的操作数(无锁统计)
    pub(crate) rejected_count: AtomicU64,
}

impl Verifier {
    /// 创建新的验证者
    ///
    /// # 参数
    /// - `config`:执行配置
    /// - `event_bus`:事件总线,用于发布 `PredictionVerified` 事件
    pub fn new(config: PvlConfig, event_bus: EventBus) -> Self {
        Self {
            config,
            event_bus,
            verified_count: AtomicU64::new(0),
            rejected_count: AtomicU64::new(0),
        }
    }

    /// 验证单个操作(同步)
    ///
    /// # 验证规则(占位实现)
    /// 1. **语法检查**:内容非空
    /// 2. **安全检查**:内容不含危险关键词(如 "rm -rf")
    /// 3. **依赖检查**:内容不含未定义引用(占位:检查 "$undefined")
    ///
    /// # 参数
    /// - `operation`:待验证操作
    ///
    /// # 返回
    /// 验证结果(通过/拒绝 + 原因)
    pub fn verify(&self, operation: &Operation) -> VerificationResult {
        // P1-9:验证超时保护 — 使用 std::time::Instant 限制验证时间
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(self.config.verification_timeout_ms);

        // 1. 语法检查:内容非空
        if operation.content.trim().is_empty() {
            return VerificationResult::rejected(
                operation.operation_id.clone(),
                "语法检查失败:内容为空",
            );
        }
        if start.elapsed() > timeout {
            return VerificationResult::rejected(
                operation.operation_id.clone(),
                "验证超时:语法检查阶段超时",
            );
        }

        // 2. 安全检查:不含危险关键词
        let content_lower = operation.content.to_lowercase();
        if start.elapsed() > timeout {
            return VerificationResult::rejected(
                operation.operation_id.clone(),
                "验证超时:安全检查阶段超时",
            );
        }
        for keyword in DANGEROUS_KEYWORDS {
            if content_lower.contains(keyword) {
                return VerificationResult::rejected(
                    operation.operation_id.clone(),
                    format!("安全检查失败:包含危险关键词 '{}'", keyword),
                );
            }
        }
        if start.elapsed() > timeout {
            return VerificationResult::rejected(
                operation.operation_id.clone(),
                "验证超时:安全检查阶段超时",
            );
        }

        // 3. 依赖检查:不含未定义引用(占位)
        if operation.content.contains("$undefined") {
            return VerificationResult::rejected(
                operation.operation_id.clone(),
                "依赖检查失败:包含未定义引用 '$undefined'",
            );
        }
        if start.elapsed() > timeout {
            return VerificationResult::rejected(
                operation.operation_id.clone(),
                "验证超时:依赖检查阶段超时",
            );
        }

        VerificationResult::passed(operation.operation_id.clone())
    }

    /// 流式验证运行循环(异步)
    ///
    /// 从通道接收操作,逐个验证,发送反馈,发布事件。
    /// 当发送端关闭(rx.recv 返回 None)时正常退出。
    ///
    /// # 流程
    /// 1. 循环:rx.recv().await 接收操作
    /// 2. 通道关闭(None)时返回 Ok(())
    /// 3. 验证操作(verify)
    /// 4. 根据验证结果更新计数(verified_count/rejected_count)
    /// 5. 发送反馈消息(feedback_tx.send().await)
    /// 6. 发布 PredictionVerified 事件
    ///
    /// # 参数
    /// - `rx`:操作接收通道(Producer→Verifier)
    /// - `feedback_tx`:反馈发送通道(Verifier→FeedbackChannel)
    pub async fn run(
        &self,
        rx: &mut mpsc::Receiver<Operation>,
        feedback_tx: &mpsc::Sender<FeedbackMessage>,
    ) -> Result<(), PvlError> {
        while let Some(mut operation) = rx.recv().await {
            // 验证操作
            let result = self.verify(&operation);

            // 更新操作状态与计数
            if result.passed {
                operation.mark_verified();
                self.verified_count.fetch_add(1, Ordering::Relaxed);
            } else {
                operation.mark_rejected();
                self.rejected_count.fetch_add(1, Ordering::Relaxed);
            }

            // 构建反馈消息并发送
            // WHY await:对应尸检教训(void Promise 无 await),
            // 显式 await 确保反馈发送完成
            let feedback = FeedbackMessage::new(operation.operation_id.clone(), result.clone());
            feedback_tx
                .send(feedback)
                .await
                .map_err(|_| PvlError::ChannelClosed)?;

            // 发布 PredictionVerified 事件
            // WHY 逐个发布:event-bus 的 PredictionVerified 事件字段为
            // op_id/score(单操作粒度),适配为每操作一事件
            let score = if result.passed { 1.0 } else { 0.0 };
            let event = NexusEvent::PredictionVerified {
                metadata: EventMetadata::new("pvl-layer"),
                op_id: operation.operation_id.to_string(),
                score,
            };
            if let Err(e) = self.event_bus.publish(event).await {
                warn!(error = %e, "发布 PredictionVerified 事件失败");
            }
        }

        Ok(())
    }

    /// 获取已验证通过的操作数
    pub fn verified_count(&self) -> u64 {
        self.verified_count.load(Ordering::Relaxed)
    }

    /// 获取已拒绝的操作数
    pub fn rejected_count(&self) -> u64 {
        self.rejected_count.load(Ordering::Relaxed)
    }

    /// 获取配置引用
    pub fn config(&self) -> &PvlConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OperationId;
    use std::time::Duration;

    /// 创建测试用操作
    fn make_operation(content: &str) -> Operation {
        let mut op = Operation::new(OperationId::new("op-test"), "quest-1", content);
        op.mark_produced(0.8);
        op
    }

    #[test]
    fn test_verify_passes_valid_content() {
        // 验证:正常内容通过验证
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
        let op = make_operation("print('hello world')");
        let result = verifier.verify(&op);
        assert!(result.passed, "正常内容应通过验证");
        assert_eq!(result.reason, "OK");
    }

    #[test]
    fn test_verify_rejects_empty_content() {
        // 验证:空内容被拒绝(语法检查)
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
        let op = make_operation("   ");
        let result = verifier.verify(&op);
        assert!(!result.passed, "空内容应被拒绝");
        assert!(result.reason.contains("语法检查失败"));
    }

    #[test]
    fn test_verify_rejects_dangerous_rm_rf() {
        // 验证:包含 "rm -rf" 被拒绝(安全检查)
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
        let op = make_operation("rm -rf /");
        let result = verifier.verify(&op);
        assert!(!result.passed, "危险命令应被拒绝");
        assert!(result.reason.contains("安全检查失败"));
        assert!(result.reason.contains("rm -rf"));
    }

    #[test]
    fn test_verify_rejects_dangerous_sudo_rm() {
        // 验证:包含 "sudo rm" 被拒绝(安全检查)
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
        let op = make_operation("sudo rm -rf /home");
        let result = verifier.verify(&op);
        assert!(!result.passed);
        assert!(result.reason.contains("安全检查失败"));
    }

    #[test]
    fn test_verify_rejects_undefined_reference() {
        // 验证:包含 "$undefined" 被拒绝(依赖检查)
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
        let op = make_operation("echo $undefined");
        let result = verifier.verify(&op);
        assert!(!result.passed, "未定义引用应被拒绝");
        assert!(result.reason.contains("依赖检查失败"));
    }

    #[test]
    fn test_verify_case_insensitive_security_check() {
        // 验证:安全检查不区分大小写
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
        let op = make_operation("RM -RF /");
        let result = verifier.verify(&op);
        assert!(!result.passed, "大写危险命令也应被拒绝");
    }

    #[tokio::test]
    async fn test_run_processes_operations() {
        // 验证:run 从通道接收操作并验证
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
        let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
        let (feedback_tx, mut feedback_rx) = mpsc::channel::<FeedbackMessage>(128);

        // 发送 3 个操作:2 个有效,1 个危险
        op_tx.send(make_operation("valid-1")).await.unwrap();
        op_tx.send(make_operation("rm -rf /")).await.unwrap();
        op_tx.send(make_operation("valid-2")).await.unwrap();
        drop(op_tx);

        // 运行验证者
        // WHY run 后 drop(feedback_tx):run 接收 &feedback_tx(引用),
        // 返回后发送端仍存活,feedback_rx.recv() 不会返回 None。
        // 必须 drop 发送端使接收端循环正常终止
        let mut rx = op_rx;
        verifier.run(&mut rx, &feedback_tx).await.unwrap();
        drop(feedback_tx);

        // 验证计数
        assert_eq!(verifier.verified_count(), 2, "应验证通过 2 个");
        assert_eq!(verifier.rejected_count(), 1, "应拒绝 1 个");

        // 验证反馈发送
        let mut passed = 0;
        let mut rejected = 0;
        while let Some(fb) = feedback_rx.recv().await {
            if fb.result.passed {
                passed += 1;
            } else {
                rejected += 1;
            }
        }
        assert_eq!(passed, 2, "应发送 2 个通过反馈");
        assert_eq!(rejected, 1, "应发送 1 个拒绝反馈");
    }

    #[tokio::test]
    async fn test_run_channel_closed_returns_ok() {
        // 验证:发送端关闭时 run 正常返回
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
        let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
        let (feedback_tx, _feedback_rx) = mpsc::channel::<FeedbackMessage>(128);

        drop(op_tx); // 立即关闭发送端

        let mut rx = op_rx;
        let result = verifier.run(&mut rx, &feedback_tx).await;
        assert!(result.is_ok(), "通道关闭应正常返回 Ok");
    }

    #[tokio::test]
    async fn test_run_publishes_prediction_verified_event() {
        // 验证:run 发布 PredictionVerified 事件
        let bus = EventBus::new();
        let mut event_rx = bus.subscribe();
        let verifier = Verifier::new(PvlConfig::default(), bus.clone());
        let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
        let (feedback_tx, _feedback_rx) = mpsc::channel::<FeedbackMessage>(128);

        op_tx.send(make_operation("valid")).await.unwrap();
        drop(op_tx);

        let mut rx = op_rx;
        verifier.run(&mut rx, &feedback_tx).await.unwrap();

        // 应收到 PredictionVerified 事件
        let event = event_rx.recv_timeout(Duration::from_millis(100)).await;
        assert!(event.is_ok(), "应收到事件");
        let event = event.unwrap();
        assert!(
            matches!(event, NexusEvent::PredictionVerified { score: 1.0, .. }),
            "应为 PredictionVerified 事件且 score=1.0,实际: {:?}",
            event
        );
    }

    #[tokio::test]
    async fn test_run_rejected_publishes_zero_score() {
        // 验证:拒绝的操作发布 score=0.0 的事件
        let bus = EventBus::new();
        let mut event_rx = bus.subscribe();
        let verifier = Verifier::new(PvlConfig::default(), bus.clone());
        let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
        let (feedback_tx, _feedback_rx) = mpsc::channel::<FeedbackMessage>(128);

        op_tx.send(make_operation("rm -rf /")).await.unwrap();
        drop(op_tx);

        let mut rx = op_rx;
        verifier.run(&mut rx, &feedback_tx).await.unwrap();

        let event = event_rx.recv_timeout(Duration::from_millis(100)).await;
        assert!(event.is_ok());
        let event = event.unwrap();
        assert!(
            matches!(event, NexusEvent::PredictionVerified { score: 0.0, .. }),
            "拒绝操作应发布 score=0.0,实际: {:?}",
            event
        );
    }
}
