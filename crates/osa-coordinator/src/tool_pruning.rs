//! 工具 Schema 动态裁剪 — Dressage 使用频率评分（设计文档 §11.5）
//!
//! 对应架构层: **L6 Router**（osa-coordinator 子模块，规范原路径）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §11.3
//! 对应论文: 微软 Dressage（33 个工具→4 个，13.5K→1.7K tokens 实证）
//! 对应 ADR: ADR-049 决策 1（内嵌 osa-coordinator）+ ADR-084 决策（W1 闭环接线）
//!
//! # 核心职责
//!
//! 基于使用频率/成功率/新近度动态裁剪工具 schema，压缩上下文 token 占用：
//! - `analyze_trajectories`: 轨迹分析累积工具使用统计
//! - `record_tool_step`: 在线单步记录（免批量轨迹，供 TokenLedger 实时反馈）
//! - `prune_tools`: 评分 = 频率×0.4 + 成功率×0.4 + 新近度×0.2 → Top-K 保留
//! - `prune_trajectories_from_ledger`: L1 TokenLedger 真实数据源适配器
//!
//! # 设计约束
//!
//! - **红线 R8**: Top-K 用 `select_nth_unstable_by`（O(n)，非全排序）
//! - **白名单钉住（§18.3 熔断）**: 命名白名单工具无条件保留且不占 Top-K
//!   配额——过度裁剪时必要工具（如审批/沙箱关键工具）免疫，自动恢复路径
//! - **min_tools 下限保护**: 裁剪后总保留数 ≥ max(keep_count, min_tools)
//! - **pruning_threshold 门控**: score ≥ threshold 的工具无条件保留
//! - **铁律6**: 每次裁剪决策追加 `decision_log`，可导出为 RLTrajectory
//! - **自足类型**: `PruneTrajectory` / `PruneToolSchema` 轻量定义，
//!   与 L7 真实轨迹的适配由消费方映射（文档如实声明适配边界）

use std::collections::HashMap;

use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector, RLTrajectory};
use nexus_contracts::token_evidence::{TokenLedgerEntry, ToolCallRecord};

/// 工具使用统计 — 轨迹分析累积结果
#[derive(Clone, Debug)]
pub struct ToolUsageStats {
    /// 工具名称
    pub tool_name: String,
    /// 调用次数
    pub call_count: u32,
    /// 成功次数
    pub success_count: u32,
    /// 累计消耗 token 数
    pub total_tokens_consumed: u32,
    /// 最近使用时间（单调时钟序号，analyze 调用递增）
    pub last_used: u64,
}

/// 裁剪轨迹步 — 轻量自足类型（L7 真实轨迹由消费方映射）
#[derive(Clone, Debug)]
pub struct PruneStep {
    /// 调用的工具名（None = 非工具调用步）
    pub tool_name: Option<String>,
    /// 该步是否成功
    pub success: bool,
}

/// 裁剪轨迹 — 步序列（轻量自足类型）
#[derive(Clone, Debug)]
pub struct PruneTrajectory {
    /// 轨迹步序列
    pub steps: Vec<PruneStep>,
}

/// 工具 schema — 裁剪输入（名称 + schema token 占用）
#[derive(Clone, Debug)]
pub struct PruneToolSchema {
    /// 工具名称
    pub name: String,
    /// schema 的 token 占用（裁剪收益估算用）
    pub schema_tokens: u32,
}

/// 裁剪结果 — 保留的工具与 token 收益
#[derive(Clone, Debug)]
pub struct PruneResult {
    /// 保留的工具 schema
    pub kept: Vec<PruneToolSchema>,
    /// 裁剪掉的工具数
    pub pruned_count: usize,
    /// 节省的 schema token 数
    pub tokens_saved: u32,
}

/// 裁剪决策日志条目 — append-only（铁律6：可导出为 RLTrajectory）
#[derive(Clone, Debug)]
pub struct PruneDecision {
    /// 决策序号（进程内单调递增）
    pub seq: u64,
    /// 本轮可用工具数
    pub available_count: usize,
    /// 本轮白名单钉住数
    pub pinned_count: usize,
    /// 本轮保留工具数
    pub kept_count: usize,
    /// 本轮裁剪工具数
    pub pruned_count: usize,
    /// 本轮节省 token 数
    pub tokens_saved: u32,
    /// 本轮 keep_count 参数
    pub keep_count_param: usize,
    /// 决策时间戳（Unix 毫秒）
    pub timestamp_ms: u64,
}

