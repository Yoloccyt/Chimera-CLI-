//! 金字塔存储映射 — TencentDB 四层 → CMT 热/温/冷/冰（设计文档 §8.1）
//!
//! 对应架构层: **L3 Storage**（cmt-tiering 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §8.1
//! 对应论文: TencentDB Agent Memory（四层金字塔）+ Dressage（分层采样经验）
//! 对应 ADR: ADR-049 决策 1（pyramid-storage 落点 cmt-tiering，内嵌模块）
//!
//! # 核心职责
//!
//! 将 L0 [`MemoryPyramidLevel`]（Phase 0 契约）映射到 CMT 热/温/冷/冰四级存储，
//! 并提供分层采样能力：
//!
//! | 金字塔层级 | CMT 层级 | 存储优先级 | 语义 |
//! |-----------|---------|-----------|------|
//! | `L0RawLog` | Ice | Archive | 全量原始对话（审计/追溯） |
//! | `L1AtomicMemory` | Cold | HighValue | 结构化原子卡片（规则/偏好） |
//! | `L2SceneBlock` | Warm | MediumValue | 场景档案（场景检索） |
//! | `L3Persona` | Hot | Critical | 人格摘要（每轮注入） |
//!
//! # 分层采样比例（TencentDB + Dressage 经验）
//!
//! 25% Hot / 25% Warm / 50% Cold / 0% Ice（Cold 高权重，Ice 仅离线）。
//! 与 `rl_replay_pool` 的 `SAMPLE_RATIOS (0.25, 0.25, 0.5)` 语义对齐
//! （Wave 4 一致性测试钉住，防漂移）。
//!
//! # 设计约束
//!
//! - **INV-8 迁移单调性**: 层级迁移经 L0 `assert_archive_monotonicity` 校验
//!   （Hot→Warm→Cold→Ice 单向降级，回升拒绝）
//! - **铁律3**: 只读持久化（存储不修改数据内容）
//! - **复用 CmtCoordinator**: 通过 `Arc<CmtCoordinator>` 调用四层 insert/get，
//!   不直接操作底层 Tier（L3 内部封装）

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nexus_contracts::{assert_archive_monotonicity, MemoryPyramidLevel};
use rand::seq::SliceRandom;
use rand::thread_rng;

use crate::coordinator::CmtCoordinator;
use crate::error::CmtError;
use crate::types::{CapabilityEntry, Tier};

/// 分层采样比例: Hot / Warm / Cold（Ice 不参与在线采样）
///
/// 与 `rl_replay_pool::SAMPLE_RATIOS` 语义对齐（Wave 4 一致性测试）。
pub const PYRAMID_SAMPLE_RATIOS: (f32, f32, f32) = (0.25, 0.25, 0.5);

/// 金字塔存储映射器 — L0 金字塔层级 → CMT 四级存储
///
/// 持有 `Arc<CmtCoordinator>` 复用四层存储；维护存储记录索引
/// （tier → 条目 ID 列表）用于分层采样（避免遍历底层存储）。
#[derive(Clone)]
pub struct PyramidStorageMapper {
    /// CMT 协调器（四层统一接口）
    coordinator: Arc<CmtCoordinator>,
    /// 存储记录索引: tier → 条目 ID 列表（采样用，Mutex 短临界区）
    stored: Arc<Mutex<HashMap<Tier, Vec<String>>>>,
}

