//! HCW 召回指标采集器 — PROBE P0.4（EWMA 漂移跟踪）
//!
//! 对应任务: PROBE 实施计划 §2.2 P0.4（efficiency-monitor 召回 collector）
//! 对应事件: `HcwRecallReported` / `HcwRecallDegraded`（均 Normal 级观测面）
//!
//! # 核心职责
//! - 订阅 `HcwRecallReported` 事件，对三项召回指标做 EWMA 平滑（f32 全程）
//! - 维护最近报告快照（tier / 原始值 / 计数），供 TUI 与监控拉取（拉模式）
//! - 为 P2 召回哨兵提供退化判定基础（连续 2 次低于基线 80% → 升档建议）
//!
//! # 并发模型
//! `Arc<Mutex<RecallStats>>` 共享（单写者订阅循环 + 多读者快照拉取）；
//! 与 `EventMetricCollector` 的 `Arc<DashMap>` 模式同构，但召回统计是
//! 标量聚合（非计数表），`Mutex` 更轻量（未争用 lock ≈ 20ns）。
//!
//! # 红线
//! - **subscribe 先于 spawn**（§4.4 反模式 3）：`start()` 在 `tokio::spawn`
//!   之前同步 `bus.subscribe()`，避免事件静默丢失
//! - f32 全程不转 f64（EWMA 计算保持 f32）
//! - `#![forbid(unsafe_code)]`（crate 级已声明）

use std::sync::{Arc, Mutex};

use event_bus::{EventBus, NexusEvent};

/// EWMA 平滑系数 — 新观测值权重（默认 0.3）
///
/// WHY 0.3: 对最新评测结果适度加权（30%），兼顾趋势跟踪与噪声抑制。
/// 与 `shadow_mode.rs::EWMA_PROMOTION_THRESHOLD`（0.7 阈值）语义正交：
/// 该常量是解冻判定阈值，本常量是平滑系数，二者不冲突。
pub const DEFAULT_EWMA_ALPHA: f32 = 0.3;

/// 召回指标快照 — 单次拉取的完整统计
///
/// # 字段
/// - `ewma_needle_at_8`: 多针召回率 EWMA ∈ [0,1]（None = 未收到报告）
/// - `ewma_position_bias`: 位置偏置比 EWMA ∈ [0,1]
/// - `ewma_chain_success`: 链路成功率 EWMA ∈ [0,1]
/// - `report_count`: 已接收报告数（诊断：0 表示尚无数据）
/// - `last_tier`: 最近报告窗口档（如 "L2"）
/// - `last_needle_at_8`: 最近原始值（未平滑，供哨兵判定）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecallStats {
    /// 多针召回率 EWMA ∈ [0,1]
    pub ewma_needle_at_8: Option<f32>,
    /// 位置偏置比 EWMA ∈ [0,1]
    pub ewma_position_bias: Option<f32>,
    /// 链路成功率 EWMA ∈ [0,1]
    pub ewma_chain_success: Option<f32>,
    /// 已接收报告数
    pub report_count: u64,
    /// 最近报告窗口档
    pub last_tier: Option<String>,
    /// 最近原始多针召回率（未平滑）
    pub last_needle_at_8: Option<f32>,
}

impl RecallStats {
    /// 是否已有数据（收到至少一次报告）
    pub fn has_data(&self) -> bool {
        self.report_count > 0
    }

    /// 最近报告是否低于基线 80%（P2 哨兵升档判定的基础）
    ///
    /// # 参数
    /// - `baseline_needle_at_8`: P0 冻结的基线多针召回率
    ///
    /// # 返回值
    /// `true` 当且仅当已有数据且最近原始值 < 基线 × 0.8
    pub fn below_baseline_80(&self, baseline_needle_at_8: f32) -> bool {
        match self.last_needle_at_8 {
            Some(v) => v < baseline_needle_at_8 * 0.8,
            None => false,
        }
    }
}

