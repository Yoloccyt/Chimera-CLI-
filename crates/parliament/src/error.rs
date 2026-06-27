//! Parliament 错误类型 — 库层错误用 thiserror enum(§4.1)
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)

use thiserror::Error;

/// Parliament 错误类型
///
/// WHY thiserror:库层错误用自定义 enum(§4.1),应用层才用 anyhow。
/// 所有变体携带足够上下文,便于调用方定位问题。
#[derive(Debug, Error)]
pub enum ParliamentError {
    /// 指定角色未在注册表中找到
    #[error("role not found: {role_id}")]
    RoleNotFound {
        /// 未找到的角色 ID
        role_id: String,
    },

    /// 辩论超时(5 角色未在 timeout_ms 内全部完成)
    ///
    /// WHY:对应架构红线"所有异步操作必须有 GQEP 聚集/超时处理",
    /// 辩论超时视为拒绝(避免孤儿调用)
    #[error("debate timed out after {timeout_ms}ms")]
    DebateTimeout {
        /// 超时阈值(毫秒)
        timeout_ms: u64,
    },

    /// 法定人数不足(参与率 < quorum_threshold)
    #[error("quorum not met: participation {participation} < required {required}")]
    QuorumNotMet {
        /// 实际参与率
        participation: f32,
        /// 要求的参与率
        required: f32,
    },

    /// 否决失败(如 Skeptic 角色不存在或无否决权)
    #[error("veto failed: {reason}")]
    VetoFailed {
        /// 否决失败原因
        reason: String,
    },

    /// 配置错误(如权重和不为 1.0、阈值为负等)
    #[error("config error: {detail}")]
    ConfigError {
        /// 配置错误详情
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_not_found_display() {
        let err = ParliamentError::RoleNotFound {
            role_id: "role-1".into(),
        };
        assert!(err.to_string().contains("role-1"));
    }

    #[test]
    fn test_debate_timeout_display() {
        let err = ParliamentError::DebateTimeout { timeout_ms: 5000 };
        assert!(err.to_string().contains("5000"));
    }

    #[test]
    fn test_quorum_not_met_display() {
        let err = ParliamentError::QuorumNotMet {
            participation: 0.4,
            required: 0.6,
        };
        let msg = err.to_string();
        assert!(msg.contains("0.4"));
        assert!(msg.contains("0.6"));
    }

    #[test]
    fn test_veto_failed_display() {
        let err = ParliamentError::VetoFailed {
            reason: "skeptic missing".into(),
        };
        assert!(err.to_string().contains("skeptic missing"));
    }

    #[test]
    fn test_config_error_display() {
        let err = ParliamentError::ConfigError {
            detail: "weights sum != 1.0".into(),
        };
        assert!(err.to_string().contains("weights sum != 1.0"));
    }
}
