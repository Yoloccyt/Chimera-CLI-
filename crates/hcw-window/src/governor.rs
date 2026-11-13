//! WindowGovernor — 每请求窗口装载计划生成器（HCW v3 蓝图 §5.4 P1，ADR-293）
//!
//! 核心职责：按模型有效能力档案动态定容常驻层（resident_cap），产出
//! [`WindowPlan`] 供 [`crate::window::HcwWindow::select_window`] 消费。
//! Ω₂ 从「压缩定律」升级为「预算定律」：逻辑工作窗口
//! W_logical = Resident × Compression_ratio_structured + Σ(按需检索层命中)，
//! 常驻层按**有效窗口**（非标称）定容且逐模型标定（蓝图 §一 四面墙）。
//!
//! # 依赖方向（蓝图 §5.2 铁律）
//! hcw-window（L2）不感知 mca-gateway（L10）/decb-governor（L8）/
//! quest-engine（L9）——[`PlanContext`] 用纯数据字段承载外部输入，
//! 由装配面（组合根）采集后传入。hcw 只消费数据，不拉取来源。
//!
//! # 默认行为
//! 未装配 governor 时 HcwWindow 走现状路径**逐位不变**（静态回退档）；
//! [`StaticGovernor`] 为显式的静态缺省实现（plan_source="static_fallback"）。

use serde::{Deserialize, Serialize};

/// R/S/E/M 四层配额比例（蓝图 §5.1 分层重参数化）
///
/// R=Resident 常驻 / S=Structured 结构化（Ω₁₀ 卡片/摘要）/ E=Episodic 情景
/// （MLC L1 + session-store 权威源）/ M=Semantic 语义（MLC L2/L3 + repo-wiki）。
/// 比例之和恒为 1.0（[`TierAlloc::is_valid`]），序列化快照进
/// `WindowPlanApplied` 事件供双跑对照回放。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TierAlloc {
    /// 常驻层配额占比
    pub resident: f32,
    /// 结构化层配额占比
    pub structured: f32,
    /// 情景层配额占比
    pub episodic: f32,
    /// 语义层配额占比
    pub semantic: f32,
}

impl TierAlloc {
    /// 缺省配额：常驻优先（0.6），结构化次之，E/M 均摊余量
    ///
    /// WHY 该比例：蓝图 §四 的 W_logical 主要由 Resident×压缩比贡献，
    /// 结构化层是 10-20× 压缩比的产能面；E/M 检索按需命中不计入常驻。
    /// 比例为初值，后续按 P2 卡片压缩比测量结果调整（蓝图 §七 持续项）。
    pub const DEFAULT: Self = Self {
        resident: 0.6,
        structured: 0.2,
        episodic: 0.1,
        semantic: 0.1,
    };

    /// 校验：四项非负且和 ≈ 1.0（容差 1e-3，与 L0 SelectorWeights::is_valid 同口径）
    pub fn is_valid(&self) -> bool {
        let sum = self.resident + self.structured + self.episodic + self.semantic;
        self.resident >= 0.0
            && self.structured >= 0.0
            && self.episodic >= 0.0
            && self.semantic >= 0.0
            && (sum - 1.0).abs() < 1e-3
    }
}

impl Default for TierAlloc {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 计划来源（`WindowPlanApplied.plan_source` 事件的分流标记）
///
/// WHY: 蓝图 §八 P1 回滚设计要求「新旧 WindowPlan 同任务双跑对照」——
/// 事件按来源分流是双跑对账的锚点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanSource {
    /// 静态缺省档（低能力机型/未装配 governor 时的现状等效路径）
    StaticFallback,
    /// governor 动态定容（按能力档案 + 预算 + 思考档位）
    Governor,
}

impl PlanSource {
    /// 事件载荷字符串（与 `WindowPlanApplied.plan_source: String` 对齐）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StaticFallback => "static_fallback",
            Self::Governor => "governor",
        }
    }
}

/// 每请求窗口装载计划（蓝图 §六 Rust 类型草案落地）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowPlan {
    /// 本请求常驻上限（token）= min(有效窗口×安全系数, 预算上限)
    pub resident_cap: u32,
    /// 页预算（沿用 4000 码元页预算传统）
    pub page_budget: u32,
    /// R/S/E/M 四层配额比例
    pub tier_allocation: TierAlloc,
    /// 计划来源（双跑对照分流标记）
    pub plan_source: PlanSource,
}

