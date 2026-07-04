//! 专家画像实现 — ExpertProfile 的构造与辅助方法
//!
//! 对应架构层:L6 Router
//!
//! # 核心职责
//! - 提供 ExpertProfile 的构造函数
//! - 提供快照转换(用于序列化)
//! - 提供使用统计的原子更新方法
//!
//! # 设计决策(WHY)
//! - 构造函数初始化 usage_count = 0、last_used_at = Instant::now()
//! - 快照方法 `to_snapshot()` 提取当前状态用于序列化
//! - `from_snapshot()` 从快照重建 Profile,恢复 usage_count 但重置 last_used_at

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::types::{ExpertProfile, ExpertProfileSnapshot, ToolId};

impl ExpertProfile {
    /// 创建新的专家画像
    ///
    /// # 参数
    /// - `tool_id`:工具唯一标识(接受 `ToolId`/`String`/`&str`,通过 `Into<ToolId>` 转换)
    /// - `expert_vector`:64 维语义向量
    /// - `capability_tags`:能力标签列表
    /// - `priority`:优先级 [0.0, 1.0]
    pub fn new(
        tool_id: impl Into<ToolId>,
        expert_vector: Vec<f32>,
        capability_tags: Vec<String>,
        priority: f32,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            expert_vector,
            capability_tags,
            usage_count: std::sync::atomic::AtomicU64::new(0),
            last_used_at: Arc::new(RwLock::new(Instant::now())),
            priority,
        }
    }

    /// 创建带初始使用次数的专家画像(测试与快照恢复用)
    pub fn with_usage_count(
        tool_id: impl Into<ToolId>,
        expert_vector: Vec<f32>,
        capability_tags: Vec<String>,
        priority: f32,
        usage_count: u64,
    ) -> Self {
        let profile = Self::new(tool_id, expert_vector, capability_tags, priority);
        profile.usage_count.store(usage_count, Ordering::Relaxed);
        profile
    }

    /// 获取当前使用次数(原子读)
    pub fn get_usage_count(&self) -> u64 {
        self.usage_count.load(Ordering::Relaxed)
    }

    /// 原子递增使用次数,返回递增后的值
    pub fn increment_usage(&self) -> u64 {
        self.usage_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 原子设置使用次数(衰减循环用)
    pub fn set_usage_count(&self, count: u64) {
        self.usage_count.store(count, Ordering::Relaxed);
    }

    /// 获取专家向量维度
    pub fn dimension(&self) -> usize {
        self.expert_vector.len()
    }

    /// 转换为可序列化的快照
    ///
    /// WHY:`AtomicU64` 和 `RwLock<Instant>` 不实现 Serialize,
    /// 通过快照提取当前值用于序列化或日志记录
    pub async fn to_snapshot(&self) -> ExpertProfileSnapshot {
        let last_used = *self.last_used_at.read().await;
        let _ = last_used; // 快照不携带 Instant(不可跨进程序列化)
        ExpertProfileSnapshot {
            tool_id: self.tool_id.clone(),
            expert_vector: self.expert_vector.clone(),
            capability_tags: self.capability_tags.clone(),
            usage_count: self.get_usage_count(),
            priority: self.priority,
        }
    }

    /// 从快照重建专家画像
    ///
    /// 恢复 tool_id/expert_vector/capability_tags/usage_count/priority,
    /// last_used_at 重置为当前时刻(Instant 不可跨进程序列化)
    pub fn from_snapshot(snapshot: ExpertProfileSnapshot) -> Self {
        Self::with_usage_count(
            snapshot.tool_id,
            snapshot.expert_vector,
            snapshot.capability_tags,
            snapshot.priority,
            snapshot.usage_count,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_expert_profile_new() {
        let profile = ExpertProfile::new("tool-1", vec![0.1; 64], vec!["code-gen".into()], 0.8);
        assert_eq!(profile.tool_id.as_str(), "tool-1");
        assert_eq!(profile.dimension(), 64);
        assert_eq!(profile.capability_tags, vec!["code-gen"]);
        assert!((profile.priority - 0.8).abs() < 1e-6);
        assert_eq!(profile.get_usage_count(), 0);
    }

    #[tokio::test]
    async fn test_increment_usage() {
        let profile = ExpertProfile::new("t1", vec![0.0; 64], vec![], 0.5);
        assert_eq!(profile.get_usage_count(), 0);
        assert_eq!(profile.increment_usage(), 1);
        assert_eq!(profile.increment_usage(), 2);
        assert_eq!(profile.get_usage_count(), 2);
    }

    #[tokio::test]
    async fn test_set_usage_count() {
        let profile = ExpertProfile::new("t1", vec![0.0; 64], vec![], 0.5);
        profile.set_usage_count(100);
        assert_eq!(profile.get_usage_count(), 100);
    }

    #[tokio::test]
    async fn test_snapshot_roundtrip() {
        let profile = ExpertProfile::with_usage_count(
            "tool-snap",
            vec![0.5; 64],
            vec!["test".into(), "debug".into()],
            0.9,
            42,
        );
        let snapshot = profile.to_snapshot().await;
        assert_eq!(snapshot.tool_id.as_str(), "tool-snap");
        assert_eq!(snapshot.usage_count, 42);
        assert!((snapshot.priority - 0.9).abs() < 1e-6);
        assert_eq!(snapshot.capability_tags.len(), 2);

        let restored = ExpertProfile::from_snapshot(snapshot);
        assert_eq!(restored.tool_id.as_str(), "tool-snap");
        assert_eq!(restored.get_usage_count(), 42);
        assert!((restored.priority - 0.9).abs() < 1e-6);
    }
}
