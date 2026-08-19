//! 算子路由器 — OpenMLE Greedy/ThreeFactor/UCB/Cooling 四策略（设计文档 §11.2）
//!
//! 对应架构层: **L6 Router**（faae-router 子模块，用户确认落点 D-2）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §11.2
//! 对应论文: 清华 OpenMLE（算子路由 Greedy / ThreeFactor / UCB / Cooling）
//! 对应 ADR: ADR-049 决策 1（operator-router 落点 faae-router，内嵌模块）
//!
//! # 核心职责
//!
//! 按任务类型与上下文路由到最合适的原子算子（消费 L5 gsoe-evolution
//! 四算子，L6→L5 向下合规）：
//! - **Greedy**: 历史成功均分 argmax（利用导向）
//! - **ThreeFactor**: quality + progress + novelty utility（规范原型）；
//!   若注入 [`ThreeFactorSelector`] 则委托 Softmax 温度采样（D-3 闭环，
//!   补足规范原型缺失的采样能力）
//! - **UCB**: avg_reward + c×√(2ln(N)/n)，未访问算子 MAX 优先（探索导向）
//! - **Cooling**: ε=exp(-cooling_rate×N) epsilon-greedy 退火（探索→利用收敛）
//!
//! # W4 闭环增强（ADR-084）
//!
//! - **增量聚合表**: per-(task_type, operator) 聚合（visits/score_sum/
//!   score_max/success_*），四策略查询 O(K)（K=适用算子数 ≤4），替代
//!   全历史线性扫描;`record_result` O(1) 增量维护
//! - **history 有界化**: VecDeque 滚动窗口（`HISTORY_CAP` = 4096）——
//!   统计语义由聚合表保真（铁律3 张力的显式化解：截断仅影响导出粒度，
//!   不影响选择决策）;proptest 锁定"聚合=全扫"等价
//! - **apply_strategy**: D3 契约（六维控制面）动态热切换
//! - **export_trajectory**: 铁律6——选择历史可导出为 RLTrajectory
//!
//! # 设计约束（铁律）
//!
//! - **铁律3**: 历史记录只追加不修改（`record_result` append-only）
//! - **铁律4**: 四策略评分均为纯函数（无副作用，同输入同输出）
//! - **铁律6**: `export_history` / `export_trajectory` 双形态导出
//! - **L0 消费**: `OperatorSelectionStrategy`（六维控制面 D3 契约，Phase 0 落地）

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use gsoe_evolution::{
    AtomicOperatorTrait, CrossoverOperator, DebugOperator, DraftOperator, ImproveOperator,
    OperatorContext, ThreeFactorSelector,
};
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector, RLTrajectory};
use nexus_contracts::{ExperienceCard, OperatorSelectionStrategy};

/// 选择历史滚动窗口容量（铁律3 张力化解：聚合表保真统计,窗口化仅限导出粒度）
pub const HISTORY_CAP: usize = 4096;

/// §16.3 按需记忆合成提供者 — L6 经依赖倒置调用 L2 合成器（Phase 10 Wave 6）
///
/// WHY trait 而非直接依赖 mlc-engine: L6→L2 直接依赖违反依赖铁律
/// （跨层通信只能走 Event Bus / MCP Mesh）。trait 注入由组合根（L10）
/// 实现桥接（L10 同时依赖 L6 与 L2，向下合规），保持 L6 依赖面最小。
pub trait MemorySynthesizer: Send + Sync {
    /// 为任务与算子按需合成上下文提示
    ///
    /// 返回 `Some(摘要文本)` = 合成成功;`None` = 无可用上下文（诚实降级，
    /// 路由不受影响）。
    fn synthesize_context(&self, task_id: &str, operator: AtomicOperator) -> Option<String>;
}

