//! 角色注册表 — 5 角色对抗性议会的角色管理
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)
//!
//! # 设计决策(WHY)
//! - `RwLock<HashMap>`:读多写少场景,读锁并发无阻塞(§4.2 模块组织模式)
//! - Skeptic 独占否决权(`can_veto = true`):红队视角的风险否决是 AHIRT 核心防线
//! - 5 角色默认注册:权重和为 1.0(归一化),保证加权赞成率 ∈ [0.0, 1.0]
//! - 角色注册后发布 `RoleRegistered` 事件(通过 EventBus);
//!   无 EventBus 连接时回退到 tracing 日志(测试场景)

use std::collections::HashMap;
use std::sync::RwLock;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::info;

use crate::config::ParliamentConfig;
use crate::error::ParliamentError;
use crate::types::{Role, RoleId, RoleProfile};

/// 角色注册表 — 维护 5 角色画像,支持动态注册与查询
///
/// WHY RwLock:角色注册后极少变更(读多写少),读锁可并发无阻塞,
/// 写锁仅在动态注册新角色时短暂持有。HashMap 提供 O(1) 查询。
///
/// # 线程安全
/// `RwLock<HashMap<RoleId, RoleProfile>>` 保证并发读写安全,
/// `RoleRegistry` 本身为 `Send + Sync`,可跨线程共享(通过 `&self` 调用)。
pub struct RoleRegistry {
    /// 角色注册表(读多写少,用 RwLock)
    registry: RwLock<HashMap<RoleId, RoleProfile>>,
    /// 可选的 EventBus 连接(用于发布 RoleRegistered 事件)
    /// WHY Option:保留 new() 向后兼容(测试场景无 bus),生产场景用 with_event_bus 注入
    event_bus: Option<EventBus>,
}

impl RoleRegistry {
    /// 创建新的角色注册表,初始化 5 角色默认配置
    ///
    /// 5 角色默认权重(Architect 0.25 / Skeptic 0.30 / Optimizer 0.20 /
    /// Librarian 0.15 / Bard 0.10),和为 1.0。Skeptic 独占否决权。
    pub fn new(config: &ParliamentConfig) -> Self {
        let mut registry = HashMap::new();

        // 5 角色默认配置(权重取自 ParliamentConfig,保证一致性)
        let default_roles = [
            RoleProfile::new(
                "role-architect",
                Role::Architect,
                "系统架构与依赖分析",
                "claude-3-opus",
                config.architect_weight,
                false,
            ),
            RoleProfile::new(
                "role-skeptic",
                Role::Skeptic,
                "风险审查与红队对抗",
                "claude-3-opus",
                config.skeptic_weight,
                true, // WHY: Skeptic 独占否决权,红队防线
            ),
            RoleProfile::new(
                "role-optimizer",
                Role::Optimizer,
                "性能优化与资源分析",
                "gpt-4-turbo",
                config.optimizer_weight,
                false,
            ),
            RoleProfile::new(
                "role-librarian",
                Role::Librarian,
                "知识检索与历史先例",
                "claude-3-sonnet",
                config.librarian_weight,
                false,
            ),
            RoleProfile::new(
                "role-bard",
                Role::Bard,
                "创意发散与用户体验",
                "claude-3-haiku",
                config.bard_weight,
                false,
            ),
        ];

        for profile in default_roles {
            registry.insert(profile.role_id.clone(), profile);
        }

        Self {
            registry: RwLock::new(registry),
            event_bus: None,
        }
    }

    /// 构造角色注册表并注入 EventBus(生产场景使用)
    ///
    /// 与 new() 的区别:持有 EventBus 引用,register() 成功后发布 RoleRegistered 事件。
    /// WHY 与 SSRA/GSOE/DECB/LSCT/CHTC 的 with_event_bus 模式一致:保持构造器 API 一致性。
    pub fn with_event_bus(config: &ParliamentConfig, bus: EventBus) -> Self {
        let mut registry = Self::new(config);
        registry.event_bus = Some(bus);
        registry
    }

