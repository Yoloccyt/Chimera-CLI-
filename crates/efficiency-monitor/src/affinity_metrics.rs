//! 亲和指标采集器 — MCA 体验对等不变量(A5/E1-E5)的度量采集
//!
//! 对应架构层:L9 Quest(efficiency-monitor)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.6 体验亲和不变量
//!
//! # 采集维度(A5 体验对等)
//! - **E1 TTFT**:首 token 延迟 p50/p95(消费 `StreamSessionCompleted.ttft_ms`)
//! - **成本速率**:每通道累计成本(微元)/ 会话数
//! - **缓存命中率**:cache_hit_tokens / input_tokens(隐式/显式缓存族统一口径)
//! - **特性启用率**(A2 分母):FullFidelity / (Full + Degraded)会话占比
//!   (消费 `AffinityCapabilityNegotiated.fidelity`)
//! - **降级计数**:`ProviderDegraded` 触发次数
//!
//! # 依赖方向(§2.2 铁律)
//! efficiency-monitor(L9)消费 event-bus(L1)的 MCA 事件——L9 → L1 合法,
//! **不依赖** L10 mca-gateway。所有 MCA 事件在 event-bus L1 定义。
//!
//! # 线程安全
//! 每通道统计走 `DashMap<String, Mutex<ChannelAffinityStats>>`;record 同步
//! 更新,collect 计算百分位,均不跨 await(C7)。TTFT 百分位用
//! `select_nth_unstable`(O(n),§4.1 Top-K 红线,禁 sort_by)。

use std::sync::Arc;

use dashmap::DashMap;

use crate::types::MetricSample;

/// TTFT 采样环容量(每通道保留最近 N 个样本供百分位计算)
const TTFT_RING_CAP: usize = 256;

/// 单通道亲和统计
#[derive(Debug, Default, Clone)]
struct ChannelAffinityStats {
    /// TTFT 采样环(最近 TTFT_RING_CAP 个,FIFO 覆盖)
    ttft_samples: Vec<u32>,
    /// 会话数
    sessions: u64,
    /// 累计输入 token
    input_tokens: u64,
    /// 累计缓存命中 token
    cache_hit_tokens: u64,
    /// 累计成本(微元)
    cost_micro: u64,
    /// FullFidelity 协商次数
    full_fidelity: u64,
    /// DegradedNotified 协商次数
    degraded: u64,
    /// ChannelRejected 协商次数
    rejected: u64,
    /// ProviderDegraded 触发次数
    degraded_events: u64,
}

impl ChannelAffinityStats {
    /// 推入 TTFT 样本(环形覆盖:满则移除最旧)
    fn push_ttft(&mut self, ttft_ms: u32) {
        if self.ttft_samples.len() >= TTFT_RING_CAP {
            self.ttft_samples.remove(0);
        }
        self.ttft_samples.push(ttft_ms);
    }

    /// 百分位(0.0-1.0)——用 select_nth_unstable O(n)(§4.1 Top-K 红线)
    fn percentile(&self, p: f64) -> Option<u32> {
        if self.ttft_samples.is_empty() {
            return None;
        }
        let mut buf = self.ttft_samples.clone();
        let n = buf.len();
        // 索引:ceil(p * n) - 1,钳制到 [0, n-1]
        let idx = (((p * n as f64).ceil() as usize).max(1) - 1).min(n - 1);
        // select_nth_unstable 把第 idx 小的元素放到位(O(n),不全排序)
        let (_, nth, _) = buf.select_nth_unstable(idx);
        Some(*nth)
    }

    /// 缓存命中率(命中 token / 输入 token;无输入返回 0)
    fn cache_hit_rate(&self) -> f64 {
        if self.input_tokens == 0 {
            0.0
        } else {
            self.cache_hit_tokens as f64 / self.input_tokens as f64
        }
    }