/// 工具 Schema 裁剪器 — Dressage 使用频率评分
#[derive(Clone, Debug)]
pub struct ToolSchemaPruner {
    /// 工具使用统计（轨迹分析累积）
    tool_usage_stats: HashMap<String, ToolUsageStats>,
    /// 保留门控阈值（score ≥ threshold 无条件保留）
    pruning_threshold: f32,
    /// 最少保留工具数（下限保护）
    min_tools: usize,
    /// 命名白名单（§18.3 熔断：无条件保留，不占 Top-K 配额）
    whitelist: Vec<String>,
    /// 单调时钟序号（新近度基准，analyze 调用递增）
    clock: u64,
    /// 裁剪决策日志（append-only，铁律6）
    decision_log: Vec<PruneDecision>,
    /// 决策序号（单调递增）
    decision_seq: u64,
}

impl ToolSchemaPruner {
    /// 创建裁剪器
    ///
    /// - `pruning_threshold`: 保留门控阈值 ∈ [0, 1]
    /// - `min_tools`: 最少保留工具数（防过度裁剪）
    pub fn new(pruning_threshold: f32, min_tools: usize) -> Self {
        Self {
            tool_usage_stats: HashMap::new(),
            pruning_threshold: pruning_threshold.clamp(0.0, 1.0),
            min_tools,
            whitelist: Vec::new(),
            clock: 0,
            decision_log: Vec::new(),
            decision_seq: 0,
        }
    }

    /// 注入命名白名单（§18.3 熔断保护：必要工具免疫过度裁剪）
    pub fn with_whitelist(mut self, whitelist: Vec<String>) -> Self {
        self.whitelist = whitelist;
        self
    }

    /// 轨迹分析 — 累积工具使用统计（频次/成功率/新近度）
    pub fn analyze_trajectories(&mut self, trajectories: &[PruneTrajectory]) {
        for traj in trajectories {
            for step in &traj.steps {
                let Some(tool_name) = &step.tool_name else {
                    continue; // 非工具调用步跳过
                };
                self.record_step(tool_name, step.success);
            }
        }
    }

    /// 在线单步记录 — 免批量轨迹的使用统计累积（W1 闭环接口）
    ///
    /// 供 TokenLedger / CHTC 实时反馈路径逐步调用；语义与
    /// `analyze_trajectories` 单步等价（同一 `record_step` 累积），
    /// 额外累积 token 消耗（批量轨迹的 `PruneStep` 不含 token 维度，
    /// 在线路径由调用方携带）。
    pub fn record_tool_step(&mut self, tool_name: &str, success: bool, tokens_consumed: u32) {
        self.record_step(tool_name, success);
        if let Some(stats) = self.tool_usage_stats.get_mut(tool_name) {
            stats.total_tokens_consumed += tokens_consumed;
        }
    }

    /// 单步累积共享逻辑 — clock 单调递增（新近度基准）
    fn record_step(&mut self, tool_name: &str, success: bool) {
        self.clock += 1;
        let stats = self
            .tool_usage_stats
            .entry(tool_name.to_string())
            .or_insert_with(|| ToolUsageStats {
                tool_name: tool_name.to_string(),
                call_count: 0,
                success_count: 0,
                total_tokens_consumed: 0,
                last_used: 0,
            });
        stats.call_count += 1;
        if success {
            stats.success_count += 1;
        }
        stats.last_used = self.clock;
    }

