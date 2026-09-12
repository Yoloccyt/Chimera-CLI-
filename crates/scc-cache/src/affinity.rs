//! 缓存亲和 — 显式 cache_control 断点规划 + 隐式会话粘性(MCA A3,ADR-065)
//!
//! 对应架构层:L3 Storage(scc-cache)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.2 缓存亲和
//!
//! # 两族缓存机制
//! 1. **显式控制族**(Anthropic 路径 `cache_control`:GLM/Kimi/MiniMax):
//!    scc-cache 的推测预取结果直接打 `cache_control` 断点,断点位置为两大
//!    稳定前缀——系统提示 + repo-wiki 检索结果。
//! 2. **隐式自动族**(DeepSeek 上下文缓存自动命中、豆包缓存命中价):
//!    路由层做**会话粘性**——同一 Quest 的后续轮次优先路由到同厂商同通道,
//!    最大化隐式缓存命中率。
//!
//! # 依赖方向(§2.2 铁律)
//! 本模块依赖 L0 `nexus_contracts::affinity::CacheSupport`(L3 → L0 合法),
//! **不依赖** L10 mca-gateway(L3 → L10 为向上依赖,禁止)。context_window/
//! CacheSupport 经 event-bus 事件流入,或由调用方直接传入。

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::affinity::CacheSupport;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Anthropic cache_control 断点最小可缓存 token 数
///
/// WHY 1024: Anthropic 要求单个 cache_control 断点覆盖的前缀 ≥ 1024 token
/// 才生效(短前缀打断点无缓存收益,反增协议开销)。
pub const MIN_CACHEABLE_TOKENS: u32 = 1024;

/// 稳定前缀种类 — 显式缓存断点的候选位置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixKind {
    /// 系统提示(会话级最稳定前缀)
    SystemPrompt,
    /// repo-wiki 检索结果(同 Quest 内稳定,ISCM 跨层共享索引产出)
    RepoWikiRetrieval,
}

/// 一段稳定前缀(种类 + token 长度)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePrefix {
    /// 前缀种类
    pub kind: PrefixKind,
    /// 该前缀的 token 长度
    pub token_len: u32,
}

/// cache_control 断点 — 在某前缀之后打断点,携带累计 token(命中判据)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheBreakpoint {
    /// 断点位于哪个前缀之后
    pub after_prefix: PrefixKind,
    /// 断点处累计 token 数(前缀链累加)
    pub cumulative_tokens: u32,
}

/// 为显式缓存族规划 cache_control 断点
///
/// 仅 `ExplicitControl` 族产出断点;`None`/`Implicit` 族返回空(隐式族靠
/// 会话粘性,不打显式断点)。断点打在累计 token ≥ 阈值的稳定前缀边界。
///
/// # WHY 累计而非单前缀判定
/// Anthropic 缓存命中是"最长公共前缀"语义:断点覆盖从头到断点的全部内容。
/// 系统提示(短)+ repo-wiki(长)累计后才够阈值时,断点打在 repo-wiki 后
/// 一次性覆盖两者,比逐前缀打更省断点配额(Anthropic 最多 4 个断点)。
pub fn plan_breakpoints(
    cache_support: CacheSupport,
    prefixes: &[CachePrefix],
) -> Vec<CacheBreakpoint> {
    if cache_support != CacheSupport::ExplicitControl {
        return Vec::new();
    }
    let mut breakpoints = Vec::new();
    let mut cumulative = 0u32;
    for prefix in prefixes {
        cumulative = cumulative.saturating_add(prefix.token_len);
        // 累计过阈值即在此前缀后打断点(覆盖前面全部稳定内容)
        if cumulative >= MIN_CACHEABLE_TOKENS {
            breakpoints.push(CacheBreakpoint {
                after_prefix: prefix.kind,
                cumulative_tokens: cumulative,
            });
        }
    }
    breakpoints
}

/// 会话粘性跟踪器 — 隐式缓存族的同通道优先(A3 会话粘性)
///
/// 同一 Quest 的后续轮次优先路由到上一轮的同厂商同通道,最大化隐式
/// 缓存命中率(DeepSeek 上下文缓存 ¥0.01/M、豆包缓存命中价)。
///
/// # 线程安全
/// DashMap 分片锁,记录/查询均为同步原子操作,guard 立即释放(不跨 await,C7)。
#[derive(Default)]
pub struct SessionAffinityTracker {
    /// quest_id → 上一轮路由键(provider/model)
    sticky: DashMap<String, String>,
    /// WS-4B:可选事件总线 — 亲和策略应用时发布 `CacheAffinityApplied`
    event_bus: Option<EventBus>,
}

