//! adapters — spec 驱动的通用厂商适配器(VendorAdapter,零厂商 if 分支)
//!
//! # 设计裁决(ADR-065 决策 3)
//! VendorAdapter 是 **struct** 而非 enum:所有厂商行为差异均由
//! `ModelAffinitySpec`(数据)驱动,不存在第二个代码实现变体。
//! 方言差异由 `Codec` enum 分发承载(C4 红线)。
//!
//! # M0 职责边界
//! - `invoke()`: 非流式请求全周期(构造 → 传输 → 解码 → 成本回算 → 事件)
//! - 流式 `stream()`(bounded mpsc 数据面)与 session 状态守恒属 M1 交付
//!
//! # 事件纪律(C6)
//! 会话级事件(ModelAffinitySelected/StreamSessionCompleted/
//! AffinityQuotaExhausted)走 event-bus;数据面永不进 event-bus。

use std::sync::Arc;

use chrono::Timelike;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::affinity::{
    AffinityRequest, AffinityResponse, CostEstimate, ModelAffinitySpec, PricingSpec,
    ProtocolDialect, ProviderReceipt, UsageReport,
};

use crate::codec::Codec;
use crate::error::AffinityError;
use crate::transport::{CircuitBreaker, RateLimiter, Transport};

/// 事件源标识(EventMetadata.source)
const EVENT_SOURCE: &str = "mca-gateway";

/// spec 驱动的通用厂商适配器 — 一个实例服务一条通道(provider × model)
///
/// Clone 语义:内部全 Arc 共享(熔断/限流状态跨 Clone 一致,
/// 避免 csn-substitutor 教训的独立副本分叉)。
///
/// WHY 手写 Debug: `EventBus` 未实现 `Debug`,无法 derive;仅暴露
/// 通道标识与方言,不打印 bus/transport 内部状态。
#[derive(Clone)]
pub struct VendorAdapter {
    /// 通道描述符(能力协商唯一事实源)
    spec: Arc<ModelAffinitySpec>,
    /// 方言码器(由 spec.preferred_dialect 装配)
    codec: Codec,
    /// HTTP 传输(超时来自 spec.endpoint)
    transport: Arc<Transport>,
    /// 通道级熔断器(连续 5 次 5xx → 30s 半开)
    breaker: Arc<CircuitBreaker>,
    /// 通道级限流器(spec.rpm_limit 驱动)
    limiter: Arc<RateLimiter>,
    /// 事件总线(None = 静默模式,单测/录播回放用)
    bus: Option<EventBus>,
}

impl VendorAdapter {
    /// 装配适配器:方言选择 + 码器构造 + 传输初始化
    ///
    /// 方言回落(P3):preferred 方言无码器时(M0 无 Responses 码器),
    /// 顺序尝试 spec 声明的其余方言;全部不可用 → Capability 错误
    /// (ChannelRejected 语义,通道不进路由池)。
    pub fn assemble(
        spec: Arc<ModelAffinitySpec>,
        bus: Option<EventBus>,
    ) -> Result<Self, AffinityError> {
        let codec = Self::pick_codec(&spec)?;
        let transport = Arc::new(Transport::new(&spec.endpoint)?);
        let limiter = Arc::new(RateLimiter::from_rpm(spec.endpoint.rpm_limit));
        Ok(Self {
            spec,
            codec,
            transport,
            breaker: Arc::new(CircuitBreaker::new()),
            limiter,
            bus,
        })
    }

    /// 方言 → 码器装配(preferred 优先,声明序回落)
    fn pick_codec(spec: &ModelAffinitySpec) -> Result<Codec, AffinityError> {
        let mut candidates: Vec<ProtocolDialect> = Vec::with_capacity(spec.dialects.len());
        if let Some(preferred) = spec.preferred_dialect() {
            candidates.push(preferred);
        }
        for d in &spec.dialects {
            if !candidates.contains(d) {
                candidates.push(*d);
            }
        }
        candidates
            .into_iter()
            .find_map(Codec::for_dialect)
            .ok_or_else(|| AffinityError::Capability {
                provider: spec.provider.clone(),
                capability: "no usable protocol dialect codec".into(),
            })
    }

    /// 通道描述符只读访问
    pub fn spec(&self) -> &ModelAffinitySpec {
        &self.spec
    }

