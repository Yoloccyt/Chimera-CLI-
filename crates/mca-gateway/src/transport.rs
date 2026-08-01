//! transport — HTTP 传输层:客户端池、超时、重试、限流、熔断、域名白名单
//!
//! # 设计要点
//! - **单 `reqwest::Client` 复用**: 内建连接池 + rustls(纯 safe),
//!   按 EndpointSpec 超时配置;HTTP/2 由 ALPN 自动协商
//! - **域名白名单(零信任补偿)**: SecCore 沙箱只管子进程,进程内 reqwest
//!   不经过其防线;transport 强制"只允许 spec 声明的 host",防 spec 注入
//!   之外的任意出网(R9 风险缓解)
//! - **重试**: 仅 429/5xx/超时(可重试面)做指数退避 + 抖动,最多 3 次;
//!   DNS/TLS 失败不重试(不可恢复)
//! - **熔断**: 连续 5 次可重试失败 → 通道熔断 30s → 半开探测
//! - **限流**: spec 声明 rpm 时启用无锁速率限制(AtomicU64 CAS 推进
//!   下次许可时间戳),未声明则不做客户端限流(P3:未声明的约束不臆造)
//!
//! # API Key 安全约定
//! 密钥只存环境变量(spec 仅存变量名),本模块读取后仅注入请求头,
//! 不落日志不入错误消息。

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::time::Duration;

use nexus_contracts::affinity::EndpointSpec;

use crate::error::AffinityError;

/// 最大重试次数(首次 + 2 次重试;429/5xx 才计入)
const MAX_ATTEMPTS: u32 = 3;
/// 重试基础退避(指数:250ms → 500ms)
const RETRY_BASE_MS: u64 = 250;
/// 熔断阈值:连续可重试失败次数
const BREAKER_THRESHOLD: u32 = 5;
/// 熔断冷却时长(开闸 → 半开探测)
const BREAKER_COOLDOWN_MS: u64 = 30_000;

// ============================================================
// 通道级熔断器
//
// WHY 本地复制而非依赖 chimera-mas(ADR-065 决策 5):
// 镜像自 `chimera-mas/src/stability.rs` 的 AtomicU8 三态 CAS 范式
// (Closed/Open/HalfOpen)。L10 → L9 依赖虽合法,但为 ~150 行熔断器
// 拉入整个 L9 crate 破坏"仅依赖 L0/L1"的 chtc-bridge 同构约束。
// 两处实现互注镜像关系,修改任一处时必须评估另一处。
// ============================================================

/// 熔断器状态编码(AtomicU8)
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// 通道级熔断器 — 无锁三态状态机
///
/// 状态迁移:Closed --连续 5 次失败--> Open --30s--> HalfOpen
/// --探测成功--> Closed / --探测失败--> Open(重新计时)
#[derive(Debug)]
pub struct CircuitBreaker {
    /// 当前状态(STATE_* 编码)
    state: AtomicU8,
    /// 连续失败计数(成功即清零)
    consecutive_failures: AtomicU32,
    /// 开闸时刻(毫秒,单调钟起点偏移)
    opened_at_ms: AtomicU64,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitBreaker {
    /// 创建关闭态熔断器
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            consecutive_failures: AtomicU32::new(0),
            opened_at_ms: AtomicU64::new(0),
        }
    }

    /// 单调毫秒时钟(进程内相对时间,熔断只需相对时长)
    fn now_ms() -> u64 {
        use std::sync::OnceLock;
        use std::time::Instant;
        static EPOCH: OnceLock<Instant> = OnceLock::new();
        EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    /// 请求前检查:是否允许通过
    ///
    /// Open 态冷却期满时 CAS 迁移到 HalfOpen(仅放行一个探测请求)。
    pub fn allow(&self) -> bool {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED | STATE_HALF_OPEN => true,
            _ => {
                let opened = self.opened_at_ms.load(Ordering::Acquire);
                if Self::now_ms().saturating_sub(opened) >= BREAKER_COOLDOWN_MS {
                    // 冷却期满:竞争 CAS 到半开,胜者放行探测
                    self.state
                        .compare_exchange(
                            STATE_OPEN,
                            STATE_HALF_OPEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                } else {
                    false
                }
            }
        }
    }

    /// 记录成功:清零计数,半开态回到关闭
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Release);
        self.state.store(STATE_CLOSED, Ordering::Release);
    }

    /// 记录可重试失败:累计到阈值即开闸;半开探测失败立即回开
    pub fn record_failure(&self) {
        let prev_state = self.state.load(Ordering::Acquire);
        let failures = self.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;
        if prev_state == STATE_HALF_OPEN || failures >= BREAKER_THRESHOLD {
            self.opened_at_ms.store(Self::now_ms(), Ordering::Release);
            self.state.store(STATE_OPEN, Ordering::Release);
        }
    }

    /// 当前是否处于开闸(不可用)状态(诊断/健康分用)
    pub fn is_open(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_OPEN
    }
}

// ============================================================
// 无锁速率限制器(rpm)
// ============================================================

