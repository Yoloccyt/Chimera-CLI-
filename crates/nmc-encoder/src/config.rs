//! NMC 配置 — 感知器中间维度与融合策略配置
//!
//! 对应架构层:L2 Memory
//!
//! # 设计决策(WHY)
//! - **text_dim 默认 256**:文本感知器的中间嵌入维度。256 维字符频率统计
//!   足以区分不同文本(每个 UTF-8 字节值对应一个桶),同时保持低计算成本
//! - **clv_dim 必须等于 CLV::DIMENSION(512)**:融合输出必须与 nexus_core::CLV
//!   维度对齐,否则 CLV::from_vec 会拒绝构造。validate() 在此显式校验,
//!   提前暴露配置错误而非等到运行时 CLV 构造失败
//! - **FusionStrategy 三选一**:Concat(拼接)/Mean(平均)/Weighted(加权),
//!   不同场景适用不同策略(如单模态用 Concat,多模态对齐用 Weighted)

use serde::{Deserialize, Serialize};

use crate::error::NmcError;

/// 融合策略 — 多个 CognitiveElement 融合为 CLV 的方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FusionStrategy {
    /// 拼接策略:将所有 embedding 顺序拼接,截断/填充到 clv_dim
    Concat,
    /// 平均策略:对齐到最大维度后取平均,截断/填充到 clv_dim
    Mean,
    /// 加权策略:按模态权重加权求和(Text:0.5/Image:0.2/Video:0.1/Audio:0.1/Desktop:0.1)
    Weighted,
}

impl FusionStrategy {
    /// 返回策略名称字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Concat => "Concat",
            Self::Mean => "Mean",
            Self::Weighted => "Weighted",
        }
    }
}

/// NMC 编码器配置 — 感知器维度与融合策略
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NmcConfig {
    /// 文本感知器中间维度(默认 256,基于字符频率统计的桶数)
    pub text_dim: usize,
    /// 最终 CLV 维度(默认 512,必须等于 CLV::DIMENSION)
    pub clv_dim: usize,
    /// 融合策略(默认 Weighted)
    pub fusion_strategy: FusionStrategy,
}

impl NmcConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置文本感知器中间维度
    pub fn with_text_dim(mut self, dim: usize) -> Self {
        self.text_dim = dim;
        self
    }

    /// 设置 CLV 维度(必须为 512)
    pub fn with_clv_dim(mut self, dim: usize) -> Self {
        self.clv_dim = dim;
        self
    }

    /// 设置融合策略
    pub fn with_fusion_strategy(mut self, strategy: FusionStrategy) -> Self {
        self.fusion_strategy = strategy;
        self
    }

    /// 校验配置合法性
    ///
    /// 校验规则:
    /// - `text_dim` 必须 > 0(感知器需要至少 1 个桶)
    /// - `clv_dim` 必须等于 `CLV::DIMENSION`(512),否则 CLV::from_vec 会失败
    pub fn validate(&self) -> Result<(), NmcError> {
        if self.text_dim == 0 {
            return Err(NmcError::ConfigError {
                reason: "text_dim 不能为 0".into(),
            });
        }
        if self.clv_dim != nexus_core::CLV::DIMENSION {
            return Err(NmcError::ConfigError {
                reason: format!(
                    "clv_dim 必须等于 CLV::DIMENSION({}),当前为 {}",
                    nexus_core::CLV::DIMENSION,
                    self.clv_dim
                ),
            });
        }
        Ok(())
    }
}

impl Default for NmcConfig {
    fn default() -> Self {
        Self {
            text_dim: 256,
            clv_dim: nexus_core::CLV::DIMENSION,
            fusion_strategy: FusionStrategy::Weighted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NmcConfig::default();
        assert_eq!(config.text_dim, 256);
        assert_eq!(config.clv_dim, 512);
        assert_eq!(config.fusion_strategy, FusionStrategy::Weighted);
    }

    #[test]
    fn test_builder_chain() {
        let config = NmcConfig::new()
            .with_text_dim(128)
            .with_clv_dim(512)
            .with_fusion_strategy(FusionStrategy::Concat);
        assert_eq!(config.text_dim, 128);
        assert_eq!(config.clv_dim, 512);
        assert_eq!(config.fusion_strategy, FusionStrategy::Concat);
    }

    #[test]
    fn test_validate_valid() {
        let config = NmcConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_text_dim() {
        let config = NmcConfig::new().with_text_dim(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, NmcError::ConfigError { .. }));
    }

    #[test]
    fn test_validate_invalid_clv_dim() {
        let config = NmcConfig::new().with_clv_dim(256);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, NmcError::ConfigError { .. }));
        assert!(err.to_string().contains("512"));
    }

    #[test]
    fn test_fusion_strategy_as_str() {
        assert_eq!(FusionStrategy::Concat.as_str(), "Concat");
        assert_eq!(FusionStrategy::Mean.as_str(), "Mean");
        assert_eq!(FusionStrategy::Weighted.as_str(), "Weighted");
    }
}
