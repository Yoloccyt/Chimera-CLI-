//! GEA 激活器主逻辑 — 门控计算、冲突消解、事件发布与缓存
//!
//! 对应架构层:L6 Router
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 设计决策(WHY)
//! - 专家注册表用 `RwLock<HashMap>`:读多写少场景,读锁并发无阻塞
//! - 缓存用 `DashMap`:线程安全,支持并发读写,LRU 容量 128
//! - `activate` 为 async:因 EventBus::publish 为 async(保留 API 稳定性)
//! - 缓存 key 用 TaskProfile 的 serde 序列化哈希:f32 不实现 Hash(NaN),
//!   用序列化避免手动处理浮点哈希

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{debug, warn};

use crate::config::GeaConfig;
use crate::conflict::resolve_conflicts;
use crate::error::GeaError;
use crate::gating::compute_gate_value;
use crate::types::{ActivationResult, ExpertId, ExpertProfile, TaskProfile};

/// 每 N 次激活发布一次缓存统计事件
const CACHE_STATS_INTERVAL: u64 = 100;

/// 缓存统计计数器(原子,线程安全)
#[derive(Debug, Default)]
struct CacheStats {
    /// 总激活次数
    total: AtomicU64,
    /// 缓存命中次数
    hits: AtomicU64,
}