    /// 装配后的实际方言(路由留痕用)
    pub fn dialect(&self) -> ProtocolDialect {
        self.codec.dialect()
    }

    /// 非流式调用全周期:构造 → 传输 → 解码 → 成本回算 → 事件闭环
    pub async fn invoke(
        &self,
        request: &AffinityRequest,
    ) -> Result<AffinityResponse, AffinityError> {
        let route_key = self.spec.route_key();
        let started = std::time::Instant::now();

        // 1. 方言原生请求构造(P2 保真)
        let body = self.codec.build_request(&self.spec, request)?;

        // 2. 路由决策留痕(P6 成本先行:预估成本随事件发布)
        let estimate = estimate_cost(&self.spec.pricing, request, current_hour());
        self.publish(NexusEvent::ModelAffinitySelected {
            metadata: EventMetadata::new(EVENT_SOURCE),
            intent_id: request.intent_id.to_string(),
            route_key: route_key.clone(),
            dialect: dialect_str(self.dialect()).to_string(),
            cost_estimate_micro: estimate.total_micro,
            peak_factor_percent: estimate.peak_factor_percent,
        })
        .await;

        // 3. 传输(白名单 + 鉴权 + 重试 + 熔断 + 限流)
        let url = format!(
            "{}{}",
            self.spec.endpoint.base_url.trim_end_matches('/'),
            dialect_path(self.dialect())
        );
        Transport::check_allowlist(&self.spec.endpoint.base_url, &url)?;
        let headers = self.auth_headers()?;
        let resp = self
            .transport
            .post_json(
                &route_key,
                &url,
                &headers,
                &body,
                &self.breaker,
                &self.limiter,
            )
            .await?;

        // 4. 配额/协议错误分类(429 重试耗尽在 post_json 内;402/403 视为配额面)
        if resp.status == 402 || resp.status == 403 {
            let reason = String::from_utf8_lossy(&resp.body).into_owned();
            self.publish(NexusEvent::AffinityQuotaExhausted {
                metadata: EventMetadata::new(EVENT_SOURCE),
                route_key: route_key.clone(),
                reason: reason.chars().take(500).collect(),
            })
            .await;
            return Err(AffinityError::Quota { route_key, reason });
        }
        if resp.status >= 400 {
            return Err(AffinityError::Protocol {
                dialect: self.dialect(),
                reason: format!("HTTP {}: {}", resp.status, excerpt(&resp.body)),
            });
        }

        // 5. 解码 + 成本回算(真实 usage,整数微元)
        let decoded = self.codec.parse_response(&resp.body)?;
        let cost = actual_cost(&self.spec.pricing, &decoded.usage, current_hour());
        let ttft_ms = started.elapsed().as_millis() as u64;

        // 6. 会话闭环事件(成本回写 EWMA/缓存命中率/E1 度量的数据源)
        self.publish(NexusEvent::StreamSessionCompleted {
            metadata: EventMetadata::new(EVENT_SOURCE),
            intent_id: request.intent_id.to_string(),
            route_key,
            input_tokens: decoded.usage.input_tokens,
            output_tokens: decoded.usage.output_tokens,
            cache_hit_tokens: decoded.usage.cache_hit_tokens,
            cost_actual_micro: cost.total_micro,
            ttft_ms,
        })
        .await;

        Ok(AffinityResponse {
            blocks: decoded.blocks,
            usage: decoded.usage,
            cost,
            finish_reason: decoded.finish_reason,
            receipt: ProviderReceipt {
                provider: self.spec.provider.clone(),
                model: self.spec.model.clone(),
                dialect: self.dialect(),
                request_id: decoded.request_id,
            },
        })
    }

    /// 按方言构造鉴权头(密钥只从环境变量读取,不落日志)
    ///
    /// - OpenAI 族: `Authorization: Bearer {key}`
    /// - Anthropic 族: `x-api-key: {key}` + `anthropic-version`
    /// - `api_key_env` 为空(本地自部署): 不加鉴权头
    fn auth_headers(&self) -> Result<Vec<(String, String)>, AffinityError> {
        let env_name = self.spec.endpoint.api_key_env.as_ref();
        if env_name.is_empty() {
            return Ok(Vec::new());
        }
        let key = std::env::var(env_name).map_err(|_| AffinityError::Transport {
            route_key: self.spec.route_key(),
            reason: format!("api key env '{env_name}' not set"),
            retryable: false,
        })?;
        Ok(match self.dialect() {
            ProtocolDialect::AnthropicMessages => vec![
                ("x-api-key".into(), key),
                ("anthropic-version".into(), "2023-06-01".into()),
            ],
            ProtocolDialect::OpenAiChat | ProtocolDialect::OpenAiResponses => {
                vec![("Authorization".into(), format!("Bearer {key}"))]
            }
        })
    }

