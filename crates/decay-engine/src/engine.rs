//! DecayEngine 实现 — 能力衰减引擎核心逻辑
//!
//! 双驱动衰减:
//! - 时间驱动:随时间自然递减(防止权限长期闲置累积)
//! - 事件驱动:违规事件触发惩罚性衰减
//!
//! 冻结/解冻 API 对应 Skeptic 否决权(Week 5 Parliament 实现):
//! - freeze:Skeptic 投票否决,立即清零权限
//! - unfreeze:否决解除,从阈值之上逐步恢复
//!
//! 线程安全:基于 DashMap,可跨 async 任务共享(Send + Sync)
//!
//! # P4-W14.4 S6 接缝扩展
//!
//! 新增 `decay_with_policy` 方法支持策略感知衰减:接收 `DecayPolicy` 参数,
//! 从中提取 `DecayProfile` 转换为临时 `DecayConfig` 应用到本次衰减操作。
//! 与既有 `decay` 共享核心逻辑（`decay_with_config`），保持向后兼容。
//! 详见 `learner_holder::DecayLearnerHolder`（C4 合规三层 fallback）。

use std::time::Instant;

use dashmap::DashMap;
use nexus_contracts::{CapabilityTokenStatus, DecayPolicy, DecayProfile, SeamId};
use tracing::{debug, warn};

use crate::capability_registry::CapabilityTokenRegistry;
use crate::error::DecayError;
use crate::types::{Capability, CapabilityLevel, DecayConfig, DecayEvent};

/// 能力衰减引擎
///
/// 管理多个能力的权限流体等级,支持双驱动衰减与冻结/解冻。
///
/// # P4-W14.5 扩展
///
/// 嵌入 `CapabilityTokenRegistry`,作为学习策略灰度授权的中心管理点。
/// 编排器（chimera-cli / quest-engine）在注入 `*Policy::Learned` 前
/// 查询 `should_activate_learned`,未授权则本地 fallback 到 Static（C4 合规）。
pub struct DecayEngine {
    /// 能力注册表(id → Capability)
    /// 使用 DashMap 而非 HashMap+RwLock:衰减是"读-改-写"复合操作,
    /// DashMap 分片锁可在同一分片内原子完成,避免 RwLock 的 writer starvation
    capabilities: DashMap<String, Capability>,
    /// 衰减配置
    config: DecayConfig,
    /// P4-W14.5: 能力场令牌注册表（学习策略灰度授权管理）
    ///
    /// WHY 嵌入 DecayEngine:
    /// - 语义一致: DecayEngine 是"能力衰减引擎",管理 CapabilityToken 与其语义一致
    /// - 集中管理: 所有能力场操作（衰减 + token）集中在 DecayEngine
    /// - 零运行时开销: 仅在策略注入路径查询 token,热路径（decay）不查询
    token_registry: CapabilityTokenRegistry,
}

impl DecayEngine {
    /// 创建新的衰减引擎
    pub fn new(config: DecayConfig) -> Self {
        Self {
            capabilities: DashMap::new(),
            config,
            token_registry: CapabilityTokenRegistry::new(),
        }
    }

    /// 注册新能力
    ///
    /// # 参数
    /// - `id`:能力唯一标识
    /// - `name`:能力名称(人类可读)
    /// - `initial_level`:初始权限等级 [0.0, 1.0]
    ///
    /// # 错误
    /// - [`DecayError::ConfigError`]:ID 已存在
    /// - [`DecayError::InvalidLevel`]:initial_level 超出 [0.0, 1.0]
    pub fn register_capability(
        &self,
        id: &str,
        name: &str,
        initial_level: f32,
    ) -> Result<(), DecayError> {
        if self.capabilities.contains_key(id) {
            return Err(DecayError::ConfigError(format!("能力已存在: {id}")));
        }

        let level = CapabilityLevel::new(initial_level)?;
        let capability = Capability {
            id: id.to_string(),
            name: name.to_string(),
            level,
            frozen: false,
            last_decay_at: Instant::now(),
        };

        self.capabilities.insert(id.to_string(), capability);
        debug!(
            capability_id = id,
            initial_level = initial_level,
            "能力已注册"
        );
        Ok(())
    }

