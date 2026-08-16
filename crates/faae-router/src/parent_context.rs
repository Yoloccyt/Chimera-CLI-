//! 三因子父本选择 L6 消费适配器 — §11.4 边界裁决落点（W4，ADR-084 决策 6）
//!
//! 对应架构层: **L6 Router**（faae-router 子模块）
//! 对应设计源: 规范 §11.4 + §16.4——规范 §4.2 规划的 `parent-selector`
//! crate 与 §16.4"ParentSelected 由 L5 产生、L6 消费"**自相矛盾**;
//! ADR-084 裁决: **L5 `gsoe_evolution::ThreeFactorSelector` 是唯一父本选择
//! 实现**（UCB bonus + 冷却 + Softmax 采样），L6 仅做消费适配——
//! 本模块把"查卡片 + 选父本"组装为可直接注入 `OperatorContext.parent_card`
//! 的上下文（§16.3: 父本 error_signature 指引 Debug 算子路由）。
//!
//! # 统一策略配置
//!
//! 选择策略 = L0 D3 契约 `OperatorSelectionStrategy`（算子路由层）+
//! L5 选择器参数（exploration_weight / cooling_coefficient / temperature，
//! 经 `ThreeFactorSelector::new` 注入）——两层配置各司其职，不经 L6 透传。
//!
//! # 设计约束
//!
//! - **诚实降级**: 候选卡片数 < `min_candidates` 时返回 None（统计不足
//!   不强选,调用方走无父本路径 Draft/Crossover）
//! - **依赖方向**: faae→gsoe-evolution（L6→L5 向下合规,D-3 先例）+
//!   faae→event-bus（L1 卡片总线索引查询）

use event_bus::ExperienceCardBus;
use gsoe_evolution::ThreeFactorSelector;
use nexus_contracts::experience_card::{AtomicOperator, ErrorSignature};
use nexus_contracts::ExperienceCard;

/// 父本选择结果 — L6 消费形态（§16.4 ParentSelected 的进程内等价物）
#[derive(Clone, Debug)]
pub struct ParentSelection {
    /// 选中的父本卡片 ID
    pub parent_card_id: String,
    /// 父本产生的算子（L7 据此决定延续/切换）
    pub parent_operator: AtomicOperator,
    /// 父本的错误签名（None = 无错误;Some → Debug 算子的关键路由信号,§16.3）
    pub error_signature: Option<ErrorSignature>,
    /// 父本评分
    pub score: f32,
    /// 候选池规模（可观测性）
    pub candidate_count: usize,
}

/// 父本上下文提供者 — L5 选择器 + L1 卡片总线的消费适配
///
/// 持有 `ThreeFactorSelector`（其 visit_counts 随选择演化——内部状态
/// 属选择统计，跨任务复用）。
pub struct ParentContextProvider {
    /// L5 三因子选择器（唯一父本选择实现,§11.4 边界裁决）
    selector: ThreeFactorSelector,
    /// 最小候选数（低于此数诚实降级返回 None）
    min_candidates: usize,
}

impl ParentContextProvider {
    /// 创建提供者（注入 L5 选择器,最小候选数默认 1）
    pub fn new(selector: ThreeFactorSelector) -> Self {
        Self {
            selector,
            min_candidates: 1,
        }
    }

    /// 设置最小候选数（诚实降级阈值）
    pub fn with_min_candidates(mut self, min: usize) -> Self {
        self.min_candidates = min;
        self
    }

    /// 为任务选择父本（§16.4: L5 产生、L6 消费）
    ///
    /// 流程: 卡片总线按任务索引取候选 → 候选数守恒校验（诚实降级）→
    /// L5 ThreeFactorSelector 三因子 Softmax 采样 → 投影为
    /// [`ParentSelection`]（供 `OperatorContext.parent_card` 注入）。
    pub fn select_parent(
        &mut self,
        card_bus: &ExperienceCardBus,
        task_id: &str,
    ) -> Option<ParentSelection> {
        let candidates = card_bus.get_cards_by_task(task_id);
        if candidates.len() < self.min_candidates.max(1) {
            return None; // 候选不足: 诚实降级（不强选,调用方走无父本路径）
        }
        let candidate_count = candidates.len();
        let parent: ExperienceCard = self.selector.select(&candidates)?;
        Some(ParentSelection {
            parent_card_id: parent.card_id.to_string(),
            parent_operator: parent.operator,
            error_signature: parent.error_signature.clone(),
            score: parent.score,
            candidate_count,
        })
    }

    /// 最小候选数只读访问（可观测性）
    pub fn min_candidates(&self) -> usize {
        self.min_candidates
    }
}
