//! capability_probe — 有效窗口探针(HCW v3 蓝图 §5.3,P0 交付物)
//!
//! 对应架构层:L10 mca-gateway(探针执行与留痕)→ L0 nexus-contracts(档案契约)
//! 对应设计源:`HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md` §5.3 / §九
//!
//! # 标定流程(蓝图 §5.3 ②)
//!
//! 四档长度 {0.25, 0.5, 0.75, 0.9} × 标称窗口,逐档发探针请求取检索准确率,
//! 拟合衰减曲线取 80% 检索准确率截点 = effective_window(置信度 =
//! [`CalibrationTier::OwnProbe`])。**探针执行器由调用方注入**
//! ([`EffectiveWindowProbe::calibrate`] 的闭包参数):本模块只承载调度、
//! 拟合与治理护栏,不绑协议/厂商——真实执行器(5 针检索探针,走最便宜
//! flash 通道 + 缓存命中)属组合根装配面。
//!
//! # observe 档语义(蓝图 §5.3,ADR-198 三态)
//!
//! 对齐 ADR-198 阈值校准闭环的 off/observe/apply 三态,**默认
//! [`ProbeMode::Observe`]**(蓝图原文两处明确):
//!
//! - [`ProbeMode::Off`]:探针关闭,`calibrate` 在调用执行器**之前**拒绝
//!   (类型上保证不烧 token——探 1M 档每针百万级);
//! - [`ProbeMode::Observe`](**默认**):只测量 + 发布
//!   `ContextCapabilityUpdated` 事件 + tracing 留档,**不回写任何 L0 档案**——
//!   profile 以共享引用传入,本模块无写入面;hcw-window(L2) 的定容输入
//!   仍读先验值,事件仅作观测留痕(下游是否消费属 L2 自身的裁决面);
//! - [`ProbeMode::Apply`]:同 Observe 流程,但 report 标记可供回写 L0
//!   (`CalibrationReport.mode` 字段)驱动定容——**转 apply 需独立治理裁决**
//!   (RSI 三件套守门),本骨架不接组合根,回写接线留待治理解锁。
//!
//! # 成本护栏(蓝图 §九风险:探针烧 token)
//!
//! observe 档限频:[`MIN_PROBE_INTERVAL`] 内重复标定被拒
//! ([`ProbeError::RateLimited`]);"最便宜 flash 通道 + 缓存命中"由注入的
//! 执行器自行保证(通道选择是装配面知识,不是探针框架知识)。
//!
//! # 锁纪律(C7)
//! [`EffectiveWindowProbe`] 的限频时间戳用 `std::sync::Mutex<Option<Instant>>`,
//! 临界区只有读/写一个 `Instant`,不跨 `.await`。

use std::error::Error as StdError;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::mca::context_capability::{CalibrationTier, ContextCapabilityProfile};

/// 事件源标识(EventMetadata.source,对齐 adapters/hcw_integration 惯例)
const EVENT_SOURCE: &str = "mca-gateway";

/// 探针四档长度(× 标称窗口,蓝图 §5.3)
const PROBE_FRACTIONS: [f32; 4] = [0.25, 0.5, 0.75, 0.9];

/// 检索准确率截点(80%,蓝图 §5.3:fitted 衰减曲线跌破该值即有效窗口边界)
const ACCURACY_CUTOFF: f32 = 0.8;

/// observe 档最小探针间隔(限频护栏,蓝图 §九"observe 档限频")
///
/// 1 小时:探 1M 档 = 每针百万级 token,observe 档只求" periodically 有观测"
/// 而非实时;apply 档的标定节奏由治理另行裁决(不受本护栏约束——apply 档
/// 本身需独立治理开启)。
pub const MIN_PROBE_INTERVAL: Duration = Duration::from_secs(3600);

/// 探针执行器错误 — 擦除边界(`BoxError` 为 hyper/tower 生态惯例;
/// 执行器是外部注入边界,错误具体类型不属于探针框架知识)
type BoxError = Box<dyn StdError + Send + Sync>;

