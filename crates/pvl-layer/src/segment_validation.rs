//! Segment-aware 分段感知验证 — Dressage 核心（设计文档 §12.4）
//!
//! 对应架构层: **L7 Execution**（pvl-layer 子模块，规范指定落点）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §12.4
//! 对应论文: 微软 Dressage（Segment-aware 验证 + 终局奖励传播）
//!
//! # 核心职责
//!
//! 轨迹分段级验证与奖励传播：
//! - **三级验证**: 复用 `rlvr.rs` [`VerifierKind`] 语法/逻辑/沙箱验证
//!   （enum dispatch，Milestone D-2c 既有基座）
//! - **铁律9 分段身份**: 同轨迹分段共享 `parent_traj_id`（注册表分组）
//! - **终局奖励传播**: anchor 段直接承载 final_reward；非 anchor 段按
//!   `process_reward + final_reward × 0.3` 传播
//! - **prompt-equal 边界**: 分段数折减由 L1 `SegmentAwarePER` 侧处理
//!   （td_error/sqrt(segment_count)），本模块不重复折减
//!
//! # 设计约束（铁律）
//!
//! - **铁律3**: [`SegmentMetadata`] 构造后不可变——奖励状态走本模块内部
//!   [`SegmentRewardState`] overlay（D-4），禁止修改 L0 契约
//! - **防御分支**: 规范原型 `segment_registry.get_mut(..).unwrap()` 替换为
//!   `Result`（未知轨迹显式错误，不 panic）

use std::collections::HashMap;

use nexus_contracts::token_evidence::SegmentMetadata;
use thiserror::Error;

use crate::rlvr::{VerifierKind, RLVR};

/// 分段验证错误（库层 thiserror，§4.1）
#[derive(Debug, Error, PartialEq)]
pub enum SegmentValidationError {
    /// 未知轨迹 ID（注册表无此 parent_traj_id）
    #[error("未知轨迹 ID: {0}")]
    UnknownTrajectory(String),
}

/// 分段奖励状态 — 奖励 overlay（D-4，不修改不可变 SegmentMetadata）
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentRewardState {
    /// 分段 ID
    pub segment_id: Box<str>,
    /// 是否 anchor 段（承载终局 reward）
    pub is_anchor: bool,
    /// 过程奖励（三级验证计算）
    pub process_reward: f32,
    /// 终局奖励（broadcast_final_reward 后 Some）
    pub final_reward: Option<f32>,
}

/// 分段验证结果（规范 §12.4 SegmentValidationResult）
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentValidationResult {
    /// 分段 ID
    pub segment_id: Box<str>,
    /// 父轨迹 ID（铁律9 共享身份）
    pub parent_traj_id: Box<str>,
    /// 语法验证通过
    pub syntax_pass: bool,
    /// 逻辑验证通过
    pub logic_pass: bool,
    /// 沙箱验证通过
    pub sandbox_pass: bool,
    /// 分段过程奖励
    pub segment_reward: f32,
    /// 是否 anchor 段
    pub is_anchor: bool,
}

/// 分段感知验证器 — 三级验证 + 奖励 overlay + 终局传播
#[derive(Debug)]
pub struct SegmentAwareValidator {
    /// 三级验证器（复用 rlvr.rs enum dispatch）
    rlvr: RLVR,
    /// 分段注册表: parent_traj_id → segments（铁律9 共享身份分组）
    segment_registry: HashMap<Box<str>, Vec<SegmentMetadata>>,
    /// 奖励 overlay: parent_traj_id → reward states（D-4）
    reward_overlay: HashMap<Box<str>, Vec<SegmentRewardState>>,
}

impl Default for SegmentAwareValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentAwareValidator {
    /// 创建验证器（三级验证器序列 [Syntax, Logic, Sandbox]）
    pub fn new() -> Self {
        Self {
            rlvr: RLVR::new(vec![
                VerifierKind::Syntax,
                VerifierKind::Logic,
                VerifierKind::Sandbox,
            ]),
            segment_registry: HashMap::new(),
            reward_overlay: HashMap::new(),
        }
    }

