//! 经验卡片系统 — OpenMLE 案例级卡片 + 全局经验板 + 三因子父本选择（设计文档 §7.1）
//!
//! 对应架构层: **L2 Memory**（mlc-engine 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §7.1
//! 对应论文: 清华 Frontis-MA1 OpenMLE（经验卡片 + 三因子评估 + UCB/冷却选择）
//! 对应 ADR: ADR-049 决策 1（experience-card-system 落点 mlc-engine，内嵌模块）
//!
//! # 核心职责
//!
//! 消费 L0 [`ExperienceCard`] 契约（Phase 0 落地）与 L1 `ExperienceCardBus`
//! 卡片流（Phase 1 落地），提供：
//! - **案例级卡片存储**: `case_cards` + `node_index`（node_id → 下标 O(1) 检索）
//! - **全局经验板**: [`GlobalExperienceBoard`]（总数/已评估/最佳分/均分/方法分布/错误聚类）
//! - **方法家族统计**: [`MethodStatistics`]（count/avg/best/success_rate）
//! - **三因子父本选择**: [`ExperienceCardSystem::select_parent`]（归一化 + UCB + 冷却）
//!
//! # 设计约束（铁律）
//!
//! - **铁律3（不可变）**: 只读消费 L0 卡片，不提供卡片变更方法；版本化由 L0 保证
//! - **铁律4（三因子纯函数）**: `select_parent` 的效用计算为纯函数（输入确定则输出确定）
//! - **红线 R8（Top-K O(n)）**: `select_parent` 选单一父本用 `max_by`（O(n)），
//!   禁止 `sort_by`（O(n log n)）
//! - **f32 红线**: GlobalExperienceBoard/MethodStatistics 含 f32，仅 derive PartialEq
//! - **Box<str> 零拷贝索引**: node_index/method_stats key 用 Box<str>（与 L0 对齐）

use std::collections::HashMap;

use nexus_contracts::experience_card::{ErrorSignature, ExecutionStatus};
use nexus_contracts::ExperienceCard;

// ============================================================
// 全局经验板
// ============================================================

/// 全局经验板 — 搜索树全局统计 + 错误聚类（OpenMLE）
///
/// 跨任务的经验总览，供 L10 经验卡片可视化与 L8 三因子裁决消费。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlobalExperienceBoard {
    /// 累计节点总数
    pub total_nodes: u64,
    /// 已评估节点数（ExecutionStatus::Success 计数）
    pub total_evaluated: u64,
    /// 历史最佳分
    pub best_score: f32,
    /// 平均分
    pub average_score: f32,
    /// 方法家族分布（method_family → 计数）
    pub method_distribution: HashMap<Box<str>, u32>,
    /// 错误聚类（error_type → 错误签名列表）
    pub error_clusters: HashMap<Box<str>, Vec<ErrorSignature>>,
    /// 高频错误（error_type → 出现次数，按频率降序由消费方排序）
    pub frequent_errors: Vec<(Box<str>, u32)>,
}

// ============================================================
// 方法家族统计
// ============================================================

/// 方法家族统计 — 单一方法家族的经验聚合
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MethodStatistics {
    /// 该家族卡片数
    pub count: u32,
    /// 累计分
    pub total_score: f32,
    /// 平均分
    pub avg_score: f32,
    /// 最佳分
    pub best_score: f32,
    /// 成功率（Success 卡片占比）
    pub success_rate: f32,
}

// ============================================================
// 经验卡片系统
// ============================================================

/// 经验卡片系统 — OpenMLE 核心（案例级存储 + 全局板 + 三因子父本选择）
#[derive(Debug, Default)]
pub struct ExperienceCardSystem {
    /// 案例级卡片（append-only，铁律3 只读消费）
    case_cards: Vec<ExperienceCard>,
    /// 全局经验板
    global_board: GlobalExperienceBoard,
    /// 方法家族统计（method_family → 统计）
    method_stats: HashMap<Box<str>, MethodStatistics>,
    /// 节点索引（node_id → case_cards 下标）
    node_index: HashMap<Box<str>, usize>,
    /// 访问计数（node_id → 被选为父本次数，UCB 用）
    visit_counts: HashMap<Box<str>, u32>,
    /// 探索权重（UCB bonus 系数）
    exploration_weight: f32,
    /// 冷却系数（随总访问数增长，抑制过度选择）
    cooling_coefficient: f32,
}