/// HCW 召回指标采集器 — 订阅 HcwRecallReported 并维护 EWMA 快照
#[derive(Debug, Clone)]
pub struct RecallCollector {
    /// 共享统计快照（单写多读）
    stats: Arc<Mutex<RecallStats>>,
    /// EWMA 平滑系数
    alpha: f32,
}

impl Default for RecallCollector {
    fn default() -> Self {
        Self::new(DEFAULT_EWMA_ALPHA)
    }
}

impl RecallCollector {
    /// 创建采集器
    ///
    /// # 参数
    /// - `alpha`: EWMA 平滑系数（默认 0.3，见 [`DEFAULT_EWMA_ALPHA`]）
    pub fn new(alpha: f32) -> Self {
        // 钳位 alpha 到 (0, 1] 防止非法配置（0 或负值导致 EWMA 无更新）
        let alpha = alpha.clamp(1e-6, 1.0);
        Self {
            stats: Arc::new(Mutex::new(RecallStats::default())),
            alpha,
        }
    }

    /// 取当前统计快照（拉模式，TUI/监控消费）
    ///
    /// # 返回值
    /// 快照拷贝（Mutex 锁内 clone 后释放，不跨锁边界返回引用）
    pub fn snapshot(&self) -> RecallStats {
        self.stats
            .lock()
            .expect("recall stats lock poisoned")
            .clone()
    }

    /// 应用单条事件（同步处理，测试与无 runtime 场景用）
    ///
    /// # 参数
    /// - `event`: NexusEvent（仅处理 `HcwRecallReported`，其余忽略）
    ///
    /// # 返回值
    /// `true` 当且仅当事件被消费（HcwRecallReported）
    pub fn apply_event(&self, event: &NexusEvent) -> bool {
        let NexusEvent::HcwRecallReported {
            tier,
            needle_recall_at_8,
            position_bias,
            chain_success_rate,
            ..
        } = event
        else {
            return false;
        };
        let mut stats = self.stats.lock().expect("recall stats lock poisoned");
        // EWMA 平滑（f32 全程，禁止 as f64 红线）
        let prev_needle = stats.ewma_needle_at_8.unwrap_or(*needle_recall_at_8);
        let prev_bias = stats.ewma_position_bias.unwrap_or(*position_bias);
        let prev_chain = stats.ewma_chain_success.unwrap_or(*chain_success_rate);
        stats.ewma_needle_at_8 =
            Some(self.alpha * *needle_recall_at_8 + (1.0 - self.alpha) * prev_needle);
        stats.ewma_position_bias =
            Some(self.alpha * *position_bias + (1.0 - self.alpha) * prev_bias);
        stats.ewma_chain_success =
            Some(self.alpha * *chain_success_rate + (1.0 - self.alpha) * prev_chain);
        stats.report_count += 1;
        stats.last_tier = Some(tier.clone());
        stats.last_needle_at_8 = Some(*needle_recall_at_8);
        true
    }

