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

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::affinity::{
    AffinityMessage, AffinityRequest, AffinityResponse, ContentBlock, CostEstimate, FinishReason,
    MessageRole, ModelAffinitySpec, ProtocolDialect, ProviderReceipt, TokenCacheKey, UsageReport,
};
use nexus_contracts::CapabilityToken;
use scc_cache::{CacheHitTracker, CachedResponse, SemanticResponseCache};
use sha2::{Digest, Sha256};

use crate::capability::{apply_output_budget, negotiate_budget};
use crate::coalescing::{coalesce_failure, CoalesceKey, JoinOutcome, RequestCoalescer};
use crate::codec::Codec;
use crate::cost::{actual_cost, current_hour, estimate_cost};
use crate::cost_guard::CostGuard;
use crate::error::AffinityError;
use crate::prompt_compress::PromptCompressor;
use crate::semantic_fingerprint::semantic_fingerprint;
use crate::token_estimate::TokenEstimator;
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
    /// 厂商缓存命中率跟踪器(None = 不观测,R1 闭环可选挂接)
    cache_tracker: Option<Arc<CacheHitTracker>>,
    /// 超长历史消息压缩器(None = 不压缩,R4 可选挂接;sidecar 失败降级原文)
    compressor: Option<PromptCompressor>,
    /// 语义响应缓存(None = 不启用 R3 语义缓存热路径,可选挂接)
    ///
    /// 挂接后 invoke() 先查缓存:命中直接返回(不发厂商调用),miss 走厂商后回填。
    /// 命名空间 = intent_id(隐私隔离),精确键 = TokenCacheKey,语义层 = CLV 代理指纹。
    semantic_cache: Option<Arc<SemanticResponseCache>>,
    /// S9 Token 效率能力令牌(None = 无灰度约束,默认启用)
    ///
    /// Provisional/Cooldown/Frozen 态 `allows_learned_policy()` = false
    /// → bypass 全部缓存逻辑(Fail-open,仅 token 消耗上升,ADR-069 回滚接缝)。
    capability_token: Option<Arc<CapabilityToken>>,
    /// 成本熔断守卫(None = 不设成本上限,ADR-069 Task 6.2 可选挂接)
    ///
    /// 挂接后 invoke() 传输前 check(累计成本超限 → Quota 拒绝),
    /// 解码回算成功后 record(cost.total_micro)。
    cost_guard: Option<Arc<CostGuard>>,
    /// 动态 token 估算器(None = 纯函数字节/4 口径,ADR-070 可选挂接)
    ///
    /// 挂接后裁剪判定与成本预估使用 EWMA 校准口径(修正每通道系统偏差),
    /// invoke 闭环以厂商真实 usage 校准系数。
    estimator: Option<Arc<TokenEstimator>>,
    /// in-flight 请求合并器(None = 不合并,ADR-072 可选挂接)
    ///
    /// 挂接后语义缓存 miss 的并发相同请求合并为一次厂商调用,
    /// 等待者共享领导者结果(零重复计费)。
    coalescer: Option<Arc<RequestCoalescer>>,
    /// 事件总线(None = 静默模式,单测/录播回放用)
    bus: Option<EventBus>,
}

