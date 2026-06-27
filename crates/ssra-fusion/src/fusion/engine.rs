//! 融合引擎核心 — 运行时低延迟模板融合
//!
//! 对应架构层:L7 Execution
//! 对应创新点:SSRA(Slime-Style Rapid Adaptation)
//!
//! ## 核心机制
//! - 从 `TemplateRegistry` 零拷贝提取源适配器的 `(weight, strategy)` 元数据
//! - 使用 `select_nth_unstable_by` 实现 O(n) Top-K 选择(降序)
//! - 根据主导策略(权重最高的模板策略)计算融合置信度
//! - `tokio::time::timeout` 包装融合逻辑,超时返回 `FusionTimeout`
//! - 融合成功后通过 EventBus 发布 `SsraFusionCompleted` 事件
//!
//! ## 防御性适配
//! - 订阅 `ConsensusReached`:预编译防御性模板(WeightedAverage 策略)
//! - 订阅 `RedTeamAudit`:预编译安全补救模板(TopK 策略)

use std::sync::Arc;
use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::warn;
use uuid::Uuid;

use crate::config::SsraConfig;
use crate::error::SsraError;
use crate::templates::{precompile, TemplateRegistry, TemplateSpec};
use crate::types::{FusionRequest, FusionResult, FusionStrategy};

/// 融合引擎 — SSRA 运行时低延迟融合的核心组件
///
/// 持有模板注册表(`Arc<TemplateRegistry>`)与可选的 EventBus。
/// 融合操作通过 `fuse` 异步方法触发,内部使用 `tokio::time::timeout` 控制截止时间。
///
/// ## 线程安全
/// `TemplateRegistry` 基于 `DashMap`(分片锁),`SsraConfig` 是 `Clone`,
/// `EventBus` 是 `Clone`(Arc 引用计数)。整个引擎可安全共享(`&self` 调用)。
pub struct SlimeFusionEngine {
    /// 模板注册表(Arc 共享,后台订阅任务可 clone)
    registry: Arc<TemplateRegistry>,
    /// 引擎配置
    config: SsraConfig,
    /// 可选事件总线(融合成功后发布事件)
    event_bus: Option<EventBus>,
}

impl SlimeFusionEngine {
    /// 创建融合引擎(无 EventBus,不发布事件)
    pub fn new(config: SsraConfig) -> Self {
        let registry = Arc::new(TemplateRegistry::new(config.template_cache_size));
        Self {
            registry,
            config,
            event_bus: None,
        }
    }

    /// 创建融合引擎并绑定 EventBus
    ///
    /// 绑定后,`fuse` 成功会发布 `SsraFusionCompleted` 事件,
    /// 调用 `start_defensive_adapter` 可订阅 `ConsensusReached`/`RedTeamAudit`。
    pub fn with_event_bus(config: SsraConfig, bus: EventBus) -> Self {
        let registry = Arc::new(TemplateRegistry::new(config.template_cache_size));
        Self {
            registry,
            config,
            event_bus: Some(bus),
        }
    }

    /// 获取模板注册表引用
    pub fn registry(&self) -> &TemplateRegistry {
        &self.registry
    }

    /// 获取配置引用
    pub fn config(&self) -> &SsraConfig {
        &self.config
    }