impl CacheStats {
    fn record_hit(&self) {
        self.total.fetch_add(1, Ordering::Relaxed);
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_miss(&self) {
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    fn hit_rate(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let hits = self.hits.load(Ordering::Relaxed);
        hits as f32 / total as f32
    }

    fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

/// GEA 激活器 — 门控专家激活调度核心
///
/// 维护专家注册表,接收任务画像,计算门控值并冲突消解,
/// 返回 Top-K 激活专家列表,同时发布事件通知订阅者。
///
/// # 线程安全
/// - `expert_registry` 用 `RwLock` 保护,读多写少
/// - `activation_cache` 用 `DashMap`,支持并发读写
/// - `cache_stats` 用原子计数器,无锁统计
pub struct GeaActivator {
    /// 专家注册表(读多写少,用 RwLock)
    expert_registry: RwLock<HashMap<ExpertId, ExpertProfile>>,
    /// GEA 配置
    config: GeaConfig,
    /// 事件总线(跨层通信唯一通道)
    event_bus: EventBus,
    /// 激活缓存:key 为 TaskProfile 序列化哈希,value 为 (结果, 写入时刻)
    activation_cache: DashMap<u64, (ActivationResult, Instant)>,
    /// 缓存命中统计
    cache_stats: CacheStats,
}

impl GeaActivator {
    /// 创建新的 GEA 激活器
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败(权重和、阈值范围等)
    pub fn new(config: GeaConfig, event_bus: EventBus) -> Result<Self, GeaError> {
        config.validate()?;
        Ok(Self {
            expert_registry: RwLock::new(HashMap::new()),
            config,
            event_bus,
            activation_cache: DashMap::new(),
            cache_stats: CacheStats::default(),
        })
    }

    /// 注册专家
    ///
    /// 若专家 ID 已存在,覆盖旧画像。
    pub fn register_expert(&self, profile: ExpertProfile) {
        let mut registry = self
            .expert_registry
            .write()
            .expect("expert_registry poisoned");
        registry.insert(profile.expert_id.clone(), profile);
    }

    /// 注销专家
    ///
    /// 若专家不存在,静默忽略(幂等)。
    pub fn unregister_expert(&self, expert_id: &ExpertId) {
        let mut registry = self
            .expert_registry
            .write()
            .expect("expert_registry poisoned");
        registry.remove(expert_id);
    }

    /// 激活专家:门控计算 → 冲突消解 → 发布事件
    ///
    /// # 流程
    /// 1. 查缓存:5 秒内相同 TaskProfile 直接返回缓存结果
    /// 2. 读注册表快照(持读锁期间计算门控值)
    /// 3. 对每个专家计算门控值,筛选 >= 动态阈值的候选
    /// 4. 冲突消解:综合评分排序 + 重叠检测 + Top-K
    /// 5. 写缓存,发布 `ExpertActivated` 事件
    /// 6. 每 100 次激活发布 `ActivationCacheStats` 事件
    ///
    /// # 错误
    /// - `ConflictResolutionFailed`:冲突消解内部错误
    /// - `ExpertNotFound`:候选专家在注册表中找不到(理论上不会发生)
    pub async fn activate(&self, task: &TaskProfile) -> Result<ActivationResult, GeaError> {
        // 步骤 1:查缓存
        let cache_key = hash_task_profile(task);
        if let Some(entry) = self.activation_cache.get(&cache_key) {
            let (cached_result, written_at) = entry.value();
            if written_at.elapsed() < Duration::from_secs(self.config.cache_ttl_secs) {
                self.cache_stats.record_hit();
                debug!("GEA cache hit, key={cache_key}");
                return Ok(cached_result.clone());
            }
        }
        // 缓存未命中或过期
        self.cache_stats.record_miss();

        // 步骤 2-4:持读锁完成门控计算与冲突消解
        // WHY 块作用域:确保 RwLockReadGuard 在 await 之前释放(clippy::await_holding_lock)
        let result = {
            let registry = self
                .expert_registry
                .read()
                .expect("expert_registry poisoned");

            // 动态阈值:基于当前注册表规模估算负载因子
            let load_factor = self.estimate_load_factor(&registry);
            let threshold = self.dynamic_threshold(load_factor);

            // 步骤 3:计算门控值,筛选候选
            let mut candidates: Vec<(ExpertId, f32)> = Vec::new();
            for (expert_id, profile) in registry.iter() {
                let gate = compute_gate_value(task, profile, &self.config);
                if gate >= threshold {
                    candidates.push((expert_id.clone(), gate));
                }
            }

            // 步骤 4:冲突消解(复用同一读锁,避免二次加锁)
            resolve_conflicts(candidates, &registry, &self.config)?
        }; // 读锁在此释放,后续 await 不持锁

        // 步骤 5:写缓存(LRU 驱逐)
        self.write_cache(cache_key, result.clone());

        // 步骤 6:发布 ExpertActivated 事件
        self.publish_activation_event(&result).await;

        // 步骤 7:每 100 次激活发布缓存统计
        self.maybe_publish_cache_stats().await;

        Ok(result)
    }

    /// 动态激活阈值:threshold = base + load_factor × 0.2
    ///
    /// `load_factor` ∈ [0.0, 1.0],负载越高阈值越高(更难激活),
    /// 避免高负载时激活过多专家导致资源争抢。
    pub fn dynamic_threshold(&self, load_factor: f32) -> f32 {
        let adjusted = self.config.activation_threshold + load_factor * 0.2;
        // clamp 到 [0.0, 1.0] 防止越界
        adjusted.clamp(0.0, 1.0)
    }

    /// 估算负载因子:基于注册表规模
    ///
    /// WHY 简单启发式:专家数 < 10 时负载低(0.0-0.3),
    /// 10-50 时中等(0.3-0.7),> 50 时高(0.7-1.0)。
    /// 后续可替换为基于 CPU/内存的真实负载指标。
    fn estimate_load_factor(&self, registry: &HashMap<ExpertId, ExpertProfile>) -> f32 {
        let count = registry.len();
        if count <= 10 {
            count as f32 / 10.0 * 0.3
        } else if count <= 50 {
            0.3 + (count - 10) as f32 / 40.0 * 0.4
        } else {
            (0.7 + (count - 50) as f32 / 50.0 * 0.3).min(1.0)
        }
    }

    /// 写缓存,执行 LRU 驱逐
    fn write_cache(&self, key: u64, result: ActivationResult) {
        // LRU 驱逐:超过容量时移除最早的条目
        if self.activation_cache.len() >= self.config.cache_capacity {
            self.evict_oldest();
        }
        self.activation_cache.insert(key, (result, Instant::now()));
    }

    /// 驱逐最旧的缓存条目(LRU)
    ///
    /// WHY 简单实现:遍历找最旧的移除。DashMap 无序,需全遍历。
    /// 缓存容量 128,遍历成本可接受。后续可换 LRU 专用数据结构优化。
    fn evict_oldest(&self) {
        let mut oldest_key: Option<u64> = None;
        let mut oldest_time = Instant::now();

        for entry in self.activation_cache.iter() {
            let (_, written_at) = entry.value();
            if *written_at < oldest_time {
                oldest_time = *written_at;
                oldest_key = Some(*entry.key());
            }
        }

        if let Some(key) = oldest_key {
            self.activation_cache.remove(&key);
            debug!("GEA cache evicted key={key}");
        }
    }

    /// 发布 ExpertActivated 事件
    async fn publish_activation_event(&self, result: &ActivationResult) {
        let event = NexusEvent::ExpertActivated {
            metadata: EventMetadata::new("gea-activator"),
            activated_experts: result.activated.iter().map(|id| id.to_string()).collect(),
            suppressed_experts: result.suppressed.iter().map(|id| id.to_string()).collect(),
            top_gate_value: result.top_gate_value,
        };

        if let Err(e) = self.event_bus.publish(event).await {
            warn!("Failed to publish ExpertActivated event: {e}");
        }
    }

    /// 每 CACHE_STATS_INTERVAL 次激活发布一次缓存统计事件
    async fn maybe_publish_cache_stats(&self) {
        let total = self.cache_stats.total();
        if total > 0 && total.is_multiple_of(CACHE_STATS_INTERVAL) {
            let event = NexusEvent::ActivationCacheStats {
                metadata: EventMetadata::new("gea-activator"),
                hit_rate: self.cache_stats.hit_rate(),
                entry_count: self.activation_cache.len() as u32,
            };

            if let Err(e) = self.event_bus.publish(event).await {
                warn!("Failed to publish ActivationCacheStats event: {e}");
            }
        }
    }

    /// 获取当前缓存命中率(测试与监控用)
    pub fn cache_hit_rate(&self) -> f32 {
        self.cache_stats.hit_rate()
    }

    /// 获取当前缓存条目数(测试与监控用)
    pub fn cache_len(&self) -> usize {
        self.activation_cache.len()
    }

    /// 获取当前注册专家数(测试与监控用)
    pub fn expert_count(&self) -> usize {
        self.expert_registry
            .read()
            .expect("expert_registry poisoned")
            .len()
    }
}

/// 计算 TaskProfile 的哈希(缓存 key)
///
/// WHY serde 序列化哈希:TaskProfile 含 Vec<f32>,f32 不实现 Hash(NaN 问题)。
/// 用 serde_json 序列化为字符串再哈希,稳定且无 NaN 歧义。
fn hash_task_profile(task: &TaskProfile) -> u64 {
    use std::hash::{Hash, Hasher};

    // 序列化为 JSON 字符串,确保稳定表示
    let json = serde_json::to_string(task).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    json.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_activator() -> GeaActivator {
        let config = GeaConfig::default();
        let event_bus = EventBus::new();
        GeaActivator::new(config, event_bus).unwrap()
    }

    fn make_expert(id: &str, vector: Vec<f32>, priority: f32, tags: Vec<&str>) -> ExpertProfile {
        ExpertProfile::new(
            id,
            vector,
            priority,
            tags.into_iter().map(String::from).collect(),
        )
    }

    fn make_task(complexity: f32, task_type: &str) -> TaskProfile {
        TaskProfile::new(complexity, task_type, 30, vec![0.5; 64])
    }

    #[test]
    fn test_dynamic_threshold() {
        let activator = make_activator();
        let base = GeaConfig::default().activation_threshold;

        // load_factor = 0.0,阈值 = base
        assert!((activator.dynamic_threshold(0.0) - base).abs() < 1e-6);
        // load_factor = 1.0,阈值 = base + 0.2
        assert!((activator.dynamic_threshold(1.0) - (base + 0.2)).abs() < 1e-6);
        // load_factor = 0.5,阈值 = base + 0.1
        assert!((activator.dynamic_threshold(0.5) - (base + 0.1)).abs() < 1e-6);
    }

    #[test]
    fn test_dynamic_threshold_clamped() {
        let activator = make_activator();
        // 超出 [0,1] 应被 clamp
        assert!(activator.dynamic_threshold(2.0) <= 1.0);
        assert!(activator.dynamic_threshold(-1.0) >= 0.0);
    }

    #[test]
    fn test_register_unregister_expert() {
        let activator = make_activator();
        assert_eq!(activator.expert_count(), 0);

        let expert = make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]);
        activator.register_expert(expert);
        assert_eq!(activator.expert_count(), 1);

        activator.unregister_expert(&ExpertId::new("e-1"));
        assert_eq!(activator.expert_count(), 0);

        // 注销不存在的专家(幂等)
        activator.unregister_expert(&ExpertId::new("nonexistent"));
        assert_eq!(activator.expert_count(), 0);
    }

    #[tokio::test]
    async fn test_activate_empty_registry() {
        let activator = make_activator();
        let task = make_task(0.8, "code-gen");
        let result = activator.activate(&task).await.unwrap();
        assert!(!result.has_activated());
    }

    #[tokio::test]
    async fn test_activate_with_experts() {
        let activator = make_activator();

        // 注册两个正交专家
        let mut v1 = vec![0.0; 64];
        v1[0] = 1.0;
        let mut v2 = vec![0.0; 64];
        v2[1] = 1.0;
        activator.register_expert(make_expert("e-1", v1, 0.8, vec!["code-gen"]));
        activator.register_expert(make_expert("e-2", v2, 0.8, vec!["refactor"]));

        let task = make_task(0.9, "code-gen");
        let result = activator.activate(&task).await.unwrap();
        assert!(result.has_activated());
    }

    #[tokio::test]
    async fn test_cache_hit_within_ttl() {
        let activator = make_activator();
        activator.register_expert(make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]));

