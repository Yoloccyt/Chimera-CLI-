//! §16.4 变体/父本事件消费订阅器 — VariantApproved + ParentSelected 接线（W4，ADR-084 决策 6）
//!
//! 对应架构层: **L6 Router**（faae-router 子模块）
//! 对应设计源: 规范 §16.4 跨层事件表——VariantApproved 由 L8 Parliament 产生、
//! L5/L6 消费;ParentSelected 由 L5 产生、L6/L9 消费。
//!
//! # 消费语义（诚实数据原则,不伪造统计）
//!
//! - **VariantApproved** → 登记到 [`ApprovedVariantRegistry`]（append-only）:
//!   议会批准过的变体是 L6 算子路由的**外部认可信号**,供后续路由决策
//!   查询（`is_approved`）。事件载荷仅有 variant_id + score,不含算子,
//!   因此**不**调用 `record_result`（那会伪造算子执行反馈）。
//! - **ParentSelected** → 同步 L5 三因子选择器的访问统计（`register_visit`）:
//!   外部选择路径的选择结果计入 visit_counts,使 UCB bonus 探索/利用平衡
//!   与真实选择演化一致（与 `select()` 内部计数同源）。
//!
//! # 设计约束
//!
//! - **§4.4 红线 3（先 subscribe 再 spawn）**: broadcast 仅投递给发布时
//!   已存在的 receiver——订阅在 spawn 之前同步完成
//! - **§4.4 红线 1（禁止持锁跨 await）**: 锁内仅同步登记（短临界区）
//! - **依赖方向合规**: faae→gsoe-evolution（L6→L5 向下允许）+ faae→event-bus

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use event_bus::EventBus;
use gsoe_evolution::ThreeFactorSelector;

/// 批准变体记录 — append-only 登记项
#[derive(Clone, Debug)]
pub struct ApprovedVariant {
    /// 变体标识(spec_name@spec_version,与 VariantApproved.variant_id 一致)
    pub variant_id: String,
    /// 变体评分(议会审议结果)
    pub score: f32,
    /// 批准时刻(Unix 秒,可观测性)
    pub approved_at: u64,
}

/// 批准变体注册表 — L6 消费 VariantApproved 的登记与查询面
#[derive(Debug, Default)]
pub struct ApprovedVariantRegistry {
    /// 批准记录(append-only,按批准顺序追加)
    approved: Vec<ApprovedVariant>,
}

impl ApprovedVariantRegistry {
    /// 登记一个批准变体(append-only,铁律3: 只追加不修改既有记录)
    pub fn record_approved(&mut self, variant_id: String, score: f32) {
        let approved_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.approved.push(ApprovedVariant {
            variant_id,
            score,
            approved_at,
        });
    }

    /// 查询变体是否已被议会批准
    pub fn is_approved(&self, variant_id: &str) -> bool {
        self.approved.iter().any(|v| v.variant_id == variant_id)
    }

    /// 批准记录只读快照(可观测性)
    pub fn snapshot(&self) -> Vec<ApprovedVariant> {
        self.approved.clone()
    }

    /// 批准总数(可观测性)
    pub fn len(&self) -> usize {
        self.approved.len()
    }

    /// 是否为空(clippy len-without-is-empty 补全)
    pub fn is_empty(&self) -> bool {
        self.approved.is_empty()
    }
}

/// 启动变体/父本事件订阅器（后台 tokio task）
///
/// 订阅 EventBus 的 VariantApproved + ParentSelected 双事件,驱动
/// [`ApprovedVariantRegistry`] 登记与三因子选择器访问统计同步。
///
/// `selector` 为可选注入:组合根未装配选择器时,ParentSelected 仅登记
/// 选择历史(见 [`ParentSelectionHistory`]);装配后同步 `register_visit`。
///
/// 返回 `JoinHandle` 供调用者管理任务生命周期（装配期调用一次）。
pub fn spawn_variant_event_subscriber(
    bus: &EventBus,
    registry: Arc<Mutex<ApprovedVariantRegistry>>,
    selector: Option<Arc<Mutex<ThreeFactorSelector>>>,
    history: Arc<Mutex<ParentSelectionHistory>>,
) -> tokio::task::JoinHandle<()> {
    // 红线 §4.4-3: subscribe 必须在 spawn 之前同步调用
    let mut rx = bus.subscribe();

    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                // Lagged(容量溢出) → 继续消费后续;Closed → 退出
                Err(err) if matches!(err, event_bus::EventBusError::SlowConsumerDropped { .. }) => {
                    tracing::debug!(error = %err, "变体事件订阅广播滞后,继续");
                    continue;
                }
                Err(_) => break,
            };
            match event {
                event_bus::NexusEvent::VariantApproved {
                    variant_id, score, ..
                } => {
                    // 议会批准 → 登记(锁内短临界区,不跨 await)
                    if let Ok(mut reg) = registry.lock() {
                        reg.record_approved(variant_id, score);
                    }
                }
                event_bus::NexusEvent::ParentSelected {
                    task_id,
                    parent_node_id,
                    quality,
                    progress,
                    novelty,
                    ..
                } => {
                    // 父本选择 → 同步选择器访问统计(外部选择计入 UCB 演化)
                    if let Some(ref sel) = selector {
                        if let Ok(mut sel) = sel.lock() {
                            sel.register_visit(&parent_node_id);
                        }
                    }
                    // 选择历史登记(进程内可观测,与卡片反馈闭环互补)
                    if let Ok(mut hist) = history.lock() {
                        hist.record(task_id, parent_node_id, quality, progress, novelty);
                    }
                }
                _ => {} // 其余事件本订阅器不关注(事件驱动过滤)
            }
        }
        tracing::warn!("变体事件订阅器退出:事件流关闭");
    })
}

