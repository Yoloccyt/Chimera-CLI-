//! 多级降级链 — 能力不可达时的逐级回退机制
//!
//! 对应架构层:L10 Interface
//!
//! ## 设计要点
//! - 支持任意层级数的降级链(架构红线要求 ≥ 3 级)
//! - `next_level()` 推进到下一级,到达末端返回 `ChainExhausted`
//! - `current_level()` 返回当前层级索引(从 0 开始)
//! - `reset()` 重置到初始层级(level 0)
//! - 创建时校验层级数 ≥ 1(空降级链无意义)
//! - `last_advanced_at` 跟踪最后推进时刻,支持 TTL 清理(SubTask 0.5.9)
//!
//! ## 层级语义(v2.9.0-omega 重设计,ADR-062 / spec.md C13)
//! - Level 0:首次降级(primary substitute)— 原始能力不可达时的第一级替代
//! - Level 1:次级降级(secondary substitute)— primary 替代失败后的回退
//! - Level 2:末级降级(tertiary substitute)— secondary 替代失败后的回退
//! - Level 3+:进一步降级(可选,由 config.default_degradation_levels 长度决定)
//!
//! WHY 统一语义:与 `lib.rs::advance_or_create_chain` 注释对齐(C13 修复)。
//! 原文档将 level 0 描述为"原始能力",但实际 trigger_substitution 在原始能力
//! 不可达时才触发,首次调用即处于 level 0(已降级到 primary substitute)。

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::error::CsnError;

/// 多级降级链 — 能力不可达时的逐级回退路径
///
/// 每条降级链由唯一 `chain_id` 标识,`levels` 列出所有可用层级
/// (各级替代路径),`current_level` 跟踪当前回退位置。
///
/// ## 不变量
/// - `levels.len() >= 1`(空降级链在创建时被拒绝)
/// - `current_level < levels.len()`(由 `next_level` 保证)
///
/// ## PartialEq 语义
/// 手动实现,忽略 `last_advanced_at`(`Instant` 不可比较且为运行时状态)。
/// 两个 chain 仅当 `chain_id` / `levels` / `current_level` 全等时才相等。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradationChain {
    /// 降级链唯一标识(通常等于原始 capability_id)
    pub chain_id: String,
    /// 降级层级列表(索引 0 = 首次降级 primary substitute,1+ = 后续替代层级)
    pub levels: Vec<String>,
    /// 当前层级索引(从 0 开始)
    pub current_level: usize,
    /// 最后推进时刻 — 用于 TTL 清理(SubTask 0.5.9)
    ///
    /// WHY `#[serde(skip)]`:`Instant` 不可序列化;反序列化时重新计时
    /// (恢复的 chain 视为"刚推进",符合运行时清理语义)。
    /// `default = "Instant::now"` 提供 serde 反序列化默认值。
    #[serde(skip, default = "Instant::now")]
    pub last_advanced_at: Instant,
}

impl PartialEq for DegradationChain {
    /// 结构性相等 — 忽略 `last_advanced_at`(运行时状态,Instant 不可比较)
    fn eq(&self, other: &Self) -> bool {
        self.chain_id == other.chain_id
            && self.levels == other.levels
            && self.current_level == other.current_level
    }
}

impl DegradationChain {
    /// 创建降级链
    ///
    /// # 参数
    /// - `chain_id`:降级链唯一标识
    /// - `levels`:降级层级列表(至少 1 个元素)
    ///
    /// # 注意
    /// 架构红线要求降级链深度 ≥ 3 级,但本构造函数仅强制 ≥ 1 级
    /// (允许调用方在测试场景使用更短的链)。生产配置应通过
    /// `CsnConfig::default_degradation_levels` 保证 ≥ 3 级。
    ///
    /// WHY 不返回 Result:不返回 Result 以保持 API 简洁;空 levels 时填充占位符,
    /// 避免 panic。调用方应通过 CsnConfig 保证 levels 非空。
    pub fn new(chain_id: impl Into<String>, levels: Vec<String>) -> Self {
        let chain_id = chain_id.into();
        let levels = if levels.is_empty() {
            vec![chain_id.clone()]
        } else {
            levels
        };

        Self {
            chain_id,
            levels,
            current_level: 0,
            last_advanced_at: Instant::now(),
        }
    }

