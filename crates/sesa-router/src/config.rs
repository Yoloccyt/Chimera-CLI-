//! SESA 配置定义
//!
//! 控制最大稀疏度比例、默认 Top-K 值与激活超时。
//! 配置项默认值经过权衡,适合大多数 L6 Router 稀疏激活场景。

use serde::{Deserialize, Serialize};

/// SESA 配置 — 控制子专家稀疏激活行为
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SesaConfig {
    /// 默认 Top-K 值
    ///
    /// 默认 8,从注册专家中选出评分最高的 8 个参与激活。
    /// WHY 8:与 FaaE/GEA 的 Top-K=8 对齐,保持全链路一致性;
    /// 过大增加计算量与稀疏度,过小丢失专家多样性。
    pub top_k: usize,

    /// 最大稀疏度比例 [0.0, 1.0]
    ///
    /// 默认 0.4(40%),对应架构要求"实测稀疏度严格 < 40%"。
    /// `enforce_sparsity` 会按此比例裁剪激活掩码,确保不超限。
    /// WHY 0.4:SESA 设计目标为激活专家数 < 总专家数的 40%,
    /// 平衡计算开销与专家覆盖度。
    pub max_sparsity_ratio: f32,

    /// 默认激活截止时间(毫秒)
    ///
    /// 默认 5,对应 SESA 设计目标 p95 ≤ 5ms。
    /// WHY 5:256-bit 掩码激活应在 5ms 内完成,超时返回 ActivationTimeout。
    pub activation_deadline_ms: u64,

    /// 掩码位宽(固定 256-bit,32 字节)
    ///
    /// 默认 256,对应 `SesaMask::bits` 数组长度为 32。
    /// WHY 固定 256:SESA 创新点要求 256-bit 位向量,可表示最多 256 个专家。
    /// 若专家数超过 256,应通过 KVBSR 粗筛降至 256 以内再激活。
    pub mask_width: usize,
}

impl Default for SesaConfig {
    fn default() -> Self {
        Self {
            top_k: 8,
            max_sparsity_ratio: 0.4,
            activation_deadline_ms: 5,
            mask_width: 256,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SesaConfig::default();
        assert_eq!(config.top_k, 8);
        assert!((config.max_sparsity_ratio - 0.4).abs() < f32::EPSILON);
        assert_eq!(config.activation_deadline_ms, 5);
        assert_eq!(config.mask_width, 256);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = SesaConfig {
            top_k: 16,
            max_sparsity_ratio: 0.3,
            activation_deadline_ms: 10,
            mask_width: 256,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: SesaConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.top_k, 16);
        assert!((restored.max_sparsity_ratio - 0.3).abs() < f32::EPSILON);
        assert_eq!(restored.activation_deadline_ms, 10);
        assert_eq!(restored.mask_width, 256);
    }

    #[test]
    fn test_config_clone() {
        let config = SesaConfig::default();
        let cloned = config.clone();
        assert_eq!(config.top_k, cloned.top_k);
        assert_eq!(config.mask_width, cloned.mask_width);
    }
}
