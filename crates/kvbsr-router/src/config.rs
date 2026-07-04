//! KVBSR 配置 — 两级语义块路由的参数化配置
//!
//! 对应架构层:L6 Router
//!
//! # 设计决策(WHY)
//! - **co_occurrence_threshold 默认 100**:共现频率 > 100 的工具归入同一块。
//!   100 次共现表明工具间存在强相关性(如同一工作流的步骤),低于此值可能为偶然共现
//! - **block_vector_dim 默认 64**:工具/块向量维度。低于 CLV 的 512 维以降低存储与计算成本,
//!   路由时从 CLV 截取前 64 维作为查询向量(见 `router::clv_to_block_dim`)
//! - **top_blocks 默认 3**:一级路由选取的块数。3 个块覆盖约 60-90 个工具(300 工具规模),
//!   平衡召回率与计算成本
//! - **top_tools 默认 8**:二级路由选取的工具数。8 个工具覆盖大多数任务需求,
//!   与 OSA 的 routing Top-K 下界对齐(§OSA 配置)
//! - **rebalance_interval 默认 1000**:每 1000 次路由自动触发重平衡。
//!   1000 次路由足以积累新的共现模式,又不至于频繁重平衡影响性能

use serde::{Deserialize, Serialize};

use crate::error::KvbsrError;

/// KVBSR 路由器配置 — 两级语义块路由的参数化控制
///
/// 所有字段在创建时填充,不可变(无需内部可变性)。
/// `validate()` 方法校验配置合法性,`Default` 提供架构手册推荐的默认值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KvbsrConfig {
    /// 共现频率阈值(默认 100),共现频率 > 此值的工具归入同一块
    ///
    /// WHY:100 次共现表明工具间存在强相关性(如同一工作流的步骤),
    /// 低于此值可能为偶然共现,不应聚类
    pub co_occurrence_threshold: u32,

    /// 块/工具向量维度(默认 64),低于 CLV 的 512 维以降低成本
    ///
    /// WHY:64 维在表达力与计算成本间平衡。路由时从 CLV 截取前 64 维作为查询向量,
    /// 前 64 维承载足够区分度(测试验证准确率 > 85%)
    ///
    /// # CLV 维度对齐(SubTask 14.8)
    /// 64-dim 是 CLV 512-dim 的降维投影。当前采用**截取前 64 维**作为临时方案
    /// (实现简单、确定性高);Week 6 NMC 编码器实现后,将接入 PCA 降维,
    /// 用学习到的投影矩阵替代截取,进一步提升区分度与路由准确率。
    /// 详见 `router::clv_to_block_dim` 的降维实现。
    pub block_vector_dim: usize,

    /// 一级路由选取的块数(默认 3)
    ///
    /// WHY:3 个块覆盖约 60-90 个工具(300 工具规模,15 块 × 20 工具/块),
    /// 平衡召回率与计算成本。过多块增加二级路由计算量,过少块降低召回率
    pub top_blocks: usize,

    /// 二级路由选取的工具数(默认 8)
    ///
    /// WHY:8 个工具覆盖大多数任务需求,与 OSA 的 routing Top-K 下界对齐。
    /// 过多工具增加下游执行负担,过少工具可能遗漏关键能力
    pub top_tools: usize,

    /// 自动重平衡间隔(默认 1000),每 N 次路由自动触发重平衡
    ///
    /// WHY:1000 次路由足以积累新的共现模式(新工具使用、工作流变化),
    /// 又不至于频繁重平衡影响性能。重平衡在独立任务中异步执行,不阻塞路由
    pub rebalance_interval: u64,
}

impl KvbsrConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置共现频率阈值
    pub fn with_co_occurrence_threshold(mut self, threshold: u32) -> Self {
        self.co_occurrence_threshold = threshold;
        self
    }

    /// 设置块/工具向量维度
    pub fn with_block_vector_dim(mut self, dim: usize) -> Self {
        self.block_vector_dim = dim;
        self
    }

    /// 设置一级路由选取的块数
    pub fn with_top_blocks(mut self, n: usize) -> Self {
        self.top_blocks = n;
        self
    }

    /// 设置二级路由选取的工具数
    pub fn with_top_tools(mut self, n: usize) -> Self {
        self.top_tools = n;
        self
    }

    /// 设置自动重平衡间隔
    pub fn with_rebalance_interval(mut self, n: u64) -> Self {
        self.rebalance_interval = n;
        self
    }

    /// 校验配置合法性,返回 KvbsrError 描述具体问题
    ///
    /// 校验规则:
    /// - `block_vector_dim > 0`:向量维度不能为 0
    /// - `top_blocks > 0`:一级路由必须至少选 1 个块
    /// - `top_tools > 0`:二级路由必须至少选 1 个工具
    /// - `rebalance_interval > 0`:重平衡间隔不能为 0
    pub fn validate(&self) -> Result<(), KvbsrError> {
        if self.block_vector_dim == 0 {
            return Err(KvbsrError::InvalidConfig(
                "block_vector_dim 不能为 0".into(),
            ));
        }
        if self.top_blocks == 0 {
            return Err(KvbsrError::InvalidConfig("top_blocks 不能为 0".into()));
        }
        if self.top_tools == 0 {
            return Err(KvbsrError::InvalidConfig("top_tools 不能为 0".into()));
        }
        if self.rebalance_interval == 0 {
            return Err(KvbsrError::InvalidConfig(
                "rebalance_interval 不能为 0".into(),
            ));
        }
        Ok(())
    }
}

impl Default for KvbsrConfig {
    fn default() -> Self {
        Self {
            co_occurrence_threshold: 100,
            block_vector_dim: 64,
            top_blocks: 3,
            top_tools: 8,
            rebalance_interval: 1000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = KvbsrConfig::default();
        assert_eq!(config.co_occurrence_threshold, 100);
        assert_eq!(config.block_vector_dim, 64);
        assert_eq!(config.top_blocks, 3);
        assert_eq!(config.top_tools, 8);
        assert_eq!(config.rebalance_interval, 1000);
    }

    #[test]
    fn test_validate_valid() {
        let config = KvbsrConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_dim() {
        let config = KvbsrConfig::new().with_block_vector_dim(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, KvbsrError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_zero_top_blocks() {
        let config = KvbsrConfig::new().with_top_blocks(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, KvbsrError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_zero_top_tools() {
        let config = KvbsrConfig::new().with_top_tools(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, KvbsrError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_zero_rebalance_interval() {
        let config = KvbsrConfig::new().with_rebalance_interval(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, KvbsrError::InvalidConfig(_)));
    }

    #[test]
    fn test_builder_chain() {
        let config = KvbsrConfig::new()
            .with_co_occurrence_threshold(50)
            .with_block_vector_dim(128)
            .with_top_blocks(5)
            .with_top_tools(16)
            .with_rebalance_interval(500);
        assert_eq!(config.co_occurrence_threshold, 50);
        assert_eq!(config.block_vector_dim, 128);
        assert_eq!(config.top_blocks, 5);
        assert_eq!(config.top_tools, 16);
        assert_eq!(config.rebalance_interval, 500);
        assert!(config.validate().is_ok());
    }
}