// ============================================================
// 探针模式(ADR-198 三态)
// ============================================================

/// 探针模式 — off/observe/apply 三态(对齐 ADR-198 阈值校准闭环)
///
/// 默认 [`ProbeMode::Observe`](蓝图 §5.3"默认 observe,这本身就是
/// RSI 三件套的一次实例"):先留证据,后转决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProbeMode {
    /// 探针关闭 — `calibrate` 直接拒绝,执行器零调用(不烧 token)
    Off,
    /// 只记录不下决策 — 测量 + 事件 + 留档,不改写 resident 定容输入(默认)
    #[default]
    Observe,
    /// 探针结果参与定容 — 需独立治理裁决(RSI 三件套守门)后开启
    Apply,
}

impl ProbeMode {
    /// 稳定字符串标识(tracing/留档用)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Observe => "observe",
            Self::Apply => "apply",
        }
    }
}

// ============================================================
// 探针错误
// ============================================================

/// 探针标定错误
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// 探针被治理关闭(mode = off)
    #[error("probe disabled (mode = off)")]
    Disabled,
    /// 限频命中 — 距上次标定不足 [`MIN_PROBE_INTERVAL`](蓝图 §九 observe 限频)
    #[error(
        "probe rate-limited: {secs_since_last}s since last run < min interval {min_interval_secs}s"
    )]
    RateLimited {
        /// 距上次标定的秒数
        secs_since_last: u64,
        /// 最小间隔秒数
        min_interval_secs: u64,
    },
    /// 单档探针请求失败 — 四档缺一不可拟合,整次标定失败
    #[error("probe step failed at {window} tokens")]
    Step {
        /// 失败档位的窗口长度(token)
        window: u32,
        /// 执行器原始错误(#[source] 保留错误链)
        #[source]
        source: BoxError,
    },
    /// 执行器返回的准确率越界(∉ [0, 1]) — 执行器 bug,拒绝入库污染拟合
    #[error("probe accuracy {accuracy} out of [0,1] at {window} tokens")]
    AccuracyOutOfRange {
        /// 越界档位的窗口长度(token)
        window: u32,
        /// 越界的准确率原始值
        accuracy: f32,
    },
}

// ============================================================
// 标定报告
// ============================================================

/// 单档探针点 — 窗口长度 + 该档检索准确率
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbePoint {
    /// 该档窗口长度(token,= fraction × 标称)
    pub window: u32,
    /// 该档检索准确率(∈ [0, 1],执行器注入)
    pub accuracy: f32,
}

/// 一次标定的完整报告 — 观测留痕载体(可回放,蓝图 P0 验收线"探针数据可回放")
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationReport {
    /// 路由键 `provider/model`(与 `ModelAffinitySpec::route_key` 同形)
    pub route_key: String,
    /// 标称窗口(官方,token)
    pub advertised_window: u32,
    /// 标定出的有效窗口(80% 截点,token)
    pub effective_window: u32,
    /// 标定置信度(自家探针恒为 [`CalibrationTier::OwnProbe`])
    pub confidence: CalibrationTier,
    /// 四档原始数据(拟合输入,审计/回放用)
    pub accuracy_points: Vec<ProbePoint>,
    /// 本次标定使用的探针模式(Observe = 留痕不驱动定容;Apply = 可回写 L0)
    pub mode: ProbeMode,
}

// ============================================================
// 有效窗口探针
// ============================================================

/// 有效窗口探针 — 调度四档测量 + 80% 截点拟合 + 治理护栏(observe 限频/三态)
///
/// 构造:[`EffectiveWindowProbe::new`](无 bus,单测/录播)或
/// [`EffectiveWindowProbe::with_bus`](标定后广播 `ContextCapabilityUpdated`)。
///
/// WHY 手写 Debug:`EventBus` 未实现 `Debug`,无法 derive(对齐 adapters.rs 先例)。
pub struct EffectiveWindowProbe {
    /// 探针模式(三态治理闸门)
    mode: ProbeMode,
    /// 最小标定间隔(限频护栏)
    min_interval: Duration,
    /// 上次标定起始时刻(限频状态;Mutex 临界区不跨 await,C7)
    last_probe_at: Mutex<Option<Instant>>,
    /// 事件总线(None = 静默,单测/录播模式,对齐 VendorAdapter 先例)
    bus: Option<EventBus>,
}

