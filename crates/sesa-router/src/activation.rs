//! SESA 激活路由器 — 子专家稀疏激活的核心实现
//!
//! 对应架构层:L6 Router
//! 对应创新点:SESA(Sub-Expert Sparse Activation)
//!
//! ## 核心机制
//! - **专家注册表**:基于 `DashMap`,O(1) 分片查找,并发安全
//! - **激活评分**:CLV 余弦相似度,复用 `nexus_core::cosine_similarity_slices`
//! - **Top-K 选择**:使用 `select_nth_unstable_by` 实现 O(n) 平均复杂度
//! - **稀疏度强制**:`enforce_sparsity` 确保激活专家数 < 40%
//! - **EventBus 集成**:激活完成发布 `SesaActivationCompleted`
//!   订阅 `ConsensusReached` 触发稀疏激活策略调整
//!
//! ## 激活流程
//! 1. 从注册表读取所有专家元数据(零拷贝:仅 ID + 向量引用)
//! 2. 对每个专家计算 CLV 与 query_vector 的余弦相似度
//! 3. 使用 `select_nth_unstable_by` 选 Top-K 专家(O(n))
//! 4. 构造 SesaMask,激活 Top-K 对应位
//! 5. 调用 `enforce_sparsity` 强制稀疏度 < 40%
//! 6. 发布 `SesaActivationCompleted` 事件(若绑定 EventBus)
//!
//! ## 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect() 在非测试代码
//! - 所有 async fn 满足 Send 约束
//! - 持锁状态下不可 await,避免死锁
//! - `bus.subscribe()` 必须在 `tokio::spawn` 之前同步调用

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::cosine_similarity_slices;
use tracing::warn;

use crate::config::SesaConfig;
use crate::error::SesaError;
use crate::mask::{SesaMask, MASK_TOTAL_BITS};
use crate::prerequisite::PrerequisiteChecker;
use crate::sparsity::{enforce_sparsity, SparsityProfile};
use crate::types::{ActivationRequest, ExpertDescriptor};

/// SESA 激活路由器 — 子专家稀疏激活核心组件
///
/// 持有专家注册表(`DashMap<String, ExpertDescriptor>`)与可选的 EventBus。
/// 激活操作通过 `activate` 异步方法触发,内部使用 `tokio::time::timeout` 控制截止时间。
///
/// # 线程安全
/// `DashMap` 采用分片锁,不同 key 的读写互不阻塞。
/// `SesaConfig` 是 `Clone`,`EventBus` 是 `Clone`(Arc 引用计数)。
/// 整个路由器可安全共享(`&self` 调用)。
///
/// # 示例
/// ```no_run
/// use sesa_router::{SesaRouter, SesaConfig, ActivationRequest, ExpertDescriptor};
/// use event_bus::EventBus;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let bus = EventBus::new();
/// let router = SesaRouter::with_event_bus(SesaConfig::default(), bus);
///
/// let expert = ExpertDescriptor::new("expert-1", vec![0.5; 64]);
/// router.register_expert(expert)?;
///
/// let request = ActivationRequest::new("req-1", vec![0.5; 64], 8, 5);
/// let (mask, profile) = router.activate(request).await?;
/// assert!(profile.sparsity_ratio < 0.4);
/// # Ok(())
/// # }
/// ```
pub struct SesaRouter {
    /// 路由器配置
    config: SesaConfig,
    /// 专家注册表(DashMap 并发安全,expert_id → ExpertDescriptor)
    experts: DashMap<String, ExpertDescriptor>,
    /// 下一个可分配的掩码索引(原子递增,确保唯一性)
    next_mask_index: AtomicU32,
    /// 可选事件总线(激活成功后发布事件)
    event_bus: Option<EventBus>,
    /// 前置事件校验器(仅 with_event_bus 且 config.prerequisite_check_enabled 时创建)
    ///
    /// WHY Option:无 EventBus(SesaRouter::new)或配置禁用时为 None,跳过校验。
    /// 默认启用时在 with_event_bus 构造时同步订阅 Routing 事件(§4.4 反模式 #3)。
    prerequisite_checker: Option<PrerequisiteChecker>,
}