    /// 动态注册新角色
    ///
    /// 若角色 ID 已存在,覆盖旧画像。注册后记录 `RoleRegistered` 日志。
    ///
    /// # 错误
    /// - `ConfigError`:角色权重非法(负数或 NaN)
    pub fn register(&self, profile: RoleProfile) -> Result<(), ParliamentError> {
        // 校验权重合法性(防御外部传入)
        if profile.voting_weight < 0.0 || profile.voting_weight.is_nan() {
            return Err(ParliamentError::ConfigError {
                detail: format!(
                    "invalid voting_weight {} for role {}",
                    profile.voting_weight, profile.role_id
                ),
            });
        }

        let role_id = profile.role_id.clone();
        let role_name = profile.role.to_string();
        // WHY 提前捕获 voting_weight:profile 在下方 insert 时被消费(move),
        // 而 RoleRegistered 事件需要该字段。role_id/role_name 同理。
        let voting_weight = profile.voting_weight;

        let mut registry = self
            .registry
            .write()
            .map_err(|e| ParliamentError::ConfigError {
                detail: format!("registry lock poisoned: {e}"),
            })?;
        registry.insert(role_id.clone(), profile);

        // 发布 RoleRegistered 事件(若有 EventBus 连接)
        // WHY publish_blocking:register() 是同步方法,不持有 async runtime;
        // publish_blocking 是 event-bus 官方同步 API,与 CMT/DECB 同步发布模式一致。
        // 事件发布失败不阻塞注册流程(角色已写入注册表),仅记录警告。
        if let Some(ref bus) = self.event_bus {
            let event = NexusEvent::RoleRegistered {
                metadata: EventMetadata::new("parliament"),
                role_id: role_id.as_str().to_string(),
                role_name,
                voting_weight,
            };
            if let Err(e) = bus.publish_blocking(event) {
                tracing::warn!(error = %e, role_id = %role_id, "RoleRegistered 事件发布失败");
            }
        } else {
            // 无 EventBus 时保留 tracing 日志(测试场景)
            info!(role_id = %role_id, role = %role_name, "角色注册完成 (RoleRegistered, 无 EventBus)");
        }

        Ok(())
    }

    /// 按 RoleId 查询角色画像
    pub fn get(&self, role_id: &RoleId) -> Option<RoleProfile> {
        let registry = self.registry.read().ok()?;
        registry.get(role_id).cloned()
    }

    /// 按 Role 枚举查询角色画像
    ///
    /// WHY 遍历而非索引:registry 以 RoleId 为 key,
    /// 按 Role 枚举查询需遍历,5 角色规模下 O(5) 可接受
    pub fn get_by_role(&self, role: Role) -> Option<RoleProfile> {
        let registry = self.registry.read().ok()?;
        registry.values().find(|p| p.role == role).cloned()
    }

    /// 返回所有已注册角色画像列表
    pub fn all_roles(&self) -> Vec<RoleProfile> {
        let registry = match self.registry.read() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        registry.values().cloned().collect()
    }

    /// 返回已注册角色数量
    pub fn count(&self) -> usize {
        self.registry.read().map(|r| r.len()).unwrap_or(0)
    }

