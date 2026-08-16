//! 六维动态调整器 — D1-D6 控制面纯规则反馈调整（规范 §11.3，ADR-084 决策 1）
//!
//! 对应架构层: **L6 Router**（osa-coordinator 内嵌，38 crate 基线不动）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §11.3
//! （**规范本身未给出算法**——本模块为 Phase 6 W2 全新建设，规则集经 ADR-084 裁决）
//!
//! # 核心职责
//!
//! 消费**既有** NexusEvent 反馈信号（零新增事件变体，ADR-084 决策 2），
//! 以纯规则调整 L0 [`HarnessConfigContract`] 的 D1-D6 字段：
//!
//! | 触发事件 | 规则（方向由载荷驱动） | 维度 |
//! |---|---|---|
//! | `HcwRecallDegraded` | 召回退化 → 祖先检索深度 +1、兄弟检索数 +1（加宽检索面） | D1 |
//! | `RouterStatsReported` | 三路由器平均命中率 < 0.5 → retrieval_top_k +1；> 0.9 → -1（死区 [0.5, 0.9] 防振荡） | D2 |
//! | `BudgetExceeded` | 预算超限 → max_tools_per_step -1（收紧工具面） | D2 |
//! | `EntropyBalanced` | 均衡后熵下降（均衡有效）→ entropy_weighting = true | D3 |
//!
//! # 设计约束（铁律）
//!
//! - **铁律4**: 规则为纯函数（同事件 + 同契约 → 同输出；确定性，无随机源）
//! - **铁律3 风格**: `journal` append-only，既有记录不可修改
//! - **clamp 边界**: 每字段调整均在 [`AdjustmentLimits`] 界内；到达边界为 no-op
//!   （不记 journal、不 bump version——避免无意义版本膨胀）
//! - **版本化**: 每次生效调整 patch 版本 +1 + SHA-256 内容重哈希（L0 仅承载字段，
//!   计算留在 L6——与 `compute_omni_mask_hash` 同分工）
//! - **C4/R2 合规**: 纯规则基（编译进二进制），无学习钩子； Learned 策略接入为
//!   R2 解冻后扩展点（记档 ADR-084，本阶段不实现）
//! - **铁律6**: `export_trajectory` 导出调整决策轨迹（奖励为 0——调整动作无即时
//!   可测收益，结局归因依赖后续反馈事件，v4.0 离线训练应按轨迹对齐回填，不伪造）

use event_bus::NexusEvent;
use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector, RLTrajectory};
use nexus_contracts::HarnessConfigContract;
use sha2::{Digest, Sha256};

use crate::tool_pruning::unix_now_ms;

/// 调整边界 — 每字段 clamp 界（防规则失控的硬边界）
#[derive(Clone, Debug)]
pub struct AdjustmentLimits {
    /// D1 祖先检索深度上限
    pub max_ancestor_retrieval_depth: u32,
    /// D1 兄弟检索数量上限
    pub max_sibling_retrieval_count: u32,
    /// D2 每步最大工具数下限（保留至少 1 个工具）
    pub min_max_tools_per_step: usize,
    /// D2 每步最大工具数上限
    pub max_max_tools_per_step: usize,
    /// D2 工具检索 Top-K 下限
    pub min_retrieval_top_k: usize,
    /// D2 工具检索 Top-K 上限
    pub max_retrieval_top_k: usize,
}

impl Default for AdjustmentLimits {
    /// 默认边界: D1 深度 ≤ 4 / 兄弟 ≤ 8; D2 工具步 [1, 16] / Top-K [1, 32]
    fn default() -> Self {
        Self {
            max_ancestor_retrieval_depth: 4,
            max_sibling_retrieval_count: 8,
            min_max_tools_per_step: 1,
            max_max_tools_per_step: 16,
            min_retrieval_top_k: 1,
            max_retrieval_top_k: 32,
        }
    }
}

/// 调整日志条目 — append-only（铁律3 风格 + 铁律6 可导出）
#[derive(Clone, Debug)]
pub struct AdjustmentRecord {
    /// 决策序号（单调递增）
    pub seq: u64,
    /// 触发事件名（如 "HcwRecallDegraded"）
    pub trigger: &'static str,
    /// 目标维度（1-6）
    pub dimension: u8,
    /// 目标字段名
    pub field: &'static str,
    /// 调整前值（bool 以 0/1 数值化）
    pub old_value: i64,
    /// 调整后值
    pub new_value: i64,
    /// 调整后契约版本
    pub version_after: String,
    /// 决策时间戳（Unix 毫秒）
    pub timestamp_ms: u64,
}