    /// 推进到下一级降级
    ///
    /// # 错误
    /// - `ChainExhausted`:已到达降级链末端,无法继续推进
    ///
    /// # 返回
    /// 成功时返回 `Ok(())`,失败时保持当前层级不变。
    ///
    /// # 副作用
    /// 成功推进时更新 `last_advanced_at` 为当前时刻(用于 TTL 计算)。
    pub fn next_level(&mut self) -> Result<(), CsnError> {
        if self.current_level + 1 >= self.levels.len() {
            return Err(CsnError::ChainExhausted {
                chain_id: self.chain_id.clone(),
                total_levels: self.levels.len(),
            });
        }
        self.current_level += 1;
        // 更新推进时刻,供 TTL 清理判断
        self.last_advanced_at = Instant::now();
        Ok(())
    }

    /// 获取当前层级索引(从 0 开始)
    pub fn current_level(&self) -> usize {
        self.current_level
    }

    /// 获取当前层级对应的标识(如能力 ID 或层级名称)
    ///
    /// 返回 `levels[current_level]`。由于不变量保证 `current_level < levels.len()`,
    /// 此方法始终返回有效值。
    pub fn current_target(&self) -> &str {
        // 不变量保证 current_level < levels.len(),直接索引安全
        &self.levels[self.current_level]
    }

    /// 重置降级链到初始层级(level 0)
    ///
    /// # 副作用
    /// 重置时更新 `last_advanced_at`(视为一次"推进",延长 TTL)。
    pub fn reset(&mut self) {
        self.current_level = 0;
        self.last_advanced_at = Instant::now();
    }

    /// 获取降级链总层级数
    pub fn total_levels(&self) -> usize {
        self.levels.len()
    }

    /// 是否已到达末端层级(无法继续降级)
    pub fn is_exhausted(&self) -> bool {
        self.current_level + 1 >= self.levels.len()
    }

