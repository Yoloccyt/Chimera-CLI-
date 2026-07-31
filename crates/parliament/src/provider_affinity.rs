//! 跨厂商去相关 — Parliament 角色的 provider 绑定与去相关校验(MCA P7,ADR-067)
//!
//! 对应架构层:L8 Parliament
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.5 跨厂商议会
//!
//! # 同源相关失败(病理 D3)
//! L8 的 Skeptic/Security/Execution 角色若由同一模型自问自答,辩论存在
//! "同源相关失败"——同一模型的盲区在所有角色间相关,AHIRT 红队形同虚设。
//! 修复:凡涉及"验证/否决/红队"的第二意见,默认与生产者**不同厂商**。
//!
//! # 侧表方案(R8 构造点雪崩规避)
//! **不动** `RoleProfile` 与 5 角色默认配置。provider 绑定走独立侧表
//! `ProviderAffinityRegistry { RwLock<HashMap<RoleId, ProviderBinding>> }`,
//! 与 `RoleRegistry` 并存(RwLock 读多写少,bind 低频)。
//!
//! # 去相关硬约束(P7)
//! - Producer(PVL 生产者)与 Verifier(PVL 验证者)默认异厂商
//! - Skeptic 角色强制与提案生产者异厂商(否则红队形同虚设)
//!
//! # 依赖方向(§2.2 铁律)
//! 本模块依赖 L0 `nexus_contracts::affinity::ProviderId`(L8 → L0 合规),
//! 不依赖 L10 mca-gateway。

use std::collections::HashMap;
use std::sync::RwLock;

use nexus_contracts::affinity::ProviderId;

use crate::error::ParliamentError;
use crate::types::RoleId;

/// 单角色的 provider 绑定 — 生产者/验证者/怀疑者三方厂商
///
/// 三方厂商用于去相关校验:验证者与怀疑者必须与生产者异厂商。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBinding {
    /// 生产者(PVL Producer / 提案方)厂商
    pub producer: ProviderId,
    /// 验证者(PVL Verifier / 第二意见)厂商
    pub verifier: ProviderId,
    /// 怀疑者(Skeptic / 红队)厂商
    pub skeptic: ProviderId,
}

impl ProviderBinding {
    /// 构造绑定
    pub fn new(producer: ProviderId, verifier: ProviderId, skeptic: ProviderId) -> Self {
        Self {
            producer,
            verifier,
            skeptic,
        }
    }
}

/// 跨厂商去相关注册表 — 每角色(RoleId)的 provider 绑定侧表
///
/// # 线程安全
/// `RwLock<HashMap>` 读多写少:bind 低频(角色装配期),validate 高频(每次审议)。
/// 读锁并发无阻塞,写锁仅 bind 时短暂持有(对齐 RoleRegistry 模式)。
pub struct ProviderAffinityRegistry {
    bindings: RwLock<HashMap<RoleId, ProviderBinding>>,
}

impl ProviderAffinityRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            bindings: RwLock::new(HashMap::new()),
        }
    }

    /// 绑定角色的三方厂商(bind 前先去相关校验)
    ///
    /// # 错误
    /// - `ConfigError`:producer==verifier 或 producer==skeptic(去相关硬约束)
    pub fn bind_provider(
        &self,
        role_id: RoleId,
        binding: ProviderBinding,
    ) -> Result<(), ParliamentError> {
        validate_cross_provider(&binding)?;
        if let Ok(mut map) = self.bindings.write() {
            map.insert(role_id, binding);
        }
        Ok(())
    }

    /// 查询角色的 provider 绑定(未绑定返回 None)
    pub fn binding_of(&self, role_id: &RoleId) -> Option<ProviderBinding> {
        self.bindings.read().ok().and_then(|m| m.get(role_id).cloned())
    }

    /// 校验某角色的绑定是否满足去相关(审议前调用)
    ///
    /// 未绑定的角色返回 true(向后兼容:未启用跨厂商议会的场景不强制)。
    pub fn is_decorrelated(&self, role_id: &RoleId) -> bool {
        match self.binding_of(role_id) {
            Some(binding) => validate_cross_provider(&binding).is_ok(),
            None => true,
        }
    }
}

impl Default for ProviderAffinityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 跨厂商去相关校验(P7 硬约束)
///
/// # 规则
/// - `producer == verifier` → 错误(PVL 生产者与验证者必须异厂商)
/// - `producer == skeptic` → 错误(Skeptic 红队必须与生产者异厂商,否则形同虚设)
/// - verifier == skeptic 允许(第二意见与红队可同厂商,只要都与生产者异厂商)
///
/// # 错误
/// - `ConfigError`:违反去相关约束,携带冲突的厂商对
pub fn validate_cross_provider(binding: &ProviderBinding) -> Result<(), ParliamentError> {
    if binding.producer == binding.verifier {
        return Err(ParliamentError::ConfigError {
            detail: format!(
                "cross-provider violation: producer == verifier ({:?});PVL 生产者与验证者必须异厂商(P7)",
                binding.producer
            ),
        });
    }
    if binding.producer == binding.skeptic {
        return Err(ParliamentError::ConfigError {
            detail: format!(
                "cross-provider violation: producer == skeptic ({:?});Skeptic 红队必须与生产者异厂商(P7,否则形同虚设)",
                binding.producer
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(p: ProviderId, v: ProviderId, s: ProviderId) -> ProviderBinding {
        ProviderBinding::new(p, v, s)
    }

    #[test]
    fn validate_rejects_producer_eq_verifier() {
        let b = binding(ProviderId::Zhipu, ProviderId::Zhipu, ProviderId::DeepSeek);
        assert!(validate_cross_provider(&b).is_err());
    }

    #[test]
    fn validate_rejects_producer_eq_skeptic() {
        // Skeptic 与生产者同厂商 → 红队形同虚设(同源相关失败)
        let b = binding(ProviderId::MiniMax, ProviderId::DeepSeek, ProviderId::MiniMax);
        assert!(validate_cross_provider(&b).is_err());
    }

    #[test]
    fn validate_allows_decorrelated() {
        // 三方互异:通过
        let b = binding(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        assert!(validate_cross_provider(&b).is_ok());
        // verifier == skeptic 但都与 producer 异厂商:允许
        let b2 = binding(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::DeepSeek);
        assert!(validate_cross_provider(&b2).is_ok());
    }

    #[test]
    fn bind_rejects_correlated_binding() {
        let reg = ProviderAffinityRegistry::new();
        let bad = binding(ProviderId::Zhipu, ProviderId::Zhipu, ProviderId::DeepSeek);
        assert!(reg.bind_provider(RoleId::new("role-architect"), bad).is_err());
        // 拒绝后未写入
        assert!(reg.binding_of(&RoleId::new("role-architect")).is_none());
    }

    #[test]
    fn bind_and_query_decorrelated() {
        let reg = ProviderAffinityRegistry::new();
        let good = binding(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        reg.bind_provider(RoleId::new("role-skeptic"), good.clone()).unwrap();
        assert_eq!(reg.binding_of(&RoleId::new("role-skeptic")), Some(good));
        assert!(reg.is_decorrelated(&RoleId::new("role-skeptic")));
    }

    #[test]
    fn unbound_role_is_decorrelated_by_default() {
        // 未绑定角色向后兼容(未启用跨厂商议会的场景不强制)
        let reg = ProviderAffinityRegistry::new();
        assert!(reg.is_decorrelated(&RoleId::new("role-bard")));
    }
}