    /// 特性启用率(Full / (Full + Degraded);无协商返回 1.0 乐观)
    fn feature_enablement_rate(&self) -> f64 {
        let total = self.full_fidelity + self.degraded;
        if total == 0 {
            1.0
        } else {
            self.full_fidelity as f64 / total as f64
        }
    }
}

/// 亲和指标采集器 — 每通道体验度量(A5)
///
/// Clone 廉价(Arc 共享),可在事件订阅后台任务与主线程间共享。
///
/// WHY DashMap 直接承载 Stats(无内层 Mutex): DashMap 分片锁已提供
/// 写时独占访问(RefMut),内层 Mutex 冗余;所有 record/collect
/// 同步执行,guard 立即释放不跨 await(C7)。
#[derive(Clone, Default)]
pub struct AffinityMetrics {
    channels: Arc<DashMap<String, ChannelAffinityStats>>,
}

impl AffinityMetrics {
    /// 创建空采集器
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次会话闭环(消费 `StreamSessionCompleted`)
    pub fn record_session(
        &self,
        route_key: &str,
        ttft_ms: u32,
        cost_micro: u64,
        input_tokens: u64,
        cache_hit_tokens: u64,
    ) {
        let mut s = self.channels.entry(route_key.to_string()).or_default();
        s.push_ttft(ttft_ms);
        s.sessions += 1;
        s.input_tokens += input_tokens;
        s.cache_hit_tokens += cache_hit_tokens;
        s.cost_micro += cost_micro;
    }

    /// 记录一次能力协商(消费 `AffinityCapabilityNegotiated`)
    ///
    /// fidelity 取值:"full_fidelity" / "degraded_notified" / "channel_rejected"
    pub fn record_negotiation(&self, route_key: &str, fidelity: &str) {
        let mut s = self.channels.entry(route_key.to_string()).or_default();
        match fidelity {
            "full_fidelity" => s.full_fidelity += 1,
            "degraded_notified" => s.degraded += 1,
            "channel_rejected" => s.rejected += 1,
            _ => {}
        }
    }

    /// 记录一次通道降级(消费 `ProviderDegraded`)
    pub fn record_degraded(&self, route_key: &str) {
        let mut s = self.channels.entry(route_key.to_string()).or_default();
        s.degraded_events += 1;
    }

    /// 查询通道 TTFT 百分位(E1 验收数据源)
    pub fn ttft_percentile(&self, route_key: &str, p: f64) -> Option<u32> {
        self.channels.get(route_key).and_then(|s| s.percentile(p))
    }

    /// 查询通道缓存命中率
    pub fn cache_hit_rate(&self, route_key: &str) -> Option<f64> {
        self.channels.get(route_key).map(|s| s.cache_hit_rate())
    }

    /// 查询通道特性启用率(A2 达标判据 ≥ 0.95)
    pub fn feature_enablement_rate(&self, route_key: &str) -> Option<f64> {
        self.channels
            .get(route_key)
            .map(|s| s.feature_enablement_rate())
    }
}

