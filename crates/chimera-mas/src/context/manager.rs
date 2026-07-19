//! AgentContext 上下文管理 — 1M Token 等效上下文的分层加载与稀疏化
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 包装 `hcw_window::HcwWindow` 实现 1M Token 等效上下文,
//!           经 HCW 稀疏化 + OSA 五维度掩码实现 Ω-Compress + Ω-Sparse。
//!
//! ## ADR-026 决策 7: 不自实现压缩
//!
//! `AgentContext` 不自实现压缩逻辑,而是委托给:
//! - `hcw_window::HcwWindow::select_window()` — 四级窗口分层选择(4K/32K/128K/1M)
//! - `hcw_window::HcwWindow::apply_sparse_mask()` — 按活跃文件 ID 稀疏化
//! - `osa_coordinator::OmniSparseCoordinator::compute_all_masks()` — 五维度稀疏掩码计算
//!
//! ## ContextBlock 优先级映射(ADR-026 决策 7)
//!
//! | 块类型 | 优先级 | 行为 |
//! |--------|--------|------|
//! | system_prompt | Critical | 永不压缩,强制加入 active_file_ids |
//! | user_intent | High | 优先保留 |
//! | task_context | Normal | 按需压缩 |
//! | wiki_knowledge | Optional | 可完全丢弃(OSA 未选中时不包含在输出) |
//!
//! ## 1M Token 等效机制(Ω-Compress)
//!
//! 1M Token 上下文 = 128K 实际加载 + 8× 稀疏压缩。
//! L3 层级容量 1M,但通过 OSA context_mask 仅加载活跃文件,
//! 实际加载 ≤ 128K(1M / 8),避免内存爆炸(§6.1 红线)。

use std::collections::HashSet;
use std::fmt;

use event_bus::EventBus;
use hcw_window::{ContextEntry, HcwWindow};
use osa_coordinator::{
    AffectedScope, FileId, OmniSparseCoordinator, RiskLevel, TaskId, TaskProfile, TaskType,
    TimePressure,
};
use serde::{Deserialize, Serialize};

use crate::error::{MasError, Result};

// ============================================================
// ContextPriority — 上下文块优先级枚举
// ============================================================

/// 上下文块优先级 — 决定保留与压缩顺序
///
/// 排序(derive Ord): `Critical > High > Normal > Low > Optional`
///
/// WHY 声明顺序: Rust derive Ord 按变体声明顺序,先声明者值更小。
/// 因此按 `Optional → Critical` 升序声明,自动满足 `Critical` 最大。
///
/// - `Critical`: 永不压缩(如 system_prompt),`is_compressible = false`
/// - `High`: 优先保留(如 user_intent)
/// - `Normal`: 标准上下文(如 task_context)
/// - `Low`: 按需压缩(如日志)
/// - `Optional`: 可完全丢弃(如 wiki_knowledge)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ContextPriority {
    /// 可选 — 最低优先级,可完全丢弃
    Optional,
    /// 低优先级 — 按需压缩
    Low,
    /// 正常优先级 — 标准上下文
    Normal,
    /// 高优先级 — 优先保留
    High,
    /// 关键 — 最高优先级,永不压缩
    Critical,
}

impl ContextPriority {
    /// 是否为 Critical 优先级
    pub fn is_critical(self) -> bool {
        self == Self::Critical
    }

    /// 是否为 Optional 优先级
    pub fn is_optional(self) -> bool {
        self == Self::Optional
    }
}

// ============================================================
// ContextBlock — 上下文块结构
// ============================================================

/// 上下文块 — Agent 上下文的最小组成单元
///
/// 每个块携带优先级与可压缩标志。Critical 块 `is_compressible = false`,
/// 永不被 HCW 稀疏化丢弃(ADR-026 决策 7 红线)。
///
/// `name` 字段同时作为 HCW 的 `file_id`,用于 OSA context_mask 稀疏化路由。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBlock {
    /// 块名称(同时作为 HCW file_id,用于 OSA 稀疏化路由)
    pub name: String,
    /// 块内容文本
    pub content: String,
    /// 块 Token 数(由调用方估算)
    pub tokens: usize,
    /// 块优先级
    pub priority: ContextPriority,
    /// 是否可压缩 — Critical 块为 false,其他为 true
    pub is_compressible: bool,
}