    /// 获取能力当前等级
    pub fn get_level(&self, id: &str) -> Result<CapabilityLevel, DecayError> {
        self.capabilities
            .get(id)
            .map(|c| c.level)
            .ok_or_else(|| DecayError::CapabilityNotFound(id.to_string()))
    }

    /// 应用衰减事件
    ///
    /// 根据事件类型更新能力等级:
    /// - [`DecayEvent::TimeDecay`]:level -= elapsed × time_decay_rate
    /// - [`DecayEvent::ViolationPenalty`]:level -= event_decay_penalty × severity
    /// - [`DecayEvent::Freeze`]:level = 0.0, frozen = true
    /// - [`DecayEvent::Restore`]:level += restore_rate × elapsed(若未冻结)
    ///
    /// 衰减后自动检查 freeze_threshold:低于阈值自动冻结
    /// (仅 TimeDecay/ViolationPenalty 触发;Restore 是恢复操作,不应因 level 低而冻结)。
    ///
    /// # 向后兼容
    ///
    /// 本方法使用构造时配置的 `self.config`，与 P4 修复前行为完全一致。
    /// 策略感知衰减请用 `decay_with_policy`（S6 接缝）。
    pub fn decay(&self, id: &str, event: DecayEvent) -> Result<CapabilityLevel, DecayError> {
        // 委托给 decay_with_config，使用 self.config（向后兼容）
        let config = self.config.clone();
        self.decay_with_config(id, event, &config)
    }

    /// 应用衰减事件（策略感知，S6 接缝）
    ///
    /// 与 `decay` 共享核心逻辑，但使用 `policy.profile()` 转换出的临时
    /// `DecayConfig`，使衰减参数随场景自适应：
    /// - 高风险写操作 → `Strict` 档位（快衰减、高惩罚）
    /// - 低风险只读操作 → `Lenient` 档位（慢衰减、低惩罚）
    ///
    /// # 设计（WHY 委托模式）
    ///
    /// - 不修改原 `decay` 签名（向后兼容）
    /// - 共享 `decay_with_config` 核心（避免逻辑漂移）
    /// - `policy.profile()` 是 const fn，零运行时开销
    ///
    /// # C4 合规
    ///
    /// 调用方传入 `DecayPolicy::fallback()` 时，行为与 `decay` 完全一致
    /// （profile = Standard = DecayConfig::default()）。`omega-learner` panic/超时
    /// 时调用方本地 fallback 到 `Static(Standard)`，无跨 crate 旗标传播。
    ///
    /// # 参数
    /// - `id`: 能力 ID
    /// - `event`: 衰减事件
    /// - `policy`: 衰减策略（Static 或 Learned）
    ///
    /// # 示例
    ///
    /// ```
    /// use decay_engine::{DecayEngine, DecayConfig, DecayEvent};
    /// use nexus_contracts::{DecayPolicy, DecayProfile};
    ///
    /// let engine = DecayEngine::new(DecayConfig::default());
    /// engine.register_capability("file_write", "文件写入", 1.0).unwrap();
    ///
    /// // 使用 Strict 档位衰减（高风险写操作场景）
    /// let policy = DecayPolicy::static_policy(DecayProfile::Strict);
    /// let level = engine.decay_with_policy(
    ///     "file_write",
    ///     DecayEvent::ViolationPenalty { capability_id: "file_write".into(), severity: 2.0 },
    ///     policy,
    /// ).unwrap();
    /// assert!(level.value() < 1.0, "Strict 档位违规后权限应下降");
    /// ```
    pub fn decay_with_policy(
        &self,
        id: &str,
        event: DecayEvent,
        policy: DecayPolicy,
    ) -> Result<CapabilityLevel, DecayError> {
        let config = profile_to_config(policy.profile());
        self.decay_with_config(id, event, &config)
    }

