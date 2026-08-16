//! GEA 激活器主逻辑 — 门控计算、冲突消解、事件发布与缓存
//!
//! 对应架构层:L9 Quest(权威源规则 §2.1,与 lib.rs 一致;旧标 L6 已于 2026-07-31 订正)
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 设计决策(WHY)
//! - 专家注册表用 `RwLock<HashMap>`:读多写少场景,读锁并发无阻塞
//! - 缓存用 `DashMap`:线程安全,支持并发读写,LRU 容量 128
//! - `activate` 为 async:因 EventBus::publish 为 async(保留 API 稳定性)
//! - 缓存 key 直接用 `TaskProfile`(已 impl Hash+Eq):零分配 O(n) 哈希,
//!   替代旧的 serde_json 序列化哈希方案。f32 经 `to_bits()` 转为确定性 u32,
//!   绕过 NaN 不可哈希问题(详见 `types::TaskProfile` 的 Hash impl 注释)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{debug, warn};

use crate::config::GeaConfig;
use crate::conflict::{resolve_conflicts_with_norms, ExpertNorm};
use crate::error::GeaError;
use crate::gating::{compute_gate_value_with_norms, prefix_l2_norm};
use crate::types::{ActivationResult, ExpertId, ExpertProfile, TaskProfile};

/// 每 N 次激活发布一次缓存统计事件
const CACHE_STATS_INTERVAL: u64 = 100;

/// 根据缓存容量计算 LRU 驱逐采样数
///
/// 使用 sqrt(capacity) 策略，使采样数随容量增长而增长。
/// 容量 128 时 sample=11（原为固定 8），容量 512 时 sample=22。
/// 限制在 [4, 32] 范围内，避免极端值。
fn compute_evict_sample_size(capacity: usize) -> usize {
    let sample = (capacity as f64).sqrt().floor() as usize;
    sample.clamp(4, 32)
}

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
    /// 专家向量范数缓存(注册时预计算,门控热路径避免重复范数计算)
    ///
    /// WHY 独立缓存而非存入 ExpertProfile:保持 ExpertProfile 序列化
    /// 结构稳定(serde roundtrip 兼容),范数属派生数据,随注册表读写同步。
    /// 同时供冲突消解复用(高密度专家池下候选构建免 O(n·d) 重算)。
    expert_norms: RwLock<HashMap<ExpertId, ExpertNorm>>,
    /// GEA 配置
    config: GeaConfig,
    /// 事件总线(跨层通信唯一通道)
    event_bus: EventBus,
    /// 激活缓存:key 为 TaskProfile(直接 Hash,value 为 (结果, 写入时刻)
    activation_cache: DashMap<TaskProfile, (ActivationResult, Instant)>,
    /// 缓存命中统计
    cache_stats: CacheStats,
    /// LRU 驱逐采样数(根据缓存容量动态计算)
    evict_sample_size: usize,
}

impl GeaActivator {
    /// 创建新的 GEA 激活器
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败(权重和、阈值范围等)
    pub fn new(config: GeaConfig, event_bus: EventBus) -> Result<Self, GeaError> {
        config.validate()?;
        let evict_sample_size = compute_evict_sample_size(config.cache_capacity);
        Ok(Self {
            expert_registry: RwLock::new(HashMap::new()),
            expert_norms: RwLock::new(HashMap::new()),
            config,
            event_bus,
            activation_cache: DashMap::new(),
            cache_stats: CacheStats::default(),
            evict_sample_size,
        })
    }

    /// 注册专家
    ///
    /// 若专家 ID 已存在,覆盖旧画像。
    pub fn register_expert(&self, profile: ExpertProfile) {
        // P2-2: PoisonError 恢复 — 与 decay-engine learner_holder 模式一致
        // (§4.1 红线:避免 expect(),用 unwrap_or_else 处理锁 poison)
        let mut registry = self.expert_registry.write().unwrap_or_else(|p| {
            warn!("expert_registry write lock poisoned, recovering with inner data");
            p.into_inner()
        });
        // 同步预计算专家向量范数(门控热路径复用,避免每次激活重复计算)
        let norm = ExpertNorm::from_vector(&profile.expert_vector);
        let expert_id = profile.expert_id.clone();
        registry.insert(expert_id.clone(), profile);
        drop(registry); // 释放注册表写锁,再更新范数缓存(避免嵌套持锁)
        let mut norms = self.expert_norms.write().unwrap_or_else(|p| {
            warn!("expert_norms write lock poisoned, recovering with inner data");
            p.into_inner()
        });
        norms.insert(expert_id, norm);
    }

