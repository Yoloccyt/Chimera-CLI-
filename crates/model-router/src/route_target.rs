//! 通道化路由目标 — MCA 路由决策单元升级(ADR-065 M3,§5.4)
//!
//! 对应架构层:L1 Core(model-router)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.4 路由亲和
//!
//! # 从"模型名"升级为三元组(§5.4)
//! 路由决策单元从单一模型名升级为 `RouteTarget = (ProviderId, model,
//! ThinkingProfile)`。这是 omega-learner LinUCB 臂空间(provider × model ×
//! thinking_mode ≈ 40 臂)与 SQLite 调用历史的统一键。
//!
//! # 最小变更策略(R3/R8 风险缓解)
//! **不动** 既有 `RoutingDecision`/`ModelInfo`/`history` 表(避免构造点雪崩与
//! 破坏性迁移)。`RouteTarget` 是**新增**类型,`RouteHistoryStore` 是**新建**
//! `route_history` 表(与既有 `history` 表并存),旧路由策略零改动。
//!
//! # 依赖方向(§2.2 铁律)
//! 本模块依赖 L0 `nexus_contracts::affinity`(L1 → L0 合规),不依赖 L10。

use nexus_contracts::affinity::{ProviderId, ThinkingPreference};
use serde::{Deserialize, Serialize};

/// 通道化路由目标 — provider × model × thinking_mode 三元组
///
/// 是 omega-learner 学习臂与 route_history 存储的统一键。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteTarget {
    /// 厂商标识
    pub provider: ProviderId,
    /// 模型名
    pub model: Box<str>,
    /// 思考偏好档位(TTG 三档)
    pub thinking: ThinkingPreference,
}

impl RouteTarget {
    /// 构造三元组
    pub fn new(
        provider: ProviderId,
        model: impl Into<Box<str>>,
        thinking: ThinkingPreference,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            thinking,
        }
    }

    /// 通道路由键 `provider/model`(与 mca-gateway 通道注册表键一致)
    pub fn route_key(&self) -> String {
        format!("{}/{}", self.provider.as_str(), self.model)
    }

    /// 学习臂标识 `provider/model/mode`(omega-learner s9 臂编码)
    ///
    /// WHY 三段编码: LinUCB 臂空间 = provider × model × thinking_mode;
    /// 字符串臂 ID 是 omega-learner `ArmId` 的既有设计(newtype String),
    /// 三元组升级零类型改动。
    pub fn arm_id(&self) -> String {
        format!(
            "{}/{}/{}",
            self.provider.as_str(),
            self.model,
            self.thinking.as_str()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_key_and_arm_id_encoding() {
        let target = RouteTarget::new(ProviderId::Zhipu, "glm-5.2", ThinkingPreference::Deep);
        assert_eq!(target.route_key(), "zhipu/glm-5.2");
        assert_eq!(target.arm_id(), "zhipu/glm-5.2/deep");
    }

    #[test]
    fn custom_provider_route_key() {
        let target = RouteTarget::new(
            ProviderId::Custom("openrouter".into()),
            "anthropic/claude-x",
            ThinkingPreference::Standard,
        );
        assert_eq!(target.route_key(), "openrouter/anthropic/claude-x");
        assert_eq!(target.arm_id(), "openrouter/anthropic/claude-x/standard");
    }

    #[test]
    fn route_target_serde_roundtrip() {
        let target = RouteTarget::new(
            ProviderId::MiniMax,
            "MiniMax-M3",
            ThinkingPreference::Standard,
        );
        let bytes = rmp_serde::to_vec(&target).unwrap();
        let back: RouteTarget = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(target, back);
    }

    #[test]
    fn route_target_hash_eq_for_map_key() {
        // 三元组作 HashMap 键:provider/model/thinking 全等才相等
        let a = RouteTarget::new(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ThinkingPreference::Fast,
        );
        let b = RouteTarget::new(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ThinkingPreference::Fast,
        );
        let c = RouteTarget::new(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ThinkingPreference::Deep,
        );
        assert_eq!(a, b);
        assert_ne!(a, c, "思考档位不同即不同臂");
    }
}