    /// 执行融合 — 异步,带超时控制与事件发布
    ///
    /// # 流程
    /// 1. `deadline_ms == 0` 直接返回超时(无可用时间)
    /// 2. `tokio::time::timeout` 包装 `fuse_inner` 同步逻辑
    /// 3. 成功后测量实际延迟并发布 `SsraFusionCompleted` 事件
    ///
    /// # 错误
    /// - `FusionTimeout`:超过 deadline_ms
    /// - `TemplateNotFound`:所有源适配器均未在 registry 注册
    pub async fn fuse(&self, request: FusionRequest) -> Result<FusionResult, SsraError> {
        // deadline_ms == 0 表示无可用时间,直接超时
        if request.deadline_ms == 0 {
            return Err(SsraError::FusionTimeout { deadline_ms: 0 });
        }

        let start = Instant::now();
        let deadline = Duration::from_millis(request.deadline_ms);

        let inner = tokio::time::timeout(deadline, async { self.fuse_inner(&request) }).await;

        match inner {
            Ok(Ok(mut result)) => {
                result.latency_ms = start.elapsed().as_millis() as u64;
                self.publish_completion(&request, &result).await;
                Ok(result)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(SsraError::FusionTimeout {
                deadline_ms: request.deadline_ms,
            }),
        }
    }

    /// 融合核心逻辑(同步,纯内存计算)
    ///
    /// 从 registry 零拷贝提取 `(weight, strategy)`,用 `select_nth_unstable_by`
    /// 选 Top-K,根据主导策略计算置信度。
    fn fuse_inner(&self, request: &FusionRequest) -> Result<FusionResult, SsraError> {
        // 零拷贝收集源适配器元数据(跳过未注册的)
        let mut metas: Vec<(f32, FusionStrategy)> =
            Vec::with_capacity(request.source_adapters.len());
        for id in &request.source_adapters {
            if let Some(meta) = self.registry.get_template_meta(id) {
                metas.push(meta);
            }
        }

        if metas.is_empty() {
            return Err(SsraError::TemplateNotFound {
                capability_id: request.source_adapters.first().cloned().unwrap_or_default(),
            });
        }

        // Top-K 选择:降序,取权重最高的 K 个
        let k = request.top_k.min(metas.len()).max(1);
        let selected = select_top_k_desc(&mut metas, k);

        // 主导策略:Top-K 中权重最高的模板策略
        let strategy = selected[0].1;
        let confidence = compute_confidence(selected, strategy);
        let selected_count = selected.len();

        Ok(FusionResult {
            fused_template_id: Uuid::now_v7().to_string(),
            latency_ms: 0,
            confidence,
            selected_count,
        })
    }

    /// 发布融合完成事件(best-effort,失败仅记录日志)
    async fn publish_completion(&self, request: &FusionRequest, result: &FusionResult) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::SsraFusionCompleted {
                metadata: EventMetadata::new("ssra-fusion"),
                quest_id: request.quest_id.clone(),
                fused_template_id: result.fused_template_id.clone(),
                latency_ms: result.latency_ms,
                confidence: result.confidence,
            };
            if let Err(e) = bus.publish(event).await {
                warn!(error = %e, "SSRA 融合完成事件发布失败");
            }
        }
    }

    /// 启动防御性适配订阅任务(后台 tokio task)
    ///
    /// 订阅 `ConsensusReached` 与 `RedTeamAudit` 事件,收到后触发防御性模板预编译:
    /// - `ConsensusReached`:以 quest_id 为基础预编译 WeightedAverage 模板
    /// - `RedTeamAudit`:以 vulnerability_type 为基础预编译 TopK 模板
    ///
    /// 返回 `JoinHandle` 供调用者管理任务生命周期。
    /// 若未绑定 EventBus,返回 `None`。
    ///
    /// # 注意
    /// 调用方必须在 tokio runtime 上下文中调用此方法。
    pub fn start_defensive_adapter(&self) -> Option<tokio::task::JoinHandle<()>> {
        let bus = self.event_bus.clone()?;
        let registry = Arc::clone(&self.registry);
        // 在 spawn 之前同步订阅,确保不会错过后续发布的事件
        // WHY: tokio::broadcast 仅投递给发布时已存在的 receiver;
        // 若在 spawn 的 async block 内 subscribe,后台任务调度时机不确定,
        // 可能晚于 publish 导致事件静默丢失(broadcast 不缓存历史消息给新订阅者)
        let mut rx = bus.subscribe();

        Some(tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                match &event {
                    NexusEvent::ConsensusReached { quest_id, .. } => {
                        defensive_adapt(&registry, quest_id, FusionStrategy::WeightedAverage);
                    }
                    NexusEvent::RedTeamAudit {
                        vulnerability_type, ..
                    } => {
                        defensive_adapt(&registry, vulnerability_type, FusionStrategy::TopK);
                    }
                    _ => {}
                }
            }
        }))
    }
}