/// 无锁速率限制器 — AtomicU64 CAS 推进"下次许可时刻"
///
/// WHY 时间戳推进而非令牌桶计数: 单原子变量即可实现匀速放行
/// (间隔 = 60s/rpm),无需第二个原子做桶余量;突发容忍度由
/// 厂商侧限流兜底,客户端只做"不主动触发 429"的第一防线。
#[derive(Debug)]
pub struct RateLimiter {
    /// 两次请求的最小间隔(纳秒;0 = 不限流)
    interval_nanos: u64,
    /// 下次允许放行的时刻(单调纳秒)
    next_allowed: AtomicU64,
}

impl RateLimiter {
    /// 按 rpm 创建(None = 不限流)
    pub fn from_rpm(rpm: Option<u32>) -> Self {
        let interval_nanos = match rpm {
            Some(rpm) if rpm > 0 => 60_000_000_000 / u64::from(rpm),
            _ => 0,
        };
        Self {
            interval_nanos,
            next_allowed: AtomicU64::new(0),
        }
    }

    /// 单调纳秒时钟
    fn now_nanos() -> u64 {
        use std::sync::OnceLock;
        use std::time::Instant;
        static EPOCH: OnceLock<Instant> = OnceLock::new();
        EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }

    /// 尝试获取放行许可;返回需等待的时长(Zero = 立即放行)
    ///
    /// WHY 返回等待时长而非阻塞: 调用方(async 上下文)自行
    /// `tokio::time::sleep`,本类型保持纯同步无锁(C7 红线友好)。
    pub fn acquire_delay(&self) -> Duration {
        if self.interval_nanos == 0 {
            return Duration::ZERO;
        }
        let now = Self::now_nanos();
        loop {
            let next = self.next_allowed.load(Ordering::Acquire);
            // 许可时刻 = max(now, next);CAS 推进到许可时刻 + 间隔
            let grant_at = next.max(now);
            let new_next = grant_at + self.interval_nanos;
            if self
                .next_allowed
                .compare_exchange_weak(next, new_next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Duration::from_nanos(grant_at.saturating_sub(now));
            }
        }
    }
}

// ============================================================
// HTTP 传输
// ============================================================

/// 已就绪的 HTTP 响应(状态码 + 原始字节)
#[derive(Debug)]
pub struct TransportResponse {
    /// HTTP 状态码
    pub status: u16,
    /// 响应体字节
    pub body: Vec<u8>,
}

/// HTTP 传输 — 单 Client 复用 + 每通道熔断/限流由适配器持有
pub struct Transport {
    client: reqwest::Client,
}