    /// 事件发布(bus 为 None 时静默,录播/单测模式)
    async fn publish(&self, event: NexusEvent) {
        if let Some(bus) = &self.bus {
            // WHY 忽略发布错误: 事件是观测面,发布失败(无订阅者等)不应
            // 中断请求主路径;Critical 事件由 bus 内部 mpsc 旁路保证送达
            let _ = bus.publish(event).await;
        }
    }
}

impl std::fmt::Debug for VendorAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VendorAdapter")
            .field("route_key", &self.spec.route_key())
            .field("dialect", &self.dialect())
            .field("breaker_open", &self.breaker.is_open())
            .finish_non_exhaustive()
    }
}

/// 响应体摘录(错误信息用,截断避免大 payload 污染日志)
fn excerpt(body: &[u8]) -> String {
    String::from_utf8_lossy(body).chars().take(200).collect()
}

/// 方言 → 端点路径(与厂商 base_url 约定配合,见 affinity.d 卡片注释)
fn dialect_path(dialect: ProtocolDialect) -> &'static str {
    match dialect {
        ProtocolDialect::OpenAiChat => "/chat/completions",
        ProtocolDialect::AnthropicMessages => "/v1/messages",
        ProtocolDialect::OpenAiResponses => "/responses",
    }
}

/// 方言 → 事件留痕字符串(serde snake_case 同形)
fn dialect_str(dialect: ProtocolDialect) -> &'static str {
    match dialect {
        ProtocolDialect::OpenAiChat => "open_ai_chat",
        ProtocolDialect::AnthropicMessages => "anthropic_messages",
        ProtocolDialect::OpenAiResponses => "open_ai_responses",
    }
}

/// 当前小时(0-23,厂商计费时区按本地时钟近似)
fn current_hour() -> u8 {
    chrono::Local::now().hour() as u8
}

/// 峰谷系数查表(小时桶 O(1),命中首条规则;无规则 = 100%)
fn peak_factor(pricing: &PricingSpec, hour: u8) -> u16 {
    for p in &pricing.peak_periods {
        // 常规区间 [start, end);跨零点规则由配置拆两条(spec_loader 文档约定)
        if p.start_hour <= hour && hour < p.end_hour {
            return p.factor_percent;
        }
    }
    100
}

/// 请求前成本预估 — 粗粒度字符启发式(P6:路由决策必须附带预估)
///
/// WHY 字符/4 启发式: 请求前无真实 token 数,中英文混排下 1 token ≈ 3-4
/// 字符是业界通用近似;预估只用于路由权重与预算预检,实际成本以
/// usage 回算为准(actual_cost),偏差由 acb-governor EWMA 自校正。
fn estimate_cost(pricing: &PricingSpec, request: &AffinityRequest, hour: u8) -> CostEstimate {
    let chars: usize = request
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .map(|b| match b {
            nexus_contracts::affinity::ContentBlock::Text { text } => text.len(),
            nexus_contracts::affinity::ContentBlock::Thinking { thinking, .. } => thinking.len(),
            nexus_contracts::affinity::ContentBlock::ToolUse { input_json, .. } => input_json.len(),
            nexus_contracts::affinity::ContentBlock::ToolResult { content, .. } => content.len(),
        })
        .sum();
    let est_input_tokens = (chars / 4) as u64;
    let factor = peak_factor(pricing, hour);
    // 整数微元:tokens × 价(微元/百万) / 1M × 峰谷百分比 / 100
    let total =
        est_input_tokens * pricing.input_micro_per_mtok / 1_000_000 * u64::from(factor) / 100;
    CostEstimate {
        total_micro: total,
        peak_factor_percent: factor,
        cache_discount_micro: 0,
    }
}