/// 父本选择历史记录 — append-only 登记项
#[derive(Clone, Debug)]
pub struct ParentSelectionRecord {
    /// 所属任务 ID
    pub task_id: String,
    /// 选中父本节点 ID
    pub parent_node_id: String,
    /// 三因子归一化评分(quality/progress/novelty)
    pub quality: f32,
    /// 进度因子(三因子分量)
    pub progress: f32,
    /// 新颖性因子(三因子分量)
    pub novelty: f32,
}

/// 父本选择历史 — L6 消费 ParentSelected 的可观测性登记面
#[derive(Debug, Default)]
pub struct ParentSelectionHistory {
    /// 选择记录(append-only)
    records: Vec<ParentSelectionRecord>,
}

impl ParentSelectionHistory {
    /// 登记一次父本选择(append-only,铁律3)
    pub fn record(
        &mut self,
        task_id: String,
        parent_node_id: String,
        quality: f32,
        progress: f32,
        novelty: f32,
    ) {
        self.records.push(ParentSelectionRecord {
            task_id,
            parent_node_id,
            quality,
            progress,
            novelty,
        });
    }

    /// 历史只读快照(可观测性)
    pub fn snapshot(&self) -> Vec<ParentSelectionRecord> {
        self.records.clone()
    }

    /// 选择总数(可观测性)
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 是否为空(clippy len-without-is-empty 补全)
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

// ============================================================
// 单元测试(§16.4 消费接线验证)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{EventMetadata, NexusEvent};

    /// 注册表:登记 + 查询 + 快照(append-only)
    #[test]
    fn approved_registry_append_only() {
        let mut reg = ApprovedVariantRegistry::default();
        reg.record_approved("spec-a@1.0".to_string(), 0.8);
        reg.record_approved("spec-b@1.0".to_string(), 0.6);
        assert_eq!(reg.len(), 2);
        assert!(reg.is_approved("spec-a@1.0"));
        assert!(!reg.is_approved("spec-c@1.0"));
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].score, 0.8);
    }

    /// 选择历史:登记 + 快照(append-only)
    #[test]
    fn parent_history_append_only() {
        let mut hist = ParentSelectionHistory::default();
        hist.record("t-1".to_string(), "n-1".to_string(), 0.9, 0.5, 0.2);
        hist.record("t-2".to_string(), "n-2".to_string(), 0.7, 0.8, 0.4);
        assert_eq!(hist.len(), 2);
        let snap = hist.snapshot();
        assert_eq!(snap[1].parent_node_id, "n-2");
        assert_eq!(snap[1].novelty, 0.4);
    }

    /// 端到端:VariantApproved → 注册表;ParentSelected → 选择器访问统计 + 历史
    #[tokio::test]
    async fn subscriber_consumes_variant_and_parent_events() {
        let bus = EventBus::new();
        let registry = Arc::new(Mutex::new(ApprovedVariantRegistry::default()));
        let selector = Arc::new(Mutex::new(ThreeFactorSelector::new(1.414, 0.1, 1.0)));
        let history = Arc::new(Mutex::new(ParentSelectionHistory::default()));

        // 先 subscribe 再 spawn(红线 §4.4-3)
        let _handle = spawn_variant_event_subscriber(
            &bus,
            Arc::clone(&registry),
            Some(Arc::clone(&selector)),
            Arc::clone(&history),
        );

        // 发布 VariantApproved
        bus.publish(NexusEvent::VariantApproved {
            metadata: EventMetadata::new("parliament"),
            variant_id: "spec-x@2.0".to_string(),
            score: 0.85,
        })
        .await
        .expect("发布成功");

        // 发布 ParentSelected
        bus.publish(NexusEvent::ParentSelected {
            metadata: EventMetadata::new("gsoe-evolution"),
            task_id: "task-1".to_string(),
            parent_node_id: "node-9".to_string(),
            quality: 0.9,
            progress: 0.6,
            novelty: 0.3,
        })
        .await
        .expect("发布成功");

        // 等待后台任务消费
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 注册表应记录批准变体
        {
            let reg = registry.lock().unwrap();
            assert!(reg.is_approved("spec-x@2.0"), "VariantApproved 应登记");
            assert_eq!(reg.len(), 1);
        }
        // 选择器访问统计应同步(ParentSelected → register_visit)
        {
            let sel = selector.lock().unwrap();
            assert_eq!(sel.visit_count("node-9"), 1, "父本选择应计入访问统计");
            assert_eq!(sel.total_visits(), 1);
        }
        // 选择历史应登记
        {
            let hist = history.lock().unwrap();
            assert_eq!(hist.len(), 1);
            assert_eq!(hist.snapshot()[0].task_id, "task-1");
        }
    }
}