impl ExperienceCardSystem {
    /// 创建经验卡片系统
    ///
    /// - `exploration_weight`: UCB 探索权重（常见 √2 ≈ 1.414）
    /// - `cooling_coefficient`: 冷却系数（常见 0.1-0.3）
    pub fn new(exploration_weight: f32, cooling_coefficient: f32) -> Self {
        Self {
            case_cards: Vec::new(),
            global_board: GlobalExperienceBoard::default(),
            method_stats: HashMap::new(),
            node_index: HashMap::new(),
            visit_counts: HashMap::new(),
            exploration_weight,
            cooling_coefficient,
        }
    }

    /// 卡片总数
    pub fn card_count(&self) -> usize {
        self.case_cards.len()
    }

    /// 全局经验板只读访问
    pub fn global_board(&self) -> &GlobalExperienceBoard {
        &self.global_board
    }

    /// 方法家族统计只读访问
    pub fn method_stats(&self) -> &HashMap<Box<str>, MethodStatistics> {
        &self.method_stats
    }

    /// 按节点 ID 检索卡片（O(1) 索引）
    pub fn get_card_by_node(&self, node_id: &str) -> Option<&ExperienceCard> {
        self.node_index
            .get(node_id)
            .and_then(|&idx| self.case_cards.get(idx))
    }

    /// 全部卡片只读访问（按需合成的祖先/兄弟遍历用，铁律3 只读）
    pub fn cards(&self) -> &[ExperienceCard] {
        &self.case_cards
    }

    /// 添加卡片 — 四索引更新 + 方法家族统计 + 错误聚类 + 全局板维护
    ///
    /// 铁律3: 仅追加存储，不修改卡片内容（卡片不可变由 L0 保证）。
    pub fn add_card(&mut self, card: ExperienceCard) {
        let idx = self.case_cards.len();
        self.node_index.insert(card.node_id.clone(), idx);

        // 全局板维护
        self.global_board.total_nodes += 1;
        if card.execution_status == ExecutionStatus::Success {
            self.global_board.total_evaluated += 1;
        }
        if card.score > self.global_board.best_score {
            self.global_board.best_score = card.score;
        }

        // 方法家族统计
        let stats = self
            .method_stats
            .entry(card.method_family.clone())
            .or_default();
        stats.count += 1;
        stats.total_score += card.score;
        stats.avg_score = stats.total_score / stats.count as f32;
        if card.score > stats.best_score {
            stats.best_score = card.score;
        }
        // 成功率: 该家族 Success 卡片占比（增量计算需遍历，规模可控）
        let success_count = self
            .case_cards
            .iter()
            .filter(|c| {
                c.method_family == card.method_family
                    && c.execution_status == ExecutionStatus::Success
            })
            .count() as u32
            + if card.execution_status == ExecutionStatus::Success {
                1
            } else {
                0
            };
        stats.success_rate = success_count as f32 / stats.count as f32;

        // 方法分布 + 错误聚类
        *self
            .global_board
            .method_distribution
            .entry(card.method_family.clone())
            .or_insert(0) += 1;
        if let Some(ref sig) = card.error_signature {
            self.global_board
                .error_clusters
                .entry(sig.error_type.clone())
                .or_default()
                .push(sig.clone());
        }

        self.case_cards.push(card);

        // 平均分增量更新（避免每次全量求和）
        self.global_board.average_score = self.global_board.average_score.mul_add(
            (self.case_cards.len() - 1) as f32,
            self.case_cards.last().map(|c| c.score).unwrap_or(0.0),
        ) / self.case_cards.len() as f32;
    }

