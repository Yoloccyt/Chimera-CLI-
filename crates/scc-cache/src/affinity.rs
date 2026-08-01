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
use nexus_contracts::affinity::CacheSupport;

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
#[derive(Debug, Default)]
pub struct SessionAffinityTracker {
    /// quest_id → 上一轮路由键(provider/model)
    sticky: DashMap<String, String>,
}

impl SessionAffinityTracker {
    /// 创建空跟踪器
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次会话路由(仅隐式缓存族需要粘性)
    ///
    /// 显式族(ExplicitControl)靠 cache_control 断点,不依赖同通道,
    /// 记录也无害但不产生粘性收益;此处按 cache_support 过滤,只对隐式族记录。
    pub fn record(&self, quest_id: &str, route_key: &str, cache_support: CacheSupport) {
        if cache_support == CacheSupport::Implicit {
            self.sticky
                .insert(quest_id.to_string(), route_key.to_string());
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

    /// 当前跟踪的会话数(诊断用)
    pub fn tracked_sessions(&self) -> usize {
        self.sticky.len()
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
}