impl std::fmt::Debug for SessionAffinityTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionAffinityTracker")
            .field("sticky", &self.sticky)
            // event_bus:EventBus 未实现 Debug,不输出
            .finish()
    }
}

impl SessionAffinityTracker {
    /// 创建空跟踪器
    pub fn new() -> Self {
        Self::default()
    }

    /// 链式注入事件总线 — 亲和策略实际应用时发布 `CacheAffinityApplied`(WS-4B)。
    #[must_use]
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// 记录一次会话路由(仅隐式缓存族需要粘性)
    ///
    /// 显式族(ExplicitControl)靠 cache_control 断点,不依赖同通道,
    /// 记录也无害但不产生粘性收益;此处按 cache_support 过滤,只对隐式族记录。
    ///
    /// WS-4B:本方法即"亲和策略实际应用点"——无论哪个缓存族,都将当前
    /// 路由所应用的亲和策略经 `CacheAffinityApplied` 事件发布留痕。
    pub fn record(&self, quest_id: &str, route_key: &str, cache_support: CacheSupport) {
        if cache_support == CacheSupport::Implicit {
            self.sticky
                .insert(quest_id.to_string(), route_key.to_string());
        }
        self.notify_cache_affinity_applied(route_key, cache_support);
    }

    /// 发布 `CacheAffinityApplied` 事件 — 亲和策略实际应用点(L3 → L1,WS-4B)。
    ///
    /// `strategy` 用 `CacheAffinityIntegration::strategy_name` 归一;
    /// 显式族 `cache_control_injected=true`。未注入 EventBus 时静默跳过;
    /// 发布失败仅 warn,不影响会话粘性记录主语义。
    fn notify_cache_affinity_applied(&self, route_key: &str, cache_support: CacheSupport) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::CacheAffinityApplied {
                metadata: EventMetadata::new("scc-cache"),
                route_key: route_key.to_string(),
                strategy: CacheAffinityIntegration::strategy_name(cache_support).to_string(),
                cache_control_injected: cache_support == CacheSupport::ExplicitControl,
                breakpoint_count: 0,
            };
            if let Err(e) = bus.publish_blocking(event) {
                tracing::warn!(error = %e, "发布 CacheAffinityApplied 事件失败");
            }
        }
    }

    /// 查询会话的粘性通道(None = 首轮或非隐式族,无粘性偏好)
    pub fn preferred(&self, quest_id: &str) -> Option<String> {
        self.sticky.get(quest_id).map(|v| v.clone())
    }

    /// 会话结束清理粘性记录(避免内存泄漏)
    pub fn clear(&self, quest_id: &str) {
        self.sticky.remove(quest_id);
    }

    /// 当前跟踪的会话数（诊断用）
    pub fn tracked_sessions(&self) -> usize {
        self.sticky.len()
    }
}

/// 厂商缓存命中率跟踪 — 分厂商口径归一（ADR-069 Token 效率度量）
///
/// 消费 `StreamSessionCompleted.cache_hit_tokens` 与 `input_tokens`，
/// 累计分厂商命中率。命中率 = cached_tokens / total_input_tokens。
///
/// # 线程安全
/// DashMap 分片锁 + AtomicU64 无锁计数，guard 立即释放（不跨 await，C7）。
#[derive(Debug, Default)]
pub struct CacheHitTracker {
    /// provider_str → (hit_tokens, total_input_tokens)
    stats: DashMap<String, (AtomicU64, AtomicU64)>,
    /// 语义缓存命中次数（原子计数，无锁安全）
    semantic_hit_count: AtomicU64,
    /// 总请求次数（原子计数，无锁安全）
    total_request_count: AtomicU64,
}

impl CacheHitTracker {
    /// 创建空跟踪器
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次响应的缓存命中情况
    ///
    /// 由 VendorAdapter invoke() 闭环后调用（消费 UsageReport 字段）。
    pub fn record(&self, provider: &str, hit_tokens: u64, total_input_tokens: u64) {
        let entry = self
            .stats
            .entry(provider.to_string())
            .or_insert_with(|| (AtomicU64::new(0), AtomicU64::new(0)));
        let (hits, total) = entry.value();
        hits.fetch_add(hit_tokens, Ordering::Relaxed);
        total.fetch_add(total_input_tokens, Ordering::Relaxed);
    }