    /// 应用衰减事件（指定配置，共享核心逻辑）
    ///
    /// `decay` 与 `decay_with_policy` 的共同实现。使用传入的 `config`
    /// 而非 `self.config`，使策略感知路径可注入临时配置。
    ///
    /// # 设计（WHY 提取共享方法）
    ///
    /// - 避免逻辑漂移:单点修改 `decay` 与 `decay_with_policy` 同时生效
    /// - 共享自动冻结检查、clamp 边界、Skeptic 冻结语义
    /// - 便于未来扩展新的策略感知路径（如运行时动态配置）
    fn decay_with_config(
        &self,
        id: &str,
        event: DecayEvent,
        config: &DecayConfig,
    ) -> Result<CapabilityLevel, DecayError> {
        let now = Instant::now();
        let mut cap = self
            .capabilities
            .get_mut(id)
            .ok_or_else(|| DecayError::CapabilityNotFound(id.to_string()))?;

        let elapsed = now.duration_since(cap.last_decay_at).as_secs_f32();
        // 自动冻结检查标志:仅在衰减操作后触发,Restore 不触发(恢复不应导致冻结)
        let mut check_auto_freeze = false;

        match event {
            DecayEvent::TimeDecay => {
                if cap.frozen {
                    debug!(capability_id = id, "能力已冻结,跳过时间衰减");
                    return Ok(cap.level);
                }
                let decay_amount = elapsed * config.time_decay_rate;
                // clamp 确保在 [min_level, 1.0] 内,避免浮点误差越界
                let lower = config.min_level.max(0.0);
                let new_value = (cap.level.value() - decay_amount).clamp(lower, 1.0);
                cap.level = CapabilityLevel::new(new_value)?;
                cap.last_decay_at = now;
                check_auto_freeze = true;
                debug!(capability_id = id, new_value, elapsed, "时间衰减应用");
            }
            DecayEvent::ViolationPenalty { severity, .. } => {
                if cap.frozen {
                    debug!(capability_id = id, "能力已冻结,跳过违规惩罚");
                    return Ok(cap.level);
                }
                let penalty = config.event_decay_penalty * severity;
                let lower = config.min_level.max(0.0);
                let new_value = (cap.level.value() - penalty).clamp(lower, 1.0);
                cap.level = CapabilityLevel::new(new_value)?;
                cap.last_decay_at = now;
                check_auto_freeze = true;
                debug!(capability_id = id, new_value, severity, "违规惩罚应用");
            }
            DecayEvent::Freeze { reason, .. } => {
                cap.level = CapabilityLevel::new(0.0)?;
                cap.frozen = true;
                cap.last_decay_at = now;
                warn!(capability_id = id, reason = %reason, "能力已冻结(Skeptic 否决)");
            }
            DecayEvent::Restore { .. } => {
                if cap.frozen {
                    debug!(capability_id = id, "能力已冻结,跳过恢复");
                    return Ok(cap.level);
                }
                let restore_amount = elapsed * config.restore_rate;
                let new_value = (cap.level.value() + restore_amount).clamp(0.0, 1.0);
                cap.level = CapabilityLevel::new(new_value)?;
                cap.last_decay_at = now;
                debug!(capability_id = id, new_value, elapsed, "能力恢复");
            }
        }

        // 自动冻结:低于阈值且未冻结则冻结
        // 防止权限过低仍可操作的安全风险(对应尸检教训:权限不应残留)
        if check_auto_freeze && !cap.frozen && cap.level.value() <= config.freeze_threshold {
            cap.frozen = true;
            cap.level = CapabilityLevel::new(0.0)?;
            warn!(
                capability_id = id,
                threshold = config.freeze_threshold,
                "能力低于冻结阈值,自动冻结"
            );
        }

        Ok(cap.level)
    }

    /// 冻结能力(对应 Skeptic 否决权)
    ///
    /// 立即将 level 清零并标记 frozen,阻止该能力的所有操作。
    /// 幂等保护:已冻结的能力再次冻结返回 [`DecayError::AlreadyFrozen`]。
    pub fn freeze(&self, id: &str, reason: &str) -> Result<(), DecayError> {
        let mut cap = self
            .capabilities
            .get_mut(id)
            .ok_or_else(|| DecayError::CapabilityNotFound(id.to_string()))?;

        if cap.frozen {
            return Err(DecayError::AlreadyFrozen(id.to_string()));
        }

        cap.level = CapabilityLevel::new(0.0)?;
        cap.frozen = true;
        cap.last_decay_at = Instant::now();
        warn!(capability_id = id, reason = %reason, "能力已手动冻结");
        Ok(())
    }

