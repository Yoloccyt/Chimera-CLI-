//! 错误类型 — SecCore 库层错误(§4.1:库层用自定义 thiserror enum)
//!
//! 所有错误携带足够的上下文信息,便于审计追溯与上层决策。

use thiserror::Error;

use crate::types::AttackType;

/// SecCore 错误类型 — 零信任沙箱内所有失败路径的统一表示。
///
/// 设计原则:
/// - 拦截类错误(`CommandBlocked`/`EnvVarBlocked`)携带攻击类型与详情,便于审计
/// - 执行类错误(`SandboxError`)携带原始错误信息,便于诊断
/// - 审计类错误(`AuditError`)携带链状态信息,便于取证
#[derive(Debug, Error)]
pub enum SecCoreError {
    /// 命令被策略拦截 — 携带攻击类型与详情。
    ///
    /// 触发场景:命令注入、权限提升、沙箱逃逸、数据泄露、未授权命令。
    #[error("命令被拦截: 检测到 {attack_type:?} 攻击, 详情: {detail}")]
    CommandBlocked {
        /// 攻击类型(6 种之一)
        attack_type: AttackType,
        /// 拦截详情(人类可读)
        detail: String,
    },

    /// 环境变量被拦截 — 携带变量名与匹配的敏感模式。
    ///
    /// 触发场景:变量名匹配 SECRET/KEY/TOKEN/PASSWORD,或不在白名单内。
    #[error("环境变量被拦截: {name} 匹配敏感模式 ({pattern})")]
    EnvVarBlocked {
        /// 被拦截的变量名
        name: String,
        /// 匹配的敏感模式或 "not_in_whitelist"
        pattern: String,
    },

    /// 沙箱执行错误 — 进程启动、等待、信号处理等失败。
    #[error("沙箱执行错误: {0}")]
    SandboxError(String),

    /// 审计错误 — 审计链追加、验证、序列化失败。
    #[error("审计错误: {0}")]
    AuditError(String),

    /// 策略违反 — 策略配置错误(如正则编译失败、白名单冲突)。
    #[error("策略违反: {0}")]
    PolicyViolation(String),

    /// ASA 对抗审计拦截 — 操作被 AsaAuditor 判定为 Block 级别。
    ///
    /// 触发场景:`safety_score < safety_threshold_block`(默认 0.5),
    /// 操作存在高风险关键字或历史失败率过高。
    /// 事中拦截优先于沙箱执行,被拦截的操作不进入沙箱。
    #[error("ASA 拦截: 操作 {operation_id} 被阻断, 原因: {block_reason}")]
    AsaBlocked {
        /// 被拦截的操作 ID
        operation_id: String,
        /// 阻断原因(人类可读,用于审计追溯)
        block_reason: String,
    },
}
