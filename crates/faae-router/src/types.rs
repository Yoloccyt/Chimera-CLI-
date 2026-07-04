//! FaaE 核心领域类型 — Function-as-Expert 语义路由的统一数据模型
//!
//! 对应架构层:L6 Router
//! 对应创新点:FaaE(Function-as-Expert)— 工具即专家的语义化路由调度
//!
//! # 类型职责
//! - `ToolId`:工具唯一标识(newtype,与 KVBSR/OSA 共享同一命名空间)
//! - `ExpertProfile`:专家画像(工具 ID + 64 维语义向量 + 能力标签 + 使用统计 + 优先级)
//! - `RoutingResult`:路由结果(路由工具 + 置信度 + Top-K 候选)
//! - `EntropyStats`:熵统计(熵值 + 总使用量 + 工具数)
//!
//! # 设计决策(WHY)
//! - **expert_vector 维度 = 64**:与 KVBSR 的 block_vector_dim 对齐,降低存储与计算成本。
//!   路由时从 CLV(512 维)截取前 64 维作为查询向量
//! - **usage_count 用 AtomicU64**:支持无锁并发更新,route 路径无需获取写锁
//! - **last_used_at 用 `Arc<RwLock<Instant>>`**:读多写少(衰减循环读,路由路径写),
//!   Arc 包裹允许 clone 后锁外访问,消除嵌套锁跨 await(B-Crit-2/B-Crit-3 修复)
//! - **ToolId 用 nexus_core::id_newtype! 宏**:消除与 KVBSR/OSA 的 newtype 重复,
//!   统一 ID 类型行为(Deref / AsRef / Borrow / From / Display)

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// 使用 nexus_core 共享的 id_newtype! 宏
// WHY:消除与 kvbsr-router / osa-coordinator 的 newtype 实现重复(约 50 行手动实现),
// 统一 ID 类型行为(Deref / AsRef / Borrow / From / Display / serde(transparent))
nexus_core::id_newtype!(ToolId, "工具唯一标识 — 与 KVBSR/OSA 共享同一命名空间");

/// 专家画像 — 工具的语义化专家表示
///
/// 每个工具注册为一个"专家",携带 64 维语义向量用于路由匹配。
/// `usage_count` 与 `last_used_at` 支持并发更新(内部可变性),
/// 允许在 `ExpertProfile` 读锁下直接更新使用统计,无需获取写锁。
///
/// # 并发设计(WHY)
/// - `usage_count: AtomicU64`:无锁原子更新,route 路径高性能
/// - `last_used_at: Arc<RwLock<Instant>>`:读多写少,衰减循环读、路由路径写。
///   WHY 用 Arc 包裹:允许在 profile 读锁内 clone Arc 后释放,再锁外 await 获取
///   last_used_at 锁,消除嵌套锁跨 await(B-Crit-2/B-Crit-3 修复)。
///   `Arc<T>` Deref 到 `T`,原有 `.read().await`/`.write().await` 访问方式不变。
/// - `usage_count` 是原子类型,无需 profile 写锁即可更新
///
/// # 序列化注意
/// `AtomicU64` 和 `Arc<RwLock<Instant>>` 不实现 Serialize/Deserialize。
/// 序列化时通过 `ExpertProfileSnapshot` 转换,反序列化时重建为带默认计数的 Profile。
#[derive(Debug)]
pub struct ExpertProfile {
    /// 工具唯一标识
    pub tool_id: ToolId,
    /// 专家语义向量(64 维,与 KVBSR block_vector_dim 对齐)
    pub expert_vector: Vec<f32>,
    /// 能力标签列表(如 ["code-gen", "refactor", "test"])
    pub capability_tags: Vec<String>,
    /// 使用次数(原子计数,支持无锁并发更新)
    pub usage_count: AtomicU64,
    /// 最后使用时间(读多写少,用于指数衰减计算)
    ///
    /// WHY 用 `Arc<RwLock<Instant>>`:可在 profile 读锁内 clone Arc 后释放,
    /// 再锁外 await 获取读/写锁,避免嵌套锁跨 await(B-Crit-2/B-Crit-3 修复)
    pub last_used_at: Arc<RwLock<Instant>>,
    /// 优先级 [0.0, 1.0],影响路由评分(高优先级工具更易被选中)
    pub priority: f32,
}

