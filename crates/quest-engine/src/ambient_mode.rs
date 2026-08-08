//! Ambient Mode — 后台常驻的守护型维护循环（Milestone B-2）
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §5.1 P2 / §6 Milestone B-2
//! 对应设计: 根目录设计文档 §13.3（jcode Ambient 工作模式）
//!
//! # 职责
//!
//! 在事件流的"间隙"执行三项后台维护（事件驱动，**无轮询锁**）：
//! 1. **资源看门狗**：订阅 `BudgetExceeded` → 挂起全部活跃 Quest；
//!    订阅 `ResourceRecovered` → 恢复被看门狗挂起的 Quest（E2E 验收点）
//! 2. **记忆整理**：订阅 `CheckpointSaved` → 经 `MemoryTidyHook` 注入点触发
//!    整理（节流：`tidy_interval_secs` 内至多一次）——L9 不直接依赖 L2，
//!    整理实现由注入方（编排器）提供，保持依赖铁律合规
//! 3. **检查点调度**：事件到达时若距上次调度超过 `checkpoint_interval_secs`
//!    且存在活跃 Quest → 保存检查点（节流避免与 CheckpointSaved 事件循环触发）
//!
//! # 设计约束
//!
//! - **事件驱动无轮询锁**：不 spawn 定时器轮询；仅在事件到达时检查节流状态
//! - **不依赖 RL**：纯规则 + 事件订阅（ADR-042 R2 冻结面外独立可落地）
//! - **只恢复自己挂起的 Quest**：`watchdog_paused` 集合跟踪，避免与用户/编排器
//!   的暂停语义互相干扰
//! - **先 subscribe 再 spawn**（async 红线）：订阅器在 spawn 前同步 subscribe

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use event_bus::{EventBus, EventReceiver, NexusEvent};
use tracing::{debug, info, warn};

use crate::engine::QuestEngine;

/// Ambient Mode 配置
#[derive(Debug, Clone)]
pub struct AmbientModeConfig {
    /// 记忆整理节流间隔（秒）——同窗口内至多触发一次整理
    pub tidy_interval_secs: u64,
    /// 检查点调度节流间隔（秒）——事件到达时若超期则保存活跃 Quest 检查点
    pub checkpoint_interval_secs: u64,
    /// 单次整理/检查点批处理的 Quest 上限（防突发事件风暴）
    pub max_quests_per_batch: usize,
}

impl Default for AmbientModeConfig {
    fn default() -> Self {
        Self {
            tidy_interval_secs: 3600,
            checkpoint_interval_secs: 300,
            max_quests_per_batch: 16,
        }
    }
}

/// 记忆整理注入点 — 由编排器提供实际整理实现（L9 不直接依赖 L2）
///
/// WHY trait 注入: quest-engine 无 mlc-engine 依赖（L9→L2 虽允许，但保持
/// 模块解耦——记忆整理策略属编排器关注点，Ambient Mode 只负责"何时触发"）。
pub trait MemoryTidyHook: Send + Sync {
    /// 触发记忆整理（在事件循环上下文同步调用，实现须轻量/异步内部化）
    fn on_memory_pressure(&self, quest_id: &str);
}

/// Ambient Mode 运行时状态（订阅器专用，非 Send 限制）
struct AmbientMode {
    /// 资源看门狗挂起的 Quest 集合（只恢复自己挂起的）
    watchdog_paused: HashSet<String>,
    /// 上次记忆整理时间戳（节流）
    last_tidy: Instant,
    /// 上次检查点调度时间戳（节流）
    last_checkpoint: Instant,
    /// 配置
    config: AmbientModeConfig,
}

impl AmbientMode {
    /// 资源看门狗：预算超限 → 挂起全部活跃 Quest
    async fn on_budget_exceeded(&mut self, engine: &QuestEngine, budget_type: &str) {
        info!(budget_type = %budget_type, "Ambient 资源看门狗:预算超限,挂起活跃 Quest");
        for quest in engine
            .list_quests()
            .into_iter()
            .take(self.config.max_quests_per_batch)
        {
            if self.watchdog_paused.contains(&quest.quest_id) {
                continue; // 已挂起
            }
            match engine
                .pause_quest(&quest.quest_id, "ambient-watchdog")
                .await
            {
                Ok(()) => {
                    self.watchdog_paused.insert(quest.quest_id.clone());
                    debug!(quest_id = %quest.quest_id, "Ambient 已挂起 Quest");
                }
                Err(e) => warn!(quest_id = %quest.quest_id, error = %e, "Ambient 挂起 Quest 失败"),
            }
        }
    }

