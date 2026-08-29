//! Task Auction 市场 — 短任务派发竞价（P3-T9，v4.0 WI-25 + AUX-5）
//!
//! 对应架构层: L7 Execution（nexus-subagent，ADR-148）
//!
//! # 设计（AUX-5 Task Auction Market）
//! SubAgent 按能力标签+负载自报价 `bid(task) → Bid{cost, capability_match}`,
//! 编排层 `min_by(cost/match)` 择胜;与 mas-sched 分工:
//! **Claim 管长任务租约（L9 mas-sched）/ Auction 管短任务派发（本模块）**。
//!
//! # 防抖动/饿死（ADR-148 门禁）
//! - 负载平滑:报价含 `load` 因子（高负载报价高,自动退让）;
//! - 最低价兜底队列:无合格报价时按 profile 顺序兜底派发（不饿死）。

/// 报价 — SubAgent 对任务的竞标
#[derive(Debug, Clone, PartialEq)]
pub struct Bid {
    /// 报价档案 ID
    pub profile_id: String,
    /// 成本（单位成本 × 负载惩罚）
    pub cost: f64,
    /// 能力匹配度（0.0-1.0）
    pub capability_match: f64,
}

impl Bid {
    /// 择胜评分 = cost / max(match, ε)——越低越优（成本优先,匹配保底）
    #[must_use]
    pub fn score(&self) -> f64 {
        self.cost / self.capability_match.max(1e-9)
    }
}

/// 任务报价请求
#[derive(Debug, Clone, PartialEq)]
pub struct TaskOffer {
    /// 任务 ID
    pub task_id: String,
    /// 需求能力标签（逗号分隔）
    pub required_capabilities: String,
}

/// 竞价结果
#[derive(Debug, Clone, PartialEq)]
pub enum AuctionOutcome {
    /// 择胜者（min_by score）
    Won(Bid),
    /// 无合格报价（兜底路径触发）
    NoBid,
}

/// 任务拍卖市场 — 报价征集 + 择胜（零依赖,纯同步;并发由调用方 Arc 共享）
#[derive(Debug, Default)]
pub struct TaskAuction {
    /// 注册档案
    profiles: Vec<crate::types::SubAgentProfile>,
}

impl TaskAuction {
    /// 新建市场
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册档案（重复 ID 覆盖——幂等更新）
    pub fn register(&mut self, profile: crate::types::SubAgentProfile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.profile_id == profile.profile_id) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    /// 注销档案
    pub fn unregister(&mut self, profile_id: &str) {
        self.profiles.retain(|p| p.profile_id != profile_id);
    }

    /// 档案数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// 征集报价 — 全部档案按能力匹配出价
    ///
    /// 报价公式:`cost = unit_cost × (1 + load)`（负载平滑:高负载报价高,
    /// 自动退让给低负载同能力者）;匹配度 = 标签交叠比例。
    #[must_use]
    pub fn collect_bids(&self, offer: &TaskOffer) -> Vec<Bid> {
        self.profiles
            .iter()
            .filter(|p| p.match_ratio(&offer.required_capabilities) > 0.0)
            .map(|p| Bid {
                profile_id: p.profile_id.clone(),
                cost: p.unit_cost * (1.0 + p.load),
                capability_match: p.match_ratio(&offer.required_capabilities),
            })
            .collect()
    }

    /// 择胜 — `min_by(score)`（成本/匹配比最小者胜）
    ///
    /// # 兜底
    /// 无匹配档案 → `NoBid`（调用方触发最低价兜底队列:按注册顺序取首个,
    /// 由调用方决定——防饿死 ADR-148）。
    #[must_use]
    pub fn auction(&self, offer: &TaskOffer) -> AuctionOutcome {
        let bids = self.collect_bids(offer);
        match bids.into_iter().min_by(|a, b| {
            a.score()
                .partial_cmp(&b.score())
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Some(b) => AuctionOutcome::Won(b),
            None => AuctionOutcome::NoBid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SubAgentKind, SubAgentProfile};

    fn market() -> TaskAuction {
        let mut m = TaskAuction::new();
        m.register(SubAgentProfile::new("coder-a", SubAgentKind::Coder, 2.0));
        m.register(SubAgentProfile::new("explore-b", SubAgentKind::Explore, 1.0));
        m.register(SubAgentProfile::new("coder-c", SubAgentKind::Coder, 1.5));
        m
    }

    /// 择胜 — 能力匹配过滤 + min_by(score) 胜出
    #[test]
    fn auction_wins_best_match() {
        let m = market();
        let offer = TaskOffer {
            task_id: "t1".into(),
            required_capabilities: "code".into(),
        };
        match m.auction(&offer) {
            AuctionOutcome::Won(b) => {
                // coder-c 成本 1.5 < coder-a 2.0 → 胜
                assert_eq!(b.profile_id, "coder-c");
            }
            AuctionOutcome::NoBid => panic!("必须有人胜出"),
        }
    }

    /// 能力过滤 — 不匹配档案不出价
    #[test]
    fn capability_filter() {
        let m = market();
        let offer = TaskOffer {
            task_id: "t2".into(),
            required_capabilities: "plan".into(),
        };
        assert_eq!(m.auction(&offer), AuctionOutcome::NoBid, "无 plan 档案 → NoBid");
    }

    /// 负载平滑 — 高负载同能力者退让
    #[test]
    fn load_smoothing() {
        let mut m = TaskAuction::new();
        let mut cheap = SubAgentProfile::new("cheap", SubAgentKind::Coder, 1.0);
        cheap.load = 0.9; // 高负载
        let mut expensive = SubAgentProfile::new("expensive", SubAgentKind::Coder, 1.2);
        expensive.load = 0.0; // 空闲
        m.register(cheap);
        m.register(expensive);
        let offer = TaskOffer { task_id: "t3".into(), required_capabilities: "code".into() };
        // cheap: 1.0 × 1.9 = 1.9;expensive: 1.2 × 1.0 = 1.2 → expensive 胜
        match m.auction(&offer) {
            AuctionOutcome::Won(b) => assert_eq!(b.profile_id, "expensive", "高负载必须退让"),
            AuctionOutcome::NoBid => panic!("必须有人胜出"),
        }
    }

    /// 注册幂等 — 重复 ID 覆盖
    #[test]
    fn register_idempotent() {
        let mut m = TaskAuction::new();
        m.register(SubAgentProfile::new("p", SubAgentKind::Coder, 1.0));
        m.register(SubAgentProfile::new("p", SubAgentKind::Coder, 9.0));
        assert_eq!(m.len(), 1, "重复注册必须覆盖");
        let offer = TaskOffer { task_id: "t".into(), required_capabilities: "code".into() };
        match m.auction(&offer) {
            AuctionOutcome::Won(b) => assert!((b.cost - 9.0).abs() < 1e-9, "新成本生效"),
            AuctionOutcome::NoBid => panic!("必须有人胜出"),
        }
    }

    /// 并发拍卖 — 只读市场多线程安全（无内部状态变更）
    #[test]
    fn concurrent_auction() {
        let m = std::sync::Arc::new(market());
        let offer = TaskOffer { task_id: "t".into(), required_capabilities: "code,explore".into() };
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let m = std::sync::Arc::clone(&m);
                let offer = offer.clone();
                std::thread::spawn(move || m.auction(&offer))
            })
            .collect();
        for h in handles {
            let outcome = h.join().expect("线程正常");
            assert!(matches!(outcome, AuctionOutcome::Won(_)), "必须一致胜出");
        }
    }
}
