//! PVL 核心类型定义
//!
//! 包含生产验证闭环(Producer-Verifier Loop)的核心领域类型:
//! - `OperationId`:操作唯一标识(基于 `nexus_core::id_newtype!` 宏)
//! - `Operation`:生产验证的基本单元(含内容、置信度、状态)
//! - `OperationStatus`:操作生命周期状态机
//! - `VerificationResult`:验证结果(通过/拒绝 + 原因)
//! - `FeedbackMessage`:Verifier→Producer 的反馈消息
//! - `ProducerStrategy`:生产策略(影响生成速率与置信度阈值)

use std::time::Instant;

use serde::{Deserialize, Serialize};

// 使用 L1 Core 的 id_newtype! 宏生成 OperationId newtype
// WHY 集中宏:消除各 crate 重复实现 newtype,确保所有 ID 类型行为一致
// (Deref/AsRef/Borrow/From/Display),且 #[serde(transparent)] 保证向后兼容
nexus_core::id_newtype!(OperationId, "PVL 操作唯一标识");

/// 操作生命周期状态
///
/// 状态流转:`Pending` → `Produced` → `Verified` / `Rejected`
/// WHY 显式状态机:对应 Claude Code 尸检教训(结果丢失 5.4% 孤儿调用),
/// 显式状态使每个操作的生命周期可追溯,避免操作"消失"
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationStatus {
    /// 待生产:操作已创建但 Producer 尚未生成内容
    Pending,
    /// 已生产:Producer 已生成内容,等待 Verifier 验证
    Produced,
    /// 已验证:Verifier 验证通过,操作可被下游消费
    Verified,
    /// 已拒绝:Verifier 验证失败,操作被驳回
    Rejected,
}

/// 生产验证的基本单元 — Producer 生成、Verifier 验证的对象
///
/// WHY 独立结构体(而非裸字符串):携带置信度与状态,使验证决策可基于
/// Producer 的自评估(置信度)进行风险门控,而非仅依赖 Verifier 的规则
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    /// 操作唯一标识
    pub operation_id: OperationId,
    /// 所属 Quest ID(关联 L9 Quest Engine)
    pub quest_id: String,
    /// 操作内容(待验证的文本/代码/命令)
    pub content: String,
    /// Producer 自评估置信度 [0.0, 1.0]
    ///
    /// WHY 置信度:占位实现基于内容哈希,未来接入模型后由模型输出。
    /// Verifier 可据此进行风险门控(低置信度操作加强检查)
    pub confidence: f32,
    /// 操作当前状态
    pub status: OperationStatus,
}

impl Operation {
    /// 创建新操作,初始状态为 `Pending`,置信度为 0.0
    pub fn new(
        operation_id: OperationId,
        quest_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            operation_id,
            quest_id: quest_id.into(),
            content: content.into(),
            confidence: 0.0,
            status: OperationStatus::Pending,
        }
    }

    /// 标记操作为已生产,设置置信度
    pub fn mark_produced(&mut self, confidence: f32) {
        self.confidence = confidence;
        self.status = OperationStatus::Produced;
    }

    /// 标记操作为已验证
    pub fn mark_verified(&mut self) {
        self.status = OperationStatus::Verified;
    }

    /// 标记操作为已拒绝
    pub fn mark_rejected(&mut self) {
        self.status = OperationStatus::Rejected;
    }
}

/// 验证结果 — Verifier 对单个操作的验证结论
///
/// WHY 独立结构体:携带 `reason` 使拒绝原因可追溯,供 FeedbackChannel
/// 进行策略调整决策(如某类拒绝频繁则调整 Producer 策略)
#[derive(Debug, Clone, PartialEq)]
pub struct VerificationResult {
    /// 被验证操作 ID
    pub operation_id: OperationId,
    /// 是否通过验证
    pub passed: bool,
    /// 验证原因(通过时为"OK",拒绝时为具体原因)
    pub reason: String,
}

impl VerificationResult {
    /// 创建通过验证的结果
    pub fn passed(operation_id: OperationId) -> Self {
        Self {
            operation_id,
            passed: true,
            reason: "OK".to_string(),
        }
    }

    /// 创建拒绝验证的结果
    pub fn rejected(operation_id: OperationId, reason: impl Into<String>) -> Self {
        Self {
            operation_id,
            passed: false,
            reason: reason.into(),
        }
    }
}

/// 反馈消息 — Verifier→Producer 的验证反馈
///
/// WHY 携带 `Instant` 时间戳:FeedbackChannel 据此计算拒绝率的时间窗口,
/// 实现滑动窗口策略调整(避免历史拒绝率影响当前决策)
#[derive(Debug, Clone)]
pub struct FeedbackMessage {
    /// 被反馈操作 ID
    pub operation_id: OperationId,
    /// 验证结果
    pub result: VerificationResult,
    /// 反馈产生时刻(单调时钟,用于时间窗口计算)
    pub timestamp: Instant,
}