impl ContextBlock {
    /// 创建新上下文块
    ///
    /// Critical 块自动设置 `is_compressible = false`(ADR-026 决策 7 红线)。
    ///
    /// ## 参数
    /// - `name`: 块名称(同时作为 HCW file_id)
    /// - `content`: 块内容文本
    /// - `tokens`: 块 Token 数
    /// - `priority`: 块优先级
    pub fn new(
        name: impl Into<String>,
        content: impl Into<String>,
        tokens: usize,
        priority: ContextPriority,
    ) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
            tokens,
            priority,
            // Critical 块永不可压缩(ADR-026 红线)
            is_compressible: !priority.is_critical(),
        }
    }
}

// ============================================================
// AgentContext — Agent 独立上下文
// ============================================================

/// Agent 独立上下文 — 1M Token 等效,经 HCW 稀疏化
///
/// 包装 `hcw_window::HcwWindow`(ADR-026 决策 7,不自实现压缩)。
/// 1M Token = 128K 实际加载 + 8× 稀疏压缩(Ω-Compress)。
///
/// WHY 不派生 Clone/Serialize/Deserialize:
/// `HcwWindow` 内部含 `Arc<RwLock<HcwState>>`,非 Clone/Serializable。
/// AgentContext 持有 `EventBus`(Arc-based,Clone 廉价)用于创建临时 HcwWindow。
///
/// ## 字段说明
///
/// - `agent_id`: 所属 Agent ID(用于隔离守卫校验)
/// - `max_tokens`: 最大 Token 预算(1M 等效)
/// - `current_tokens`: 当前已用 Token 数(实际加载,非稀疏后)
/// - `blocks`: 上下文块列表(按添加顺序存储,build_prompt 时按优先级排序)
/// - `event_bus`: 事件总线(创建临时 HcwWindow + OmniSparseCoordinator)
pub struct AgentContext {
    /// 所属 Agent ID
    pub agent_id: String,
    /// 最大 Token 预算(1M 等效 = 128K 实际 + 8× 稀疏)
    pub max_tokens: usize,
    /// 当前已用 Token 数(实际加载)
    pub current_tokens: usize,
    /// 上下文块列表
    blocks: Vec<ContextBlock>,
    /// 事件总线(创建临时 HcwWindow + OSA coordinator)
    event_bus: EventBus,
}

impl fmt::Debug for AgentContext {
    /// 手动实现 Debug,避免依赖 EventBus 的 Debug 实现
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentContext")
            .field("agent_id", &self.agent_id)
            .field("max_tokens", &self.max_tokens)
            .field("current_tokens", &self.current_tokens)
            .field("blocks_count", &self.blocks.len())
            .finish()
    }
}

impl AgentContext {
    /// 创建新的 Agent 上下文
    ///
    /// ## 参数
    /// - `agent_id`: 所属 Agent ID
    /// - `max_tokens`: 最大 Token 预算(1M 等效,如 `1_048_576`)
    /// - `event_bus`: 事件总线(HCW + OSA 内部通信所需)
    ///
    /// ## 示例
    ///
    /// ```no_run
    /// use chimera_mas::prelude::*;
    /// use event_bus::EventBus;
    ///
    /// let ctx = AgentContext::new("agent-1", 1_048_576, EventBus::new()).unwrap();
    /// assert_eq!(ctx.max_tokens, 1_048_576);
    /// ```
    pub fn new(
        agent_id: impl Into<String>,
        max_tokens: usize,
        event_bus: EventBus,
    ) -> Result<Self> {
        Ok(Self {
            agent_id: agent_id.into(),
            max_tokens,
            current_tokens: 0,
            blocks: Vec::new(),
            event_bus,
        })
    }