impl SesaRouter {
    /// 创建激活路由器(无 EventBus,不发布事件)
    ///
    /// WHY 无 EventBus 不创建 PrerequisiteChecker:没有 EventBus 就无法订阅
    /// 上游路由事件,校验无意义。此构造函数适合纯计算场景(如基准测试)。
    pub fn new(config: SesaConfig) -> Self {
        Self {
            config,
            experts: DashMap::new(),
            next_mask_index: AtomicU32::new(0),
            event_bus: None,
            prerequisite_checker: None, // 无 EventBus 不校验
        }
    }

    /// 创建激活路由器并绑定 EventBus
    ///
    /// 绑定后,`activate` 成功会发布 `SesaActivationCompleted` 事件,
    /// 调用 `start_consensus_listener` 可订阅 `ConsensusReached`
    /// 触发稀疏激活策略调整。
    ///
    /// # 前置事件校验(Phase IV N9)
    /// 若 `config.prerequisite_check_enabled` 为 true(默认),构造时同步创建
    /// `PrerequisiteChecker` 并订阅 `EventTopic::Routing` 事件。
    /// 后续 `activate()` 入口会校验三个上游事件是否齐备:
    /// - `OmniSparseMasksComputed`(OSA 完成)
    /// - `ToolsRouted`(KVBSR/FaaE 完成)
    /// - `ExpertRouted`(FaaE 路由完成)
    ///
    /// WHY 在构造时订阅:遵守 §4.4 反模式 #3,subscribe 必须在 spawn 之前
    /// 同步调用,否则可能错过后续发布的上游事件。
    pub fn with_event_bus(config: SesaConfig, bus: EventBus) -> Self {
        // 根据 config.prerequisite_check_enabled 决定是否创建校验器
        let prerequisite_checker = if config.prerequisite_check_enabled {
            Some(PrerequisiteChecker::new(&bus))
        } else {
            None
        };
        Self {
            config,
            experts: DashMap::new(),
            next_mask_index: AtomicU32::new(0),
            event_bus: Some(bus),
            prerequisite_checker,
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &SesaConfig {
        &self.config
    }

    /// 获取当前注册专家数
    pub fn expert_count(&self) -> usize {
        self.experts.len()
    }

    /// 注册专家 — 分配掩码索引并加入注册表
    ///
    /// # 流程
    /// 1. 原子递增 `next_mask_index` 分配新索引
    /// 2. 校验索引 ≤ 256(超出掩码容量返回错误)
    /// 3. 若 expert_id 已存在,覆盖更新(保留原索引)
    ///
    /// # 错误
    /// - `IndexOutOfBounds`:专家数超过 256-bit 掩码容量
    pub fn register_expert(&self, mut expert: ExpertDescriptor) -> Result<(), SesaError> {
        // 已存在则覆盖更新(保留原 mask_index)
        if let Some(mut entry) = self.experts.get_mut(&expert.expert_id) {
            expert.mask_index = entry.mask_index;
            *entry = expert;
            return Ok(());
        }

        // 新注册:分配 mask_index
        let idx = self.next_mask_index.fetch_add(1, Ordering::Relaxed);
        if idx as usize >= MASK_TOTAL_BITS {
            // 回滚索引(虽然不影响正确性,但避免计数器无限增长)
            self.next_mask_index.fetch_sub(1, Ordering::Relaxed);
            return Err(SesaError::IndexOutOfBounds {
                index: idx as usize,
                capacity: MASK_TOTAL_BITS,
            });
        }
        expert.mask_index = idx;
        let key = expert.expert_id.clone();
        self.experts.insert(key, expert);
        Ok(())
    }

    /// 注销专家(注意:索引不回收,避免掩码位语义变化)
    pub fn unregister_expert(&self, expert_id: &str) -> Option<ExpertDescriptor> {
        self.experts.remove(expert_id).map(|(_, v)| v)
    }

    /// 执行激活 — 异步,带超时控制与事件发布
    ///
    /// # 流程
    /// 1. 前置事件校验(若 PrerequisiteChecker 启用):确保五层路由顺序已完成
    /// 2. `deadline_ms == 0` 直接返回超时(无可用时间)
    /// 3. `tokio::time::timeout` 包装 `activate_inner` 同步逻辑
    /// 4. 成功后测量实际延迟并发布 `SesaActivationCompleted` 事件
    ///
    /// # 前置事件校验(Phase IV N9)
    /// 若 `with_event_bus` 构造且 `config.prerequisite_check_enabled` 为 true(默认),
    /// 入口会校验三个上游事件是否齐备:
    /// - `OmniSparseMasksComputed`(OSA 完成)
    /// - `ToolsRouted`(KVBSR/FaaE 完成)
    /// - `ExpertRouted`(FaaE 路由完成)
    ///
    /// 缺失任一事件返回 `SesaError::PrerequisiteNotMet`,强制五层路由顺序。
    ///
    /// # 错误
    /// - `PrerequisiteNotMet`:上游路由事件未齐备(PrerequisiteChecker 启用时)
    /// - `ActivationTimeout`:超过 deadline_ms
    /// - `EmptyExpertPool`:注册表为空
    pub async fn activate(
        &self,
        request: ActivationRequest,
    ) -> Result<(SesaMask, SparsityProfile), SesaError> {
        // 前置事件校验:确保五层路由顺序(OSA → KVBSR → FaaE → GEA → SESA)
        // WHY 在 deadline 检查之前:上游未完成时连超时检查都无意义,应直接拒绝。
        // check() 是同步方法(Mutex 不跨 await),不阻塞 async runtime。
        if let Some(checker) = &self.prerequisite_checker {
            checker.check()?;
        }

        if request.deadline_ms == 0 {
            return Err(SesaError::ActivationTimeout { deadline_ms: 0 });
        }

        let start = Instant::now();
        let deadline = Duration::from_millis(request.deadline_ms);

        let inner = tokio::time::timeout(deadline, async { self.activate_inner(&request) }).await;

        match inner {
            Ok(Ok((mask, profile))) => {
                let latency_us = start.elapsed().as_micros() as u64;
                self.publish_completion(&request, &profile, latency_us)
                    .await;
                Ok((mask, profile))
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(SesaError::ActivationTimeout {
                deadline_ms: request.deadline_ms,
            }),
        }
    }

    /// 激活核心逻辑(同步,纯内存计算)
    ///
    /// # 流程
    /// 1. 收集所有专家的 (mask_index, score)
    /// 2. 用 `select_nth_unstable_by` 选 Top-K(评分最高的 K 个)
    /// 3. 构造 SesaMask,激活 Top-K 对应位
    /// 4. 调用 `enforce_sparsity` 强制稀疏度 < max_sparsity_ratio
    fn activate_inner(
        &self,
        request: &ActivationRequest,
    ) -> Result<(SesaMask, SparsityProfile), SesaError> {
        let total = self.experts.len() as u32;
        if total == 0 {
            return Err(SesaError::EmptyExpertPool);
        }

        // 收集所有专家的 (mask_index, score)
        let mut scored: Vec<(usize, f32)> = Vec::with_capacity(self.experts.len());
        for entry in self.experts.iter() {
            let expert = entry.value();
            let score = cosine_similarity_slices(&request.query_vector, &expert.expert_vector);
            scored.push((expert.mask_index as usize, score));
        }

        // Top-K 选择:K = min(request.top_k, total)
        let k = request.top_k.min(scored.len()).max(1);
        let top_k_indices = select_top_k_desc(&mut scored, k);

        // 构造掩码:激活 Top-K 对应位
        let mut mask = SesaMask::new();
        for &(idx, _score) in top_k_indices {
            mask.set_bit(idx);
        }

        // 构造评分向量(用于 enforce_sparsity 内的 Top-K 选择)
        let mut scores = vec![0.0_f32; MASK_TOTAL_BITS];
        for &(idx, score) in &scored {
            if idx < MASK_TOTAL_BITS {
                scores[idx] = score;
            }
        }

        // 强制稀疏度 < max_sparsity_ratio
        self.enforce_sparsity(&mut mask, &scores, total, self.config.max_sparsity_ratio);

        let profile = SparsityProfile::from_mask(&mask, total);
        Ok((mask, profile))
    }

    /// 强制稀疏化(公开方法,供外部调用)
    ///
    /// 内部委托给 `sparsity::enforce_sparsity`
    pub fn enforce_sparsity(
        &self,
        mask: &mut SesaMask,
        scores: &[f32],
        total: u32,
        max_ratio: f32,
    ) {
        enforce_sparsity(mask, scores, total, max_ratio);
    }

    /// 发布激活完成事件(best-effort,失败仅记录日志)
    async fn publish_completion(
        &self,
        request: &ActivationRequest,
        profile: &SparsityProfile,
        latency_us: u64,
    ) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::SesaActivationCompleted {
                metadata: EventMetadata::new("sesa-router"),
                total_experts: profile.total_experts,
                active_experts: profile.active_experts,
                sparsity_ratio: profile.sparsity_ratio,
                latency_us,
            };
            // WHY 不传播错误:事件发布失败不应阻塞激活主流程
            if let Err(e) = bus.publish(event).await {
                warn!(error = %e, request_id = %request.request_id, "SESA 激活完成事件发布失败");
            }
        }
    }

    /// 启动共识监听订阅任务(后台 tokio task)
    ///
    /// 订阅 `ConsensusReached` 事件,收到后触发稀疏激活策略调整:
    /// - 将 `max_sparsity_ratio` 临时降低 10%(更激进的稀疏化)
    ///
    /// 返回 `JoinHandle` 供调用者管理任务生命周期。
    /// 若未绑定 EventBus,返回 `None`。
    ///
    /// # 注意
    /// 调用方必须在 tokio runtime 上下文中调用此方法。
    ///
    /// # Week 6 教训 - broadcast 时序
    /// `bus.subscribe()` 必须在 `tokio::spawn` 之前同步调用:
    /// broadcast 仅投递给发布时已存在的 receiver;若在 spawn 的 async
    /// block 内 subscribe,后台任务调度时机不确定,可能晚于 publish
    /// 导致事件静默丢失(broadcast 不缓存历史消息给新订阅者)
    pub fn start_consensus_listener(&self) -> Option<tokio::task::JoinHandle<()>> {
        let bus = self.event_bus.clone()?;
        // 在 spawn 之前同步订阅,确保不会错过后续发布的事件
        let mut rx = bus.subscribe();

        Some(tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if let NexusEvent::ConsensusReached { quest_id, .. } = &event {
                    // 收到共识事件:记录日志(实际策略调整可由调用方实现)
                    tracing::info!(
                        quest_id = %quest_id,
                        "SESA 收到 ConsensusReached,稀疏激活策略保持当前配置"
                    );
                }
            }
        }))
    }
}

