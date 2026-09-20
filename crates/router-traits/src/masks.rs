//! Sparse Mask Provider Trait — OSA 稀疏掩码提供者接口

use nexus_contracts::OmniSparseMasks;
use thiserror::Error;

/// Sparse Mask Provider Error
#[derive(Error, Debug, Clone, PartialEq)]
pub enum SparseMaskError {
    /// 掩码更新失败
    #[error("mask update failed: {0}")]
    UpdateFailed(String),

    /// 掩码为空
    #[error("masks are empty")]
    EmptyMasks,
}

/// Sparse Mask Provider Trait — OSA 稀疏掩码提供者接口
///
/// ★ Insight: 将 osa-coordinator 的具体实现抽象为 trait，使其他 router crate
/// 可以依赖抽象而非具体实现，消除星型耦合。
pub trait SparseMaskProvider: Send + Sync {
    /// 获取当前工具稀疏掩码
    fn tool_masks(&self) -> &OmniSparseMasks;

    /// 按需更新掩码（可选，默认静态）
    ///
    /// Returns:
    /// - `true`: 掩码已更新
    /// - `false`: 掩码未变化或无需更新
    fn update_masks_if_needed(&mut self) -> bool {
        false // 默认不更新
    }
}