        let task = make_task(0.8, "code-gen");
        // 第一次激活:缓存未命中
        let _ = activator.activate(&task).await.unwrap();
        assert_eq!(activator.cache_hit_rate(), 0.0);

        // 第二次激活相同任务:应命中缓存
        let _ = activator.activate(&task).await.unwrap();
        assert!(activator.cache_hit_rate() > 0.0);
    }

    #[tokio::test]
    async fn test_cache_miss_different_tasks() {
        let activator = make_activator();
        activator.register_expert(make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]));

        let task1 = make_task(0.8, "code-gen");
        let task2 = make_task(0.9, "refactor");
        // 不同任务应缓存未命中
        let _ = activator.activate(&task1).await.unwrap();
        let _ = activator.activate(&task2).await.unwrap();
        assert_eq!(activator.cache_hit_rate(), 0.0);
    }

    #[tokio::test]
    async fn test_cache_lru_eviction() {
        // 配置小缓存容量,触发 LRU 驱逐
        let config = GeaConfig {
            cache_capacity: 2,
            ..Default::default()
        };
        let activator = GeaActivator::new(config, EventBus::new()).unwrap();
        activator.register_expert(make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]));

        // 插入 3 个不同任务,应驱逐最旧的
        let task1 = make_task(0.8, "code-gen");
        let task2 = make_task(0.8, "refactor");
        let task3 = make_task(0.8, "test");

        let _ = activator.activate(&task1).await.unwrap();
        let _ = activator.activate(&task2).await.unwrap();
        let _ = activator.activate(&task3).await.unwrap();

        // 缓存容量 2,应有 2 个条目
        assert_eq!(activator.cache_len(), 2);
    }

    #[test]
    fn test_invalid_config_rejected() {
        let config = GeaConfig {
            w1: -0.1,
            ..Default::default()
        };
        let result = GeaActivator::new(config, EventBus::new());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_event_published() {
        let event_bus = EventBus::new();
        let mut rx = event_bus.subscribe();

        let activator = GeaActivator::new(GeaConfig::default(), event_bus).unwrap();
        activator.register_expert(make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]));

        let task = make_task(0.9, "code-gen");
        let _ = activator.activate(&task).await.unwrap();

        // 应收到 ExpertActivated 事件
        let event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("recv failed");
        assert_eq!(event.type_name(), "ExpertActivated");
    }
}
