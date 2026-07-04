//! 孤儿调用检测器
//!
//! 对应 Claude Code 尸检教训:5.4% 孤儿调用(void Promise 无 await)。
//!
//! ## 机制
//!
//! `OrphanDetector` 负责收集与查询孤儿调用报告。
//! 真正的检测逻辑由 `OrphanGuard`(在 `protocol.rs` 中)实现:
//! - `entangle()` 创建 future 时,同时创建 `OrphanGuard`
//! - `OrphanGuard` 持有一个 `completed: Arc<AtomicBool>` 标志
//! - future 正常完成时,`entangle()` 将 `completed` 设为 `true`
//! - future 被 drop(未完成)时,`OrphanGuard::drop` 检测到 `completed == false`,
//!   生成 `OrphanReport` 并调用 `OrphanDetector::report_orphan`
//!
//! 这种设计利用 Rust 的 Drop 语义,无论 future 因何种原因被 drop
//! (超时、取消、调用者不 await),都能被检测到。

use std::time::Duration;

use crate::types::OrphanReport;

/// 孤儿调用检测器
///
/// 收集所有由 `OrphanGuard` 报告的孤儿调用,提供查询与清理接口。
/// 在 `QeepProtocol` 中通过 `Mutex<OrphanDetector>` 包装,实现内部可变性。
pub struct OrphanDetector {
    /// 已检测到的孤儿调用报告列表
    orphans: Vec<OrphanReport>,
    /// 检测间隔(预留:未来用于后台周期性扫描长时间 pending 的调用)
    #[allow(dead_code)]
    detection_interval: Duration,
}

impl Default for OrphanDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl OrphanDetector {
    /// 创建新的孤儿检测器,默认检测间隔 60 秒。
    pub fn new() -> Self {
        Self {
            orphans: Vec::new(),
            detection_interval: Duration::from_secs(60),
        }
    }

    /// 报告一个孤儿调用(由 `OrphanGuard::drop` 调用)。
    pub fn report_orphan(&mut self, report: OrphanReport) {
        self.orphans.push(report);
    }

    /// 获取所有已检测到的孤儿调用报告(只读视图)。
    pub fn detect_orphans(&self) -> &[OrphanReport] {
        &self.orphans
    }

    /// 获取孤儿调用数量。
    pub fn orphan_count(&self) -> usize {
        self.orphans.len()
    }

    /// 清空所有孤儿调用报告(用于测试或周期性归档后重置)。
    pub fn clear(&mut self) {
        self.orphans.clear();
    }
}
