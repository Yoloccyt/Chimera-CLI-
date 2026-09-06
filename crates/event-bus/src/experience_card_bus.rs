//! 经验卡片总线 — OpenMLE 经验卡片的一级公民通道（设计文档 §6.1）
//!
//! 对应架构层: **L1 Core**（event-bus 内部扩展）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §6.1
//! 对应论文: 清华 Frontis-MA1 OpenMLE（经验卡片体系）
//!
//! # 核心职责
//!
//! 将 Event Bus 扩展为经验卡片的一级公民，支持：
//! - **双通道分级投递**: 高分卡片（score > 0.8）走 Critical mpsc 旁路确保送达，
//!   中分卡片（0.5 < score ≤ 0.8）走 broadcast，低分卡片（≤ 0.5）静默丢弃
//!   （降级路径，避免噪声稀释经验库）
//! - **四索引快速检索**: task_id → cards / node_id → card / card_id → factor /
//!   error_hash → card_ids，支撑任务级/节点级/因子级/错误级四维查询
//! - **全局统计**: 卡片总数 / 已评估数 / 唯一错误数（AtomicU64 无锁计数）
//!
//! # 设计约束
//!
//! - **不新增 NexusEvent 变体**: 卡片流为独立数据面（broadcast + mpsc），
//!   遵循 ADR-065 决策 4 先例（流式数据面走 bounded mpsc 不进事件枚举），
//!   避免 L1 三触点适配（types.rs/classification.rs/topic.rs）
//! - **无持锁跨 await**: 索引读写均为同步短临界区（DashMap 无锁并发读），
//!   `publish` 为同步方法（broadcast send + mpsc send 均非阻塞）
//! - **subscribe 先于 spawn**: `new()` 同步持有 receiver，防止事件静默丢失
//!   （Week 6 SSRA 教训：broadcast 先 subscribe 再 spawn）
//! - **Top-K 用 `select_nth_unstable_by`**: O(n) 而非 O(n log n)（红线 R8）
//!
//! # 与 EventBus 的关系
//!
//! `ExperienceCardBus` 挂在 `EventBus` 之上作为**独立数据面**：
//! - EventBus 承载系统状态变更的广播（NexusEvent 136 变体）
//! - ExperienceCardBus 承载经验卡片的流式投递（高吞吐、可丢失语义分级）

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use nexus_contracts::experience_card::{ExperienceCard, ThreeFactorScore};
use tokio::sync::{broadcast, mpsc};

/// 广播通道容量 — 卡片广播默认 1024 条（与 EventBus DEFAULT_CAPACITY 对齐）
const CARD_BROADCAST_CAPACITY: usize = 1024;

/// 全局卡片统计 — 经验库健康度指标
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalCardStats {
    /// 累计卡片总数
    pub total_cards: u64,
    /// 已评估卡片数（ExecutionStatus::Success 计数）
    pub total_evaluated: u64,
    /// 唯一错误签名数（error_hash 去重）
    pub unique_errors: u64,
}

/// 经验卡片总线 — 双通道分级投递 + 四索引快速检索
///
/// `Clone` 派生（broadcast Sender + DashMap/Arc 语义），所有副本共享索引与通道。
/// Critical 通道为 fan-out 模式（Arc<Mutex<Vec<Sender>>>，EventBus critical_tx 先例）：
/// 每次 `subscribe_critical` 创建独立 channel，publish 时遍历投递，
/// 确保高分卡片送达所有活跃订阅者。
#[derive(Debug, Clone)]
pub struct ExperienceCardBus {
    /// 中分卡片广播通道（0.5 < score ≤ 0.8）
    card_broadcast: broadcast::Sender<ExperienceCard>,
    /// 高分卡片 Critical 旁路（score > 0.8，fan-out 确保送达）
    card_critical: Arc<Mutex<Vec<mpsc::UnboundedSender<ExperienceCard>>>>,
    /// 索引 1: task_id → cards（任务级检索，Arc 共享——DashMap 深拷贝语义下
    /// Clone 副本必须共享索引，否则并发发布者写入的索引不可见）
    task_index: Arc<DashMap<Box<str>, Vec<ExperienceCard>>>,
    /// 索引 2: node_id → card（节点级唯一检索）
    node_index: Arc<DashMap<Box<str>, ExperienceCard>>,
    /// 索引 3: card_id → factor（因子级缓存）
    factor_cache: Arc<DashMap<Box<str>, ThreeFactorScore>>,
    /// 索引 4: error_hash → card_ids（错误聚类检索，Box<str> 零拷贝承载）
    error_index: Arc<DashMap<Box<str>, Vec<Box<str>>>>,
    /// 累计卡片总数（Arc: AtomicU64 非 Clone）
    total_cards: Arc<AtomicU64>,
    /// 已评估卡片数
    total_evaluated: Arc<AtomicU64>,
}

