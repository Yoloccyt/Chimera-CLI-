//! health — 通道健康探针(TTFT/成功率 EWMA,喂给 model-router 路由权重)
//!
//! # 度量(A5 体验亲和的数据源)
//! - **成功率 EWMA**:每次调用记 1.0(成功)/0.0(失败),EWMA α=0.1
//!   (对齐 ADR-037 既有机制),折算 health_score(0-100)
//! - **TTFT EWMA**:首 token 延迟毫秒的 EWMA,喂 E1 体验不变量验收
//! - **健康分**:主要由成功率驱动;TTFT 恶化只影响路由权重(不直接掉健康分),
//!   避免网络抖动误判通道不可用
//!
//! # 无锁设计
//! `ChannelHealth` 用 AtomicU64 位打包 f64(`to_bits`/`from_bits`)承载 EWMA,
//! CAS 循环更新;`HealthRegistry` 用 DashMap 承载每通道健康态(高频写,
//! 与网关 spec 的 ArcSwap 快照读写分离)。禁止持 DashMap guard 跨 await(C7)。
//!
//! # 与熔断器的关系
//! 熔断器(transport.rs `CircuitBreaker`)是**硬开关**(连续失败即拒绝放行);
//! 健康探针是**软权重**(EWMA 平滑,喂路由决策)。两者互补:熔断快速止血,
//! 健康分平滑降权。健康分低于阈值时网关发布 `ProviderDegraded`(Normal 级)。
//!
//! # ProviderDegraded 生产点([`DegradedPublisher`])
//!
//! `ProviderDegraded` 事件的唯一生产方:网关周期调用
//! [`DegradedPublisher::publish_degraded_if_any`],对**新恶化**的通道发布事件。
//! 去重 = 状态沿触发(rising edge):持续恶化不重发,恢复后清沿、再恶化再发布。
//! WHY 独立 wrapper 而非挂进 `record()`:`record` 是高频同步路径而
//! `EventBus::publish` 是 async——同步方法内无法 await,硬塞要么持锁跨 await
//! (C7 红线)要么 fire-and-forget spawn(反模式 §4.4-7);周期拉取让降级风暴
//! 天然节流,EWMA 基建([`ChannelHealth`]/[`HealthRegistry`])零改动复用。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use event_bus::{EventBus, EventMetadata, NexusEvent};

/// EWMA 平滑系数(α=0.1 承袭 ADR-037 原 acb-governor 设计;
/// 该 crate 已按 ADR-182 退役删除 2026-09-16)
const EWMA_ALPHA: f64 = 0.1;

/// 健康分降级阈值(< 该值时通道视为恶化,触发 ProviderDegraded)
pub const DEGRADED_THRESHOLD: u8 = 60;

/// 单通道健康态 — 无锁 EWMA(成功率 + TTFT)
///
/// 初始乐观:success_ewma=1.0(新通道默认健康,首次失败后快速回落)。
#[derive(Debug)]
pub struct ChannelHealth {
    /// 成功率 EWMA(f64 位打包;1.0=全成功)
    success_ewma_bits: AtomicU64,
    /// TTFT EWMA 毫秒(f64 位打包;0=无样本)
    ttft_ewma_bits: AtomicU64,
    /// 累计样本数(诊断/预热判断用)
    samples: AtomicU64,
}

impl Default for ChannelHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelHealth {
    /// 创建乐观初始态(成功率 1.0,无 TTFT 样本)
    pub fn new() -> Self {
        Self {
            success_ewma_bits: AtomicU64::new(1.0f64.to_bits()),
            ttft_ewma_bits: AtomicU64::new(0.0f64.to_bits()),
            samples: AtomicU64::new(0),
        }
    }