    /// 注册分段 — 按 parent_traj_id 分组（铁律9 身份共享）
    ///
    /// 重复注册同 segment_id 幂等覆盖（防重放）。
    pub fn register_segment(&mut self, metadata: SegmentMetadata) {
        let key: Box<str> = metadata.parent_traj_id.clone();
        let segments = self.segment_registry.entry(key.clone()).or_default();
        // 幂等: 同 segment_id 替换
        if let Some(existing) = segments
            .iter_mut()
            .find(|s| s.segment_id == metadata.segment_id)
        {
            *existing = metadata.clone();
        } else {
            segments.push(metadata.clone());
        }
        let overlay = self.reward_overlay.entry(key).or_default();
        if let Some(state) = overlay
            .iter_mut()
            .find(|s| s.segment_id == metadata.segment_id)
        {
            // 重注册保留 process_reward，重置终局奖励
            state.is_anchor = metadata.is_anchor;
            state.final_reward = None;
        } else {
            overlay.push(SegmentRewardState {
                segment_id: metadata.segment_id.clone(),
                is_anchor: metadata.is_anchor,
                process_reward: 0.0,
                final_reward: None,
            });
        }
    }

    /// 验证分段 — 三级验证 + 过程奖励记录
    ///
    /// `output`: 分段执行输出；`latency_ms`: 执行延迟（RLVR 惩罚输入）。
    /// 分段必须已注册（未注册分段先自动注册，容错孤儿输入）。
    pub fn validate_segment(
        &mut self,
        segment: &SegmentMetadata,
        output: &str,
        latency_ms: u64,
    ) -> SegmentValidationResult {
        let syntax_pass = VerifierKind::Syntax.verify(output);
        let logic_pass = VerifierKind::Logic.verify(output);
        let sandbox_pass = VerifierKind::Sandbox.verify(output);
        // 过程奖励经 RLVR 统一计算（空测试用例，沙箱档由 pass_rate 表达）
        let segment_reward = self.rlvr.compute_reward(output, &[], latency_ms);

        // 未注册分段自动注册（容错孤儿输入）
        if !self
            .segment_registry
            .get(segment.parent_traj_id.as_ref())
            .is_some_and(|segs| segs.iter().any(|s| s.segment_id == segment.segment_id))
        {
            self.register_segment(segment.clone());
        }
        // 记录过程奖励到 overlay
        if let Some(states) = self.reward_overlay.get_mut(segment.parent_traj_id.as_ref()) {
            if let Some(state) = states
                .iter_mut()
                .find(|s| s.segment_id == segment.segment_id)
            {
                state.process_reward = segment_reward;
            }
        }

        SegmentValidationResult {
            segment_id: segment.segment_id.clone(),
            parent_traj_id: segment.parent_traj_id.clone(),
            syntax_pass,
            logic_pass,
            sandbox_pass,
            segment_reward,
            is_anchor: segment.is_anchor,
        }
    }

    /// 广播终局奖励 — anchor 直接承载，非 anchor 按 0.3 系数传播
    ///
    /// 规范 §12.4 broadcast_final_reward；未知轨迹返回错误（防御分支，
    /// 替换原型 unwrap）。返回受影响分段数。
    pub fn broadcast_final_reward(
        &mut self,
        parent_traj_id: &str,
        final_reward: f32,
    ) -> Result<usize, SegmentValidationError> {
        let states = self
            .reward_overlay
            .get_mut(parent_traj_id)
            .ok_or_else(|| SegmentValidationError::UnknownTrajectory(parent_traj_id.to_string()))?;
        let mut affected = 0;
        for state in states.iter_mut() {
            if state.is_anchor {
                // anchor 段直接承载终局奖励（铁律9）
                state.final_reward = Some(final_reward);
            } else {
                // 非 anchor 段: process_reward + final_reward × 0.3 传播
                let propagated = state.process_reward + final_reward * 0.3;
                state.final_reward = Some(propagated);
            }
            affected += 1;
        }
        Ok(affected)
    }

    /// 分段奖励状态只读访问（可观测性）
    pub fn reward_states(&self, parent_traj_id: &str) -> Option<&[SegmentRewardState]> {
        self.reward_overlay.get(parent_traj_id).map(Vec::as_slice)
    }

    /// 轨迹分段数只读访问（可观测性）
    pub fn segment_count(&self, parent_traj_id: &str) -> usize {
        self.segment_registry
            .get(parent_traj_id)
            .map(Vec::len)
            .unwrap_or(0)
    }

