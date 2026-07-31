//! 批次账本 — 独立批次胜负序列聚合与晋级检查点门控
//!
//! 对应架构层: L9 Quest(chimera-mas shadow 子模块)
//! 对应 ADR: ADR-053-rev3 决策 3A′-P(判定规则/样本量/快照单元)+ 决策 6(晋级门)
//!
//! # 核心职责:只做数据聚合,不做统计裁决
//!
//! - 记录每批 [`BatchRecord`](含只读谱系快照哈希——rev3 快照单元语义:
//!   每批取不可变谱系快照,结果**永不回流**为进化反馈)
//! - 拒绝重复批次 ID(防同批重复计入抬高样本量)
//! - **检查点门控(防 optional stopping)**:胜负序列只在 n=14 与 n=25
//!   两个预注册检查点对外暴露([`outcomes_at_checkpoint`]),中途窥视
//!   返回 `None`——"禁止中途窥视后随意加批"(rev3,防 α 膨胀)
//!
//! 统计裁决(Wilson/哨兵/bootstrap)由 orchestrator 调用 stats 模块完成,
//! 本模块不持有配置、不做数值判定(单一职责)。

use crate::error::{MasError, Result};

// ============================================================
// 预注册检查点常量(rev3 决策 3A′-P,禁止运行时调整)
// ============================================================

/// 基础晋级检查点:≥14 独立批次
pub const BASE_BATCHES: usize = 14;

/// 预注册扩展检查点:25 批(仅当 14 批 Wilson 下界落扩展带时激活)
pub const EXTENDED_BATCHES: usize = 25;

/// 扩展带 [0.45, 0.5]:14 批下界落入此带 → 扩展至 25 批做唯一终判
/// (rev3 预注册;R3-E06-2 登记"扩展带定义冻结"为实跑标定项)
pub const EXTENSION_BAND: (f64, f64) = (0.45, 0.5);

// ============================================================
// 批次记录
// ============================================================

/// 单批记录 — 胜负 + 审计溯源字段
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchRecord {
    /// 批次唯一 ID(重复 ID 拒绝记录)
    pub batch_id: String,
    /// 只读谱系快照哈希(rev3 快照单元:证明本批基于不可变谱系评估,
    /// 结果不回流进化反馈)
    pub lineage_snapshot_hash: String,
    /// 本批是否计胜(证据门裁决结果)
    pub win: bool,
    /// 非胜原因(计胜时为空;审计追溯)
    pub reasons: Vec<String>,
}

/// 检查点标识 — 预注册的两个终判时点
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checkpoint {
    /// n=14 基础检查点
    Base,
    /// n=25 扩展检查点(仅扩展激活后有效)
    Extended,
}

// ============================================================
// 批次账本
// ============================================================

/// 批次账本 — 胜负序列的唯一持有者(检查点外不可窥视)
#[derive(Debug, Default)]
pub struct BatchLedger {
    records: Vec<BatchRecord>,
    /// 扩展是否已激活(14 批终判落扩展带后由 orchestrator 标记,
    /// 激活后账本才接受第 15-25 批)
    extension_active: bool,
}

impl BatchLedger {
    /// 创建空账本
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一批结果
    ///
    /// # 错误(fail-closed)
    /// - 重复 `batch_id` → [`MasError::ShadowGateRejected`](防重复计入)
    /// - 已达 14 批且扩展未激活 → 拒绝(必须先在检查点终判,防悄悄加批)
    /// - 已达 25 批 → 拒绝(25 批是唯一终判,无第三检查点)
    pub fn record(&mut self, record: BatchRecord) -> Result<()> {
        if self.records.iter().any(|r| r.batch_id == record.batch_id) {
            return Err(MasError::ShadowGateRejected {
                reason: format!("批次 ID {} 重复,拒绝记录(防重复计入)", record.batch_id),
            });
        }
        let n = self.records.len();
        if n >= EXTENDED_BATCHES {
            return Err(MasError::ShadowGateRejected {
                reason: format!("已达扩展检查点 {EXTENDED_BATCHES} 批,不接受更多批次(唯一终判)"),
            });
        }
        if n >= BASE_BATCHES && !self.extension_active {
            return Err(MasError::ShadowGateRejected {
                reason: format!(
                    "已达基础检查点 {BASE_BATCHES} 批且扩展未激活,须先执行检查点终判(防 optional stopping)"
                ),
            });
        }
        self.records.push(record);
        Ok(())
    }

    /// 激活预注册扩展(14→25 批)
    ///
    /// 仅允许在恰好 14 批时激活——由 orchestrator 在 14 批终判确认
    /// Wilson 下界落扩展带后调用。
    ///
    /// # 错误
    /// 批数 ≠ 14 时拒绝(扩展只能在基础检查点触发)。
    pub fn activate_extension(&mut self) -> Result<()> {
        if self.records.len() != BASE_BATCHES {
            return Err(MasError::ShadowGateRejected {
                reason: format!(
                    "扩展只能在恰好 {BASE_BATCHES} 批时激活,当前 {} 批",
                    self.records.len()
                ),
            });
        }
        self.extension_active = true;
        Ok(())
    }

    /// 当前批数
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 账本是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 累计胜数
    #[must_use]
    pub fn wins(&self) -> usize {
        self.records.iter().filter(|r| r.win).count()
    }

    /// 扩展是否已激活
    #[must_use]
    pub fn extension_active(&self) -> bool {
        self.extension_active
    }

