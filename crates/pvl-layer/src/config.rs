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

    /// 操作验证超时(毫秒,默认 5000 = 5 秒)
    ///
    /// WHY(P1-9):Verifier 处理单个操作的最大时间限制。
    /// 超过此时间,操作被标记为超时拒绝,避免阻塞整个流水线。
    /// 5 秒覆盖大多数验证场景(语法检查+安全扫描+依赖检查),
    /// 复杂操作可在 Verifier 内部分段处理。
    pub verification_timeout_ms: u64,

    /// 生产者流式超时(毫秒,默认 30000 = 30 秒)
    ///
    /// WHY(P1-9):produce() 批量生成操作的总时间限制。
    /// 超过此时间,未完成的操作被丢弃,Producer 策略降级为 Conservative。
    pub produce_timeout_ms: u64,

    /// 分块大小（Producer分块生成时的默认块大小）
    ///
    /// 默认128，与mpsc通道容量对齐，减少内存峰值。
    /// WHY 128: 参考FlashAttention的tile大小，与通道容量一致。
    pub chunk_size: usize,

    /// 最小分块大小
    ///
    /// 默认16，防止分块过小导致调度overhead。
    pub min_chunk_size: usize,

    /// 最大分块大小
    ///
    /// 默认512，限制单块内存占用。
    pub max_chunk_size: usize,

    /// 启用优先级调度
    ///
    /// 默认true，操作数超过阈值时启用注意力权重调度。
    pub enable_priority_scheduling: bool,

    /// 优先级调度阈值
    ///
    /// 操作数超过此值才启用优先级调度，避免小批量overhead。
    /// 默认32。
    pub priority_threshold: usize,
}

impl Default for PvlConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 128,
            rejection_rate_threshold: 0.3,
            producer_rate_limit: 10,
            verification_timeout_ms: 5000,
            produce_timeout_ms: 30000,
            chunk_size: 128,
            min_chunk_size: 16,
            max_chunk_size: 512,
            enable_priority_scheduling: true,
            priority_threshold: 32,
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
        assert_eq!(
            config.rejection_rate_threshold.total_cmp(&0.3f32),
            std::cmp::Ordering::Equal
        );
        assert_eq!(config.producer_rate_limit, 10);
        assert_eq!(config.verification_timeout_ms, 5000);
        assert_eq!(config.produce_timeout_ms, 30000);
        assert_eq!(config.chunk_size, 128);
        assert_eq!(config.min_chunk_size, 16);
        assert_eq!(config.max_chunk_size, 512);
        assert!(config.enable_priority_scheduling);
        assert_eq!(config.priority_threshold, 32);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = PvlConfig {
            channel_capacity: 256,
            rejection_rate_threshold: 0.2,
            producer_rate_limit: 20,
            verification_timeout_ms: 5000,
            produce_timeout_ms: 30000,
            chunk_size: 256,
            min_chunk_size: 32,
            max_chunk_size: 512,
            enable_priority_scheduling: true,
            priority_threshold: 64,
        };
        let json = match serde_json::to_string(&config) {
            Ok(s) => s,
            Err(e) => panic!("序列化失败: {}", e),
        };
        let restored: PvlConfig = match serde_json::from_str(&json) {
            Ok(c) => c,
            Err(e) => panic!("反序列化失败: {}", e),
        };
        assert_eq!(restored.channel_capacity, 256);
        assert_eq!(
            restored.rejection_rate_threshold.total_cmp(&0.2f32),
            std::cmp::Ordering::Equal
        );
        assert_eq!(restored.producer_rate_limit, 20);
        assert_eq!(restored.chunk_size, 256);
        assert_eq!(restored.min_chunk_size, 32);
        assert_eq!(restored.max_chunk_size, 512);
        assert!(restored.enable_priority_scheduling);
        assert_eq!(restored.priority_threshold, 64);
    }

    #[test]
    fn test_config_clone() {
        let config = PvlConfig::default();
        let cloned = config.clone();
        assert_eq!(config.channel_capacity, cloned.channel_capacity);
        assert_eq!(config.producer_rate_limit, cloned.producer_rate_limit);
        assert_eq!(config.chunk_size, cloned.chunk_size);
        assert_eq!(config.min_chunk_size, cloned.min_chunk_size);
        assert_eq!(config.max_chunk_size, cloned.max_chunk_size);
        assert_eq!(
            config.enable_priority_scheduling,
            cloned.enable_priority_scheduling
        );
        assert_eq!(config.priority_threshold, cloned.priority_threshold);
    }
}