    /// 添加上下文块
    ///
    /// 超过 `max_tokens` 时返回 `MasError::TokenBudgetExceeded`,块不被添加。
    ///
    /// ## 参数
    /// - `block`: 要添加的上下文块
    pub fn add_block(&mut self, block: ContextBlock) -> Result<()> {
        let new_tokens = self.current_tokens + block.tokens;
        if new_tokens > self.max_tokens {
            return Err(MasError::TokenBudgetExceeded {
                agent_id: self.agent_id.clone(),
                // current_tokens 记录尝试达到的总量(含被拒绝块),便于错误诊断
                current_tokens: new_tokens,
                max_tokens: self.max_tokens,
            });
        }
        self.current_tokens = new_tokens;
        self.blocks.push(block);
        Ok(())
    }

    /// 返回上下文块的不可变迭代器
    ///
    /// 供 `ContextIsolationGuard::create_safe_summary()` 遍历块内容提取摘要。
    pub fn blocks_iter(&self) -> impl Iterator<Item = &ContextBlock> {
        self.blocks.iter()
    }

    /// 构建提示词 — 调用 HCW select_window() + OSA compute_all_masks() 稀疏化
    ///
    /// ADR-026 决策 7: 不自实现压缩,委托给 hcw_window + osa_coordinator。
    ///
    /// ## 算法(&self,不改存储状态)
    ///
    /// 1. 创建临时 HcwWindow(避免多次调用导致 entry 累积)
    /// 2. 插入 blocks 作为 ContextEntry(file_id = block.name)
    /// 3. 估算复杂度(total_tokens → complexity f32)
    /// 4. `select_window(complexity)` 触发窗口层级选择
    /// 5. `compute_all_masks(&profile)` 计算 OSA 五维度掩码
    /// 6. 增强 `active_file_ids = OSA context.active_ids ∪ Critical 块 name`
    /// 7. `apply_sparse_mask(active_file_ids)` 执行稀疏化
    /// 8. 按 priority 降序拼接保留的 blocks 内容
    ///
    /// ## 返回
    /// - `Ok(String)`: 稀疏化后的提示词
    /// - `Err(MasError::ContextCompressionFailed)`: HCW 或 OSA 失败
    pub async fn build_prompt(&self) -> Result<String> {
        if self.blocks.is_empty() {
            return Ok(String::new());
        }

        // 1. 估算复杂度(基于总 token 数)
        let total_tokens: usize = self.blocks.iter().map(|b| b.tokens).sum();
        let complexity = estimate_complexity(total_tokens);

        // 2. 创建临时 HcwWindow,插入 blocks 作为 ContextEntry
        let temp_window = HcwWindow::with_default_config(self.event_bus.clone()).map_err(|e| {
            MasError::ContextCompressionFailed {
                agent_id: self.agent_id.clone(),
                reason: format!("HCW 创建失败: {e}"),
            }
        })?;

        for (i, block) in self.blocks.iter().enumerate() {
            let entry = ContextEntry::new(
                format!("entry-{i}"),
                &block.name,
                &block.content,
                block.tokens,
            );
            temp_window
                .insert(entry)
                .await
                .map_err(|e| MasError::ContextCompressionFailed {
                    agent_id: self.agent_id.clone(),
                    reason: format!("HCW 插入失败: {e}"),
                })?;
        }

        // 3. select_window 触发窗口层级选择(溢出时自动压缩)
        temp_window.select_window(complexity).await.map_err(|e| {
            MasError::ContextCompressionFailed {
                agent_id: self.agent_id.clone(),
                reason: format!("HCW select_window 失败: {e}"),
            }
        })?;

        // 4. 创建 OSA coordinator,计算稀疏掩码
        let coord = OmniSparseCoordinator::new(self.event_bus.clone());

        // 构造 TaskProfile(available_files = 所有 block name,供 OSA context_mask 选取)
        let available_files: Vec<FileId> = self
            .blocks
            .iter()
            .map(|b| FileId::new(b.name.clone()))
            .collect();
        let profile = TaskProfile {
            task_id: TaskId::new(format!("ctx-{}", self.agent_id)),
            task_type: TaskType::Read,
            complexity_score: complexity,
            risk_level: RiskLevel::Low,
            time_pressure: TimePressure::Low,
            affected_scope: AffectedScope::Local,
            available_tools: Vec::new(),
            available_files,
            available_memories: Vec::new(),
            recent_operations: Vec::new(),
            active_tasks: Vec::new(),
        };

        let masks = coord.compute_all_masks(&profile).await.map_err(|e| {
            MasError::ContextCompressionFailed {
                agent_id: self.agent_id.clone(),
                reason: format!("OSA compute_all_masks 失败: {e}"),
            }
        })?;

        // 5. 增强 active_file_ids = OSA context.active_ids ∪ Critical 块 name
        // WHY Critical 块强制加入:确保永不因 OSA 稀疏化丢失(ADR-026 红线)
        let mut active_names: HashSet<String> = masks
            .context
            .active_ids
            .iter()
            .map(|f| f.to_string())
            .collect();
        for block in &self.blocks {
            if block.priority.is_critical() {
                active_names.insert(block.name.clone());
            }
        }

        // 6. apply_sparse_mask(HCW 实际执行稀疏化,发布 ContextCompressed 事件)
        let active_file_ids: Vec<String> = active_names.iter().cloned().collect();
        temp_window
            .apply_sparse_mask(active_file_ids.clone())
            .await
            .map_err(|e| MasError::ContextCompressionFailed {
                agent_id: self.agent_id.clone(),
                reason: format!("HCW apply_sparse_mask 失败: {e}"),
            })?;

        // 7. 按优先级降序拼接保留的 blocks 内容
        // WHY 用 HashSet 查找:O(1) 替代 Vec::contains O(n),大量块时性能更优
        let active_set: HashSet<&str> = active_file_ids.iter().map(|s| s.as_str()).collect();
        let mut retained_blocks: Vec<&ContextBlock> = self
            .blocks
            .iter()
            .filter(|b| active_set.contains(b.name.as_str()))
            .collect();
        // 按 priority 降序排列(Critical 在前,Optional 在后)
        retained_blocks.sort_by_key(|b| std::cmp::Reverse(b.priority));

        let prompt = retained_blocks
            .iter()
            .map(|b| b.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(prompt)
    }
}

/// 估算复杂度 — 基于总 token 数映射到 [0.0, 1.0] 区间
///
/// 启发式策略(对应 HCW 四级窗口):
/// - total ≥ 131_072 (128K) → 0.9 (L3, UltraComplex)
/// - total ≥ 32_768 (32K) → 0.6 (L2, Complex)
/// - total ≥ 4_096 (4K) → 0.4 (L1, Regular)
/// - else → 0.1 (L0, Simple)
fn estimate_complexity(total_tokens: usize) -> f32 {
    if total_tokens >= 131_072 {
        0.9
    } else if total_tokens >= 32_768 {
        0.6
    } else if total_tokens >= 4_096 {
        0.4
    } else {
        0.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_priority_ordering() {
        assert!(ContextPriority::Critical > ContextPriority::High);
        assert!(ContextPriority::High > ContextPriority::Normal);
        assert!(ContextPriority::Normal > ContextPriority::Low);
        assert!(ContextPriority::Low > ContextPriority::Optional);
    }

    #[test]
    fn test_context_priority_predicates() {
        assert!(ContextPriority::Critical.is_critical());
        assert!(!ContextPriority::High.is_critical());
        assert!(ContextPriority::Optional.is_optional());
        assert!(!ContextPriority::Low.is_optional());
    }

    #[test]
    fn test_context_block_new_critical_not_compressible() {
        let block = ContextBlock::new("system-prompt", "content", 100, ContextPriority::Critical);
        assert!(!block.is_compressible, "Critical 块不可压缩");
    }

    #[test]
    fn test_context_block_new_normal_compressible() {
        let block = ContextBlock::new("block-1", "content", 100, ContextPriority::Normal);
        assert!(block.is_compressible, "Normal 块默认可压缩");
    }

    #[test]
    fn test_estimate_complexity_thresholds() {
        assert!((estimate_complexity(100) - 0.1).abs() < f32::EPSILON);
        assert!((estimate_complexity(4_096) - 0.4).abs() < f32::EPSILON);
        assert!((estimate_complexity(32_768) - 0.6).abs() < f32::EPSILON);
        assert!((estimate_complexity(131_072) - 0.9).abs() < f32::EPSILON);
    }
}