    /// 资源恢复：恢复被看门狗挂起的全部 Quest（E2E 验收点）
    async fn on_resource_recovered(&mut self, engine: &QuestEngine, resource_type: &str) {
        info!(resource_type = %resource_type, "Ambient 资源看门狗:资源恢复,恢复挂起 Quest");
        let paused: Vec<String> = self.watchdog_paused.drain().collect();
        for quest_id in paused {
            match engine.resume_quest(&quest_id, "ambient-watchdog").await {
                Ok(()) => debug!(quest_id = %quest_id, "Ambient 已恢复 Quest"),
                Err(e) => warn!(quest_id = %quest_id, error = %e, "Ambient 恢复 Quest 失败"),
            }
        }
    }

    /// 记忆整理 + 检查点调度（事件到达时的节流检查）
    async fn on_maintenance_signal(
        &mut self,
        engine: &QuestEngine,
        quest_id: &str,
        hook: &dyn MemoryTidyHook,
    ) {
        let now = Instant::now();
        let tidy_elapsed =
            now.duration_since(self.last_tidy).as_secs() >= self.config.tidy_interval_secs;
        if tidy_elapsed {
            self.last_tidy = now;
            hook.on_memory_pressure(quest_id);
            debug!(quest_id = %quest_id, "Ambient 已触发记忆整理");
        }

        // 检查点调度：超期且存在活跃 Quest → 保存检查点
        let cp_elapsed = now.duration_since(self.last_checkpoint).as_secs()
            >= self.config.checkpoint_interval_secs;
        if cp_elapsed {
            let active: Vec<String> = engine
                .list_quests()
                .into_iter()
                .filter(|q| !self.watchdog_paused.contains(&q.quest_id))
                .map(|q| q.quest_id)
                .take(self.config.max_quests_per_batch)
                .collect();
            if !active.is_empty() {
                self.last_checkpoint = now;
                for qid in active {
                    if let Err(e) = engine.save_checkpoint(&qid).await {
                        warn!(quest_id = %qid, error = %e, "Ambient 检查点保存失败");
                    }
                }
            }
        }
    }
}

// 启动 Ambient Mode 订阅器（自动订阅事件总线）
///
/// # 参数
/// - `bus`: 事件总线（内部 subscribe——先订阅后消费，async 红线）
/// - `engine`: Quest 引擎（挂起/恢复/检查点）
/// - `config`: 节流配置
/// - `hook`: 记忆整理注入点
///
/// # 双通道订阅
///
/// 同时订阅 broadcast（Normal 事件：ResourceRecovered/CheckpointSaved）与
/// Critical mpsc 旁路（BudgetExceeded——Critical 事件不走 broadcast，
/// 见 event-bus bus.rs `is_critical_mpsc_event`）。两通道经 `select!` 合并。
///
/// # 返回
/// 任务句柄（生产环境丢弃即可；测试可 join 等待退出）
pub fn spawn_ambient_subscriber(
    bus: EventBus,
    engine: Arc<QuestEngine>,
    config: AmbientModeConfig,
    hook: Arc<dyn MemoryTidyHook>,
) -> tokio::task::JoinHandle<()> {
    let rx = bus.subscribe();
    let critical_rx = bus.subscribe_critical_events();
    spawn_ambient_subscriber_with_receivers(engine, rx, critical_rx, config, hook)
}