/// governor 的外部输入快照（蓝图 §5.4：能力档案 + 预算头寸 + 思考档位）
///
/// WHY 纯数据而非引用：hcw-window（L2）不感知 decb-governor（L8）与
/// quest-engine（L9）——组合根采集 `DecbGovernor::get_stats()` 与
/// `TtgGovernor::current_mode()` 后填入本结构传入。
#[derive(Debug, Clone)]
pub struct PlanContext {
    /// 模型有效窗口（token）——来自 L0 [`nexus_contracts_mca::mca::context_capability::ContextCapabilityProfile::effective_window`]，
    /// 由装配面传入（hcw 不感知 L10）
    pub effective_window: u32,
    /// decb 预算利用率（0.0=空载, 1.0=耗尽；超阈值触发 resident 收缩）
    pub budget_utilization: f32,
    /// 思考档位（"Fast"/"Standard"/"Deep"——Deep 档放宽安全系数）
    pub thinking_mode: String,
}

/// WindowGovernor trait — 组合根可注入 mock 供测试（蓝图 §六）
pub trait WindowGovernor: Send + Sync {
    /// 产出本请求的窗口装载计划
    fn plan(&self, ctx: &PlanContext) -> WindowPlan;
}

/// 静态缺省 governor——不做动态定容（现状等效路径的显式形态）
///
/// WHY 存在：蓝图 §八「默认档=静态回退」+ 双跑对照需要 static 臂。
/// resident_cap 交给调用方的既有容量路径（HcwWindow 对 StaticFallback
/// plan 不做收缩），本实现仅产出标记性 plan。
#[derive(Debug, Default, Clone, Copy)]
pub struct StaticGovernor;

impl WindowGovernor for StaticGovernor {
    fn plan(&self, _ctx: &PlanContext) -> WindowPlan {
        WindowPlan {
            // u32::MAX = 不设上限（收缩语义见 HcwWindow 侧 min 合并）
            resident_cap: u32::MAX,
            page_budget: 4_000,
            tier_allocation: TierAlloc::DEFAULT,
            plan_source: PlanSource::StaticFallback,
        }
    }
}

/// 自适应 governor——按有效窗口/预算/思考档位动态定容（蓝图 §5.4 核心交付）
///
/// 定容公式（蓝图 §四）：`Resident = min(effective_window × s, 预算收缩项)`
/// - 安全系数 s：缺省 0.8（有效窗口本身已含 rot 余量）；Deep 档放宽 0.9
/// - 预算收缩：utilization > [`Self::BUDGET_SHRINK_THRESHOLD`] 时
///   `resident ×= max(0.5, 1.0 - utilization)`（蓝图 §5.6 降级链的
///   resident 收缩项；0.5 下界防预算压力把常驻压到不可用）
/// - 下界保护：`resident_floor`（缺省 4K = 低能力档下限），防过度收缩
#[derive(Debug, Clone)]
pub struct AdaptiveGovernor {
    /// 常驻下界（token）——收缩保护
    resident_floor: u32,
    /// 页预算（透传进 plan）
    page_budget: u32,
    /// R/S/E/M 配额（可覆盖缺省）
    tier_allocation: TierAlloc,
}

impl AdaptiveGovernor {
    /// 预算收缩触发阈值（utilization 超过即开始收缩 resident）
    pub const BUDGET_SHRINK_THRESHOLD: f32 = 0.9;
    /// 缺省安全系数（蓝图 §四 s 初值 0.8）
    pub const SAFETY_FACTOR: f32 = 0.8;
    /// Deep 思考档放宽系数（推理密集任务放宽常驻）
    pub const DEEP_SAFETY_FACTOR: f32 = 0.9;
    /// 收缩下界系数（预算压力下的最低保留比例）
    pub const SHRINK_FLOOR_FACTOR: f32 = 0.5;

    /// 创建自适应 governor（参数走缺省：floor=4K、page_budget=4000、DEFAULT 配额）
    pub fn new() -> Self {
        Self {
            resident_floor: 4_096,
            page_budget: 4_000,
            tier_allocation: TierAlloc::DEFAULT,
        }
    }

    /// 自定义参数构造
    pub fn with_params(resident_floor: u32, page_budget: u32, tier_allocation: TierAlloc) -> Self {
        Self {
            resident_floor,
            page_budget,
            tier_allocation,
        }
    }
}