/// per-(task_type, operator) 增量聚合 — 四策略 O(K) 查询的统计底座
///
/// 与全历史扫描的等价性由 proptest 锁定（Greedy/ThreeFactor/UCB 三策略）。
#[derive(Clone, Debug, Default)]
pub struct OperatorAggregate {
    /// 总访问次数（该 task_type × operator 的记录数）
    pub visits: u32,
    /// 全记录评分总和（quality = score_sum / visits）
    pub score_sum: f32,
    /// 全记录最高分（progress = score_max - quality）
    pub score_max: f32,
    /// Success 状态记录数
    pub success_count: u32,
    /// Success 状态评分总和（Greedy 均值 = success_score_sum / success_count）
    pub success_score_sum: f32,
}

/// 算子选择历史记录 — append-only（铁律3）
#[derive(Clone, Debug)]
pub struct OperatorSelectionRecord {
    /// 任务类型
    pub task_type: String,
    /// 被选中的算子
    pub selected_operator: AtomicOperator,
    /// 执行结果评分
    pub result_score: f32,
    /// 执行状态（六类，L0 契约）
    pub execution_status: ExecutionStatus,
    /// 记录时间戳
    pub timestamp: DateTime<Utc>,
}

/// 算子路由器 — 四策略算子选择 + 历史记录
///
/// `operators` 注册表承载 L5 四算子实例（Draft/Improve/Debug/Crossover），
/// `select_operator` 按策略分派选择，`record_result` 追加历史。
pub struct OperatorRouter {
    /// 算子注册表（L5 四算子，Arc 共享只读——铁律3 算子无状态）
    operators: HashMap<AtomicOperator, std::sync::Arc<dyn AtomicOperatorTrait>>,
    /// 选择策略（消费 L0 六维 D3 契约;W4: apply_strategy 可热切换）
    selection_strategy: OperatorSelectionStrategy,
    /// 选择历史（append-only 滚动窗口，铁律3 + HISTORY_CAP）
    history: VecDeque<OperatorSelectionRecord>,
    /// 增量聚合表（W4: per-(task_type, operator) O(1) 查询底座）
    aggregates: HashMap<(String, AtomicOperator), OperatorAggregate>,
    /// UCB 探索常数（默认 √2）
    ucb_constant: f32,
    /// Cooling 退火率（ε 衰减速度）
    cooling_rate: f32,
    /// 累计选择次数（UCB/Cooling 的 N）
    total_selections: u32,
    /// ThreeFactor Softmax 委托选择器（D-3；None = 规范原型 argmax）
    three_factor_selector: Option<ThreeFactorSelector>,
    /// §16.3 按需合成注入点（Wave 6;None = 未装配,路由不合成上下文）
    synthesizer: Option<std::sync::Arc<dyn MemorySynthesizer>>,
    /// 最近一次合成摘要（可观测性;None = 从未合成或无可合成上下文）
    last_synthesis: Option<String>,
}

/// 四算子规范序 — `select_operator` 适用集的确定性迭代序
///
/// WHY: `operators` 为 HashMap,其迭代序在进程内不稳定——适用集顺序随机
/// 会使"首个未访问算子优先"等平局裁决非确定,违反铁律4（proptest 等价性
/// 验证捕获）。规范序 = 枚举声明序（Draft/Improve/Debug/Crossover）。
const OPERATOR_ORDER: [AtomicOperator; 4] = [
    AtomicOperator::Draft,
    AtomicOperator::Improve,
    AtomicOperator::Debug,
    AtomicOperator::Crossover,
];

impl OperatorRouter {
    /// 创建路由器 — 注册 L5 四算子（消费 L0 策略契约）
    pub fn new(strategy: OperatorSelectionStrategy) -> Self {
        let mut operators: HashMap<AtomicOperator, std::sync::Arc<dyn AtomicOperatorTrait>> =
            HashMap::new();
        operators.insert(AtomicOperator::Draft, std::sync::Arc::new(DraftOperator));
        operators.insert(
            AtomicOperator::Improve,
            std::sync::Arc::new(ImproveOperator),
        );
        operators.insert(AtomicOperator::Debug, std::sync::Arc::new(DebugOperator));
        operators.insert(
            AtomicOperator::Crossover,
            std::sync::Arc::new(CrossoverOperator),
        );
        Self {
            operators,
            selection_strategy: strategy,
            history: VecDeque::new(),
            aggregates: HashMap::new(),
            ucb_constant: 1.414,
            cooling_rate: 0.01,
            total_selections: 0,
            three_factor_selector: None,
            synthesizer: None,
            last_synthesis: None,
        }
    }

