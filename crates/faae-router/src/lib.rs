//! Function-as-Expert 语义路由 — 工具即专家的语义化路由调度
//!
//! 对应架构层:L6 Router
//! 对应创新点:FaaE(Function-as-Expert)+ EDSB(Entropy-Driven Self-Balancing)
//!
//! # 核心职责
//! - **FaaE 语义路由**:基于 CLV(上下文潜在向量)与专家向量的余弦相似度,
//!   从 KVBSR 粗筛的候选工具集中精筛 Top-K 工具
//! - **EDSB 熵均衡**:通过香农熵度量负载分布,当熵值低于阈值时,
//!   以 `p = 1 - entropy` 的概率将请求重分配到次优工具
//! - **指数衰减**:定期对使用计数应用指数衰减,近期使用权重更高
//! - **专家注册/注销**:动态管理工具专家注册表
//!
//! # FaaE 与 KVBSR 的关系
//! FaaE 作为 KVBSR 的"精筛"层:
//! 1. KVBSR 粗筛:从全量工具中选 Top-3 块(覆盖约 60-90 工具)
//! 2. FaaE 精筛:从候选工具集中按语义相似度选 Top-8 工具
//!
//! # EDSB 均衡策略
//! - 香农熵 `H = -Σ(p_i × ln(p_i)) / ln(n)`,归一化到 [0, 1]
//! - 熵 < 0.6 时触发均衡,概率 `p = 1 - entropy` 重分配到次优工具
//! - 不强制均衡:概率性折中语义准确性与负载均衡
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect() 在非测试代码
//! - 所有 async fn 满足 Send 约束
//! - 持锁状态下不可 await,避免死锁
//!
//! # 快速示例
//! ```no_run
//! use faae_router::{FaaeRouter, FaaeConfig, ExpertProfile};
//! use event_bus::EventBus;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = EventBus::new();
//! let router = FaaeRouter::new(bus);
//!
//! let profile = ExpertProfile::new("tool-1", vec![0.5; 64], vec!["code".into()], 0.8);
//! router.register_expert(profile).await;
//!
//! let clv = vec![0.5; 64];
//! let candidates = vec!["tool-1".into()];
//! let result = router.route(&clv, &candidates).await?;
//! println!("路由到 {}, 置信度 {:.2}", result.routed_tool, result.confidence);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod config;
pub mod edsb;
pub mod error;
pub mod expert;
pub mod router;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::FaaeConfig;
pub use edsb::EdsbBalancer;
pub use error::FaaeError;
pub use router::FaaeRouter;
pub use types::{EntropyStats, ExpertProfile, ExpertProfileSnapshot, RoutingResult, ToolId};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::FaaeConfig;
    pub use crate::edsb::EdsbBalancer;
    pub use crate::error::FaaeError;
    pub use crate::router::FaaeRouter;
    pub use crate::types::{
        EntropyStats, ExpertProfile, ExpertProfileSnapshot, RoutingResult, ToolId,
    };
}
