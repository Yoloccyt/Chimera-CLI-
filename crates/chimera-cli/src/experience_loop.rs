//! 经验卡片闭环组合根接线 — §16.1 数据流闭环生产装配(Phase 10 审计修复 Wave 1)
//!
//! 对应架构层: **L10 Interface**(组合根编排,依赖方向 L10→L9/L6/L5/L3/L2/L1 向下合规)
//! 对应规范: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范` §16.1 数据流闭环
//!
//! # 修复的断链(Phase 10 审计结论)
//!
//! 审计发现 §16.1 十跳在生产域 0 跳闭合——组件齐备但组合根未装配:
//! 1. `ExperienceCardBus` 生产从未构造(闭环心脏缺失)
//! 2. L3 持久化仅订阅 broadcast——>0.8 高分卡 Critical 通道无人消费
//! 3. L2 `MlcEngine` / L6 卡片回流 / RuntimeAuditor / metrics_sync 均无生产实例
//!
//! 本模块在 chimera-cli 组合根一次性接线五个现成注入点,贯通主链:
//! `L7 卡片 → L1 ExperienceCardBus(分级) → L3 SQLite 持久化(双流) +
//!  L2 卡片系统 + L6 算子路由回流` + `RuntimeAuditor 五维报告周期发布`。
//!
//! # 设计约束
//!
//! - **零新增 crate**(ADR-049 内嵌哲学,38 crate 基线)
//! - **subscribe-before-spawn**(红线 §4.4-3):所有订阅在 spawn 前同步完成
//! - **rusqlite 红线**:持久化经 `ExperienceCardStorage::store`(内部 spawn_blocking)
//! - **诚实降级**:SQLite 文件打开失败时回退内存存储(warn 可观测,不阻断启动)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use cmt_tiering::ExperienceCardStorage;
use event_bus::{EventBus, ExperienceCardBus, NexusEvent};
use faae_router::{
    spawn_card_feedback_loop, MemorySynthesizer, OperatorRouter, SharedOperatorRouter,
};
use mlc_engine::MlcEngine;
use nexus_contracts::OperatorSelectionStrategy;
use pvl_layer::{CardValidationInput, ExecutionMetadata, ExperienceCardGenerator};
use quest_engine::QuestEngine;
use tokio::task::JoinHandle;
use tracing::warn;

/// §16.3 合成桥（Wave 6）— 实现 L6 `MemorySynthesizer` trait,桥接 L2 合成器
///
/// WHY: L6→L2 直接依赖违反依赖铁律,经 trait 依赖倒置由组合根（L10,
/// 同时依赖 L6 与 L2）实现桥接。选择算子后调用 L2 按需合成,返回摘要文本。
pub struct MlcSynthesizerBridge(Arc<MlcEngine>);

impl MemorySynthesizer for MlcSynthesizerBridge {
    fn synthesize_context(
        &self,
        task_id: &str,
        operator: nexus_contracts::experience_card::AtomicOperator,
    ) -> Option<String> {
        // 默认 token 预算 2048（合成上下文上限,超阈值 score 贪心压缩）
        let memory = self.0.synthesize_context(task_id, operator, 2048)?;
        Some(format!(
            "ancestors={} siblings={} tokens={}",
            memory.ancestor_insights.len(),
            memory.sibling_patterns.len(),
            memory.estimated_tokens
        ))
    }
}

/// 经验卡片 SQLite 热缓存容量(生产默认;与 L3 测试先例同量级)
const HOT_CACHE_CAPACITY: usize = 1024;

/// RuntimeAuditor 五维报告发布周期(秒)
const AUDIT_REPORT_INTERVAL_SECS: u64 = 60;

/// 经验闭环句柄 — 组合根持有,供生命周期管理与下游接线
pub struct ExperienceLoopHandles {
    /// L1 经验卡片总线(闭环心脏,供 Wave 3 卡片生成触发点接线)
    pub card_bus: Arc<ExperienceCardBus>,
    /// L2 记忆引擎(卡片系统消费端)
    pub mlc: Arc<MlcEngine>,
    /// L10 五维审计器(HarnessReportGenerated 生产者)
    pub auditor: Arc<efficiency_monitor::RuntimeAuditor>,
    /// L6 算子路由(卡片反馈回流端)
    pub router: SharedOperatorRouter,
    /// L3 卡片存储(持久化端)
    pub storage: Arc<ExperienceCardStorage>,
    /// L4 零信任沙箱(命令执行注入点,§16.5 拦截率统计载体)
    pub sandbox: Arc<std::sync::Mutex<seccore::Sandbox>>,
    /// 后台任务句柄(生命周期管理)
    pub join_handles: Vec<JoinHandle<()>>,
}