    /// W4: D3 契约策略热切换（六维调整器动态下发入口）
    ///
    /// 切换保留全部统计（聚合表/历史/total_selections）——策略是
    /// "如何利用统计"的决策，与统计本体正交。
    pub fn apply_strategy(&mut self, strategy: OperatorSelectionStrategy) {
        self.selection_strategy = strategy;
    }

    /// 注入 ThreeFactor Softmax 委托选择器（D-3 闭环）
    ///
    /// WHY 委托不重复实现: 铁律4 纯函数复用 L5 ThreeFactorSelector 的
    /// UCB + Softmax + 冷却能力；L6 仅做算子历史 → 伪卡片投影。
    pub fn with_three_factor_selector(mut self, selector: ThreeFactorSelector) -> Self {
        self.three_factor_selector = Some(selector);
        self
    }

    /// §16.3 注入按需记忆合成器（Wave 6）— 组合根装配时经 trait 桥接 L2
    ///
    /// 选择完成后调用注入的合成器,将合成上下文摘要记录到
    /// `last_synthesis`（可观测性;不阻塞选择主路径）。
    pub fn with_synthesizer(mut self, synthesizer: std::sync::Arc<dyn MemorySynthesizer>) -> Self {
        self.synthesizer = Some(synthesizer);
        self
    }

    /// 设置 UCB 探索常数（可配置）
    pub fn with_ucb_constant(mut self, c: f32) -> Self {
        self.ucb_constant = c;
        self
    }

    /// 设置 Cooling 退火率（可配置）
    pub fn with_cooling_rate(mut self, rate: f32) -> Self {
        self.cooling_rate = rate;
        self
    }

