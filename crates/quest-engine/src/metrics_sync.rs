//! 协调度量样本同步 — 订阅审议/委托观测事件,按 Quest 缓存待合并样本
//!
//! 对应架构层:L9 Quest
//! 对应分析:P2-1 后续增强(三重悖论推理悖论红线 — 采集接线闭环)
//!
//! # 核心职责
//! - 订阅 EventBus 上的 `DebateCompleted`(L8 Parliament 发布)与
//!   `DelegationCompleted`(L9 chimera-mas 发布)观测事件
//! - 按 quest_id 缓存审议延迟 / 委托开销 / 共识质量到 [`PendingCoordSample`]
//! - `complete_quest` 时由引擎 take 缓存,经既有 builder 填充
//!   `CoordinationCostSample` / `InferenceGainSample` 的 Option 字段
//!
//! # 依赖铁律合规(§2.2)
//! quest-engine(L9)与 parliament(L8)/chimera-mas(L9)互不直接依赖,
//! 指标接线只走 event-bus 事件(L9 → L1 合法)。
//!
//! # 设计决策(WHY)
//! - 独立模块 + 拆分 `impl QuestEngine`:将采集接线与引擎核心解耦,
//!   与 `control.rs` 控制订阅器的组织方式保持一致
//! - `ingest_metrics_event` 为同步方法:测试可直接投喂事件,无需启动
//!   后台任务;DashMap entry 锁内无 `.await`(§4.4 反模式 #1)
//! - 尽力合并语义:事件丢失/时序错过时缓存字段保持 None,
//!   与 `CoordinationCostSample` 的 Option 设计一致,不阻塞不报错

use event_bus::{EventBus, EventReceiver, NexusEvent};
use std::sync::Arc;
use tracing::{debug, error};

use crate::engine::QuestEngine;

/// 单个 Quest 的待合并协调度量样本
///
/// 由 [`QuestEngine::ingest_metrics_event`] 逐事件填充,
/// `complete_quest` 时 take 并合并进 EWMA 度量收集器。
///
/// # 聚合语义(字段注释中逐一说明 WHY)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingCoordSample {
    /// 议会审议延迟(ms)— **取最近值**
    ///
    /// WHY 最近值:同一 Quest 多次审议时,最近一次审议反映当前决策路径的
    /// 协调成本;历史审议的成本已在其发生时刻由事件流观测,不重复累计。
    pub parliament_debate_latency_ms: Option<f64>,

    /// 多 Agent 委托开销(ms)— **累加**
    ///
    /// WHY 累加:一个 Quest 可能触发多批委托(切块/重试),每批的
    /// wall-clock 开销都真实占用 Quest 生命周期,总开销为各批之和。
    pub delegation_overhead_ms: Option<f64>,

    /// 议会共识质量 proxy [0.0, 1.0] — **取最近 Some 值**
    ///
    /// 取自 `DebateCompleted.weighted_approval_rate`(共识置信度代理)。
    /// FastPath/Vetoed 路径无投票(事件字段为 None)时保留既有值。
    pub consensus_quality: Option<f32>,

    /// TTG 思考模式切换延迟(ms)— **累加**
    ///
    /// WHY 累加:一个 Quest 可能多次切换思考模式(自动选择 + 手动覆盖 +
    /// 预算联动),每次切换的治理开销都计入协调成本。
    pub ttg_switch_latency_ms: Option<f64>,

    /// 意见分歧度 [0.0, 1.0] — **取最近 Some 值**(M2-T2.3 多维观测)
    ///
    /// 取自 `DebateCompleted.divergence`。仅供 tracing/TUI 观测,不进入
    /// InferenceGainSample 主通道(保持 EWMA 以 weighted_approval_rate 为主 proxy)。
    pub divergence: Option<f32>,

    /// 弃权率 [0.0, 1.0] — **取最近 Some 值**(M2-T2.3 多维观测)
    ///
    /// 取自 `DebateCompleted.abstention_rate`。高弃权率提示共识基础薄弱。
    pub abstention_rate: Option<f32>,

    /// 共识裕度 [-1.0, 1.0] — **取最近 Some 值**(M2-T2.3 多维观测)
    ///
    /// 取自 `DebateCompleted.consensus_margin`。负值提示共识仅勉强达成。
    pub consensus_margin: Option<f32>,
}