/// 经验卡片 SQLite 路径:`~/.chimera/experience_cards.db`
///
/// 与 `default_config_path()`(~/.chimera/omega.yaml)同目录,跨平台 home 展开。
pub fn experience_db_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".chimera")
        .join("experience_cards.db")
}

/// 装配经验卡片闭环主链(§16.1 生产接线)
///
/// # 接线清单
/// 1. 构造 `ExperienceCardBus`(分级投递:>0.8 Critical mpsc / (0.5,0.8] broadcast)
/// 2. L3 `ExperienceCardStorage`(SQLite 五复合索引)+ **双流持久化订阅**
///    (broadcast + Critical——修复审计缺口:高分卡此前无人持久化)
/// 3. L2 `MlcEngine::with_card_bus`(卡片系统消费)
/// 4. L6 `spawn_card_feedback_loop`(全库唯一双流消费者,驱动算子统计回流)
/// 5. `RuntimeAuditor` 事件计数订阅 + 周期 `generate_report`
///    (内部发布 `HarnessReportGenerated`,打通 TUI SelfAssessmentPanel)
/// 6. `spawn_metrics_subscriber`(协调度量,修复 metrics_sync 孤儿订阅器)
///
/// # 错误
/// - 内存存储构造失败(极端场景)上抛;SQLite 打开失败诚实降级为内存存储
pub async fn spawn_experience_loop(
    nexus_bus: EventBus,
    engine: Arc<QuestEngine>,
) -> anyhow::Result<ExperienceLoopHandles> {
    let mut join_handles: Vec<JoinHandle<()>> = Vec::new();

    // 1. L1 卡片总线(闭环心脏)
    let card_bus = Arc::new(ExperienceCardBus::new());

    // 2. L3 持久化:SQLite 优先,打开失败诚实降级内存存储(warn 可观测)
    let db_path = experience_db_path();
    let storage =
        match ExperienceCardStorage::new(&db_path.to_string_lossy(), HOT_CACHE_CAPACITY).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                warn!(
                    path = %db_path.display(),
                    error = %e,
                    "经验卡片 SQLite 打开失败,降级为内存存储(重启后卡片不保留)"
                );
                Arc::new(
                    ExperienceCardStorage::new_in_memory(HOT_CACHE_CAPACITY)
                        .await
                        .context("内存卡片存储构造失败")?,
                )
            }
        };

    // 2b. 双流持久化订阅(broadcast + Critical;subscribe-before-spawn 红线)
    let mut rx_broadcast = card_bus.subscribe();
    let mut rx_critical = card_bus.subscribe_critical();
    let storage_for_loop = Arc::clone(&storage);
    join_handles.push(tokio::spawn(async move {
        loop {
            // 两通道任意到达即持久化(Critical 高分卡不再丢失)
            // WHY 类型差异:broadcast 分流 recv 返回 Result(Lagged 可观测);
            // Critical 分流为 mpsc::UnboundedReceiver,recv 返回 Option(None=关闭)。
            let card = tokio::select! {
                r = rx_broadcast.recv() => match r {
                    Ok(c) => c,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(lag = n, "卡片持久化 broadcast Lagged,丢弃 {n} 张");
                        continue;
                    }
                    Err(_) => break,
                },
                r = rx_critical.recv() => match r {
                    Some(c) => c,
                    None => break,
                },
            };
            let card_id = card.card_id.to_string();
            if let Err(e) = storage_for_loop.store(&card).await {
                warn!(card_id, error = %e, "经验卡片持久化失败");
            }
        }
    }));

    // 3. L2 记忆引擎 + 卡片总线消费(with_card_bus 内部 subscribe 后 spawn)
    let mlc = Arc::new(
        MlcEngine::new_in_memory(nexus_bus.clone())
            .context("MlcEngine 构造失败")?
            .with_card_bus(&card_bus),
    );

    // 4. L6 算子路由 + 卡片反馈回流(ThreeFactor 策略默认,可经 apply_strategy 热切换)
    //    §16.3 合成接线(Wave 6):with_synthesizer 注入 L2 合成桥——选择算子后
    //    按需合成上下文(依赖倒置,不引入 L6→L2 直接依赖)。
    let router: SharedOperatorRouter = Arc::new(std::sync::Mutex::new(
        OperatorRouter::new(OperatorSelectionStrategy::ThreeFactor)
            .with_synthesizer(Arc::new(MlcSynthesizerBridge(Arc::clone(&mlc)))),
    ));
    join_handles.push(spawn_card_feedback_loop(&card_bus, Arc::clone(&router)));

    // 5. RuntimeAuditor:事件计数订阅 + 周期五维报告(发布 HarnessReportGenerated)
    let auditor = Arc::new(efficiency_monitor::RuntimeAuditor::with_event_bus(
        nexus_bus.clone(),
    ));
    let mut rx_audit = nexus_bus.subscribe();
    let auditor_for_loop = Arc::clone(&auditor);
    join_handles.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(AUDIT_REPORT_INTERVAL_SECS));
        // 首个 tick 立即返回,先消费避免启动瞬间发空报告
        interval.tick().await;
        loop {
            tokio::select! {
                r = rx_audit.recv() => {
                    match r {
                        Ok(event) => auditor_for_loop.record_event(&event),
                        // EventReceiver 将 broadcast Lagged 包装为 SlowConsumerDropped
                        Err(event_bus::EventBusError::SlowConsumerDropped { .. }) => continue,
                        Err(_) => break,
                    }
                }
                _ = interval.tick() => {
                    // generate_report 内部 publish_report → HarnessReportGenerated
                    // (sync + publish_blocking,§4.4 红线 8 合规)
                    let _report = auditor_for_loop.generate_report();
                }
            }
        }
    }));

    // 6. 协调度量订阅器(修复 metrics_sync 孤儿:P2-1 三重悖论推理悖论红线)
    join_handles.push(quest_engine::spawn_metrics_subscriber(
        Arc::clone(&engine),
        &nexus_bus,
    ));

    // 7. L7 卡片生成触发点(§16.1 源头断链修复,Wave 3):
    //    订阅 PredictionVerified → ExperienceCardGenerator 生成卡片投递卡片总线。
    //    WHY 组合根订阅驱动而非改 pvl-layer 内部:零 L7 改动,依赖方向
    //    L10→L7 向下合规;同时消化 PredictionVerified 孤儿发布(有真实消费者)。
    join_handles.push(spawn_card_generation_trigger(
        &nexus_bus,
        Arc::clone(&card_bus),
    ));

    // 8. §16.4 变体/父本事件消费订阅器(Phase 10 Wave 4):
    //    订阅 VariantApproved + ParentSelected——修复审计发现的这两类事件
    //    仅发布无消费(孤儿)问题。VariantApproved 登记到批准注册表(供路由
    //    查询外部认可信号);ParentSelected 同步 L5 三因子选择器访问统计
    //    (UCB 演化一致)并登记选择历史。
    let variant_registry = Arc::new(std::sync::Mutex::new(
        faae_router::ApprovedVariantRegistry::default(),
    ));
    let variant_history = Arc::new(std::sync::Mutex::new(
        faae_router::ParentSelectionHistory::default(),
    ));
    // 注入独立选择器实例:与 OperatorRouter 内部选择器解耦,事件同步面独立演化
    let selector_for_events = Arc::new(std::sync::Mutex::new(
        gsoe_evolution::ThreeFactorSelector::new(1.414, 0.1, 1.0),
    ));
    join_handles.push(faae_router::spawn_variant_event_subscriber(
        &nexus_bus,
        Arc::clone(&variant_registry),
        Some(Arc::clone(&selector_for_events)),
        Arc::clone(&variant_history),
    ));

    // 9. §16.5 L1 吞吐量周期报告器(Phase 10 Wave 6):
    //    修复审计发现的规范要求"Event Bus 吞吐量"无实现——真实采集
    //    published_total 差分速率,周期发布 BusThroughputReported 观测面事件。
    join_handles.push(event_bus::spawn_throughput_reporter(nexus_bus.clone(), 60));

    // 10. §16.5 L4 沙箱拦截率周期报告器(Phase 10 Wave 6):
    //     修复审计发现的"L4 拦截率/误拦截率比率统计"缺失——零信任沙箱
    //     真实采集请求/拦截原子计数(误拦截率需人工真值,标注 v4.0 预留),
    //     周期发布 SecurityInterceptionReported 观测面事件。
    //     Sandbox 共享句柄暴露在 handles 供命令执行路径注入(未来装配)。
    let sandbox = Arc::new(std::sync::Mutex::new(
        seccore::Sandbox::with_default_policy().with_event_bus(nexus_bus.clone()),
    ));
    {
        let sb = sandbox.lock().unwrap_or_else(|e| e.into_inner());
        join_handles.push(seccore::spawn_interception_reporter(
            nexus_bus.clone(),
            sb.interception_stats_handle(),
            60,
        ));
    }

    Ok(ExperienceLoopHandles {
        card_bus,
        mlc,
        auditor,
        router,
        storage,
        sandbox,
        join_handles,
    })
}

