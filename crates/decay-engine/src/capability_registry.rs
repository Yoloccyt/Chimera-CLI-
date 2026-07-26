//! 能力场令牌注册表 — 学习策略灰度授权管理（P4-W14.5 C4 合规）
//!
//! 对应架构层: **L4 Security**（decay-engine 内部组件）
//! 对应 ADR: **ADR-037**（能力场灰度 C4 合规，提议中）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §C4 + §7.4
//! 对应任务: **P4-W14.5.1**（CapabilityTokenRegistry 类型设计 + holder 集成）
//!
//! # 核心职责
//!
//! 承载六接缝（S1/S2/S3/S4/S5/S6）的 `CapabilityToken` 注册表，为编排器
//! （chimera-cli / quest-engine）提供统一的 token 查询与操作入口。
//!
//! # C4 合规接入路径
//!
//! ```text
//! omega-learner (L6) ──(Learned 策略)──▶ 编排器 (L9)
//!                                          │
//!                                          ▼ 查询 token
//! L4 decay-engine ◀──(嵌入)── CapabilityTokenRegistry
//!                                          │
//!                                          ▼ 返回 allows_learned
//! 编排器根据返回值决定:
//!   - true  → holder.update_policy(Learned)（注入学习策略）
//!   - false → holder.fallback_to_static()  （本地 fallback，C4 合规第三层）
//! ```
//!
//! # 设计决策（WHY 嵌入 DecayEngine 而非独立组件）
//!
//! - **语义一致**: DecayEngine 是"能力衰减引擎"，管理 CapabilityToken 与其语义一致
//! - **集中管理**: 所有能力场操作（衰减 + token）集中在 DecayEngine，避免分散
//! - **零运行时开销**: 仅在策略注入路径查询 token，热路径（decay/decay_with_policy）不查询
//!
//! # 线程安全
//!
//! 内部用 `DashMap<SeamId, CapabilityToken>` 存储：
//! - **分片锁并发**: 多接缝 token 操作互不阻塞
//! - **读多写少**: 查询（`token_authorized_level`）高频，写入（`record_token_outcome`）低频
//! - **Send + Sync**: 可跨 async 任务共享（与 DecayEngine 一致）
//!
//! # 示例
//!
//! ## 基础灰度授权流程
//!
//! ```
//! use decay_engine::capability_registry::CapabilityTokenRegistry;
//! use nexus_contracts::SeamId;
//!
//! let registry = CapabilityTokenRegistry::new();
//!
//! // 1. 注册六接缝的初始 token
//! for seam in SeamId::all() {
//!     registry.register_capability_token(seam).unwrap();
//! }
//!
//! // 2. 查询激活状态（初始全部未激活）
//! assert!(!registry.should_activate_learned(SeamId::S6Decay, 0));
//!
//! // 3. S6 接缝记录多次成功 outcome + 提升
//! for _ in 0..20 {
//!     registry.record_token_outcome(SeamId::S6Decay, true).unwrap();
//!     registry.maybe_promote_token(SeamId::S6Decay).unwrap();
//! }
//!
//! // 4. S6 接缝激活，其他接缝仍为初始状态
//! assert!(registry.should_activate_learned(SeamId::S6Decay, 0));
//! assert!(!registry.should_activate_learned(SeamId::S1Density, 0));
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use nexus_contracts::{CapabilityToken, CapabilityTokenStatus, SeamId};
use tracing::{debug, info, warn};

use crate::error::DecayError;

// ============================================================
// 时间工具函数
// ============================================================

/// 获取当前 UTC 时间戳（秒）
///
/// WHY 提供: capability_registry 不依赖 chrono（保持 L4 依赖最小化），
/// 使用 SystemTime 获取 UTC 秒时间戳，与 L0 `temporal::TemporalMeta` 一致。
///
/// # 返回
/// - `Ok(secs)`: 当前 UTC 秒时间戳
/// - `Err`: 系统时间早于 UNIX_EPOCH（极端情况，不应发生）
///
/// # 错误处理
/// 返回 `DecayError::ConfigError` 而非 panic，遵循 §4.1 "避免 unwrap/expect"
pub fn current_utc_secs() -> Result<i64, DecayError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|e| DecayError::ConfigError(format!("系统时间异常: {e}")))
}

// ============================================================
// CapabilityTokenRegistry
// ============================================================