/// 可选挂接依赖集合 — 收敛 assemble 系构造函数(避免参数无限膨胀)
///
/// WHY struct 而非逐参追加: assemble 系已有 4 个构造函数,继续加参
/// 会令调用方签名失读;统一收拢为"必选(spec/bus)+ 可选(options)",
/// 既有构造函数全部委托 `assemble_with_options`,调用方零改动。
#[derive(Clone, Default)]
pub struct AdapterOptions {
    /// 厂商缓存命中率跟踪器(R1 观测闭环)
    pub cache_tracker: Option<Arc<CacheHitTracker>>,
    /// 超长历史消息压缩器(R4)
    pub compressor: Option<PromptCompressor>,
    /// 语义响应缓存(R3,挂接后 invoke 走命中热路径)
    pub semantic_cache: Option<Arc<SemanticResponseCache>>,
    /// S9 能力令牌(未授权态 bypass 缓存逻辑)
    pub capability_token: Option<Arc<CapabilityToken>>,
    /// 成本熔断守卫(累计成本超限熔断,Task 6.2)
    pub cost_guard: Option<Arc<CostGuard>>,
    /// 动态 token 估算器(ADR-070,EWMA 校准;None = 纯函数字节/4 口径)
    pub estimator: Option<Arc<TokenEstimator>>,
    /// in-flight 请求合并器(ADR-072;None = 不合并)
    pub coalescer: Option<Arc<RequestCoalescer>>,
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
        Self::assemble_with_tracker(spec, bus, None)
    }

    /// 装配适配器并可选挂接厂商缓存命中率跟踪器(R1 观测闭环,ADR-069)
    ///
    /// tracker 为 None 时与 `assemble` 行为完全一致(静默模式);
    /// 挂接后每次 invoke() 解码即原子累计分厂商命中率
    /// (CacheHitTracker::record,与 StreamSessionCompleted 事件同源消费
    /// usage.cache_hit_tokens / input_tokens)。
    pub fn assemble_with_tracker(
        spec: Arc<ModelAffinitySpec>,
        bus: Option<EventBus>,
        cache_tracker: Option<Arc<CacheHitTracker>>,
    ) -> Result<Self, AffinityError> {
        Self::assemble_full(spec, bus, cache_tracker, None)
    }

    /// 装配适配器并可选挂接缓存命中率跟踪器与历史消息压缩器(R4,ADR-069)
    ///
    /// cache_tracker 为 None 时与 `assemble` 行为完全一致(静默模式);
    /// compressor 为 None 时不压缩历史消息;两者均可独立挂接。
    /// 既有 `assemble` / `assemble_with_tracker` 签名保持兼容(委托本函数)。
    pub fn assemble_full(
        spec: Arc<ModelAffinitySpec>,
        bus: Option<EventBus>,
        cache_tracker: Option<Arc<CacheHitTracker>>,
        compressor: Option<PromptCompressor>,
    ) -> Result<Self, AffinityError> {
        Self::assemble_full_with_semantic(spec, bus, cache_tracker, compressor, None, None)
    }

    /// 装配适配器并可选挂接语义响应缓存与 S9 能力令牌(R3,ADR-069 Task 5)
    ///
    /// 在 `assemble_full` 基础上扩展两个可选参数:
    /// - `semantic_cache`: 挂接后 invoke() 走语义缓存热路径(命中免厂商调用)
    /// - `capability_token`: S9 灰度门,未授权态(Provisional/Cooldown/Frozen)
    ///   bypass 全部缓存逻辑(Fail-open)
    ///
    /// 两者均为 None 时与 `assemble_full` 行为完全一致(静默模式);
    /// 既有 `assemble` / `assemble_with_tracker` / `assemble_full` 委托本函数,
    /// 调用方无需改动即可沿用旧装配路径。
    pub fn assemble_full_with_semantic(
        spec: Arc<ModelAffinitySpec>,
        bus: Option<EventBus>,
        cache_tracker: Option<Arc<CacheHitTracker>>,
        compressor: Option<PromptCompressor>,
        semantic_cache: Option<Arc<SemanticResponseCache>>,
        capability_token: Option<Arc<CapabilityToken>>,
    ) -> Result<Self, AffinityError> {
        Self::assemble_with_options(
            spec,
            bus,
            AdapterOptions {
                cache_tracker,
                compressor,
                semantic_cache,
                capability_token,
                cost_guard: None,
                estimator: None,
                coalescer: None,
            },
        )
    }

    /// 最完整装配构造 — 必选(spec/bus)+ 可选挂接集合(options)
    ///
    /// 新增可选依赖(如成本熔断守卫)只扩 `AdapterOptions` 字段,
    /// 既有构造函数签名全部保持兼容(委托本函数,调用方零改动)。
    pub fn assemble_with_options(
        spec: Arc<ModelAffinitySpec>,
        bus: Option<EventBus>,
        options: AdapterOptions,
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
            cache_tracker: options.cache_tracker,
            compressor: options.compressor,
            semantic_cache: options.semantic_cache,
            capability_token: options.capability_token,
            cost_guard: options.cost_guard,
            estimator: options.estimator,
            coalescer: options.coalescer,
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

        // 每次 invoke 调用记录一次请求（语义缓存命中率分母）
        if let Some(tracker) = &self.cache_tracker {
            tracker.record_request();
        }

        // 0. R3 语义缓存热路径(ADR-069 Task 5):命中直接返回,不发厂商调用。
        //    顺序固定为"缓存查询 → 裁剪 → 厂商调用 → 回填":缓存键/指纹/上下文
        //    哈希基于原始 request(裁剪/压缩只影响实际发送内容,不影响缓存面)。
        let semantic = self.semantic_cache_inputs(request);
        if let Some(inputs) = &semantic {
            if let Some(cached) = inputs.lookup() {
                let namespace = inputs.namespace.clone();
                let similarity = cached.similarity;
                let resp = Self::cached_response(&self.spec, self.dialect(), cached);
                self.publish(NexusEvent::SemanticCacheHit {
                    metadata: EventMetadata::new(EVENT_SOURCE),
                    namespace,
                    similarity,
                })
                .await;
                // 语义缓存命中计数（原子递增，无锁安全）
                if let Some(tracker) = &self.cache_tracker {
                    tracker.record_semantic_hit();
                }
                // 发布 StreamSessionCompleted(semantic_cache_hit=true)
                // 零 usage/cost/TTFT(未发厂商调用),efficiency-monitor 据此区分
                // 语义缓存命中与厂商调用路径,分别统计命中率。
                self.publish(NexusEvent::StreamSessionCompleted {
                    metadata: EventMetadata::new(EVENT_SOURCE),
                    intent_id: request.intent_id.to_string(),
                    route_key,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_hit_tokens: 0,
                    cost_actual_micro: 0,
                    ttft_ms: 0,
                    semantic_cache_hit: true,
                    // 语义缓存命中路径:未发厂商调用,无裁剪/压缩/早停/合并观测
                    trimmed_before_tokens: None,
                    trimmed_after_tokens: None,
                    compressed_ratio: None,
                    early_stop_reason: None,
                    coalesced: false,
                })
                .await;
                return Ok(resp);
            }
        }

        // 0.4 C in-flight 请求合并(ADR-072 决策 ④):语义缓存 miss 后,
        //    并发相同请求合并为一次厂商调用(多 Agent 并行/重试去重)。
        //    - 合并键 = TokenCacheKey + context_hash(消息内容),与语义缓存
        //      同键空间,仅"完全相同的请求"合并(不牺牲正确性);
        //      键独立构造,不依赖语义缓存挂接(合并是独立优化面)
        //    - S9 未授权(CapabilityToken 存在且未授权)时不合并
        //      (Fail-open 语义一致)
        //    - 等待者超时 = min(endpoint.timeout_ms, 30s),超时按 retryable
        //      处理(可重试);领导者异常未释放时同样由超时兜底
        let coalesce_key = CoalesceKey::new(
            crate::prompt_norm::build_token_cache_key(&self.spec, request),
            context_hash(&request.messages),
        );
        let s9_bypass = self
            .capability_token
            .as_ref()
            .is_some_and(|t| !t.allows_learned_policy(unix_now_secs() as i64));
        if let (Some(coalescer), false) = (&self.coalescer, s9_bypass) {
            match coalescer.join(coalesce_key.clone()) {
                JoinOutcome::Wait(rx) => {
                    let wait_ms = self.spec.endpoint.timeout_ms.clamp(1_000, 30_000);
                    let outcome =
                        tokio::time::timeout(std::time::Duration::from_millis(wait_ms), rx).await;
                    return match outcome {
                        // 等待者:共享领导者结果(零厂商调用,零重复计费)
                        Ok(Ok(Ok(resp))) => {
                            self.publish(NexusEvent::StreamSessionCompleted {
                                metadata: EventMetadata::new(EVENT_SOURCE),
                                intent_id: request.intent_id.to_string(),
                                route_key: route_key.clone(),
                                input_tokens: resp.usage.input_tokens,
                                output_tokens: resp.usage.output_tokens,
                                cache_hit_tokens: resp.usage.cache_hit_tokens,
                                cost_actual_micro: 0,
                                ttft_ms: 0,
                                semantic_cache_hit: false,
                                trimmed_before_tokens: None,
                                trimmed_after_tokens: None,
                                compressed_ratio: None,
                                early_stop_reason: None,
                                coalesced: true,
                            })
                            .await;
                            // 等待者语义:响应与领导者一致(同请求同响应)
                            Ok((*resp).clone())
                        }
                        // 领导者失败/异常/超时:retryable(重试后可能重新合并)
                        Ok(Ok(Err(reason))) => Err(coalesce_failure(&route_key, reason)),
                        Ok(Err(_)) | Err(_) => Err(coalesce_failure(
                            &route_key,
                            "leader vanished or wait timeout".into(),
                        )),
                    };
                }
                JoinOutcome::Lead => {}
            }
        }

        // 0.5 成本熔断前置检查(ADR-069 Task 6.2):熔断中拒绝,不发厂商调用。
        //    check() 同步无锁(原子),放在传输前保证超限通道零额外成本消耗;
        //    语义缓存命中已提前返回,缓存命中不消耗预算,不受熔断约束。
        if let Some(guard) = &self.cost_guard {
            if let Err(e) = guard.check(unix_now_secs() as i64) {
                return Err(AffinityError::Quota {
                    route_key,
                    reason: e.to_string(),
                });
            }
        }

        // 0. R4 上下文预算与动态裁剪(ADR-069 Task 3):超预算裁剪 + 超长历史压缩。
        //    不修改原引用——裁剪/压缩作用于副本,原 request 保持不可变;
        //    预算源为 L6 osa-coordinator compute_token_budget(复杂度档 × 窗口 × 0.6)。
        //    裁剪/压缩观测(ADR-070):before/after 估算与压缩率随事件发布,
        //    efficiency-monitor 据此验证 R4 收益(SMART 等效输入成本目标)。
        //    估算口径(ADR-070):挂接 TokenEstimator 时用 EWMA 校准值,
        //    否则回落纯函数字节/4(行为与旧版一致)。
        let mut effective = std::borrow::Cow::Borrowed(request);
        let budget = crate::conversation_trim::conversation_budget(&self.spec, request);
        let estimated = match &self.estimator {
            Some(est) => est.estimate_messages_calibrated(
                self.spec.provider.as_str(),
                &self.spec.model,
                &request.messages,
            ),
            None => crate::conversation_trim::estimate_tokens(&request.messages),
        };
        let mut trimmed_before_tokens: Option<u64> = None;
        let mut trimmed_after_tokens: Option<u64> = None;
        let mut compressed_ratio: Option<f32> = None;
        if estimated > budget {
            trimmed_before_tokens = Some(u64::from(estimated));
            effective.to_mut().messages =
                crate::conversation_trim::trim_to_budget(request.messages.clone(), budget);
            trimmed_after_tokens = Some(match &self.estimator {
                Some(est) => u64::from(est.estimate_messages_calibrated(
                    self.spec.provider.as_str(),
                    &self.spec.model,
                    &effective.messages,
                )),
                None => u64::from(crate::conversation_trim::estimate_tokens(
                    &effective.messages,
                )),
            });
            tracing::debug!(
                route_key = %route_key,
                budget,
                estimated,
                before = request.messages.len(),
                after = effective.messages.len(),
                "conversation trimmed to token budget"
            );
        }
        if self.compressor.is_some() {
            compressed_ratio = self.maybe_compress_history(effective.to_mut()).await;
        }

        // 0.75 隐式族稳定前缀重排(ADR-072 决策 ④):System 消息重定位到末尾,
        //    最大化 DeepSeek/Qwen/豆包自动前缀缓存的跨轮次公共前缀。
        //    仅 Implicit 族 + OpenAI Chat 方言生效,无 System 消息时零操作。
        if crate::prompt_norm::layout_messages(&self.spec, effective.to_mut()) {
            tracing::debug!(
                route_key = %route_key,
                "implicit-cache prefix layout applied (system moved to tail)"
            );
        }

        // 1. 方言原生请求构造(P2 保真)
        let mut body = self.codec.build_request(&self.spec, &effective)?;

        // 1.5 输出预算注入(ADR-069: TTG 档 × max_output → 具体 token 数)
        let budget = negotiate_budget(
            &self.spec.capabilities,
            request.thinking_pref,
            request.budget_hint_micro,
        );
        apply_output_budget(&mut body, &budget);

        // 2. 路由决策留痕(P6 成本先行:预估成本随事件发布;基于裁剪后实际发送内容)
        let estimate = estimate_cost(&self.spec.pricing, &effective, current_hour());
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
        // 5.1 R3 语义缓存回填:miss 响应写入缓存(键/指纹/上下文哈希与查询段一致)。
        //     S9 bypass 时 semantic 为 None(查询段已判定),此处同样不写入。
        if let Some(inputs) = &semantic {
            inputs.backfill(&decoded.blocks);
        }
        let cost = actual_cost(&self.spec.pricing, &decoded.usage, current_hour());
        let ttft_ms = started.elapsed().as_millis() as u64;

        // 5.2 成本熔断累计(ADR-069 Task 6.2):实际成本入账(原子累计),
        //    跨线检测由下次 invoke 的 check() 触发(唯一入口,防重放发布)。
        if let Some(guard) = &self.cost_guard {
            guard.record(cost.total_micro);
        }

        // 5.3 Token 估算校准(ADR-070):以厂商真实 input_tokens 与发送内容
        //    估算之比更新 EWMA 系数(修正每通道 BPE 系统偏差)。
        //    校准源 = 裁剪/压缩后的发送内容,与厂商计费口径一致。
        if let Some(est) = &self.estimator {
            let sent_estimated = crate::conversation_trim::estimate_tokens(&effective.messages);
            est.calibrate(
                self.spec.provider.as_str(),
                &self.spec.model,
                decoded.usage.input_tokens,
                u64::from(sent_estimated),
            );
        }

        // 5.5 Token 效率观测闭环(ADR-069 Task 1/2):
        // ① L2 前缀稳定性校验——工具声明含时间戳/UUID 等动态内容时
        //    稳定前缀每轮漂移,缓存命中率归零;仅观测(warn)不阻断请求;
        // ② 厂商缓存命中率原子累计(R1,R2 计量口径的同步 side effect);
        // ③ Token 缓存键构造并留痕(Task 5 语义缓存将复用本键)。
        let tools_json = crate::prompt_norm::deterministic_tools_json(&request.tools);
        if let Err(e) = crate::prompt_norm::validate_prefix_stability(&tools_json, "L2") {
            tracing::warn!(route_key = %route_key, error = %e, "L2 tool declarations unstable, cache hit rate at risk");
        }
        if let Some(tracker) = &self.cache_tracker {
            tracker.record(
                self.spec.provider.as_str(),
                decoded.usage.cache_hit_tokens,
                decoded.usage.input_tokens,
            );
        }
        let cache_key = crate::prompt_norm::build_token_cache_key(&self.spec, &effective);
        tracing::debug!(
            route_key = %route_key,
            model = %cache_key.model,
            "token cache key computed"
        );

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
            semantic_cache_hit: false,
            trimmed_before_tokens,
            trimmed_after_tokens,
            compressed_ratio,
            // invoke() 非流式路径无 early stop(流式数据面在 Phase 4 接入)
            early_stop_reason: None,
            // 领导者自身不算合并(等待者 coalesced=true,已提前返回)
            coalesced: false,
        })
        .await;

        // 6.5 合并释放(ADR-072):领导者成功 → 向全部等待者分发同一响应。
        //    失败路径由等待者超时兑底(保守设计,避免错误路径散布 complete 调用);
        //    complete 幂等:S9 bypass 时未 join,remove 不存在键零操作。
        let response = AffinityResponse {
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
        };
        if let Some(coalescer) = &self.coalescer {
            coalescer.complete(&coalesce_key, Ok(Arc::new(response.clone())));
        }
        Ok(response)
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

    /// 超长历史消息压缩(R4,ADR-069)— 压缩最长的可压缩历史消息
    ///
    /// 可压缩 = 非 System/Tool 且非最后一条(token 估算 > 4K 才触发)。
    /// sidecar 失败/未实际压缩 → 返回原文不阻塞(graceful degradation)。
    /// 压缩对象是 conversation 历史消息,不参与 L1+L2 system_prompt_hash,
    /// 缓存键保持稳定(见 prompt_norm 模块文档)。
    ///
    /// 返回实际压缩率(压缩后/压缩前,< 1.0 表示压缩发生;None = 未压缩),
    /// 供 StreamSessionCompleted.compressed_ratio 观测(ADR-070)。
    async fn maybe_compress_history(&self, effective: &mut AffinityRequest) -> Option<f32> {
        let compressor = match &self.compressor {
            Some(c) => c,
            None => return None,
        };
        let idx = crate::conversation_trim::longest_compressible_message(&effective.messages)?;
        if crate::conversation_trim::estimate_message_tokens(&effective.messages[idx])
            <= crate::conversation_trim::COMPRESS_THRESHOLD_TOKENS
        {
            return None;
        }
        // 拼接该消息的全部 Text 块作为压缩输入(Thinking/ToolUse 块保留不参与)
        let text: String = effective.messages[idx]
            .blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_ref()),
                _ => None,
            })
            .collect();
        let Ok(compressed) = compressor
            .compress(&text, crate::conversation_trim::COMPRESS_TARGET_RATIO)
            .await
        else {
            return None; // compress 契约恒 Ok(失败返回原文),此处为契约防御
        };
        if compressed == text {
            return None; // 未实际压缩(sidecar 降级路径,保留原文)
        }
        // 替换 Text 块为压缩结果,保留 Thinking/ToolUse/ToolResult 块
        let after_chars = compressed.len();
        let before_chars = text.len();
        let msg = &mut effective.messages[idx];
        msg.blocks
            .retain(|b| !matches!(b, ContentBlock::Text { .. }));
        msg.blocks.insert(
            0,
            ContentBlock::Text {
                text: compressed.into(),
            },
        );
        tracing::debug!(
            route_key = %self.spec.route_key(),
            before_chars,
            after_chars,
            "longest history message compressed"
        );
        Some(after_chars as f32 / before_chars as f32)
    }

    /// R3 语义缓存输入构造 — S9 灰度门 + 键/指纹/上下文哈希一次性计算
    ///
    /// 返回 None = 语义缓存未挂接或 S9 未授权(bypass 全部缓存逻辑)。
    /// 查询与回填共享同一份输入(同一次 invoke 内键/指纹/哈希恒定)。
    fn semantic_cache_inputs(&self, request: &AffinityRequest) -> Option<SemanticCacheInputs> {
        let cache = self.semantic_cache.as_ref()?;
        // S9 灰度门(Fail-open):token 存在且未授权 → bypass 缓存,仅 token 消耗上升。
        // CapabilityToken 字段全为 Sync,Arc 只读共享即可(无需 Mutex);
        // allows_learned_policy 是 &self 查询,取布尔快照即用即弃,无持锁跨 await
        if let Some(token) = &self.capability_token {
            if !token.allows_learned_policy(unix_now_secs() as i64) {
                return None;
            }
        }
        Some(SemanticCacheInputs {
            cache: Arc::clone(cache),
            namespace: request.intent_id.to_string(),
            key: crate::prompt_norm::build_token_cache_key(&self.spec, request),
            clv: semantic_fingerprint(&request.messages, &request.tools),
            context_hash: context_hash(&request.messages),
            now_secs: unix_now_secs(),
        })
    }

    /// 缓存命中响应构造 — 最小响应:文本块 + 零 usage/cost + Stop 语义
    ///
    /// WHY usage/cost 全零:未发厂商调用,无真实计量;命中留痕由
    /// SemanticCacheHit 事件承载(观测方从事件侧累计命中率,不污染 EWMA)。
    fn cached_response(
        spec: &ModelAffinitySpec,
        dialect: ProtocolDialect,
        cached: CachedResponse,
    ) -> AffinityResponse {
        AffinityResponse {
            blocks: vec![ContentBlock::Text {
                text: cached.response.to_string().into(),
            }],
            usage: UsageReport::default(),
            cost: CostEstimate::default(),
            finish_reason: FinishReason::Stop,
            receipt: ProviderReceipt {
                provider: spec.provider.clone(),
                model: spec.model.clone(),
                dialect,
                request_id: None, // 未发厂商请求,无厂商侧请求标识
            },
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

/// R3 语义缓存热路径输入 — 一次 invoke 内查询与回填共享的缓存面参数
///
/// 查询(miss)与回填(厂商返回后)必须使用同一份键/指纹/上下文哈希,
/// 否则回填条目永远无法被后续查询命中(键面漂移)。
struct SemanticCacheInputs {
    cache: Arc<SemanticResponseCache>,
    namespace: String,
    key: TokenCacheKey,
    clv: Vec<f32>,
    context_hash: [u8; 32],
    now_secs: u64,
}

impl SemanticCacheInputs {
    /// 语义查询:精确键 + 语义指纹 + Context Ledger 漂移校验
    fn lookup(&self) -> Option<CachedResponse> {
        self.cache.lookup_with_context(
            &self.namespace,
            &self.key,
            &self.clv,
            Some(&self.context_hash),
        )
    }

    /// 回填:解码响应 Text 块拼合后插入(空响应不缓存,避免命中空块混淆语义)
    fn backfill(&self, blocks: &[ContentBlock]) {
        let mut text = String::new();
        for b in blocks {
            if let ContentBlock::Text { text: t } = b {
                text.push_str(t);
            }
        }
        if text.is_empty() {
            return; // 空响应无缓存价值
        }
        self.cache.insert(
            &self.namespace,
            self.key.clone(),
            self.clv.clone(),
            &text,
            self.context_hash,
            self.now_secs,
        );
    }
}

/// 会话上下文哈希 — 分段 Context Ledger(ADR-070/072)
///
/// 响应决定段校验:S1 = 全部 System 消息 + S3 = 最近 K=4 条消息(含工具结果),
/// 拼接后 SHA-256。更早历史(S5)不参与——多轮工具调用会话中早期消息
/// 追加/修改不再失效全部语义缓存条目(修复 ADR-069 全量哈希失效粒度过粗)。
///
/// WHY 分段而非全量:语义缓存本质是"近义请求复用",响应主要取决于
/// 系统提示 + 当前意图 + 最近工具结果;早期历史对响应影响弱,豁免
/// 不产生实质正确性风险(命中后仍在 namespace 内,精确键 + 语义层双校验)。
///
/// serde derive 序列化按字段声明序、无随机空白,与 prompt_norm 同确定性契约。
fn context_hash(messages: &[AffinityMessage]) -> [u8; 32] {
    // 最近 K 条消息参与哈希(覆盖当前意图 + 最近工具结果链)
    const SEGMENT_RECENT_K: usize = 4;
    let recent = messages.len().saturating_sub(SEGMENT_RECENT_K);
    let mut hasher = Sha256::new();
    // 段 1:全部 System 消息(会话级稳定前缀,响应风格的决定因子)
    for m in messages.iter().filter(|m| m.role == MessageRole::System) {
        hasher.update(serde_json::to_string(m).unwrap_or_else(|_| "[]".into()));
    }
    hasher.update([0x01]); // 段分隔符:防跨段拼接碰撞
                           // 段 3:最近 K 条消息(当前意图 + 最近工具结果)
    for m in messages.iter().skip(recent) {
        hasher.update(serde_json::to_string(m).unwrap_or_else(|_| "[]".into()));
    }
    hasher.finalize().into()
}

/// 当前 Unix 秒(缓存条目时间戳;时钟回拨按 0 兜底,仅影响驱逐序)
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::peak_factor;
    use nexus_contracts::affinity::{
        AffinityMessage, AffinityOverrides, ContentBlock, MessageRole, OutputFormat, PeakPeriod,
        PricingSpec, ProviderId, SamplingParams, ThinkingPreference,
    };

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

    // ============================================================
    // 分段 Context Ledger(ADR-072 决策 ⑤):早期历史豁免
    // ============================================================

    fn text_msg(role: MessageRole, text: &str) -> AffinityMessage {
        AffinityMessage {
            role,
            blocks: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    #[test]
    fn segment_hash_stable_across_early_history_growth() {
        // 核心性质:多轮会话早期历史追加 → 分段哈希不变(仅最近 K 条 + System 参与)
        // 旧全量哈希下此场景全部失效,语义缓存命中率趋近 0(ADR-072 修复目标)
        let mut base = vec![
            text_msg(MessageRole::System, "sys"),
            text_msg(MessageRole::User, "old turn"),
            text_msg(MessageRole::Assistant, "old reply"),
            text_msg(MessageRole::User, "recent q"),
            text_msg(MessageRole::Tool, "recent tool result"),
            text_msg(MessageRole::User, "current q"),
        ];
        let h1 = context_hash(&base);
        // 早期历史追加 2 轮(超过最近 K=4 的窗口外)
        base.insert(2, text_msg(MessageRole::User, "extra history"));
        base.insert(2, text_msg(MessageRole::Assistant, "extra reply"));
        let h2 = context_hash(&base);
        assert_eq!(h1, h2, "早期历史(S5 段)变化不得失效缓存");
    }

    #[test]
    fn segment_hash_changes_on_recent_message_change() {
        // 最近 K 条消息变化 → 哈希变(当前意图变更必须失效)
        let base = vec![
            text_msg(MessageRole::User, "old history"),
            text_msg(MessageRole::User, "current question A"),
        ];
        let mut modified = base.clone();
        modified[1] = text_msg(MessageRole::User, "current question B");
        assert_ne!(context_hash(&base), context_hash(&modified));
    }

    #[test]
    fn segment_hash_changes_on_system_change() {
        // System 提示变化 → 哈希变(会话级稳定前缀变更必须失效)
        let base = vec![
            text_msg(MessageRole::System, "sys A"),
            text_msg(MessageRole::User, "q"),
        ];
        let mut modified = base.clone();
        modified[0] = text_msg(MessageRole::System, "sys B");
        assert_ne!(context_hash(&base), context_hash(&modified));
    }

    #[test]
    fn segment_hash_deterministic() {
        let msgs = vec![
            text_msg(MessageRole::System, "sys"),
            text_msg(MessageRole::User, "q"),
        ];
        assert_eq!(context_hash(&msgs), context_hash(&msgs), "必须确定性");
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

    // ============================================================
    // R1 厂商缓存命中率观测闭环(ADR-069 Task 1)
    // ============================================================

    use scc_cache::CacheHitTracker;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    /// 启动本地 mock OpenAI Chat 端点,返回 base_url(响应体由 handler 决定)
    ///
    /// 零外部网络依赖:allowlist 校验只比对 host,本地环回地址天然通过。
    async fn spawn_chat_mock<F, B>(handler: F) -> String
    where
        F: Fn() -> B + Send + Sync + Clone + 'static,
        B: axum::response::IntoResponse + Send + 'static,
    {
        let app = axum::Router::new().route(
            "/chat/completions",
            axum::routing::post(move || {
                let h = handler.clone();
                async move { h() }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// 可捕获请求体的 mock 端点(断言裁剪后实际发送的消息数)
    async fn spawn_chat_mock_with_body<F, B>(handler: F) -> String
    where
        F: Fn(axum::body::Bytes) -> B + Send + Sync + Clone + 'static,
        B: axum::response::IntoResponse + Send + 'static,
    {
        let app = axum::Router::new().route(
            "/chat/completions",
            axum::routing::post(move |body: axum::body::Bytes| {
                let h = handler.clone();
                async move { h(body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// OpenAI Chat 方言响应(cache_hit 注入 usage.prompt_cache_hit_tokens)
    fn chat_response(cache_hit: u64) -> axum::response::Json<serde_json::Value> {
        axum::response::Json(serde_json::json!({
            "id": "chatcmpl-mock-001",
            "object": "chat.completion",
            "model": "deepseek-v4-flash",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "ok" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 24,
                "completion_tokens": 10,
                "total_tokens": 34,
                "prompt_cache_hit_tokens": cache_hit
            }
        }))
    }

    /// DeepSeek mock 通道 spec(base_url 指向本地 mock,免鉴权)
    fn mock_spec(base_url: &str) -> ModelAffinitySpec {
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        );
        spec.endpoint.base_url = base_url.into();
        spec.endpoint.timeout_ms = 5_000;
        spec.endpoint.connect_timeout_ms = 1_000;
        spec
    }

    fn mock_request() -> AffinityRequest {
        AffinityRequest {
            intent_id: "intent-r1".into(),
            messages: vec![AffinityMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            thinking_pref: ThinkingPreference::Fast,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
        }
    }

    #[tokio::test]
    async fn tracker_records_cache_hit_and_rate() {
        // 命中路径:16 缓存命中 / 24 输入 → 命中率 66%
        let base = spawn_chat_mock(|| chat_response(16)).await;
        let tracker = Arc::new(CacheHitTracker::new());
        let adapter = VendorAdapter::assemble_with_tracker(
            Arc::new(mock_spec(&base)),
            None,
            Some(tracker.clone()),
        )
        .unwrap();

        let resp = adapter.invoke(&mock_request()).await.unwrap();
        assert_eq!(resp.usage.cache_hit_tokens, 16);
        assert_eq!(
            tracker.hit_rate_percent("deep_seek"),
            66,
            "invoke 闭环后 tracker 必须记录命中率"
        );
        assert_eq!(tracker.tracked_providers(), 1);
    }

    #[tokio::test]
    async fn tracker_zero_hit_path() {
        // 零命中:cache_hit_tokens=0 → 命中率 0(厂商未命中缓存)
        let base = spawn_chat_mock(|| chat_response(0)).await;
        let tracker = Arc::new(CacheHitTracker::new());
        let adapter = VendorAdapter::assemble_with_tracker(
            Arc::new(mock_spec(&base)),
            None,
            Some(tracker.clone()),
        )
        .unwrap();

        adapter.invoke(&mock_request()).await.unwrap();
        assert_eq!(tracker.hit_rate_percent("deep_seek"), 0, "零命中 → 0%");
        assert_eq!(tracker.tracked_providers(), 1, "零命中也需记录输入 token");
    }

    #[tokio::test]
    async fn tracker_accumulates_mixed_hits() {
        // 混合路径:首次零命中 + 二次 16/24 命中 → 累计 16/48 = 33%
        let n = Arc::new(AtomicU64::new(0));
        let n2 = n.clone();
        let base = spawn_chat_mock(move || {
            let hit = if n2.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                0
            } else {
                16
            };
            chat_response(hit)
        })
        .await;
        let tracker = Arc::new(CacheHitTracker::new());
        let adapter = VendorAdapter::assemble_with_tracker(
            Arc::new(mock_spec(&base)),
            None,
            Some(tracker.clone()),
        )
        .unwrap();

        adapter.invoke(&mock_request()).await.unwrap();
        adapter.invoke(&mock_request()).await.unwrap();
        assert_eq!(
            tracker.hit_rate_percent("deep_seek"),
            33,
            "累计 16/48 命中率"
        );
    }

    // ============================================================
    // R4 上下文预算与动态裁剪(ADR-069 Task 3)
    // ============================================================

    use std::sync::atomic::AtomicUsize;

    /// 超预算会话必须在传输前裁剪(Simple 档 4096 窗口 → 预算 1024;
    /// 10 × 500 字符 ≈ 1250 tokens 超预算 → 实际发送消息数 < 10)
    #[tokio::test]
    async fn invoke_trims_over_budget_conversation_before_send() {
        let sent_count = Arc::new(AtomicUsize::new(0));
        let c = sent_count.clone();
        let base = spawn_chat_mock_with_body(move |body: axum::body::Bytes| {
            let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&body))
                .unwrap_or_else(|_| serde_json::json!({ "messages": [] }));
            c.store(
                parsed["messages"].as_array().map(|a| a.len()).unwrap_or(0),
                AtomicOrdering::SeqCst,
            );
            chat_response(0)
        })
        .await;

        let adapter = VendorAdapter::assemble(Arc::new(mock_spec(&base)), None).unwrap();
        let mut req = mock_request();
        req.messages = (0..10)
            .map(|i| AffinityMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: format!("history message {i} {}", "x".repeat(480)).into(),
                }],
            })
            .collect();
        adapter.invoke(&req).await.unwrap();
        assert!(
            sent_count.load(AtomicOrdering::SeqCst) < 10,
            "超预算会话必须裁剪后发送, sent = {}",
            sent_count.load(AtomicOrdering::SeqCst)
        );
    }

    /// compressor 配置但 sidecar 不可用 → graceful degradation 返回原文,
    /// invoke 必须不阻塞(降级路径守护)
    #[tokio::test]
    async fn invoke_with_compressor_sidecar_failure_degrades_gracefully() {
        let base = spawn_chat_mock(|| chat_response(0)).await;
        let mut spec = mock_spec(&base);
        // 大窗口:预算 150K tokens,不触发裁剪,只测压缩路径
        spec.capabilities.context_window = 1_000_000;
        let compressor =
            crate::prompt_compress::PromptCompressor::new("nonexistent_python_binary_xyz", "s.py")
                .with_timeout(std::time::Duration::from_secs(2));
        let adapter =
            VendorAdapter::assemble_full(Arc::new(spec), None, None, Some(compressor)).unwrap();
        let mut req = mock_request();
        req.messages = vec![
            AffinityMessage {
                role: MessageRole::Assistant,
                // ≈5K tokens > 4K 压缩阈值 → 触发压缩路径(sidecar 失败 → 原文)
                blocks: vec![ContentBlock::Text {
                    text: "a".repeat(20_000).into(),
                }],
            },
            AffinityMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: "final user input".into(),
                }],
            },
        ];
        let resp = adapter.invoke(&req).await.unwrap();
        assert!(!resp.blocks.is_empty(), "压缩降级(返回原文)不得阻塞 invoke");
    }

    // ============================================================
    // R3 语义响应缓存热路径(ADR-069 Task 5.3)
    // ============================================================

    use nexus_contracts::{CapabilityToken, SeamId};
    use scc_cache::SemanticResponseCache;

    /// 提取响应中的全部 Text 块文本(命中/厂商路径对比用)
    fn text_of(blocks: &[ContentBlock]) -> String {
        blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    /// 命中路径:首次 miss 走厂商并回填,第二次相同请求命中缓存
    /// → 不发厂商请求(调用计数不增长),返回缓存响应(零成本)
    #[tokio::test]
    async fn semantic_cache_hit_skips_vendor_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
            c.fetch_add(1, AtomicOrdering::SeqCst);
            chat_response(0)
        })
        .await;
        let cache = Arc::new(SemanticResponseCache::default());
        let adapter = VendorAdapter::assemble_full_with_semantic(
            Arc::new(mock_spec(&base)),
            None,
            None,
            None,
            Some(cache.clone()),
            None,
        )
        .unwrap();
        let req = mock_request();

        // 第一次:miss → 走厂商(计数 1)+ 解码后回填缓存
        let resp1 = adapter.invoke(&req).await.unwrap();
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "首次必须走厂商");
        assert_eq!(text_of(&resp1.blocks), "ok");
        assert_eq!(cache.namespace_len(&req.intent_id), 1, "miss 后必须回填");

        // 第二次:命中 → 不发厂商请求,直接返回缓存响应
        let resp2 = adapter.invoke(&req).await.unwrap();
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "命中后不得再发厂商请求"
        );
        assert_eq!(text_of(&resp2.blocks), "ok", "命中响应必须与缓存内容一致");
        assert_eq!(resp2.cost.total_micro, 0, "命中响应零成本(未发厂商调用)");
        assert_eq!(resp2.usage.input_tokens, 0, "命中响应零计量");
    }

    /// 跨 namespace(不同 intent_id)不命中:隐私隔离红线,即使键/指纹/哈希全同
    #[tokio::test]
    async fn semantic_cache_miss_across_namespace() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
            c.fetch_add(1, AtomicOrdering::SeqCst);
            chat_response(0)
        })
        .await;
        let spec = mock_spec(&base);
        let cache = Arc::new(SemanticResponseCache::default());
        // 手动预填 namespace "intent-a"(与请求同键/同指纹/同上下文哈希)
        let req = mock_request();
        let key = crate::prompt_norm::build_token_cache_key(&Arc::new(spec.clone()), &req);
        let clv = semantic_fingerprint(&req.messages, &req.tools);
        let hash = context_hash(&req.messages);
        cache.insert("intent-a", key, clv, "prefilled", hash, 1);

        // 用不同 intent_id("intent-b")请求 → 跨 namespace miss → 走厂商
        let adapter = VendorAdapter::assemble_full_with_semantic(
            Arc::new(spec),
            None,
            None,
            None,
            Some(cache.clone()),
            None,
        )
        .unwrap();
        let mut req_b = req;
        req_b.intent_id = "intent-b".into();
        let resp = adapter.invoke(&req_b).await.unwrap();
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "跨 namespace 必须 miss 走厂商"
        );
        assert_eq!(text_of(&resp.blocks), "ok");
    }

    /// S9 灰度门:Provisional 未授权 token → bypass 全部缓存逻辑
    /// (不查缓存也不回填,Fail-open 仅 token 消耗上升)
    #[tokio::test]
    async fn s9_provisional_token_bypasses_semantic_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
            c.fetch_add(1, AtomicOrdering::SeqCst);
            chat_response(0)
        })
        .await;
        let spec = mock_spec(&base);
        let cache = Arc::new(SemanticResponseCache::default());
        // 预填缓存:即使存在可命中条目,bypass 也不得查询
        let req = mock_request();
        let key = crate::prompt_norm::build_token_cache_key(&Arc::new(spec.clone()), &req);
        let clv = semantic_fingerprint(&req.messages, &req.tools);
        let hash = context_hash(&req.messages);
        cache.insert(req.intent_id.as_ref(), key, clv, "prefilled", hash, 1);

        // 初始 Provisional(level 0.2 < 激活阈值 0.3)→ allows_learned_policy = false
        let token = Arc::new(CapabilityToken::new(
            "s9-token-efficiency",
            SeamId::S9TokenEfficiency,
        ));
        assert!(
            !token.allows_learned_policy(unix_now_secs() as i64),
            "Provisional 必须未授权"
        );

        let adapter = VendorAdapter::assemble_full_with_semantic(
            Arc::new(spec),
            None,
            None,
            None,
            Some(cache.clone()),
            Some(token),
        )
        .unwrap();
        let resp = adapter.invoke(&req).await.unwrap();
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "bypass:必须走厂商(不查缓存)"
        );
        assert_eq!(text_of(&resp.blocks), "ok", "响应必须来自厂商而非缓存");
    }

    /// 命中路径发布 SemanticCacheHit 事件(观测面闭环)
    #[tokio::test]
    async fn semantic_cache_hit_publishes_event() {
        let base = spawn_chat_mock(|| chat_response(0)).await;
        // broadcast 纪律:subscribe 必须在 invoke(spawn) 之前同步调用
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let cache = Arc::new(SemanticResponseCache::default());
        let adapter = VendorAdapter::assemble_full_with_semantic(
            Arc::new(mock_spec(&base)),
            Some(bus),
            None,
            None,
            Some(cache.clone()),
            None,
        )
        .unwrap();
        let req = mock_request();
        adapter.invoke(&req).await.unwrap(); // miss → 回填
        adapter.invoke(&req).await.unwrap(); // hit → SemanticCacheHit

        let mut hit = false;
        while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(100)).await {
            if matches!(ev, NexusEvent::SemanticCacheHit { .. }) {
                hit = true;
                break;
            }
        }
        assert!(hit, "命中路径必须发布 SemanticCacheHit 事件");
    }

    // ============================================================
    // 6.1 观测接线验证(ADR-069 Task 6.1)
    // ============================================================

    /// 显式断言 SemanticCacheHit 事件字段:namespace = intent_id、similarity 达标
    #[tokio::test]
    async fn semantic_cache_hit_event_carries_namespace_and_similarity() {
        let base = spawn_chat_mock(|| chat_response(0)).await;
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let cache = Arc::new(SemanticResponseCache::default());
        let adapter = VendorAdapter::assemble_full_with_semantic(
            Arc::new(mock_spec(&base)),
            Some(bus),
            None,
            None,
            Some(cache.clone()),
            None,
        )
        .unwrap();
        let req = mock_request();
        adapter.invoke(&req).await.unwrap(); // miss → 回填
        adapter.invoke(&req).await.unwrap(); // hit → SemanticCacheHit

        let mut seen: Option<(String, f32)> = None;
        while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(100)).await {
            if let NexusEvent::SemanticCacheHit {
                namespace,
                similarity,
                ..
            } = ev
            {
                seen = Some((namespace, similarity));
                break;
            }
        }
        let (ns, sim) = seen.expect("命中路径必须发布 SemanticCacheHit");
        assert_eq!(ns, req.intent_id.to_string(), "namespace 必须是 intent_id");
        // 同键同指纹同哈希 → 余弦 = 1.0,必然 >= 缓存相似度阈值
        assert!(
            sim >= scc_cache::semantic_cache::DEFAULT_SIMILARITY_THRESHOLD,
            "similarity {sim} 必须 >= 阈值 {}",
            scc_cache::semantic_cache::DEFAULT_SIMILARITY_THRESHOLD
        );
    }

    /// 断言厂商命中率遥测字段:StreamSessionCompleted 携带
    /// cache_hit_tokens / input_tokens(R1 计量口径同步)
    #[tokio::test]
    async fn stream_session_completed_carries_cache_hit_telemetry() {
        let base = spawn_chat_mock(|| chat_response(16)).await;
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let adapter = VendorAdapter::assemble(Arc::new(mock_spec(&base)), Some(bus)).unwrap();

        adapter.invoke(&mock_request()).await.unwrap();

        let mut seen: Option<(u64, u64)> = None;
        while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(100)).await {
            if let NexusEvent::StreamSessionCompleted {
                input_tokens,
                cache_hit_tokens,
                ..
            } = ev
            {
                seen = Some((input_tokens, cache_hit_tokens));
                break;
            }
        }
        let (input, hit) = seen.expect("invoke 必须发布 StreamSessionCompleted");
        assert_eq!(input, 24, "input_tokens 来自 mock usage.prompt_tokens");
        assert_eq!(
            hit, 16,
            "cache_hit_tokens 来自 mock usage.prompt_cache_hit_tokens"
        );
    }

    // ============================================================
    // 6.2 成本熔断接线(ADR-069 Task 6.2)
    // ============================================================

    use crate::cost_guard::{CostGuard, BUDGET_TYPE};

    /// 累计成本超限后下一次 invoke 被拒(CircuitOpen → Quota),不再发厂商
    #[tokio::test]
    async fn cost_guard_rejects_invoke_after_budget_crossed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
            c.fetch_add(1, AtomicOrdering::SeqCst);
            chat_response(0)
        })
        .await;
        let guard = Arc::new(CostGuard::new(Some(1_000_000)));
        let adapter = VendorAdapter::assemble_with_options(
            Arc::new(mock_spec(&base)),
            None,
            AdapterOptions {
                cost_guard: Some(guard.clone()),
                ..AdapterOptions::default()
            },
        )
        .unwrap();

        // invoke #1:未超限 → 放行,厂商被调用
        adapter.invoke(&mock_request()).await.unwrap();
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "未超限必须放行");

        // 测试侧直接累计跨线(模拟后续 invoke 的实际成本入账)
        guard.record(1_000_000);

        // invoke #2:跨线 → 熔断 → Quota 拒绝,不再发厂商
        let err = adapter.invoke(&mock_request()).await.unwrap_err();
        assert!(
            matches!(err, AffinityError::Quota { .. }),
            "熔断拒绝必须映射为 Quota 错误, got {err}"
        );
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "熔断后不得再发厂商请求"
        );
    }

    /// 超限时发布 BudgetExceeded(订阅者收到,字段正确)且防重放只发一次
    #[tokio::test]
    async fn cost_guard_invoke_publishes_budget_exceeded_once() {
        let base = spawn_chat_mock(|| chat_response(0)).await;
        // broadcast 纪律:subscribe 必须在 check(publish) 之前同步调用
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let guard = Arc::new(CostGuard::with_bus(Some(1_000_000), Some(bus)));
        let adapter = VendorAdapter::assemble_with_options(
            Arc::new(mock_spec(&base)),
            None,
            AdapterOptions {
                cost_guard: Some(guard.clone()),
                ..AdapterOptions::default()
            },
        )
        .unwrap();

        adapter.invoke(&mock_request()).await.unwrap(); // 未超限
        guard.record(1_000_000); // 跨线
        assert!(adapter.invoke(&mock_request()).await.is_err());

        let mut budget_events = 0;
        while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(100)).await {
            if let NexusEvent::BudgetExceeded {
                budget_type,
                current,
                limit,
                ..
            } = ev
            {
                budget_events += 1;
                assert_eq!(budget_type, BUDGET_TYPE);
                assert_eq!(current, guard.spent_micro(), "current 为发布时刻累计成本");
                assert_eq!(limit, 1_000_000);
            }
        }
        assert_eq!(budget_events, 1, "防重放:只发布一次 BudgetExceeded");

        // 熔断期内第三次 invoke 仍拒绝,且不重发事件
        assert!(adapter.invoke(&mock_request()).await.is_err());
        let mut extra = 0;
        while let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(100)).await {
            if let NexusEvent::BudgetExceeded { .. } = ev {
                extra += 1;
            }
        }
        assert_eq!(extra, 0, "熔断期内不得重复发布");
    }

    /// 熔断检查必须发生在传输前:limit = 0 时 invoke 直接拒绝,厂商零调用
    #[tokio::test]
    async fn cost_guard_zero_limit_rejects_before_vendor_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
            c.fetch_add(1, AtomicOrdering::SeqCst);
            chat_response(0)
        })
        .await;
        let adapter = VendorAdapter::assemble_with_options(
            Arc::new(mock_spec(&base)),
            None,
            AdapterOptions {
                cost_guard: Some(Arc::new(CostGuard::new(Some(0)))),
                ..AdapterOptions::default()
            },
        )
        .unwrap();

        let err = adapter.invoke(&mock_request()).await.unwrap_err();
        assert!(
            matches!(err, AffinityError::Quota { .. }),
            "limit=0 首次 check 即跨线熔断, got {err}"
        );
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            0,
            "熔断必须发生在传输前(厂商零调用)"
        );
    }

    // ============================================================
    // C in-flight 请求合并(ADR-072 决策 ④)
    // ============================================================

    /// 50 并发相同请求 → 合并为恰好 1 次厂商调用(单线程 runtime 下
    /// 首个请求成为领导者,其余 49 个在 mock 响应返回前 join 为等待者)
    #[tokio::test]
    async fn coalescer_merges_concurrent_identical_requests() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
            c.fetch_add(1, AtomicOrdering::SeqCst);
            chat_response(0)
        })
        .await;
        let coalescer = Arc::new(RequestCoalescer::new());
        let adapter = VendorAdapter::assemble_with_options(
            Arc::new(mock_spec(&base)),
            None,
            AdapterOptions {
                coalescer: Some(coalescer.clone()),
                ..AdapterOptions::default()
            },
        )
        .unwrap();
        let req = mock_request();

        // 50 并发相同请求(多 Agent 并行场景)
        let mut handles = Vec::with_capacity(50);
        for _ in 0..50 {
            let a = adapter.clone();
            let r = req.clone();
            handles.push(tokio::spawn(async move { a.invoke(&r).await }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "50 并发相同请求必须合并为 1 次厂商调用"
        );
        assert_eq!(coalescer.inflight_count(), 0, "完成后条目必须清空");
    }

    /// 不同请求(消息不同)不合并:各自独立调用
    #[tokio::test]
    async fn coalescer_does_not_merge_distinct_requests() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let base = spawn_chat_mock_with_body(move |_body: axum::body::Bytes| {
            c.fetch_add(1, AtomicOrdering::SeqCst);
            chat_response(0)
        })
        .await;
        let coalescer = Arc::new(RequestCoalescer::new());
        let adapter = VendorAdapter::assemble_with_options(
            Arc::new(mock_spec(&base)),
            None,
            AdapterOptions {
                coalescer: Some(coalescer.clone()),
                ..AdapterOptions::default()
            },
        )
        .unwrap();

        // 并发两个不同请求(context_hash 不同 → 不合并)
        let mut req_a = mock_request();
        req_a.messages[0] = AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "question A".into(),
            }],
        };
        let mut req_b = mock_request();
        req_b.messages[0] = AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "question B".into(),
            }],
        };
        let a1 = adapter.clone();
        let a2 = adapter.clone();
        let (r1, r2) = tokio::join!(a1.invoke(&req_a), a2.invoke(&req_b));
        r1.unwrap();
        r2.unwrap();
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            2,
            "不同请求必须独立调用(不牺牲正确性)"
        );
    }
}
