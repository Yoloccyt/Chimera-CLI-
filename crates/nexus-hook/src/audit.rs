//! Hook 审计 — 全量执行记录（P3-T3，v4.0 WI-24）
//!
//! 对应架构层: L9 Quest（nexus-hook，ADR-146）
//!
//! 审计条目记录:事件/命令/退出码/耗时/中断标志;可接 session-store
//! （预留:审计为 append-only,与"model-visible means logged"不变量对齐）。

use std::time::Duration;

/// 单条审计记录
#[derive(Debug, Clone, PartialEq)]
pub struct HookAuditEntry {
    /// 事件
    pub event: crate::lifecycle::LifecycleEvent,
    /// 命令
    pub command: String,
    /// 退出码（None = 超时熔断）
    pub exit_code: Option<i32>,
    /// 耗时
    pub duration_ms: u64,
    /// 是否中断（非零退出码 + 可中断事件 → 拒否）
    pub interrupted: bool,
    /// 是否被沙箱拒绝（未执行）
    pub sandbox_denied: bool,
}

/// Hook 审计 — append-only 记录（线程安全）
#[derive(Debug, Default)]
pub struct HookAudit {
    /// 审计条目
    entries: std::sync::Mutex<Vec<HookAuditEntry>>,
}

impl HookAudit {
    /// 新建审计
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加条目
    pub fn push(&self, entry: HookAuditEntry) {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(entry);
    }

    /// 条目数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 快照（审计导出/接 session-store）
    #[must_use]
    pub fn snapshot(&self) -> Vec<HookAuditEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 中断次数（诊断）
    #[must_use]
    pub fn interrupted_count(&self) -> usize {
        self.snapshot()
            .iter()
            .filter(|e| e.interrupted)
            .count()
    }

    /// 沙箱拒绝次数（诊断）
    #[must_use]
    pub fn sandbox_denied_count(&self) -> usize {
        self.snapshot()
            .iter()
            .filter(|e| e.sandbox_denied)
            .count()
    }

    /// 平均耗时（ms;空审计返回 0）
    #[must_use]
    pub fn avg_duration_ms(&self) -> f64 {
        let snap = self.snapshot();
        if snap.is_empty() {
            return 0.0;
        }
        snap.iter().map(|e| e.duration_ms).sum::<u64>() as f64 / snap.len() as f64
    }
}

/// 审计汇出接口 — 全量审计接持久化层（session-store 注入点,P3-T3 补）
///
/// 组合根装配 session-store 适配器（append-only 落盘）;默认 [`NoopAuditSink`]
/// 保持纯内存审计（空载 = 现状,回退路径安全）。
pub trait AuditSink: Send + Sync {
    /// 汇出一条审计记录（幂等;失败由实现侧记录,不阻塞主流程）
    fn push_audit(&self, entry: &HookAuditEntry);
}

/// 无操作汇出 — 默认（纯内存审计）
#[derive(Debug, Default)]
pub struct NoopAuditSink;

impl AuditSink for NoopAuditSink {
    fn push_audit(&self, _entry: &HookAuditEntry) {}
}

/// 审计条目构造辅助（executor 内部使用）
pub(crate) fn make_entry(
    event: crate::lifecycle::LifecycleEvent,
    command: &str,
    exit_code: Option<i32>,
    elapsed: Duration,
    interrupted: bool,
    sandbox_denied: bool,
) -> HookAuditEntry {
    HookAuditEntry {
        event,
        command: command.to_string(),
        exit_code,
        duration_ms: elapsed.as_millis() as u64,
        interrupted,
        sandbox_denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::LifecycleEvent;

    /// 审计 append-only — 顺序保留 + 计数
    #[test]
    fn audit_append_only() {
        let a = HookAudit::new();
        assert!(a.is_empty());
        a.push(make_entry(LifecycleEvent::PreToolUse, "git stash", Some(0), Duration::from_millis(10), false, false));
        a.push(make_entry(LifecycleEvent::PostToolUse, "notify", Some(1), Duration::from_millis(5), true, false));
        a.push(make_entry(LifecycleEvent::Error, "echo err", None, Duration::from_millis(100), false, true));
        assert_eq!(a.len(), 3);
        assert_eq!(a.interrupted_count(), 1);
        assert_eq!(a.sandbox_denied_count(), 1);
        assert!((a.avg_duration_ms() - (10.0 + 5.0 + 100.0) / 3.0).abs() < 1e-9);
        // 快照顺序 = 追加序
        let snap = a.snapshot();
        assert_eq!(snap[0].command, "git stash");
        assert_eq!(snap[2].exit_code, None, "超时熔断 exit_code=None");
    }

    /// 空审计 — 计数/均值零值安全
    #[test]
    fn empty_audit_zero_safe() {
        let a = HookAudit::new();
        assert_eq!(a.avg_duration_ms(), 0.0);
        assert_eq!(a.interrupted_count(), 0);
    }
}