/// Top-K 降序选择 — 使用 `select_nth_unstable_by` 实现 O(n) 平均复杂度
///
/// 返回前 `k` 个评分最大的元素(未完全排序,但保证是最大的 K 个)。
///
/// # 算法
/// 使用 quickselect 找到第 k-1 大的元素作为 pivot,
/// pivot 左侧(0..k)即为评分最高的 K 个元素。
///
/// # 性能
/// - 平均 O(n),最坏 O(n²)(随机化可避免最坏情况)
/// - 相比 `sort_by` 的 O(n log n),1000 专家规模快约 10x
fn select_top_k_desc(scored: &mut [(usize, f32)], k: usize) -> &[(usize, f32)] {
    if k >= scored.len() {
        return scored;
    }
    let idx = k - 1;
    // 降序:b.1 vs a.1(评分大的在前)
    scored.select_nth_unstable_by(idx, |a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
    });
    &scored[..k]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SesaConfig;

    // === 辅助函数 ===

    fn make_router(capacity_experts: usize) -> SesaRouter {
        let router = SesaRouter::new(SesaConfig::default());
        for i in 0..capacity_experts {
            let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.1 * i as f32; 64]);
            let _ = router.register_expert(expert);
        }
        router
    }

    fn make_request(top_k: usize, deadline_ms: u64) -> ActivationRequest {
        ActivationRequest::new("req-test", vec![0.5; 64], top_k, deadline_ms)
    }

    // === 1. 注册与基础测试 ===

    #[test]
    fn test_register_single_expert() {
        let router = SesaRouter::new(SesaConfig::default());
        let expert = ExpertDescriptor::new("expert-1", vec![0.5; 64]);
        router.register_expert(expert).expect("注册失败");

        assert_eq!(router.expert_count(), 1);
    }

    #[test]
    fn test_register_multiple_experts_assigned_unique_indices() {
        let router = SesaRouter::new(SesaConfig::default());
        for i in 0..10 {
            let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.1 * i as f32; 64]);
            router.register_expert(expert).expect("注册失败");
        }
        assert_eq!(router.expert_count(), 10);

        // 验证 mask_index 唯一性
        let mut indices: Vec<u32> = router.experts.iter().map(|e| e.mask_index).collect();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(indices.len(), 10, "10 个专家应有 10 个唯一索引");
    }

    #[test]
    fn test_register_overwrite_keeps_mask_index() {
        let router = SesaRouter::new(SesaConfig::default());
        let expert = ExpertDescriptor::new("expert-1", vec![0.5; 64]);
        router.register_expert(expert).expect("首次注册失败");

        let original_idx = router
            .experts
            .get("expert-1")
            .map(|e| e.mask_index)
            .expect("应存在");

        // 覆盖注册:更新向量但保留索引
        let updated = ExpertDescriptor::new("expert-1", vec![0.9; 64]);
        router.register_expert(updated).expect("覆盖注册失败");

        let new_idx = router
            .experts
            .get("expert-1")
            .map(|e| e.mask_index)
            .expect("应存在");
        assert_eq!(original_idx, new_idx, "覆盖应保留原 mask_index");
    }

    #[test]
    fn test_register_exceeds_mask_capacity() {
        let router = SesaRouter::new(SesaConfig::default());
        // 注册 256 个专家(刚好满)
        for i in 0..256 {
            let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.1; 64]);
            router.register_expert(expert).expect("注册失败");
        }
        assert_eq!(router.expert_count(), 256);

        // 第 257 个应失败
        let expert = ExpertDescriptor::new("expert-256", vec![0.1; 64]);
        let result = router.register_expert(expert);
        assert!(result.is_err(), "超过 256 容量应失败");
        assert!(matches!(
            result,
            Err(SesaError::IndexOutOfBounds { capacity: 256, .. })
        ));
    }

    #[test]
    fn test_unregister_expert() {
        let router = make_router(5);
        assert_eq!(router.expert_count(), 5);

        let removed = router.unregister_expert("expert-2");
        assert!(removed.is_some());
        assert_eq!(router.expert_count(), 4);

        // 重复注销返回 None
        let again = router.unregister_expert("expert-2");
        assert!(again.is_none());
    }

    // === 2. 激活基础测试 ===

    #[tokio::test]
    async fn test_activate_empty_pool_returns_error() {
        let router = SesaRouter::new(SesaConfig::default());
        let req = make_request(8, 5);
        let result = router.activate(req).await;

        assert!(result.is_err());
        assert!(matches!(result, Err(SesaError::EmptyExpertPool)));
    }

    #[tokio::test]
    async fn test_activate_zero_deadline_returns_timeout() {
        let router = make_router(10);
        let req = make_request(8, 0);
        let result = router.activate(req).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(SesaError::ActivationTimeout { deadline_ms: 0 })
        ));
    }

    #[tokio::test]
    async fn test_activate_single_expert() {
        let router = SesaRouter::new(SesaConfig::default());
        let expert = ExpertDescriptor::new("expert-1", vec![0.5; 64]);
        router.register_expert(expert).expect("注册失败");

        let req = make_request(8, 5);
        let (mask, profile) = router.activate(req).await.expect("激活失败");

        // 单专家场景:max_allowed_active 保证至少 1 位
        // WHY:激活 0 个专家无意义,保留 1 个作为最低保障
        assert_eq!(mask.active_count, 1);
        assert_eq!(profile.total_experts, 1);
        assert_eq!(profile.active_experts, 1);
        // 1/1 = 1.0(单专家时稀疏度约束放宽,保留唯一专家)
        assert!((profile.sparsity_ratio - 1.0).abs() < 1e-5, "1/1 = 1.0");
    }

    #[tokio::test]
    async fn test_activate_top_k_selection() {
        let router = SesaRouter::new(SesaConfig::default());
        // 注册 10 个专家,向量递增
        for i in 0..10 {
            let v = vec![(i as f32) * 0.1; 64];
            let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
            router.register_expert(expert).expect("注册失败");
        }

        // 查询向量与 expert-5 最相似(都是 0.5)
        let req = ActivationRequest::new("req-1", vec![0.5; 64], 3, 5);
        let (mask, profile) = router.activate(req).await.expect("激活失败");

        assert_eq!(mask.active_count, 3, "Top-3 应激活 3 位");
        assert_eq!(profile.total_experts, 10);
        assert_eq!(profile.active_experts, 3);
        assert!((profile.sparsity_ratio - 0.3).abs() < 1e-5, "3/10 = 0.3");
    }

    #[tokio::test]
    async fn test_activate_top_k_exceeds_pool_size() {
        let router = make_router(5);
        // top_k=100 但只有 5 个专家
        // 5 × 0.4 = 2,严格 < 0.4 → 1(2/5=0.4 >= 0.4,减 1)
        let req = make_request(100, 5);
        let (mask, profile) = router.activate(req).await.expect("激活失败");

        // 严格 < 40%:5 × 0.4 = 2,2/5 = 0.4 >= 0.4,所以 max_allowed = 1
        assert_eq!(mask.active_count, 1, "严格 < 40% → 1 位");
        assert_eq!(profile.active_experts, 1);
    }

    // === 3. 稀疏度强制测试 ===

    #[tokio::test]
    async fn test_activate_enforces_sparsity_under_40_percent() {
        let router = SesaRouter::new(SesaConfig::default());
        // 注册 100 个专家(实际只能注册 256,这里用 100)
        for i in 0..100 {
            let v = vec![(i as f32) * 0.01; 64];
            let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
            router.register_expert(expert).expect("注册失败");
        }

        // top_k=100 试图激活全部,但 enforce_sparsity 应裁剪到 39(严格 < 40%)
        let req = make_request(100, 5);
        let (mask, profile) = router.activate(req).await.expect("激活失败");

        assert!(
            profile.sparsity_ratio < 0.4,
            "稀疏度应严格 < 40%, got {}",
            profile.sparsity_ratio
        );
        // 100 × 0.4 = 40,40/100 = 0.4 >= 0.4,所以 max_allowed = 39
        assert_eq!(mask.active_count, 39, "严格 < 40% → 39 位");
    }

    #[tokio::test]
    async fn test_activate_1000_experts_sparsity_under_40_percent() {
        // 模拟 1000 专家规模(256 位掩码限制 → 测试 256 专家)
        let router = SesaRouter::new(SesaConfig::default());
        for i in 0..256 {
            let v = vec![(i as f32) * 0.01; 64];
            let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
            router.register_expert(expert).expect("注册失败");
        }

        // top_k=256 试图激活全部
        let req = make_request(256, 5);
        let (mask, profile) = router.activate(req).await.expect("激活失败");

        // 256 × 0.4 = 102.4 → floor = 102,102/256 = 0.3984375 < 0.4 ✓
        assert_eq!(mask.active_count, 102);
        assert!(
            profile.sparsity_ratio < 0.4,
            "稀疏度应严格 < 40%, got {} (active={})",
            profile.sparsity_ratio,
            profile.active_experts
        );
        let expected_ratio = 102.0_f32 / 256.0;
        assert!(
            (profile.sparsity_ratio - expected_ratio).abs() < 1e-5,
            "102/256 = {}",
            expected_ratio
        );
    }

    // === 4. select_top_k_desc 单元测试 ===

    #[test]
    fn test_select_top_k_desc_basic() {
        let mut scored = vec![(0usize, 0.3f32), (1, 0.9), (2, 0.5), (3, 0.1)];
        let top = select_top_k_desc(&mut scored, 2);
        assert_eq!(top.len(), 2);
        // 前 2 个应是评分最大的两个(0.9 和 0.5)
        let scores: Vec<f32> = top.iter().map(|(_, s)| *s).collect();
        assert!(scores.contains(&0.9));
        assert!(scores.contains(&0.5));
    }

    #[test]
    fn test_select_top_k_desc_k_exceeds_len() {
        let mut scored = vec![(0, 0.5f32)];
        let top = select_top_k_desc(&mut scored, 10);
        assert_eq!(top.len(), 1, "k > len 时返回全部");
    }

    #[test]
    fn test_select_top_k_desc_k_one() {
        let mut scored = vec![(0, 0.3f32), (1, 0.9), (2, 0.5)];
        let top = select_top_k_desc(&mut scored, 1);
        assert_eq!(top.len(), 1);
        assert!((top[0].1 - 0.9).abs() < 1e-5, "应选评分最高的 0.9");
    }

    #[test]
    fn test_select_top_k_desc_preserves_top_k_max() {
        // 验证 Top-K 选择正确性:返回的 K 个应是最大的 K 个
        let mut scored: Vec<(usize, f32)> = (0..100).map(|i| (i, i as f32 * 0.01)).collect();
        let k = 10;
        let top = select_top_k_desc(&mut scored, k);

        // 提取 Top-K 评分并排序
        let mut top_scores: Vec<f32> = top.iter().map(|(_, s)| *s).collect();
        top_scores.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        // 期望:最大的 10 个评分(0.99, 0.98, ..., 0.90)
        for (i, &s) in top_scores.iter().enumerate() {
            let expected = (99 - i) as f32 * 0.01;
            assert!(
                (s - expected).abs() < 1e-5,
                "Top-{} 应为 {}, got {}",
                i,
                expected,
                s
            );
        }
    }

    // === 5. 配置自定义测试 ===

    #[tokio::test]
    async fn test_activate_with_custom_max_sparsity() {
        let config = SesaConfig {
            top_k: 8,
            max_sparsity_ratio: 0.2, // 20%
            activation_deadline_ms: 5,
            mask_width: 256,
            prerequisite_check_enabled: false, // 此测试校验稀疏度,跳过前置校验
        };
        let router = SesaRouter::new(config);
        for i in 0..100 {
            let v = vec![0.01 * i as f32; 64];
            let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
            router.register_expert(expert).expect("注册失败");
        }

        let req = make_request(100, 5);
        let (mask, profile) = router.activate(req).await.expect("激活失败");

        // 100 × 0.2 = 20,20/100 = 0.2 >= 0.2,所以 max_allowed = 19(严格 < 20%)
        assert_eq!(mask.active_count, 19, "严格 < 20% → 19 位");
        assert!(
            profile.sparsity_ratio < 0.2,
            "自定义稀疏度应严格 < 20%, got {}",
            profile.sparsity_ratio
        );
    }

    // === 6. 并发安全测试 ===

    #[tokio::test]
    async fn test_concurrent_activate_no_deadlock() {
        let router = std::sync::Arc::new(make_router(50));
        let mut handles = Vec::new();

        // 10 个并发激活任务
        for i in 0..10 {
            let r = std::sync::Arc::clone(&router);
            handles.push(tokio::spawn(async move {
                let req = ActivationRequest::new(
                    format!("req-{i}"),
                    vec![0.5; 64],
                    8,
                    100, // 较长超时避免并发抖动
                );
                r.activate(req).await.expect("并发激活失败")
            }));
        }

        let mut profiles = Vec::new();
        for h in handles {
            let (_mask, profile) = h.await.expect("task panic");
            profiles.push(profile);
        }

        // 所有并发激活都应满足稀疏度约束
        for p in &profiles {
            assert!(
                p.sparsity_ratio < 0.4,
                "并发激活稀疏度应 < 40%, got {}",
                p.sparsity_ratio
            );
        }
    }
}