impl Transport {
    /// 按端点配置创建传输(超时来自 spec,P8 数据驱动)
    pub fn new(endpoint: &EndpointSpec) -> Result<Self, AffinityError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(endpoint.timeout_ms))
            .connect_timeout(Duration::from_millis(endpoint.connect_timeout_ms))
            .build()
            .map_err(|e| AffinityError::Transport {
                route_key: String::new(),
                reason: format!("client build failed: {e}"),
                retryable: false,
            })?;
        Ok(Self { client })
    }

    /// 域名白名单校验:目标 URL 的 host 必须与 spec base_url 的 host 一致
    ///
    /// WHY(R9 零信任补偿): SecCore 沙箱不覆盖进程内 HTTP,本检查保证
    /// 网关只能访问 spec 显式声明的厂商域名,配置之外零出网面。
    pub fn check_allowlist(base_url: &str, target_url: &str) -> Result<(), AffinityError> {
        let host_of = |url: &str| -> Option<String> {
            let rest = url.split_once("://")?.1;
            let end = rest.find(['/', '?']).unwrap_or(rest.len());
            Some(rest[..end].to_ascii_lowercase())
        };
        match (host_of(base_url), host_of(target_url)) {
            (Some(allowed), Some(actual)) if allowed == actual => Ok(()),
            (allowed, actual) => Err(AffinityError::Transport {
                route_key: String::new(),
                reason: format!(
                    "allowlist violation: target host {actual:?} != spec host {allowed:?}"
                ),
                retryable: false,
            }),
        }
    }

    /// POST JSON(含重试退避 + 熔断协作),返回状态码与原始字节
    ///
    /// 调用方(适配器)负责:URL 拼装、鉴权头构造、熔断器与限流器的
    /// 每通道实例;本方法负责单次请求生命周期与重试决策。
    pub async fn post_json(
        &self,
        route_key: &str,
        url: &str,
        headers: &[(String, String)],
        body: &serde_json::Value,
        breaker: &CircuitBreaker,
        limiter: &RateLimiter,
    ) -> Result<TransportResponse, AffinityError> {
        if !breaker.allow() {
            return Err(AffinityError::Transport {
                route_key: route_key.to_string(),
                reason: "circuit breaker open (channel cooling down)".into(),
                retryable: false,
            });
        }
        let mut last_err: Option<AffinityError> = None;
        for attempt in 0..MAX_ATTEMPTS {
            // 限流等待(无锁获取应等时长,async sleep 不持任何锁,C7 合规)
            let delay = limiter.acquire_delay();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            if attempt > 0 {
                // 指数退避 + 全抖动(避免重试风暴同步化)
                let backoff = RETRY_BASE_MS << (attempt - 1);
                let jitter = Self::jitter_ms(backoff);
                tokio::time::sleep(Duration::from_millis(backoff + jitter)).await;
            }

            match self.send_once(url, headers, body).await {
                Ok(resp) if resp.status == 429 || resp.status >= 500 => {
                    breaker.record_failure();
                    last_err = Some(AffinityError::Transport {
                        route_key: route_key.to_string(),
                        reason: format!("HTTP {} (attempt {})", resp.status, attempt + 1),
                        retryable: true,
                    });
                }
                Ok(resp) => {
                    breaker.record_success();
                    return Ok(resp);
                }
                Err(e) if e.is_retryable() => {
                    breaker.record_failure();
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        // 重试耗尽:返回最后一次错误(unwrap_or_else 兜底,禁 expect)
        Err(last_err.unwrap_or_else(|| AffinityError::Transport {
            route_key: route_key.to_string(),
            reason: "retries exhausted".into(),
            retryable: true,
        }))
    }

    /// 单次 HTTP 请求(错误分类:超时可重试,连接/TLS 不可重试)
    async fn send_once(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &serde_json::Value,
    ) -> Result<TransportResponse, AffinityError> {
        let mut req = self.client.post(url).json(body);
        for (name, value) in headers {
            req = req.header(name, value);
        }
        let resp = req.send().await.map_err(|e| AffinityError::Transport {
            route_key: String::new(),
            // WHY 不带 e 的 Display 全文: reqwest 错误可能含完整 URL(带路径),
            // 保守截断为分类描述,避免日志泄漏面
            reason: if e.is_timeout() {
                "request timeout".into()
            } else if e.is_connect() {
                "connection failed".into()
            } else {
                "request failed".into()
            },
            retryable: e.is_timeout(),
        })?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|_| AffinityError::Transport {
                route_key: String::new(),
                reason: "body read failed".into(),
                retryable: true,
            })?
            .to_vec();
        Ok(TransportResponse { status, body })
    }

    /// 抖动:借系统时钟低位做轻量伪随机(0..backoff/2)
    ///
    /// WHY 不引入 rand 依赖: 抖动只需打散同步性,不需要统计随机质量;
    /// 纳秒低位足够(faae-router EDSB 用 rand 是因概率均衡需可复现种子,
    /// 此处场景不同)。
    fn jitter_ms(backoff: u64) -> u64 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        nanos % (backoff / 2 + 1)
    }
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_after_threshold_and_half_opens() {
        let b = CircuitBreaker::new();
        assert!(b.allow());
        for _ in 0..BREAKER_THRESHOLD {
            b.record_failure();
        }
        assert!(b.is_open());
        assert!(!b.allow(), "开闸冷却期内拒绝放行");
        // 成功恢复路径:半开探测成功 → 关闭(直接驱动状态验证语义)
        b.record_success();
        assert!(!b.is_open());
        assert!(b.allow());
    }

    #[test]
    fn breaker_success_resets_failure_count() {
        let b = CircuitBreaker::new();
        for _ in 0..BREAKER_THRESHOLD - 1 {
            b.record_failure();
        }
        b.record_success();
        // 清零后再失败一次不应开闸
        b.record_failure();
        assert!(!b.is_open());
    }

    #[test]
    fn rate_limiter_unlimited_when_no_rpm() {
        let l = RateLimiter::from_rpm(None);
        for _ in 0..100 {
            assert_eq!(l.acquire_delay(), Duration::ZERO);
        }
    }

    #[test]
    fn rate_limiter_spaces_out_requests() {
        // rpm=60 → 间隔 1s:第 1 个立即,第 2 个应等待 ~1s
        let l = RateLimiter::from_rpm(Some(60));
        assert_eq!(l.acquire_delay(), Duration::ZERO);
        let wait = l.acquire_delay();
        assert!(
            wait > Duration::from_millis(900) && wait <= Duration::from_secs(1),
            "第二次许可应等待约 1s,实际 {wait:?}"
        );
    }

    #[test]
    fn allowlist_matches_host_only() {
        let base = "https://api.deepseek.com";
        assert!(
            Transport::check_allowlist(base, "https://api.deepseek.com/chat/completions").is_ok()
        );
        // 不同 host 拒绝(R9 零信任:spec 之外零出网面)
        assert!(Transport::check_allowlist(base, "https://evil.example.com/chat").is_err());
        // 子域也拒绝(精确匹配,不做后缀宽容)
        assert!(Transport::check_allowlist(base, "https://x.api.deepseek.com/chat").is_err());
    }

    #[test]
    fn allowlist_case_insensitive_host() {
        assert!(Transport::check_allowlist(
            "https://API.DeepSeek.com",
            "https://api.deepseek.com/v1"
        )
        .is_ok());
    }
}