    /// 解冻能力
    ///
    /// 解冻后 level 设为 freeze_threshold 之上,避免立即被自动冻结
    /// (否则解冻毫无意义:解冻→衰减检查→再次冻结)。
    pub fn unfreeze(&self, id: &str) -> Result<(), DecayError> {
        let mut cap = self
            .capabilities
            .get_mut(id)
            .ok_or_else(|| DecayError::CapabilityNotFound(id.to_string()))?;

        if !cap.frozen {
            return Err(DecayError::NotFrozen(id.to_string()));
        }

        // 解冻后从 freeze_threshold 之上起步:避免解冻后立即被自动冻结
        let restore_level = self
            .config
            .min_level
            .max(self.config.freeze_threshold + 0.01)
            .min(1.0);
        cap.level = CapabilityLevel::new(restore_level)?;
        cap.frozen = false;
        cap.last_decay_at = Instant::now();
        debug!(capability_id = id, restore_level, "能力已解冻");
        Ok(())
    }

    /// 查询能力是否冻结
    pub fn is_frozen(&self, id: &str) -> Result<bool, DecayError> {
        self.capabilities
            .get(id)
            .map(|c| c.frozen)
            .ok_or_else(|| DecayError::CapabilityNotFound(id.to_string()))
    }

    /// 列出所有能力(id, level, frozen)
    pub fn list_capabilities(&self) -> Vec<(String, CapabilityLevel, bool)> {
        self.capabilities
            .iter()
            .map(|c| (c.id.clone(), c.level, c.frozen))
            .collect()
    }

    // ============================================================
    // P4-W14.5: CapabilityToken 委托方法
    // ============================================================
    //
    // 以下方法委托给内部的 `token_registry`,使编排器无需直接访问 registry,
    // 通过 `DecayEngine` 即可完成所有 token 操作。
    //
    // 设计决策（WHY 委托模式而非 pub 字段）:
    // - 封装: 隐藏 registry 的内部实现（DashMap）
    // - 可演进: 未来可替换 registry 实现,不影响调用方
    // - 一致性: 所有能力场操作（衰减 + token）通过 DecayEngine 统一入口

    /// 获取 token registry 的引用（用于深度诊断或批量操作）
    ///
    /// WHY 提供: 某些批量操作（如 `freeze_all_learned_tokens`）需要直接访问 registry,
    /// 避免逐个接缝调用委托方法的开销。
    pub fn token_registry(&self) -> &CapabilityTokenRegistry {
        &self.token_registry
    }

    /// 注册指定接缝的能力令牌（初始低能力 + Provisional 状态）
    ///
    /// 委托 [`CapabilityTokenRegistry::register_capability_token`]。
    pub fn register_capability_token(&self, seam: SeamId) -> Result<(), DecayError> {
        self.token_registry.register_capability_token(seam)
    }

    /// 查询指定接缝的授权等级
    ///
    /// 委托 [`CapabilityTokenRegistry::token_authorized_level`]。
    pub fn token_authorized_level(&self, seam: SeamId) -> Result<f32, DecayError> {
        self.token_registry.token_authorized_level(seam)
    }

    /// 查询指定接缝的状态
    ///
    /// 委托 [`CapabilityTokenRegistry::token_status`]。
    pub fn token_status(&self, seam: SeamId) -> Result<CapabilityTokenStatus, DecayError> {
        self.token_registry.token_status(seam)
    }

    /// 查询是否允许 Learned 策略（C4 合规核心查询）
    ///
    /// 编排器在调用 `holder.update_policy(Learned)` 前必须查询此方法。
    /// 返回 false 时,编排器应本地 fallback 到 Static（C4 合规第三层）。
    ///
    /// 委托 [`CapabilityTokenRegistry::should_activate_learned`]。
    pub fn should_activate_learned(&self, seam: SeamId, now: i64) -> Result<bool, DecayError> {
        self.token_registry.should_activate_learned(seam, now)
    }

    /// 记录 token 执行结果（EWMA 更新）
    ///
    /// 委托 [`CapabilityTokenRegistry::record_token_outcome`]。
    pub fn record_token_outcome(&self, seam: SeamId, success: bool) -> Result<(), DecayError> {
        self.token_registry.record_token_outcome(seam, success)
    }

    /// 尝试渐进授权提升
    ///
    /// 委托 [`CapabilityTokenRegistry::maybe_promote_token`]。
    pub fn maybe_promote_token(&self, seam: SeamId) -> Result<bool, DecayError> {
        self.token_registry.maybe_promote_token(seam)
    }

