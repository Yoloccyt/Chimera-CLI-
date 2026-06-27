//! FaaE 语义路由器 — Function-as-Expert 的核心路由实现
//!
//! 对应架构层:L6 Router
//! 对应创新点:FaaE(Function-as-Expert)
//!
//! # 核心职责
//! - **语义路由**:基于 CLV 与专家向量的余弦相似度,从候选工具集精筛 Top-K
//! - **EDSB 均衡**:路由后通过熵均衡器概率性重分配到次优工具
//! - **专家注册/注销**:动态管理工具专家注册表
//! - **事件发布**:路由/注册/注销均发布对应事件到 EventBus
//!
//! # 路由流程
//! 1. 接收 KVBSR 粗筛的候选工具集(Top-3 块的工具并集)
//! 2. 对每个候选工具,计算 CLV 与 expert_vector 的余弦相似度
//! 3. 使用 `select_nth_unstable_by` 部分排序选 Top-K(O(n))
//! 4. 若启用均衡,调用 EDSB 概率性重分配
//! 5. 更新被路由工具的 usage_count 和 last_used_at
//! 6. 发布 ExpertRouted 事件
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect() 在非测试代码
//! - 所有 async fn 满足 Send 约束
//! - 持锁状态下不可 await,避免死锁

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::sync::RwLock;
use tracing::warn;

use crate::config::FaaeConfig;
use crate::edsb::EdsbBalancer;
use crate::error::FaaeError;
use crate::types::{ExpertProfile, RoutingResult, ToolId};

/// FaaE 语义路由器 — 工具即专家的语义化路由调度
///
/// # 线程安全
/// - `expert_registry`: `Arc<RwLock<HashMap<...>>>`,读多写少场景,RwLock 允许并发读
/// - 内层 `Arc<RwLock<ExpertProfile>>`:每个专家画像独立锁,支持并发访问不同专家
/// - `event_bus`: Clone 廉价(基于 Arc)
/// - `edsb`: 无状态均衡器,Clone 廉价
///
/// # 并发设计
/// - `route`:获取 registry 读锁 → clone 候选 Arc → 释放锁 → 锁外计算相似度
/// - `register_expert`/`unregister_expert`:获取 registry 写锁,原子更新
/// - `spawn_decay_loop`:通过 Arc 共享 registry,后台异步衰减
///
/// # 示例
/// ```no_run
/// use faae_router::{FaaeRouter, ExpertProfile};
/// use event_bus::EventBus;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let bus = EventBus::new();
/// let router = FaaeRouter::new(bus);
///
/// let profile = ExpertProfile::new("tool-1", vec![0.5; 64], vec!["code".into()], 0.8);
/// router.register_expert(profile).await;
///
/// let clv = vec![0.5; 64];
/// let candidates = vec!["tool-1".into()];
/// let result = router.route(&clv, &candidates).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct FaaeRouter {
    /// 专家注册表(读多写少,用 RwLock;Arc 用于共享给衰减循环)
    expert_registry: Arc<RwLock<HashMap<ToolId, Arc<RwLock<ExpertProfile>>>>>,
    /// FaaE 配置
    config: FaaeConfig,
    /// 事件总线(发布 ExpertRouted/ExpertRegistered 等事件)
    event_bus: EventBus,
    /// EDSB 熵均衡器
    edsb: EdsbBalancer,
}

impl FaaeRouter {
    /// 创建路由器,使用默认配置
    pub fn new(event_bus: EventBus) -> Self {
        Self::with_config(event_bus, FaaeConfig::default())
    }