impl PyramidStorageMapper {
    /// 创建金字塔存储映射器
    pub fn new(coordinator: Arc<CmtCoordinator>) -> Self {
        Self {
            coordinator,
            stored: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 金字塔层级 → CMT 层级映射（纯函数）
    ///
    /// 映射语义: 越精炼的记忆（高层级）存储越热（访问越频繁）。
    pub fn pyramid_to_tier(level: MemoryPyramidLevel) -> Tier {
        match level {
            // L0 Raw 日志: 体积大、低频访问 → Ice 归档
            MemoryPyramidLevel::L0RawLog => Tier::Ice,
            // L1 原子卡片: 规则/偏好，中低频 → Cold
            MemoryPyramidLevel::L1AtomicMemory => Tier::Cold,
            // L2 场景档案: 场景检索，中频 → Warm
            MemoryPyramidLevel::L2SceneBlock => Tier::Warm,
            // L3 人格摘要: 每轮注入，高频 → Hot
            MemoryPyramidLevel::L3Persona => Tier::Hot,
        }
    }

    /// CMT 层级 → 金字塔层级反向映射（纯函数）
    pub fn tier_to_pyramid(tier: Tier) -> MemoryPyramidLevel {
        match tier {
            Tier::Hot => MemoryPyramidLevel::L3Persona,
            Tier::Warm => MemoryPyramidLevel::L2SceneBlock,
            Tier::Cold => MemoryPyramidLevel::L1AtomicMemory,
            Tier::Ice => MemoryPyramidLevel::L0RawLog,
        }
    }

    /// 存储金字塔层级数据 — 按映射路由到对应 CMT 层级
    ///
    /// - `level`: 金字塔层级（决定目标 CMT 层级）
    /// - `id`: 条目唯一标识
    /// - `content`: 已序列化的数据内容（调用方负责 L0 类型序列化，MessagePack 建议）
    ///
    /// 铁律3: 只读持久化，不修改 content 内容。
    pub async fn store_pyramid_level(
        &self,
        level: MemoryPyramidLevel,
        id: &str,
        content: &str,
    ) -> Result<(), CmtError> {
        let tier = Self::pyramid_to_tier(level);
        let entry = CapabilityEntry::new(id, content, tier);
        self.coordinator.insert(entry).await?;
        // 记录存储索引（采样用，短临界区）
        let mut stored = self.stored.lock().unwrap_or_else(|e| e.into_inner());
        stored.entry(tier).or_default().push(id.to_string());
        Ok(())
    }

    /// 分层采样 — 25% Hot / 25% Warm / 50% Cold / 0% Ice
    ///
    /// 返回采样的条目 ID 列表（按层级比例从存储索引随机抽取）。
    /// Ice 层不参与在线采样（仅离线分析）。
    pub fn sample_pyramid(&self, batch_size: usize) -> Vec<String> {
        if batch_size == 0 {
            return Vec::new();
        }
        let hot_n = batch_size / 4; // 25%
        let warm_n = batch_size / 4; // 25%
        let cold_n = batch_size - hot_n - warm_n; // 50%（剩余，含整除余数）

        let stored = self.stored.lock().unwrap_or_else(|e| e.into_inner());
        let mut rng = thread_rng();
        let mut samples = Vec::with_capacity(batch_size);
        samples.extend(Self::sample_tier(&stored, Tier::Hot, hot_n, &mut rng));
        samples.extend(Self::sample_tier(&stored, Tier::Warm, warm_n, &mut rng));
        samples.extend(Self::sample_tier(&stored, Tier::Cold, cold_n, &mut rng));
        // Ice 层不采样（0%）
        samples
    }

    /// 从指定层级的存储索引随机抽取 n 个条目 ID（不足则全取）
    fn sample_tier(
        stored: &HashMap<Tier, Vec<String>>,
        tier: Tier,
        n: usize,
        rng: &mut impl rand::Rng,
    ) -> Vec<String> {
        let Some(ids) = stored.get(&tier) else {
            return Vec::new();
        };
        if ids.is_empty() || n == 0 {
            return Vec::new();
        }
        let take = n.min(ids.len());
        ids.choose_multiple(rng, take).cloned().collect()
    }

    /// 校验金字塔层级迁移单调性（INV-8）
    ///
    /// 验证 `from → to` 不构成存储温度回升（经 L0 ArchiveTier 判定）。
    /// - `Ok(())`: 合法降级或同层保持
    /// - `Err(CmtError::InvariantViolated)`: 回升方向（如 Ice→Hot），拒绝
    pub fn validate_migration(
        from: MemoryPyramidLevel,
        to: MemoryPyramidLevel,
    ) -> Result<(), CmtError> {
        let from_tier = Self::pyramid_to_tier(from);
        let to_tier = Self::pyramid_to_tier(to);
        let from_archive = from_tier.to_archive_tier();
        let to_archive = to_tier.to_archive_tier();
        assert_archive_monotonicity(from_archive, to_archive).map_err(|v| {
            CmtError::InvariantViolated(format!(
                "金字塔层级迁移违反 INV-8: {:?} -> {:?} ({})",
                from, to, v.msg
            ))
        })
    }

    /// 各层存储条目数快照（可观测性）
    pub fn stored_counts(&self) -> HashMap<Tier, usize> {
        let stored = self.stored.lock().unwrap_or_else(|e| e.into_inner());
        stored.iter().map(|(t, ids)| (*t, ids.len())).collect()
    }
}

// Tier 需要作为 HashMap key（Hash + Eq），已在 types.rs derive。
// to_archive_tier 是 pub(crate)，本模块内可调用。

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyramid_to_tier_mapping_four_levels() {
        assert_eq!(
            PyramidStorageMapper::pyramid_to_tier(MemoryPyramidLevel::L0RawLog),
            Tier::Ice
        );
        assert_eq!(
            PyramidStorageMapper::pyramid_to_tier(MemoryPyramidLevel::L1AtomicMemory),
            Tier::Cold
        );
        assert_eq!(
            PyramidStorageMapper::pyramid_to_tier(MemoryPyramidLevel::L2SceneBlock),
            Tier::Warm
        );
        assert_eq!(
            PyramidStorageMapper::pyramid_to_tier(MemoryPyramidLevel::L3Persona),
            Tier::Hot
        );
    }

    #[test]
    fn tier_to_pyramid_roundtrip() {
        // 双向映射往返一致
        for level in [
            MemoryPyramidLevel::L0RawLog,
            MemoryPyramidLevel::L1AtomicMemory,
            MemoryPyramidLevel::L2SceneBlock,
            MemoryPyramidLevel::L3Persona,
        ] {
            let tier = PyramidStorageMapper::pyramid_to_tier(level);
            let back = PyramidStorageMapper::tier_to_pyramid(tier);
            assert_eq!(back, level, "双向映射应往返一致");
        }
    }

    #[test]
    fn sample_ratios_match_spec() {
        // §8.1: 25% Hot / 25% Warm / 50% Cold
        assert_eq!(PYRAMID_SAMPLE_RATIOS, (0.25, 0.25, 0.5));
        let sum = PYRAMID_SAMPLE_RATIOS.0 + PYRAMID_SAMPLE_RATIOS.1 + PYRAMID_SAMPLE_RATIOS.2;
        assert!((sum - 1.0).abs() < 1e-6, "采样比例总和应为 1.0");
    }

    #[test]
    fn validate_migration_monotonic_demotion_ok() {
        // 合法降级: L3Persona(Hot) → L0RawLog(Ice)
        assert!(PyramidStorageMapper::validate_migration(
            MemoryPyramidLevel::L3Persona,
            MemoryPyramidLevel::L0RawLog
        )
        .is_ok());
        // 同层保持合法
        assert!(PyramidStorageMapper::validate_migration(
            MemoryPyramidLevel::L2SceneBlock,
            MemoryPyramidLevel::L2SceneBlock
        )
        .is_ok());
    }

    #[test]
    fn validate_migration_rejects_promotion() {
        // 回升拒绝: L0RawLog(Ice) → L3Persona(Hot)（INV-8）
        assert!(PyramidStorageMapper::validate_migration(
            MemoryPyramidLevel::L0RawLog,
            MemoryPyramidLevel::L3Persona
        )
        .is_err());
    }

    #[test]
    fn sample_tier_empty_returns_empty() {
        let stored: HashMap<Tier, Vec<String>> = HashMap::new();
        let mut rng = thread_rng();
        let samples = PyramidStorageMapper::sample_tier(&stored, Tier::Hot, 10, &mut rng);
        assert!(samples.is_empty(), "空存储索引应返回空采样");
    }

    #[test]
    fn sample_tier_insufficient_takes_all() {
        let mut stored: HashMap<Tier, Vec<String>> = HashMap::new();
        stored.insert(Tier::Hot, vec!["a".into(), "b".into()]);
        let mut rng = thread_rng();
        // 请求 10 个但只有 2 个 → 全取
        let samples = PyramidStorageMapper::sample_tier(&stored, Tier::Hot, 10, &mut rng);
        assert_eq!(samples.len(), 2);
    }
}
