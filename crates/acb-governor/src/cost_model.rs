//! MCA 成本模型 — 通道成本 EWMA + 预算否决 + service_tier 授权(§5.5,ADR-068)
//!
//! 对应架构层:L8 Parliament(acb-governor)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.5 治理亲和
//!
//! # 成本治理三职责(P6 成本先行)
//! 1. **成本 EWMA**:每通道预估/实际成本 EWMA(α=0.1,对齐 ADR-037)。
//!    `ModelAffinitySelected` 携带 `cost_estimate` → record_estimate;
//!    `StreamSessionCompleted` 携带 `cost_actual` → record_actual 回写。
//! 2. **预算否决**:acb-governor 有权否决路由——预估成本推预算过阈即否决
//!    (返回否决,调用方回落廉价档级联或换通道)。日成本 > 预算 120% →
//!    `BudgetExceeded`(既有 Critical 事件,零新增)。
//! 3. **service_tier 授权**:priority 加价档(MiniMax 1.5×)必须由 BudgetMask
//!    显式授权,禁止默认开启(P6)。
//!
//! # 微元整数口径
//! 成本单位为微元(µ¥/µ$,1e-6),EWMA 内部用 f64(平滑需浮点),
//! 累计总额用整数(避免浮点累加误差)。峰谷/缓存折扣在 mca-gateway
//! 上游已计入 cost_micro,本模型只做聚合治理。
//!
//! # C7 红线
//! `Mutex<HashMap>` 同步聚合,锁内完成不跨 await;调用方在 async 上下文
//! 直接调用(record 是微秒级同步操作)。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// 成本 EWMA 平滑系数(对齐 ADR-037 α=0.1)
const COST_EWMA_ALPHA: f64 = 0.1;

/// 预算超限阈值百分比(日成本 > 预算 120% 触发 BudgetExceeded)
pub const BUDGET_EXCEEDED_PERCENT: u64 = 120;

/// 单通道成本统计
#[derive(Debug, Clone, Default)]
struct ChannelCost {
    /// 预估成本 EWMA(微元)
    estimate_ewma: f64,
    /// 实际成本 EWMA(微元)
    actual_ewma: f64,
    /// 累计实际成本(微元,整数避免浮点累加误差)
    total_actual_micro: u64,
    /// 样本数
    samples: u64,
}

/// 路由成本裁决 — acb-governor 对路由的成本否决权(P6)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostVerdict {
    /// 允许路由(预估成本在预算内)
    Allow,
    /// 否决路由(预估成本推预算过阈,应回落廉价档或换通道)
    Veto,
}

/// MCA 亲和成本模型 — 每通道成本聚合 + 预算治理
///
/// Clone 不适用(内部 Mutex);由 AcbGovernor 持有单实例,跨任务共享用 Arc。
#[derive(Debug)]
pub struct AffinityCostModel {
    /// 每通道成本统计(route_key → ChannelCost)
    channels: Mutex<HashMap<String, ChannelCost>>,
    /// 累计实际总支出(微元,无锁原子)
    total_spend_micro: AtomicU64,
    /// 日预算上限(微元;0 = 不限)
    daily_budget_micro: u64,
}

impl AffinityCostModel {
    /// 创建成本模型(daily_budget_micro = 0 表示不限预算)
    pub fn new(daily_budget_micro: u64) -> Self {
        Self {
            channels: Mutex::new(HashMap::new()),
            total_spend_micro: AtomicU64::new(0),
            daily_budget_micro,
        }
    }

    /// 记录路由预估成本(消费 `ModelAffinitySelected.cost_estimate_micro`)
    pub fn record_estimate(&self, route_key: &str, cost_micro: u64) {
        if let Ok(mut map) = self.channels.lock() {
            let c = map.entry(route_key.to_string()).or_default();
            c.estimate_ewma = ewma(c.estimate_ewma, cost_micro as f64, c.samples == 0);
        }
    }

    /// 记录路由实际成本(消费 `StreamSessionCompleted.cost_actual_micro`)
    ///
    /// 回写 EWMA + 累计总支出;返回记录后是否超预算(供发布 BudgetExceeded)。
    pub fn record_actual(&self, route_key: &str, cost_micro: u64) -> bool {
        if let Ok(mut map) = self.channels.lock() {
            let c = map.entry(route_key.to_string()).or_default();
            c.actual_ewma = ewma(c.actual_ewma, cost_micro as f64, c.samples == 0);
            c.total_actual_micro += cost_micro;
            c.samples += 1;
        }
        self.total_spend_micro
            .fetch_add(cost_micro, Ordering::AcqRel);
        self.is_over_budget()
    }

