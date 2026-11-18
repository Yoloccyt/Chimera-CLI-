//! 观测与访问面子模块 — `EventBus` 指标读取、配置访问与吞吐量周期报告器。
//!
//! 从 `bus/mod.rs` 按职责拆出:本文件承载不触碰发布/订阅/分片路径的
//! 访问面(计数器读取、容量/日志/sink 访问、周期上报),方法体逐字平移。

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

use super::EventBus;
use crate::credit_flow::{CreditFlow, CreditStats};
use crate::critical_sink::CriticalSink;
use crate::logging::BusLogger;
use crate::types::{EventMetadata, NexusEvent};

impl EventBus {
    /// E-2 累计被降采样的事件数(运维可见性;默认策略下恒 0)
    #[must_use]
    pub fn downsampled_total(&self) -> u64 {
        self.ds.downsampled.load(Ordering::Relaxed)
    }

    /// E-2 当前是否处于压力区(latch 锁存态;默认策略下恒 false)
    #[must_use]
    pub fn pressure_engaged(&self) -> bool {
        self.ds.engaged.load(Ordering::Relaxed) == 1
    }

    /// 替换 Critical 保底送达 sink(B-a,M0,builder 风格)
    ///
    /// 默认 sink 为 [`LogCriticalSink`](crate::critical_sink::LogCriticalSink)
    /// (结构化 `error!` 日志);需要告警管道/持久化等自定义落点时,以
    /// `Arc<dyn CriticalSink>` 注入替换。仅影响"无 mpsc 订阅者"分支的投递目标,
    /// 既有 mpsc 订阅语义(`subscribe_critical_events`)完全不变。
    ///
    /// # 参数
    /// - `sink`:保底落点实现(`Send + Sync + 'static`,内部 `Arc` 共享)
    ///
    /// # 返回
    /// 消费 `self` 并返回,支持链式调用:`EventBus::new().with_critical_fallback(..)`
    ///
    /// # 示例
    /// ```no_run
    /// use std::sync::Arc;
    /// use event_bus::{EventBus, CriticalSink, NexusEvent};
    ///
    /// struct MySink;
    /// impl CriticalSink for MySink {
    ///     fn on_critical(&self, _event: &NexusEvent) { /* 转发告警管道 */ }
    /// }
    /// let bus = EventBus::new().with_critical_fallback(Arc::new(MySink));
    /// ```
    pub fn with_critical_fallback(mut self, sink: Arc<dyn CriticalSink>) -> Self {
        self.critical_fallback = sink;
        self
    }

    /// 为已有总线设置日志记录器(仅在未设置时生效)
    ///
    /// 返回 self 以便链式调用。
    /// 若已设置 logger,此调用无效果(保留第一个 logger)。
    pub fn set_logger(&mut self, logger: BusLogger) {
        if self.logger.is_none() {
            self.logger = Some(Arc::new(logger));
        }
    }

    /// 获取日志记录器的引用(若已设置)
    pub fn logger(&self) -> Option<&BusLogger> {
        self.logger.as_deref()
    }

    /// 当前订阅者数量(broadcast 接收端计数)
    ///
    /// WHY 观测面:发布方据此判断"事件是否有人听"(无订阅者时 Critical 事件
    /// 仍走保底 sink,见 [`with_critical_fallback`](Self::with_critical_fallback))。
    ///
    /// 注:本方法的 rustdoc 曾误挂 `publish` 的说明(拆分时发现的历史文档错位,
    /// 已就地纠正);`publish` 的说明现位于 `bus/publish.rs::publish`。
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// 获取通道容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 获取背压监控统计(A3 容量监控)
    ///
    /// 返回 `(lagged_count, backpressure_warning_count)`:
    /// - `lagged_count`:broadcast 通道因慢消费者 Lagged 而丢弃的累计事件数
    ///   (接收端递增,反映实际丢失的事件数量)
    /// - `backpressure_warning_count`:缓冲区占用超过 75% 阈值的累计告警次数
    ///   (发送端递增,反映背压触发频率)
    ///
    /// 两个计数器均单调递增,不重置。供 efficiency-monitor 周期性拉取并
    /// 发布告警,同时供 TUI 显示背压趋势。
    ///
    /// WHY 返回元组而非 struct:轻量 API,两个 u64 语义清晰,无需额外类型定义。
    /// WHY Relaxed 内存序:统计指标,非控制流信号,毫秒级读取偏差可容忍。
    pub fn backpressure_stats(&self) -> (u64, u64) {
        let lagged = self.lagged_count.load(Ordering::Relaxed);
        let warnings = self.backpressure_warning_count.load(Ordering::Relaxed);
        (lagged, warnings)
    }

