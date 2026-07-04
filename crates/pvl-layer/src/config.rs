//! PVL 配置定义
//!
//! 控制生产验证闭环的通道容量、拒绝率阈值与生产速率。
//! 配置项默认值经过权衡,适合大多数 L7 Execution 层操作场景。

use serde::{Deserialize, Serialize};

/// PVL 配置 — 控制生产验证闭环行为
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvlConfig {
    /// 通道容量(Producer→Verifier 与 Verifier→Feedback 共用)
    ///
    /// 默认 128,平衡内存占用与背压控制。
    /// WHY 128:每个 Operation 约 100-500 字节,128 容量约占 64KB,
    /// 可吸收短时突发;持续高吞吐时 Producer 会被背压阻塞(避免内存爆炸)
    pub channel_capacity: usize,

    /// 拒绝率阈值 — 触发 Producer 策略调整的拒绝率上限
    ///
    /// 默认 0.3(30%),拒绝率超过此值时 FeedbackChannel 将 Producer
    /// 策略调整为 Conservative(降速+提阈值)。
    /// WHY 0.3:经验值,30% 拒绝率意味着 Producer 生成质量显著下降,
    /// 需要降速止损;低于 30% 属正常波动
    pub rejection_rate_threshold: f32,

    /// 生产者速率限制(操作/秒)
    ///
    /// 默认 10,限制 Producer 每秒生成的操作数,避免淹没 Verifier。
    /// WHY 10:L7 层操作(如代码生成、命令执行)通常需要 100ms+ 验证,
    /// 10 操作/秒与单线程 Verifier 处理能力匹配
    pub producer_rate_limit: u32,
}

impl Default for PvlConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 128,
            rejection_rate_threshold: 0.3,
            producer_rate_limit: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PvlConfig::default();
        assert_eq!(config.channel_capacity, 128);
        assert!((config.rejection_rate_threshold - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.producer_rate_limit, 10);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = PvlConfig {
            channel_capacity: 256,
            rejection_rate_threshold: 0.2,
            producer_rate_limit: 20,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: PvlConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.channel_capacity, 256);
        assert!((restored.rejection_rate_threshold - 0.2).abs() < f32::EPSILON);
        assert_eq!(restored.producer_rate_limit, 20);
    }

    #[test]
    fn test_config_clone() {
        let config = PvlConfig::default();
        let cloned = config.clone();
        assert_eq!(config.channel_capacity, cloned.channel_capacity);
        assert_eq!(config.producer_rate_limit, cloned.producer_rate_limit);
    }
}