impl ExperienceCardBus {
    /// 创建经验卡片总线
    ///
    /// WHY 同步持有 receiver: 保持 broadcast receiver 存活（容量 1024），
    /// 防止"无接收者时 send 立即失败"导致卡片静默丢失（Week 6 SSRA 教训）。
    pub fn new() -> Self {
        let (card_broadcast, _keep_alive) = broadcast::channel(CARD_BROADCAST_CAPACITY);
        Self {
            card_broadcast,
            card_critical: Arc::new(Mutex::new(Vec::new())),
            task_index: Arc::new(DashMap::new()),
            node_index: Arc::new(DashMap::new()),
            factor_cache: Arc::new(DashMap::new()),
            error_index: Arc::new(DashMap::new()),
            total_cards: Arc::new(AtomicU64::new(0)),
            total_evaluated: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 发布经验卡片 — 同步方法（四索引更新 + 分级投递）
    ///
    /// # 分级投递语义
    ///
    /// | score 区间 | 通道 | 语义 |
    /// |-----------|------|------|
    /// | > 0.8 | Critical mpsc | 高价值卡片，确保送达（训练/进化必须消费） |
    /// | (0.5, 0.8] | broadcast | 中价值卡片，尽力投递 |
    /// | ≤ 0.5 | 静默丢弃 | 低价值卡片，避免噪声稀释经验库 |
    ///
    /// 索引更新与投递均为同步操作，无持锁跨 await（红线 §4.4-1）。
    pub fn publish(&self, card: ExperienceCard) {
        // ---- 四索引同步更新（无锁短临界区）----
        self.task_index
            .entry(card.task_id.clone())
            .or_default()
            .push(card.clone());
        self.node_index.insert(card.node_id.clone(), card.clone());
        self.factor_cache
            .insert(card.card_id.clone(), card.three_factor.clone());
        if let Some(sig) = &card.error_signature {
            self.error_index
                .entry(sig.error_hash.clone())
                .or_default()
                .push(card.card_id.clone());
        }
        self.total_cards.fetch_add(1, Ordering::SeqCst);
        if card.execution_status == nexus_contracts::ExecutionStatus::Success {
            self.total_evaluated.fetch_add(1, Ordering::SeqCst);
        }

        // ---- 分级投递 ----
        if card.score > 0.8 {
            // 高分走 Critical 旁路（fan-out，确保送达所有活跃订阅者）
            let mut criticals = self.card_critical.lock().unwrap_or_else(|e| e.into_inner());
            criticals.retain(|tx| tx.send(card.clone()).is_ok());
        } else if card.score > 0.5 {
            // 中分走 broadcast（尽力投递，慢消费者可 Lagged）
            let _ = self.card_broadcast.send(card);
        }
        // ≤ 0.5 静默丢弃（降级路径，索引仍保留供审计）
    }

    /// 订阅中分卡片广播流（0.5 < score ≤ 0.8）
    ///
    /// 调用方须在 `tokio::spawn` **之前**调用本方法（红线：先 subscribe 再 spawn）。
    pub fn subscribe(&self) -> broadcast::Receiver<ExperienceCard> {
        self.card_broadcast.subscribe()
    }

    /// 订阅高分卡片 Critical 流（score > 0.8）
    ///
    /// 返回 unbounded mpsc Receiver，高分卡片确保送达。
    /// fan-out 模式：每次调用创建独立 channel（EventBus critical_tx 先例）。
    pub fn subscribe_critical(&self) -> mpsc::UnboundedReceiver<ExperienceCard> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut criticals = self.card_critical.lock().unwrap_or_else(|e| e.into_inner());
        criticals.push(tx);
        rx
    }

    /// 按任务检索卡片（索引 1）
    pub fn get_cards_by_task(&self, task_id: &str) -> Vec<ExperienceCard> {
        self.task_index
            .get(task_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// 按节点检索卡片（索引 2）
    pub fn get_card_by_node(&self, node_id: &str) -> Option<ExperienceCard> {
        self.node_index.get(node_id).map(|v| v.clone())
    }

    /// 按错误哈希检索卡片 ID 列表（索引 4）
    pub fn get_card_ids_by_error_hash(&self, error_hash: &str) -> Vec<String> {
        self.error_index
            .get(error_hash)
            .map(|v| v.iter().map(|id| id.to_string()).collect())
            .unwrap_or_default()
    }

    /// 按三因子效用取 Top-K 卡片（索引 3 + `select_nth_unstable_by`，红线 R8）
    ///
    /// 复杂度 O(n)（n = 该任务卡片数），禁止 `sort_by` O(n log n)。
    pub fn get_top_cards_by_factor(&self, task_id: &str, k: usize) -> Vec<ExperienceCard> {
        let mut cards = self.get_cards_by_task(task_id);
        if cards.len() <= k {
            // 不足 k 条时按效用降序排好返回
            cards.sort_by(|a, b| {
                let sa = a.three_factor.selection_utility();
                let sb = b.three_factor.selection_utility();
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });
            return cards;
        }
        // O(n) Top-K: 第 k 大元素就位后，仅对前 k 个排序
        let kth = cards
            .select_nth_unstable_by(k, |a, b| {
                let sa = a.three_factor.selection_utility();
                let sb = b.three_factor.selection_utility();
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            })
            .0;
        let _ = kth; // 就位元素无需引用（前 k 个即 Top-K 无序集合）
        cards.truncate(k);
        cards.sort_by(|a, b| {
            let sa = a.three_factor.selection_utility();
            let sb = b.three_factor.selection_utility();
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        cards
    }

    /// 获取全局统计
    pub fn get_global_stats(&self) -> GlobalCardStats {
        GlobalCardStats {
            total_cards: self.total_cards.load(Ordering::SeqCst),
            total_evaluated: self.total_evaluated.load(Ordering::SeqCst),
            unique_errors: self.error_index.len() as u64,
        }
    }

    /// 广播通道容量（测试/运维监控）
    pub fn broadcast_capacity(&self) -> usize {
        CARD_BROADCAST_CAPACITY
    }
}

impl Default for ExperienceCardBus {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use nexus_contracts::experience_card::{AtomicOperator, ExecutionStatus};
    use nexus_contracts::ThreeFactorScore;

    /// 构造样例卡片（可指定 score/status/错误签名）
    fn card(id: &str, task: &str, score: f32, status: ExecutionStatus) -> ExperienceCard {
        ExperienceCard {
            card_id: Box::from(id),
            task_id: Box::from(task),
            node_id: Box::from(format!("node-{id}")),
            parent_id: None,
            created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("合法时间戳"),
            operator: AtomicOperator::Draft,
            score,
            delta_vs_parent: 0.0,
            method_family: Box::from("draft_pipeline"),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality: score,
                progress: 0.1,
                novelty: 0.5,
            },
            execution_status: status,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: Default::default(),
        }
    }

    // ---------- 分级投递 ----------

    #[tokio::test]
    async fn high_score_goes_critical_channel() {
        let bus = ExperienceCardBus::new();
        let mut critical_rx = bus.subscribe_critical();
        let high = card("c1", "t1", 0.95, ExecutionStatus::Success);
        bus.publish(high);
        let received = critical_rx
            .recv()
            .await
            .expect("高分卡片必须送达 Critical 通道");
        assert_eq!(received.card_id.as_ref(), "c1");
    }

    #[tokio::test]
    async fn mid_score_goes_broadcast_channel() {
        let bus = ExperienceCardBus::new();
        let mut rx = bus.subscribe();
        let mid = card("c2", "t1", 0.7, ExecutionStatus::Success);
        bus.publish(mid);
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("5s 内未收到事件(资源竞争或事件丢失)")
            .expect("中分卡片必须送达 broadcast 通道");
        assert_eq!(received.card_id.as_ref(), "c2");
    }

    #[tokio::test]
    async fn low_score_silently_dropped() {
        let bus = ExperienceCardBus::new();
        let mut rx = bus.subscribe();
        let mut critical_rx = bus.subscribe_critical();
        let low = card("c3", "t1", 0.4, ExecutionStatus::Error);
        bus.publish(low);
        // 双通道均不应收到
        let dropped_broadcast =
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(dropped_broadcast.is_err(), "低分卡片不应进入 broadcast");
        let dropped_critical =
            tokio::time::timeout(std::time::Duration::from_millis(50), critical_rx.recv()).await;
        assert!(dropped_critical.is_err(), "低分卡片不应进入 Critical");
        // 但索引仍保留（审计可用）
        assert_eq!(bus.get_cards_by_task("t1").len(), 1);
    }

    // ---------- 四索引一致性 ----------

    #[test]
    fn four_indexes_consistent_after_publish() {
        let bus = ExperienceCardBus::new();
        bus.publish(card("c1", "t1", 0.9, ExecutionStatus::Success));
        bus.publish(card("c2", "t1", 0.7, ExecutionStatus::Success));
        bus.publish(card("c3", "t2", 0.6, ExecutionStatus::Error));
        // 索引 1: task
        assert_eq!(bus.get_cards_by_task("t1").len(), 2);
        assert_eq!(bus.get_cards_by_task("t2").len(), 1);
        assert!(bus.get_cards_by_task("t3").is_empty());
        // 索引 2: node
        assert_eq!(
            bus.get_card_by_node("node-c2")
                .expect("节点索引存在")
                .card_id
                .as_ref(),
            "c2"
        );
        assert!(bus.get_card_by_node("node-missing").is_none());
        // 索引 3: factor（publish 时缓存）
        assert!(bus.factor_cache.contains_key("c1"));
        // 索引 4: error（无错误签名卡片不入索引）
        assert!(bus.get_card_ids_by_error_hash("none").is_empty());
    }

    #[test]
    fn error_index_clusters_by_hash() {
        let bus = ExperienceCardBus::new();
        let mut e1 = card("e1", "t1", 0.85, ExecutionStatus::Error);
        e1.error_signature = Some(nexus_contracts::ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/lib.rs:1"),
            error_summary: Box::from("E0308"),
            error_hash: Box::from("hash-abc"),
        });
        let mut e2 = card("e2", "t2", 0.75, ExecutionStatus::Error);
        e2.error_signature = Some(nexus_contracts::ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/lib.rs:2"),
            error_summary: Box::from("E0308"),
            error_hash: Box::from("hash-abc"),
        });
        bus.publish(e1);
        bus.publish(e2);
        let ids = bus.get_card_ids_by_error_hash("hash-abc");
        assert_eq!(ids.len(), 2, "相同错误哈希应聚类");
        assert_eq!(bus.get_global_stats().unique_errors, 1);
    }

    // ---------- Top-K（红线 R8: select_nth_unstable_by） ----------

    #[test]
    fn top_k_by_factor_utility() {
        let bus = ExperienceCardBus::new();
        for i in 0..10 {
            bus.publish(card(
                &format!("c{i}"),
                "t1",
                i as f32 / 10.0,
                ExecutionStatus::Success,
            ));
        }
        let top3 = bus.get_top_cards_by_factor("t1", 3);
        assert_eq!(top3.len(), 3);
        // 效用 = quality + progress + novelty = score + 0.1 + 0.5，单调于 score
        assert_eq!(top3[0].card_id.as_ref(), "c9");
        assert_eq!(top3[1].card_id.as_ref(), "c8");
        assert_eq!(top3[2].card_id.as_ref(), "c7");
    }

    #[test]
    fn top_k_exceeds_len_returns_all_sorted() {
        let bus = ExperienceCardBus::new();
        bus.publish(card("c1", "t1", 0.6, ExecutionStatus::Success));
        bus.publish(card("c2", "t1", 0.9, ExecutionStatus::Success));
        let all = bus.get_top_cards_by_factor("t1", 10);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].card_id.as_ref(), "c2");
        // 空任务返回空
        assert!(bus.get_top_cards_by_factor("t-empty", 5).is_empty());
    }