    /// 累计发布事件总数(§16.5 L1 吞吐量指标,Phase 10 Wave 6)
    ///
    /// 监控方周期拉取两次采样差值/间隔即得吞吐速率(真实采集)。
    pub fn published_total(&self) -> u64 {
        self.published_total.load(Ordering::Relaxed)
    }

    /// 信用流观测统计(P1-T11,手册 §8.5 观测面)
    ///
    /// 返回:
    /// - `available`:当前可用信用(256 初始,随 publish 扣减 / release 归还)
    /// - `shed_total`:信用耗尽回退 broadcast 的累计事件数
    ///   (**不是丢弃数** —— 回退后事件仍走 broadcast,仅失去信用流信号)
    /// - `high_wait_total`:高优事件进入等待窗口的累计次数
    ///
    /// WHY 单一方法返回 struct 而非三个 getter:观测方一次性拉取关联指标,
    /// 避免多次原子 load 间读到的状态不一致(available 与 shed 属于同一
    /// 信用生命周期)。
    pub fn credit_stats(&self) -> CreditStats {
        CreditStats {
            available: self.credit_flow.credit_available(),
            shed_total: self.credit_shed_total.load(Ordering::Relaxed),
            high_wait_total: self.credit_flow.high_wait_total(),
        }
    }

    /// 手动归还信用(P1-T11)—— 归还时机文档
    ///
    /// # 归还时机(设计约定)
    /// 订阅者**按消费速率批量归还**(ADR-125 批提交语义):例如每消费 N 个
    /// 事件调用一次 `release_credit(N)`,而不是逐事件归还。
    ///
    /// WHY 逐事件归还的缺点:每次归还都触发 `notify_waiters` 核外唤醒
    /// (挂起→唤醒的调度开销),高频归还会放大调度噪声;批提交将归还
    /// 频率降至与消费批次一致,唤醒成本可忽略。
    ///
    /// # 本任务不强制后台归还任务
    /// 归还由调用方显式触发(本方法 + [`credit_flow`](Self::credit_flow)
    /// 直接访问原语);T12 分片改造时接入后台自动归还(按消费速率授信)。
    ///
    /// # 不会膨胀
    /// 归还封顶到初始信用池(见 [`CreditFlow::release`]),过度归还不破坏
    /// 信用守恒。
    pub fn release_credit(&self, n: u64) {
        self.credit_flow.release(n);
    }

    /// 获取信用流原语引用(高级用途)
    ///
    /// 供调用方执行高优等待语义 [`CreditFlow::acquire_priority`](crate::credit_flow::CreditFlow::acquire_priority)
    /// (如 High 事件在信用不足时异步等待 ≤100ms 窗口)或直接观测
    /// [`CreditFlow::credit_available`]。
    pub fn credit_flow(&self) -> &CreditFlow {
        &self.credit_flow
    }
}

/// §16.5 L1 吞吐量周期报告器(Phase 10 Wave 6)
///
/// 周期拉取 [`EventBus::published_total`] 差分计算事件/秒速率,发布
/// `BusThroughputReported` 观测面事件(真实采集,非伪造指标)。
///
/// 首 tick 立即返回先采样基线,避免窗口 0 除零;窗口长度由
/// `interval_secs` 决定(下限 1 秒,防止 0 间隔忙循环)。
pub fn spawn_throughput_reporter(bus: EventBus, interval_secs: u64) -> tokio::task::JoinHandle<()> {
    let window_secs = interval_secs.max(1);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(window_secs));
        interval.tick().await; // 基线采样:跳过首个立即 tick
        let mut prev = bus.published_total();
        // 第三轮 T1.1:shadow_stats 的首个生产消费者 —— 零订阅丢弃以窗口限速
        // 浮出(≤1 warn/窗口;worker 侧仅首见 warn 一次,长期静默会掩盖持续增长)
        let mut prev_closed = bus.shadow_stats().merge_closed_total;
        loop {
            interval.tick().await;
            let now = bus.published_total();
            let events_per_sec = (now.saturating_sub(prev)) as f64 / window_secs as f64;
            prev = now;
            let closed_now = bus.shadow_stats().merge_closed_total;
            if closed_now > prev_closed {
                warn!(
                    window_delta = closed_now - prev_closed,
                    merge_closed_total = closed_now,
                    "分片汇入零订阅者丢弃增长:merged_total 含此类丢弃,送达观测见 ShadowStats"
                );
                prev_closed = closed_now;
            }
            let event = NexusEvent::BusThroughputReported {
                metadata: EventMetadata::new("event-bus"),
                published_total: now,
                events_per_sec,
                window_secs,
            };
            if let Err(e) = bus.publish_blocking(event) {
                warn!(error = %e, "BusThroughputReported 发布失败");
            }
        }
    })
}