/// 能力场令牌注册表 — 六接缝灰度授权管理中心
///
/// 承载 `omega-learner` 学习策略的灰度授权状态，为编排器提供统一的
/// token 查询与操作入口。嵌入 `DecayEngine`，与能力衰减共享生命周期。
///
/// # 设计决策（WHY DashMap 而非 HashMap+RwLock）
///
/// - **读多写少**: 查询（`should_activate_learned`）高频，写入（`record_token_outcome`）低频
/// - **分片锁并发**: 不同接缝的 token 操作互不阻塞
/// - **与 DecayEngine 一致**: DecayEngine 的 `capabilities` 也用 DashMap，保持一致
///
/// # 线程安全
///
/// `DashMap` 提供 `Send + Sync`，可跨 async 任务共享。
/// 所有方法为 `&self`（非 `&mut self`），允许多线程并发调用。
///
/// # 示例
///
/// ```
/// use decay_engine::capability_registry::CapabilityTokenRegistry;
/// use nexus_contracts::SeamId;
///
/// let registry = CapabilityTokenRegistry::new();
/// registry.register_capability_token(SeamId::S6Decay).unwrap();
///
/// // 初始状态：Provisional，不允许 Learned
/// assert!(!registry.should_activate_learned(SeamId::S6Decay, 0));
/// ```
#[derive(Debug)]
pub struct CapabilityTokenRegistry {
    /// 按接缝索引的 token 存储
    ///
    /// WHY 用 SeamId 作为键（而非 token_id 字符串）:
    /// - 每个接缝只有一个 token，SeamId 是天然唯一键
    /// - 避免 token_id 字符串重复导致的歧义
    /// - SeamId 是 Copy 类型，查询零开销
    tokens: DashMap<SeamId, CapabilityToken>,
}

impl CapabilityTokenRegistry {
    /// 创建空的 token 注册表
    ///
    /// WHY 不预注册六接缝: 调用方按需注册，避免未使用接缝的 token 占用内存
    pub fn new() -> Self {
        Self {
            tokens: DashMap::new(),
        }
    }

    /// 创建并预注册所有六接缝的 token
    ///
    /// WHY 提供: 便于编排器初始化时一键注册所有接缝
    pub fn with_all_seams() -> Self {
        let registry = Self::new();
        for seam in SeamId::all() {
            // WHY unwrap 安全: register_capability_token 仅在已存在时返回错误，
            // 预注册时注册表为空，不会冲突
            registry
                .register_capability_token(seam)
                .expect("预注册时注册表为空，不会冲突");
        }
        registry
    }

    /// 注册新接缝的 token（初始低能力 + Provisional 状态）
    ///
    /// # 参数
    /// - `seam`: 接缝标识
    ///
    /// # 错误
    /// - [`DecayError::ConfigError`]: 该接缝已注册 token
    ///
    /// # 示例
    ///
    /// ```
    /// use decay_engine::capability_registry::CapabilityTokenRegistry;
    /// use nexus_contracts::SeamId;
    ///
    /// let registry = CapabilityTokenRegistry::new();
    /// registry.register_capability_token(SeamId::S1Density).unwrap();
    /// // 重复注册返回错误
    /// assert!(registry.register_capability_token(SeamId::S1Density).is_err());
    /// ```
    pub fn register_capability_token(&self, seam: SeamId) -> Result<(), DecayError> {
        if self.tokens.contains_key(&seam) {
            return Err(DecayError::ConfigError(format!(
                "接缝 {} 已注册 token",
                seam.short_name()
            )));
        }

        let token_id = format!("{}-v1", seam.short_name());
        let token = CapabilityToken::new(token_id, seam);
        self.tokens.insert(seam, token);
        debug!(seam = %seam, "能力令牌已注册（初始低能力）");
        Ok(())
    }

    /// 查询指定接缝的授权等级
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    pub fn token_authorized_level(&self, seam: SeamId) -> Result<f32, DecayError> {
        self.tokens
            .get(&seam)
            .map(|t| t.authorized_level())
            .ok_or(DecayError::TokenNotFound(seam))
    }

    /// 查询指定接缝的状态
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    pub fn token_status(&self, seam: SeamId) -> Result<CapabilityTokenStatus, DecayError> {
        self.tokens
            .get(&seam)
            .map(|t| t.status())
            .ok_or(DecayError::TokenNotFound(seam))
    }

