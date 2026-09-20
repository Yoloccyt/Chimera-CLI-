//! Router Config — 路由配置契约

use std::sync::Arc;

/// Router ID — 路由组件的唯一标识
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RouterId {
    /// OSA Coordinator
    OsCoordinator,
    
    /// KV Block Semantic Router
    Kvbsr,
    
    /// Function-as-Expert Router
    Faae,
    
    /// Sub-Expert Sparse Activation Router
    Sesa,
    
    /// GEA Activator
    Gea,
    
    /// Omega Learner
    OmegaLearner,
    
    /// 未知/自定义路由
    Custom(Arc<str>),
    
    /// 未初始化
    Unknown,
}

impl Default for RouterId {
    fn default() -> Self {
        RouterId::Unknown
    }
}

impl RouterId {
    /// 转换为字符串
    pub fn as_str(&self) -> &str {
        match self {
            RouterId::OsCoordinator => "osa-coordinator",
            RouterId::Kvbsr => "kvbsr-router",
            RouterId::Faae => "faae-router",
            RouterId::Sesa => "sesa-router",
            RouterId::Gea => "gea-activator",
            RouterId::OmegaLearner => "omega-learner",
            RouterId::Custom(s) => s,
            RouterId::Unknown => "unknown",
        }
    }
}

/// Router Config — 路由配置契约
/// 
/// ★ Insight: 将路由器的配置信息抽象为 trait，便于统一管理和 mock 测试。
pub trait RouterConfig: Send + Sync {
    /// 获取路由 ID
    fn router_id(&self) -> RouterId;
    
    /// 获取优先级权重（用于负载均衡）
    fn priority_weight(&self) -> f64 {
        1.0 // 默认权重
    }
}
