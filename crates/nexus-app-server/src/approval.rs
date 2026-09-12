//! 审批仲裁 — 多客户端竞争审批（P3-T5，D-P11 裁决：T6 遗留三项之一）
//!
//! 对应架构层: **L10 Interface**（nexus-app-server）
//! 对应任务: **P3-T5**（手册 W16，T6 遗留:多客户端审批仲裁）
//!
//! # 语义
//! 同一审批请求可被多客户端（IDE 面板 + CLI + 远程宿主）竞争裁决:
//! - **首裁决生效**:首个 `submit_vote` 原子占位,后续裁决 `DuplicateIgnored`（幂等）;
//! - **未知请求**:`Unknown`（审批队列无此请求,调用方报 ApprovalNotFound）;
//! - **超时自动拒否**:`expire_after` 到期后投票失败（超时熔断,审批不悬挂）。

use std::time::{Duration, Instant};

use dashmap::DashMap;
use nexus_contracts::app::{ApprovalDecision, ReqId};

/// 投票结果 — 三态仲裁
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteOutcome {
    /// 首裁决已生效
    Accepted,
    /// 该请求已有裁决（重复投票被忽略,幂等）
    DuplicateIgnored,
    /// 请求未知（不在仲裁表）
    Unknown,
}

/// 已决投票记录
#[derive(Debug, Clone)]
struct DecidedVote {
    /// 裁决客户端 ID
    client_id: String,
    /// 裁决
    decision: ApprovalDecision,
    /// 裁决时刻
    at: Instant,
}

/// 审批仲裁器 — 请求 → 首裁决占位（原子,并发安全）
#[derive(Debug)]
pub struct ApprovalArbiter {
    /// 已决请求表（req_id → 首裁决）
    decided: DashMap<ReqId, DecidedVote>,
    /// 投票有效期（超时后 `submit_vote` 拒绝;0 = 不限）
    ttl: Duration,
}