    /// 查询是否允许 Learned 策略（C4 合规核心查询）
    ///
    /// 编排器在调用 `holder.update_policy(Learned)` 前必须查询此方法。
    ///
    /// # 参数
    /// - `seam`: 接缝标识
    /// - `now`: 当前 UTC 秒时间戳
    ///
    /// # 返回
    /// - `Ok(true)`: 允许 Learned 策略
    /// - `Ok(false)`: 不允许（Provisional/Cooldown/Frozen 或 level 不足）
    /// - `Err(TokenNotFound)`: 接缝未注册 token
    ///
    /// # 示例
    ///
    /// ```
    /// use decay_engine::capability_registry::CapabilityTokenRegistry;
    /// use nexus_contracts::SeamId;
    ///
    /// let registry = CapabilityTokenRegistry::new();
    /// registry.register_capability_token(SeamId::S6Decay).unwrap();
    ///
    /// // 初始状态：不允许 Learned
    /// assert!(!registry.should_activate_learned(SeamId::S6Decay, 0).unwrap());
    /// ```
    pub fn should_activate_learned(&self, seam: SeamId, now: i64) -> Result<bool, DecayError> {
        self.tokens
            .get(&seam)
            .map(|t| t.allows_learned_policy(now))
            .ok_or(DecayError::TokenNotFound(seam))
    }

    /// 记录 token 执行结果（EWMA 更新）
    ///
    /// # 参数
    /// - `seam`: 接缝标识
    /// - `success`: true 表示成功，false 表示失败
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    /// - [`DecayError::TokenFrozen`]: token 已冻结，不应再记录 outcome
    pub fn record_token_outcome(&self, seam: SeamId, success: bool) -> Result<(), DecayError> {
        let mut token = self
            .tokens
            .get_mut(&seam)
            .ok_or(DecayError::TokenNotFound(seam))?;

        // WHY 拒绝在 Frozen 状态记录: 冻结的 token 不应再影响 EWMA
        // 避免冻结期间累积的 outcome 影响解冻后的策略评估
        if token.status() == CapabilityTokenStatus::Frozen {
            return Err(DecayError::TokenFrozen(seam));
        }

        token.record_outcome(success);
        debug!(
            seam = %seam,
            success,
            ewma = token.success_ewma(),
            samples = token.sample_count(),
            "令牌 outcome 已记录"
        );
        Ok(())
    }

    /// 尝试渐进授权提升
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    /// - [`DecayError::TokenFrozen`]: token 已冻结
    /// - [`DecayError::CooldownActive`]: token 处于冷却期
    ///
    /// # 返回
    /// - `Ok(true)`: level 实际提升
    /// - `Ok(false)`: level 未提升（EWMA 不足或已达上限）
    pub fn maybe_promote_token(&self, seam: SeamId) -> Result<bool, DecayError> {
        let mut token = self
            .tokens
            .get_mut(&seam)
            .ok_or(DecayError::TokenNotFound(seam))?;

        // WHY 拒绝在 Frozen/Cooldown 状态提升: 这两个状态需要先恢复
        match token.status() {
            CapabilityTokenStatus::Frozen => return Err(DecayError::TokenFrozen(seam)),
            CapabilityTokenStatus::Cooldown => return Err(DecayError::CooldownActive(seam)),
            CapabilityTokenStatus::Provisional | CapabilityTokenStatus::Authorized => {}
        }

        let promoted = token.maybe_promote();
        if promoted {
            info!(
                seam = %seam,
                new_level = token.authorized_level(),
                status = %token.status(),
                "令牌已提升授权等级"
            );
        }
        Ok(promoted)
    }

    /// 触发 AsaIntervention 安全闭环
    ///
    /// 编排器在收到 `AsaIntervention` 事件时调用此方法。
    ///
    /// # 参数
    /// - `seam`: 接缝标识
    ///
    /// # 返回
    /// - `Ok(true)`: 触发了自动冻结（连续 ASA 达阈值）
    /// - `Ok(false)`: 仅进入冷却期
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    ///
    /// # 设计决策（WHY 不需要传入 now）
    ///
    /// 内部调用 `current_utc_secs()` 获取当前时间，简化编排器调用。
    /// 编排器在收到 AsaIntervention 事件时无需关心时间戳。
    pub fn trigger_asa_intervention(&self, seam: SeamId) -> Result<bool, DecayError> {
        let now = current_utc_secs()?;
        let mut token = self
            .tokens
            .get_mut(&seam)
            .ok_or(DecayError::TokenNotFound(seam))?;

        let frozen = token.trigger_asa_intervention(now);
        if frozen {
            warn!(
                seam = %seam,
                consecutive_asa = token.consecutive_asa_count(),
                "令牌连续 ASA 触发达阈值，已自动冻结"
            );
        } else {
            warn!(
                seam = %seam,
                consecutive_asa = token.consecutive_asa_count(),
                level = token.authorized_level(),
                "令牌触发 ASA 干预，进入冷却期"
            );
        }
        Ok(frozen)
    }