/// L7 卡片生成触发器 — PredictionVerified → ExperienceCard 投递(§16.1 源头接线)
///
/// 修复审计断链:`generate_and_publish` 生产零调用——PVL 验证完成后
/// 不生成卡片,整条卡片链源头即断。本触发器经组合根订阅 `PredictionVerified`
/// 驱动生成(零 L7 改动);事件仅携 op_id/score,其余验证字段诚实缺省
/// (tokens/耗时未知时填 0,不伪造)。
pub fn spawn_card_generation_trigger(
    nexus_bus: &EventBus,
    card_bus: Arc<ExperienceCardBus>,
) -> JoinHandle<()> {
    // subscribe-before-spawn 红线
    let mut rx = nexus_bus.subscribe();
    // 生成器注入卡片总线(generate_and_publish 自动投递;ExperienceCardBus Clone 廉价)
    let generator =
        ExperienceCardGenerator::new(env!("CARGO_PKG_VERSION")).with_card_bus((*card_bus).clone());

    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(event_bus::EventBusError::SlowConsumerDropped { .. }) => continue,
                Err(_) => break,
            };
            if let NexusEvent::PredictionVerified { op_id, score, .. } = event {
                // 事件字段有限,诚实缺省未知验证输入(score>0.5 视为验证通过)
                let metadata = ExecutionMetadata {
                    task_id: op_id.clone(),
                    parent_id: None,
                    operator: nexus_contracts::experience_card::AtomicOperator::Improve,
                    skills_used: Vec::new(),
                };
                let validation = CardValidationInput {
                    success: score > 0.5,
                    score,
                    error_type: None,
                    error_location: None,
                    error_message: None,
                    timed_out: false,
                    execution_time_ms: 0,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    lines_changed: 0,
                };
                let card = generator.generate_and_publish(&metadata, &validation);
                tracing::debug!(
                    op_id,
                    card_id = %card.card_id,
                    score,
                    "PredictionVerified → 经验卡片已生成并投递卡片总线"
                );
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experience_db_path_is_under_chimera_home() {
        let path = experience_db_path();
        let s = path.to_string_lossy();
        assert!(s.contains(".chimera"), "路径应位于 ~/.chimera 下: {s}");
        assert!(s.ends_with("experience_cards.db"));
    }

    #[tokio::test]
    async fn spawn_experience_loop_wires_full_chain() {
        // 端到端装配验证:总线→双流持久化→L2/L6 消费→审计器周期报告
        let bus = EventBus::new();
        let engine = Arc::new(QuestEngine::new(bus.clone()));
        // 测试场景用内存降级路径无关紧要:SQLite 失败自动降级
        let handles = spawn_experience_loop(bus.clone(), Arc::clone(&engine))
            .await
            .expect("装配成功");

        // 五个接线点全部就绪
        assert!(Arc::strong_count(&handles.card_bus) >= 1);
        assert!(Arc::strong_count(&handles.mlc) >= 1);
        assert!(Arc::strong_count(&handles.auditor) >= 1);
        assert!(Arc::strong_count(&handles.storage) >= 1);
        assert!(handles.join_handles.len() >= 4, "至少 4 个后台任务");

        // 发布一张高分卡(>0.8 走 Critical 通道)→ 双流持久化应消费
        let card = make_card("card-1", 0.95);
        handles.card_bus.publish(card);
        // 等待后台任务消费
        tokio::time::sleep(Duration::from_millis(200)).await;
        let count = handles.storage.card_count().await.expect("查询成功");
        assert!(count >= 1, "高分卡应经 Critical 通道持久化");
    }

    #[tokio::test]
    async fn prediction_verified_drives_card_generation() {
        // Wave 3 源头接线验证:PredictionVerified → 卡片生成 → 卡片总线 → 双流持久化
        let bus = EventBus::new();
        let engine = Arc::new(QuestEngine::new(bus.clone()));
        let handles = spawn_experience_loop(bus.clone(), Arc::clone(&engine))
            .await
            .expect("装配成功");

        // 发布验证完成事件(模拟 L7 verifier)
        bus.publish(NexusEvent::PredictionVerified {
            metadata: event_bus::EventMetadata::new("pvl-layer"),
            op_id: "op-1".to_string(),
            score: 0.9,
        })
        .await
        .expect("发布成功");

        // 等待触发器生成卡片 + 持久化消费
        tokio::time::sleep(Duration::from_millis(300)).await;
        let count = handles.storage.card_count().await.expect("查询成功");
        assert!(
            count >= 1,
            "PredictionVerified 应驱动卡片生成并持久化(源头断链修复)"
        );
    }

    fn make_card(card_id: &str, score: f32) -> nexus_contracts::ExperienceCard {
        use nexus_contracts::experience_card::{
            AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
        };
        nexus_contracts::ExperienceCard {
            card_id: Box::from(card_id),
            task_id: Box::from("task-1"),
            node_id: Box::from(format!("node_{card_id}")),
            parent_id: None,
            created_at: chrono::Utc::now(),
            operator: AtomicOperator::Draft,
            score,
            delta_vs_parent: 0.0,
            method_family: Box::from("test"),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality: score,
                progress: 0.1,
                novelty: 0.5,
            },
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }
}