    /// 选择算子 — is_applicable 过滤 → 四策略分派
    ///
    /// 返回 None 当且仅当无适用算子（空上下文防御）。
    pub fn select_operator(
        &mut self,
        task_type: &str,
        context: &OperatorContext,
    ) -> Option<AtomicOperator> {
        // 适用性过滤（算子自判定,L5 契约）——按 OPERATOR_ORDER 规范序遍历,
        // 保证适用集顺序确定（铁律4: 同输入同输出）
        let applicable: Vec<AtomicOperator> = OPERATOR_ORDER
            .iter()
            .filter(|op| {
                self.operators
                    .get(*op)
                    .map(|operator| operator.is_applicable(context))
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        if applicable.is_empty() {
            return None;
        }
        let selected = match self.selection_strategy {
            OperatorSelectionStrategy::Greedy => self.select_greedy(task_type, &applicable),
            OperatorSelectionStrategy::ThreeFactor => {
                self.select_three_factor(task_type, &applicable)
            }
            OperatorSelectionStrategy::Ucb => self.select_ucb(task_type, &applicable),
            OperatorSelectionStrategy::Cooling => self.select_cooling(task_type, &applicable),
        };
        self.total_selections += 1;
        // §16.3 合成接线(Wave 6):选择后经注入点调用 L2 按需合成(依赖倒置,
        // 不引入 L6→L2 直接依赖);None = 未装配或无可合成上下文,静默跳过。
        if let (Some(synth), Some(op)) = (&self.synthesizer, selected) {
            self.last_synthesis = synth.synthesize_context(task_type, op);
        }
        selected
    }

    /// 记录执行结果 — append-only（铁律3：只追加不修改既有记录）
    ///
    /// W4: 同步维护增量聚合表（O(1)），滚动窗口超出 `HISTORY_CAP`
    /// 时淘汰最旧记录（统计语义由聚合表保真——决策不受窗口影响）。
    pub fn record_result(
        &mut self,
        task_type: &str,
        operator: AtomicOperator,
        score: f32,
        status: ExecutionStatus,
    ) {
        // 聚合表 O(1) 增量维护
        let aggregate = self
            .aggregates
            .entry((task_type.to_string(), operator))
            .or_default();
        aggregate.visits += 1;
        aggregate.score_sum += score;
        aggregate.score_max = aggregate.score_max.max(score);
        if status == ExecutionStatus::Success {
            aggregate.success_count += 1;
            aggregate.success_score_sum += score;
        }
        self.history.push_back(OperatorSelectionRecord {
            task_type: task_type.to_string(),
            selected_operator: operator,
            result_score: score,
            execution_status: status,
            timestamp: Utc::now(),
        });
        if self.history.len() > HISTORY_CAP {
            self.history.pop_front();
        }
    }

    /// 导出选择历史（窗口内）— 铁律6 原始记录形态
    pub fn export_history(&self) -> Vec<OperatorSelectionRecord> {
        self.history.iter().cloned().collect()
    }

    /// 导出选择轨迹 — 铁律6 RLTrajectory 形态（W4 补齐）
    ///
    /// 投影约定:
    /// - state.layer_features[0..4] = [operator_code, score, status_code, visits]
    /// - action = 被选算子（action_code = operator 编码,parameters = [score]）
    /// - reward = result_score（执行结果评分——执行反馈的即时信号）
    /// - timestamps = 记录时刻 Unix 毫秒
    pub fn export_trajectory(&self, episode_id: &str) -> RLTrajectory {
        let states: Vec<RLStateVector> = self
            .history
            .iter()
            .map(|r| {
                let mut state = RLStateVector::zeros();
                state.layer_features[0] = operator_code(r.selected_operator) as f32;
                state.layer_features[1] = r.result_score;
                state.layer_features[2] = status_code(r.execution_status) as f32;
                state.layer_features[3] = self
                    .aggregates
                    .get(&(r.task_type.clone(), r.selected_operator))
                    .map(|a| a.visits)
                    .unwrap_or(0) as f32;
                state
            })
            .collect();
        let actions: Vec<RLActionVector> = self
            .history
            .iter()
            .map(|r| {
                RLActionVector::new(
                    "l6_operator_router",
                    operator_code(r.selected_operator),
                    vec![r.result_score],
                )
            })
            .collect();
        let rewards: Vec<f32> = self.history.iter().map(|r| r.result_score).collect();
        let timestamps: Vec<u64> = self
            .history
            .iter()
            .map(|r| r.timestamp.timestamp_millis().max(0) as u64)
            .collect();
        RLTrajectory::new(episode_id, states, actions, rewards, timestamps)
    }

    /// 聚合表只读访问（W4 可观测性/等价性验证）
    pub fn aggregates(&self) -> &HashMap<(String, AtomicOperator), OperatorAggregate> {
        &self.aggregates
    }

    /// 累计选择次数只读访问（可观测性）
    pub fn total_selections(&self) -> u32 {
        self.total_selections
    }

    /// 当前策略只读访问（可观测性）
    pub fn selection_strategy(&self) -> OperatorSelectionStrategy {
        self.selection_strategy
    }

    /// 最近一次合成摘要只读访问（可观测性;None = 未装配/无可合成上下文）
    pub fn last_synthesis(&self) -> Option<&str> {
        self.last_synthesis.as_deref()
    }

    /// 获取算子实例只读引用（供调用方 execute 执行）
    pub fn get_operator(
        &self,
        op: AtomicOperator,
    ) -> Option<&std::sync::Arc<dyn AtomicOperatorTrait>> {
        self.operators.get(&op)
    }

    // ========================================================
    // 四策略评分（铁律4 纯函数;W4: 聚合表 O(K) 查询）
    // ========================================================

    /// Greedy — 历史成功均分 argmax（利用导向）
    ///
    /// W4: success_score_sum/success_count 聚合查询,无记录时 0.0。
    fn select_greedy(
        &self,
        task_type: &str,
        applicable: &[AtomicOperator],
    ) -> Option<AtomicOperator> {
        let mut best_operator = *applicable.first()?;
        let mut best_score = -1.0;
        for op in applicable {
            let score = self
                .aggregates
                .get(&(task_type.to_string(), *op))
                .filter(|a| a.success_count > 0)
                .map(|a| a.success_score_sum / a.success_count as f32)
                .unwrap_or(0.0);
            if score > best_score {
                best_score = score;
                best_operator = *op;
            }
        }
        Some(best_operator)
    }

    /// ThreeFactor — quality + progress + novelty utility（规范原型 argmax；
    /// 注入 selector 时委托 Softmax 采样，D-3）
    ///
    /// WHY &mut self: 委托路径需 ThreeFactorSelector 的 &mut（采样更新 visit_counts）。
    fn select_three_factor(
        &mut self,
        task_type: &str,
        applicable: &[AtomicOperator],
    ) -> Option<AtomicOperator> {
        // D-3 闭环: 注入 selector → 聚合投影伪卡片 → Softmax 采样
        //
        // WHY 自由函数委托: self.three_factor_selector 的 &mut 与 self.aggregates 的
        // & 同时借用会触发 E0502，拆为自由函数分别传递两个借用。
        if let Some(selector) = self.three_factor_selector.as_mut() {
            return softmax_delegate(selector, &self.aggregates, task_type, applicable);
        }
        // 规范原型: utility argmax（未访问算子优先探索）
        let mut best_operator = *applicable.first()?;
        let mut best_utility = -1.0;
        for op in applicable {
            let Some(aggregate) = self.aggregates.get(&(task_type.to_string(), *op)) else {
                return Some(*op); // 未访问算子优先（探索）
            };
            let quality = aggregate.score_sum / aggregate.visits.max(1) as f32;
            let progress = aggregate.score_max - quality;
            let novelty = 1.0 / (aggregate.visits as f32 + 1.0);
            let utility = quality + progress + novelty;
            if utility > best_utility {
                best_utility = utility;
                best_operator = *op;
            }
        }
        Some(best_operator)
    }

    /// UCB — avg_reward + c×√(2ln(N)/n)，未访问 MAX 优先（探索导向）
    fn select_ucb(&self, task_type: &str, applicable: &[AtomicOperator]) -> Option<AtomicOperator> {
        let mut best_operator = *applicable.first()?;
        let mut best_score = -f32::MAX;
        for op in applicable {
            let ucb = match self.aggregates.get(&(task_type.to_string(), *op)) {
                Some(a) if a.visits > 0 && self.total_selections > 0 => {
                    let avg_reward = a.score_sum / a.visits as f32;
                    avg_reward
                        + self.ucb_constant
                            * ((2.0 * (self.total_selections as f32).ln()) / a.visits as f32).sqrt()
                }
                _ => f32::MAX, // 未访问算子优先
            };
            if ucb > best_score {
                best_score = ucb;
                best_operator = *op;
            }
        }
        Some(best_operator)
    }

    /// Cooling — ε=exp(-cooling_rate×N) epsilon-greedy 退火
    fn select_cooling(
        &self,
        task_type: &str,
        applicable: &[AtomicOperator],
    ) -> Option<AtomicOperator> {
        let epsilon = (-self.cooling_rate * self.total_selections as f32).exp();
        let mut rng = rand::thread_rng();
        if rand::Rng::gen::<f32>(&mut rng) < epsilon {
            // 探索: 随机选择
            let idx = rand::Rng::gen_range(&mut rng, 0..applicable.len());
            return Some(applicable[idx]);
        }
        // 利用: 退火到 Greedy
        self.select_greedy(task_type, applicable)
    }
}

/// ThreeFactor Softmax 委托 — 聚合表投影伪卡片 → L5 选择器采样（D-3）
///
/// WHY 自由函数: 避免 self.three_factor_selector 的 &mut 与 self.aggregates 的
/// & 同时借用冲突（E0502）；投影逻辑为纯函数（铁律4）。
fn softmax_delegate(
    selector: &mut ThreeFactorSelector,
    aggregates: &HashMap<(String, AtomicOperator), OperatorAggregate>,
    task_type: &str,
    applicable: &[AtomicOperator],
) -> Option<AtomicOperator> {
    // 算子 → 伪 ExperienceCard 投影（node_id 编码算子名，三因子取聚合统计）
    let pseudo_cards: Vec<ExperienceCard> = applicable
        .iter()
        .map(|op| {
            let aggregate = aggregates
                .get(&(task_type.to_string(), *op))
                .cloned()
                .unwrap_or_default();
            let quality = if aggregate.visits == 0 {
                0.0
            } else {
                aggregate.score_sum / aggregate.visits as f32
            };
            let progress = if aggregate.visits == 0 {
                0.0
            } else {
                aggregate.score_max - quality
            };
            let novelty = 1.0 / (aggregate.visits as f32 + 1.0);
            ExperienceCard {
                card_id: format!("pseudo-{op:?}").into(),
                task_id: task_type.into(),
                node_id: format!("op-{op:?}").into(),
                parent_id: None,
                created_at: Utc::now(),
                operator: *op,
                score: quality,
                delta_vs_parent: progress,
                method_family: "operator-routing".into(),
                error_signature: None,
                three_factor: ThreeFactorScore {
                    quality,
                    progress,
                    novelty,
                },
                execution_status: ExecutionStatus::Success,
                token_evidence_ids: Vec::new(),
                segment_id: None,
                metadata: CardMetadata::default(),
            }
        })
        .collect();
    // 委托 L5 选择器（UCB + Softmax + 冷却，采样更新其内部 visit_counts）
    let selected_card = selector.select(&pseudo_cards)?;
    // 伪卡片 node_id 反查算子
    applicable
        .iter()
        .find(|op| format!("op-{op:?}") == selected_card.node_id.as_ref())
        .copied()
}

/// 算子 → 离散编码（RLTrajectory 投影: Draft=0/Improve=1/Debug=2/Crossover=3）
fn operator_code(op: AtomicOperator) -> u32 {
    match op {
        AtomicOperator::Draft => 0,
        AtomicOperator::Improve => 1,
        AtomicOperator::Debug => 2,
        AtomicOperator::Crossover => 3,
    }
}

/// 执行状态 → 离散编码（RLTrajectory 投影: 六类铁律8 全链路追踪）
fn status_code(status: ExecutionStatus) -> u32 {
    match status {
        ExecutionStatus::Success => 0,
        ExecutionStatus::Error => 1,
        ExecutionStatus::MissingCode => 2,
        ExecutionStatus::NoSubmit => 3,
        ExecutionStatus::ScoreFailed => 4,
        ExecutionStatus::Timeout => 5,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn context(requirements: &str) -> OperatorContext {
        OperatorContext {
            task_id: "t-1".to_string(),
            task_type: "code_gen".to_string(),
            parent_card: None,
            error_signature: None,
            requirements: requirements.to_string(),
            code: None,
            card_query: None,
        }
    }

    #[test]
    fn new_registers_all_four_operators() {
        let router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
        assert!(router.get_operator(AtomicOperator::Draft).is_some());
        assert!(router.get_operator(AtomicOperator::Improve).is_some());
        assert!(router.get_operator(AtomicOperator::Debug).is_some());
        assert!(router.get_operator(AtomicOperator::Crossover).is_some());
    }

    #[test]
    fn select_operator_returns_applicable() {
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
        // Draft 无父卡片适用；空历史 Greedy 返回首个适用算子
        let selected = router.select_operator("code_gen", &context("build parser"));
        assert!(selected.is_some());
        assert_eq!(router.total_selections(), 1);
    }

    #[test]
    fn greedy_prefers_high_success_average() {
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
        // Draft 高分 / Improve 低分
        router.record_result("t", AtomicOperator::Draft, 0.9, ExecutionStatus::Success);
        router.record_result("t", AtomicOperator::Improve, 0.3, ExecutionStatus::Success);
        let selected = router.select_greedy("t", &[AtomicOperator::Draft, AtomicOperator::Improve]);
        assert_eq!(selected, Some(AtomicOperator::Draft));
    }

    #[test]
    fn three_factor_unvisited_operator_priority() {
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::ThreeFactor);
        router.record_result("t", AtomicOperator::Draft, 0.9, ExecutionStatus::Success);
        // Improve 未访问 → 优先返回（探索）
        let selected = {
            let applicable = [AtomicOperator::Draft, AtomicOperator::Improve];
            router.select_three_factor("t", &applicable)
        };
        assert_eq!(selected, Some(AtomicOperator::Improve));
    }

    #[test]
    fn ucb_unvisited_operator_max_priority() {
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::Ucb);
        router.record_result("t", AtomicOperator::Draft, 0.9, ExecutionStatus::Success);
        router.total_selections = 1;
        let selected = router.select_ucb("t", &[AtomicOperator::Draft, AtomicOperator::Improve]);
        assert_eq!(
            selected,
            Some(AtomicOperator::Improve),
            "未访问算子 UCB=MAX 优先"
        );
    }

    #[test]
    fn cooling_high_rate_explores_eventually_exploits() {
        let mut router =
            OperatorRouter::new(OperatorSelectionStrategy::Cooling).with_cooling_rate(0.5);
        router.record_result("t", AtomicOperator::Draft, 0.9, ExecutionStatus::Success);
        // 大量选择后 ε→0，退火到 Greedy（Draft 高分主导）
        router.total_selections = 100;
        let mut draft_count = 0;
        for _ in 0..50 {
            if router.select_cooling("t", &[AtomicOperator::Draft, AtomicOperator::Improve])
                == Some(AtomicOperator::Draft)
            {
                draft_count += 1;
            }
        }
        assert!(
            draft_count > 40,
            "退火后应主导利用高分算子（实际 {draft_count}/50）"
        );
    }

    #[test]
    fn record_result_append_only() {
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
        router.record_result("t", AtomicOperator::Draft, 0.5, ExecutionStatus::Success);
        router.record_result("t", AtomicOperator::Draft, 0.8, ExecutionStatus::Success);
        let history = router.export_history();
        // 铁律3: 既有记录不被修改（两条都在，分数保持）
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].result_score, 0.5);
        assert_eq!(history[1].result_score, 0.8);
    }

    #[test]
    fn three_factor_softmax_delegation_returns_valid_operator() {
        let selector = ThreeFactorSelector::new(1.414, 0.1, 1.0);
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::ThreeFactor)
            .with_three_factor_selector(selector);
        // 默认上下文 Draft/Crossover 均适用（is_applicable 恒 true），
        // Improve/Debug 不适用（无父卡片/错误签名）；
        // 多次选择验证 Softmax 委托路径不 panic 且只返回适用算子（铁律4）
        for _ in 0..20 {
            let selected = router.select_operator("t", &context("build parser"));
            assert!(matches!(
                selected,
                Some(AtomicOperator::Draft) | Some(AtomicOperator::Crossover)
            ));
        }
    }

    /// §16.3 合成接线(Wave 6):未注入合成器时 last_synthesis 为 None
    #[test]
    fn without_synthesizer_no_synthesis_recorded() {
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
        let selected = router.select_operator("code_gen", &context("build parser"));
        assert!(selected.is_some(), "无合成器不影响路由主路径");
        assert_eq!(router.last_synthesis(), None, "未注入合成器不合成");
    }

    /// §16.3 合成接线(Wave 6):注入合成器后选择即调用,last_synthesis 记录摘要
    #[test]
    fn with_synthesizer_invoked_after_selection() {
        // 测试合成器:固定返回摘要文本(验证调用链成立)
        struct FakeSynthesizer;
        impl MemorySynthesizer for FakeSynthesizer {
            fn synthesize_context(
                &self,
                task_id: &str,
                operator: AtomicOperator,
            ) -> Option<String> {
                Some(format!("synth[{task_id}:{operator:?}]"))
            }
        }
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy)
            .with_synthesizer(std::sync::Arc::new(FakeSynthesizer));
        let selected = router.select_operator("code_gen", &context("build parser"));
        assert!(selected.is_some());
        let synthesis = router.last_synthesis().expect("选择后应调用合成器");
        assert!(
            synthesis.starts_with("synth[code_gen:"),
            "摘要应含任务与算子(实际 {synthesis})"
        );
    }
}