    /// 衰减指定接缝的 token（手动衰减）
    ///
    /// # 参数
    /// - `seam`: 接缝标识
    /// - `amount`: 衰减量（正数）
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    pub fn decay_capability_token(&self, seam: SeamId, amount: f32) -> Result<(), DecayError> {
        let mut token = self
            .tokens
            .get_mut(&seam)
            .ok_or(DecayError::TokenNotFound(seam))?;

        token.decay(amount);
        debug!(seam = %seam, amount, new_level = token.authorized_level(), "令牌已衰减");
        Ok(())
    }

    /// 冻结指定接缝的 token
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    pub fn freeze_token(&self, seam: SeamId) -> Result<(), DecayError> {
        let mut token = self
            .tokens
            .get_mut(&seam)
            .ok_or(DecayError::TokenNotFound(seam))?;

        token.freeze();
        warn!(seam = %seam, "令牌已手动冻结");
        Ok(())
    }

    /// 解冻指定接缝的 token（重置为初始状态）
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    pub fn unfreeze_token(&self, seam: SeamId) -> Result<(), DecayError> {
        let mut token = self
            .tokens
            .get_mut(&seam)
            .ok_or(DecayError::TokenNotFound(seam))?;

        token.unfreeze();
        info!(seam = %seam, "令牌已手动解冻，回到初始状态");
        Ok(())
    }

    /// 冻结所有已激活的 token（批量熔断）
    ///
    /// WHY 提供: 编排器在严重故障（如 quest 失败）时一键回退所有 Learned 策略。
    /// 仅冻结 `Authorized` / `Cooldown` 状态的 token，`Frozen` 状态保持不变（幂等）。
    pub fn freeze_all_learned_tokens(&self) {
        let mut frozen_count = 0u32;
        for mut entry in self.tokens.iter_mut() {
            let token = entry.value_mut();
            if token.status() != CapabilityTokenStatus::Frozen {
                token.freeze();
                frozen_count += 1;
            }
        }
        if frozen_count > 0 {
            warn!(frozen_count, "批量冻结所有已激活令牌（紧急熔断）");
        }
    }

    /// 检查并恢复所有冷却期结束的 token
    ///
    /// WHY 提供: 编排器定期调用（如每秒），将冷却期结束的 token 恢复到正常状态。
    /// 避免冷却期结束后 token 仍停留在 Cooldown 状态。
    pub fn maybe_recover_all_from_cooldown(&self) -> Result<i32, DecayError> {
        let now = current_utc_secs()?;
        let mut recovered_count = 0i32;
        for mut entry in self.tokens.iter_mut() {
            let token = entry.value_mut();
            if token.maybe_recover_from_cooldown(now) {
                recovered_count += 1;
                debug!(seam = %token.seam(), "令牌冷却期结束，已恢复");
            }
        }
        if recovered_count > 0 {
            info!(recovered_count, "批量恢复冷却期结束的令牌");
        }
        Ok(recovered_count)
    }

    /// 列出所有 token 的状态摘要（用于诊断/审计）
    ///
    /// 返回 `(seam, authorized_level, status, sample_count, consecutive_asa_count)` 元组列表
    pub fn list_tokens(&self) -> Vec<(SeamId, f32, CapabilityTokenStatus, u64, u32)> {
        self.tokens
            .iter()
            .map(|entry| {
                let token = entry.value();
                (
                    token.seam(),
                    token.authorized_level(),
                    token.status(),
                    token.sample_count(),
                    token.consecutive_asa_count(),
                )
            })
            .collect()
    }

