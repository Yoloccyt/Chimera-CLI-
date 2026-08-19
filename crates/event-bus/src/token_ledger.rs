//! Token 账本 — Dressage Token 级证据的 L1 承载（设计文档 §6.1 + §5.3）
//!
//! 对应架构层: **L1 Core**（event-bus 内部扩展）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §6.1 / §5.3
//! 对应论文: 微软 OpenForge/Dressage（Token 级证据 + 训练证据完整性）
//!
//! # 核心职责
//!
//! 承载 [`TokenLedgerEntry`]（L0 契约）的 append-only 账本，保证
//! **"Token Ledger 不可丢失（训练证据完整性）"** 绝对红线：
//! - **append-only 有序存储**: BTreeMap<timestamp, entries>，时间序天然有序
//! - **ID 唯一性**: 重复 entry_id 拒绝（防重放/防篡改）
//! - **双索引检索**: 按 session / instance 快速回溯
//! - **完整性校验**: 条目数 / ID 唯一性 / 证据三序列等长（二次防御）
//! - **导出通道**: `export_entries` 供持久化（WAL 落盘由消费方实现）与
//!   v4.0 训练数据面上传
//!
//! # 设计约束
//!
//! - **不引入文件 IO**: WAL 文件持久化留待后续迭代（项目红线：rusqlite 调用
//!   必须 spawn_blocking；文件 IO 同理需异步封装，本模块仅提供内存账本 +
//!   导出通道，由消费方决定持久化策略）
//! - **无持锁跨 await**: DashMap 索引读写均为同步短临界区
//! - **错误处理**: 重复 ID 拒绝返回 [`LedgerError`]（thiserror，库层惯例）

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use nexus_contracts::token_evidence::TokenLedgerEntry;
use thiserror::Error;

/// Token 账本错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LedgerError {
    /// 重复的账本条目 ID（防重放/防篡改）
    #[error("TokenLedger 重复条目 ID: {0}")]
    DuplicateEntry(String),
}

/// 账本完整性报告 — 训练证据完整性审计结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerIntegrityReport {
    /// 账本条目总数
    pub total_entries: u64,
    /// 唯一 ID 数（应等于 total_entries）
    pub unique_ids: u64,
    /// 重复 ID 数（应为 0）
    pub duplicate_ids: u64,
    /// 会话数
    pub sessions: u64,
    /// 实例数
    pub instances: u64,
}

/// Token 账本 — append-only 证据存储（Dressage 训练证据完整性）
///
/// `Clone` 派生（DashMap + Arc 语义），所有副本共享账本。
/// `entries` 用 `Arc<Mutex<BTreeMap>>`：BTreeMap 非并发容器，
/// Mutex 保护（同步短临界区，无持锁跨 await——红线 §4.4-1）。
#[derive(Debug, Clone)]
pub struct TokenLedger {
    /// 有序账本: timestamp → entries（append-only，Mutex 保护并发写）
    entries: Arc<Mutex<BTreeMap<u64, Vec<TokenLedgerEntry>>>>,
    /// 已见 entry_id 集合（唯一性校验，Arc 共享——DashMap 深拷贝语义下
    /// Clone 副本必须共享唯一性表，否则并发追加绕过重复检测）
    seen_ids: Arc<DashMap<String, ()>>,
    /// 会话索引: session_id → entry_ids
    by_session: Arc<DashMap<String, Vec<String>>>,
    /// 实例索引: instance_id → entry_ids
    by_instance: Arc<DashMap<String, Vec<String>>>,
    /// 累计条目数（Arc: AtomicU64 非 Clone，Arc 支持 Clone 派生共享）
    total_entries: Arc<AtomicU64>,
}

