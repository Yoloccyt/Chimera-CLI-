//! MTPE 配置 — 执行器运行参数
//!
//! 对应架构层:L7 Execution
//!
//! # 默认值依据
//! - `max_n = 10`:架构决策上限,超过 10 步预测成功率过低(见 spec.md)
//! - `success_rate_threshold = 0.8`:低于此阈值触发 N 值降级
//! - `rollback_enabled = true`:默认启用回退,失败步回退到单步预测

use serde::{Deserialize, Serialize};

/// MTPE 执行器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpeConfig {
    /// 最大预测步数,有效范围 [1, max_n]
    pub max_n: usize,
    /// 成功率阈值,低于此值时考虑降级 N
    pub success_rate_threshold: f32,
    /// 是否启用失败回退(关闭时失败直接返回错误)
    pub rollback_enabled: bool,
    /// 推理服务端点(如 "http://203.0.113.1:8080/v1/predict")
    /// None 表示使用 Mock 模式
    pub inference_endpoint: Option<String>,
    /// 推理请求超时(毫秒)
    pub inference_timeout_ms: u64,
    /// 是否启用 Mock 推理模式(跳过网络,直接返回伪预测)
    pub inference_mock: bool,
}

impl Default for MtpeConfig {
    fn default() -> Self {
        Self {
            max_n: 10,
            success_rate_threshold: 0.8,
            rollback_enabled: true,
            inference_endpoint: None,
            inference_timeout_ms: 5000,
            inference_mock: true,
        }
    }
}

impl MtpeConfig {
    /// 校验 N 值是否在有效范围 [1, max_n]
    ///
    /// WHY 内联:校验逻辑简单且高频,内联避免函数调用开销
    pub fn is_valid_n(&self, n: usize) -> bool {
        n >= 1 && n <= self.max_n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MtpeConfig::default();
        assert_eq!(config.max_n, 10);
        assert!((config.success_rate_threshold - 0.8).abs() < f32::EPSILON);
        assert!(config.rollback_enabled);
        assert!(config.inference_endpoint.is_none());
        assert_eq!(config.inference_timeout_ms, 5000);
        assert!(config.inference_mock);
    }

    #[test]
    fn test_is_valid_n() {
        let config = MtpeConfig::default();
        assert!(!config.is_valid_n(0));
        assert!(config.is_valid_n(1));
        assert!(config.is_valid_n(5));
        assert!(config.is_valid_n(10));
        assert!(!config.is_valid_n(11));
    }

    #[test]
    fn test_config_serialization() {
        let config = MtpeConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let restored: MtpeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.max_n, restored.max_n);
        assert_eq!(
            config.success_rate_threshold,
            restored.success_rate_threshold
        );
        assert_eq!(config.inference_mock, restored.inference_mock);
        assert_eq!(config.inference_timeout_ms, restored.inference_timeout_ms);
    }
}