    /// 触发 AsaIntervention 安全闭环
    ///
    /// 编排器在收到 `AsaIntervention` 事件时调用此方法。
    ///
    /// 委托 [`CapabilityTokenRegistry::trigger_asa_intervention`]。
    pub fn trigger_asa_intervention(&self, seam: SeamId) -> Result<bool, DecayError> {
        self.token_registry.trigger_asa_intervention(seam)
    }

    /// 衰减指定接缝的 token（手动衰减）
    ///
    /// 委托 [`CapabilityTokenRegistry::decay_capability_token`]。
    pub fn decay_capability_token(&self, seam: SeamId, amount: f32) -> Result<(), DecayError> {
        self.token_registry.decay_capability_token(seam, amount)
    }

    /// 冻结指定接缝的 token
    ///
    /// 委托 [`CapabilityTokenRegistry::freeze_token`]。
    pub fn freeze_token(&self, seam: SeamId) -> Result<(), DecayError> {
        self.token_registry.freeze_token(seam)
    }

    /// 解冻指定接缝的 token（重置为初始状态）
    ///
    /// 委托 [`CapabilityTokenRegistry::unfreeze_token`]。
    pub fn unfreeze_token(&self, seam: SeamId) -> Result<(), DecayError> {
        self.token_registry.unfreeze_token(seam)
    }

    /// 冻结所有已激活的 token（批量熔断）
    ///
    /// 编排器在严重故障时一键回退所有 Learned 策略。
    ///
    /// 委托 [`CapabilityTokenRegistry::freeze_all_learned_tokens`]。
    pub fn freeze_all_learned_tokens(&self) {
        self.token_registry.freeze_all_learned_tokens();
    }

    /// 检查并恢复所有冷却期结束的 token
    ///
    /// 编排器定期调用（如每秒），将冷却期结束的 token 恢复到正常状态。
    ///
    /// 委托 [`CapabilityTokenRegistry::maybe_recover_all_from_cooldown`]。
    pub fn maybe_recover_all_from_cooldown(&self) -> Result<i32, DecayError> {
        self.token_registry.maybe_recover_all_from_cooldown()
    }
}

// ============================================================
// S6 接缝辅助函数
// ============================================================

