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
    /// P0-10:已通过 `tokio::sync::Semaphore` 实现硬限流,
    /// 超过 `max_concurrency` 的 future 将等待 permit 释放后才启动。
    pub max_concurrency: usize,

    /// 全局 gather 超时阈值(毫秒) — 整个聚集流程的 deadline
    ///
    /// 与 `default_timeout_ms`(单操作超时)构成双层超时防护:
    /// - `default_timeout_ms`:保护单个 future(entangle 内 tokio::time::timeout)
    /// - `gather_deadline_ms`:保护整个 gather 流程,包裹 stream.next() 循环,
    ///   防止大规模 gather 时单操作超时累积导致整体执行时间失控。
    ///
    /// WHY 5000ms 默认值:5s 平衡了大规模 gather 的容错性与系统延迟。
    /// L6-L10 层典型 gather(沙箱执行/模型调用/Wiki 沉淀)多在 1-3s 内完成,
    /// 5s deadline 足以覆盖正常波动,同时为异常场景兜底,避免永久挂起。
    ///
    /// 特殊值:`0` 表示禁用全局超时(向后兼容,行为与无全局超时一致),
    /// 此时仅单操作超时生效。与 `with_timeout` 的 `timeout_ms=0` 语义保持一致。
    pub gather_deadline_ms: u64,

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
            gather_deadline_ms: 5000,
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
        assert_eq!(config.gather_deadline_ms, 5000);
        assert!(config.batch_atomic_enabled);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = GqepConfig {
            default_timeout_ms: 3000,
            max_concurrency: 50,
            gather_deadline_ms: 8000,
            batch_atomic_enabled: false,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: GqepConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.default_timeout_ms, 3000);
        assert_eq!(restored.max_concurrency, 50);
        assert_eq!(restored.gather_deadline_ms, 8000);
        assert!(!restored.batch_atomic_enabled);
    }

    #[test]
    fn test_config_clone() {
        let config = GqepConfig::default();
        let cloned = config.clone();
        assert_eq!(config.default_timeout_ms, cloned.default_timeout_ms);
        assert_eq!(config.gather_deadline_ms, cloned.gather_deadline_ms);
    }
}