    /// 注销专家
    ///
    /// 若专家不存在,静默忽略(幂等)。
    pub fn unregister_expert(&self, expert_id: &ExpertId) {
        let mut registry = self.expert_registry.write().unwrap_or_else(|p| {
            warn!("expert_registry write lock poisoned, recovering with inner data");
            p.into_inner()
        });
        registry.remove(expert_id);
        drop(registry);
        let mut norms = self.expert_norms.write().unwrap_or_else(|p| {
            warn!("expert_norms write lock poisoned, recovering with inner data");
            p.into_inner()
        });
        norms.remove(expert_id);
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
        // 步骤 1:查缓存(直接用 TaskProfile 作 key,TaskProfile 已 impl Hash+Eq)
        if let Some(entry) = self.activation_cache.get(task) {
            let (cached_result, written_at) = entry.value();
            if written_at.elapsed() < Duration::from_secs(self.config.cache_ttl_secs) {
                self.cache_stats.record_hit();
                debug!("GEA cache hit, task_type={}", task.task_type);
                return Ok(cached_result.clone());
            }
        }
        // 缓存未命中或过期
        // WHY 滞后记录 miss:阈值估算需用本次激活前的历史命中率统计,
        // 若先 record_miss 再估算,首次激活时 miss_rate=1.0 会异常抬升阈值,
        // 导致冷启动首个任务难以激活任何专家。miss 在结果计算后补记。

        // 步骤 2-4:持读锁完成门控计算与冲突消解
        // WHY 块作用域:确保 RwLockReadGuard 在 await 之前释放(clippy::await_holding_lock)
        let result = {
            let registry = self.expert_registry.read().unwrap_or_else(|p| {
                warn!("expert_registry read lock poisoned, recovering with inner data");
                p.into_inner()
            });
            let norms = self.expert_norms.read().unwrap_or_else(|p| {
                warn!("expert_norms read lock poisoned, recovering with inner data");
                p.into_inner()
            });

            // 动态阈值:基于当前注册表规模 + 缓存命中率估算负载因子
            let load_factor = self.estimate_load_factor(registry.len());
            let threshold = self.dynamic_threshold(load_factor);

            // 预计算 task 前缀范数(一次):
            // 按注册表中最短专家向量维度计算,保证与 compute_gate_value_with_norms
            // 的 min-length 前缀语义一致(项目约定 CLV 512 维 ≥ 专家向量 64 维)。
            let min_expert_dim = registry
                .values()
                .map(|p| p.expert_vector.len())
                .min()
                .unwrap_or(0);
            let task_norm = prefix_l2_norm(&task.clv, min_expert_dim);

            // 步骤 3:计算门控值,筛选候选
            // 热路径:使用预计算范数(专家范数注册时缓存),避免每次激活重复
            // 计算 2× 范数 + sqrt;维度异常时函数内部自动回退精确路径。
            let mut candidates: Vec<(ExpertId, f32)> = Vec::new();
            for (expert_id, profile) in registry.iter() {
                let expert_norm = norms.get(expert_id).map(|n| n.l2_norm).unwrap_or(0.0);
                let gate = compute_gate_value_with_norms(
                    task,
                    task_norm,
                    profile,
                    expert_norm,
                    &self.config,
                );
                if gate >= threshold {
                    candidates.push((expert_id.clone(), gate));
                }
            }

            // 步骤 4:冲突消解(复用同一读锁 + 预计算范数,避免二次加锁与重算)
            resolve_conflicts_with_norms(candidates, &registry, &norms, &self.config)?
        }; // 读锁在此释放,后续 await 不持锁

        // 步骤 5:写缓存(LRU 驱逐),key 为 TaskProfile 克隆(零序列化)
        self.write_cache(task.clone(), result.clone());
        self.cache_stats.record_miss();

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

    /// 估算负载因子:注册表规模(60%) + 缓存未命中率(40%)
    ///
    /// WHY 融合未命中率(专家 Agent 优化 2026-08-11):纯规模启发式无法反映
    /// 真实负载——任务多样性高(未命中率高)时即使专家数少也可能资源紧张。
    /// 语义自洽:未命中率高(任务多样)→ 负载高 → 阈值高 → 激活更少专家,
    /// 防止资源争抢;命中率高(重复任务)→ 负载低 → 阈值低 → 覆盖更多专家。
    /// 冷启动(尚未激活)时未命中率无统计意义,按 0 处理,行为与旧版一致。
    ///
    /// 规模启发式(旧):专家数 < 10 时负载低(0.0-0.3),10-50 时中等
    /// (0.3-0.7),> 50 时高(0.7-1.0)。后续可替换为基于 CPU/内存的真实负载。
    fn estimate_load_factor(&self, registry_len: usize) -> f32 {
        let count = registry_len;
        let scale = if count <= 10 {
            count as f32 / 10.0 * 0.3
        } else if count <= 50 {
            0.3 + (count - 10) as f32 / 40.0 * 0.4
        } else {
            (0.7 + (count - 50) as f32 / 50.0 * 0.3).min(1.0)
        };
        // 冷启动(尚无激活统计):未命中率无统计意义,纯规模因子(与旧行为一致)
        if self.cache_stats.total() == 0 {
            return scale;
        }
        // 未命中率(历史统计):命中率高 → 负载低;未命中率高 → 负载高
        let miss_rate = 1.0 - self.cache_stats.hit_rate();
        (0.6 * scale + 0.4 * miss_rate).clamp(0.0, 1.0)
    }

    /// 写缓存,执行 LRU 驱逐
    fn write_cache(&self, key: TaskProfile, result: ActivationResult) {
        // LRU 驱逐:超过容量时移除最早的条目
        if self.activation_cache.len() >= self.config.cache_capacity {
            self.evict_oldest();
        }
        self.activation_cache.insert(key, (result, Instant::now()));
    }

    /// 驱逐一个缓存条目(近似 LRU)
    ///
    /// WHY 采样近似而非全遍历(L9 优化 2.2):旧实现遍历整个 DashMap 找
    /// 严格最旧条目——容量满后每次插入触发 O(n) 全扫描,且循环内对
    /// "当前最旧" key 反复 clone(单调递减序列最坏 O(n) 次深拷贝,含 64 维
    /// f32 向量)。改为 Redis 风格采样近似 LRU:只检查迭代器前
    /// `self.evict_sample_size` 个条目,驱逐其中最旧者,复杂度降为 O(sample),
    /// 且全程只 clone 一次 key(最终选中者)。
    ///
    /// # 近似语义(WHY 可接受)
    /// activate() 已在读取时按 TTL 过滤过期条目,evict 仅在容量满时触发,
    /// 无需严格 LRU;采样近似是缓存驱逐的行业标准(Redis maxmemory-policy)。
    /// DashMap 的 remove 会改变内部分片布局,使后续采样窗口自然轮换,
    /// 避免固定驱逐同一批条目。
    ///
    /// # 采样数动态调整
    /// 采样数由 `compute_evict_sample_size()` 根据 `cache_capacity` 动态计算，
    /// 使用 sqrt(capacity) 策略，范围 [4, 32]。
    fn evict_oldest(&self) {
        let mut oldest_key: Option<TaskProfile> = None;
        let mut oldest_time = Instant::now();

        // 只采样前 self.evict_sample_size 个条目(O(sample) 替代 O(n) 全遍历)
        for entry in self.activation_cache.iter().take(self.evict_sample_size) {
            let (_, written_at) = entry.value();
            if *written_at <= oldest_time {
                oldest_time = *written_at;
                // WHY 循环内仍可能多次 clone,但采样上限 self.evict_sample_size
                // 使其为 O(sample) 常数级,远优于旧版最坏 O(n)
                oldest_key = Some(entry.key().clone());
            }
        }

        if let Some(key) = oldest_key {
            // key 是 owned TaskProfile,remove 仅借用不移走所有权,debug 可直接读 key.task_type
            self.activation_cache.remove(&key);
            debug!("GEA cache evicted task_type={}", key.task_type);
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
            .unwrap_or_else(|p| {
                warn!("expert_registry read lock poisoned, recovering with inner data");
                p.into_inner()
            })
            .len()
    }

    /// 上报专家激活结果反馈(成功/失败 + 延迟) — 能力画像闭环入口
    ///
    /// 由下游任务执行方调用:任务成功时 `success=true`,失败时 `success=false`。
    /// 反馈进入专家画像统计,经 `ExpertProfile::confidence()` 影响后续门控值——
    /// 高成功率专家更易被激活(Ω-Evolve 闭环)。
    ///
    /// 专家不存在时静默忽略(幂等,注册表为读多写少场景,不为此加锁热路径)。
    pub fn record_expert_outcome(&self, expert_id: &ExpertId, success: bool, latency_ms: f32) {
        let mut registry = self.expert_registry.write().unwrap_or_else(|p| {
            warn!("expert_registry write lock poisoned, recovering with inner data");
            p.into_inner()
        });
        if let Some(profile) = registry.get_mut(expert_id) {
            profile.record_outcome(success, latency_ms);
        }
    }

    /// 查询专家历史成功率(测试与监控用);专家不存在返回 None
    pub fn expert_success_rate(&self, expert_id: &ExpertId) -> Option<f32> {
        let registry = self.expert_registry.read().unwrap_or_else(|p| {
            warn!("expert_registry read lock poisoned, recovering with inner data");
            p.into_inner()
        });
        registry.get(expert_id).map(|p| p.success_rate())
    }
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
    fn test_estimate_load_factor_cold_start_matches_scale_only() {
        // 冷启动(无激活统计):未命中率不参与,负载因子 = 规模因子
        let activator = make_activator();
        // 1 个专家:0.1×0.3 = 0.03;5 个:0.5×0.3 = 0.15;10 个:0.3
        let lf = activator.estimate_load_factor(5);
        assert!(
            (lf - 0.15).abs() < 1e-6,
            "冷启动 5 专家负载因子应=0.15, got {lf}"
        );
    }

    #[tokio::test]
    async fn test_estimate_load_factor_blends_miss_rate() {
        // 激活后未命中率参与:全未命中(命中率 0)→ 负载因子被 40% 未命中率抬升
        let activator = make_activator();
        activator.register_expert(make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]));
        // 两次不同任务 → 全部未命中
        let t1 = make_task(0.8, "code-gen");
        let t2 = make_task(0.8, "refactor");
        let _ = activator.activate(&t1).await.unwrap();
        let _ = activator.activate(&t2).await.unwrap();
        assert_eq!(activator.cache_hit_rate(), 0.0);
        // 规模因子(1 专家)= 0.03,未命中率 = 1.0
        // load = 0.6×0.03 + 0.4×1.0 = 0.418
        let lf = activator.estimate_load_factor(1);
        assert!(
            (lf - 0.418).abs() < 1e-5,
            "全未命中时负载因子应=0.418, got {lf}"
        );
    }