/// 真实成本回算 — usage 三元组 × 定价(缓存命中按折扣价计)
fn actual_cost(pricing: &PricingSpec, usage: &UsageReport, hour: u8) -> CostEstimate {
    let factor = u64::from(peak_factor(pricing, hour));
    // 缓存命中部分按 cache_hit 价,未命中输入按全价(DeepSeek/豆包计费口径)
    let cached = usage.cache_hit_tokens.min(usage.input_tokens);
    let uncached = usage.input_tokens - cached;
    let input_cost = uncached * pricing.input_micro_per_mtok / 1_000_000;
    let cache_cost = cached * pricing.cache_hit_micro_per_mtok / 1_000_000;
    let output_cost = usage.output_tokens * pricing.output_micro_per_mtok / 1_000_000;
    let full_price_would_be = usage.input_tokens * pricing.input_micro_per_mtok / 1_000_000;
    CostEstimate {
        total_micro: (input_cost + cache_cost + output_cost) * factor / 100,
        peak_factor_percent: factor as u16,
        cache_discount_micro: full_price_would_be.saturating_sub(input_cost + cache_cost) * factor
            / 100,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{PeakPeriod, ProviderId};

    fn pricing() -> PricingSpec {
        PricingSpec {
            currency: nexus_contracts::affinity::Currency::Cny,
            input_micro_per_mtok: 1_000_000,  // ¥1/M tok
            output_micro_per_mtok: 4_000_000, // ¥4/M tok
            cache_hit_micro_per_mtok: 10_000, // ¥0.01/M tok(DeepSeek 档)
            peak_periods: vec![PeakPeriod {
                start_hour: 8,
                end_hour: 20,
                factor_percent: 200,
            }],
        }
    }

    #[test]
    fn peak_factor_lookup() {
        let p = pricing();
        assert_eq!(peak_factor(&p, 12), 200, "高峰时段 2×");
        assert_eq!(peak_factor(&p, 23), 100, "谷段 1×");
        assert_eq!(peak_factor(&p, 8), 200, "起始小时含");
        assert_eq!(peak_factor(&p, 20), 100, "结束小时不含");
    }

    #[test]
    fn actual_cost_integer_math_with_cache_discount() {
        let usage = UsageReport {
            input_tokens: 1_000_000,   // 1M 输入
            output_tokens: 500_000,    // 0.5M 输出
            cache_hit_tokens: 600_000, // 0.6M 缓存命中
            thinking_tokens: None,
        };
        // 谷段(factor=100):未命中 0.4M×1 + 命中 0.6M×0.01 + 输出 0.5M×4
        // = 400_000 + 6_000 + 2_000_000 = 2_406_000 微元
        let cost = actual_cost(&pricing(), &usage, 23);
        assert_eq!(cost.total_micro, 2_406_000);
        // 折扣 = 全价输入 1_000_000 - 实付输入 406_000 = 594_000
        assert_eq!(cost.cache_discount_micro, 594_000);
        // 高峰翻倍
        let peak = actual_cost(&pricing(), &usage, 12);
        assert_eq!(peak.total_micro, 4_812_000);
    }

    #[test]
    fn actual_cost_caps_cache_hit_at_input() {
        // 厂商返回异常 cache_hit > input 时钳制,不产生下溢 panic
        let usage = UsageReport {
            input_tokens: 100,
            output_tokens: 0,
            cache_hit_tokens: 999,
            thinking_tokens: None,
        };
        let cost = actual_cost(&pricing(), &usage, 23);
        assert_eq!(cost.peak_factor_percent, 100);
    }

    #[test]
    fn pick_codec_uses_preferred_dialect() {
        // preferred_dialect(dialects 首个,无 PreferDialect 怪癖)驱动码器选择;
        // M1 三方言均有码器,Responses 首选即装配 Responses 码器
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiResponses,
        );
        spec.dialects.push(ProtocolDialect::OpenAiChat);
        let adapter = VendorAdapter::assemble(Arc::new(spec), None).unwrap();
        assert_eq!(adapter.dialect(), ProtocolDialect::OpenAiResponses);
    }

    #[test]
    fn pick_codec_rejects_when_no_dialect_declared() {
        // 空 dialects → 无可用码器 → ChannelRejected 语义(通道不进路由池)
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        );
        spec.dialects.clear();
        let err = VendorAdapter::assemble(Arc::new(spec), None).unwrap_err();
        assert!(matches!(err, AffinityError::Capability { .. }));
    }
}