    /// EWMA 更新单个位打包 f64 原子量(CAS 循环,无锁)
    fn update_ewma(cell: &AtomicU64, sample: f64, first_sample: bool) {
        loop {
            let cur_bits = cell.load(Ordering::Acquire);
            let cur = f64::from_bits(cur_bits);
            // 首个 TTFT 样本直接取样本值(避免 0 基线拖低);成功率有乐观初值不需要
            let next = if first_sample {
                sample
            } else {
                EWMA_ALPHA * sample + (1.0 - EWMA_ALPHA) * cur
            };
            if cell
                .compare_exchange_weak(
                    cur_bits,
                    next.to_bits(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return;
            }
        }
    }

    /// 记录一次调用结果(成功 + TTFT,或失败)
    ///
    /// ttft_ms 仅在成功时有意义(失败无首 token);失败只更新成功率。
    pub fn record(&self, success: bool, ttft_ms: Option<u64>) {
        let prev_samples = self.samples.fetch_add(1, Ordering::AcqRel);
        Self::update_ewma(
            &self.success_ewma_bits,
            if success { 1.0 } else { 0.0 },
            false,
        );
        if let Some(ttft) = ttft_ms {
            // 首个 TTFT 样本(samples 从 0 起)直接取值,避免 0 基线
            let is_first_ttft = f64::from_bits(self.ttft_ewma_bits.load(Ordering::Acquire)) == 0.0
                && prev_samples == 0;
            Self::update_ewma(&self.ttft_ewma_bits, ttft as f64, is_first_ttft);
        }
    }

    /// 健康分(0-100,成功率 EWMA 折算)
    pub fn health_score(&self) -> u8 {
        let s = f64::from_bits(self.success_ewma_bits.load(Ordering::Acquire));
        (s.clamp(0.0, 1.0) * 100.0).round() as u8
    }

    /// TTFT EWMA(毫秒;E1 体验不变量验收数据源)
    pub fn ttft_ewma_ms(&self) -> f64 {
        f64::from_bits(self.ttft_ewma_bits.load(Ordering::Acquire))
    }

    /// 累计样本数
    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::Acquire)
    }

    /// 是否恶化(健康分低于阈值;需至少 1 样本,避免乐观初值误判)
    pub fn is_degraded(&self) -> bool {
        self.samples() > 0 && self.health_score() < DEGRADED_THRESHOLD
    }
}

/// 通道健康注册表 — 每路由键一个健康态(高频原子写)
#[derive(Debug, Default)]
pub struct HealthRegistry {
    channels: DashMap<String, ChannelHealth>,
}

impl HealthRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次调用(通道不存在则乐观初始化)
    ///
    /// WHY entry API 不跨 await: 整个记录是同步原子操作,guard 立即释放(C7)。
    pub fn record(&self, route_key: &str, success: bool, ttft_ms: Option<u64>) {
        self.channels
            .entry(route_key.to_string())
            .or_default()
            .record(success, ttft_ms);
    }

    /// 查询通道健康分(未记录过返回 None)
    pub fn health_score(&self, route_key: &str) -> Option<u8> {
        self.channels.get(route_key).map(|h| h.health_score())
    }

    /// 查询通道 TTFT EWMA(未记录过返回 None)
    pub fn ttft_ewma_ms(&self, route_key: &str) -> Option<f64> {
        self.channels.get(route_key).map(|h| h.ttft_ewma_ms())
    }

    /// 列出恶化通道(健康分低于阈值)——供网关发布 ProviderDegraded
    pub fn degraded_channels(&self) -> Vec<String> {
        self.channels
            .iter()
            .filter(|e| e.value().is_degraded())
            .map(|e| e.key().clone())
            .collect()
    }
}

/// 事件源标识(EventMetadata.source,对齐 adapters/hcw_integration 惯例)
const EVENT_SOURCE: &str = "mca-gateway";

/// ProviderDegraded 事件发布器 — 健康面唯一生产方(状态沿去重)
///
/// 持 [`HealthRegistry`] 引用周期扫描恶化通道,对**新恶化**的通道发布
/// `ProviderDegraded`(Normal 级,types.rs 事件契约:
/// route_key / reason / health_score)。
///
/// # 去重机制(状态沿,任务红线)
/// `announced` 集合记录"已发布过降级事件"的通道:
/// - 通道**首次**跨过 [`DEGRADED_THRESHOLD`] → `insert` 返回 true(上升沿)→ 发布;
/// - 通道**持续**恶化 → `insert` 返回 false → 不重发(每次 record 都发的风暴被节流);
/// - 通道**恢复**健康(分数回到阈值上,或记录消失)→ `retain` 清沿 →
///   下次再恶化可再次发布。
#[derive(Debug)]
pub struct DegradedPublisher {
    /// 健康注册表(与网关共享同一 EWMA 面,Arc 供周期任务 spawn 'static)
    registry: Arc<HealthRegistry>,
    /// 已发布降级事件的通道集合(状态沿状态)
    announced: DashSet<String>,
}