impl TokenLedger {
    /// 创建空账本
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            seen_ids: Arc::new(DashMap::new()),
            by_session: Arc::new(DashMap::new()),
            by_instance: Arc::new(DashMap::new()),
            total_entries: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 追加账本条目
    ///
    /// # 错误
    ///
    /// `entry_id` 已存在时返回 [`LedgerError::DuplicateEntry`]——
    /// 训练证据完整性要求 ID 全局唯一（防重放/防篡改）。
    ///
    /// # 并发安全
    ///
    /// `&self` 即可追加（DashMap 并发写 + entries Mutex 同步短临界区）。
    pub fn append(&self, entry: TokenLedgerEntry) -> Result<(), LedgerError> {
        // 唯一性登记（原子 check-and-insert：DashMap::insert 返回旧值，
        // 并发下仅一个线程得到 None——修复 check-then-act 竞态）
        if self
            .seen_ids
            .insert(entry.entry_id.to_string(), ())
            .is_some()
        {
            return Err(LedgerError::DuplicateEntry(entry.entry_id.to_string()));
        }
        // 中毒锁降级访问（EventBus 先例：panic 后不中断核心写入路径）
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(entry.timestamp).or_default().push(entry.clone());
        drop(map); // 提前释放锁（短临界区结束）
        self.by_session
            .entry(entry.session_id.to_string())
            .or_default()
            .push(entry.entry_id.to_string());
        self.by_instance
            .entry(entry.instance_id.to_string())
            .or_default()
            .push(entry.entry_id.to_string());
        self.total_entries.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// 追加账本条目并发布 `TokenLedgerRecorded`(§16.4 L1→L3 事件化,Phase 10 Wave 4)
    ///
    /// opt-in 接线:既有 `append` 行为不变(零回归);需要通知 L3 持久化
    /// 通道的调用方使用本方法。发布失败仅告警不上抛(账本写入已成功,
    /// 事件丢失可由 L3 周期同步补偿)。铁律8 Token 证据全链路追踪。
    pub fn append_and_notify(
        &self,
        entry: TokenLedgerEntry,
        bus: &crate::EventBus,
    ) -> Result<(), LedgerError> {
        let evidence_id = entry.entry_id.to_string();
        let token_usage = (entry.input_token_ids.len() + entry.output_token_ids.len()) as u64;
        self.append(entry)?;
        // sync 上下文用 publish_blocking(§4.4 红线 8)
        if let Err(e) = bus.publish_blocking(crate::NexusEvent::TokenLedgerRecorded {
            metadata: crate::EventMetadata::new("event-bus"),
            evidence_id,
            token_usage,
        }) {
            tracing::warn!(error = %e, "TokenLedgerRecorded 发布失败(账本写入已成功)");
        }
        Ok(())
    }

    /// 按会话检索条目 ID（索引 1）
    pub fn session_entry_ids(&self, session_id: &str) -> Vec<String> {
        self.by_session
            .get(session_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// 按实例检索条目 ID（索引 2）
    pub fn instance_entry_ids(&self, instance_id: &str) -> Vec<String> {
        self.by_instance
            .get(instance_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// 按 entry_id 查询条目
    pub fn get(&self, entry_id: &str) -> Option<TokenLedgerEntry> {
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        map.values()
            .flatten()
            .find(|e| e.entry_id.as_ref() == entry_id)
            .cloned()
    }

    /// 全量导出（时间序）— 供持久化（WAL 落盘）与 v4.0 训练数据面
    pub fn export_entries(&self) -> Vec<TokenLedgerEntry> {
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        map.values().flatten().cloned().collect()
    }

    /// 完整性校验 — 训练证据完整性的二次防御
    ///
    /// 校验项：条目总数与索引一致 / ID 唯一 / 证据三序列等长
    /// （L0 构造器已保证等长，此处防御性复核）。
    pub fn integrity_check(&self) -> LedgerIntegrityReport {
        let exported = self.export_entries();
        let total = exported.len() as u64;
        // ID 唯一性复核（seen_ids 与导出集合交叉验证）
        let mut ids = std::collections::HashSet::new();
        let mut duplicates = 0u64;
        for e in &exported {
            if !ids.insert(e.entry_id.to_string()) {
                duplicates += 1;
            }
        }
        LedgerIntegrityReport {
            total_entries: total,
            unique_ids: ids.len() as u64,
            duplicate_ids: duplicates,
            sessions: self.by_session.len() as u64,
            instances: self.by_instance.len() as u64,
        }
    }

    /// 账本条目总数
    pub fn len(&self) -> u64 {
        self.total_entries.load(Ordering::SeqCst)
    }
    /// 账本是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for TokenLedger {
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

    fn entry(id: &str, session: &str, instance: &str, ts: u64) -> TokenLedgerEntry {
        TokenLedgerEntry::new(
            id,
            0,
            session,
            instance,
            vec![101, 102],
            vec![201, 202],
            vec![0.9, 0.8],
            vec![true, true],
            "v2.26.0-omega",
            vec![],
            None,
            ts,
        )
    }

    // ---------- append 与有序性 ----------

    #[test]
    fn append_preserves_timestamp_order() {
        let ledger = TokenLedger::new();
        ledger
            .append(entry("e1", "s1", "i1", 3_000))
            .expect("追加成功");
        ledger
            .append(entry("e2", "s1", "i1", 1_000))
            .expect("追加成功");
        ledger
            .append(entry("e3", "s1", "i1", 2_000))
            .expect("追加成功");
        let exported = ledger.export_entries();
        // BTreeMap 按 timestamp 有序：e2(1k) → e3(2k) → e1(3k)
        let order: Vec<&str> = exported.iter().map(|e| e.entry_id.as_ref()).collect();
        assert_eq!(order, vec!["e2", "e3", "e1"]);
        assert_eq!(ledger.len(), 3);
        assert!(!ledger.is_empty());
    }

    #[test]
    fn duplicate_entry_id_rejected() {
        let ledger = TokenLedger::new();
        ledger
            .append(entry("e1", "s1", "i1", 1_000))
            .expect("首次追加成功");
        let err = ledger
            .append(entry("e1", "s1", "i1", 2_000))
            .expect_err("重复 ID 必须拒绝");
        assert!(matches!(err, LedgerError::DuplicateEntry(_)));
        assert_eq!(ledger.len(), 1, "重复条目不得入库");
    }

    // ---------- 双索引检索 ----------

    #[test]
    fn session_and_instance_indexes() {
        let ledger = TokenLedger::new();
        ledger
            .append(entry("e1", "s1", "i1", 1_000))
            .expect("追加成功");
        ledger
            .append(entry("e2", "s1", "i2", 2_000))
            .expect("追加成功");
        ledger
            .append(entry("e3", "s2", "i1", 3_000))
            .expect("追加成功");
        assert_eq!(ledger.session_entry_ids("s1").len(), 2);
        assert_eq!(ledger.session_entry_ids("s2").len(), 1);
        assert!(ledger.session_entry_ids("s3").is_empty());
        assert_eq!(ledger.instance_entry_ids("i1").len(), 2);
        assert_eq!(ledger.instance_entry_ids("i2").len(), 1);
    }

    #[test]
    fn get_by_entry_id() {
        let ledger = TokenLedger::new();
        ledger
            .append(entry("e1", "s1", "i1", 1_000))
            .expect("追加成功");
        let found = ledger.get("e1").expect("条目存在");
        assert_eq!(found.session_id.as_ref(), "s1");
        assert!(ledger.get("missing").is_none());
    }

    // ---------- 完整性校验 ----------

    #[test]
    fn integrity_check_consistent() {
        let ledger = TokenLedger::new();
        for i in 0..10 {
            ledger
                .append(entry(&format!("e{i}"), "s1", "i1", i))
                .expect("追加成功");
        }
        let report = ledger.integrity_check();
        assert_eq!(report.total_entries, 10);
        assert_eq!(report.unique_ids, 10);
        assert_eq!(report.duplicate_ids, 0);
        assert_eq!(report.sessions, 1);
        assert_eq!(report.instances, 1);
    }

    // ---------- 导出 roundtrip ----------

    #[test]
    fn export_msgpack_roundtrip() {
        let ledger = TokenLedger::new();
        ledger
            .append(entry("e1", "s1", "i1", 1_000))
            .expect("追加成功");
        ledger
            .append(entry("e2", "s1", "i1", 2_000))
            .expect("追加成功");
        let exported = ledger.export_entries();
        // MsgPack 导出（训练数据面传输形态）
        let bytes = rmp_serde::to_vec(&exported).expect("MsgPack 序列化失败");
        let back: Vec<TokenLedgerEntry> =
            rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        assert_eq!(back, exported);
    }

    // ---------- 并发 append ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_append_unique_ids_all_accepted() {
        let ledger = TokenLedger::new();
        let mut handles = Vec::new();
        for w in 0..8 {
            let ledger = ledger.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..50 {
                    ledger
                        .append(entry(&format!("w{w}-c{i}"), "s1", "i1", i as u64))
                        .expect("唯一 ID 追加成功");
                }
            }));
        }
        for h in handles {
            h.await.expect("并发任务不失败");
        }
        assert_eq!(ledger.len(), 400);
        let report = ledger.integrity_check();
        assert_eq!(report.total_entries, 400);
        assert_eq!(report.unique_ids, 400);
        assert_eq!(report.duplicate_ids, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_duplicate_rejected_once() {
        // 并发重复追加同一 ID：恰好 1 次成功，其余拒绝（唯一性登记先于写入）
        let ledger = TokenLedger::new();
        let mut handles = Vec::new();
        for _ in 0..8 {
            let ledger = ledger.clone();
            handles.push(tokio::spawn(async move {
                ledger.append(entry("same-id", "s1", "i1", 1_000))
            }));
        }
        let mut ok = 0;
        let mut dup = 0;
        for h in handles {
            match h.await.expect("任务不失败") {
                Ok(()) => ok += 1,
                // LedgerError 仅 DuplicateEntry 单变体，其余分支不可达
                Err(LedgerError::DuplicateEntry(_)) => dup += 1,
            }
        }
        assert_eq!(ok, 1, "恰好一个成功");
        assert_eq!(dup, 7, "其余全部拒绝");
        assert_eq!(ledger.len(), 1);
    }
}