/// 六维动态调整器 — 纯规则反馈调整 D1-D6 控制面契约
///
/// 非 `Clone`（内部含 append-only journal 与单调 seq，克隆会产生分叉历史）。
pub struct SixDimensionAdjuster {
    /// 当前契约（每次生效调整产出新版本）
    contract: HarnessConfigContract,
    /// 调整日志（append-only）
    journal: Vec<AdjustmentRecord>,
    /// clamp 边界
    limits: AdjustmentLimits,
    /// 决策序号（单调递增）
    seq: u64,
}

impl SixDimensionAdjuster {
    /// 创建调整器 — 从默认契约起步（version "0.1.0"）
    pub fn new() -> Self {
        Self::with_contract(HarnessConfigContract::default_contract())
    }

    /// 创建调整器 — 从指定契约起步
    pub fn with_contract(contract: HarnessConfigContract) -> Self {
        Self {
            contract,
            journal: Vec::new(),
            limits: AdjustmentLimits::default(),
            seq: 0,
        }
    }

    /// 注入自定义 clamp 边界
    pub fn with_limits(mut self, limits: AdjustmentLimits) -> Self {
        self.limits = limits;
        self
    }

    /// 应用反馈事件 — 纯规则分派（铁律4: 同输入同输出）
    ///
    /// 非反馈事件静默忽略（调整器只消费载荷可定方向的明确信号）。
    pub fn apply_feedback(&mut self, event: &NexusEvent) {
        match event {
            NexusEvent::HcwRecallDegraded { .. } => self.widen_retrieval(),
            NexusEvent::RouterStatsReported {
                kvbsr_stats,
                sesa_stats,
                faae_stats,
                ..
            } => {
                // 三路由器平均命中率（确定性聚合，死区控制器）
                let avg = (kvbsr_stats.hit_rate + sesa_stats.hit_rate + faae_stats.hit_rate)
                    / 3.0f32;
                self.adjust_retrieval_top_k(avg);
            }
            NexusEvent::BudgetExceeded { .. } => self.tighten_tools_per_step(),
            NexusEvent::EntropyBalanced {
                old_entropy,
                new_entropy,
                ..
            } => self.enable_entropy_weighting(*old_entropy, *new_entropy),
            _ => {}
        }
    }

    /// 当前契约只读访问
    pub fn current_contract(&self) -> &HarnessConfigContract {
        &self.contract
    }

    /// 调整日志只读访问（append-only，铁律6 可观测性）
    pub fn journal(&self) -> &[AdjustmentRecord] {
        &self.journal
    }

    /// clamp 边界只读访问
    pub fn limits(&self) -> &AdjustmentLimits {
        &self.limits
    }

    /// 导出调整决策轨迹（铁律6）
    ///
    /// 投影约定: state.layer_features[0..4] = [dimension, old, new, seq];
    /// action = 维度调整（action_code = dimension, parameters = [new - old]）;
    /// reward 恒 0 —— 调整动作无即时可测收益（诚实边界，见模块文档）。
    pub fn export_trajectory(&self, episode_id: &str) -> RLTrajectory {
        let states: Vec<RLStateVector> = self
            .journal
            .iter()
            .map(|r| {
                let mut state = RLStateVector::zeros();
                state.layer_features[0] = r.dimension as f32;
                state.layer_features[1] = r.old_value as f32;
                state.layer_features[2] = r.new_value as f32;
                state.layer_features[3] = r.seq as f32;
                state
            })
            .collect();
        let actions: Vec<RLActionVector> = self
            .journal
            .iter()
            .map(|r| {
                RLActionVector::new(
                    "l6_six_dimension_adjuster",
                    r.dimension as u32,
                    vec![(r.new_value - r.old_value) as f32],
                )
            })
            .collect();
        let rewards: Vec<f32> = self.journal.iter().map(|_| 0.0).collect();
        let timestamps: Vec<u64> = self.journal.iter().map(|r| r.timestamp_ms).collect();
        RLTrajectory::new(episode_id, states, actions, rewards, timestamps)
    }

    // ========================================================
    // 四条规则（载荷驱动方向，全部 clamp;边界 no-op 不记 journal）
    // ========================================================