    /// 计算所有角色权重总和(用于校验归一化)
    pub fn weights_sum(&self) -> f32 {
        let registry = match self.registry.read() {
            Ok(r) => r,
            Err(_) => return 0.0,
        };
        registry.values().map(|p| p.voting_weight).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry() -> RoleRegistry {
        RoleRegistry::new(&ParliamentConfig::default())
    }

    #[test]
    fn test_default_five_roles_registered() {
        let registry = make_registry();
        assert_eq!(registry.count(), 5);

        // 验证 5 角色均存在
        assert!(registry.get_by_role(Role::Architect).is_some());
        assert!(registry.get_by_role(Role::Skeptic).is_some());
        assert!(registry.get_by_role(Role::Optimizer).is_some());
        assert!(registry.get_by_role(Role::Librarian).is_some());
        assert!(registry.get_by_role(Role::Bard).is_some());
    }

    #[test]
    fn test_skeptic_has_veto_others_not() {
        let registry = make_registry();

        let skeptic = registry.get_by_role(Role::Skeptic).unwrap();
        assert!(skeptic.can_veto, "Skeptic 应拥有否决权");

        // 其余角色无否决权
        for role in [
            Role::Architect,
            Role::Optimizer,
            Role::Librarian,
            Role::Bard,
        ] {
            let profile = registry.get_by_role(role).unwrap();
            assert!(!profile.can_veto, "{role} 不应拥有否决权");
        }
    }

    #[test]
    fn test_weights_sum_to_one() {
        let registry = make_registry();
        let sum = registry.weights_sum();
        assert!((sum - 1.0).abs() < 1e-6, "权重总和应为 1.0,实际: {sum}");
    }

    #[test]
    fn test_default_weights_match_config() {
        let config = ParliamentConfig::default();
        let registry = make_registry();

        let architect = registry.get_by_role(Role::Architect).unwrap();
        assert!((architect.voting_weight - config.architect_weight).abs() < 1e-6);

        let skeptic = registry.get_by_role(Role::Skeptic).unwrap();
        assert!((skeptic.voting_weight - config.skeptic_weight).abs() < 1e-6);

        let optimizer = registry.get_by_role(Role::Optimizer).unwrap();
        assert!((optimizer.voting_weight - config.optimizer_weight).abs() < 1e-6);

        let librarian = registry.get_by_role(Role::Librarian).unwrap();
        assert!((librarian.voting_weight - config.librarian_weight).abs() < 1e-6);

        let bard = registry.get_by_role(Role::Bard).unwrap();
        assert!((bard.voting_weight - config.bard_weight).abs() < 1e-6);
    }

    #[test]
    fn test_get_by_role_id() {
        let registry = make_registry();

        let profile = registry.get(&RoleId::new("role-architect"));
        assert!(profile.is_some());
        assert_eq!(profile.unwrap().role, Role::Architect);

        // 不存在的 ID
        assert!(registry.get(&RoleId::new("nonexistent")).is_none());
    }

    #[test]
    fn test_dynamic_register_new_role() {
        let registry = make_registry();
        assert_eq!(registry.count(), 5);

        // 动态注册新角色(覆盖现有 ID)
        let new_profile = RoleProfile::new(
            "role-custom",
            Role::Architect,
            "自定义架构师",
            "custom-model",
            0.05,
            false,
        );
        registry.register(new_profile).unwrap();
        assert_eq!(registry.count(), 6);

        // 验证可查询
        assert!(registry.get(&RoleId::new("role-custom")).is_some());
    }

    #[test]
    fn test_register_overwrites_existing() {
        let registry = make_registry();
        assert_eq!(registry.count(), 5);

        // 覆盖现有角色
        let updated = RoleProfile::new(
            "role-architect",
            Role::Architect,
            "更新后的架构师",
            "new-model",
            0.30,
            false,
        );
        registry.register(updated).unwrap();

        // 数量不变(覆盖)
        assert_eq!(registry.count(), 5);

        // 验证内容已更新
        let profile = registry.get(&RoleId::new("role-architect")).unwrap();
        assert_eq!(profile.specialty, "更新后的架构师");
        assert!((profile.voting_weight - 0.30).abs() < 1e-6);
    }

    #[test]
    fn test_register_rejects_negative_weight() {
        let registry = make_registry();

        let bad_profile = RoleProfile::new("role-bad", Role::Bard, "负权重", "model", -0.1, false);
        let result = registry.register(bad_profile);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ParliamentError::ConfigError { .. }
        ));
    }

    #[test]
    fn test_register_rejects_nan_weight() {
        let registry = make_registry();

        let bad_profile =
            RoleProfile::new("role-nan", Role::Bard, "NaN 权重", "model", f32::NAN, false);
        let result = registry.register(bad_profile);
        assert!(result.is_err());
    }

    #[test]
    fn test_all_roles_returns_all() {
        let registry = make_registry();
        let all = registry.all_roles();
        assert_eq!(all.len(), 5);

        // 验证包含所有角色
        let roles: Vec<Role> = all.iter().map(|p| p.role).collect();
        for role in Role::all() {
            assert!(roles.contains(&role), "缺少角色: {role}");
        }
    }

    #[test]
    fn test_custom_config_weights() {
        // 自定义权重配置(和仍为 1.0)
        let config = ParliamentConfig {
            architect_weight: 0.30,
            skeptic_weight: 0.25,
            optimizer_weight: 0.20,
            librarian_weight: 0.15,
            bard_weight: 0.10,
            ..Default::default()
        };
        let registry = RoleRegistry::new(&config);

        let architect = registry.get_by_role(Role::Architect).unwrap();
        assert!((architect.voting_weight - 0.30).abs() < 1e-6);

        let skeptic = registry.get_by_role(Role::Skeptic).unwrap();
        assert!((skeptic.voting_weight - 0.25).abs() < 1e-6);
    }

    /// 验证 with_event_bus 构造器 + register() 发布 RoleRegistered 事件
    ///
    /// 覆盖点:
    /// - with_event_bus 注入的 EventBus 被 register() 正确使用
    /// - RoleRegistered 事件字段(metadata.source / role_id / role_name / voting_weight)正确
    /// - WHY 先 subscribe:broadcast 不缓存历史,subscribe 必须在 publish 前
    #[test]
    fn test_register_publishes_role_registered_event() {
        let bus = EventBus::new();
        // 先订阅,再注入 bus(注入会 move bus,但 subscribe 借用 &bus 已完成)
        let mut receiver = bus.subscribe();

        let config = ParliamentConfig::default();
        let registry = RoleRegistry::with_event_bus(&config, bus);

        // 注册新角色(覆盖 default 5 角色之外的 ID,避免与 new() 的注册混淆)
        let profile = RoleProfile::new(
            "role-test-custom",
            Role::Architect,
            "测试角色",
            "test-model",
            0.5,
            false,
        );
        let result = registry.register(profile);
        assert!(result.is_ok(), "register 应成功");

        // 验证 RoleRegistered 事件已发布并字段正确
        let event = receiver
            .try_recv()
            .expect("try_recv 不应返回通道错误")
            .expect("应收到 RoleRegistered 事件");
        match event {
            NexusEvent::RoleRegistered {
                role_id,
                role_name,
                voting_weight,
                metadata,
            } => {
                assert_eq!(role_id, "role-test-custom");
                assert_eq!(role_name, "architect");
                assert!((voting_weight - 0.5).abs() < 1e-6);
                assert_eq!(metadata.source, "parliament");
            }
            other => panic!("预期 RoleRegistered 事件,实际: {other:?}"),
        }
    }

    /// 验证无 EventBus 时 register() 仍正常工作(向后兼容)
    ///
    /// 覆盖点:new() 构造的 registry 无 event_bus,register() 走 tracing 日志分支,
    /// 不应 panic 也不应返回错误。
    #[test]
    fn test_register_without_event_bus_backward_compatible() {
        let config = ParliamentConfig::default();
        let registry = RoleRegistry::new(&config);

        let profile = RoleProfile::new(
            "role-test-no-bus",
            Role::Architect,
            "测试角色",
            "test-model",
            0.5,
            false,
        );
        let result = registry.register(profile);
        assert!(result.is_ok(), "无 EventBus 时 register 也应成功");

        // 验证角色确实写入注册表
        assert!(registry.get(&RoleId::new("role-test-no-bus")).is_some());
    }
}
