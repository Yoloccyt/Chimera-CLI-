//! L6 Security Traits — 安全层核心契约（供 L7 消费）
//! 
//! ★ Insight: 将 QEEP 零孤儿调用保证上提至 L6，使 L7 Execution 层依赖
//! L6 trait 而非 L4 qeep-protocol，实现合规的跨层通信。
//! 
//! **依赖方向**:
//! - Before: gqep-executor (L7) → qeep-protocol (L4) ✗ 违规
//! - After: gqep-executor (L7) → router-traits::ZeroOrphanGuarantee (L6) ✓ 合规
//!          qeep-protocol (L4) → router-traits::ZeroOrphanGuarantee (L6) 实现

use nexus_contracts::VerificationResult;
use thiserror::Error;

/// Orphan Reason — 孤儿调用原因
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrphanReason {
    /// 无父调用上下文
    NoParentContext,
    
    /// 调用 ID 未注册
    UnregisteredCallId,
    
    /// 超时未响应
    Timeout,
    
    /// 权限不足
    InsufficientPermissions,
    
    /// 沙箱拒绝
    SandboxDenied,
}

impl std::fmt::Display for OrphanReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrphanReason::NoParentContext => write!(f, "no parent context"),
            OrphanReason::UnregisteredCallId => write!(f, "unregistered call id"),
            OrphanReason::Timeout => write!(f, "timeout"),
            OrphanReason::InsufficientPermissions => write!(f, "insufficient permissions"),
            OrphanReason::SandboxDenied => write!(f, "sandbox denied"),
        }
    }
}

/// Zero Orphan Guarantee — QEEP 零孤儿调用保证
/// 
/// ★ Insight: 这是 QEEP 协议的核心不变量，上提至 L6 后：
/// - L4 qeep-protocol 负责实现具体验证逻辑
/// - L7 gqep-executor 只需依赖 trait，不直接依赖 L4
pub trait ZeroOrphanGuarantee: Send + Sync {
    /// 验证调用是否满足零孤儿约束
    /// 
    /// # Arguments
    /// * `call_id` - 调用唯一标识
    /// 
    /// # Returns
    /// - `true`: 调用合法，非孤儿
    /// - `false`: 调用非法，是孤儿
    fn verify_zero_orphan(&self, call_id: &str) -> bool;
    
    /// 注册孤儿检测器（可选）
    fn register_orphan_detector(&mut self, _detector: Box<dyn OrphanDetector>) {
        // 默认空实现
    }
    
    /// 获取当前孤儿统计
    fn orphan_stats(&self) -> OrphanStats {
        OrphanStats::default()
    }
}

/// Orphan Detector — 孤儿检测器 trait
pub trait OrphanDetector: Send + Sync {
    /// 检测孤儿调用
    /// 
    /// # Arguments
    /// * `call_id` - 调用唯一标识
    /// 
    /// # Returns
    /// - `Some(reason)`: 检测到孤儿，返回原因
    /// - `None`: 非孤儿
    fn detect_orphan(&self, call_id: &str) -> Option<OrphanReason>;
}

/// Orphan Stats — 孤儿统计信息
#[derive(Debug, Clone, Default)]
pub struct OrphanStats {
    /// 总调用数
    pub total_calls: u64,
    
    /// 孤儿调用数
    pub orphan_calls: u64,
    
    /// 最近一次检测结果
    pub last_result: Option<VerificationResult>,
}

impl OrphanStats {
    /// 孤儿率
    pub fn orphan_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.orphan_calls as f64 / self.total_calls as f64
        }
    }
}