impl std::fmt::Debug for EffectiveWindowProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectiveWindowProbe")
            .field("mode", &self.mode)
            .field("min_interval", &self.min_interval)
            .field("last_probe_at", &self.last_probe_at)
            .field("bus", &self.bus.is_some())
            .finish()
    }
}

impl EffectiveWindowProbe {
    /// 创建探针(默认限频 [`MIN_PROBE_INTERVAL`],无事件总线)
    pub fn new(mode: ProbeMode) -> Self {
        Self {
            mode,
            min_interval: MIN_PROBE_INTERVAL,
            last_probe_at: Mutex::new(None),
            bus: None,
        }
    }

    /// 创建探针并挂接事件总线(标定完成后广播 `ContextCapabilityUpdated`)
    pub fn with_bus(mode: ProbeMode, bus: EventBus) -> Self {
        Self {
            bus: Some(bus),
            ..Self::new(mode)
        }
    }

    /// 覆写最小标定间隔(builder 风格;单测注入短间隔,生产保持默认护栏)
    pub fn with_min_interval(mut self, min_interval: Duration) -> Self {
        self.min_interval = min_interval;
        self
    }

    /// 当前探针模式
    pub fn mode(&self) -> ProbeMode {
        self.mode
    }

    /// 执行一次有效窗口标定(蓝图 §5.3 ②)
    ///
    /// # 参数
    /// - `profile`: 待标定模型的能力档案(读 `advertised_window`/`provider`/`model`,
    ///   **共享引用**——observe 档语义的编译期保证:本方法无任何 L0 写入面)
    /// - `probe`: 探针执行器闭包,输入窗口长度(token),返回该档检索准确率
    ///   (∈ [0,1]);真实执行器走最便宜 flash 通道 + 缓存命中(蓝图 §九)
    ///
    /// # 流程
    /// 三态闸门 → 限频闸门(锁内检查+置位一次完成,释放后才发请求)→
    /// 四档逐档测量 → 80% 截点拟合 → 发布 `ContextCapabilityUpdated` → 返回报告。
    ///
    /// # 错误
    /// 见 [`ProbeError`]:Off 拒绝 / 限频 / 单档失败 / 准确率越界。
    /// 事件发布失败不中断标定(观测面,对齐 VendorAdapter::publish 先例)。
    pub async fn calibrate<P, Fut>(
        &self,
        profile: &ContextCapabilityProfile,
        probe: P,
    ) -> Result<CalibrationReport, ProbeError>
    where
        P: Fn(u32) -> Fut,
        Fut: Future<Output = Result<f32, BoxError>>,
    {
        // 三态闸门:off 在触碰执行器之前拒绝(类型上保证不烧 token)
        if self.mode == ProbeMode::Off {
            return Err(ProbeError::Disabled);
        }

        // 限频闸门:检查+置位一次锁内完成(并发调用也不叠放),释放后才发请求(C7)
        {
            // WHY 恢复 poison 而非 panic:时间戳是普通值,持锁线程 panic 也不破坏
            // 其自洽性,E-5 lint 禁生产代码 expect(std::sync::PoisonError 惯例)
            let mut last = self
                .last_probe_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(prev) = *last {
                let elapsed = prev.elapsed();
                if elapsed < self.min_interval {
                    return Err(ProbeError::RateLimited {
                        secs_since_last: elapsed.as_secs(),
                        min_interval_secs: self.min_interval.as_secs(),
                    });
                }
            }
            *last = Some(Instant::now());
        }

        // 四档逐档测量(执行器错误/越界即整次失败——四档缺一不可拟合)
        let mut points = Vec::with_capacity(PROBE_FRACTIONS.len());
        for fraction in PROBE_FRACTIONS {
            let window = (profile.advertised_window as f32 * fraction).round() as u32;
            let accuracy = probe(window)
                .await
                .map_err(|e| ProbeError::Step { window, source: e })?;
            if !(0.0..=1.0).contains(&accuracy) {
                return Err(ProbeError::AccuracyOutOfRange { window, accuracy });
            }
            points.push(ProbePoint { window, accuracy });
        }

        let effective_window = fit_80pct_cutoff(&points);

        let report = CalibrationReport {
            route_key: format!("{}/{}", profile.provider.as_str(), profile.model),
            advertised_window: profile.advertised_window,
            effective_window,
            // 自家探针标定恒为 OwnProbe(蓝图 §5.3 置信度分层)
            confidence: CalibrationTier::OwnProbe,
            accuracy_points: points,
            mode: self.mode,
        };

        // observe/apply 档均发布事件 + tracing 留档(留痕;L0 回写仅 apply 档由
        // 组合根接线,本骨架不写——见模块文档 observe 语义节)
        if let Some(bus) = &self.bus {
            let event = NexusEvent::ContextCapabilityUpdated {
                metadata: EventMetadata::new(EVENT_SOURCE),
                route_key: report.route_key.clone(),
                advertised_window: report.advertised_window,
                effective_window: report.effective_window,
                confidence: report.confidence.as_str().to_string(),
            };
            let _ = bus.publish(event).await;
        }
        tracing::info!(
            route_key = %report.route_key,
            advertised = report.advertised_window,
            effective = report.effective_window,
            confidence = report.confidence.as_str(),
            mode = self.mode.as_str(),
            "capability_probe: effective window calibrated (observe/apply 留痕,不改 L0 档案)"
        );

        Ok(report)
    }
}