    /// D1 规则: 召回退化 → 加宽检索面（祖先深度 +1、兄弟数 +1）
    fn widen_retrieval(&mut self) {
        let d1 = &mut self.contract.d1_context;
        let old_depth = d1.ancestor_retrieval_depth;
        let old_siblings = d1.sibling_retrieval_count;
        let new_depth = (old_depth + 1).min(self.limits.max_ancestor_retrieval_depth);
        let new_siblings = (old_siblings + 1).min(self.limits.max_sibling_retrieval_count);
        d1.ancestor_retrieval_depth = new_depth;
        d1.sibling_retrieval_count = new_siblings;
        if new_depth != old_depth {
            self.commit(
                "HcwRecallDegraded",
                1,
                "ancestor_retrieval_depth",
                old_depth as i64,
                new_depth as i64,
            );
        }
        if new_siblings != old_siblings {
            self.commit(
                "HcwRecallDegraded",
                1,
                "sibling_retrieval_count",
                old_siblings as i64,
                new_siblings as i64,
            );
        }
    }

    /// D2 规则: 三路由器平均命中率死区控制（< 0.5 加宽 / > 0.9 收窄 / 死区 no-op）
    fn adjust_retrieval_top_k(&mut self, avg_hit_rate: f32) {
        let delta: i64 = if avg_hit_rate < 0.5 {
            1
        } else if avg_hit_rate > 0.9 {
            -1
        } else {
            return; // 死区: 防止规则与统计噪声振荡
        };
        let old = self.contract.d2_tool.retrieval_top_k as i64;
        let new = (old + delta)
            .clamp(
                self.limits.min_retrieval_top_k as i64,
                self.limits.max_retrieval_top_k as i64,
            );
        if new == old {
            return; // 边界 no-op
        }
        self.contract.d2_tool.retrieval_top_k = new as usize;
        self.commit("RouterStatsReported", 2, "retrieval_top_k", old, new);
    }

    /// D2 规则: 预算超限 → 收紧每步工具数（下限 1，防过度裁剪）
    fn tighten_tools_per_step(&mut self) {
        let old = self.contract.d2_tool.max_tools_per_step as i64;
        let new = (old - 1).max(self.limits.min_max_tools_per_step as i64);
        if new == old {
            return; // 已到下限 no-op
        }
        self.contract.d2_tool.max_tools_per_step = new as usize;
        self.commit("BudgetExceeded", 2, "max_tools_per_step", old, new);
    }

    /// D3 规则: 均衡后熵下降（均衡有效）→ 启用熵加权
    fn enable_entropy_weighting(&mut self, old_entropy: f32, new_entropy: f32) {
        if new_entropy >= old_entropy || self.contract.d3_generation.entropy_weighting {
            return; // 均衡无效或已启用 → no-op
        }
        self.contract.d3_generation.entropy_weighting = true;
        self.commit("EntropyBalanced", 3, "entropy_weighting", 0, 1);
    }

    /// 提交一次生效调整: journal 追加 + patch 版本递增 + 内容重哈希
    fn commit(
        &mut self,
        trigger: &'static str,
        dimension: u8,
        field: &'static str,
        old_value: i64,
        new_value: i64,
    ) {
        self.seq += 1;
        self.bump_version_and_hash();
        self.journal.push(AdjustmentRecord {
            seq: self.seq,
            trigger,
            dimension,
            field,
            old_value,
            new_value,
            version_after: self.contract.version.clone(),
            timestamp_ms: unix_now_ms(),
        });
    }

    /// patch 版本递增（末段 +1;解析失败回退追加 ".1"）+ SHA-256 内容重哈希
    ///
    /// 哈希口径: 将 `hash` 字段清零后序列化的 JSON 的 SHA-256 hex
    /// （自指消除——哈希值本身不参与哈希输入）。
    fn bump_version_and_hash(&mut self) {
        let segments: Vec<&str> = self.contract.version.split('.').collect();
        let bumped = match segments.last().and_then(|last| last.parse::<u64>().ok()) {
            Some(patch) => {
                let mut owned: Vec<String> =
                    self.contract.version.split('.').map(String::from).collect();
                let last_idx = owned.len() - 1;
                owned[last_idx] = (patch + 1).to_string();
                owned.join(".")
            }
            None => format!("{}.1", self.contract.version),
        };
        self.contract.version = bumped;
        self.contract.hash = String::new();
        let json = serde_json::to_string(&self.contract).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        self.contract.hash = hex::encode(hasher.finalize());
    }
}