    /// 查询指定接缝的 token 引用（用于深度诊断）
    ///
    /// # 错误
    /// - [`DecayError::TokenNotFound`]: 接缝未注册 token
    ///
    /// # 返回
    /// - `Ok(dashmap::mapref::one::Ref<SeamId, CapabilityToken>)`: token 引用
    ///
    /// # 设计决策（WHY 不返回 Clone）
    ///
    /// 返回 DashMap 的 Ref 而非 Clone，避免大 struct 复制开销。
    /// 调用方持有 Ref 期间，对应分片被读锁定，需要尽快释放。
    pub fn get_token(
        &self,
        seam: SeamId,
    ) -> Result<dashmap::mapref::one::Ref<'_, SeamId, CapabilityToken>, DecayError> {
        self.tokens
            .get(&seam)
            .ok_or(DecayError::TokenNotFound(seam))
    }
}

impl Default for CapabilityTokenRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::SeamId;

    // ============================================================
    // new / with_all_seams 测试
    // ============================================================

    #[test]
    fn test_new_empty() {
        let registry = CapabilityTokenRegistry::new();
        assert!(registry.list_tokens().is_empty());
    }

    #[test]
    fn test_default_equals_new() {
        let r1 = CapabilityTokenRegistry::new();
        let r2 = CapabilityTokenRegistry::default();
        assert_eq!(r1.list_tokens().len(), r2.list_tokens().len());
    }

    #[test]
    fn test_with_all_seams_registers_six() {
        let registry = CapabilityTokenRegistry::with_all_seams();
        let tokens = registry.list_tokens();
        assert_eq!(tokens.len(), 6);
        for seam in SeamId::all() {
            assert!(
                tokens.iter().any(|(s, _, _, _, _)| *s == seam),
                "接缝 {} 未注册",
                seam.short_name()
            );
        }
    }

    // ============================================================
    // register_capability_token 测试
    // ============================================================

    #[test]
    fn test_register_capability_token_success() {
        let registry = CapabilityTokenRegistry::new();
        registry.register_capability_token(SeamId::S6Decay).unwrap();

        let token = registry.get_token(SeamId::S6Decay).unwrap();
        assert_eq!(token.seam(), SeamId::S6Decay);
        assert!((token.authorized_level() - 0.2).abs() < 1e-6);
        assert_eq!(token.status(), CapabilityTokenStatus::Provisional);
    }

    #[test]
    fn test_register_capability_token_duplicate_fails() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S1Density)
            .unwrap();

        let result = registry.register_capability_token(SeamId::S1Density);
        assert!(matches!(result, Err(DecayError::ConfigError(_))));
    }

    // ============================================================
    // token_authorized_level / token_status 测试
    // ============================================================

    #[test]
    fn test_token_authorized_level_initial() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S2Memory)
            .unwrap();

        let level = registry.token_authorized_level(SeamId::S2Memory).unwrap();
        assert!((level - CapabilityToken::INITIAL_LEVEL).abs() < 1e-6);
    }

    #[test]
    fn test_token_status_initial() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S4Selector)
            .unwrap();

        let status = registry.token_status(SeamId::S4Selector).unwrap();
        assert_eq!(status, CapabilityTokenStatus::Provisional);
    }

    #[test]
    fn test_token_not_found_errors() {
        let registry = CapabilityTokenRegistry::new();

        let r1 = registry.token_authorized_level(SeamId::S1Density);
        assert!(matches!(r1, Err(DecayError::TokenNotFound(_))));

        let r2 = registry.token_status(SeamId::S6Decay);
        assert!(matches!(r2, Err(DecayError::TokenNotFound(_))));
    }

    // ============================================================
    // should_activate_learned 测试
    // ============================================================

    #[test]
    fn test_should_activate_learned_initial_false() {
        let registry = CapabilityTokenRegistry::with_all_seams();
        for seam in SeamId::all() {
            assert!(
                !registry.should_activate_learned(seam, 0).unwrap(),
                "接缝 {} 初始不应激活",
                seam.short_name()
            );
        }
    }

    #[test]
    fn test_should_activate_learned_after_promotion() {
        let registry = CapabilityTokenRegistry::new();
        registry.register_capability_token(SeamId::S6Decay).unwrap();

        // 多次成功 outcome + 提升
        for _ in 0..20 {
            registry
                .record_token_outcome(SeamId::S6Decay, true)
                .unwrap();
            registry.maybe_promote_token(SeamId::S6Decay).unwrap();
        }

        assert!(registry
            .should_activate_learned(SeamId::S6Decay, 0)
            .unwrap());
    }

    // ============================================================
    // record_token_outcome 测试
    // ============================================================

    #[test]
    fn test_record_token_outcome_success() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S5Parliament)
            .unwrap();

        registry
            .record_token_outcome(SeamId::S5Parliament, true)
            .unwrap();

        let token = registry.get_token(SeamId::S5Parliament).unwrap();
        assert_eq!(token.sample_count(), 1);
        assert!(token.success_ewma() > 0.5);
    }

    #[test]
    fn test_record_token_outcome_failure() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S5Parliament)
            .unwrap();

        registry
            .record_token_outcome(SeamId::S5Parliament, false)
            .unwrap();

        let token = registry.get_token(SeamId::S5Parliament).unwrap();
        assert_eq!(token.sample_count(), 1);
        assert!(token.success_ewma() < 0.5);
    }

    #[test]
    fn test_record_token_outcome_frozen_rejected() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S1Density)
            .unwrap();
        registry.freeze_token(SeamId::S1Density).unwrap();

        let result = registry.record_token_outcome(SeamId::S1Density, true);
        assert!(matches!(result, Err(DecayError::TokenFrozen(_))));
    }

    // ============================================================
    // maybe_promote_token 测试
    // ============================================================

    #[test]
    fn test_maybe_promote_token_no_promotion_when_ewma_low() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S3Prefetch)
            .unwrap();

        let promoted = registry.maybe_promote_token(SeamId::S3Prefetch).unwrap();
        assert!(!promoted);
    }

    #[test]
    fn test_maybe_promote_token_promotion_after_ewma_high() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S3Prefetch)
            .unwrap();

        for _ in 0..10 {
            registry
                .record_token_outcome(SeamId::S3Prefetch, true)
                .unwrap();
        }
        let promoted = registry.maybe_promote_token(SeamId::S3Prefetch).unwrap();
        assert!(promoted);
    }

    #[test]
    fn test_maybe_promote_token_frozen_rejected() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S1Density)
            .unwrap();
        registry.freeze_token(SeamId::S1Density).unwrap();

        let result = registry.maybe_promote_token(SeamId::S1Density);
        assert!(matches!(result, Err(DecayError::TokenFrozen(_))));
    }

    #[test]
    fn test_maybe_promote_token_cooldown_rejected() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S1Density)
            .unwrap();
        registry
            .trigger_asa_intervention(SeamId::S1Density)
            .unwrap();

        let result = registry.maybe_promote_token(SeamId::S1Density);
        assert!(matches!(result, Err(DecayError::CooldownActive(_))));
    }

    // ============================================================
    // trigger_asa_intervention 测试
    // ============================================================

    #[test]
    fn test_trigger_asa_intervention_enters_cooldown() {
        let registry = CapabilityTokenRegistry::new();
        registry.register_capability_token(SeamId::S6Decay).unwrap();

        let frozen = registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();
        assert!(!frozen);

        let status = registry.token_status(SeamId::S6Decay).unwrap();
        assert_eq!(status, CapabilityTokenStatus::Cooldown);
    }

    #[test]
    fn test_trigger_asa_intervention_auto_freeze_after_three() {
        let registry = CapabilityTokenRegistry::new();
        registry.register_capability_token(SeamId::S6Decay).unwrap();

        registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();
        registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();
        let frozen = registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();

        assert!(frozen);
        assert_eq!(
            registry.token_status(SeamId::S6Decay).unwrap(),
            CapabilityTokenStatus::Frozen
        );
    }

    // ============================================================
    // freeze_token / unfreeze_token 测试
    // ============================================================

    #[test]
    fn test_freeze_token_sets_frozen() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S2Memory)
            .unwrap();

        registry.freeze_token(SeamId::S2Memory).unwrap();
        assert_eq!(
            registry.token_status(SeamId::S2Memory).unwrap(),
            CapabilityTokenStatus::Frozen
        );
    }

    #[test]
    fn test_unfreeze_token_resets_to_initial() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S2Memory)
            .unwrap();
        registry.freeze_token(SeamId::S2Memory).unwrap();

        registry.unfreeze_token(SeamId::S2Memory).unwrap();
        assert_eq!(
            registry.token_status(SeamId::S2Memory).unwrap(),
            CapabilityTokenStatus::Provisional
        );
        assert!(
            (registry.token_authorized_level(SeamId::S2Memory).unwrap()
                - CapabilityToken::INITIAL_LEVEL)
                .abs()
                < 1e-6
        );
    }

    // ============================================================
    // freeze_all_learned_tokens 测试
    // ============================================================

    #[test]
    fn test_freeze_all_learned_tokens_freezes_authorized_and_cooldown() {
        let registry = CapabilityTokenRegistry::with_all_seams();

        // S6 提升到 Authorized
        for _ in 0..20 {
            registry
                .record_token_outcome(SeamId::S6Decay, true)
                .unwrap();
            registry.maybe_promote_token(SeamId::S6Decay).unwrap();
        }
        // S1 进入 Cooldown
        registry
            .trigger_asa_intervention(SeamId::S1Density)
            .unwrap();

        // 批量冻结
        registry.freeze_all_learned_tokens();

        // 所有 token 应为 Frozen
        for seam in SeamId::all() {
            assert_eq!(
                registry.token_status(seam).unwrap(),
                CapabilityTokenStatus::Frozen,
                "接缝 {} 应被冻结",
                seam.short_name()
            );
        }
    }

    #[test]
    fn test_freeze_all_learned_tokens_idempotent() {
        let registry = CapabilityTokenRegistry::with_all_seams();
        registry.freeze_all_learned_tokens();
        registry.freeze_all_learned_tokens(); // 二次调用不应 panic

        for seam in SeamId::all() {
            assert_eq!(
                registry.token_status(seam).unwrap(),
                CapabilityTokenStatus::Frozen
            );
        }
    }

    // ============================================================
    // maybe_recover_all_from_cooldown 测试
    // ============================================================

    #[test]
    fn test_maybe_recover_all_from_cooldown_recovers_expired() {
        let registry = CapabilityTokenRegistry::with_all_seams();

        // S1 提升到 Authorized
        for _ in 0..20 {
            registry
                .record_token_outcome(SeamId::S1Density, true)
                .unwrap();
            registry.maybe_promote_token(SeamId::S1Density).unwrap();
        }
        // S1 进入冷却期（当前时间约 now）
        registry
            .trigger_asa_intervention(SeamId::S1Density)
            .unwrap();
        assert_eq!(
            registry.token_status(SeamId::S1Density).unwrap(),
            CapabilityTokenStatus::Cooldown
        );

        // 等待冷却期结束（通过手动调整 cooldown_until）
        // WHY 不直接 sleep: 单元测试不应依赖真实时间流逝
        {
            let mut token = registry.tokens.get_mut(&SeamId::S1Density).unwrap();
            // 将 cooldown_until 设为过去时间，模拟冷却期结束
            token.cooldown_until = Some(0); // 远古时间，确保已过冷却期
        }

        let recovered = registry.maybe_recover_all_from_cooldown().unwrap();
        assert!(recovered >= 1);
        // 冷却期结束后，level 仍 >= 阈值，恢复为 Authorized
        assert_eq!(
            registry.token_status(SeamId::S1Density).unwrap(),
            CapabilityTokenStatus::Authorized
        );
    }

    // ============================================================
    // list_tokens 测试
    // ============================================================

    #[test]
    fn test_list_tokens_returns_all_registered() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S1Density)
            .unwrap();
        registry.register_capability_token(SeamId::S6Decay).unwrap();

        let tokens = registry.list_tokens();
        assert_eq!(tokens.len(), 2);
        assert!(tokens.iter().any(|(s, _, _, _, _)| *s == SeamId::S1Density));
        assert!(tokens.iter().any(|(s, _, _, _, _)| *s == SeamId::S6Decay));
    }

    // ============================================================
    // get_token 测试
    // ============================================================

    #[test]
    fn test_get_token_returns_ref() {
        let registry = CapabilityTokenRegistry::new();
        registry
            .register_capability_token(SeamId::S5Parliament)
            .unwrap();

        let token_ref = registry.get_token(SeamId::S5Parliament).unwrap();
        assert_eq!(token_ref.seam(), SeamId::S5Parliament);
    }

    #[test]
    fn test_get_token_not_found() {
        let registry = CapabilityTokenRegistry::new();
        let result = registry.get_token(SeamId::S6Decay);
        assert!(matches!(result, Err(DecayError::TokenNotFound(_))));
    }

    // ============================================================
    // current_utc_secs 测试
    // ============================================================

    #[test]
    fn test_current_utc_secs_returns_positive() {
        let secs = current_utc_secs().unwrap();
        // 2026 年的时间戳应远大于 0
        assert!(secs > 1_700_000_000); // 2023-11-14 之后的时间戳
    }

    // ============================================================
    // 端到端场景测试
    // ============================================================

    #[test]
    fn test_scenario_full_lifecycle_s6() {
        // 模拟 S6 接缝的完整生命周期
        let registry = CapabilityTokenRegistry::new();
        registry.register_capability_token(SeamId::S6Decay).unwrap();

        // 1. 初始状态：不允许 Learned
        assert!(!registry
            .should_activate_learned(SeamId::S6Decay, 0)
            .unwrap());

        // 2. omega-learner 反馈 20 次成功 outcome + 提升
        for _ in 0..20 {
            registry
                .record_token_outcome(SeamId::S6Decay, true)
                .unwrap();
            registry.maybe_promote_token(SeamId::S6Decay).unwrap();
        }

        // 3. 达到激活阈值
        assert!(registry
            .should_activate_learned(SeamId::S6Decay, 0)
            .unwrap());

        // 4. AsaIntervention 触发，进入冷却期
        registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();
        assert!(!registry
            .should_activate_learned(SeamId::S6Decay, 0)
            .unwrap());

        // 5. 冷却期结束后恢复（手动调整 cooldown_until 模拟）
        {
            let mut token = registry.tokens.get_mut(&SeamId::S6Decay).unwrap();
            token.cooldown_until = Some(0);
        }
        registry.maybe_recover_all_from_cooldown().unwrap();
        assert!(registry
            .should_activate_learned(SeamId::S6Decay, 0)
            .unwrap());

        // 6. 连续 3 次 ASA 自动冻结
        registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();
        registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();
        let frozen = registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();
        assert!(frozen);
        assert_eq!(
            registry.token_status(SeamId::S6Decay).unwrap(),
            CapabilityTokenStatus::Frozen
        );

        // 7. 手动解冻，回到初始状态
        registry.unfreeze_token(SeamId::S6Decay).unwrap();
        assert_eq!(
            registry.token_status(SeamId::S6Decay).unwrap(),
            CapabilityTokenStatus::Provisional
        );
    }

    #[test]
    fn test_scenario_multi_seam_independent() {
        // 验证多接缝 token 操作互不影响
        let registry = CapabilityTokenRegistry::with_all_seams();

        // 仅 S6 提升到 Authorized
        for _ in 0..20 {
            registry
                .record_token_outcome(SeamId::S6Decay, true)
                .unwrap();
            registry.maybe_promote_token(SeamId::S6Decay).unwrap();
        }

        // S6 激活，其他接缝仍为 Provisional
        assert!(registry
            .should_activate_learned(SeamId::S6Decay, 0)
            .unwrap());
        for seam in [
            SeamId::S1Density,
            SeamId::S2Memory,
            SeamId::S3Prefetch,
            SeamId::S4Selector,
            SeamId::S5Parliament,
        ] {
            assert!(
                !registry.should_activate_learned(seam, 0).unwrap(),
                "接缝 {} 不应激活",
                seam.short_name()
            );
        }

        // S6 触发 ASA，不影响其他接缝
        registry.trigger_asa_intervention(SeamId::S6Decay).unwrap();
        assert_eq!(
            registry.token_status(SeamId::S6Decay).unwrap(),
            CapabilityTokenStatus::Cooldown
        );
        for seam in [
            SeamId::S1Density,
            SeamId::S2Memory,
            SeamId::S3Prefetch,
            SeamId::S4Selector,
            SeamId::S5Parliament,
        ] {
            assert_eq!(
                registry.token_status(seam).unwrap(),
                CapabilityTokenStatus::Provisional,
                "接缝 {} 状态应不受影响",
                seam.short_name()
            );
        }
    }

    #[test]
    fn test_scenario_freeze_all_emergency() {
        // 模拟紧急熔断场景
        let registry = CapabilityTokenRegistry::with_all_seams();

        // S1/S2/S3 提升到 Authorized
        for seam in [SeamId::S1Density, SeamId::S2Memory, SeamId::S3Prefetch] {
            for _ in 0..20 {
                registry.record_token_outcome(seam, true).unwrap();
                registry.maybe_promote_token(seam).unwrap();
            }
            assert!(registry.should_activate_learned(seam, 0).unwrap());
        }

        // 紧急熔断：一键冻结所有
        registry.freeze_all_learned_tokens();

        for seam in SeamId::all() {
            assert_eq!(
                registry.token_status(seam).unwrap(),
                CapabilityTokenStatus::Frozen
            );
        }
    }
}