    /// 当前所处检查点(非检查点时 `None`)
    #[must_use]
    pub fn current_checkpoint(&self) -> Option<Checkpoint> {
        match self.records.len() {
            n if n == BASE_BATCHES && !self.extension_active => Some(Checkpoint::Base),
            n if n == EXTENDED_BATCHES => Some(Checkpoint::Extended),
            _ => None,
        }
    }

    /// 在预注册检查点导出胜负序列(唯一的序列窥视口)
    ///
    /// WHY 仅检查点可读:胜负序列若可随时读取,调用方就能"看着下界
    /// 决定要不要再加批"(optional stopping),使 α 膨胀。结构上收口到
    /// n=14(扩展未激活)与 n=25 两个预注册时点,统计前提才成立。
    #[must_use]
    pub fn outcomes_at_checkpoint(&self) -> Option<(Checkpoint, Vec<bool>)> {
        self.current_checkpoint()
            .map(|cp| (cp, self.records.iter().map(|r| r.win).collect()))
    }

    /// 审计导出:全部批次记录的只读视图(不含裁决用途,仅审计)
    ///
    /// WHY 与 [`outcomes_at_checkpoint`] 分离:审计追溯需要随时读原因
    /// 与快照哈希,但**不暴露聚合胜率**——单条记录不构成 optional stopping
    /// 的判定信息(调用方自行聚合即绕过收口,属故意违规,由代码评审拦截)。
    #[must_use]
    pub fn audit_records(&self) -> &[BatchRecord] {
        &self.records
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, win: bool) -> BatchRecord {
        BatchRecord {
            batch_id: id.into(),
            lineage_snapshot_hash: format!("hash-{id}"),
            win,
            reasons: Vec::new(),
        }
    }

    /// 重复批次 ID 拒绝
    #[test]
    fn test_duplicate_batch_id_rejected() {
        let mut ledger = BatchLedger::new();
        ledger.record(record("b1", true)).expect("首次记录应成功");
        assert!(ledger.record(record("b1", false)).is_err());
        assert_eq!(ledger.len(), 1);
    }

    /// 检查点前不可窥视胜负序列(防 optional stopping)
    #[test]
    fn test_no_peeking_before_checkpoint() {
        let mut ledger = BatchLedger::new();
        for i in 0..13 {
            ledger
                .record(record(&format!("b{i}"), true))
                .expect("记录应成功");
        }
        assert!(ledger.outcomes_at_checkpoint().is_none(), "13 批不是检查点");

        ledger.record(record("b13", true)).expect("第 14 批应成功");
        let (cp, outcomes) = ledger.outcomes_at_checkpoint().expect("14 批是基础检查点");
        assert_eq!(cp, Checkpoint::Base);
        assert_eq!(outcomes.len(), BASE_BATCHES);
    }

    /// 14 批后未激活扩展则拒绝加批(防悄悄加批)
    #[test]
    fn test_batch_15_requires_extension_activation() {
        let mut ledger = BatchLedger::new();
        for i in 0..14 {
            ledger
                .record(record(&format!("b{i}"), true))
                .expect("记录应成功");
        }
        assert!(
            ledger.record(record("b14", true)).is_err(),
            "扩展未激活应拒绝"
        );

        ledger.activate_extension().expect("14 批时激活应成功");
        ledger
            .record(record("b14", true))
            .expect("激活后第 15 批应成功");
    }

    /// 扩展只能在恰好 14 批时激活
    #[test]
    fn test_extension_only_at_base_checkpoint() {
        let mut ledger = BatchLedger::new();
        for i in 0..10 {
            ledger
                .record(record(&format!("b{i}"), true))
                .expect("记录应成功");
        }
        assert!(ledger.activate_extension().is_err(), "10 批不可激活扩展");
    }

    /// 25 批为唯一终判,拒绝第 26 批
    #[test]
    fn test_batch_26_rejected() {
        let mut ledger = BatchLedger::new();
        for i in 0..14 {
            ledger
                .record(record(&format!("b{i}"), i % 2 == 0))
                .expect("记录应成功");
        }
        ledger.activate_extension().expect("激活扩展");
        for i in 14..25 {
            ledger
                .record(record(&format!("b{i}"), true))
                .expect("记录应成功");
        }
        let (cp, _) = ledger.outcomes_at_checkpoint().expect("25 批是扩展检查点");
        assert_eq!(cp, Checkpoint::Extended);
        assert!(
            ledger.record(record("b25", true)).is_err(),
            "第 26 批应拒绝"
        );
    }

    /// 扩展激活后 15-24 批之间同样不可窥视
    #[test]
    fn test_no_peeking_between_checkpoints() {
        let mut ledger = BatchLedger::new();
        for i in 0..14 {
            ledger
                .record(record(&format!("b{i}"), true))
                .expect("记录应成功");
        }
        ledger.activate_extension().expect("激活扩展");
        ledger.record(record("b14", true)).expect("第 15 批");
        assert!(ledger.outcomes_at_checkpoint().is_none(), "15 批不是检查点");
    }

    /// 胜数统计正确
    #[test]
    fn test_wins_count() {
        let mut ledger = BatchLedger::new();
        for i in 0..5 {
            ledger
                .record(record(&format!("b{i}"), i < 3))
                .expect("记录应成功");
        }
        assert_eq!(ledger.wins(), 3);
        assert_eq!(ledger.audit_records().len(), 5);
    }
}
