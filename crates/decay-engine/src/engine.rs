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

use std::time::Instant;

use dashmap::DashMap;
use tracing::{debug, warn};

use crate::error::DecayError;
use crate::types::{Capability, CapabilityLevel, DecayConfig, DecayEvent};

/// 能力衰减引擎
///
/// 管理多个能力的权限流体等级,支持双驱动衰减与冻结/解冻。
pub struct DecayEngine {
    /// 能力注册表(id → Capability)
    /// 使用 DashMap 而非 HashMap+RwLock:衰减是"读-改-写"复合操作,
    /// DashMap 分片锁可在同一分片内原子完成,避免 RwLock 的 writer starvation
    capabilities: DashMap<String, Capability>,
    /// 衰减配置
    config: DecayConfig,
}

impl DecayEngine {
    /// 创建新的衰减引擎
    pub fn new(config: DecayConfig) -> Self {
        Self {
            capabilities: DashMap::new(),
            config,
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
    pub fn decay(&self, id: &str, event: DecayEvent) -> Result<CapabilityLevel, DecayError> {
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
                let decay_amount = elapsed * self.config.time_decay_rate;
                // clamp 确保在 [min_level, 1.0] 内,避免浮点误差越界
                let lower = self.config.min_level.max(0.0);
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
                let penalty = self.config.event_decay_penalty * severity;
                let lower = self.config.min_level.max(0.0);
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
                let restore_amount = elapsed * self.config.restore_rate;
                let new_value = (cap.level.value() + restore_amount).clamp(0.0, 1.0);
                cap.level = CapabilityLevel::new(new_value)?;
                cap.last_decay_at = now;
                debug!(capability_id = id, new_value, elapsed, "能力恢复");
            }
        }

        // 自动冻结:低于阈值且未冻结则冻结
        // 防止权限过低仍可操作的安全风险(对应尸检教训:权限不应残留)
        if check_auto_freeze && !cap.frozen && cap.level.value() <= self.config.freeze_threshold {
            cap.frozen = true;
            cap.level = CapabilityLevel::new(0.0)?;
            warn!(
                capability_id = id,
                threshold = self.config.freeze_threshold,
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
}