impl Default for SixDimensionAdjuster {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{EventMetadata, RouterStatsPayload};

    fn meta() -> EventMetadata {
        EventMetadata::new("test-six-dimension")
    }

    fn stats(hit_rate: f32) -> RouterStatsPayload {
        RouterStatsPayload {
            hit_rate,
            p50_latency_us: 10,
            p95_latency_us: 100,
            p99_latency_us: 500,
            hot_capabilities: Vec::new(),
        }
    }

    fn recall_degraded() -> NexusEvent {
        NexusEvent::HcwRecallDegraded {
            metadata: meta(),
            tier: "L2".into(),
            recall_rate: 0.3,
            baseline_recall: 0.8,
            reason: "sentinel_2x_below_baseline".into(),
        }
    }

    fn budget_exceeded() -> NexusEvent {
        NexusEvent::BudgetExceeded {
            metadata: meta(),
            budget_type: "token".into(),
            current: 1200,
            limit: 1000,
        }
    }

    fn entropy_balanced(old: f32, new: f32) -> NexusEvent {
        NexusEvent::EntropyBalanced {
            metadata: meta(),
            old_entropy: old,
            new_entropy: new,
            redistributed_count: 3,
        }
    }

    fn router_stats(hit: f32) -> NexusEvent {
        NexusEvent::RouterStatsReported {
            metadata: meta(),
            kvbsr_stats: stats(hit),
            sesa_stats: stats(hit),
            faae_stats: stats(hit),
        }
    }

    #[test]
    fn recall_degraded_widens_d1_retrieval() {
        let mut adjuster = SixDimensionAdjuster::new();
        // 默认 D1: 深度 2 / 兄弟 3
        adjuster.apply_feedback(&recall_degraded());
        let d1 = &adjuster.current_contract().d1_context;
        assert_eq!(d1.ancestor_retrieval_depth, 3, "2 → 3");
        assert_eq!(d1.sibling_retrieval_count, 4, "3 → 4");
        assert_eq!(adjuster.journal().len(), 2, "两条字段调整各记一条");
    }

    #[test]
    fn recall_widen_clamped_at_limits() {
        let mut adjuster = SixDimensionAdjuster::new();
        // 连续 10 次退化事件: 深度封顶 4 / 兄弟封顶 8, 之后 no-op
        for _ in 0..10 {
            adjuster.apply_feedback(&recall_degraded());
        }
        let d1 = &adjuster.current_contract().d1_context;
        assert_eq!(d1.ancestor_retrieval_depth, 4, "深度封顶");
        assert_eq!(d1.sibling_retrieval_count, 8, "兄弟封顶");
        // 2→4 共 2 次 + 3→8 共 5 次 = 7 条 journal（封顶后不再追加）
        assert_eq!(adjuster.journal().len(), 7, "边界 no-op 不记 journal");
    }

    #[test]
    fn budget_exceeded_tightens_d2_tools() {
        let mut adjuster = SixDimensionAdjuster::new();
        // 默认 D2: max_tools_per_step = 5
        adjuster.apply_feedback(&budget_exceeded());
        assert_eq!(adjuster.current_contract().d2_tool.max_tools_per_step, 4);
        // 连续超限: 下限 1 封底
        for _ in 0..10 {
            adjuster.apply_feedback(&budget_exceeded());
        }
        assert_eq!(adjuster.current_contract().d2_tool.max_tools_per_step, 1);
        // 5→1 共 4 次生效调整
        assert_eq!(adjuster.journal().len(), 4);
    }

    #[test]
    fn router_stats_dead_zone_controller() {
        let mut adjuster = SixDimensionAdjuster::new();
        // 默认 retrieval_top_k = 10
        // 死区 [0.5, 0.9]: 0.7 → no-op
        adjuster.apply_feedback(&router_stats(0.7));
        assert_eq!(adjuster.current_contract().d2_tool.retrieval_top_k, 10);
        assert!(adjuster.journal().is_empty());
        // 低命中 0.3 → +1 加宽
        adjuster.apply_feedback(&router_stats(0.3));
        assert_eq!(adjuster.current_contract().d2_tool.retrieval_top_k, 11);
        // 高命中 0.95 → -1 收窄
        adjuster.apply_feedback(&router_stats(0.95));
        assert_eq!(adjuster.current_contract().d2_tool.retrieval_top_k, 10);
        assert_eq!(adjuster.journal().len(), 2);
    }

