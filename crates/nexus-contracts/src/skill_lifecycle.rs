//! Skill 生命周期契约 — MSCE 融合（设计文档 §5.5）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §5.5
//! 对应论文: MSCE（Skill 生命周期: probationary → active → archived）
//!
//! # 核心职责
//!
//! 承载技能生命周期状态机契约，使 L5 skill-graph / L6 skills-progressive-loader
//! 按统一契约管理技能状态：
//!
//! | 类型 | 职责 | 消费层 |
//! |------|------|--------|
//! | [`SkillLifecycleState`] | 三态生命周期（试用/激活/归档） | L5 skill-graph lifecycle |
//! | [`SkillLifecycleContract`] | 生命周期契约（阈值/计数/时间线） | L5 skill-graph / L6 loader |
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型 + 纯函数**: 仅类型定义与状态转移判定纯函数（无 IO 无副作用）
//! - **零 crate 依赖**: 仅 `serde` derive
//! - **`Box<str>` 优化**: 不可变文本字段采用堆紧凑形态
//! - **状态机判定**: `next_state` 为纯函数（输入状态 + 计数 → 输出状态），
//!   与 `archive_monotonicity` 同类先例（ADR-033 纯函数例外）

use serde::{Deserialize, Serialize};

// ============================================================
// 生命周期状态
// ============================================================

/// Skill 生命周期状态 — 三态状态机（MSCE）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLifecycleState {
    /// 试用期 — 刚生成，待验证（不可检索使用）
    Probationary,
    /// 激活 — 通过验证，可检索使用
    Active,
    /// 归档 — 长期未使用或负面反馈，降级存储
    Archived,
}

impl SkillLifecycleState {
    /// 是否可被检索使用（仅 Active）
    pub fn is_retrievable(&self) -> bool {
        matches!(self, SkillLifecycleState::Active)
    }
}

// ============================================================
// 生命周期契约
// ============================================================

/// Skill 生命周期契约 — 状态机参数与时间线
///
/// 转移规则（由 `next_state` 纯函数判定）:
/// - `Probationary → Active`: 成功计数 ≥ activation_threshold（默认 3）
/// - `Active → Archived`: 失败计数 ≥ archive_threshold（默认 5）
/// - `Probationary → Archived`: 失败计数 ≥ archive_threshold
/// - 归档为终态（MSCE 状态机无复活路径，重新生成即新 skill_id）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillLifecycleContract {
    /// 技能 ID
    pub skill_id: Box<str>,
    /// 当前状态
    pub state: SkillLifecycleState,
    /// 试用期开始时间（Unix 毫秒）
    pub probation_start: u64,
    /// 试用期结束时间（None = 仍处于试用期）
    pub probation_end: Option<u64>,
    /// 激活所需成功次数（默认 3）
    pub activation_threshold: u32,
    /// 累计成功次数
    pub success_count: u32,
    /// 累计失败次数
    pub failure_count: u32,
    /// 归档所需失败次数（默认 5）
    pub archive_threshold: u32,
    /// 最近使用时间（Unix 毫秒）
    pub last_used: u64,
}

impl SkillLifecycleContract {
    /// 创建试用期契约（新技能默认入口）
    pub fn new_probationary(skill_id: &str, now_ms: u64) -> Self {
        Self {
            skill_id: Box::from(skill_id),
            state: SkillLifecycleState::Probationary,
            probation_start: now_ms,
            probation_end: None,
            activation_threshold: 3,
            success_count: 0,
            failure_count: 0,
            archive_threshold: 5,
            last_used: now_ms,
        }
    }

    /// 判定下一次状态转移（纯函数，铁律4）
    ///
    /// 输入当前状态与计数 → 输出目标状态；不修改自身。
    /// 返回 `None` 表示状态保持不变。
    pub fn next_state(&self) -> Option<SkillLifecycleState> {
        match self.state {
            // 试用期: 成功达标 → 激活；失败超限 → 归档
            SkillLifecycleState::Probationary => {
                if self.success_count >= self.activation_threshold {
                    Some(SkillLifecycleState::Active)
                } else if self.failure_count >= self.archive_threshold {
                    Some(SkillLifecycleState::Archived)
                } else {
                    None
                }
            }
            // 激活: 失败超限 → 归档
            SkillLifecycleState::Active => {
                if self.failure_count >= self.archive_threshold {
                    Some(SkillLifecycleState::Archived)
                } else {
                    None
                }
            }
            // 归档: 终态，无转移
            SkillLifecycleState::Archived => None,
        }
    }

    /// 记录一次成功（返回转移后的新契约，不可变风格）
    pub fn record_success(&self, now_ms: u64) -> Self {
        let mut next = self.clone();
        next.success_count += 1;
        next.last_used = now_ms;
        if let Some(target) = next.next_state() {
            next.state = target;
            if target == SkillLifecycleState::Active && next.probation_end.is_none() {
                next.probation_end = Some(now_ms);
            }
        }
        next
    }