impl Default for AdaptiveGovernor {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowGovernor for AdaptiveGovernor {
    fn plan(&self, ctx: &PlanContext) -> WindowPlan {
        // 安全系数按思考档位选择（Deep 放宽；其余档 0.8）
        let factor = if ctx.thinking_mode == "Deep" {
            Self::DEEP_SAFETY_FACTOR
        } else {
            Self::SAFETY_FACTOR
        };
        // 基础定容：有效窗口 × 安全系数
        // WHY f32 乘法而非整数化:0.8 as u64 == 0（f32→int 截断），会令全部
        // resident 归零落 floor——安全系数必须保持浮点参与运算
        let mut resident = (ctx.effective_window as f32 * factor) as u32;

        // 预算收缩项：utilization 超阈值时按剩余头寸比例收缩（0.5 下界）
        if ctx.budget_utilization > Self::BUDGET_SHRINK_THRESHOLD {
            let shrink = (1.0 - ctx.budget_utilization).max(Self::SHRINK_FLOOR_FACTOR);
            resident = ((resident as f32) * shrink) as u32;
        }

        // 下界保护 + 上界钳制（不超过有效窗口本身）
        resident = resident.max(self.resident_floor).min(ctx.effective_window);

        WindowPlan {
            resident_cap: resident,
            page_budget: self.page_budget,
            tier_allocation: self.tier_allocation,
            plan_source: PlanSource::Governor,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn ctx(window: u32, util: f32, mode: &str) -> PlanContext {
        PlanContext {
            effective_window: window,
            budget_utilization: util,
            thinking_mode: mode.to_string(),
        }
    }

    #[test]
    fn tier_alloc_default_is_valid() {
        assert!(TierAlloc::DEFAULT.is_valid());
        assert_eq!(
            TierAlloc::DEFAULT.resident
                + TierAlloc::DEFAULT.structured
                + TierAlloc::DEFAULT.episodic
                + TierAlloc::DEFAULT.semantic,
            1.0
        );
    }

    #[test]
    fn static_governor_never_shrinks() {
        let plan = StaticGovernor.plan(&ctx(1_000_000, 1.0, "Fast"));
        assert_eq!(plan.resident_cap, u32::MAX, "静态回退不设收缩上限");
        assert_eq!(plan.plan_source, PlanSource::StaticFallback);
        assert_eq!(plan.plan_source.as_str(), "static_fallback");
    }

    #[test]
    fn adaptive_standard_uses_safety_factor() {
        // 1M 有效窗口 × 0.8 = 800K
        let plan = AdaptiveGovernor::new().plan(&ctx(1_000_000, 0.0, "Standard"));
        assert_eq!(plan.resident_cap, 800_000);
        assert_eq!(plan.plan_source, PlanSource::Governor);
    }

    #[test]
    fn adaptive_deep_relaxes_factor() {
        // Deep 档 0.9：1M → 900K
        let plan = AdaptiveGovernor::new().plan(&ctx(1_000_000, 0.0, "Deep"));
        assert_eq!(plan.resident_cap, 900_000);
    }

    #[test]
    fn adaptive_budget_pressure_shrinks_with_floor() {
        // utilization=1.0 → shrink=max(0, 0.5)=0.5 → 800K×0.5=400K
        let plan = AdaptiveGovernor::new().plan(&ctx(1_000_000, 1.0, "Standard"));
        assert_eq!(plan.resident_cap, 400_000);
        // 病态小模型:effective_window(1K) < floor(4K) 时**上界优先**——
        // resident 不得超过有效窗口(窗口只有 1K,resident 4096 无意义):
        // max(800,floor)=4096 → min(4096, effective)=1000
        let tiny = AdaptiveGovernor::new().plan(&ctx(1_000, 0.0, "Standard"));
        assert_eq!(tiny.resident_cap, 1_000);
    }

    #[test]
    fn adaptive_clamps_to_effective_window() {
        // 预算头寸为负的病态输入也不超过有效窗口
        let plan = AdaptiveGovernor::new().plan(&ctx(100_000, 0.0, "Deep"));
        assert_eq!(plan.resident_cap, 90_000);
        assert!(plan.resident_cap <= 100_000);
    }

    #[test]
    fn window_plan_serde_roundtrip() {
        let plan = AdaptiveGovernor::new().plan(&ctx(500_000, 0.95, "Fast"));
        let json = serde_json::to_string(&plan).expect("序列化应成功");
        let back: WindowPlan = serde_json::from_str(&json).expect("反序列化应成功");
        assert_eq!(back, plan, "serde 往返逐字段一致（快照回放依据）");
    }
}