    /// 查询厂商缓存命中率（百分数 0-100；无数据 = 0）
    pub fn hit_rate_percent(&self, provider: &str) -> u8 {
        match self.stats.get(provider) {
            Some(entry) => {
                let (hits, total) = entry.value();
                let h = hits.load(Ordering::Relaxed);
                let t = total.load(Ordering::Relaxed);
                if t == 0 {
                    0
                } else {
                    // WHY checked_div: 避免 t==0 时除零（clippy manual_checked_division）
                    ((h * 100).checked_div(t).unwrap_or(0)).min(100) as u8
                }
            }
            None => 0,
        }
    }

    /// 查询厂商缓存命中率（浮点比率 0.0-1.0；input_tokens == 0 时返回 None）
    ///
    /// 与 `hit_rate_percent` 互补：本方法返回精确浮点比率，适合遥测管道聚合；
    /// `hit_rate_percent` 返回 u8 整数百分数，适合 TUI 面板展示。
    pub fn hit_rate(&self, provider: &str) -> Option<f32> {
        self.stats.get(provider).and_then(|entry| {
            let (hits, total) = entry.value();
            let h = hits.load(Ordering::Relaxed);
            let t = total.load(Ordering::Relaxed);
            if t == 0 {
                None
            } else {
                // WHY 钳制: 厂商返回异常(cache_hit > input)时命中率不超过 1.0
                Some((h as f32 / t as f32).min(1.0))
            }
        })
    }

    /// 批量查询所有厂商的缓存命中率（浮点比率 0.0-1.0）
    ///
    /// 返回 HashMap<provider_name, hit_rate>，仅包含 input_tokens > 0 的厂商。
    /// 空 tracker 或无有效数据时返回空 HashMap。
    pub fn all_hit_rates(&self) -> HashMap<String, f32> {
        let mut rates = HashMap::with_capacity(self.stats.len());
        for entry in self.stats.iter() {
            let (hits, total) = entry.value();
            let h = hits.load(Ordering::Relaxed);
            let t = total.load(Ordering::Relaxed);
            if t > 0 {
                let rate = (h as f32 / t as f32).min(1.0);
                rates.insert(entry.key().clone(), rate);
            }
        }
        rates
    }

    /// 全局缓存命中率（所有厂商汇总，百分数 0-100）
    pub fn global_hit_rate_percent(&self) -> u8 {
        let mut total_hits: u64 = 0;
        let mut total_input: u64 = 0;
        for entry in self.stats.iter() {
            let (hits, total) = entry.value();
            total_hits += hits.load(Ordering::Relaxed);
            total_input += total.load(Ordering::Relaxed);
        }
        if total_input == 0 {
            0
        } else {
            // WHY checked_div: 避免 total_input==0 时除零（clippy manual_checked_division）
            ((total_hits * 100).checked_div(total_input).unwrap_or(0)).min(100) as u8
        }
    }

    /// 已跟踪的厂商数（诊断用）
    pub fn tracked_providers(&self) -> usize {
        self.stats.len()
    }

