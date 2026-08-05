//! 召回哨兵 — 定期注入 needle 评测语料，测量窗口压缩产物的召回质量（PROBE P2.3）
//!
//! 对应架构层: L2 Memory（hcw-window 内部）
//! 对应任务: P2.3-B（哨兵复用 P0 eval harness）
//!
//! # 职责
//!
//! 每 N 个 Quest（可配，默认 50）执行一次闭环测量：
//! 构造确定性 needle 语料（SplitMix64）→ 注入临时 HcwWindow → 压缩 →
//! 对压缩产物计算 `needle_recall@8` / `position_bias` / `chain_success_rate`。
//!
//! # 判定语义（复用 P0.4 below_baseline_80）
//!
//! - `needle_recall_at_8 < baseline × 0.8` 记为一次"低于基线"
//! - **连续 2 次**低于基线 → 发布 `HcwRecallDegraded`（**只告警不升档**——
//!   warn_only 语义；升档动作由影子期（P2.4）裁决）
//! - 正常（≥ baseline × 0.8）→ 复位计数并发布 `HcwRecallReported`
//!
//! # 首个生产发布者
//!
//! `HcwRecallReported` / `HcwRecallDegraded` 两事件变体（P0.3 已加）此前
//! 仅有消费者与测试构造——本哨兵是首个生产发布者，事件契约首次经受真实链路验证。
//!
//! # 热路径隔离
//!
//! 测量为 async 全量重放（约 200 块 insert + 1 次压缩），仅每 N Quest 触发
//! 一次（低频）；事件发布走 EventBus（Normal 级，非 Critical——无 mpsc 需求）。

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::error::HcwError;
use crate::recall::eval::position_bias;
use crate::recall::types::BlockId;
use crate::types::ContextEntry;
use crate::window::HcwWindow;

/// 默认 Quest 触发间隔（每 N 个 Quest 测量一次）
pub const DEFAULT_QUEST_INTERVAL: usize = 50;
/// 默认基线 needle_recall@8（P0 冻结的 static 对照值 0.25）
pub const DEFAULT_BASELINE_NEEDLE_AT_8: f32 = 0.25;
/// 低于基线判定阈值（复用 P0.4 below_baseline_80：last < baseline × 0.8）
pub const BASELINE_FACTOR: f32 = 0.8;
/// 连续低于基线次数达到此值才告警（复用 P0.4"连续 2 次"语义）
pub const DEGRADED_STREAK_THRESHOLD: u32 = 2;

/// 哨兵测量指标（单次闭环测量的产物）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SentinelMetrics {
    /// 多针召回率 needle_recall@8 ∈ [0, 1]
    pub needle_recall_at_8: f32,
    /// 位置偏置比 ∈ [0, 1]
    pub position_bias: f32,
    /// 链路成功率 ∈ [0, 1]
    pub chain_success_rate: f32,
    /// 压缩产物选中块数
    pub selected_count: usize,
}

impl SentinelMetrics {
    /// 是否低于基线（复用 below_baseline_80 语义）
    ///
    /// # 参数
    /// - `baseline`: 基线召回率（P0 冻结对照值）
    pub fn below_baseline_80(&self, baseline: f32) -> bool {
        self.needle_recall_at_8 < baseline * BASELINE_FACTOR
    }
}

/// 哨兵判定结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentinelDecision {
    /// 未到触发间隔（无事发生）
    Skipped,
    /// 指标健康（≥ baseline × 0.8）——已发布 `HcwRecallReported`
    Healthy,
    /// 低于基线但未达连续阈值（观察中）——不发布
    Watching,
    /// 连续 2 次低于基线——已发布 `HcwRecallDegraded`（只告警不升档）
    DegradedWarn,
}

/// 召回哨兵（PROBE P2.3）
///
/// # 用法
///
/// ```rust,ignore
/// let mut sentinel = RecallSentinel::new(bus.clone());
/// // 编排器每完成一个 Quest 调用一次（低频，内部计数节流）
/// if let Some(decision) = sentinel.on_quest().await? {
///     // decision 可记录/上报，不阻塞主链路
/// }
/// ```
pub struct RecallSentinel {
    /// 事件总线（发布 HcwRecallReported / HcwRecallDegraded）
    event_bus: EventBus,
    /// 基线 needle_recall@8（P0 冻结对照值）
    baseline_needle_at_8: f32,
    /// 触发间隔（每 N Quest 测量一次）
    quest_interval: usize,
    /// 已完成 Quest 计数（原子——订阅回调可跨任务）
    quest_counter: AtomicU32,
    /// 误报开关：true = 只告警不升档（本阶段恒 true）
    warn_only: bool,
    /// 连续低于基线次数
    below_streak: u32,
}