    /// 记录一次失败（返回转移后的新契约，不可变风格）
    pub fn record_failure(&self, now_ms: u64) -> Self {
        let mut next = self.clone();
        next.failure_count += 1;
        next.last_used = now_ms;
        if let Some(target) = next.next_state() {
            next.state = target;
            if target == SkillLifecycleState::Archived && next.probation_end.is_none() {
                next.probation_end = Some(now_ms);
            }
        }
        next
    }

    /// 是否可被检索使用（状态快捷访问）
    pub fn is_retrievable(&self) -> bool {
        self.state.is_retrievable()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 状态枚举 ----------

    #[test]
    fn lifecycle_state_closed_enum() {
        let all = [
            SkillLifecycleState::Probationary,
            SkillLifecycleState::Active,
            SkillLifecycleState::Archived,
        ];
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn lifecycle_state_retrievability() {
        assert!(SkillLifecycleState::Active.is_retrievable());
        assert!(!SkillLifecycleState::Probationary.is_retrievable());
        assert!(!SkillLifecycleState::Archived.is_retrievable());
    }

    // ---------- 状态机转移 ----------

    #[test]
    fn probationary_activates_after_threshold() {
        // 成功 3 次（默认阈值）→ 激活
        let contract = SkillLifecycleContract::new_probationary("skill-1", 100);
        let mut current = contract;
        for i in 1..=3 {
            current = current.record_success(100 + i);
        }
        assert_eq!(current.state, SkillLifecycleState::Active);
        assert_eq!(current.success_count, 3);
        assert!(current.probation_end.is_some());
        assert!(current.is_retrievable());
    }

    #[test]
    fn probationary_archives_after_failures() {
        // 失败 5 次（默认阈值）→ 归档
        let contract = SkillLifecycleContract::new_probationary("skill-2", 100);
        let mut current = contract;
        for i in 1..=5 {
            current = current.record_failure(100 + i);
        }
        assert_eq!(current.state, SkillLifecycleState::Archived);
        assert_eq!(current.failure_count, 5);
        assert!(!current.is_retrievable());
    }

    #[test]
    fn active_archives_after_failures() {
        // 已激活技能失败 5 次 → 归档
        let contract = SkillLifecycleContract::new_probationary("skill-3", 100);
        let mut current = contract;
        for i in 1..=3 {
            current = current.record_success(100 + i);
        }
        assert_eq!(current.state, SkillLifecycleState::Active);
        for i in 1..=5 {
            current = current.record_failure(200 + i);
        }
        assert_eq!(current.state, SkillLifecycleState::Archived);
        // 归档为终态: 再成功也不复活
        let after = current.record_success(300);
        assert_eq!(after.state, SkillLifecycleState::Archived);
    }

    #[test]
    fn below_threshold_stays_probationary() {
        // 成功 2 次 < 阈值 3 → 仍试用期
        let contract = SkillLifecycleContract::new_probationary("skill-4", 100);
        let mut current = contract;
        for i in 1..=2 {
            current = current.record_success(100 + i);
        }
        assert_eq!(current.state, SkillLifecycleState::Probationary);
        assert!(current.probation_end.is_none());
    }

    #[test]
    fn custom_thresholds_respected() {
        // 自定义阈值: 激活需 5 次成功
        let mut contract = SkillLifecycleContract::new_probationary("skill-5", 100);
        contract.activation_threshold = 5;
        let mut current = contract;
        for i in 1..=4 {
            current = current.record_success(100 + i);
        }
        assert_eq!(current.state, SkillLifecycleState::Probationary);
        current = current.record_success(200);
        assert_eq!(current.state, SkillLifecycleState::Active);
    }

    #[test]
    fn next_state_is_pure_function() {
        // 铁律4: 同一输入多次调用结果一致
        let contract = SkillLifecycleContract::new_probationary("skill-6", 100);
        assert_eq!(contract.next_state(), contract.next_state());
        let active = contract
            .record_success(200)
            .record_success(300)
            .record_success(400);
        assert_eq!(active.next_state(), active.next_state());
    }

    // ---------- 序列化 ----------

    #[test]
    fn skill_lifecycle_json_roundtrip() {
        let contract = SkillLifecycleContract::new_probationary("skill-1", 1_700_000_000_000);
        let json = serde_json::to_string(&contract).expect("JSON 序列化失败");
        let decoded: SkillLifecycleContract =
            serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, contract);
    }

    #[test]
    fn skill_lifecycle_msgpack_roundtrip() {
        let contract = SkillLifecycleContract::new_probationary("skill-2", 1_700_000_000_000);
        let bytes = rmp_serde::to_vec(&contract).expect("MsgPack 序列化失败");
        let decoded: SkillLifecycleContract =
            rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        assert_eq!(decoded, contract);
    }

    #[test]
    fn skill_lifecycle_wire_format_frozen() {
        let contract = SkillLifecycleContract::new_probationary("skill-1", 1_700_000_000_000);
        let json = serde_json::to_string(&contract).expect("JSON 序列化失败");
        assert!(json.contains("\"state\":\"probationary\""));
        assert!(json.contains("\"activation_threshold\":3"));
        assert!(json.contains("\"archive_threshold\":5"));
    }
}