    /// 启动后台订阅循环
    ///
    /// # 参数
    /// - `bus`: EventBus 克隆（broadcast 通道）
    ///
    /// # 红线
    /// **subscribe 先于 spawn**：`bus.subscribe()` 在 `tokio::spawn` 之前同步调用，
    /// 否则事件在订阅建立前发布会静默丢失（§4.4 反模式 3）。
    ///
    /// # 返回
    /// 无（订阅由后台任务持有至进程结束；EventReceiver 非 Clone，
    /// move 进任务闭包，drop 即停止——进程生命周期内持续订阅）
    pub fn start(&self, bus: EventBus) {
        // 同步订阅（spawn 之前）——红线：broadcast 不缓存历史消息
        let mut rx = bus.subscribe();
        let collector = self.clone();
        tokio::spawn(async move {
            // while let: Ok 处理、Err(Lagged/Closed) 自动退出——Normal 级观测面事件允许丢失补偿
            while let Ok(event) = rx.recv().await {
                collector.apply_event(&event);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    fn recall_event(tier: &str, needle: f32, bias: f32, chain: f32) -> NexusEvent {
        NexusEvent::HcwRecallReported {
            metadata: EventMetadata::new("hcw-window"),
            tier: tier.into(),
            needle_recall_at_8: needle,
            position_bias: bias,
            chain_success_rate: chain,
            selected_count: 100,
        }
    }

    #[test]
    fn test_initial_snapshot_empty() {
        let collector = RecallCollector::default();
        let snap = collector.snapshot();
        assert!(!snap.has_data());
        assert!(snap.ewma_needle_at_8.is_none());
        assert_eq!(snap.report_count, 0);
    }

    #[test]
    fn test_apply_recall_event() {
        let collector = RecallCollector::default();
        assert!(collector.apply_event(&recall_event("L2", 0.90, 0.85, 0.80)));
        let snap = collector.snapshot();
        assert!(snap.has_data());
        assert_eq!(snap.report_count, 1);
        assert_eq!(snap.last_tier.as_deref(), Some("L2"));
        // 首个观测值直接成为 EWMA（无历史）
        assert!((snap.ewma_needle_at_8.unwrap() - 0.90).abs() < 1e-6);
        assert!((snap.ewma_position_bias.unwrap() - 0.85).abs() < 1e-6);
        assert!((snap.ewma_chain_success.unwrap() - 0.80).abs() < 1e-6);
    }

    #[test]
    fn test_apply_unrelated_event_ignored() {
        let collector = RecallCollector::default();
        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        assert!(!collector.apply_event(&unrelated));
        assert!(!collector.snapshot().has_data());
    }

    #[test]
    fn test_ewma_smoothing() {
        let collector = RecallCollector::new(0.5);
        collector.apply_event(&recall_event("L2", 1.0, 1.0, 1.0));
        collector.apply_event(&recall_event("L2", 0.0, 0.0, 0.0));
        let snap = collector.snapshot();
        // 第二次 EWMA = 0.5*0.0 + 0.5*1.0 = 0.5（f32 全程）
        assert!((snap.ewma_needle_at_8.unwrap() - 0.5).abs() < 1e-5);
        assert_eq!(snap.report_count, 2);
    }

    #[test]
    fn test_below_baseline_80() {
        let collector = RecallCollector::default();
        // 无数据 → false
        assert!(!collector.snapshot().below_baseline_80(0.9));
        // 0.85 ≥ 0.9×0.8=0.72 → false
        collector.apply_event(&recall_event("L2", 0.85, 0.8, 0.8));
        assert!(!collector.snapshot().below_baseline_80(0.9));
        // 0.70 < 0.72 → true（触发 P2 哨兵升档判定）
        collector.apply_event(&recall_event("L2", 0.70, 0.8, 0.8));
        assert!(collector.snapshot().below_baseline_80(0.9));
    }

    #[test]
    fn test_alpha_clamped() {
        // 非法 alpha（0 / 负值）钳位到 (0,1]
        let c1 = RecallCollector::new(0.0);
        assert!(c1.alpha > 0.0);
        let c2 = RecallCollector::new(-0.5);
        assert!(c2.alpha > 0.0);
        let c3 = RecallCollector::new(1.5);
        assert!(c3.alpha <= 1.0);
    }

    #[tokio::test]
    async fn test_start_subscribes_before_spawn() {
        let bus = EventBus::new();
        let collector = RecallCollector::default();
        // start() 内部先 subscribe 再 spawn（红线）
        collector.start(bus.clone());
        // publish 为 async 方法，必须 await 才会真正广播（未 await 的 future 被丢弃）
        bus.publish(recall_event("L1", 0.95, 0.9, 0.85))
            .await
            .expect("publish should succeed");
        // 给后台任务处理时间（bounded wait）
        for _ in 0..20 {
            if collector.snapshot().has_data() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let snap = collector.snapshot();
        assert!(snap.has_data(), "collector should receive published event");
        assert!((snap.ewma_needle_at_8.unwrap() - 0.95).abs() < 1e-5);
    }
}