/// 将 `DecayProfile` 转换为 `DecayConfig`（S6 接缝辅助函数）
///
/// `DecayProfile` 是 L0 契约层定义的枚举（4 档位），
/// `DecayConfig` 是 L4 decay-engine 的运行时配置（5 浮点字段）。
/// 本函数完成 L0 → L4 的类型转换，供 `decay_with_policy` 使用。
///
/// # 设计决策（WHY 自由函数而非 trait）
///
/// - **避免 L0 依赖 L4**: 若在 `DecayProfile` 上实现 `to_config()` 方法，
///   会让 L0 `nexus-contracts` 依赖 L4 `decay-engine`（违反依赖铁律向上禁止）
/// - **L4 主动适配 L0**: 转换函数放在 L4 decay-engine，L4 → L0 依赖方向合规
/// - **`pub(crate)` 可见性**: 仅 engine.rs 内部使用，不暴露给外部
///
/// # 参数映射
///
/// | DecayProfile 字段 | DecayConfig 字段 |
/// |-------------------|-------------------|
/// | `time_decay_rate()` | `time_decay_rate` |
/// | `event_decay_penalty()` | `event_decay_penalty` |
/// | `min_level()` | `min_level` |
/// | `freeze_threshold()` | `freeze_threshold` |
/// | `restore_rate()` | `restore_rate` |
pub(crate) fn profile_to_config(profile: DecayProfile) -> DecayConfig {
    DecayConfig {
        time_decay_rate: profile.time_decay_rate(),
        event_decay_penalty: profile.event_decay_penalty(),
        min_level: profile.min_level(),
        freeze_threshold: profile.freeze_threshold(),
        restore_rate: profile.restore_rate(),
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::{DecayPolicy, DecayProfile};

    // ============================================================
    // profile_to_config 测试
    // ============================================================

    #[test]
    fn test_profile_to_config_lenient() {
        let config = profile_to_config(DecayProfile::Lenient);
        assert!((config.time_decay_rate - 0.0005).abs() < 1e-6);
        assert!((config.event_decay_penalty - 0.05).abs() < 1e-6);
        assert!((config.freeze_threshold - 0.02).abs() < 1e-6);
        assert!((config.restore_rate - 0.01).abs() < 1e-6);
        assert!(config.min_level.abs() < 1e-6);
    }

    #[test]
    fn test_profile_to_config_standard() {
        // Standard 档位必须等价于 DecayConfig::default()（C4 合规）
        let config = profile_to_config(DecayProfile::Standard);
        let default_config = DecayConfig::default();
        assert!((config.time_decay_rate - default_config.time_decay_rate).abs() < 1e-6);
        assert!((config.event_decay_penalty - default_config.event_decay_penalty).abs() < 1e-6);
        assert!((config.freeze_threshold - default_config.freeze_threshold).abs() < 1e-6);
        assert!((config.restore_rate - default_config.restore_rate).abs() < 1e-6);
        assert!((config.min_level - default_config.min_level).abs() < 1e-6);
    }

    #[test]
    fn test_profile_to_config_strict() {
        let config = profile_to_config(DecayProfile::Strict);
        assert!((config.time_decay_rate - 0.005).abs() < 1e-6);
        assert!((config.event_decay_penalty - 0.15).abs() < 1e-6);
        assert!((config.freeze_threshold - 0.10).abs() < 1e-6);
    }

    #[test]
    fn test_profile_to_config_aggressive() {
        let config = profile_to_config(DecayProfile::Aggressive);
        assert!((config.time_decay_rate - 0.01).abs() < 1e-6);
        assert!((config.event_decay_penalty - 0.2).abs() < 1e-6);
        assert!((config.freeze_threshold - 0.15).abs() < 1e-6);
    }

    // ============================================================
    // decay_with_policy 基础行为测试
    // ============================================================

    #[test]
    fn test_decay_with_policy_fallback_equivalent_to_decay() {
        // C4 合规: 传入 fallback() (Static(Standard)) 行为应与 decay() 完全一致
        let engine1 = DecayEngine::new(DecayConfig::default());
        let engine2 = DecayEngine::new(DecayConfig::default());

        engine1.register_capability("cap", "测试", 1.0).unwrap();
        engine2.register_capability("cap", "测试", 1.0).unwrap();

        let event = DecayEvent::ViolationPenalty {
            capability_id: "cap".into(),
            severity: 1.0,
        };

        let level1 = engine1.decay("cap", event.clone()).unwrap();
        let level2 = engine2
            .decay_with_policy("cap", event, DecayPolicy::fallback())
            .unwrap();
        assert!((level1.value() - level2.value()).abs() < 1e-6);
    }

    #[test]
    fn test_decay_with_policy_strict_higher_penalty() {
        // Strict 档位惩罚应大于 Standard（同 severity 下）
        let engine1 = DecayEngine::new(DecayConfig::default());
        let engine2 = DecayEngine::new(DecayConfig::default());

        engine1.register_capability("cap1", "测试1", 1.0).unwrap();
        engine2.register_capability("cap2", "测试2", 1.0).unwrap();

        let event = DecayEvent::ViolationPenalty {
            capability_id: "cap1".into(),
            severity: 1.0,
        };

        let level_standard = engine1
            .decay_with_policy(
                "cap1",
                event.clone(),
                DecayPolicy::static_policy(DecayProfile::Standard),
            )
            .unwrap();
        let level_strict = engine2
            .decay_with_policy(
                "cap2",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap2".into(),
                    severity: 1.0,
                },
                DecayPolicy::static_policy(DecayProfile::Strict),
            )
            .unwrap();

        // Strict penalty=0.15 > Standard penalty=0.1，所以 Strict 衰减更多
        assert!(level_strict.value() < level_standard.value());
    }

    #[test]
    fn test_decay_with_policy_lenient_lower_penalty() {
        // Lenient 档位惩罚应小于 Standard
        let engine1 = DecayEngine::new(DecayConfig::default());
        let engine2 = DecayEngine::new(DecayConfig::default());

        engine1.register_capability("cap1", "测试1", 1.0).unwrap();
        engine2.register_capability("cap2", "测试2", 1.0).unwrap();

        let level_standard = engine1
            .decay_with_policy(
                "cap1",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap1".into(),
                    severity: 1.0,
                },
                DecayPolicy::static_policy(DecayProfile::Standard),
            )
            .unwrap();
        let level_lenient = engine2
            .decay_with_policy(
                "cap2",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap2".into(),
                    severity: 1.0,
                },
                DecayPolicy::static_policy(DecayProfile::Lenient),
            )
            .unwrap();

        // Lenient penalty=0.05 < Standard penalty=0.1，所以 Lenient 衰减更少
        assert!(level_lenient.value() > level_standard.value());
    }

    #[test]
    fn test_decay_with_policy_aggressive_highest_penalty() {
        // Aggressive 档位惩罚最高
        let engine = DecayEngine::new(DecayConfig::default());
        engine.register_capability("cap", "测试", 1.0).unwrap();

        let level = engine
            .decay_with_policy(
                "cap",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap".into(),
                    severity: 1.0,
                },
                DecayPolicy::static_policy(DecayProfile::Aggressive),
            )
            .unwrap();

        // Aggressive penalty=0.2，衰减后 level = 1.0 - 0.2 = 0.8
        assert!((level.value() - 0.8).abs() < 1e-6);
    }

    // ============================================================
    // decay_with_policy 学习策略测试
    // ============================================================

    #[test]
    fn test_decay_with_policy_learned_equivalent_to_static() {
        // Learned 与 Static 同档位行为应一致
        let engine1 = DecayEngine::new(DecayConfig::default());
        let engine2 = DecayEngine::new(DecayConfig::default());

        engine1.register_capability("cap1", "测试1", 1.0).unwrap();
        engine2.register_capability("cap2", "测试2", 1.0).unwrap();

        let event = DecayEvent::ViolationPenalty {
            capability_id: "cap1".into(),
            severity: 1.0,
        };

        let level_static = engine1
            .decay_with_policy(
                "cap1",
                event.clone(),
                DecayPolicy::static_policy(DecayProfile::Strict),
            )
            .unwrap();
        let level_learned = engine2
            .decay_with_policy(
                "cap2",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap2".into(),
                    severity: 1.0,
                },
                DecayPolicy::learned(1, DecayProfile::Strict),
            )
            .unwrap();

        assert!((level_static.value() - level_learned.value()).abs() < 1e-6);
    }

    #[test]
    fn test_decay_with_policy_freeze_ignores_profile() {
        // Freeze 事件清零权限，与档位无关
        let engine = DecayEngine::new(DecayConfig::default());
        engine.register_capability("cap", "测试", 1.0).unwrap();

        let level = engine
            .decay_with_policy(
                "cap",
                DecayEvent::Freeze {
                    capability_id: "cap".into(),
                    reason: "测试冻结".into(),
                },
                DecayPolicy::static_policy(DecayProfile::Aggressive),
            )
            .unwrap();

        assert!(level.value().abs() < 1e-6);
        assert!(engine.is_frozen("cap").unwrap());
    }

    #[test]
    fn test_decay_with_policy_capability_not_found() {
        // 不存在的能力 ID 应返回错误
        let engine = DecayEngine::new(DecayConfig::default());
        let result = engine.decay_with_policy(
            "nonexistent",
            DecayEvent::TimeDecay,
            DecayPolicy::fallback(),
        );
        assert!(matches!(result, Err(DecayError::CapabilityNotFound(_))));
    }

    #[test]
    fn test_decay_with_policy_frozen_skips_time_decay() {
        // 已冻结能力跳过时间衰减（与 decay 行为一致）
        let engine = DecayEngine::new(DecayConfig::default());
        engine.register_capability("cap", "测试", 0.5).unwrap();
        engine.freeze("cap", "预冻结").unwrap();

        let level = engine
            .decay_with_policy(
                "cap",
                DecayEvent::TimeDecay,
                DecayPolicy::static_policy(DecayProfile::Aggressive),
            )
            .unwrap();

        // 已冻结，level 保持 0.0
        assert!(level.value().abs() < 1e-6);
    }

    // ============================================================
    // decay 向后兼容测试
    // ============================================================

    #[test]
    fn test_decay_unchanged_after_refactor() {
        // 重构后 decay 行为应与 P4 修复前一致
        let engine = DecayEngine::new(DecayConfig::default());
        engine.register_capability("cap", "测试", 1.0).unwrap();

        let level = engine
            .decay(
                "cap",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap".into(),
                    severity: 1.0,
                },
            )
            .unwrap();

        // penalty = 0.1 × 1.0 = 0.1，level = 1.0 - 0.1 = 0.9
        assert!((level.value() - 0.9).abs() < 1e-6);
    }

    // ============================================================
    // 自动冻结阈值测试（不同档位）
    // ============================================================

    #[test]
    fn test_decay_with_policy_auto_freeze_strict_threshold() {
        // Strict 档位 freeze_threshold=0.10，权限降到 0.10 以下应自动冻结
        let engine = DecayEngine::new(DecayConfig::default());
        // 初始权限设为 0.20（高于 Strict 阈值 0.10）
        engine.register_capability("cap", "测试", 0.20).unwrap();

        // 使用 Strict 档位 + severity=2.0 衰减
        // penalty = 0.15 × 2.0 = 0.30，new_level = 0.20 - 0.30 = -0.10 → clamp 到 min_level=0.0
        // 但 0.0 <= 0.10（Strict freeze_threshold），应触发自动冻结
        let level = engine
            .decay_with_policy(
                "cap",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap".into(),
                    severity: 2.0,
                },
                DecayPolicy::static_policy(DecayProfile::Strict),
            )
            .unwrap();

        // 自动冻结：level = 0.0
        assert!(level.value().abs() < 1e-6);
        assert!(engine.is_frozen("cap").unwrap());
    }

    #[test]
    fn test_decay_with_policy_no_freeze_lenient_threshold() {
        // Lenient 档位 freeze_threshold=0.02，相同 level 不会触发冻结
        let engine = DecayEngine::new(DecayConfig::default());
        engine.register_capability("cap", "测试", 0.20).unwrap();

        // Lenient penalty = 0.05 × 2.0 = 0.10，new_level = 0.20 - 0.10 = 0.10
        // 0.10 > 0.02（Lenient freeze_threshold），不触发自动冻结
        let level = engine
            .decay_with_policy(
                "cap",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap".into(),
                    severity: 2.0,
                },
                DecayPolicy::static_policy(DecayProfile::Lenient),
            )
            .unwrap();

        assert!((level.value() - 0.10).abs() < 1e-6);
        assert!(!engine.is_frozen("cap").unwrap());
    }

    // ============================================================
    // 端到端策略切换测试
    // ============================================================

    #[test]
    fn test_scenario_switching_profiles_across_operations() {
        // 模拟:同一能力在不同场景下使用不同档位衰减
        let engine = DecayEngine::new(DecayConfig::default());
        engine
            .register_capability("file_write", "文件写入", 1.0)
            .unwrap();

        // 1. 低风险场景: 使用 Lenient
        let level1 = engine
            .decay_with_policy(
                "file_write",
                DecayEvent::ViolationPenalty {
                    capability_id: "file_write".into(),
                    severity: 1.0,
                },
                DecayPolicy::static_policy(DecayProfile::Lenient),
            )
            .unwrap();
        // Lenient penalty=0.05，level = 1.0 - 0.05 = 0.95
        assert!((level1.value() - 0.95).abs() < 1e-6);

        // 2. 高风险场景: 使用 Strict
        let level2 = engine
            .decay_with_policy(
                "file_write",
                DecayEvent::ViolationPenalty {
                    capability_id: "file_write".into(),
                    severity: 1.0,
                },
                DecayPolicy::static_policy(DecayProfile::Strict),
            )
            .unwrap();
        // Strict penalty=0.15，level = 0.95 - 0.15 = 0.80
        assert!((level2.value() - 0.80).abs() < 1e-6);

        // 3. 学习策略下发 Aggressive
        let level3 = engine
            .decay_with_policy(
                "file_write",
                DecayEvent::ViolationPenalty {
                    capability_id: "file_write".into(),
                    severity: 1.0,
                },
                DecayPolicy::learned(1, DecayProfile::Aggressive),
            )
            .unwrap();
        // Aggressive penalty=0.2，level = 0.80 - 0.20 = 0.60
        assert!((level3.value() - 0.60).abs() < 1e-6);

        // 4. learner panic: fallback 到 Standard
        let level4 = engine
            .decay_with_policy(
                "file_write",
                DecayEvent::ViolationPenalty {
                    capability_id: "file_write".into(),
                    severity: 1.0,
                },
                DecayPolicy::fallback(),
            )
            .unwrap();
        // Standard penalty=0.1，level = 0.60 - 0.10 = 0.50
        assert!((level4.value() - 0.50).abs() < 1e-6);
    }
}
