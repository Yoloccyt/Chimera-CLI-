//! GQEP 配置定义
//!
//! 控制聚集执行的超时、并发度与原子性行为。
//! 配置项默认值经过权衡,适合大多数 L6-L10 层操作场景。

use serde::{Deserialize, Serialize};

/// GQEP 执行器配置
///
/// 控制聚集执行的超时、并发度与原子性行为。
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GqepConfig {
    /// 默认操作超时(毫秒)
    ///
    /// 单个操作超过此时间未完成则触发 `OperationTimeout`。
    /// 默认 5000ms,平衡响应性与长操作容忍度。
    ///
    /// WHY 5000ms:L6-L10 层操作(如沙箱执行、模型调用、Wiki 沉淀)
    /// 通常在 1-3 秒内完成,5 秒超时足够覆盖正常波动,
    /// 同时避免长时间挂起(对应尸检教训:无超时导致永久挂起)。
    pub default_timeout_ms: u64,

    /// 最大并发操作数
    ///
    /// 限制 `FuturesUnordered` 中同时 pending 的 future 数量,
    /// 防止资源耗尽(对应架构红线:1M Token 暴力加载)。
    /// 默认 100,适合大多数 L6-L10 层操作。
    ///
    /// NOTE:当前实现暂未强制限流(`FuturesUnordered` 一次性放入所有 future)。
    /// 未来可通过 `futures::stream::buffer_unordered` 实现限流,
    /// 或分批聚集。保留此字段用于未来扩展与配置一致性。
    #[allow(dead_code)]
    pub max_concurrency: usize,

    /// 是否启用批量原子性
    ///
    /// 启用后,`gather_atomic` 中任一操作失败将触发回滚。
    /// 默认 true,保证批量操作的一致性。
    pub batch_atomic_enabled: bool,
}

impl Default for GqepConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 5000,
            max_concurrency: 100,
            batch_atomic_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GqepConfig::default();
        assert_eq!(config.default_timeout_ms, 5000);
        assert_eq!(config.max_concurrency, 100);
        assert!(config.batch_atomic_enabled);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = GqepConfig {
            default_timeout_ms: 3000,
            max_concurrency: 50,
            batch_atomic_enabled: false,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: GqepConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.default_timeout_ms, 3000);
        assert_eq!(restored.max_concurrency, 50);
        assert!(!restored.batch_atomic_enabled);
    }

    #[test]
    fn test_config_clone() {
        let config = GqepConfig::default();
        let cloned = config.clone();
        assert_eq!(config.default_timeout_ms, cloned.default_timeout_ms);
    }
}
