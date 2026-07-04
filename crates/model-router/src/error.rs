//! 路由器错误类型 — 库层 thiserror enum
//!
//! 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow

use thiserror::Error;

/// 模型路由器错误
#[derive(Debug, Error)]
pub enum RouterError {
    /// 模型未找到 — 指定 model_id 在注册表中不存在
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// 注册表为空 — 无任何已注册模型,无法路由
    #[error("no models registered")]
    NoModelsRegistered,

    /// 预算超限 — 预估成本超出预算上限
    #[error("budget exceeded: cost {cost} exceeds limit {limit}")]
    BudgetExceeded {
        /// 当前预估成本(美分)
        cost: u64,
        /// 预算上限(美分)
        limit: u64,
    },

    /// 事件总线错误 — 发布/订阅失败
    #[error("event bus error: {0}")]
    EventBusError(#[from] event_bus::EventBusError),

    /// 配置错误 — 配置解析或语义错误
    #[error("config error: {0}")]
    ConfigError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_not_found_display() {
        let e = RouterError::ModelNotFound("gpt-4".into());
        assert_eq!(e.to_string(), "model not found: gpt-4");
    }

    #[test]
    fn test_no_models_registered_display() {
        let e = RouterError::NoModelsRegistered;
        assert_eq!(e.to_string(), "no models registered");
    }

    #[test]
    fn test_budget_exceeded_display() {
        let e = RouterError::BudgetExceeded {
            cost: 100,
            limit: 50,
        };
        let msg = e.to_string();
        assert!(msg.contains("100"));
        assert!(msg.contains("50"));
    }
}