    /// 裁剪工具 — 评分 Top-K 保留（红线 R8: select_nth_unstable_by）
    ///
    /// 评分 = 频率×0.4 + 成功率×0.4 + 新近度×0.2（规范 §11.5）。
    /// 保留策略（W1 闭环，ADR-084）:
    /// 1. **白名单钉住（§18.3 熔断）**: 命名白名单工具无条件保留，
    ///    不消耗 Top-K 配额（必要工具免疫过度裁剪）
    /// 2. **门控保留**: score ≥ pruning_threshold 无条件保留
    /// 3. **Top-K 补足**: 其余按评分补足至 max(keep_count, min_tools)，
    ///    白名单数从下限中扣除（白名单为空时与旧语义逐位等价）
    ///
    /// 每次调用追加一条 `PruneDecision`（铁律6 可观测性）。
    pub fn prune_tools(&mut self, available: &[PruneToolSchema], keep_count: usize) -> PruneResult {
        if available.is_empty() {
            let result = PruneResult {
                kept: Vec::new(),
                pruned_count: 0,
                tokens_saved: 0,
            };
            self.append_decision(0, 0, keep_count, &result);
            return result;
        }
        // 评分（纯函数）
        let scored: Vec<(PruneToolSchema, f32)> = available
            .iter()
            .map(|tool| (tool.clone(), self.score_tool(&tool.name)))
            .collect();

        // 白名单钉住: 先分区摘出，不占 Top-K 配额
        let (pinned, mut rest): (Vec<ScoredEntry>, Vec<ScoredEntry>) =
            scored
                .into_iter()
                .partition(|(tool, _)| self.whitelist.iter().any(|w| w == &tool.name));
        let pinned_count = pinned.len();

        // 非白名单目标保留数: max(keep, min_tools) 扣除钉住数（白名单空 = 旧语义）
        let target = keep_count
            .max(self.min_tools)
            .saturating_sub(pinned_count)
            .min(rest.len());

        // 红线 R8: select_nth_unstable_by O(n) 部分排序（仅非白名单集合）
        if target < rest.len() {
            rest.select_nth_unstable_by(target, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        // 保留集合 = 白名单钉住 + Top-target + 门控保留
        let mut kept: Vec<PruneToolSchema> = pinned.into_iter().map(|(t, _)| t).collect();
        kept.extend(rest.iter().take(target).map(|(t, _)| t.clone()));
        for (tool, score) in rest.iter().skip(target) {
            if *score >= self.pruning_threshold {
                kept.push(tool.clone());
            }
        }
        let kept_tokens: u32 = kept.iter().map(|t| t.schema_tokens).sum();
        let total_tokens: u32 = available.iter().map(|t| t.schema_tokens).sum();
        let result = PruneResult {
            pruned_count: available.len() - kept.len(),
            tokens_saved: total_tokens.saturating_sub(kept_tokens),
            kept,
        };
        self.append_decision(available.len(), pinned_count, keep_count, &result);
        result
    }

    /// 追加裁剪决策日志（铁律6 可观测性；append-only）
    fn append_decision(
        &mut self,
        available_count: usize,
        pinned_count: usize,
        keep_param: usize,
        result: &PruneResult,
    ) {
        self.decision_seq += 1;
        self.decision_log.push(PruneDecision {
            seq: self.decision_seq,
            available_count,
            pinned_count,
            kept_count: result.kept.len(),
            pruned_count: result.pruned_count,
            tokens_saved: result.tokens_saved,
            keep_count_param: keep_param,
            timestamp_ms: unix_now_ms(),
        });
    }

    /// 导出裁剪决策轨迹（铁律6：统计学习机制可导出为 RLTrajectory）
    ///
    /// 投影约定:
    /// - state.layer_features[0..5] = [available, kept, pinned, pruned, tokens_saved]
    /// - action = 保留数决策（action_code = kept_count，parameters = [keep_count_param]）
    /// - reward = tokens_saved（即时效率信号；任务结局归因属 L5/L8 信用层，
    ///   本层不伪造结局奖励——诚实边界）
    pub fn export_trajectory(&self, episode_id: &str) -> RLTrajectory {
        let states: Vec<RLStateVector> = self
            .decision_log
            .iter()
            .map(|d| {
                let mut state = RLStateVector::zeros();
                state.layer_features[0] = d.available_count as f32;
                state.layer_features[1] = d.kept_count as f32;
                state.layer_features[2] = d.pinned_count as f32;
                state.layer_features[3] = d.pruned_count as f32;
                state.layer_features[4] = d.tokens_saved as f32;
                state
            })
            .collect();
        let actions: Vec<RLActionVector> = self
            .decision_log
            .iter()
            .map(|d| {
                RLActionVector::new(
                    "l6_tool_pruner",
                    d.kept_count as u32,
                    vec![d.keep_count_param as f32],
                )
            })
            .collect();
        let rewards: Vec<f32> = self.decision_log.iter().map(|d| d.tokens_saved as f32).collect();
        let timestamps: Vec<u64> = self.decision_log.iter().map(|d| d.timestamp_ms).collect();
        RLTrajectory::new(episode_id, states, actions, rewards, timestamps)
    }

    /// 单工具评分 — 频率×0.4 + 成功率×0.4 + 新近度×0.2（铁律4 纯函数）
    fn score_tool(&self, tool_name: &str) -> f32 {
        let Some(stats) = self.tool_usage_stats.get(tool_name) else {
            return 0.0; // 无统计 → 最低分
        };
        let total_calls: u32 = self.tool_usage_stats.values().map(|s| s.call_count).sum();
        let frequency = stats.call_count as f32 / total_calls.max(1) as f32;
        let success_rate = stats.success_count as f32 / stats.call_count.max(1) as f32;
        let recency = self.recency_score(stats.last_used);
        frequency * 0.4 + success_rate * 0.4 + recency * 0.2
    }

    /// 新近度评分 — last_used 相对当前 clock 的线性衰减（铁律4 纯函数）
    fn recency_score(&self, last_used: u64) -> f32 {
        if self.clock == 0 {
            return 0.0;
        }
        last_used as f32 / self.clock as f32
    }

    /// 统计只读访问（可观测性）
    pub fn usage_stats(&self) -> &HashMap<String, ToolUsageStats> {
        &self.tool_usage_stats
    }

    /// 阈值只读访问（可观测性）
    pub fn pruning_threshold(&self) -> f32 {
        self.pruning_threshold
    }

    /// 下限只读访问（可观测性）
    pub fn min_tools(&self) -> usize {
        self.min_tools
    }

    /// 白名单只读访问（可观测性）
    pub fn whitelist(&self) -> &[String] {
        &self.whitelist
    }

    /// 决策日志只读访问（铁律6 可观测性）
    pub fn decision_log(&self) -> &[PruneDecision] {
        &self.decision_log
    }
}

/// 当前 Unix 毫秒时间戳（决策日志可观测性；与 faae OperatorSelectionRecord
/// 的 timestamp 同语义——记录产生时刻，非确定性输入）
pub(crate) fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// TokenLedger → PruneTrajectory 适配器（W1 真实数据源闭环）
///
/// WHY 注入成功分类器: `ToolCallRecord` 的 result 为文本形态（L0 契约不解析），
/// 成功与否属系统边界判断，由调用方注入（按退出码/结构化结果判定），
/// 默认启发式用 [`success_if_result_nonempty`]。
pub fn prune_trajectories_from_ledger<F>(
    entries: &[TokenLedgerEntry],
    success_of: F,
) -> Vec<PruneTrajectory>
where
    F: Fn(&ToolCallRecord) -> bool,
{
    entries
        .iter()
        .map(|entry| PruneTrajectory {
            steps: entry
                .tool_calls
                .iter()
                .map(|call| PruneStep {
                    tool_name: Some(call.tool_name.to_string()),
                    success: success_of(call),
                })
                .collect(),
        })
        .collect()
}

/// 默认成功启发式 — result 文本非空（系统边界回退）
pub fn success_if_result_nonempty(record: &ToolCallRecord) -> bool {
    !record.result.trim().is_empty()
}

/// 评分条目 — 工具 schema + 使用分（clippy type_complexity 收敛）
type ScoredEntry = (PruneToolSchema, f32);

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn traj(tool: &str, success: bool) -> PruneTrajectory {
        PruneTrajectory {
            steps: vec![PruneStep {
                tool_name: Some(tool.to_string()),
                success,
            }],
        }
    }

    fn schema(name: &str, tokens: u32) -> PruneToolSchema {
        PruneToolSchema {
            name: name.to_string(),
            schema_tokens: tokens,
        }
    }

    #[test]
    fn score_weight_math() {
        let mut pruner = ToolSchemaPruner::new(0.5, 1);
        // 先分析 cold（clock=1）再分析 hot×2（clock=3，hot.last_used=3 → 新近度 1.0）
        pruner.analyze_trajectories(&[traj("cold", false), traj("hot", true), traj("hot", true)]);
        let hot_score = pruner.score_tool("hot");
        // 频率 = 2/3 × 0.4 + 成功率 1.0 × 0.4 + 新近度 1.0 × 0.2
        let expected = (2.0 / 3.0) * 0.4 + 1.0 * 0.4 + 1.0 * 0.2;
        assert!(
            (hot_score - expected).abs() < 1e-6,
            "评分权重数学（实际 {hot_score}，期望 {expected}）"
        );
    }

    #[test]
    fn unknown_tool_scores_zero() {
        let pruner = ToolSchemaPruner::new(0.5, 1);
        assert_eq!(pruner.score_tool("ghost"), 0.0);
    }

    #[test]
    fn top_k_selection_matches_full_sort() {
        let mut pruner = ToolSchemaPruner::new(0.99, 0); // 高阈值不干扰 Top-K
                                                         // a=3 次成功 / b=2 次 / c=1 次 / d=0 次
        for _ in 0..3 {
            pruner.analyze_trajectories(&[traj("a", true)]);
        }
        for _ in 0..2 {
            pruner.analyze_trajectories(&[traj("b", true)]);
        }
        pruner.analyze_trajectories(&[traj("c", true)]);
        let available: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| schema(n, 100))
            .collect();
        let result = pruner.prune_tools(&available, 2);
        // Top-2 应为 a 和 b（红线 R8 select_nth_unstable 与全排序等价）
        let kept_names: Vec<&str> = result.kept.iter().map(|t| t.name.as_str()).collect();
        assert!(kept_names.contains(&"a"));
        assert!(kept_names.contains(&"b"));
        assert_eq!(result.pruned_count, 2);
        assert_eq!(result.tokens_saved, 200);
    }