/// 四点衰减拟合 80% 检索准确率截点(蓝图 §5.3)
///
/// 前置条件:`points` 非空且按 window 升序(calibrate 保证)。
/// 取第一个"准确率从 ≥ 截点下穿到 < 截点"的相邻档间做线性插值;
/// 全档 ≥ 截点 → 衰减未跌穿,保守取最大探针档;全档 < 截点 →
/// 保守取最小探针档。非单调噪声(上升对)跳过,继续找下穿对。
fn fit_80pct_cutoff(points: &[ProbePoint]) -> u32 {
    for pair in points.windows(2) {
        let (len0, acc0) = (pair[0].window as f32, pair[0].accuracy);
        let (len1, acc1) = (pair[1].window as f32, pair[1].accuracy);
        if acc0 >= ACCURACY_CUTOFF && acc1 < ACCURACY_CUTOFF && acc0 > acc1 {
            let t = (acc0 - ACCURACY_CUTOFF) / (acc0 - acc1);
            return (len0 + t * (len1 - len0)).round() as u32;
        }
    }
    if points.last().is_some_and(|p| p.accuracy >= ACCURACY_CUTOFF) {
        points[points.len() - 1].window
    } else {
        points[0].window
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::ProviderId;

    /// 构造标称 1M 的测试档案(数值 fixture,非厂商声明)
    fn profile_1m() -> ContextCapabilityProfile {
        ContextCapabilityProfile::new(
            ProviderId::Zhipu,
            "glm-5.2".into(),
            1_000_000,
            650_000,
            CalibrationTier::ExternalPrior,
            Vec::new(),
            nexus_contracts::mca::context_capability::ThreadSupport::None,
            128_000,
            None,
        )
    }

    #[tokio::test]
    async fn calibrate_fits_80pct_cutoff_with_mock_decay() {
        // mock 衰减函数 accuracy = 1 - 0.5 × (w/1M):四档 0.875/0.75/0.625/0.55,
        // 下穿对 [250K, 500K],解析截点 = 0.4 × 1M = 400_000(整数,无浮点歧义)
        let probe_runner = EffectiveWindowProbe::new(ProbeMode::Observe);
        let report = probe_runner
            .calibrate(&profile_1m(), |w| {
                let acc = 1.0 - 0.5 * (w as f32 / 1_000_000.0);
                async move { Ok(acc) }
            })
            .await
            .unwrap();

        assert_eq!(report.effective_window, 400_000);
        assert_eq!(report.confidence, CalibrationTier::OwnProbe);
        assert_eq!(report.mode, ProbeMode::Observe);
        assert_eq!(report.route_key, "zhipu/glm-5.2");
        assert_eq!(report.advertised_window, 1_000_000);
        assert_eq!(report.accuracy_points.len(), 4);
        // 四档窗口 = fraction × 标称
        let windows: Vec<u32> = report.accuracy_points.iter().map(|p| p.window).collect();
        assert_eq!(windows, vec![250_000, 500_000, 750_000, 900_000]);
    }

    #[tokio::test]
    async fn calibrate_all_above_cutoff_takes_max_probe_length() {
        // 全档准确率恒 0.9(≥ 0.8):衰减未跌穿,保守取最大探针档 0.9 × 1M
        let probe_runner = EffectiveWindowProbe::new(ProbeMode::Observe);
        let report = probe_runner
            .calibrate(&profile_1m(), |_w| async { Ok(0.9_f32) })
            .await
            .unwrap();
        assert_eq!(report.effective_window, 900_000);
    }

    #[tokio::test]
    async fn calibrate_all_below_cutoff_takes_min_probe_length() {
        // 全档准确率恒 0.1(< 0.8):连最小档都衰减穿底,保守取最小探针档 0.25 × 1M
        let probe_runner = EffectiveWindowProbe::new(ProbeMode::Observe);
        let report = probe_runner
            .calibrate(&profile_1m(), |_w| async { Ok(0.1_f32) })
            .await
            .unwrap();
        assert_eq!(report.effective_window, 250_000);
    }

    #[test]
    fn fit_handles_non_monotonic_noise() {
        // 首对为上升噪声(0.6 → 0.9)跳过,第二对 [500K, 750K] 正常下穿:
        // t = (0.9-0.8)/(0.9-0.7) = 0.5 → 500K + 0.5×250K = 625_000
        let points = vec![
            ProbePoint {
                window: 250_000,
                accuracy: 0.6,
            },
            ProbePoint {
                window: 500_000,
                accuracy: 0.9,
            },
            ProbePoint {
                window: 750_000,
                accuracy: 0.7,
            },
            ProbePoint {
                window: 900_000,
                accuracy: 0.4,
            },
        ];
        assert_eq!(fit_80pct_cutoff(&points), 625_000);
    }

    #[tokio::test]
    async fn off_mode_rejects_before_touching_executor() {
        // Off 闸门:执行器零调用(计数器机证"不烧 token")
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe_runner = EffectiveWindowProbe::new(ProbeMode::Off);
        let counter = calls.clone();
        let err = probe_runner
            .calibrate(&profile_1m(), move |w| {
                counter.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                async move { Ok::<f32, BoxError>(1.0 - w as f32 / 1_000_000.0) }
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ProbeError::Disabled));
        assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn rate_limit_blocks_immediate_second_run() {
        // 默认限频 1h:第一次成功后立即重跑必被拒(无需 sleep 真等)
        let probe_runner = EffectiveWindowProbe::new(ProbeMode::Observe);
        probe_runner
            .calibrate(&profile_1m(), |_w| async { Ok(0.9_f32) })
            .await
            .unwrap();
        let err = probe_runner
            .calibrate(&profile_1m(), |_w| async { Ok(0.9_f32) })
            .await
            .unwrap_err();
        match err {
            ProbeError::RateLimited {
                min_interval_secs, ..
            } => assert_eq!(min_interval_secs, 3600),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zero_min_interval_allows_repeated_runs() {
        // 注入零间隔(builder):限频护栏解除,连跑两次都成功
        let probe_runner =
            EffectiveWindowProbe::new(ProbeMode::Observe).with_min_interval(Duration::ZERO);
        for _ in 0..2 {
            probe_runner
                .calibrate(&profile_1m(), |_w| async { Ok(0.9_f32) })
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn executor_error_fails_whole_calibration() {
        // 单档失败(第二档 Err)→ 整次标定失败,错误携带失败档位窗口
        let probe_runner = EffectiveWindowProbe::new(ProbeMode::Observe);
        let err = probe_runner
            .calibrate(&profile_1m(), |w| {
                // WHY 分支收进单个 async block:no two async blocks have the same
                // type——if/else 各自返回独立匿名 Future 类型无法统一
                async move {
                    if w == 500_000 {
                        Err("executor broken".into())
                    } else {
                        Ok(0.9_f32)
                    }
                }
            })
            .await
            .unwrap_err();
        match err {
            ProbeError::Step { window, .. } => assert_eq!(window, 500_000),
            other => panic!("expected Step, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn accuracy_out_of_range_rejected() {
        // 执行器返回越界准确率(1.5)→ 拒绝入库污染拟合
        let probe_runner = EffectiveWindowProbe::new(ProbeMode::Observe);
        let err = probe_runner
            .calibrate(&profile_1m(), |_w| async { Ok(1.5_f32) })
            .await
            .unwrap_err();
        match err {
            ProbeError::AccuracyOutOfRange { accuracy, .. } => assert_eq!(accuracy, 1.5),
            other => panic!("expected AccuracyOutOfRange, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn calibrate_publishes_context_capability_updated() {
        // 标定完成 → ContextCapabilityUpdated 广播(route_key/confidence 同形先验)
        let bus = EventBus::new();
        // WHY 先订阅再标定:broadcast 不缓存历史,订阅必须先于发布(§4.4 反模式 3)
        let mut rx = bus.subscribe();
        let probe_runner = EffectiveWindowProbe::with_bus(ProbeMode::Observe, bus);

        let report = probe_runner
            .calibrate(&profile_1m(), |w| {
                let acc = 1.0 - 0.5 * (w as f32 / 1_000_000.0);
                async move { Ok(acc) }
            })
            .await
            .unwrap();
        assert_eq!(report.effective_window, 400_000);

        // 循环跳过订阅者先收到的自身事件流(对齐 hcw_integration 测试先例)
        let event = loop {
            let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for ContextCapabilityUpdated")
                .expect("recv error");
            if matches!(event, NexusEvent::ContextCapabilityUpdated { .. }) {
                break event;
            }
        };
        match event {
            NexusEvent::ContextCapabilityUpdated {
                route_key,
                advertised_window,
                effective_window,
                confidence,
                ..
            } => {
                assert_eq!(route_key, "zhipu/glm-5.2");
                assert_eq!(advertised_window, 1_000_000);
                assert_eq!(effective_window, 400_000);
                assert_eq!(confidence, "own_probe");
            }
            other => panic!("Expected ContextCapabilityUpdated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn observe_mode_does_not_rewrite_profile() {
        // observe 语义机证:标定前后传入的 L0 档案逐字段不变
        // (共享引用传入 + 探针无写入面,此处数值断言双保险)
        let profile = profile_1m();
        let before = profile.clone();
        let probe_runner = EffectiveWindowProbe::new(ProbeMode::Observe);
        probe_runner
            .calibrate(&profile, |_w| async { Ok(0.9_f32) })
            .await
            .unwrap();
        assert_eq!(profile, before);
    }

    #[test]
    fn probe_mode_default_is_observe() {
        // 蓝图 §5.3 裁决:off/observe/apply 三态默认 observe(蓝图原文两处明确)
        assert_eq!(ProbeMode::default(), ProbeMode::Observe);
        assert_eq!(ProbeMode::Observe.as_str(), "observe");
        assert_eq!(ProbeMode::Off.as_str(), "off");
        assert_eq!(ProbeMode::Apply.as_str(), "apply");
    }
}
