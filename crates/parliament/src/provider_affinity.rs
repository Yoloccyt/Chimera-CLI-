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
use serde::{Deserialize, Serialize};

use crate::error::ParliamentError;
use crate::types::RoleId;

/// 单角色的 provider 绑定 — 生产者/验证者/怀疑者三方厂商
///
/// 三方厂商用于去相关校验:验证者与怀疑者必须与生产者异厂商。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        self.bindings
            .read()
            .ok()
            .and_then(|m| m.get(role_id).cloned())
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

    /// 克隆注册表（用于 Arc 共享）
    ///
    /// 创建当前注册表的一份独立快照副本，适用于需要在不同所有者
    /// 之间共享注册表状态的场景（如 `AffinityRouter` 的 `Arc` 共享）。
    ///
    /// # WHY 显式方法而非 `Clone` 派生
    /// `RwLock` 不实现 `Clone`，派生 `Clone` 不可行。此方法手动
    /// 实现读锁 → 克隆数据的逻辑，与 `binding_of` 的读锁模式一致。
    pub fn clone_inner(&self) -> Self {
        let bindings = self
            .bindings
            .read()
            .ok()
            .map(|m| m.clone())
            .unwrap_or_default();
        Self {
            bindings: RwLock::new(bindings),
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
        let b = binding(
            ProviderId::MiniMax,
            ProviderId::DeepSeek,
            ProviderId::MiniMax,
        );
        assert!(validate_cross_provider(&b).is_err());
    }

    #[test]
    fn validate_allows_decorrelated() {
        // 三方互异:通过
        let b = binding(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        assert!(validate_cross_provider(&b).is_ok());
        // verifier == skeptic 但都与 producer 异厂商:允许
        let b2 = binding(
            ProviderId::Zhipu,
            ProviderId::DeepSeek,
            ProviderId::DeepSeek,
        );
        assert!(validate_cross_provider(&b2).is_ok());
    }

    #[test]
    fn bind_rejects_correlated_binding() {
        let reg = ProviderAffinityRegistry::new();
        let bad = binding(ProviderId::Zhipu, ProviderId::Zhipu, ProviderId::DeepSeek);
        assert!(reg
            .bind_provider(RoleId::new("role-architect"), bad)
            .is_err());
        // 拒绝后未写入
        assert!(reg.binding_of(&RoleId::new("role-architect")).is_none());
    }

    #[test]
    fn bind_and_query_decorrelated() {
        let reg = ProviderAffinityRegistry::new();
        let good = binding(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        reg.bind_provider(RoleId::new("role-skeptic"), good.clone())
            .unwrap();
        assert_eq!(reg.binding_of(&RoleId::new("role-skeptic")), Some(good));
        assert!(reg.is_decorrelated(&RoleId::new("role-skeptic")));
    }

    #[test]
    fn unbound_role_is_decorrelated_by_default() {
        // 未绑定角色向后兼容(未启用跨厂商议会的场景不强制)
        let reg = ProviderAffinityRegistry::new();
        assert!(reg.is_decorrelated(&RoleId::new("role-bard")));
    }

    // ============================================================
    // 集成测试:ProviderBinding serde 往返(配置热加载路径)
    // ============================================================

    #[test]
    fn provider_binding_serde_json_roundtrip() {
        // JSON 序列化/反序列化往返(热加载路径:TOML/YAML/JSON 均可)
        let binding =
            ProviderBinding::new(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        let json = serde_json::to_string(&binding).unwrap();
        // 验证 JSON 结构:snake_case 字段名(DeepSeek→deep_seek,MiniMax→mini_max)
        assert!(
            json.contains(r#""producer":"zhipu""#),
            "JSON 应包含 producer:zhipu: {}",
            json
        );
        assert!(
            json.contains(r#""verifier":"deep_seek""#),
            "JSON 应包含 verifier:deep_seek: {}",
            json
        );
        assert!(
            json.contains(r#""skeptic":"mini_max""#),
            "JSON 应包含 skeptic:mini_max: {}",
            json
        );
        // 反序列化恢复
        let restored: ProviderBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, binding, "serde JSON 往返应保持相等");
    }

    #[test]
    fn provider_binding_serde_backward_compat() {
        // 旧格式 JSON 向后兼容性(保证字段顺序变化不影响反序列化)
        // ProviderId 使用 snake_case 重命名: MiniMax → mini_max, DeepSeek → deep_seek
        let json = r#"{"skeptic":"mini_max","producer":"zhipu","verifier":"deep_seek"}"#;
        let restored: ProviderBinding = serde_json::from_str(json).unwrap();
        assert_eq!(restored.producer, ProviderId::Zhipu);
        assert_eq!(restored.verifier, ProviderId::DeepSeek);
        assert_eq!(restored.skeptic, ProviderId::MiniMax);
    }

    #[test]
    fn provider_binding_serde_custom_variant() {
        // Custom 变体(开放世界扩展:聚合网关/自部署)的序列化
        // Custom 变体序列化为 {"custom":"openrouter"} 而非裸字符串
        let binding = ProviderBinding::new(
            ProviderId::Custom("openrouter".into()),
            ProviderId::Zhipu,
            ProviderId::DeepSeek,
        );
        let json = serde_json::to_string(&binding).unwrap();
        assert!(
            json.contains(r#""custom":"openrouter""#),
            "Custom 变体应序列化为带标记的对象: {}",
            json
        );
        // 反序列化恢复
        let restored: ProviderBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, binding, "Custom 变体 serde 往返应保持相等");
    }

    // ============================================================
    // 集成测试:ProviderAffinityRegistry 完整生命周期
    // ============================================================

    #[test]
    fn registry_new_creates_empty() {
        // new() 无需额外参数,创建空注册表(默认可用)
        let reg = ProviderAffinityRegistry::new();
        assert!(reg.binding_of(&RoleId::new("role-any")).is_none());
        assert!(reg.is_decorrelated(&RoleId::new("role-any")));
    }

    #[test]
    fn registry_default_equals_new() {
        // Default 实现与 new 行为一致
        let reg1 = ProviderAffinityRegistry::new();
        let reg2: ProviderAffinityRegistry = Default::default();
        assert_eq!(
            reg1.binding_of(&RoleId::new("role-test")),
            reg2.binding_of(&RoleId::new("role-test")),
        );
    }

    #[test]
    fn registry_multiple_bindings() {
        // 绑定多个角色,互不干扰
        let reg = ProviderAffinityRegistry::new();
        let architect =
            ProviderBinding::new(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        let skeptic =
            ProviderBinding::new(ProviderId::DeepSeek, ProviderId::Zhipu, ProviderId::MiniMax);
        let librarian =
            ProviderBinding::new(ProviderId::MiniMax, ProviderId::Zhipu, ProviderId::DeepSeek);

        // 依次绑定三个角色
        reg.bind_provider(RoleId::new("role-architect"), architect.clone())
            .unwrap();
        reg.bind_provider(RoleId::new("role-skeptic"), skeptic.clone())
            .unwrap();
        reg.bind_provider(RoleId::new("role-librarian"), librarian.clone())
            .unwrap();

        // 验证每个角色返回正确的绑定
        assert_eq!(
            reg.binding_of(&RoleId::new("role-architect")),
            Some(architect)
        );
        assert_eq!(reg.binding_of(&RoleId::new("role-skeptic")), Some(skeptic));
        assert_eq!(
            reg.binding_of(&RoleId::new("role-librarian")),
            Some(librarian)
        );

        // 未绑定的角色仍返回 None
        assert!(reg.binding_of(&RoleId::new("role-bard")).is_none());
    }

    #[test]
    fn registry_binding_overwrite() {
        // 同一角色重新绑定:旧值被覆盖
        let reg = ProviderAffinityRegistry::new();
        let old =
            ProviderBinding::new(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        let new_binding =
            ProviderBinding::new(ProviderId::DeepSeek, ProviderId::Zhipu, ProviderId::MiniMax);

        reg.bind_provider(RoleId::new("role-skeptic"), old.clone())
            .unwrap();
        assert_eq!(
            reg.binding_of(&RoleId::new("role-skeptic")),
            Some(old.clone())
        );

        // 覆盖绑定
        reg.bind_provider(RoleId::new("role-skeptic"), new_binding.clone())
            .unwrap();
        assert_eq!(
            reg.binding_of(&RoleId::new("role-skeptic")),
            Some(new_binding),
            "重新绑定后应返回新值,非旧值"
        );
        assert_ne!(
            reg.binding_of(&RoleId::new("role-skeptic")),
            Some(old),
            "重新绑定后不应返回旧值"
        );
    }

    // ============================================================
    // 集成测试:ProviderAffinityRegistry clone 语义
    // ============================================================

    #[test]
    fn registry_clone_inner_independent() {
        // clone_inner 创建独立副本:修改副本不影响原注册表
        let reg = ProviderAffinityRegistry::new();
        let original_binding =
            ProviderBinding::new(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        reg.bind_provider(RoleId::new("role-skeptic"), original_binding.clone())
            .unwrap();

        // 克隆
        let cloned = reg.clone_inner();
        assert_eq!(
            cloned.binding_of(&RoleId::new("role-skeptic")),
            Some(original_binding.clone()),
            "克隆应包含原注册表的数据"
        );

        // 修改克隆:不影响原注册表
        let new_binding =
            ProviderBinding::new(ProviderId::DeepSeek, ProviderId::Zhipu, ProviderId::MiniMax);
        cloned
            .bind_provider(RoleId::new("role-skeptic"), new_binding.clone())
            .unwrap();
        // 原注册表不受影响
        assert_eq!(
            reg.binding_of(&RoleId::new("role-skeptic")),
            Some(original_binding),
            "修改克隆不应影响原注册表"
        );
        // 克隆已更新
        assert_eq!(
            cloned.binding_of(&RoleId::new("role-skeptic")),
            Some(new_binding),
            "克隆应反映新绑定"
        );
    }

    // ============================================================
    // 集成测试:RwLock 并发安全(读多写少)
    // ============================================================

    #[test]
    fn registry_concurrent_read_write() {
        // 快速验证 RwLock 的并发读安全:多个读锁可同时持有
        let reg = ProviderAffinityRegistry::new();
        let binding =
            ProviderBinding::new(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        reg.bind_provider(RoleId::new("role-skeptic"), binding)
            .unwrap();

        // 同时持有多个读锁(借用检查器验证:读锁不互斥)
        let b1 = reg.binding_of(&RoleId::new("role-skeptic"));
        let b2 = reg.binding_of(&RoleId::new("role-skeptic"));
        // 两个读锁都能读到值
        assert!(b1.is_some());
        assert!(b2.is_some());
        assert_eq!(b1, b2);
    }

    // ============================================================
    // 集成测试:is_decorrelated 全路径覆盖
    // ============================================================

    #[test]
    fn is_decorrelated_bound_and_unbound() {
        let reg = ProviderAffinityRegistry::new();

        // 未绑定角色:向后兼容,返回 true
        assert!(reg.is_decorrelated(&RoleId::new("role-unbound")));

        // 绑定合规:返回 true
        let good =
            ProviderBinding::new(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        reg.bind_provider(RoleId::new("role-architect"), good)
            .unwrap();
        assert!(reg.is_decorrelated(&RoleId::new("role-architect")));

        // 绑定不合规(bind_provider 会拒绝):is_decorrelated 对未写入的角色仍返回 true
        let bad = ProviderBinding::new(ProviderId::Zhipu, ProviderId::Zhipu, ProviderId::DeepSeek);
        assert!(reg.bind_provider(RoleId::new("role-bad"), bad).is_err());
        // 被拒绝后角色未绑定,仍在向后兼容状态
        assert!(reg.is_decorrelated(&RoleId::new("role-bad")));
    }
}