    #[test]
    fn min_tools_floor_protection() {
        let mut pruner = ToolSchemaPruner::new(0.99, 3); // 下限 3
        pruner.analyze_trajectories(&[traj("a", true)]);
        let available: Vec<_> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|n| schema(n, 10))
            .collect();
        let result = pruner.prune_tools(&available, 0); // keep_count=0 → min_tools 兜底
        assert_eq!(result.kept.len(), 3, "min_tools 下限保护");
    }

    #[test]
    fn threshold_unconditional_keep() {
        let mut pruner = ToolSchemaPruner::new(0.3, 0);
        // hot 高分（≥0.3 无条件保留）/ low 低分
        for _ in 0..5 {
            pruner.analyze_trajectories(&[traj("hot", true)]);
        }
        pruner.analyze_trajectories(&[traj("low", false)]);
        let available = vec![schema("hot", 100), schema("low", 100), schema("ghost", 100)];
        let result = pruner.prune_tools(&available, 1);
        // Top-1 = hot；其余 score < 0.3 被裁（low/ghost）
        let kept_names: Vec<&str> = result.kept.iter().map(|t| t.name.as_str()).collect();
        assert!(kept_names.contains(&"hot"));
        assert!(!kept_names.contains(&"ghost"));
    }

    #[test]
    fn empty_available_returns_empty() {
        let mut pruner = ToolSchemaPruner::new(0.5, 1);
        let result = pruner.prune_tools(&[], 5);
        assert!(result.kept.is_empty());
        assert_eq!(result.pruned_count, 0);
        assert_eq!(result.tokens_saved, 0);
        // 空输入也记录决策（铁律6 可观测性）
        assert_eq!(pruner.decision_log().len(), 1);
    }

    #[test]
    fn dressage_scenario_33_to_4() {
        // Dressage 实证: 33 个工具 → 4 个（13.5K → 1.7K tokens）
        let mut pruner = ToolSchemaPruner::new(0.99, 0);
        // 4 个高频工具各 10 次成功，29 个低频各 1 次失败
        for i in 0..4 {
            for _ in 0..10 {
                pruner.analyze_trajectories(&[traj(&format!("hot-{i}"), true)]);
            }
        }
        for i in 0..29 {
            pruner.analyze_trajectories(&[traj(&format!("low-{i}"), false)]);
        }
        let mut available: Vec<_> = (0..4).map(|i| schema(&format!("hot-{i}"), 425)).collect();
        for i in 0..29 {
            available.push(schema(&format!("low-{i}"), 400));
        }
        let total_tokens: u32 = available.iter().map(|t| t.schema_tokens).sum();
        let result = pruner.prune_tools(&available, 4);
        assert_eq!(result.kept.len(), 4, "33 → 4 裁剪");
        assert_eq!(result.pruned_count, 29);
        // 保留的必须是 hot 工具
        assert!(result.kept.iter().all(|t| t.name.starts_with("hot-")));
        let saved = total_tokens - result.kept.iter().map(|t| t.schema_tokens).sum::<u32>();
        assert_eq!(result.tokens_saved, saved);
    }

    #[test]
    fn non_tool_steps_skipped() {
        let mut pruner = ToolSchemaPruner::new(0.5, 1);
        pruner.analyze_trajectories(&[PruneTrajectory {
            steps: vec![
                PruneStep {
                    tool_name: None,
                    success: true,
                },
                PruneStep {
                    tool_name: Some("a".into()),
                    success: true,
                },
            ],
        }]);
        assert_eq!(pruner.usage_stats().len(), 1);
    }

    // ============================================================
    // W1 闭环新增测试（ADR-084）
    // ============================================================

    #[test]
    fn whitelist_pinned_survives_zero_keep() {
        // §18.3 熔断: keep_count=0 + min_tools=0 的极限裁剪下，
        // 白名单工具仍无条件保留（不占配额语义使 pinned 独立于 target）
        let mut pruner = ToolSchemaPruner::new(0.99, 0).with_whitelist(vec!["approval".into()]);
        pruner.analyze_trajectories(&[traj("approval", false), traj("hot", true)]);
        let available = vec![schema("approval", 100), schema("hot", 100)];
        let result = pruner.prune_tools(&available, 0);
        let kept_names: Vec<&str> = result.kept.iter().map(|t| t.name.as_str()).collect();
        assert!(kept_names.contains(&"approval"), "白名单钉住必须幸存");
        assert!(!kept_names.contains(&"hot"), "非白名单在 keep=0 时被裁");
        assert_eq!(result.pruned_count, 1);
    }

    #[test]
    fn whitelist_does_not_consume_quota() {
        // keep=4 总上限, 白名单钉住 2 → 非白名单配额 = 4-2 = 2
        //（钉住项计入总上限但不参与竞争, 使配额完整留给非白名单 Top-N）
        let mut pruner =
            ToolSchemaPruner::new(0.99, 0).with_whitelist(vec!["w1".into(), "w2".into()]);
        // a(1 次) < b(2 次) < c(3 次): 新近度与频率均 c > b > a
        pruner.analyze_trajectories(&[traj("a", true)]);
        pruner.analyze_trajectories(&[traj("b", true), traj("b", true)]);
        pruner.analyze_trajectories(&[traj("c", true), traj("c", true), traj("c", true)]);
        let available = vec![
            schema("w1", 10),
            schema("w2", 10),
            schema("a", 10),
            schema("b", 10),
            schema("c", 10),
        ];
        let result = pruner.prune_tools(&available, 4);
        assert_eq!(result.kept.len(), 4, "钉住 2 + 非白名单配额 2");
        let kept_names: Vec<&str> = result.kept.iter().map(|t| t.name.as_str()).collect();
        assert!(kept_names.contains(&"w1") && kept_names.contains(&"w2"), "钉住项");
        assert!(kept_names.contains(&"c") && kept_names.contains(&"b"), "评分 Top-2 = c,b");
        assert!(!kept_names.contains(&"a"), "最低分被裁");
    }

    #[test]
    fn whitelist_empty_equivalent_to_legacy_semantics() {
        // 白名单为空时 target = max(keep, min_tools).min(len)（与旧实现逐位等价）
        let mut pruner = ToolSchemaPruner::new(0.99, 3); // 无白名单
        pruner.analyze_trajectories(&[traj("a", true)]);
        let available: Vec<_> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|n| schema(n, 10))
            .collect();
        let result = pruner.prune_tools(&available, 0);
        assert_eq!(result.kept.len(), 3, "min_tools 下限兜底（旧语义）");
    }

    #[test]
    fn record_tool_step_equivalent_to_analyze() {
        // 在线单步记录与批量轨迹分析的统计累积等价
        let mut online = ToolSchemaPruner::new(0.5, 1);
        let mut batch = ToolSchemaPruner::new(0.5, 1);
        for _ in 0..3 {
            online.record_tool_step("t1", true, 100);
        }
        online.record_tool_step("t2", false, 50);
        batch.analyze_trajectories(&[
            traj("t1", true),
            traj("t1", true),
            traj("t1", true),
            traj("t2", false),
        ]);
        let s_online = online.usage_stats().get("t1").expect("t1 在线统计");
        let s_batch = batch.usage_stats().get("t1").expect("t1 批量统计");
        assert_eq!(s_online.call_count, s_batch.call_count);
        assert_eq!(s_online.success_count, s_batch.success_count);
        assert_eq!(s_online.last_used, s_batch.last_used);
        assert_eq!(s_online.total_tokens_consumed, 300, "token 累积仅在线路径");
        // 评分一致（铁律4 纯函数，同统计同分）
        assert_eq!(online.score_tool("t1"), batch.score_tool("t1"));
    }

    #[test]
    fn export_trajectory_invariants() {
        // 铁律6: 决策日志 → RLTrajectory 四序列等长 + 投影字段正确
        let mut pruner = ToolSchemaPruner::new(0.99, 0);
        pruner.analyze_trajectories(&[traj("hot", true), traj("hot", true)]);
        let available = vec![schema("hot", 425), schema("cold", 400)];
        pruner.prune_tools(&available, 1);
        pruner.prune_tools(&available, 2);
        let trajectory = pruner.export_trajectory("episode-w1");
        assert_eq!(trajectory.len(), 2);
        assert_eq!(trajectory.states.len(), trajectory.actions.len());
        assert_eq!(trajectory.states.len(), trajectory.rewards.len());
        assert_eq!(trajectory.states.len(), trajectory.timestamps.len());
        // 第 1 次裁剪: available=2 kept=1 → state 投影 + action 编码
        assert_eq!(trajectory.states[0].layer_features[0], 2.0);
        assert_eq!(trajectory.states[0].layer_features[1], 1.0);
        assert_eq!(trajectory.actions[0].layer.as_ref(), "l6_tool_pruner");
        assert_eq!(trajectory.actions[0].action_code, 1);
        // 第 2 次裁剪 kept=2
        assert_eq!(trajectory.actions[1].action_code, 2);
        // 决策日志 append-only: seq 单调递增
        let log = pruner.decision_log();
        assert!(log[0].seq < log[1].seq);
    }

    #[test]
    fn ledger_adapter_maps_tool_calls() {
        // TokenLedger → PruneTrajectory 适配: 工具调用映射 + 成功分类器注入
        use nexus_contracts::token_evidence::{TokenLedgerEntry, ToolCallRecord};
        let record_ok = ToolCallRecord::new("read_file", "{}", "ok", 10);
        let record_err = ToolCallRecord::new("bash", "{}", "", 10);
        let make_entry = |id: &str, calls: Vec<ToolCallRecord>| {
            TokenLedgerEntry::new(
                id, 1, "s-1", "i-1", vec![], vec![], vec![], vec![], "v1", calls, None, 0,
            )
        };
        let entry = make_entry("e-1", vec![record_ok.clone(), record_err]);
        let trajectories = prune_trajectories_from_ledger(&[entry], success_if_result_nonempty);
        assert_eq!(trajectories.len(), 1);
        assert_eq!(trajectories[0].steps.len(), 2);
        assert_eq!(
            trajectories[0].steps[0].tool_name.as_deref(),
            Some("read_file")
        );
        assert!(trajectories[0].steps[0].success, "result 非空 → 成功");
        assert!(!trajectories[0].steps[1].success, "result 空 → 失败");
        // 自定义分类器注入（系统边界判断由调用方决定）
        let always_fail = prune_trajectories_from_ledger(
            &[make_entry("e-2", vec![record_ok])],
            |_| false,
        );
        assert!(!always_fail[0].steps[0].success);
    }
}