/// Top-K 降序选择 — 使用 `select_nth_unstable_by` 实现 O(n) 平均复杂度
///
/// 返回前 `k` 个权重最大的元素(未完全排序,但保证是最大的 K 个)。
fn select_top_k_desc(metas: &mut [(f32, FusionStrategy)], k: usize) -> &[(f32, FusionStrategy)] {
    if k >= metas.len() {
        return metas;
    }
    let idx = k - 1;
    // 降序:b.0 vs a.0(大的在前)
    metas.select_nth_unstable_by(idx, |a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    &metas[..k]
}

/// 根据融合策略计算置信度
///
/// - `WeightedAverage`:Σ(w_i²) / Σ(w_i),偏向高权重模板
/// - `TopK`:max(w_i),取最强模板权重
/// - `MeanField`:Σ(w_i) / k,Top-K 算术平均
fn compute_confidence(top: &[(f32, FusionStrategy)], strategy: FusionStrategy) -> f32 {
    if top.is_empty() {
        return 0.0;
    }
    let k = top.len() as f32;
    match strategy {
        FusionStrategy::WeightedAverage => {
            let sum_w: f32 = top.iter().map(|(w, _)| *w).sum();
            let sum_w2: f32 = top.iter().map(|(w, _)| w * w).sum();
            if sum_w > 0.0 {
                (sum_w2 / sum_w).clamp(0.0, 1.0)
            } else {
                0.0
            }
        }
        FusionStrategy::TopK => top
            .iter()
            .map(|(w, _)| *w)
            .fold(0.0_f32, f32::max)
            .clamp(0.0, 1.0),
        FusionStrategy::MeanField => {
            let sum: f32 = top.iter().map(|(w, _)| *w).sum();
            (sum / k).clamp(0.0, 1.0)
        }
    }
}

/// 防御性适配 — 预编译并注册模板(best-effort,缓存满时先驱逐)
fn defensive_adapt(registry: &TemplateRegistry, base_id: &str, strategy: FusionStrategy) {
    let cap_id = format!("defensive-{base_id}");
    let spec = TemplateSpec::new(cap_id.clone(), vec![], strategy);
    let template = precompile(spec);

    if registry.register(template).is_err() {
        // 缓存满,驱逐最旧后重试一次
        registry.evict_oldest();
        let retry_spec = TemplateSpec::new(cap_id, vec![], strategy);
        if let Err(e) = registry.register(precompile(retry_spec)) {
            warn!(error = %e, "防御性适配模板注册失败");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SlimeTemplate;

    // === 辅助函数 ===

    fn make_engine_with_templates(
        capacity: usize,
        templates: Vec<(&str, f32, FusionStrategy)>,
    ) -> SlimeFusionEngine {
        let config = SsraConfig {
            template_cache_size: capacity,
            ..Default::default()
        };
        let engine = SlimeFusionEngine::new(config);
        for (id, weight, strategy) in templates {
            let t = SlimeTemplate::new(id, vec!["x".into()], strategy).with_weight(weight);
            engine.registry().register(t).expect("注册失败");
        }
        engine
    }

    fn make_request(source: Vec<&str>, top_k: usize, deadline_ms: u64) -> FusionRequest {
        FusionRequest::new(
            "q-test",
            source.into_iter().map(String::from).collect(),
            "target",
            deadline_ms,
            top_k,
        )
    }

    // === 1. 单模板融合 ===

    #[tokio::test]
    async fn test_fuse_single_template() {
        let engine = make_engine_with_templates(16, vec![("cap-1", 0.8, FusionStrategy::TopK)]);
        let req = make_request(vec!["cap-1"], 8, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        assert_eq!(result.selected_count, 1);
        assert!(
            (result.confidence - 0.8).abs() < 1e-5,
            "TopK 策略取最大权重"
        );
        assert!(!result.fused_template_id.is_empty());
    }

    // === 2. 多模板融合 ===

    #[tokio::test]
    async fn test_fuse_multiple_templates() {
        let engine = make_engine_with_templates(
            16,
            vec![
                ("cap-1", 0.3, FusionStrategy::TopK),
                ("cap-2", 0.9, FusionStrategy::TopK),
                ("cap-3", 0.5, FusionStrategy::TopK),
            ],
        );
        let req = make_request(vec!["cap-1", "cap-2", "cap-3"], 3, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        assert_eq!(result.selected_count, 3);
        // TopK 策略:max(0.3, 0.9, 0.5) = 0.9
        assert!((result.confidence - 0.9).abs() < 1e-5);
    }

    // === 3. 空源适配器列表 ===

    #[tokio::test]
    async fn test_fuse_empty_source_adapters() {
        let engine = make_engine_with_templates(16, vec![]);
        let req = make_request(vec![], 8, 20);
        let result = engine.fuse(req).await;

        assert!(result.is_err(), "空源列表应返回错误");
        assert!(matches!(result, Err(SsraError::TemplateNotFound { .. })));
    }

    // === 4. 源适配器全部未注册 ===

    #[tokio::test]
    async fn test_fuse_all_adapters_not_found() {
        let engine = make_engine_with_templates(16, vec![("cap-1", 0.5, FusionStrategy::TopK)]);
        let req = make_request(vec!["missing-1", "missing-2"], 8, 20);
        let result = engine.fuse(req).await;

        assert!(result.is_err(), "全部未注册应返回错误");
        match result {
            Err(SsraError::TemplateNotFound { capability_id }) => {
                assert_eq!(capability_id, "missing-1");
            }
            _ => panic!("期望 TemplateNotFound 错误"),
        }
    }

    // === 5. 部分源适配器未注册(跳过未注册的) ===

    #[tokio::test]
    async fn test_fuse_partial_adapters_not_found() {
        let engine = make_engine_with_templates(16, vec![("cap-1", 0.8, FusionStrategy::TopK)]);
        // cap-1 存在,missing 不存在
        let req = make_request(vec!["cap-1", "missing"], 8, 20);
        let result = engine.fuse(req).await.expect("部分缺失不应失败");

        assert_eq!(result.selected_count, 1, "仅 1 个有效模板参与融合");
    }

    // === 6. 超时(deadline_ms == 0) ===

    #[tokio::test]
    async fn test_fuse_timeout_zero_deadline() {
        let engine = make_engine_with_templates(16, vec![("cap-1", 0.5, FusionStrategy::TopK)]);
        let req = make_request(vec!["cap-1"], 8, 0);
        let result = engine.fuse(req).await;

        assert!(result.is_err(), "deadline=0 应返回超时");
        assert!(matches!(
            result,
            Err(SsraError::FusionTimeout { deadline_ms: 0 })
        ));
    }

    // === 7. Top-K 边界:top_k=1 ===

    #[tokio::test]
    async fn test_fuse_top_k_one() {
        let engine = make_engine_with_templates(
            16,
            vec![
                ("cap-1", 0.3, FusionStrategy::TopK),
                ("cap-2", 0.9, FusionStrategy::TopK),
            ],
        );
        let req = make_request(vec!["cap-1", "cap-2"], 1, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        assert_eq!(result.selected_count, 1, "top_k=1 只选 1 个");
        assert!((result.confidence - 0.9).abs() < 1e-5, "应选权重最高的 0.9");
    }

    // === 8. Top-K 边界:top_k > 源数量 ===

    #[tokio::test]
    async fn test_fuse_top_k_exceeds_source_count() {
        let engine = make_engine_with_templates(16, vec![("cap-1", 0.5, FusionStrategy::TopK)]);
        let req = make_request(vec!["cap-1"], 100, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        assert_eq!(result.selected_count, 1, "源仅 1 个,selected 不能超过");
    }

    // === 9. WeightedAverage 策略 ===

    #[tokio::test]
    async fn test_fuse_weighted_average_strategy() {
        let engine = make_engine_with_templates(
            16,
            vec![
                ("cap-1", 0.2, FusionStrategy::WeightedAverage),
                ("cap-2", 0.8, FusionStrategy::WeightedAverage),
            ],
        );
        let req = make_request(vec!["cap-1", "cap-2"], 2, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        // WeightedAverage = Σ(w²) / Σ(w) = (0.04 + 0.64) / (0.2 + 0.8) = 0.68 / 1.0 = 0.68
        assert!(
            (result.confidence - 0.68).abs() < 1e-5,
            "WeightedAverage 应为 0.68, got {}",
            result.confidence
        );
    }

    // === 10. MeanField 策略 ===

    #[tokio::test]
    async fn test_fuse_meanfield_strategy() {
        let engine = make_engine_with_templates(
            16,
            vec![
                ("cap-1", 0.2, FusionStrategy::MeanField),
                ("cap-2", 0.8, FusionStrategy::MeanField),
            ],
        );
        let req = make_request(vec!["cap-1", "cap-2"], 2, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        // MeanField = (0.2 + 0.8) / 2 = 0.5
        assert!(
            (result.confidence - 0.5).abs() < 1e-5,
            "MeanField 应为 0.5, got {}",
            result.confidence
        );
    }

    // === 11. 置信度 ∈ [0.0, 1.0] ===

    #[tokio::test]
    async fn test_fuse_confidence_in_range() {
        let engine = make_engine_with_templates(
            16,
            vec![
                ("cap-1", 1.0, FusionStrategy::WeightedAverage),
                ("cap-2", 1.0, FusionStrategy::WeightedAverage),
                ("cap-3", 1.0, FusionStrategy::WeightedAverage),
            ],
        );
        let req = make_request(vec!["cap-1", "cap-2", "cap-3"], 3, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        assert!(
            result.confidence >= 0.0 && result.confidence <= 1.0,
            "confidence 应在 [0, 1], got {}",
            result.confidence
        );
    }

    // === 12. 延迟记录 ===

    #[tokio::test]
    async fn test_fuse_latency_recorded() {
        let engine = make_engine_with_templates(16, vec![("cap-1", 0.5, FusionStrategy::TopK)]);
        let req = make_request(vec!["cap-1"], 8, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        // 纯内存操作,延迟应在 20ms 以内
        assert!(
            result.latency_ms <= 20,
            "延迟应 ≤ 20ms, got {}ms",
            result.latency_ms
        );
    }

    // === 13. select_top_k_desc 单元测试 ===

    #[test]
    fn test_select_top_k_desc() {
        let mut metas = vec![
            (0.3_f32, FusionStrategy::TopK),
            (0.9, FusionStrategy::TopK),
            (0.5, FusionStrategy::TopK),
            (0.1, FusionStrategy::TopK),
        ];
        let top = select_top_k_desc(&mut metas, 2);
        assert_eq!(top.len(), 2);
        // 前 2 个应是最大的两个(0.9 和 0.5),但顺序不保证
        let weights: Vec<f32> = top.iter().map(|(w, _)| *w).collect();
        assert!(weights.contains(&0.9));
        assert!(weights.contains(&0.5));
    }

    #[test]
    fn test_select_top_k_desc_k_exceeds_len() {
        let mut metas = vec![(0.5_f32, FusionStrategy::TopK)];
        let top = select_top_k_desc(&mut metas, 10);
        assert_eq!(top.len(), 1, "k > len 时返回全部");
    }

    // === 14. compute_confidence 边界 ===

    #[test]
    fn test_compute_confidence_empty() {
        let confidence = compute_confidence(&[], FusionStrategy::TopK);
        assert_eq!(confidence, 0.0);
    }

    #[test]
    fn test_compute_confidence_topk_max() {
        let top = vec![(0.3_f32, FusionStrategy::TopK), (0.9, FusionStrategy::TopK)];
        let confidence = compute_confidence(&top, FusionStrategy::TopK);
        assert!((confidence - 0.9).abs() < 1e-5);
    }

    // === 15. 主导策略(权重最高模板的策略) ===

    #[tokio::test]
    async fn test_fuse_dominant_strategy() {
        // cap-1 权重最高(0.9)用 MeanField,cap-2 权重低(0.2)用 TopK
        // 主导策略应为 cap-1 的 MeanField
        let engine = make_engine_with_templates(
            16,
            vec![
                ("cap-1", 0.9, FusionStrategy::MeanField),
                ("cap-2", 0.2, FusionStrategy::TopK),
            ],
        );
        let req = make_request(vec!["cap-1", "cap-2"], 2, 20);
        let result = engine.fuse(req).await.expect("融合失败");

        // MeanField = (0.9 + 0.2) / 2 = 0.55
        assert!(
            (result.confidence - 0.55).abs() < 1e-5,
            "主导策略 MeanField 应为 0.55, got {}",
            result.confidence
        );
    }
}