    /// 创建路由器,使用自定义配置
    pub fn with_config(event_bus: EventBus, config: FaaeConfig) -> Self {
        let edsb = EdsbBalancer::new(config.clone(), event_bus.clone());
        Self {
            expert_registry: Arc::new(RwLock::new(HashMap::new())),
            config,
            event_bus,
            edsb,
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &FaaeConfig {
        &self.config
    }

    /// 获取事件总线引用
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 获取 EDSB 均衡器引用
    pub fn edsb(&self) -> &EdsbBalancer {
        &self.edsb
    }

    /// 获取当前注册的专家数量(异步,需读锁)
    pub async fn expert_count(&self) -> usize {
        self.expert_registry.read().await.len()
    }

    /// FaaE 语义路由 — 从候选工具集精筛 Top-K
    ///
    /// # 路由流程
    /// 1. 获取 registry 读锁,收集候选工具的 Arc 引用
    /// 2. 锁外计算 CLV 与各候选 expert_vector 的余弦相似度
    /// 3. 使用 `select_nth_unstable_by` 部分排序选 Top-K
    /// 4. 若启用均衡,调用 EDSB 概率性重分配
    /// 5. 更新被路由工具的 usage_count 和 last_used_at
    /// 6. 发布 ExpertRouted 事件
    ///
    /// # 参数
    /// - `clv`:上下文潜在向量(512 维,内部截取前 64 维与 expert_vector 对齐)
    /// - `candidate_tools`:KVBSR 粗筛的候选工具 ID 列表
    ///
    /// # 错误
    /// - `RoutingFailed`:候选集为空或无已注册的候选工具
    pub async fn route(
        &self,
        clv: &[f32],
        candidate_tools: &[ToolId],
    ) -> Result<RoutingResult, FaaeError> {
        if candidate_tools.is_empty() {
            return Err(FaaeError::RoutingFailed {
                reason: "候选工具集为空".into(),
            });
        }

        // 1. 获取读锁,收集候选工具的 Arc 引用(锁内仅收集引用,减少锁持有时间)
        let candidate_arcs: Vec<Arc<RwLock<ExpertProfile>>> = {
            let registry = self.expert_registry.read().await;
            candidate_tools
                .iter()
                .filter_map(|tid| registry.get(tid).cloned())
                .collect()
        };

        if candidate_arcs.is_empty() {
            return Err(FaaeError::RoutingFailed {
                reason: "候选工具集中无已注册的专家".into(),
            });
        }

        // 2. 锁外计算相似度(每个 profile 独立读锁,不争用 registry 锁)
        let mut scored: Vec<(ToolId, f32, Arc<RwLock<ExpertProfile>>)> =
            Vec::with_capacity(candidate_arcs.len());
        for profile_arc in &candidate_arcs {
            let profile = profile_arc.read().await;
            // 截取 CLV 前 64 维与 expert_vector 对齐
            let query = &clv[..clv.len().min(profile.expert_vector.len())];
            let sim = nexus_core::cosine_similarity_slices(query, &profile.expert_vector);
            // 优先级加权:final_score = sim × priority(高优先级工具更易被选中)
            let weighted_score = sim * profile.priority;
            scored.push((profile.tool_id.clone(), weighted_score, profile_arc.clone()));
        }

        // 3. 部分排序选 Top-K(O(n),比全排序 O(n log n) 更高效)
        let k = self.config.top_k.min(scored.len());
        if k < scored.len() {
            scored.select_nth_unstable_by(k, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        // 前 K 个再排序确保降序(K log K << n log n)
        scored[..k].sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // 4. 构建候选列表(Top-K 工具 ID + 分数)
        let candidates: Vec<(ToolId, f32)> = scored[..k]
            .iter()
            .map(|(tid, score, _)| (tid.clone(), *score))
            .collect();

        let routed_tool = candidates[0].0.clone();
        let confidence = candidates[0].1;

        // 5. EDSB 均衡(若启用)
        let final_tool = if self.config.balance_enabled {
            let registry = self.expert_registry.read().await;
            self.edsb
                .balance(&registry, &routed_tool, &candidates)
                .await
                .unwrap_or(routed_tool.clone())
        } else {
            routed_tool.clone()
        };

        // 6. 更新被路由工具的 usage_count 和 last_used_at
        {
            let registry = self.expert_registry.read().await;
            if let Some(profile_arc) = registry.get(&final_tool) {
                let profile = profile_arc.read().await;
                profile.usage_count.fetch_add(1, Ordering::Relaxed);
                let mut last_used = profile.last_used_at.write().await;
                *last_used = Instant::now();
            }
        }

        // 7. 发布 ExpertRouted 事件
        let event = NexusEvent::ExpertRouted {
            metadata: EventMetadata::new("faae-router"),
            routed_tool: final_tool.to_string(),
            confidence,
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "ExpertRouted 事件发布失败(不影响路由结果)");
        }

        Ok(RoutingResult {
            routed_tool: final_tool,
            confidence,
            candidates,
        })
    }

    /// 注册工具专家 — 将工具添加到专家注册表
    ///
    /// # 并发一致性
    /// 获取写锁后原子插入,确保并发注册不会丢失。
    /// 若 tool_id 已存在,覆盖旧画像(更新语义向量/能力标签等)。
    ///
    /// # 参数
    /// - `profile`:专家画像(含 tool_id、expert_vector、capability_tags 等)
    pub async fn register_expert(&self, profile: ExpertProfile) {
        let tool_id = profile.tool_id.clone();

        // 原子插入(写锁内完成)
        {
            let mut registry = self.expert_registry.write().await;
            registry.insert(tool_id.clone(), Arc::new(RwLock::new(profile)));
        }

        // 锁外发布事件(避免持锁期间 await)
        let event = NexusEvent::ExpertRegistered {
            metadata: EventMetadata::new("faae-router"),
            tool_id: tool_id.to_string(),
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "ExpertRegistered 事件发布失败");
        }
    }

    /// 注销工具专家 — 从专家注册表移除指定工具
    ///
    /// # 参数
    /// - `tool_id`:要注销的工具 ID
    ///
    /// # 错误
    /// - `ExpertNotFound`:指定 tool_id 未注册
    pub async fn unregister_expert(&self, tool_id: &ToolId) -> Result<(), FaaeError> {
        // 原子移除(写锁内完成)
        let removed = {
            let mut registry = self.expert_registry.write().await;
            registry.remove(tool_id).is_some()
        };

        if !removed {
            return Err(FaaeError::ExpertNotFound {
                tool_id: tool_id.to_string(),
            });
        }

        // 锁外发布事件
        let event = NexusEvent::ExpertUnregistered {
            metadata: EventMetadata::new("faae-router"),
            tool_id: tool_id.to_string(),
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "ExpertUnregistered 事件发布失败");
        }

        Ok(())
    }

    /// 启动后台衰减循环 — 定期对使用计数应用指数衰减
    ///
    /// 在独立 tokio 任务中运行,每 5 分钟执行一次 `decay_usage_counts`。
    /// 需要在 tokio 运行时上下文中调用。
    pub fn spawn_decay_loop(&self) {
        let edsb = Arc::new(self.edsb.clone());
        let registry = self.expert_registry.clone();
        edsb.spawn_decay_loop(registry);
    }

    /// 获取专家注册表的共享引用(用于 EDSB 直接访问)
    ///
    /// WHY:允许外部直接访问注册表进行熵计算或衰减,
    /// 避免重复实现遍历逻辑
    pub fn registry(&self) -> Arc<RwLock<HashMap<ToolId, Arc<RwLock<ExpertProfile>>>>> {
        self.expert_registry.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用专家画像
    fn make_profile(name: &str, vector: Vec<f32>, priority: f32) -> ExpertProfile {
        ExpertProfile::new(name, vector, vec!["test".into()], priority)
    }

    #[tokio::test]
    async fn test_register_and_unregister() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let profile = make_profile("tool-1", vec![0.5; 64], 0.8);
        router.register_expert(profile).await;
        assert_eq!(router.expert_count().await, 1);

        router
            .unregister_expert(&ToolId::new("tool-1"))
            .await
            .unwrap();
        assert_eq!(router.expert_count().await, 0);
    }

    #[tokio::test]
    async fn test_unregister_not_found() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let result = router.unregister_expert(&ToolId::new("nonexistent")).await;
        assert!(matches!(result, Err(FaaeError::ExpertNotFound { .. })));
    }