    /// 路由成本否决判定(P6:acb-governor 有权否决路由)
    ///
    /// 预估成本叠加当前支出若超预算阈值 → Veto(调用方回落廉价档/换通道)。
    pub fn route_verdict(&self, estimate_micro: u64) -> CostVerdict {
        if self.daily_budget_micro == 0 {
            return CostVerdict::Allow;
        }
        let projected = self.total_spend_micro.load(Ordering::Acquire) + estimate_micro;
        let threshold = self.daily_budget_micro * BUDGET_EXCEEDED_PERCENT / 100;
        if projected > threshold {
            CostVerdict::Veto
        } else {
            CostVerdict::Allow
        }
    }

    /// 是否已超预算(日成本 > 预算 × 120%)
    pub fn is_over_budget(&self) -> bool {
        if self.daily_budget_micro == 0 {
            return false;
        }
        let spend = self.total_spend_micro.load(Ordering::Acquire);
        spend > self.daily_budget_micro * BUDGET_EXCEEDED_PERCENT / 100
    }

    /// service_tier 授权:priority 加价档必须由 BudgetMask 显式授权(P6)
    ///
    /// # 参数
    /// - `is_priority`: 请求的服务档是否为 priority(加价档)
    /// - `budget_mask_authorized`: BudgetMask 是否显式授权加价
    ///
    /// 返回是否允许使用该服务档。standard 档恒允许;priority 档仅在
    /// 显式授权时允许(禁止默认开启,MiniMax priority 1.5× 价红线)。
    pub fn authorize_service_tier(is_priority: bool, budget_mask_authorized: bool) -> bool {
        !is_priority || budget_mask_authorized
    }

    /// 通道实际成本 EWMA(微元;未记录返回 None)
    pub fn channel_actual_ewma(&self, route_key: &str) -> Option<f64> {
        self.channels
            .lock()
            .ok()
            .and_then(|m| m.get(route_key).map(|c| c.actual_ewma))
    }

    /// 累计总支出(微元)
    pub fn total_spend_micro(&self) -> u64 {
        self.total_spend_micro.load(Ordering::Acquire)
    }

    /// 日预算上限(微元;0 = 不限)
    pub fn daily_budget_micro(&self) -> u64 {
        self.daily_budget_micro
    }
}

/// EWMA 更新(首样本直取,避免 0 基线拖低)
fn ewma(current: f64, sample: f64, first: bool) -> f64 {
    if first {
        sample
    } else {
        COST_EWMA_ALPHA * sample + (1.0 - COST_EWMA_ALPHA) * current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_actual_accumulates_total_spend() {
        let model = AffinityCostModel::new(0);
        model.record_actual("deep_seek/deepseek-v4-flash", 1000);
        model.record_actual("zhipu/glm-5.2", 2000);
        assert_eq!(model.total_spend_micro(), 3000);
    }

    #[test]
    fn ewma_first_sample_direct_then_smoothed() {
        let model = AffinityCostModel::new(0);
        model.record_actual("x/y", 100);
        assert!((model.channel_actual_ewma("x/y").unwrap() - 100.0).abs() < 1e-6);
        model.record_actual("x/y", 300);
        // EWMA: 0.1*300 + 0.9*100 = 120
        assert!((model.channel_actual_ewma("x/y").unwrap() - 120.0).abs() < 1e-6);
    }

    #[test]
    fn route_verdict_vetoes_when_over_budget() {
        // 日预算 1000 微元,阈值 120% = 1200
        let model = AffinityCostModel::new(1000);
        // 已花 1000,再预估 300 → 投影 1300 > 1200 → Veto
        model.record_actual("x/y", 1000);
        assert_eq!(model.route_verdict(300), CostVerdict::Veto);
        // 小额预估仍在阈值内 → Allow
        assert_eq!(model.route_verdict(100), CostVerdict::Allow);
    }

    #[test]
    fn unlimited_budget_never_vetoes() {
        let model = AffinityCostModel::new(0);
        model.record_actual("x/y", 1_000_000_000);
        assert_eq!(model.route_verdict(1_000_000), CostVerdict::Allow);
        assert!(!model.is_over_budget());
    }

    #[test]
    fn over_budget_detection() {
        let model = AffinityCostModel::new(1000);
        assert!(!model.is_over_budget());
        // 花费 1201 > 1200(120%)→ 超预算
        model.record_actual("x/y", 1201);
        assert!(model.is_over_budget());
    }

    #[test]
    fn record_actual_returns_over_budget_flag() {
        let model = AffinityCostModel::new(1000);
        assert!(!model.record_actual("x/y", 500));
        // 累计 500 + 800 = 1300 > 1200 → 返回 true(触发 BudgetExceeded)
        assert!(model.record_actual("x/y", 800));
    }

    #[test]
    fn service_tier_priority_requires_authorization() {
        // standard 恒允许
        assert!(AffinityCostModel::authorize_service_tier(false, false));
        // priority 未授权 → 拒绝(禁止默认开启)
        assert!(!AffinityCostModel::authorize_service_tier(true, false));
        // priority 显式授权 → 允许
        assert!(AffinityCostModel::authorize_service_tier(true, true));
    }
}