impl DegradedPublisher {
    /// 创建发布器(与网关共享同一 [`HealthRegistry`])
    pub fn new(registry: Arc<HealthRegistry>) -> Self {
        Self {
            registry,
            announced: DashSet::new(),
        }
    }

    /// 扫描恶化通道并对新恶化者发布 ProviderDegraded(由网关周期调用)
    ///
    /// 返回本次新发布的通道清单(观测面,调用方可留档/断言)。
    /// 发布失败不中断扫描(观测面事件,对齐 VendorAdapter::publish 先例);
    /// WHY 先 collect 再进 async 发布循环:DashMap 迭代/guard 不跨 await(C7)。
    pub async fn publish_degraded_if_any(&self, bus: &EventBus) -> Vec<String> {
        let degraded = self.registry.degraded_channels();
        let mut published = Vec::new();
        for route_key in degraded {
            // 状态沿:insert 返回 true = 集合里没有 = 上升沿(新恶化)才发布
            if !self.announced.insert(route_key.clone()) {
                continue;
            }
            // 恶化名单刚从 registry 查出,正常必有分数;并发清理时放弃本轮该通道
            let Some(score) = self.registry.health_score(&route_key) else {
                continue;
            };
            let event = NexusEvent::ProviderDegraded {
                metadata: EventMetadata::new(EVENT_SOURCE),
                route_key: route_key.clone(),
                reason: format!("health_score {score} < DEGRADED_THRESHOLD {DEGRADED_THRESHOLD}"),
                health_score: score,
            };
            let _ = bus.publish(event).await;
            published.push(route_key);
        }
        // 清沿:已宣布但分数已回到阈值上(或记录消失)的通道移出集合,
        // 保证"恢复 → 再恶化"能再次触发发布
        self.announced.retain(|key| {
            self.registry
                .health_score(key)
                .is_some_and(|score| score < DEGRADED_THRESHOLD)
        });
        published
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn new_channel_is_optimistically_healthy() {
        let h = ChannelHealth::new();
        assert_eq!(h.health_score(), 100);
        assert!(!h.is_degraded(), "无样本不判恶化(避免乐观初值误判)");
    }

    #[test]
    fn consecutive_failures_drop_health_below_threshold() {
        let h = ChannelHealth::new();
        // 连续失败:EWMA α=0.1,从 1.0 逐步回落
        for _ in 0..40 {
            h.record(false, None);
        }
        assert!(
            h.health_score() < DEGRADED_THRESHOLD,
            "持续失败后健康分应低于阈值,实际 {}",
            h.health_score()
        );
        assert!(h.is_degraded());
    }

    #[test]
    fn success_recovers_health() {
        let h = ChannelHealth::new();
        for _ in 0..40 {
            h.record(false, None);
        }
        assert!(h.is_degraded());
        // 恢复:连续成功拉回健康分
        for _ in 0..60 {
            h.record(true, Some(100));
        }
        assert!(
            !h.is_degraded(),
            "恢复后健康分应回升,实际 {}",
            h.health_score()
        );
    }

    #[test]
    fn ttft_ewma_tracks_first_token_latency() {
        let h = ChannelHealth::new();
        h.record(true, Some(200));
        // 首样本直接取值(避免 0 基线拖低)
        assert!((h.ttft_ewma_ms() - 200.0).abs() < 1.0);
        // 后续样本 EWMA 平滑
        h.record(true, Some(400));
        let ewma = h.ttft_ewma_ms();
        assert!(
            ewma > 200.0 && ewma < 400.0,
            "EWMA 应在两样本之间,实际 {ewma}"
        );
    }

    #[test]
    fn registry_tracks_multiple_channels() {
        let reg = HealthRegistry::new();
        for _ in 0..40 {
            reg.record("deep_seek/deepseek-v4-flash", false, None);
        }
        reg.record("zhipu/glm-5.2", true, Some(150));
        let degraded = reg.degraded_channels();
        assert!(degraded.contains(&"deep_seek/deepseek-v4-flash".to_string()));
        assert!(!degraded.contains(&"zhipu/glm-5.2".to_string()));
        assert_eq!(reg.health_score("zhipu/glm-5.2"), Some(100));
        assert!(reg.health_score("unknown/model").is_none());
    }

    /// 从事件流收取一条 ProviderDegraded(跳过流内其他事件,对齐 hcw_integration 先例)
    async fn recv_degraded(rx: &mut event_bus::EventReceiver) -> (String, String, u8) {
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for ProviderDegraded")
                .expect("recv error");
            if let NexusEvent::ProviderDegraded {
                route_key,
                reason,
                health_score,
                ..
            } = event
            {
                return (route_key, reason, health_score);
            }
        }
    }