    #[test]
    fn entropy_drop_enables_weighting_once() {
        let mut adjuster = SixDimensionAdjuster::new();
        // 默认 entropy_weighting = true → no-op（已启用）
        adjuster.apply_feedback(&entropy_balanced(2.0, 1.0));
        assert!(adjuster.journal().is_empty(), "已启用不重复调整");
        // 手动关闭后再触发: 熵下降 → 启用
        adjuster.contract.d3_generation.entropy_weighting = false;
        adjuster.apply_feedback(&entropy_balanced(2.0, 1.5));
        assert!(adjuster.current_contract().d3_generation.entropy_weighting);
        assert_eq!(adjuster.journal().len(), 1);
        // 熵上升（均衡无效）→ no-op
        adjuster.contract.d3_generation.entropy_weighting = false;
        adjuster.apply_feedback(&entropy_balanced(1.0, 2.0));
        assert!(!adjuster.current_contract().d3_generation.entropy_weighting);
        assert_eq!(adjuster.journal().len(), 1);
    }

    #[test]
    fn non_feedback_events_ignored() {
        let mut adjuster = SixDimensionAdjuster::new();
        let irrelevant = NexusEvent::ToolsRouted {
            metadata: meta(),
            routed_count: 3,
            top_tool: "bash".into(),
            routed_tools: Vec::new(),
        };
        adjuster.apply_feedback(&irrelevant);
        assert!(adjuster.journal().is_empty(), "非反馈事件静默忽略");
        assert_eq!(adjuster.current_contract().version, "0.1.0", "版本不变");
    }

    #[test]
    fn version_bump_and_hash_recompute() {
        let mut adjuster = SixDimensionAdjuster::new();
        assert_eq!(adjuster.current_contract().version, "0.1.0");
        assert!(adjuster.current_contract().hash.is_empty(), "默认契约 hash 由消费方填充");
        adjuster.apply_feedback(&budget_exceeded());
        let contract = adjuster.current_contract();
        assert_eq!(contract.version, "0.1.1", "patch 递增");
        assert_eq!(contract.hash.len(), 64, "SHA-256 hex");
        // 哈希确定性: 同一调整序列重放得到相同哈希（铁律4）
        let mut replay = SixDimensionAdjuster::new();
        replay.apply_feedback(&budget_exceeded());
        assert_eq!(
            replay.current_contract().hash,
            contract.hash,
            "同输入同输出（纯规则确定性）"
        );
    }

    #[test]
    fn journal_append_only_and_seq_monotonic() {
        let mut adjuster = SixDimensionAdjuster::new();
        adjuster.apply_feedback(&recall_degraded());
        let first = adjuster.journal()[0].clone();
        adjuster.apply_feedback(&budget_exceeded());
        let journal = adjuster.journal();
        assert_eq!(journal[0].seq, first.seq, "既有记录不可修改（铁律3 风格）");
        assert_eq!(journal[0].old_value, first.old_value);
        assert!(journal[1].seq > journal[0].seq, "seq 单调递增");
    }

    #[test]
    fn export_trajectory_invariants() {
        let mut adjuster = SixDimensionAdjuster::new();
        adjuster.apply_feedback(&recall_degraded());
        adjuster.apply_feedback(&budget_exceeded());
        let trajectory = adjuster.export_trajectory("episode-w2");
        assert_eq!(trajectory.len(), 3, "2(召回) + 1(预算)");
        assert_eq!(trajectory.actions[0].layer.as_ref(), "l6_six_dimension_adjuster");
        // 维度编码: D1 x2 + D2 x1
        assert_eq!(trajectory.actions[0].action_code, 1);
        assert_eq!(trajectory.actions[2].action_code, 2);
        // 调整幅度参数: max_tools 5→4 → -1
        assert_eq!(trajectory.actions[2].parameters[0], -1.0);
        // reward 恒 0（诚实边界: 延迟奖励由后续反馈事件对齐回填）
        assert!(trajectory.rewards.iter().all(|r| *r == 0.0));
        assert!(trajectory.timestamps.iter().all(|t| *t > 0));
    }
}