impl RecallSentinel {
    /// 创建哨兵（默认间隔 50、基线 0.25、warn_only）
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            event_bus,
            baseline_needle_at_8: DEFAULT_BASELINE_NEEDLE_AT_8,
            quest_interval: DEFAULT_QUEST_INTERVAL,
            quest_counter: AtomicU32::new(0),
            warn_only: true,
            below_streak: 0,
        }
    }

    /// 指定基线（builder；A/B 对照场景传入 probe 侧实测基线）
    pub fn with_baseline(mut self, baseline: f32) -> Self {
        self.baseline_needle_at_8 = baseline;
        self
    }

    /// 设置触发间隔（builder）
    ///
    /// # 参数
    /// - `interval`: 每 N Quest 测量一次（clamp 到 [1, u32::MAX]——
    ///   内部计数器为 u32，超界截断会产生意外触发周期）
    pub fn with_quest_interval(mut self, interval: usize) -> Self {
        self.quest_interval = interval.clamp(1, u32::MAX as usize);
        self
    }

    /// 设置误报开关（builder；本阶段恒 true——只告警不升档）
    pub fn with_warn_only(mut self, warn_only: bool) -> Self {
        self.warn_only = warn_only;
        self
    }

    /// Quest 完成钩子：计数并（到间隔时）执行闭环测量
    ///
    /// # 返回
    /// - `Ok(Some(decision))`: 本次 Quest 触发了测量并完成判定
    /// - `Ok(None)`: 未到触发间隔
    /// - `Err`: 测量失败（窗口操作异常）——哨兵失败不影响主链路（调用方记录日志即可）
    ///
    /// # 红线
    /// - async 内 await publish（EventBus::publish）——Normal 级事件无 mpsc 需求
    /// - 测量在临时窗口上进行，不触碰生产窗口状态
    pub async fn on_quest(&mut self) -> Result<Option<SentinelDecision>, HcwError> {
        let count = self.quest_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if !count.is_multiple_of(self.quest_interval as u32) {
            return Ok(None);
        }
        let metrics = self.measure().await?;
        Ok(Some(self.record(metrics).await))
    }

    /// 执行闭环测量（确定性语料 → 注入 → 压缩 → 指标）
    ///
    /// # 测量流程
    /// 1. `CorpusBuilder` 构造 200 块 / 8 针语料（SplitMix64 确定性）
    /// 2. 临时 HcwWindow 逐块 insert（ContextEntry 包装）
    /// 3. `select_window(0.9)` 触发降级压缩（L3 方向）
    /// 4. 快照压缩产物 → 计算 needle_recall@8 / position_bias / chain_success
    pub async fn measure(&self) -> Result<SentinelMetrics, HcwError> {
        let corpus = crate::recall::eval::CorpusBuilder::new()
            .with_block_count(200)
            .with_needle_count(8)
            .with_needle_topic_bias(0.5)
            .with_temporal_ratio(0.0)
            .build()
            .map_err(|e| HcwError::CompressionFailed(format!("哨兵语料构造失败: {e}")))?;

        let window = HcwWindow::with_default_config(self.event_bus.clone())?;
        for block in &corpus.blocks {
            window
                .insert(ContextEntry::new(
                    block.id.to_string(),
                    "sentinel",
                    &block.content,
                    16,
                ))
                .await?;
        }
        // 降级触发压缩（复杂度 0.9 → 高复杂 → 低档位 → 压缩）
        window.select_window(0.9).await?;

        let selected: Vec<BlockId> = window
            .snapshot_entries()
            .await
            .iter()
            .map(|e| e.id.clone())
            .collect();
        let needles: HashSet<BlockId> = corpus.needle_ids.iter().cloned().collect();

        let needle_recall = crate::recall::eval::needle_recall_at_k(&selected, &needles);
        // 位置偏置：head/middle/tail 各取 needle 子集（无头尾标注时全量作 middle）
        let pb = position_bias(&selected, &HashSet::new(), &needles, &HashSet::new());
        Ok(SentinelMetrics {
            needle_recall_at_8: needle_recall,
            position_bias: pb,
            chain_success_rate: 1.0, // 哨兵语料无多跳链，链路成功率为恒真（不参与判定）
            selected_count: selected.len(),
        })
    }

    /// 判定并发布事件（私有——经 `on_quest` 调用）
    async fn record(&mut self, metrics: SentinelMetrics) -> SentinelDecision {
        if metrics.below_baseline_80(self.baseline_needle_at_8) {
            self.below_streak += 1;
            if self.below_streak >= DEGRADED_STREAK_THRESHOLD {
                // 只告警不升档（warn_only 语义）；若未来允许自动升档，在此分支扩展
                self.publish_degraded(metrics).await;
                SentinelDecision::DegradedWarn
            } else {
                SentinelDecision::Watching
            }
        } else {
            self.below_streak = 0;
            self.publish_reported(metrics).await;
            SentinelDecision::Healthy
        }
    }

    /// 发布健康报告（Normal 级）
    async fn publish_reported(&self, metrics: SentinelMetrics) {
        let _ = self
            .event_bus
            .publish(NexusEvent::HcwRecallReported {
                metadata: EventMetadata::new("hcw-window::sentinel"),
                tier: "sentinel".to_string(),
                needle_recall_at_8: metrics.needle_recall_at_8,
                position_bias: metrics.position_bias,
                chain_success_rate: metrics.chain_success_rate,
                selected_count: metrics.selected_count as u32,
            })
            .await;
    }

    /// 发布退化告警（Normal 级；只告警不升档）
    async fn publish_degraded(&self, metrics: SentinelMetrics) {
        let _ = self
            .event_bus
            .publish(NexusEvent::HcwRecallDegraded {
                metadata: EventMetadata::new("hcw-window::sentinel"),
                tier: "sentinel".to_string(),
                recall_rate: metrics.needle_recall_at_8,
                baseline_recall: self.baseline_needle_at_8,
                reason: "sentinel_2x_below_baseline".to_string(),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_interval_skip() {
        // 未到触发间隔：on_quest 返回 None（不测量不发布）
        let bus = EventBus::new();
        let mut sentinel = RecallSentinel::new(bus).with_quest_interval(5);
        assert!(sentinel.on_quest().await.unwrap().is_none());
        for _ in 0..3 {
            assert!(sentinel.on_quest().await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn test_measure_returns_metrics() {
        // 测量闭环：确定性语料 → 压缩 → 指标可计算
        let bus = EventBus::new();
        let sentinel = RecallSentinel::new(bus);
        let metrics = sentinel.measure().await.unwrap();
        assert!(metrics.needle_recall_at_8 >= 0.0 && metrics.needle_recall_at_8 <= 1.0);
        assert!(metrics.selected_count > 0, "压缩产物不应为空");
    }

    #[tokio::test]
    async fn test_degraded_after_two_streaks() {
        // 连续 2 次低于基线 → DegradedWarn（只告警）；record 直测（指标可控）
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut sentinel = RecallSentinel::new(bus).with_baseline(1.0); // 基线 1.0 → 0.1 < 0.8
        let low = SentinelMetrics {
            needle_recall_at_8: 0.1,
            position_bias: 0.1,
            chain_success_rate: 0.1,
            selected_count: 50,
        };
        // 第一次：Watching（streak=1）
        let d1 = sentinel.record(low).await;
        assert_eq!(d1, SentinelDecision::Watching);
        // 第二次：DegradedWarn（streak=2）→ 发布 Degraded
        let d2 = sentinel.record(low).await;
        assert_eq!(d2, SentinelDecision::DegradedWarn);
        // 发布-消费闭环验证（subscribe 先于 publish ✓）
        let mut saw_degraded = false;
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
        if let Ok(Ok(NexusEvent::HcwRecallDegraded { .. })) = timeout {
            saw_degraded = true;
        }
        assert!(
            saw_degraded,
            "应收到 HcwRecallDegraded（首个生产发布者验证）"
        );
    }

    #[tokio::test]
    async fn test_healthy_resets_streak() {
        // 先退化一次（streak=1）→ 健康测量复位 → 发布 Reported
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut sentinel = RecallSentinel::new(bus).with_baseline(1.0);
        let low = SentinelMetrics {
            needle_recall_at_8: 0.1,
            position_bias: 0.1,
            chain_success_rate: 0.1,
            selected_count: 50,
        };
        assert_eq!(sentinel.record(low).await, SentinelDecision::Watching);
        let high = SentinelMetrics {
            needle_recall_at_8: 0.9,
            position_bias: 0.9,
            chain_success_rate: 0.9,
            selected_count: 60,
        };
        let d = sentinel.record(high).await;
        assert_eq!(d, SentinelDecision::Healthy);
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
        assert!(
            matches!(timeout, Ok(Ok(NexusEvent::HcwRecallReported { .. }))),
            "应收到 HcwRecallReported（健康报告）"
        );
    }
}