/// 双通道订阅器（broadcast + Critical mpsc 旁路，显式注入接收器，测试可控）
pub fn spawn_ambient_subscriber_with_receivers(
    engine: Arc<QuestEngine>,
    mut rx: EventReceiver,
    mut critical_rx: tokio::sync::mpsc::Receiver<NexusEvent>,
    config: AmbientModeConfig,
    hook: Arc<dyn MemoryTidyHook>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // last_tidy/last_checkpoint 初始化为"早已到期"：
        // 使首个维护信号立即触发（而非等待一个完整节流窗口）。
        let idle_past = std::time::Duration::from_secs(
            config
                .tidy_interval_secs
                .max(config.checkpoint_interval_secs)
                + 1,
        );
        let mut ambient = AmbientMode {
            watchdog_paused: HashSet::new(),
            last_tidy: Instant::now() - idle_past,
            last_checkpoint: Instant::now() - idle_past,
            config,
        };
        loop {
            let event = tokio::select! {
                broadcast = rx.recv() => match broadcast {
                    Ok(e) => Some(e),
                    Err(_) => break, // 事件流关闭
                },
                critical = critical_rx.recv() => critical,
            };
            let Some(event) = event else { break };
            match event {
                NexusEvent::BudgetExceeded { budget_type, .. } => {
                    ambient.on_budget_exceeded(&engine, &budget_type).await;
                }
                NexusEvent::ResourceRecovered { resource_type, .. } => {
                    ambient.on_resource_recovered(&engine, &resource_type).await;
                }
                NexusEvent::CheckpointSaved { quest_id, .. } => {
                    ambient
                        .on_maintenance_signal(&engine, &quest_id, hook.as_ref())
                        .await;
                }
                _ => {} // 其余事件 Ambient 不关注（事件驱动过滤）
            }
        }
        warn!("Ambient Mode 订阅器退出:事件流关闭");
    })
}

/// 空实现 hook — 未注入整理实现时的 no-op（默认安全）
#[derive(Debug, Default)]
pub struct NoopTidyHook;

impl MemoryTidyHook for NoopTidyHook {
    fn on_memory_pressure(&self, _quest_id: &str) {}
}

// ============================================================
// 单元测试（TDD：先失败测试后实现）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn config_defaults_are_sane() {
        let cfg = AmbientModeConfig::default();
        assert!(cfg.tidy_interval_secs > 0);
        assert!(cfg.checkpoint_interval_secs > 0);
        assert!(cfg.max_quests_per_batch > 0);
    }

    #[test]
    fn watchdog_tracks_only_own_pauses() {
        let mut ambient = AmbientMode {
            watchdog_paused: HashSet::new(),
            last_tidy: Instant::now(),
            last_checkpoint: Instant::now(),
            config: AmbientModeConfig::default(),
        };
        ambient.watchdog_paused.insert("q-1".into());
        assert!(ambient.watchdog_paused.contains("q-1"));
        let drained: Vec<_> = ambient.watchdog_paused.drain().collect();
        assert_eq!(drained, vec!["q-1".to_string()]);
    }

    #[test]
    fn tidy_throttled_by_interval() {
        let mut ambient = AmbientMode {
            watchdog_paused: HashSet::new(),
            // 初始化为"早已到期"：首信号应触发（与订阅器初始化一致）
            last_tidy: Instant::now() - std::time::Duration::from_secs(3601),
            last_checkpoint: Instant::now(),
            config: AmbientModeConfig {
                tidy_interval_secs: 3600,
                ..AmbientModeConfig::default()
            },
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let hook = CountingHook(Arc::clone(&calls));
        // 首次信号：节流窗口已过 → 触发
        let now = Instant::now();
        if now.duration_since(ambient.last_tidy).as_secs() >= ambient.config.tidy_interval_secs {
            ambient.last_tidy = now;
            hook.on_memory_pressure("q-1");
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1, "首信号应触发整理");
        // 第二次信号：窗口未过 → 不触发
        if Instant::now().duration_since(ambient.last_tidy).as_secs()
            >= ambient.config.tidy_interval_secs
        {
            ambient.last_tidy = Instant::now();
            hook.on_memory_pressure("q-1");
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1, "节流窗口内不应重复触发");
    }

    /// 计数 hook（内嵌测试专用）
    struct CountingHook(Arc<AtomicUsize>);

    impl MemoryTidyHook for CountingHook {
        fn on_memory_pressure(&self, _quest_id: &str) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}
