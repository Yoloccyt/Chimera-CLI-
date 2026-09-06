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
pub mod card_feedback;
pub mod config;
pub mod edsb;
pub mod error;
pub mod expert;
/// Phase 6 §11.2: 算子路由器（OpenMLE Greedy/ThreeFactor/UCB/Cooling，ADR-049 内嵌；
/// L6→L5 向下依赖 gsoe-evolution 四算子 + ThreeFactorSelector Softmax 委托 D-3）
pub mod operator_router;
/// P1-T14: 批量专家评分的 ComputeBridge 并行注入（WI-34 首批注入）
pub mod parallel;
/// Phase 6 §11.4(W4): 三因子父本选择 L6 消费适配器（§16.4 边界裁决，ADR-084 决策 6）
pub mod parent_context;
pub mod router;
/// P3-T6: TSR×MoE 收口（WI-09 尾）— 任务类型×成功率矩阵 + MCP 同平面注册面 + 误剪救回率埋点
pub mod routing_history;
/// P2-T12: TSR×MoE 稀疏路由偏置均衡（v4.0 WI-09,aux-loss-free + top-k 6~8）
pub mod tsr_moe;
pub mod types;
/// Phase 10 Wave 4: §16.4 变体/父本事件消费订阅器（VariantApproved + ParentSelected 接线）
pub mod variant_subscriber;

// === 关键类型重导出,简化外部导入 ===
pub use card_feedback::{spawn_card_feedback_loop, SharedOperatorRouter};
pub use config::FaaeConfig;
pub use edsb::EdsbBalancer;
pub use error::FaaeError;
// Phase 6 §11.2: 算子路由器公开 API 重导出（W4 含聚合表/轨迹导出/热切换）
pub use operator_router::{
    MemorySynthesizer, OperatorAggregate, OperatorRouter, OperatorSelectionRecord, HISTORY_CAP,
};
pub use parent_context::{ParentContextProvider, ParentSelection};
pub use router::FaaeRouter;
// P3-T6: TSR×MoE 收口公开 API（WI-09 尾）
pub use routing_history::{
    ExpertCatalog, ExpertKind, ExpertMeta, MissedRecoveryTracker, RoutingHistory,
};
pub use types::{EntropyStats, ExpertProfile, ExpertProfileSnapshot, RoutingResult, ToolId};
// Phase 10 Wave 4: §16.4 事件消费订阅器公开 API 重导出
pub use variant_subscriber::{
    spawn_variant_event_subscriber, ApprovedVariant, ApprovedVariantRegistry,
    ParentSelectionHistory, ParentSelectionRecord,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::card_feedback::{spawn_card_feedback_loop, SharedOperatorRouter};
    pub use crate::config::FaaeConfig;
    pub use crate::edsb::EdsbBalancer;
    pub use crate::error::FaaeError;
    pub use crate::operator_router::{OperatorRouter, OperatorSelectionRecord};
    pub use crate::parent_context::{ParentContextProvider, ParentSelection};
    pub use crate::router::FaaeRouter;
    pub use crate::types::{
        EntropyStats, ExpertProfile, ExpertProfileSnapshot, RoutingResult, ToolId,
    };
}