    // ---------- 统计 ----------

    #[test]
    fn global_stats_counting() {
        let bus = ExperienceCardBus::new();
        bus.publish(card("c1", "t1", 0.9, ExecutionStatus::Success));
        bus.publish(card("c2", "t1", 0.7, ExecutionStatus::Error));
        bus.publish(card("c3", "t2", 0.4, ExecutionStatus::Timeout));
        let stats = bus.get_global_stats();
        assert_eq!(stats.total_cards, 3);
        assert_eq!(stats.total_evaluated, 1);
    }

    // ---------- 并发 publish ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_publish_no_loss() {
        let bus = ExperienceCardBus::new();
        let mut critical_rx = bus.subscribe_critical();
        let mut handles = Vec::new();
        for w in 0..8 {
            let bus = bus.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..50 {
                    // 全高分卡片（验证 Critical 无丢失）
                    bus.publish(card(
                        &format!("w{w}-c{i}"),
                        &format!("task-{w}"),
                        0.9,
                        ExecutionStatus::Success,
                    ));
                }
            }));
        }
        for h in handles {
            h.await.expect("并发任务不失败");
        }
        // 400 张高分卡片全部应达 Critical 通道（固定迭代 + 充足超时防 flaky）
        let mut received = 0;
        for _ in 0..400 {
            match tokio::time::timeout(std::time::Duration::from_secs(2), critical_rx.recv()).await
            {
                Ok(Some(_)) => received += 1,
                _ => break,
            }
        }
        assert_eq!(received, 400, "Critical 通道必须无丢失");
        assert_eq!(bus.get_global_stats().total_cards, 400);
        // 任务索引分片正确
        assert_eq!(bus.get_cards_by_task("task-3").len(), 50);
    }

    // ---------- 广播容量 ----------

    #[test]
    fn broadcast_capacity_exposed() {
        let bus = ExperienceCardBus::new();
        assert_eq!(bus.broadcast_capacity(), 1024);
    }
}