/// 专家画像快照 — 可序列化的 ExpertProfile 表示
///
/// WHY:`AtomicU64` 和 `RwLock<Instant>` 不实现 Serialize/Deserialize,
/// 需要通过快照类型转换。序列化时提取当前值,反序列化时重建为带默认计数的 Profile。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpertProfileSnapshot {
    /// 工具唯一标识
    pub tool_id: ToolId,
    /// 专家语义向量(64 维)
    pub expert_vector: Vec<f32>,
    /// 能力标签列表
    pub capability_tags: Vec<String>,
    /// 使用次数快照(序列化时刻的值)
    pub usage_count: u64,
    /// 优先级
    pub priority: f32,
}

/// 路由结果 — FaaE 语义路由的输出
///
/// # 字段说明
/// - `routed_tool`:最终路由到的工具 ID(可能经 EDSB 均衡调整)
/// - `confidence`:路由置信度 [0.0, 1.0],即 CLV 与 expert_vector 的余弦相似度
/// - `candidates`:Top-K 候选列表(工具 ID + 相似度分数),按相似度降序
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingResult {
    /// 最终路由到的工具 ID
    pub routed_tool: ToolId,
    /// 路由置信度 [0.0, 1.0](Top-1 候选的余弦相似度)
    pub confidence: f32,
    /// Top-K 候选列表(工具 ID + 相似度分数),按相似度降序
    pub candidates: Vec<(ToolId, f32)>,
}

/// 熵统计 — EDSB 负载分布的度量结果
///
/// # 字段说明
/// - `entropy`:归一化香农熵 [0.0, 1.0],0 表示完全集中,1 表示完全均匀
/// - `total_usage`:所有工具的使用次数总和(衰减后)
/// - `tool_count`:参与统计的工具数量
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntropyStats {
    /// 归一化香农熵 [0.0, 1.0]
    pub entropy: f32,
    /// 所有工具的使用次数总和
    pub total_usage: u64,
    /// 参与统计的工具数量
    pub tool_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_id_newtype() {
        let id = ToolId::new("tool-1");
        assert_eq!(id.as_str(), "tool-1");
        let s: &str = &id;
        assert_eq!(s, "tool-1");
        assert_eq!(id.to_string(), "tool-1");
    }

    #[test]
    fn test_tool_id_from() {
        let id1 = ToolId::from(String::from("from-string"));
        assert_eq!(id1.as_str(), "from-string");

        let id2 = ToolId::from("from-str");
        assert_eq!(id2.as_str(), "from-str");
    }

    #[test]
    fn test_tool_id_eq_hash() {
        let id1 = ToolId::new("same");
        let id2 = ToolId::new("same");
        let id3 = ToolId::new("different");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);

        use std::collections::HashMap;
        let mut map: HashMap<ToolId, i32> = HashMap::new();
        map.insert(id1, 42);
        assert_eq!(map.get(&id2), Some(&42));
    }

    #[test]
    fn test_routing_result() {
        let result = RoutingResult {
            routed_tool: ToolId::new("t1"),
            confidence: 0.95,
            candidates: vec![(ToolId::new("t1"), 0.95), (ToolId::new("t2"), 0.80)],
        };
        assert_eq!(result.routed_tool.as_str(), "t1");
        assert!((result.confidence - 0.95).abs() < 1e-6);
        assert_eq!(result.candidates.len(), 2);
    }

    #[test]
    fn test_entropy_stats() {
        let stats = EntropyStats {
            entropy: 0.75,
            total_usage: 1000,
            tool_count: 10,
        };
        assert!((stats.entropy - 0.75).abs() < 1e-6);
        assert_eq!(stats.total_usage, 1000);
        assert_eq!(stats.tool_count, 10);
    }
}