impl QuestEngine {
    /// 消费单个观测事件,更新待合并样本缓存(同步方法)
    ///
    /// 仅处理 `DebateCompleted` / `DelegationCompleted`,其他事件被忽略。
    /// 测试可直接调用此方法投喂事件,无需启动订阅任务。
    ///
    /// # 并发安全
    /// DashMap entry 写锁仅在本方法内持有,无 `.await` 点(§4.4 反模式 #1)。
    pub fn ingest_metrics_event(&self, event: &NexusEvent) {
        match event {
            NexusEvent::DebateCompleted {
                quest_id,
                debate_latency_ms,
                weighted_approval_rate,
                divergence,
                abstention_rate,
                consensus_margin,
                ..
            } => {
                let mut entry = self.pending_samples().entry(quest_id.clone()).or_default();
                entry.parliament_debate_latency_ms = Some(*debate_latency_ms);
                // 无投票路径(FastPath/Vetoed)字段为 None,保留既有质量值
                if let Some(rate) = weighted_approval_rate {
                    entry.consensus_quality = Some(*rate);
                }
                // M2-T2.3:多维质量取最近 Some 值(无投票路径为 None 时保留旧值)
                if let Some(d) = divergence {
                    entry.divergence = Some(*d);
                }
                if let Some(a) = abstention_rate {
                    entry.abstention_rate = Some(*a);
                }
                if let Some(m) = consensus_margin {
                    entry.consensus_margin = Some(*m);
                }
                debug!(
                    quest_id = %quest_id,
                    debate_latency_ms,
                    "已缓存议会审议延迟与多维质量样本(待 Quest 完成时合并)"
                );
            }
            NexusEvent::DelegationCompleted {
                quest_id,
                total_overhead_ms,
                ..
            } => {
                let Some(quest_id) = quest_id else {
                    // 调用方未设置 AgentTask.quest_id,无法归因(尽力合并语义)
                    debug!(
                        total_overhead_ms,
                        "DelegationCompleted 无 quest_id,跳过委托开销归因"
                    );
                    return;
                };
                let mut entry = self.pending_samples().entry(quest_id.clone()).or_default();
                // 累加语义:多批委托的 wall-clock 开销求和
                *entry.delegation_overhead_ms.get_or_insert(0.0) += *total_overhead_ms;
                debug!(
                    quest_id = %quest_id,
                    total_overhead_ms,
                    "已累加委托开销样本(待 Quest 完成时合并)"
                );
            }
            _ => {}
        }
    }

    /// 记录 TTG 思考模式切换延迟到待合并样本(累加,同步方法)
    ///
    /// 由 `switch_thinking_mode` / `ttg_auto_select` 等 TTG 路径在
    /// 治理调用结束后回填,修复 `complete_quest` 此前硬编码 0.0 的缺口。
    pub(crate) fn record_ttg_latency(&self, quest_id: &str, elapsed_ms: f64) {
        let mut entry = self
            .pending_samples()
            .entry(quest_id.to_string())
            .or_default();
        *entry.ttg_switch_latency_ms.get_or_insert(0.0) += elapsed_ms;
    }

    /// 查询指定 Quest 的待合并样本快照(测试与 TUI 观测用)
    ///
    /// 返回 None 表示该 Quest 尚无任何缓存样本。
    pub fn pending_coordination_sample(&self, quest_id: &str) -> Option<PendingCoordSample> {
        self.pending_samples().get(quest_id).map(|r| r.clone())
    }
}

/// 启动后台协调度量订阅任务
///
/// 订阅者在后台循环接收事件并委托给 `ingest_metrics_event`;
/// 接收错误(通道关闭)时记录日志并退出。
///
/// WHY 先 subscribe 再 spawn:遵循 event-bus "subscribe-before-spawn" 规则,
/// 避免启动瞬间的事件丢失(§4.4 反模式 #3,与 control.rs 一致)。
pub fn spawn_metrics_subscriber(
    engine: Arc<QuestEngine>,
    bus: &EventBus,
) -> tokio::task::JoinHandle<()> {
    let rx = bus.subscribe();
    spawn_metrics_subscriber_with_receiver(engine, rx)
}