impl FeedbackMessage {
    /// 创建反馈消息,时间戳为当前时刻
    pub fn new(operation_id: OperationId, result: VerificationResult) -> Self {
        Self {
            operation_id,
            result,
            timestamp: Instant::now(),
        }
    }
}

/// 生产策略 — 影响 Producer 的生成速率与置信度阈值
///
/// WHY 三级策略:对应 PVL 设计中的自适应生产控制。
/// 拒绝率升高时降级到 Conservative(降速+提阈值),
/// 拒绝率降低时升级到 Aggressive(提速+降阈值),
/// Normal 为默认平衡态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProducerStrategy {
    /// 正常策略:默认生成速率与置信度阈值
    #[default]
    Normal,
    /// 保守策略:降低生成速率,提高置信度阈值(拒绝率高时启用)
    Conservative,
    /// 激进策略:提高生成速率,降低置信度阈值(拒绝率低时启用)
    Aggressive,
}

impl ProducerStrategy {
    /// 获取策略名称(用于事件发布与日志)
    pub fn name(&self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Conservative => "Conservative",
            Self::Aggressive => "Aggressive",
        }
    }

    /// 获取当前策略下的生成间隔(毫秒)
    ///
    /// WHY 不同间隔:Conservative 降速避免持续产生低质量操作,
    /// Aggressive 提速充分利用低拒绝率窗口
    pub fn produce_interval_ms(&self) -> u64 {
        match self {
            Self::Normal => 0,
            Self::Conservative => 10,
            Self::Aggressive => 0,
        }
    }

    /// 获取当前策略下的最低置信度阈值
    ///
    /// WHY 阈值门控:低于阈值的操作在 Producer 端被过滤,
    /// 减少Verifier 的无效验证负载
    pub fn min_confidence(&self) -> f32 {
        match self {
            Self::Normal => 0.3,
            Self::Conservative => 0.6,
            Self::Aggressive => 0.1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_id_newtype() {
        let id = OperationId::new("op-001");
        assert_eq!(id.as_str(), "op-001");
        // Deref<Target=str> 允许 &id 当作 &str
        let s: &str = &id;
        assert_eq!(s, "op-001");
        // From<&str>
        let id2 = OperationId::from("op-001");
        assert_eq!(id, id2);
        // Display
        assert_eq!(id.to_string(), "op-001");
    }

    #[test]
    fn test_operation_lifecycle() {
        let mut op = Operation::new(OperationId::new("op-1"), "quest-1", "print('hello')");
        assert_eq!(op.status, OperationStatus::Pending);
        assert_eq!(op.confidence, 0.0);

        op.mark_produced(0.85);
        assert_eq!(op.status, OperationStatus::Produced);
        assert!((op.confidence - 0.85).abs() < f32::EPSILON);

        op.mark_verified();
        assert_eq!(op.status, OperationStatus::Verified);
    }

    #[test]
    fn test_verification_result_passed() {
        let id = OperationId::new("op-1");
        let result = VerificationResult::passed(id.clone());
        assert!(result.passed);
        assert_eq!(result.reason, "OK");
    }

    #[test]
    fn test_verification_result_rejected() {
        let id = OperationId::new("op-2");
        let result = VerificationResult::rejected(id.clone(), "危险命令");
        assert!(!result.passed);
        assert_eq!(result.reason, "危险命令");
    }

    #[test]
    fn test_feedback_message_timestamp() {
        let id = OperationId::new("op-1");
        let result = VerificationResult::passed(id.clone());
        let feedback = FeedbackMessage::new(id, result);
        // 时间戳应为当前时刻附近(允许微小偏差)
        assert!(feedback.timestamp.elapsed().as_secs() < 1);
    }

    #[test]
    fn test_producer_strategy_name() {
        assert_eq!(ProducerStrategy::Normal.name(), "Normal");
        assert_eq!(ProducerStrategy::Conservative.name(), "Conservative");
        assert_eq!(ProducerStrategy::Aggressive.name(), "Aggressive");
    }

    #[test]
    fn test_producer_strategy_intervals() {
        // Conservative 应有间隔(降速),Normal/Aggressive 无间隔
        assert_eq!(ProducerStrategy::Normal.produce_interval_ms(), 0);
        assert!(ProducerStrategy::Conservative.produce_interval_ms() > 0);
        assert_eq!(ProducerStrategy::Aggressive.produce_interval_ms(), 0);
    }

    #[test]
    fn test_producer_strategy_confidence_thresholds() {
        // Conservative 阈值最高(严格门控),Aggressive 最低(宽松门控)
        assert!(
            ProducerStrategy::Conservative.min_confidence()
                > ProducerStrategy::Normal.min_confidence()
        );
        assert!(
            ProducerStrategy::Normal.min_confidence()
                > ProducerStrategy::Aggressive.min_confidence()
        );
    }

    #[test]
    fn test_operation_status_serde() {
        let status = OperationStatus::Produced;
        let json = serde_json::to_string(&status).expect("序列化失败");
        let restored: OperationStatus = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(status, restored);
    }
}