    /// 已注册轨迹数只读访问（可观测性）
    pub fn trajectory_count(&self) -> usize {
        self.segment_registry.len()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::token_evidence::SegmentCreationReason;

    fn segment(id: &str, traj: &str, index: u32, is_anchor: bool) -> SegmentMetadata {
        SegmentMetadata::new(
            id,
            traj,
            index,
            is_anchor,
            Vec::new(),
            Vec::new(),
            0,
            5,
            SegmentCreationReason::NaturalBoundary,
        )
    }

    #[test]
    fn register_and_count() {
        let mut validator = SegmentAwareValidator::new();
        validator.register_segment(segment("s0", "traj-1", 0, true));
        validator.register_segment(segment("s1", "traj-1", 1, false));
        validator.register_segment(segment("s0", "traj-2", 0, true));
        assert_eq!(validator.segment_count("traj-1"), 2, "铁律9 同轨迹分组");
        assert_eq!(validator.segment_count("traj-2"), 1);
        assert_eq!(validator.trajectory_count(), 2);
    }

    #[test]
    fn register_idempotent() {
        let mut validator = SegmentAwareValidator::new();
        validator.register_segment(segment("s0", "traj-1", 0, true));
        validator.register_segment(segment("s0", "traj-1", 0, true));
        assert_eq!(validator.segment_count("traj-1"), 1, "重复注册幂等");
    }

    #[test]
    fn validate_segment_three_levels() {
        let mut validator = SegmentAwareValidator::new();
        let seg = segment("s0", "traj-1", 0, true);
        validator.register_segment(seg.clone());
        // 输出含逻辑标记 + PASS → 三级全过
        let result = validator.validate_segment(&seg, "fn main() { PASS }", 10);
        assert!(result.syntax_pass);
        assert!(result.logic_pass);
        assert!(result.sandbox_pass);
        assert!(result.is_anchor);
        assert_eq!(result.parent_traj_id.as_ref(), "traj-1");
    }

    #[test]
    fn validate_segment_auto_registers_orphan() {
        let mut validator = SegmentAwareValidator::new();
        // 孤儿分段（未注册）自动注册（容错）
        let seg = segment("s-orphan", "traj-x", 0, false);
        validator.validate_segment(&seg, "fn f() {}", 0);
        assert_eq!(validator.segment_count("traj-x"), 1);
    }

    #[test]
    fn validate_records_process_reward() {
        let mut validator = SegmentAwareValidator::new();
        let seg = segment("s0", "traj-1", 0, false);
        validator.register_segment(seg.clone());
        let result = validator.validate_segment(&seg, "fn main() { PASS }", 0);
        let states = validator.reward_states("traj-1").expect("已注册");
        assert_eq!(states[0].process_reward, result.segment_reward);
    }

    #[test]
    fn broadcast_anchor_gets_final_reward() {
        let mut validator = SegmentAwareValidator::new();
        validator.register_segment(segment("s0", "traj-1", 0, true));
        let affected = validator
            .broadcast_final_reward("traj-1", 1.0)
            .expect("已知轨迹");
        assert_eq!(affected, 1);
        let states = validator.reward_states("traj-1").expect("已注册");
        // anchor 直接承载终局奖励
        assert_eq!(states[0].final_reward, Some(1.0));
    }

    #[test]
    fn broadcast_non_anchor_propagates_03() {
        let mut validator = SegmentAwareValidator::new();
        validator.register_segment(segment("s0", "traj-1", 0, true));
        validator.register_segment(segment("s1", "traj-1", 1, false));
        // 先验证非 anchor 段记录 process_reward（syntax+logic 通过 = 0.5+1.0=1.5）
        let non_anchor = segment("s1", "traj-1", 1, false);
        let result = validator.validate_segment(&non_anchor, "fn f() {}", 0);
        validator
            .broadcast_final_reward("traj-1", 2.0)
            .expect("已知轨迹");
        let states = validator.reward_states("traj-1").expect("已注册");
        let s1 = states
            .iter()
            .find(|s| s.segment_id.as_ref() == "s1")
            .unwrap();
        // 传播公式: process_reward + final_reward × 0.3
        let expected = result.segment_reward + 2.0 * 0.3;
        assert!(
            (s1.final_reward.unwrap() - expected).abs() < 1e-6,
            "非 anchor 传播系数 0.3（实际 {:?}，期望 {expected}）",
            s1.final_reward
        );
    }

    #[test]
    fn broadcast_unknown_trajectory_errors() {
        // 防御分支: 未知轨迹返回错误（替换原型 unwrap）
        let mut validator = SegmentAwareValidator::new();
        let err = validator
            .broadcast_final_reward("ghost", 1.0)
            .expect_err("未知轨迹应报错");
        assert!(matches!(err, SegmentValidationError::UnknownTrajectory(_)));
    }

    #[test]
    fn multi_trajectory_isolation() {
        let mut validator = SegmentAwareValidator::new();
        validator.register_segment(segment("s0", "traj-a", 0, true));
        validator.register_segment(segment("s0", "traj-b", 0, true));
        validator
            .broadcast_final_reward("traj-a", 5.0)
            .expect("已知");
        // traj-b 不受 traj-a 广播影响
        let states_b = validator.reward_states("traj-b").expect("已注册");
        assert_eq!(states_b[0].final_reward, None, "多轨迹奖励隔离");
    }
}