    /// 三因子父本选择 — OpenMLE 核心算法（铁律4 纯函数效用计算）
    ///
    /// 效用 = 归一化三因子和 + UCB 探索项 × exploration_weight − 冷却项。
    /// 选单一父本用 `max_by`（O(n)，红线 R8）；选中后访问计数 +1。
    ///
    /// 返回 `None` 当 candidates 为空。
    pub fn select_parent<'a>(
        &mut self,
        candidates: &[&'a ExperienceCard],
    ) -> Option<&'a ExperienceCard> {
        if candidates.is_empty() {
            return None;
        }
        // 三因子归一化基准（各维度最大值，防除零 max(1e-8)）
        let max_quality = candidates
            .iter()
            .map(|c| c.three_factor.quality)
            .fold(0.0f32, f32::max)
            .max(1e-8);
        let max_progress = candidates
            .iter()
            .map(|c| c.three_factor.progress.abs())
            .fold(0.0f32, f32::max)
            .max(1e-8);
        let max_novelty = candidates
            .iter()
            .map(|c| c.three_factor.novelty)
            .fold(0.0f32, f32::max)
            .max(1e-8);

        // 计算各候选效用（纯函数，铁律4）
        let mut best: Option<(&ExperienceCard, f32)> = None;
        for card in candidates {
            let normalized = card
                .three_factor
                .normalize(max_quality, max_progress, max_novelty);
            let ucb_bonus = self.ucb_bonus(&card.node_id);
            let cooling = self.cooling_factor();
            let utility = normalized.quality
                + normalized.progress
                + normalized.novelty
                + ucb_bonus * self.exploration_weight
                - cooling;
            // UCB bonus 为 MAX 时（未访问节点）必然优先——直接短路
            if ucb_bonus == f32::MAX {
                best = Some((card, f32::MAX));
                break;
            }
            match best {
                Some((_, best_utility)) if utility <= best_utility => {}
                _ => best = Some((card, utility)),
            }
        }