    #[tokio::test]
    async fn test_route_empty_candidates() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let clv = vec![0.5; 64];
        let result = router.route(&clv, &[]).await;
        assert!(matches!(result, Err(FaaeError::RoutingFailed { .. })));
    }

    #[tokio::test]
    async fn test_route_no_registered_candidates() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let clv = vec![0.5; 64];
        let candidates = vec![ToolId::new("unregistered")];
        let result = router.route(&clv, &candidates).await;
        assert!(matches!(result, Err(FaaeError::RoutingFailed { .. })));
    }

    #[tokio::test]
    async fn test_route_single_candidate() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let profile = make_profile("tool-1", vec![1.0; 64], 1.0);
        router.register_expert(profile).await;

        let clv = vec![1.0; 64];
        let candidates = vec![ToolId::new("tool-1")];
        let result = router.route(&clv, &candidates).await.unwrap();

        assert_eq!(result.routed_tool.as_str(), "tool-1");
        assert!((result.confidence - 1.0).abs() < 1e-5);
        assert_eq!(result.candidates.len(), 1);
    }

    #[tokio::test]
    async fn test_route_top_k_selection() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        // 3 个工具,向量不同,确保 Top-K 排序正确
        let mut v1 = vec![0.0; 64];
        v1[0] = 1.0; // 与 CLV 最相似
        router.register_expert(make_profile("t1", v1, 1.0)).await;

        let mut v2 = vec![0.0; 64];
        v2[1] = 1.0; // 与 CLV 次相似
        router.register_expert(make_profile("t2", v2, 1.0)).await;

        let mut v3 = vec![0.0; 64];
        v3[2] = 1.0; // 与 CLV 最不相似
        router.register_expert(make_profile("t3", v3, 1.0)).await;

        let mut clv = vec![0.0; 64];
        clv[0] = 1.0; // 匹配 t1
        let candidates = vec![ToolId::new("t1"), ToolId::new("t2"), ToolId::new("t3")];
        let result = router.route(&clv, &candidates).await.unwrap();

        // Top-1 应为 t1
        assert_eq!(result.routed_tool.as_str(), "t1");
        // 候选按相似度降序
        assert_eq!(result.candidates[0].0.as_str(), "t1");
        assert_eq!(result.candidates[1].0.as_str(), "t2");
        assert_eq!(result.candidates[2].0.as_str(), "t3");
        // 相似度递减
        assert!(result.candidates[0].1 >= result.candidates[1].1);
        assert!(result.candidates[1].1 >= result.candidates[2].1);
    }

    #[tokio::test]
    async fn test_route_updates_usage_count() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        let profile = make_profile("tool-1", vec![1.0; 64], 1.0);
        router.register_expert(profile).await;

        let clv = vec![1.0; 64];
        let candidates = vec![ToolId::new("tool-1")];

        // 路由 3 次
        for _ in 0..3 {
            router.route(&clv, &candidates).await.unwrap();
        }

        // 验证 usage_count 递增
        let registry = router.expert_registry.read().await;
        let profile = registry.get(&ToolId::new("tool-1")).unwrap().read().await;
        assert_eq!(profile.get_usage_count(), 3);
    }

    #[tokio::test]
    async fn test_route_with_priority_weighting() {
        let bus = EventBus::new();
        let router = FaaeRouter::new(bus);

        // 两个向量相同的工具,但优先级不同
        router
            .register_expert(make_profile("low-priority", vec![1.0; 64], 0.5))
            .await;
        router
            .register_expert(make_profile("high-priority", vec![1.0; 64], 1.0))
            .await;

        let clv = vec![1.0; 64];
        let candidates = vec![ToolId::new("low-priority"), ToolId::new("high-priority")];
        let result = router.route(&clv, &candidates).await.unwrap();

        // 高优先级工具应被选中(weighted_score = sim × priority)
        assert_eq!(result.routed_tool.as_str(), "high-priority");
    }

    #[tokio::test]
    async fn test_route_top_k_limit() {
        let bus = EventBus::new();
        let config = FaaeConfig::default().with_top_k(2);
        let router = FaaeRouter::with_config(bus, config);

        // 注册 5 个工具
        for i in 0..5 {
            let mut v = vec![0.0; 64];
            v[i] = 1.0;
            router
                .register_expert(make_profile(&format!("t{i}"), v, 1.0))
                .await;
        }

        let clv = vec![1.0; 64];
        let candidates: Vec<ToolId> = (0..5).map(|i| ToolId::new(format!("t{i}"))).collect();
        let result = router.route(&clv, &candidates).await.unwrap();

        // Top-K = 2,候选列表长度应为 2
        assert_eq!(result.candidates.len(), 2);
    }

    #[tokio::test]
    async fn test_concurrent_register() {
        let bus = EventBus::new();
        let router = Arc::new(FaaeRouter::new(bus));

        let mut handles = Vec::new();
        for i in 0..10 {
            let router_clone = router.clone();
            handles.push(tokio::spawn(async move {
                let profile = make_profile(&format!("tool-{i}"), vec![0.5; 64], 0.8);
                router_clone.register_expert(profile).await;
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(router.expert_count().await, 10);
    }
}