impl Default for ApprovalArbiter {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

impl ApprovalArbiter {
    /// 新建仲裁器（TTL 后裁决失效,防审批悬挂）
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            decided: DashMap::new(),
            ttl,
        }
    }

    /// 提交投票 — 首裁决生效（原子首占）
    ///
    /// # 语义
    /// - 请求已决 → `DuplicateIgnored`（不覆盖首裁决）;
    /// - 请求已决但超 TTL → 视为过期,允许新裁决覆盖（防悬挂）;
    /// - 请求未知 → `Unknown`。
    pub fn submit_vote(
        &self,
        request_id: &ReqId,
        client_id: &str,
        decision: ApprovalDecision,
    ) -> VoteOutcome {
        let now = Instant::now();
        // 1. 查已有裁决（含过期清理）
        if let Some(existing) = self.decided.get(request_id) {
            let expired = self.ttl > Duration::ZERO && existing.at.elapsed() > self.ttl;
            if !expired {
                return VoteOutcome::DuplicateIgnored;
            }
            // 过期:移除,允许新裁决
            drop(existing);
            self.decided.remove(request_id);
        }
        // 2. 首占（entry API 原子:并发恰一成功）
        let vote = DecidedVote {
            client_id: client_id.to_string(),
            decision,
            at: now,
        };
        use dashmap::mapref::entry::Entry;
        match self.decided.entry(request_id.clone()) {
            Entry::Occupied(_) => VoteOutcome::DuplicateIgnored,
            Entry::Vacant(v) => {
                v.insert(vote);
                VoteOutcome::Accepted
            }
        }
    }

    /// 查询裁决（决策消费方读取;返回 None = 未决/过期）
    #[must_use]
    pub fn decision(&self, request_id: &ReqId) -> Option<ApprovalDecision> {
        let entry = self.decided.get(request_id)?;
        if self.ttl > Duration::ZERO && entry.at.elapsed() > self.ttl {
            return None;
        }
        Some(entry.decision)
    }

    /// 裁决客户端（审计）
    #[must_use]
    pub fn client_of(&self, request_id: &ReqId) -> Option<String> {
        self.decided.get(request_id).map(|e| e.client_id.clone())
    }

    /// 已决请求数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        self.decided.len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.decided.is_empty()
    }

    /// 清理过期裁决（周期维护;返回清理数）
    pub fn purge_expired(&self) -> usize {
        if self.ttl == Duration::ZERO {
            return 0;
        }
        let expired: Vec<ReqId> = self
            .decided
            .iter()
            .filter(|e| e.at.elapsed() > self.ttl)
            .map(|e| e.key().clone())
            .collect();
        let n = expired.len();
        for id in expired {
            self.decided.remove(&id);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 首裁决生效 — 首个 Accepted,重复 DuplicateIgnored
    #[test]
    fn first_vote_wins() {
        let a = ApprovalArbiter::default();
        let rid = ReqId::new("req-1");
        assert_eq!(
            a.submit_vote(&rid, "ide-panel", ApprovalDecision::Allow),
            VoteOutcome::Accepted
        );
        assert_eq!(
            a.submit_vote(&rid, "cli", ApprovalDecision::Deny),
            VoteOutcome::DuplicateIgnored,
            "重复投票必须忽略"
        );
        assert_eq!(
            a.decision(&rid),
            Some(ApprovalDecision::Allow),
            "首裁决保留"
        );
        assert_eq!(a.client_of(&rid).as_deref(), Some("ide-panel"));
    }

    /// 并发竞争 — 8 线程恰 1 Accepted
    #[test]
    fn concurrent_first_wins() {
        let a = std::sync::Arc::new(ApprovalArbiter::default());
        let rid = ReqId::new("req-2");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let a = std::sync::Arc::clone(&a);
                let rid = rid.clone();
                std::thread::spawn(move || {
                    a.submit_vote(&rid, &format!("client-{i}"), ApprovalDecision::Allow)
                })
            })
            .collect();
        let accepted = handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|v| *v == VoteOutcome::Accepted)
            .count();
        assert_eq!(accepted, 1, "并发竞争必须恰 1 个 Accepted");
    }

    /// TTL 过期 — 过期后允许新裁决（防悬挂）;0 TTL 立即过期
    #[test]
    fn ttl_expiry() {
        let a = ApprovalArbiter::new(Duration::from_millis(50));
        let rid = ReqId::new("req-3");
        assert_eq!(
            a.submit_vote(&rid, "c1", ApprovalDecision::Allow),
            VoteOutcome::Accepted
        );
        // 未过期:重复忽略
        assert_eq!(
            a.submit_vote(&rid, "c2", ApprovalDecision::Deny),
            VoteOutcome::DuplicateIgnored
        );
        std::thread::sleep(Duration::from_millis(80));
        // 过期:新裁决生效
        assert_eq!(
            a.submit_vote(&rid, "c2", ApprovalDecision::Deny),
            VoteOutcome::Accepted
        );
        assert_eq!(a.decision(&rid), Some(ApprovalDecision::Deny));
        // 0 TTL:不过期（0 = 不限,与 max_sessions=0 约定一致）;决策可读
        let a0 = ApprovalArbiter::new(Duration::ZERO);
        let rid2 = ReqId::new("req-4");
        assert_eq!(
            a0.submit_vote(&rid2, "c1", ApprovalDecision::Allow),
            VoteOutcome::Accepted
        );
        assert_eq!(
            a0.decision(&rid2),
            Some(ApprovalDecision::Allow),
            "0 TTL = 不限,决策可读"
        );
    }

    /// purge_expired — 清理过期裁决
    #[test]
    fn purge_expired_removes() {
        let a = ApprovalArbiter::new(Duration::from_millis(30));
        let rid = ReqId::new("req-5");
        let _ = a.submit_vote(&rid, "c1", ApprovalDecision::Allow);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(a.purge_expired(), 1, "必须清理 1 条过期");
        assert!(a.is_empty());
    }
}