/// 从已有接收者启动后台协调度量订阅任务
///
/// 适用于调用方已提前订阅、希望直接传入接收者的场景(如测试)。
pub fn spawn_metrics_subscriber_with_receiver(
    engine: Arc<QuestEngine>,
    mut rx: EventReceiver,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                // ingest 为同步方法,无 handler 错误可传播
                Ok(event) => engine.ingest_metrics_event(&event),
                Err(e) => {
                    error!(error = %e, "协调度量订阅者接收错误,退出");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    fn make_engine() -> QuestEngine {
        QuestEngine::new(EventBus::new())
    }

    fn debate_event(quest_id: &str, latency: f64, approval: Option<f32>) -> NexusEvent {
        NexusEvent::DebateCompleted {
            metadata: EventMetadata::new("parliament"),
            quest_id: quest_id.into(),
            proposal_id: "p-1".into(),
            debate_latency_ms: latency,
            strategy: "full".into(),
            weighted_approval_rate: approval,
            participation_rate: approval.map(|_| 1.0),
            // 多维质量:有投票时携带,无投票(approval=None)时为 None
            divergence: approval.map(|_| 0.3),
            abstention_rate: approval.map(|_| 0.0),
            consensus_margin: approval.map(|a| a - 0.6),
            outcome: if approval.is_some() {
                "Reached".into()
            } else {
                "Vetoed".into()
            },
        }
    }

    fn delegation_event(quest_id: Option<&str>, overhead: f64) -> NexusEvent {
        NexusEvent::DelegationCompleted {
            metadata: EventMetadata::new("chimera-mas:DelegationExecutor"),
            parent_id: "root".into(),
            quest_id: quest_id.map(String::from),
            total_overhead_ms: overhead,
            sub_task_count: 2,
            success_count: 2,
        }
    }

    #[test]
    fn test_ingest_debate_completed_caches_latency_and_quality() {
        let engine = make_engine();
        engine.ingest_metrics_event(&debate_event("q-1", 120.0, Some(0.85)));

        let sample = engine
            .pending_coordination_sample("q-1")
            .expect("应有缓存样本");
        assert_eq!(sample.parliament_debate_latency_ms, Some(120.0));
        assert_eq!(sample.consensus_quality, Some(0.85));
        assert!(sample.delegation_overhead_ms.is_none());
        // M2-T2.3:多维质量也应被缓存
        assert_eq!(sample.divergence, Some(0.3));
        assert_eq!(sample.abstention_rate, Some(0.0));
        assert!(sample.consensus_margin.is_some(), "共识裕度应被缓存");
    }

    #[test]
    fn test_ingest_debate_latest_latency_wins_quality_preserved() {
        // 审议延迟取最近值;Vetoed(无投票)不覆盖既有共识质量
        let engine = make_engine();
        engine.ingest_metrics_event(&debate_event("q-1", 100.0, Some(0.9)));
        engine.ingest_metrics_event(&debate_event("q-1", 50.0, None));

        let sample = engine
            .pending_coordination_sample("q-1")
            .expect("应有缓存样本");
        assert_eq!(sample.parliament_debate_latency_ms, Some(50.0), "取最近值");
        assert_eq!(sample.consensus_quality, Some(0.9), "None 不覆盖既有质量");
    }

    #[test]
    fn test_ingest_delegation_accumulates_overhead() {
        // 委托开销累加(多批委托求和)
        let engine = make_engine();
        engine.ingest_metrics_event(&delegation_event(Some("q-1"), 200.0));
        engine.ingest_metrics_event(&delegation_event(Some("q-1"), 300.0));

        let sample = engine
            .pending_coordination_sample("q-1")
            .expect("应有缓存样本");
        assert_eq!(sample.delegation_overhead_ms, Some(500.0), "多批累加");
    }

    #[test]
    fn test_ingest_delegation_without_quest_id_skipped() {
        // 无 quest_id 无法归因,尽力合并语义:跳过不报错
        let engine = make_engine();
        engine.ingest_metrics_event(&delegation_event(None, 999.0));
        assert!(engine.pending_coordination_sample("q-1").is_none());
    }

    #[test]
    fn test_ingest_ignores_unrelated_events() {
        let engine = make_engine();
        engine.ingest_metrics_event(&NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k".into(),
        });
        assert!(engine.pending_coordination_sample("q-1").is_none());
    }

    #[tokio::test]
    async fn test_spawn_metrics_subscriber_ingests_events() {
        // 后台订阅任务应消费事件并更新缓存(subscribe-before-spawn)
        let bus = EventBus::new();
        let engine = Arc::new(QuestEngine::new(bus.clone()));
        let handle = spawn_metrics_subscriber(Arc::clone(&engine), &bus);

        bus.publish(debate_event("q-sub", 88.0, Some(0.7)))
            .await
            .expect("发布应成功");

        // 轮询等待订阅者消费(异步投递,最多 2s)
        let mut found = false;
        for _ in 0..40 {
            if engine.pending_coordination_sample("q-sub").is_some() {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        handle.abort();
        assert!(found, "订阅者应消费 DebateCompleted 并更新缓存");
    }
}