impl crate::collectors::MetricCollector for AffinityMetrics {
    /// 产出全通道亲和指标样本(供 Prometheus 渲染与 TUI 面板消费)
    fn collect(&self) -> Vec<MetricSample> {
        let mut samples = Vec::new();
        for entry in self.channels.iter() {
            let route_key = entry.key().clone();
            let s = entry.value();
            let label = vec![("route".to_string(), route_key)];
            if let Some(p50) = s.percentile(0.50) {
                samples.push(MetricSample::new(
                    "mca_ttft_p50_ms",
                    p50 as f64,
                    label.clone(),
                ));
            }
            if let Some(p95) = s.percentile(0.95) {
                samples.push(MetricSample::new(
                    "mca_ttft_p95_ms",
                    p95 as f64,
                    label.clone(),
                ));
            }
            samples.push(MetricSample::new(
                "mca_cache_hit_rate",
                s.cache_hit_rate(),
                label.clone(),
            ));
            samples.push(MetricSample::new(
                "mca_feature_enablement_rate",
                s.feature_enablement_rate(),
                label.clone(),
            ));
            samples.push(MetricSample::new(
                "mca_cost_micro_total",
                s.cost_micro as f64,
                label.clone(),
            ));
            samples.push(MetricSample::new(
                "mca_sessions_total",
                s.sessions as f64,
                label.clone(),
            ));
            samples.push(MetricSample::new(
                "mca_provider_degraded_total",
                s.degraded_events as f64,
                label,
            ));
        }
        samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::MetricCollector;

    #[test]
    fn ttft_percentile_selects_correctly() {
        let m = AffinityMetrics::new();
        // 注入 100 个样本 1..=100 ms
        for i in 1..=100u32 {
            m.record_session("zhipu/glm-5.2", i, 0, 0, 0);
        }
        // p50 ≈ 50,p95 ≈ 95(select_nth_unstable 精确选择)
        assert_eq!(m.ttft_percentile("zhipu/glm-5.2", 0.50), Some(50));
        assert_eq!(m.ttft_percentile("zhipu/glm-5.2", 0.95), Some(95));
    }

    #[test]
    fn cache_hit_rate_accumulates() {
        let m = AffinityMetrics::new();
        m.record_session("deep_seek/deepseek-v4-flash", 100, 1000, 1000, 600);
        m.record_session("deep_seek/deepseek-v4-flash", 120, 1000, 1000, 400);
        // 总命中 1000 / 总输入 2000 = 0.5
        let rate = m.cache_hit_rate("deep_seek/deepseek-v4-flash").unwrap();
        assert!((rate - 0.5).abs() < 1e-6);
    }

    #[test]
    fn feature_enablement_rate_tracks_negotiation() {
        let m = AffinityMetrics::new();
        // 9 次全保真 + 1 次降级 → 启用率 0.9
        for _ in 0..9 {
            m.record_negotiation("zhipu/glm-5.2", "full_fidelity");
        }
        m.record_negotiation("zhipu/glm-5.2", "degraded_notified");
        let rate = m.feature_enablement_rate("zhipu/glm-5.2").unwrap();
        assert!((rate - 0.9).abs() < 1e-6);
    }

    #[test]
    fn feature_enablement_optimistic_when_no_negotiation() {
        let m = AffinityMetrics::new();
        m.record_session("x/y", 10, 0, 0, 0);
        // 无协商记录 → 乐观 1.0
        assert_eq!(m.feature_enablement_rate("x/y"), Some(1.0));
    }

    #[test]
    fn ttft_ring_bounded() {
        let m = AffinityMetrics::new();
        // 注入超过环容量的样本,p95 仍可计算(不 OOM)
        for i in 0..(TTFT_RING_CAP as u32 + 100) {
            m.record_session("x/y", i % 1000, 0, 0, 0);
        }
        assert!(m.ttft_percentile("x/y", 0.95).is_some());
    }

    #[test]
    fn collect_emits_per_channel_samples() {
        let m = AffinityMetrics::new();
        m.record_session("zhipu/glm-5.2", 150, 2000, 100, 80);
        m.record_negotiation("zhipu/glm-5.2", "full_fidelity");
        m.record_degraded("deep_seek/deepseek-v4-flash");
        let samples = m.collect();
        // 至少含 TTFT p50/p95、缓存命中率、启用率、成本、会话数、降级计数
        assert!(samples.iter().any(|s| s.name == "mca_ttft_p50_ms"));
        assert!(samples.iter().any(|s| s.name == "mca_cache_hit_rate"));
        assert!(samples
            .iter()
            .any(|s| s.name == "mca_provider_degraded_total"));
        // 标签含 route 维度
        assert!(samples
            .iter()
            .all(|s| s.labels.iter().any(|(k, _)| k == "route")));
    }
}