    #[tokio::test]
    async fn degraded_publisher_fires_on_rising_edge_only() {
        // 状态沿:新恶化发布一次,持续恶化不重发(每次 record 都发的风暴被节流)
        let bus = EventBus::new();
        // WHY 先订阅再触发:broadcast 不缓存历史(§4.4 反模式 3)
        let mut rx = bus.subscribe();
        let reg = Arc::new(HealthRegistry::new());
        let publisher = DegradedPublisher::new(reg.clone());

        for _ in 0..40 {
            reg.record("deep_seek/deepseek-v4-flash", false, None);
        }

        // 第一轮:上升沿 → 发布 1 条,字段符合事件契约
        let published = publisher.publish_degraded_if_any(&bus).await;
        assert_eq!(published, vec!["deep_seek/deepseek-v4-flash".to_string()]);
        let (route_key, reason, score) = recv_degraded(&mut rx).await;
        assert_eq!(route_key, "deep_seek/deepseek-v4-flash");
        assert!(score < DEGRADED_THRESHOLD);
        assert!(
            reason.contains(&score.to_string()),
            "reason 应含当前分数,实际 {reason}"
        );

        // 继续恶化:状态沿不变 → 不重发
        reg.record("deep_seek/deepseek-v4-flash", false, None);
        let published = publisher.publish_degraded_if_any(&bus).await;
        assert!(published.is_empty(), "持续恶化不得重发,实际 {published:?}");
    }

    #[tokio::test]
    async fn degraded_publisher_rearms_after_recovery() {
        // 恢复清沿 → 再恶化再发布(清沿缺失 = 一次降级后永远静默)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let reg = Arc::new(HealthRegistry::new());
        let publisher = DegradedPublisher::new(reg.clone());

        for _ in 0..40 {
            reg.record("zhipu/glm-5.2", false, None);
        }
        assert_eq!(publisher.publish_degraded_if_any(&bus).await.len(), 1);
        recv_degraded(&mut rx).await;

        // 恢复:连续成功拉回健康分 → 无发布且清沿
        for _ in 0..60 {
            reg.record("zhipu/glm-5.2", true, Some(100));
        }
        let published = publisher.publish_degraded_if_any(&bus).await;
        assert!(published.is_empty(), "恢复后不得发布,实际 {published:?}");

        // 再次恶化:清沿已生效 → 再次发布
        for _ in 0..40 {
            reg.record("zhipu/glm-5.2", false, None);
        }
        let published = publisher.publish_degraded_if_any(&bus).await;
        assert_eq!(
            published,
            vec!["zhipu/glm-5.2".to_string()],
            "恢复后再恶化应重新发布"
        );
        recv_degraded(&mut rx).await;
    }

    #[tokio::test]
    async fn degraded_publisher_per_channel_independence() {
        // 多通道:各自独立沿;只恶化一个时只发一个
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let reg = Arc::new(HealthRegistry::new());
        let publisher = DegradedPublisher::new(reg.clone());

        for _ in 0..40 {
            reg.record("zhipu/glm-5.2", false, None);
        }
        for _ in 0..40 {
            reg.record("moonshot/kimi-k3", false, None);
        }
        let mut published = publisher.publish_degraded_if_any(&bus).await;
        published.sort();
        assert_eq!(
            published,
            vec!["moonshot/kimi-k3".to_string(), "zhipu/glm-5.2".to_string()]
        );
        recv_degraded(&mut rx).await;
        recv_degraded(&mut rx).await;

        // 其中一个恢复:另一个仍恶化者不重发,恢复者清沿
        for _ in 0..60 {
            reg.record("moonshot/kimi-k3", true, Some(100));
        }
        let published = publisher.publish_degraded_if_any(&bus).await;
        assert!(published.is_empty(), "沿状态互不干扰,实际 {published:?}");
    }
}