        let selected = best.map(|(c, _)| c);
        if let Some(card) = selected {
            *self.visit_counts.entry(card.node_id.clone()).or_insert(0) += 1;
        }
        selected
    }

    /// UCB 探索项 — 未访问节点返回 MAX（必选），否则 √(2·ln(N)/n)
    fn ucb_bonus(&self, node_id: &str) -> f32 {
        let visits = self.visit_counts.get(node_id).copied().unwrap_or(0);
        if visits == 0 {
            return f32::MAX;
        }
        let total_visits: u32 = self.visit_counts.values().sum();
        if total_visits == 0 {
            return 0.0;
        }
        (2.0 * (total_visits as f32).ln() / visits as f32).sqrt()
    }

    /// 冷却项 — cooling_coefficient × ln(总访问数)，随探索推进抑制过度选择
    fn cooling_factor(&self) -> f32 {
        let total_visits: u32 = self.visit_counts.values().sum();
        if total_visits == 0 {
            return 0.0;
        }
        self.cooling_coefficient * (total_visits as f32).ln().max(0.0)
    }

    /// 访问计数只读访问（测试/可观测性）
    pub fn visit_count(&self, node_id: &str) -> u32 {
        self.visit_counts.get(node_id).copied().unwrap_or(0)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use nexus_contracts::experience_card::{AtomicOperator, CardMetadata};
    use nexus_contracts::ThreeFactorScore;

    /// 构造样例卡片
    fn card(node: &str, method: &str, score: f32, status: ExecutionStatus) -> ExperienceCard {
        ExperienceCard {
            card_id: Box::from(format!("card-{node}")),
            task_id: Box::from("t1"),
            node_id: Box::from(node),
            parent_id: None,
            created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("合法时间戳"),
            operator: AtomicOperator::Draft,
            score,
            delta_vs_parent: 0.0,
            method_family: Box::from(method),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality: score,
                progress: 0.1,
                novelty: 0.5,
            },
            execution_status: status,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }

    #[test]
    fn add_card_updates_indexes_and_board() {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        system.add_card(card("n1", "draft_pipeline", 0.8, ExecutionStatus::Success));
        system.add_card(card("n2", "draft_pipeline", 0.6, ExecutionStatus::Error));

        assert_eq!(system.card_count(), 2);
        assert_eq!(system.global_board().total_nodes, 2);
        assert_eq!(system.global_board().total_evaluated, 1);
        assert!((system.global_board().best_score - 0.8).abs() < 1e-6);
        // 节点索引 O(1) 检索
        assert_eq!(system.get_card_by_node("n1").expect("存在").score, 0.8);
        assert!(system.get_card_by_node("missing").is_none());
    }

    #[test]
    fn method_stats_aggregation() {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        system.add_card(card("n1", "fam_a", 0.8, ExecutionStatus::Success));
        system.add_card(card("n2", "fam_a", 0.6, ExecutionStatus::Success));
        system.add_card(card("n3", "fam_b", 0.4, ExecutionStatus::Error));

        let stats = system.method_stats();
        let fam_a = stats.get("fam_a").expect("fam_a 存在");
        assert_eq!(fam_a.count, 2);
        assert!((fam_a.avg_score - 0.7).abs() < 1e-6);
        assert!((fam_a.best_score - 0.8).abs() < 1e-6);
        assert!((fam_a.success_rate - 1.0).abs() < 1e-6);
        let fam_b = stats.get("fam_b").expect("fam_b 存在");
        assert_eq!(fam_b.count, 1);
        assert!((fam_b.success_rate - 0.0).abs() < 1e-6);
        // 方法分布
        assert_eq!(
            system.global_board().method_distribution.get("fam_a"),
            Some(&2)
        );
    }

    #[test]
    fn error_clustering_by_type() {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        let mut c = card("n1", "fam_a", 0.3, ExecutionStatus::Error);
        c.error_signature = Some(ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/lib.rs:1"),
            error_summary: Box::from("E0308"),
            error_hash: Box::from("abc123"),
        });
        system.add_card(c);
        let clusters = &system.global_board().error_clusters;
        assert_eq!(clusters.get("compile_error").map(Vec::len), Some(1));
    }

    #[test]
    fn select_parent_prefers_unvisited_ucb() {
        // 未访问节点 UCB bonus = MAX → 必选（全覆盖语义）
        let mut system = ExperienceCardSystem::new(1.414, 0.0);
        let c1 = card("n1", "fam_a", 0.9, ExecutionStatus::Success);
        let c2 = card("n2", "fam_a", 0.5, ExecutionStatus::Success);
        // 先让 n1 被访问过
        let _ = system.select_parent(&[&c1]);
        assert_eq!(system.visit_count("n1"), 1);
        // 再选：n2 未访问（MAX）应优先于 n1
        let selected = system.select_parent(&[&c1, &c2]).expect("非空");
        assert_eq!(selected.node_id.as_ref(), "n2");
        assert_eq!(system.visit_count("n2"), 1);
    }

    #[test]
    fn select_parent_three_factor_ordering() {
        // 两节点都访问过后，按三因子效用排序（高分优先）
        let mut system = ExperienceCardSystem::new(0.0, 0.0);
        let mut c1 = card("n1", "fam_a", 0.9, ExecutionStatus::Success);
        let mut c2 = card("n2", "fam_a", 0.3, ExecutionStatus::Success);
        // 提升 c1 三因子
        c1.three_factor = ThreeFactorScore {
            quality: 0.9,
            progress: 0.5,
            novelty: 0.8,
        };
        c2.three_factor = ThreeFactorScore {
            quality: 0.3,
            progress: 0.1,
            novelty: 0.2,
        };
        // 先各访问一次消除 MAX
        let _ = system.select_parent(&[&c1]);
        let _ = system.select_parent(&[&c2]);
        let selected = system.select_parent(&[&c1, &c2]).expect("非空");
        assert_eq!(selected.node_id.as_ref(), "n1", "三因子高效用节点应优先");
    }

    #[test]
    fn select_parent_empty_returns_none() {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        assert!(system.select_parent(&[]).is_none());
    }

    #[test]
    fn board_average_score_incremental() {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        system.add_card(card("n1", "fam_a", 0.6, ExecutionStatus::Success));
        system.add_card(card("n2", "fam_a", 0.8, ExecutionStatus::Success));
        // 增量平均: (0.6 + 0.8) / 2 = 0.7
        assert!(
            (system.global_board().average_score - 0.7).abs() < 1e-6,
            "增量平均应为 0.7（实际 {})",
            system.global_board().average_score
        );
    }

    #[test]
    fn cards_are_immutable_consumption() {
        // 铁律3: add_card 只读消费，卡片内容不被系统修改
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        let original = card("n1", "fam_a", 0.75, ExecutionStatus::Success);
        let score_before = original.score;
        system.add_card(original.clone());
        let stored = system.get_card_by_node("n1").expect("存在");
        assert_eq!(stored.score, score_before, "系统不得修改卡片分数");
        assert_eq!(stored.method_family.as_ref(), "fam_a");
    }
}
