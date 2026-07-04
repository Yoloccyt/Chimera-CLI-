//! SSRA 配置定义
//!
//! 控制模板缓存容量、融合截止时间与默认 Top-K 值。
//! 配置项默认值经过权衡,适合大多数 L7 Execution 层快速适配场景。

use serde::{Deserialize, Serialize};

/// SSRA 配置 — 控制模板缓存与融合行为
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsraConfig {
    /// 模板缓存容量上限
    ///
    /// 默认 1024,平衡内存占用与命中率。
    /// WHY 1024:每个 SlimeTemplate 约 200-400 字节,1024 容量约占 0.4MB,
    /// 可容纳典型工作集;超出时按 compiled_at LRU 驱逐最旧模板。
    pub template_cache_size: usize,

    /// 默认融合截止时间(毫秒)
    ///
    /// 默认 20,对应 SSRA 设计目标 p95 ≤ 20ms。
    /// WHY 20:GLM 5.2 slime 机制要求 2 天合并专家的运行时融合延迟 < 20ms,
    /// 超出则返回 FusionTimeout,调用方降级为直接使用最强单模板。
    pub fusion_deadline_ms: u64,

    /// 默认 Top-K 值
    ///
    /// 默认 8,从源适配器模板中选出评分最高的 8 个参与融合。
    /// WHY 8:与 FaaE/GEA 的 Top-K=8 对齐,保持全链路一致性;
    /// 过大增加计算量,过小丢失多样性。
    pub top_k: usize,
}

impl Default for SsraConfig {
    fn default() -> Self {
        Self {
            template_cache_size: 1024,
            fusion_deadline_ms: 20,
            top_k: 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SsraConfig::default();
        assert_eq!(config.template_cache_size, 1024);
        assert_eq!(config.fusion_deadline_ms, 20);
        assert_eq!(config.top_k, 8);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = SsraConfig {
            template_cache_size: 512,
            fusion_deadline_ms: 15,
            top_k: 4,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: SsraConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.template_cache_size, 512);
        assert_eq!(restored.fusion_deadline_ms, 15);
        assert_eq!(restored.top_k, 4);
    }

    #[test]
    fn test_config_clone() {
        let config = SsraConfig::default();
        let cloned = config.clone();
        assert_eq!(config.template_cache_size, cloned.template_cache_size);
        assert_eq!(config.fusion_deadline_ms, cloned.fusion_deadline_ms);
        assert_eq!(config.top_k, cloned.top_k);
    }
}
