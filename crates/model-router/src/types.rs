//! 路由器核心类型 — 模型信息、路由策略、请求与决策
//!
//! 对应架构:L1 Core,被 L9 Quest Engine 等上层 crate 依赖
//!
//! # 类型职责
//! - `ModelInfo`:模型元信息(成本、延迟、上下文容量、质量评分)
//! - `RoutingStrategy`:三种路由策略(Lite/Efficient/Auto)
//! - `RoutingRequest`:路由请求,携带意图、预估 token 数与策略
//! - `RoutingDecision`:路由决策结果,含选中模型、原因、预估成本与候选列表

use serde::{Deserialize, Serialize};

/// 模型信息 — 描述一个可路由的底层模型
///
/// `quality_score` 在 Week 2 阶段为静态值,Week 6 后由 GSOE 在线进化动态更新。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    /// 模型唯一标识(如 "lite-model"、"gpt-4o")
    pub model_id: String,
    /// 模型提供方(如 "openai"、"anthropic"、"local")
    pub provider: String,
    /// 每千 token 成本(美元),用于 Lite 策略与成本预估
    pub cost_per_1k_tokens: f64,
    /// 平均延迟(毫秒),用于 Efficient 策略
    pub avg_latency_ms: u64,
    /// 最大上下文长度(token 数),用于过滤不匹配的模型
    pub max_context: u32,
    /// 质量评分 [0.0, 1.0],Week 2 阶段为静态值,Week 6 后由 GSOE 动态更新
    pub quality_score: f32,
}

/// 路由策略 — 决定如何从候选模型中选择执行模型
///
/// WHY:三策略对应不同任务场景:
/// - `Lite`:简单任务(如查询、格式化),优先成本最低
/// - `Efficient`:实时场景(如交互式对话),优先延迟最低
/// - `Auto`:综合最优,平衡成本、延迟与质量
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RoutingStrategy {
    /// 轻量级:选择成本最低的模型
    Lite,
    /// 效率优先:选择延迟最低的模型
    Efficient,
    /// 自动:加权评分选择综合最优
    Auto,
}

/// 路由请求 — 由 Quest Engine 在分解任务后发起
#[derive(Debug, Clone)]
pub struct RoutingRequest {
    /// 所属 Quest ID,用于事件追踪
    pub quest_id: String,
    /// 用户意图(含风险等级,影响模型选择)
    pub intent: nexus_core::UserIntent,
    /// 预估 token 数(输入 + 输出),用于成本预估
    pub estimated_tokens: u32,
    /// 路由策略
    pub strategy: RoutingStrategy,
}

/// 路由决策 — 路由器返回的选择结果
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingDecision {
    /// 选中的模型 ID
    pub model_id: String,
    /// 路由原因(人类可读,用于审计与事件)
    pub route_reason: String,
    /// 预估成本(美分,1 美元 = 100 美分)
    pub estimated_cost: u64,
    /// 候选模型列表(按策略优先级降序排序)
    pub candidates: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model() -> ModelInfo {
        ModelInfo {
            model_id: "test-model".into(),
            provider: "local".into(),
            cost_per_1k_tokens: 0.001,
            avg_latency_ms: 200,
            max_context: 8192,
            quality_score: 0.75,
        }
    }

    #[test]
    fn test_model_info_serde() {
        let model = make_model();
        let json = serde_json::to_string(&model).unwrap();
        let de: ModelInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(de, model);
    }

    #[test]
    fn test_routing_strategy_serde() {
        let s = RoutingStrategy::Auto;
        let json = serde_json::to_string(&s).unwrap();
        let de: RoutingStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(de, s);
    }

    #[test]
    fn test_routing_strategy_equality() {
        assert_eq!(RoutingStrategy::Lite, RoutingStrategy::Lite);
        assert_ne!(RoutingStrategy::Lite, RoutingStrategy::Efficient);
        assert_ne!(RoutingStrategy::Efficient, RoutingStrategy::Auto);
    }
}