    /// 记录一次语义缓存命中（原子递增）
    pub fn record_semantic_hit(&self) {
        self.semantic_hit_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次请求（原子递增）
    pub fn record_request(&self) {
        self.total_request_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 语义缓存命中率百分数（0-100）
    pub fn semantic_hit_rate_percent(&self) -> u8 {
        let hits = self.semantic_hit_count.load(Ordering::Relaxed);
        let total = self.total_request_count.load(Ordering::Relaxed);
        if total == 0 {
            0
        } else {
            ((hits * 100).checked_div(total).unwrap_or(0)).min(100) as u8
        }
    }

    /// 总请求数
    pub fn total_requests(&self) -> u64 {
        self.total_request_count.load(Ordering::Relaxed)
    }
}

/// 缓存亲和集成 — 统一缓存策略决策入口(MCA A3,ADR-065)
///
/// 为 mca-gateway codec 层提供三个决策:
/// 1. 是否注入 cache_control 断点(仅 ExplicitControl 族)
/// 2. 当前缓存策略描述(用于 CacheAffinityApplied 事件留痕)
/// 3. 预估缓存命中收益(路由调优用)
///
/// # 设计考量
/// 本类型是纯函数集合(无状态),不持有任何数据。三族缓存策略:
/// - None: 无缓存
/// - Implicit: 隐式自动缓存(厂商自动命中),无需注入 cache_control
/// - ExplicitControl: 显式控制(Anthropic 族 cache_control 断点)
#[derive(Debug, Clone, Copy)]
pub struct CacheAffinityIntegration;

impl CacheAffinityIntegration {
    /// 根据缓存支持度决定是否注入 cache_control 断点
    ///
    /// 仅 `ExplicitControl` 族需要注入;`None`/`Implicit` 族返回 false,
    /// 前者因无缓存,后者由厂商自动缓存不需显式注入。
    pub fn should_inject_cache_control(support: CacheSupport) -> bool {
        support == CacheSupport::ExplicitControl
    }

    /// 获取缓存策略的字符串描述(用于事件留痕和日志)
    ///
    /// 返回值是 `CacheAffinityApplied` 事件的 `strategy` 字段的枚举值:
    /// - "none": 无缓存
    /// - "implicit": 隐式自动缓存(厂商自动命中)
    /// - "explicit_control": 显式控制(cache_control 断点)
    pub fn strategy_name(support: CacheSupport) -> &'static str {
        match support {
            CacheSupport::None => "none",
            CacheSupport::Implicit => "implicit",
            CacheSupport::ExplicitControl => "explicit_control",
        }
    }

    /// 预估缓存命中收益(百分数,0-100)
    ///
    /// 用于路由调优的辅信号:
    /// - ExplicitControl: 80%(显式断点通常针对稳定前缀,命中率高)
    /// - Implicit: 50%(隐式自动缓存依赖会话粘性,中等命中率)
    /// - None: 0%(无缓存)
    ///
    /// WHY 硬编码而非动态: 动态命中率需要运行时统计(属 M1+ 度量),
    /// M0 阶段使用保守估值,后续由 `AffinityMetrics` 采集器替换。
    pub fn estimated_hit_rate(support: CacheSupport) -> u8 {
        match support {
            CacheSupport::ExplicitControl => 80,
            CacheSupport::Implicit => 50,
            CacheSupport::None => 0,
        }
    }

    /// 有效命中率 — 真实采集优先,无数据回落静态估值(ADR-072 决策 ⑦)
    ///
    /// CacheHitTracker 已采集分厂商真实命中率(`StreamSessionCompleted`
    /// 闭环回流),路由决策应消费真实数据而非静态估值:
    /// - tracker 有该 provider 数据 → 真实命中率(累计口径天然平滑)
    /// - tracker 无数据(首轮/冷启动) → 回落 `estimated_hit_rate` 静态估值
    ///
    /// WHY 真实优先: 静态估值(80%/50%/0%)是 M0 保守假设,与实测偏差
    /// 可达 30%+(如显式族实测 60% 而估值 80%),误导期望成本排序。
    pub fn effective_hit_rate(
        support: CacheSupport,
        provider: &str,
        tracker: &CacheHitTracker,
    ) -> u8 {
        let real = tracker.hit_rate_percent(provider);
        if real > 0 {
            real
        } else {
            Self::estimated_hit_rate(support)
        }
    }

    /// 期望成本 — 命中率感知的路由决策函数(ADR-072 决策 ⑦)
    ///
    /// E[cost] = (1-hit_rate)×input_price×input_tokens + output_price×output_tokens
    ///
    /// 整数微元运算(u64 中间值,禁止浮点——u64 大数百分比计算红线):
    /// 未命中输入按全价计,命中部分按缓存价计(命中率即缓存折扣的期望)。
    ///
    /// # 用途
    /// 多通道路由时选 E[cost] 最小的通道;粘性权重在期望成本平价时
    /// 作为偏向因子(显式族 = 0,隐式族按会话粘性偏好)。
    pub fn expected_cost(
        hit_rate_percent: u8,
        input_price_micro_per_mtok: u64,
        output_price_micro_per_mtok: u64,
        input_tokens: u64,
        output_tokens: u64,
    ) -> u64 {
        let hr = u64::from(hit_rate_percent.min(100));
        // 未命中比率 = (100 - hr)/100;整数运算避免浮点
        let uncached_input = (100 - hr)
            .saturating_mul(input_price_micro_per_mtok)
            .saturating_mul(input_tokens)
            / 100
            / 1_000_000;
        let output = output_price_micro_per_mtok.saturating_mul(output_tokens) / 1_000_000;
        uncached_input.saturating_add(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implicit_and_none_produce_no_explicit_breakpoints() {
        let prefixes = [
            CachePrefix {
                kind: PrefixKind::SystemPrompt,
                token_len: 2000,
            },
            CachePrefix {
                kind: PrefixKind::RepoWikiRetrieval,
                token_len: 5000,
            },
        ];
        assert!(plan_breakpoints(CacheSupport::Implicit, &prefixes).is_empty());
        assert!(plan_breakpoints(CacheSupport::None, &prefixes).is_empty());
    }

    #[test]
    fn explicit_breakpoints_on_stable_prefixes() {
        // 系统提示 2000 tok(过阈)+ repo-wiki 5000 tok(累计过阈)→ 两断点
        let prefixes = [
            CachePrefix {
                kind: PrefixKind::SystemPrompt,
                token_len: 2000,
            },
            CachePrefix {
                kind: PrefixKind::RepoWikiRetrieval,
                token_len: 5000,
            },
        ];
        let bps = plan_breakpoints(CacheSupport::ExplicitControl, &prefixes);
        assert_eq!(bps.len(), 2);
        assert_eq!(bps[0].after_prefix, PrefixKind::SystemPrompt);
        assert_eq!(bps[0].cumulative_tokens, 2000);
        assert_eq!(bps[1].after_prefix, PrefixKind::RepoWikiRetrieval);
        assert_eq!(bps[1].cumulative_tokens, 7000);
    }

    #[test]
    fn short_system_prompt_folds_into_repowiki_breakpoint() {
        // 系统提示 500 tok(不够阈)+ repo-wiki 800 tok(累计 1300 过阈)
        // → 只在 repo-wiki 后打一个断点,覆盖两者(省断点配额)
        let prefixes = [
            CachePrefix {
                kind: PrefixKind::SystemPrompt,
                token_len: 500,
            },
            CachePrefix {
                kind: PrefixKind::RepoWikiRetrieval,
                token_len: 800,
            },
        ];
        let bps = plan_breakpoints(CacheSupport::ExplicitControl, &prefixes);
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].after_prefix, PrefixKind::RepoWikiRetrieval);
        assert_eq!(bps[0].cumulative_tokens, 1300);
    }

    #[test]
    fn session_stickiness_only_for_implicit_family() {
        let tracker = SessionAffinityTracker::new();
        // 隐式族(DeepSeek):记录粘性
        tracker.record(
            "quest-1",
            "deep_seek/deepseek-v4-flash",
            CacheSupport::Implicit,
        );
        assert_eq!(
            tracker.preferred("quest-1").as_deref(),
            Some("deep_seek/deepseek-v4-flash")
        );
        // 显式族(GLM Anthropic):不产生粘性(靠 cache_control 断点)
        tracker.record("quest-2", "zhipu/glm-5.2", CacheSupport::ExplicitControl);
        assert!(tracker.preferred("quest-2").is_none());
    }

    #[test]
    fn session_stickiness_prefers_same_channel_across_turns() {
        let tracker = SessionAffinityTracker::new();
        // 首轮:无粘性
        assert!(tracker.preferred("q").is_none());
        // 记录首轮路由 → 后续轮次优先同通道(最大化隐式缓存命中)
        tracker.record("q", "deep_seek/deepseek-v4-flash", CacheSupport::Implicit);
        assert_eq!(
            tracker.preferred("q").as_deref(),
            Some("deep_seek/deepseek-v4-flash")
        );
        // 会话结束清理
        tracker.clear("q");
        assert!(tracker.preferred("q").is_none());
        assert_eq!(tracker.tracked_sessions(), 0);
    }

    /// WS-4B:CacheAffinityApplied 幽灵事件生产者验证 —
    /// 亲和策略实际应用(publish 留痕)时发布本事件。
    #[test]
    fn cache_affinity_applied_published_on_record() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let tracker = SessionAffinityTracker::new().with_event_bus(bus);

        tracker.record(
            "quest-1",
            "deep_seek/deepseek-v4-flash",
            CacheSupport::Implicit,
        );

        // publish_blocking 同步投递,try_recv 立即可取
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let event = loop {
            if let Ok(Some(e)) = rx.try_recv() {
                break e;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "接收 CacheAffinityApplied 事件超时"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };

        assert_eq!(
            event.metadata().source,
            "scc-cache",
            "CacheAffinityApplied 事件 source 应为 scc-cache"
        );
        match event {
            NexusEvent::CacheAffinityApplied {
                route_key,
                strategy,
                cache_control_injected,
                breakpoint_count,
                ..
            } => {
                assert_eq!(route_key, "deep_seek/deepseek-v4-flash");
                assert_eq!(strategy, "implicit");
                assert!(!cache_control_injected);
                assert_eq!(breakpoint_count, 0);
            }
            other => panic!("期望 CacheAffinityApplied 事件,收到 {other:?}"),
        }
    }

    #[test]
    fn cache_hit_tracker_records_and_computes_rate() {
        let tracker = CacheHitTracker::new();
        // 无数据时命中率 = 0
        assert_eq!(tracker.hit_rate_percent("zhipu"), 0);
        assert_eq!(tracker.global_hit_rate_percent(), 0);

        // 记录: 100 命中 / 200 总输入 = 50%
        tracker.record("zhipu", 100, 200);
        assert_eq!(tracker.hit_rate_percent("zhipu"), 50);

        // 追加: 150 命中 / 300 总输入 → 累计 250/500 = 50%
        tracker.record("zhipu", 150, 300);
        assert_eq!(tracker.hit_rate_percent("zhipu"), 50);

        // 多厂商: deepseek 80/100 = 80%
        tracker.record("deep_seek", 80, 100);
        assert_eq!(tracker.hit_rate_percent("deep_seek"), 80);

        // 全局: (250+80) / (500+100) = 330/600 = 55%
        assert_eq!(tracker.global_hit_rate_percent(), 55);
        assert_eq!(tracker.tracked_providers(), 2);
    }

    #[test]
    fn cache_hit_tracker_zero_total_no_panic() {
        let tracker = CacheHitTracker::new();
        tracker.record("test", 0, 0);
        assert_eq!(tracker.hit_rate_percent("test"), 0);
    }

    #[test]
    fn record_semantic_hit_and_request() {
        let tracker = CacheHitTracker::new();
        assert_eq!(tracker.total_requests(), 0);
        assert_eq!(tracker.semantic_hit_rate_percent(), 0);

        // 3 次请求，1 次语义命中
        tracker.record_request();
        tracker.record_request();
        tracker.record_semantic_hit();
        tracker.record_request();
        assert_eq!(tracker.total_requests(), 3);
        assert_eq!(tracker.semantic_hit_rate_percent(), 33);
    }

    #[test]
    fn semantic_hit_rate_percent() {
        let tracker = CacheHitTracker::new();
        // 无请求时命中率 = 0
        assert_eq!(tracker.semantic_hit_rate_percent(), 0);

        // 5 次请求，2 次命中 → 40%
        for _ in 0..5 {
            tracker.record_request();
        }
        tracker.record_semantic_hit();
        tracker.record_semantic_hit();
        assert_eq!(tracker.semantic_hit_rate_percent(), 40);
        assert_eq!(tracker.total_requests(), 5);
    }

    // ============================================================
    // hit_rate / all_hit_rates 分厂商口径归一（ADR-069 Task 3）
    // ============================================================

    #[test]
    fn hit_rate_returns_correct_float_ratio() {
        let tracker = CacheHitTracker::new();
        // 16 缓存命中 / 24 输入 → 命中率 0.666...
        tracker.record("deep_seek", 16, 24);
        let rate = tracker
            .hit_rate("deep_seek")
            .expect("input_tokens > 0 必须返回 Some");
        // 浮点比较: 16/24 ≈ 0.6667, 允许 ±0.001 误差
        assert!((rate - 0.6667).abs() < 0.001, "16/24 ≈ 0.6667, got {rate}");

        // 追加: 34 命中 / 76 输入 → 累计 50/100 = 0.5
        tracker.record("deep_seek", 34, 76);
        let rate2 = tracker.hit_rate("deep_seek").unwrap();
        assert!((rate2 - 0.5).abs() < 0.001, "50/100 = 0.5, got {rate2}");
    }

    #[test]
    fn hit_rate_clamps_at_one_when_cache_hit_exceeds_input() {
        // 厂商返回异常 cache_hit > input 时钳制为 1.0（不产生 >1.0 的荒谬值）
        let tracker = CacheHitTracker::new();
        tracker.record("anomaly_vendor", 999, 100);
        let rate = tracker
            .hit_rate("anomaly_vendor")
            .expect("input_tokens > 0 必须返回 Some");
        assert!(
            (rate - 1.0).abs() < f32::EPSILON,
            "钳制后必须为 1.0, got {rate}"
        );
    }

    #[test]
    fn hit_rate_input_tokens_zero_returns_none() {
        let tracker = CacheHitTracker::new();
        tracker.record("vendor", 0, 0);
        assert!(
            tracker.hit_rate("vendor").is_none(),
            "input_tokens == 0 时 hit_rate 必须返回 None"
        );
    }

    #[test]
    fn hit_rate_unrecorded_provider_returns_none() {
        let tracker = CacheHitTracker::new();
        // 未 record 过的厂商 → None（不是 Some(0.0)）
        assert!(
            tracker.hit_rate("nonexistent").is_none(),
            "未 record 的 provider 必须返回 None"
        );
    }

    #[test]
    fn all_hit_rates_returns_correct_map() {
        let tracker = CacheHitTracker::new();
        tracker.record("zhipu", 60, 100); // 0.6
        tracker.record("deep_seek", 80, 100); // 0.8
        tracker.record("minimax", 0, 50); // 0.0

        let rates = tracker.all_hit_rates();
        assert_eq!(rates.len(), 3, "三个厂商均有 input_tokens > 0");
        assert!((*rates.get("zhipu").unwrap() - 0.6).abs() < 0.001);
        assert!((*rates.get("deep_seek").unwrap() - 0.8).abs() < 0.001);
        assert!((*rates.get("minimax").unwrap() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn all_hit_rates_excludes_zero_input_tokens() {
        // input_tokens == 0 的厂商不进入结果集（除零保护）
        let tracker = CacheHitTracker::new();
        tracker.record("valid", 10, 20); // 0.5
        tracker.record("zero_input", 0, 0); // 无有效数据
        let rates = tracker.all_hit_rates();
        assert_eq!(rates.len(), 1, "只有 valid 进入结果集");
        assert!(
            !rates.contains_key("zero_input"),
            "input_tokens==0 不进入结果集"
        );
    }

    #[test]
    fn all_hit_rates_empty_tracker_returns_empty_map() {
        let tracker = CacheHitTracker::new();
        let rates = tracker.all_hit_rates();
        assert!(rates.is_empty(), "空 tracker 返回空 HashMap");
    }

    #[test]
    fn all_hit_rates_providers_independent() {
        // 各厂商命中率独立累计，互不干扰
        let tracker = CacheHitTracker::new();
        tracker.record("a", 10, 100); // 0.1
        tracker.record("b", 90, 100); // 0.9
        tracker.record("a", 40, 100); // a 累计 50/200 = 0.25
        let rates = tracker.all_hit_rates();
        assert!((*rates.get("a").unwrap() - 0.25).abs() < 0.001);
        assert!((*rates.get("b").unwrap() - 0.9).abs() < 0.001);
    }

    // ============================================================
    // 多厂商并发累计（tokio::spawn 模拟多个 invoke 同时调用 record）
    // ============================================================

    #[tokio::test]
    async fn concurrent_multi_vendor_record() {
        use std::sync::Arc;

        let tracker = Arc::new(CacheHitTracker::new());
        let mut handles = Vec::new();

        // 3 厂商 × 100 并发 record = 300 并发任务
        for v in 0..3 {
            let t = Arc::clone(&tracker);
            let provider = format!("vendor_{v}");
            handles.push(tokio::spawn(async move {
                for _ in 0..100 {
                    // 每次 record: 命中 50 / 输入 100 = 50% 命中率
                    t.record(&provider, 50, 100);
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // 每个厂商 100 次 × 50 命中 / 100 输入 = 5000 命中 / 10000 输入 = 50%
        for v in 0..3 {
            let rate = tracker
                .hit_rate(&format!("vendor_{v}"))
                .expect("并发累计后必须有数据");
            assert!(
                (rate - 0.5).abs() < 0.001,
                "vendor_{v}: expected 0.5, got {rate}"
            );
        }
        assert_eq!(tracker.tracked_providers(), 3);
    }

    #[tokio::test]
    async fn concurrent_record_total_consistency() {
        // 单厂商 1000 次并发 record → 累计值必须 = 1000 × 每次值
        use std::sync::Arc;

        let tracker = Arc::new(CacheHitTracker::new());
        let mut handles = Vec::new();

        // 10 个并发任务，每个 record 100 次
        // 每次 record: 命中 3 / 输入 10
        for _ in 0..10 {
            let t = Arc::clone(&tracker);
            handles.push(tokio::spawn(async move {
                for _ in 0..100 {
                    t.record("single", 3, 10);
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // 10 × 100 × 3 = 3000 命中 / 10 × 100 × 10 = 10000 输入 = 0.3
        let rate = tracker.hit_rate("single").expect("并发累计后必须有数据");
        assert!(
            (rate - 0.3).abs() < 0.001,
            "并发累计 3000/10000 = 0.3, got {rate}"
        );
        assert_eq!(tracker.tracked_providers(), 1);
    }

    // ============================================================
    // 真实命中率反馈路由(ADR-072 决策 ⑦)
    // ============================================================

    #[test]
    fn effective_hit_rate_prefers_real_over_estimate() {
        let tracker = CacheHitTracker::new();
        // 真实数据存在(60%)→ 覆盖显式族静态估值 80%
        tracker.record("zhipu", 60, 100);
        assert_eq!(
            CacheAffinityIntegration::effective_hit_rate(
                CacheSupport::ExplicitControl,
                "zhipu",
                &tracker,
            ),
            60,
            "真实命中率必须优先于静态估值"
        );
    }

    #[test]
    fn effective_hit_rate_falls_back_to_estimate_without_data() {
        // 冷启动(无真实数据)→ 回落静态估值(显式 80 / 隐式 50 / 无 0)
        let tracker = CacheHitTracker::new();
        assert_eq!(
            CacheAffinityIntegration::effective_hit_rate(
                CacheSupport::ExplicitControl,
                "zhipu",
                &tracker,
            ),
            80
        );
        assert_eq!(
            CacheAffinityIntegration::effective_hit_rate(
                CacheSupport::Implicit,
                "deep_seek",
                &tracker,
            ),
            50
        );
        assert_eq!(
            CacheAffinityIntegration::effective_hit_rate(CacheSupport::None, "step_fun", &tracker,),
            0
        );
    }

    #[test]
    fn expected_cost_decreases_with_hit_rate() {
        // 同通道:命中率越高期望成本越低(路由排序依据)
        let input_price = 4_000_000u64; // ¥4/M
        let output_price = 12_000_000u64; // ¥12/M
        let (input_tokens, output_tokens) = (100_000u64, 10_000u64);
        let c0 = CacheAffinityIntegration::expected_cost(
            0,
            input_price,
            output_price,
            input_tokens,
            output_tokens,
        );
        let c50 = CacheAffinityIntegration::expected_cost(
            50,
            input_price,
            output_price,
            input_tokens,
            output_tokens,
        );
        let c90 = CacheAffinityIntegration::expected_cost(
            90,
            input_price,
            output_price,
            input_tokens,
            output_tokens,
        );
        // 输出成本恒 120,000 微元;输入成本随命中率递减
        assert!(c0 > c50 && c50 > c90, "命中率越高期望成本必须越低");
        // 精确值:0% → 400000+120000 = 520000;50% → 200000+120000 = 320000;
        // 90% → 40000+120000 = 160000
        assert_eq!(c0, 520_000);
        assert_eq!(c50, 320_000);
        assert_eq!(c90, 160_000);
    }

    #[test]
    fn expected_cost_ranks_channels_for_routing() {
        // 多通道路由:命中率差异改变排序(DeepSeek 隐式 0.8 粘性 vs Qwen 0.7)
        // 通道 A:命中率 90%,输入价 ¥4/M;通道 B:命中率 50%,输入价 ¥2/M
        // 输出价相同 ¥12/M,输入 100K 输出 10K
        let a = CacheAffinityIntegration::expected_cost(90, 4_000_000, 12_000_000, 100_000, 10_000);
        let b = CacheAffinityIntegration::expected_cost(50, 2_000_000, 12_000_000, 100_000, 10_000);
        // A: 10%×4×100K = 40000 + 120000 = 160000
        // B: 50%×2×100K = 100000 + 120000 = 220000
        // 高命中率通道 A 更优(即使单价更贵)——命中率反馈修正路由决策
        assert!(a < b, "期望成本最小化必须选择命中率感知后的最优通道");
    }
}