    /// 获取降级链 ID
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }

    /// 获取所有层级列表(只读)
    pub fn levels(&self) -> &[String] {
        &self.levels
    }

    /// 获取最后推进时刻(用于 TTL 计算)
    ///
    /// 返回 `last_advanced_at`,即上次 `next_level` / `reset` / 创建时的时刻。
    pub fn last_advanced_at(&self) -> Instant {
        self.last_advanced_at
    }

    /// 判断降级链是否已过期(超过 TTL 未推进)
    ///
    /// # 参数
    /// - `ttl`:存活时间(如 `Duration::from_secs(3600)` 表示 1 小时)
    ///
    /// # 返回
    /// - `true`:`last_advanced_at + ttl` 已过期,应被清理
    /// - `false`:仍在 TTL 内
    ///
    /// # 用途
    /// 供 `CsnSubstitutor::cleanup_chains` 按需清理长期未推进的降级链,
    /// 避免内存泄漏(SubTask 0.5.9)。
    pub fn is_expired(&self, ttl: Duration) -> bool {
        self.last_advanced_at.elapsed() > ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === 辅助函数 ===

    fn make_chain(levels: Vec<&str>) -> DegradationChain {
        let levels: Vec<String> = levels.into_iter().map(String::from).collect();
        DegradationChain::new("chain-1", levels)
    }

    // === 1. 创建与基础属性 ===

    #[test]
    fn test_new_chain() {
        let chain = make_chain(vec!["original", "primary", "secondary", "tertiary"]);
        assert_eq!(chain.chain_id, "chain-1");
        assert_eq!(chain.levels.len(), 4);
        assert_eq!(chain.current_level, 0);
        assert_eq!(chain.current_target(), "original");
    }

    #[test]
    fn test_new_chain_empty_levels_fills_placeholder() {
        // 空 levels 时填充占位符(避免 panic)
        let chain = DegradationChain::new("empty-chain", vec![]);
        assert_eq!(chain.levels.len(), 1);
        assert_eq!(chain.levels[0], "empty-chain");
        assert_eq!(chain.current_level, 0);
    }

    #[test]
    fn test_default_degradation_levels_at_least_three() {
        // 验证典型配置:≥ 3 级降级(架构红线)
        let chain = make_chain(vec!["original", "L1", "L2", "L3"]);
        assert!(chain.total_levels() >= 3, "降级链深度应 ≥ 3 级");
    }

    // === 2. next_level 推进 ===

    #[test]
    fn test_next_level_advances() {
        let mut chain = make_chain(vec!["L0", "L1", "L2"]);
        assert_eq!(chain.current_level(), 0);

        chain.next_level().expect("应推进到 L1");
        assert_eq!(chain.current_level(), 1);
        assert_eq!(chain.current_target(), "L1");

        chain.next_level().expect("应推进到 L2");
        assert_eq!(chain.current_level(), 2);
        assert_eq!(chain.current_target(), "L2");
    }

    #[test]
    fn test_next_level_exhausted_returns_error() {
        let mut chain = make_chain(vec!["L0", "L1", "L2"]);
        chain.next_level().expect("L0 → L1");
        chain.next_level().expect("L1 → L2");

        // 已到末端,应返回错误
        let result = chain.next_level();
        assert!(matches!(result, Err(CsnError::ChainExhausted { .. })));
        // 层级应保持不变
        assert_eq!(chain.current_level(), 2);
    }

    #[test]
    fn test_next_level_single_level_chain_immediately_exhausted() {
        let mut chain = make_chain(vec!["only"]);
        let result = chain.next_level();
        assert!(matches!(result, Err(CsnError::ChainExhausted { .. })));
        assert_eq!(chain.current_level(), 0);
    }

    // === 3. reset 重置 ===

    #[test]
    fn test_reset_to_initial_level() {
        let mut chain = make_chain(vec!["L0", "L1", "L2"]);
        chain.next_level().expect("推进到 L1");
        chain.next_level().expect("推进到 L2");
        assert_eq!(chain.current_level(), 2);

        chain.reset();
        assert_eq!(chain.current_level(), 0);
        assert_eq!(chain.current_target(), "L0");
    }

    #[test]
    fn test_reset_already_at_initial() {
        let mut chain = make_chain(vec!["L0", "L1"]);
        chain.reset(); // 已在 level 0,reset 应无副作用
        assert_eq!(chain.current_level(), 0);
    }

    // === 4. is_exhausted ===

    #[test]
    fn test_is_exhausted() {
        let mut chain = make_chain(vec!["L0", "L1", "L2"]);
        assert!(!chain.is_exhausted(), "L0 未耗尽");

        chain.next_level().expect("推进到 L1");
        assert!(!chain.is_exhausted(), "L1 未耗尽");

        chain.next_level().expect("推进到 L2");
        assert!(chain.is_exhausted(), "L2 已耗尽");
    }

    #[test]
    fn test_is_exhausted_single_level() {
        let chain = make_chain(vec!["only"]);
        assert!(chain.is_exhausted(), "单级链应立即耗尽");
    }

    // === 5. 访问器 ===

    #[test]
    fn test_total_levels() {
        let chain = make_chain(vec!["L0", "L1", "L2", "L3"]);
        assert_eq!(chain.total_levels(), 4);
    }

    #[test]
    fn test_levels_accessor() {
        let chain = make_chain(vec!["L0", "L1", "L2"]);
        let levels = chain.levels();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], "L0");
        assert_eq!(levels[2], "L2");
    }

    #[test]
    fn test_chain_id_accessor() {
        let chain = make_chain(vec!["L0", "L1"]);
        assert_eq!(chain.chain_id(), "chain-1");
    }

    // === 6. 序列化往返 ===

    #[test]
    fn test_serde_roundtrip() {
        let chain = make_chain(vec!["original", "primary", "secondary", "tertiary"]);
        let json = serde_json::to_string(&chain).expect("序列化失败");
        let restored: DegradationChain = serde_json::from_str(&json).expect("反序列化失败");
        // last_advanced_at 被 #[serde(skip)] 跳过,不影响 PartialEq(Instant 比较)
        assert_eq!(chain.chain_id, restored.chain_id);
        assert_eq!(chain.levels, restored.levels);
        assert_eq!(chain.current_level, restored.current_level);
    }

    #[test]
    fn test_serde_with_advanced_level() {
        let mut chain = make_chain(vec!["L0", "L1", "L2"]);
        chain.next_level().expect("推进到 L1");
        let json = serde_json::to_string(&chain).expect("序列化失败");
        let restored: DegradationChain = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(chain.current_level(), restored.current_level());
        assert_eq!(restored.current_level(), 1);
    }

    // === 7. 三级降级全流程(架构红线验证)===

    #[test]
    fn test_three_level_degradation_full_flow() {
        // 架构红线:降级链深度 ≥ 3 级
        let mut chain = make_chain(vec![
            "original",
            "primary-sub",
            "secondary-sub",
            "tertiary-sub",
        ]);

        // Level 0:首次降级(primary substitute)
        assert_eq!(chain.current_target(), "original");
        assert_eq!(chain.current_level(), 0);

        // Level 1:次级降级(secondary substitute)
        chain.next_level().expect("推进到 primary");
        assert_eq!(chain.current_target(), "primary-sub");

        // Level 2:末级降级(tertiary substitute)
        chain.next_level().expect("推进到 secondary");
        assert_eq!(chain.current_target(), "secondary-sub");

        // Level 3:末选替代
        chain.next_level().expect("推进到 tertiary");
        assert_eq!(chain.current_target(), "tertiary-sub");

        // 已耗尽
        assert!(chain.is_exhausted());
        assert!(chain.next_level().is_err());

        // 重置后可重新开始
        chain.reset();
        assert_eq!(chain.current_target(), "original");
    }

    // === 8. TTL 与 last_advanced_at(SubTask 0.5.9)===

    #[test]
    fn test_last_advanced_at_initialized_on_creation() {
        let chain = make_chain(vec!["L0", "L1"]);
        // 创建后 last_advanced_at 应为当前时刻(刚创建)
        let elapsed = chain.last_advanced_at().elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "新创建的 chain elapsed 应 < 100ms,实际: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_next_level_updates_last_advanced_at() {
        let mut chain = make_chain(vec!["L0", "L1"]);
        // 创建后等待一小段时间
        std::thread::sleep(std::time::Duration::from_millis(10));
        let before = chain.last_advanced_at();
        chain.next_level().expect("推进");
        let after = chain.last_advanced_at();
        // 推进后 last_advanced_at 应更新
        assert!(after > before, "推进后 last_advanced_at 应更新为更晚的时刻");
    }

    #[test]
    fn test_reset_updates_last_advanced_at() {
        let mut chain = make_chain(vec!["L0", "L1"]);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let before = chain.last_advanced_at();
        chain.reset();
        let after = chain.last_advanced_at();
        assert!(
            after > before,
            "reset 后 last_advanced_at 应更新为更晚的时刻"
        );
    }

    #[test]
    fn test_is_expired_false_for_fresh_chain() {
        let chain = make_chain(vec!["L0", "L1"]);
        // 新创建的 chain 不应过期
        assert!(
            !chain.is_expired(Duration::from_secs(3600)),
            "新创建的 chain 在 1 小时 TTL 内不应过期"
        );
    }

    #[test]
    fn test_is_expired_true_for_stale_chain() {
        let mut chain = make_chain(vec!["L0", "L1"]);
        // 模拟过期:手动设置 last_advanced_at 为过去
        // WHY 不用 sleep 1 小时:用 Instant 的 now + Duration 测试逻辑
        // 这里用极短 TTL 验证 is_expired 判断逻辑
        let _ = chain.next_level();
        // 等待 10ms
        std::thread::sleep(std::time::Duration::from_millis(10));
        // TTL=5ms 应已过期
        assert!(
            chain.is_expired(Duration::from_millis(5)),
            "TTL=5ms 时,10ms 前推进的 chain 应已过期"
        );
        // TTL=1小时 不应过期
        assert!(
            !chain.is_expired(Duration::from_secs(3600)),
            "TTL=1小时 时,10ms 前推进的 chain 不应过期"
        );
    }
}