    #[tokio::test]
    async fn test_estimate_load_factor_high_hit_rate_lowers_load() {
        // 高命中率场景:重复任务 → 未命中率低 → 负载因子贴近规模因子
        let activator = make_activator();
        activator.register_expert(make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]));
        let t = make_task(0.8, "code-gen");
        let _ = activator.activate(&t).await.unwrap(); // miss
        let _ = activator.activate(&t).await.unwrap(); // hit
        let _ = activator.activate(&t).await.unwrap(); // hit
        assert!(activator.cache_hit_rate() > 0.5, "命中率应 > 0.5");
        let lf_high_hit = activator.estimate_load_factor(1);
        let lf_cold = { 0.6 * 0.03 + 0.4 * (1.0 - activator.cache_hit_rate()) };
        assert!(
            (lf_high_hit - lf_cold).abs() < 1e-6,
            "负载因子应与公式一致: {lf_high_hit} vs {lf_cold}"
        );
        // 高命中率负载必须低于全未命中场景(0.418)
        assert!(lf_high_hit < 0.418);
    }

    #[tokio::test]
    async fn test_activate_uses_norm_cached_gate_path() {
        // 回归:范数缓存路径下激活结果与注册表一致性(激活仍正常发生)
        let activator = make_activator();
        activator.register_expert(make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]));
        activator.register_expert(make_expert("e-2", vec![0.5; 64], 0.8, vec!["refactor"]));
        let task = make_task(0.9, "code-gen");
        let result = activator.activate(&task).await.unwrap();
        assert!(result.has_activated());
        // 注销后范数缓存同步移除,重新激活不 panic
        activator.unregister_expert(&ExpertId::new("e-1"));
        let result2 = activator.activate(&task).await.unwrap();
        assert!(result2.has_activated());
    }

    #[tokio::test]
    async fn test_record_outcome_updates_success_rate() {
        // 能力画像闭环:反馈上报后成功率/门控 confidence 生效
        let activator = make_activator();
        let id = ExpertId::new("e-1");
        activator.register_expert(make_expert("e-1", vec![0.5; 64], 0.8, vec!["code-gen"]));

        // 无反馈时成功率 = 0.5 中性
        assert_eq!(activator.expert_success_rate(&id), Some(0.5));

        // 上报 8 次成功 + 2 次失败 → 成功率 0.8
        for _ in 0..8 {
            activator.record_expert_outcome(&id, true, 12.0);
        }
        for _ in 0..2 {
            activator.record_expert_outcome(&id, false, 30.0);
        }
        let rate = activator.expert_success_rate(&id).expect("专家应存在");
        assert!((rate - 0.8).abs() < 1e-6, "成功率应=0.8, got {rate}");

        // 未注册专家反馈静默忽略
        activator.record_expert_outcome(&ExpertId::new("missing"), true, 1.0);
        assert_eq!(
            activator.expert_success_rate(&ExpertId::new("missing")),
            None
        );
    }

    #[tokio::test]
    async fn test_confidence_raises_gate_for_high_success_expert() {
        // w4 启用时:高成功率专家门控值应高于无反馈专家
        let config = GeaConfig {
            w1: 0.3,
            w2: 0.2,
            w3: 0.2,
            w4_confidence: 0.3,
            ..Default::default()
        };
        let bus = EventBus::new();
        let activator = GeaActivator::new(config, bus).unwrap();
        let id_high = ExpertId::new("e-high");
        let id_fresh = ExpertId::new("e-fresh");
        activator.register_expert(make_expert("e-high", vec![0.5; 64], 0.8, vec!["code-gen"]));
        activator.register_expert(make_expert("e-fresh", vec![0.5; 64], 0.8, vec!["code-gen"]));

        // 高成功率专家:10 次全成功 → confidence = 1.0
        for _ in 0..10 {
            activator.record_expert_outcome(&id_high, true, 10.0);
        }
        // 新鲜专家:无反馈 → confidence = 0.5

        let task = make_task(0.9, "code-gen");
        let result = activator.activate(&task).await.unwrap();
        // 高成功率专家必须被激活(confidence 加分使其门控值更高)
        assert!(
            result.activated.contains(&id_high),
            "高成功率专家应优先激活: {:?}",
            result.activated
        );
        // 高成功率专家应排在新鲜专家之前(评分更高)
        let pos_high = result.activated.iter().position(|id| id == &id_high);
        let pos_fresh = result.activated.iter().position(|id| id == &id_fresh);
        if let (Some(ph), Some(pf)) = (pos_high, pos_fresh) {
            assert!(ph < pf, "高成功率专家应排在前面");
        }
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

    #[test]
    fn test_compute_evict_sample_size_default() {
        // 默认容量 128 时 sample_size = 11
        assert_eq!(compute_evict_sample_size(128), 11);
    }

    #[test]
    fn test_compute_evict_sample_size_larger() {
        // 容量 512 时 sample_size = 22
        assert_eq!(compute_evict_sample_size(512), 22);
    }

    #[test]
    fn test_compute_evict_sample_size_min() {
        // 容量 1 时 sample_size = 4（最小值）
        assert_eq!(compute_evict_sample_size(1), 4);
    }

    #[test]
    fn test_compute_evict_sample_size_max() {
        // 容量 10000 时 sample_size = 32（最大值）
        assert_eq!(compute_evict_sample_size(10000), 32);
    }

    #[test]
    fn test_activator_evict_sample_size_from_config() {
        // 验证 GeaActivator 的 evict_sample_size 字段与配置容量一致
        let config = GeaConfig::default();
        let activator = GeaActivator::new(config, EventBus::new()).unwrap();
        assert_eq!(activator.evict_sample_size, 11);
    }

    #[test]
    fn test_activator_evict_sample_size_custom() {
        // 验证自定义容量时采样数正确
        let config = GeaConfig {
            cache_capacity: 512,
            ..Default::default()
        };
        let activator = GeaActivator::new(config, EventBus::new()).unwrap();
        assert_eq!(activator.evict_sample_size, 22);
    }
}
