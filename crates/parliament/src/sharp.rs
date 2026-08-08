//! SHARP：Shapley 信用分配（Milestone C-6，设计 §12.1 目标形态）
//!
//! 精确 Shapley 值计算（Ω₅ Credit）：
//! `φᵢ = Σ_{S ⊆ N\{i}} |S|!(n−|S|−1)!/n! × (v(S∪{i}) − v(S))`
//!
//! 数学保证（测试断言）：
//! - **效率公理** Σφᵢ = v(N) − v(∅)——总信用恰等于全体协作增量
//! - **对称性**：价值贡献相同 → 信用相同
//!
//! 工程约束：
//! - `n > MAX_EXACT_AGENTS` 返回 `None`（2^n 幂集指数爆炸保护）
//! - 未登记联盟价值视为 0（空集恒 0）
//! - 三元分解奖励：`global(0.3) + shapley(0.5) + process(0.2)` 三通道

use crate::mappo::{AgentRewards, AgentRole};
use std::collections::HashMap;

/// 精确 Shapley 的参与者数上限：2^7 = 128 子集/agent，毫秒级可算
///
/// WHY 8：n=8 时每 agent 枚举 2^7=128 联盟，合计 384 次联盟求值，
/// 仍属廉价计算；n>8 转蒙特卡洛近似是生产替换方向（本实现只做精确）。
pub const MAX_EXACT_AGENTS: usize = 8;

/// 验证阶段（设计 §12.1 `VerificationResult` 的目标形态四档：
/// SyntaxPass/LogicPass/SandboxPass/Failed）
///
/// WHY 不复用 `nexus_contracts::VerificationResult`：其实际变体为
/// Satisfied/Violated/Skipped（形式化采样语义），与设计的验证阶段推进
/// 语义（语法→逻辑→沙箱）不一致，混用会造成 process 通道得分失真。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStage {
    /// 语法验证通过
    SyntaxPass,
    /// 逻辑验证通过
    LogicPass,
    /// 沙箱执行验证通过
    SandboxPass,
    /// 验证失败（任一档未过）
    Failed,
}

impl VerificationStage {
    /// 设计 §12.1 process 通道得分：SyntaxPass 0.5 / LogicPass 1.0 /
    /// SandboxPass 1.5 / Failed −2.0
    pub fn process_score(self) -> f32 {
        match self {
            VerificationStage::SyntaxPass => 0.5,
            VerificationStage::LogicPass => 1.0,
            VerificationStage::SandboxPass => 1.5,
            VerificationStage::Failed => -2.0,
        }
    }
}

/// 环境反馈结果（三元分解奖励的 process 通道输入）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// 本次协作的验证阶段结果
    pub verification: VerificationStage,
}

/// SHARP：Shapley 信用分配器（设计 §12.1）
///
/// `coalition_values` 登记联盟价值 v(S)；成员顺序无关（内部排序归一）。
#[derive(Debug, Clone, Default)]
pub struct SHARP {
    coalition_values: HashMap<Vec<String>, f32>,
}

impl SHARP {
    /// 空分配器（所有联盟价值为 0）
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记联盟价值 v(S)；重复登记以最近一次为准
    pub fn set_coalition_value(
        &mut self,
        members: impl IntoIterator<Item = impl Into<String>>,
        value: f32,
    ) {
        let mut key: Vec<String> = members.into_iter().map(Into::into).collect();
        // WHY 排序：Shapley 联盟是集合语义，`{"Skeptic","Security"}` 与
        // `{"Security","Skeptic"}` 必须命中同一价值（测试对称性依赖此归一）
        key.sort_unstable();
        self.coalition_values.insert(key, value);
    }

    /// 查询联盟价值；未登记返回 0.0（含空集恒 0）
    pub fn coalition_value(&self, members: &[String]) -> f32 {
        let mut key = members.to_vec();
        key.sort_unstable();
        self.coalition_values.get(&key).copied().unwrap_or(0.0)
    }

    /// 精确 Shapley 值 φ(agent_id)（设计 §12.1 `compute_shapley`）
    ///
    /// - `n > MAX_EXACT_AGENTS` → `None`（指数爆炸保护）
    /// - agent 不在集合中 → `Some(0.0)`（无贡献即无归因）
    /// - 幂集枚举边际贡献 × factorial 权重
    pub fn compute_shapley(&self, agent_id: &str, all: &[String]) -> Option<f32> {
        let n = all.len();
        if n > MAX_EXACT_AGENTS {
            return None;
        }
        if !all.iter().any(|a| a == agent_id) {
            return Some(0.0);
        }
        let others: Vec<String> = all.iter().filter(|a| *a != agent_id).cloned().collect();
        // f64 中间精度：factorial 权重与边际乘积在 f32 下可能消减
        let mut shapley = 0.0f64;
        for mask in 0..(1u32 << others.len()) {
            let mut subset: Vec<String> = Vec::with_capacity(others.len());
            for (i, other) in others.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    subset.push(other.clone());
                }
            }
            let v_without = f64::from(self.coalition_value(&subset));
            let mut with = subset.clone();
            with.push(agent_id.to_string());
            let v_with = f64::from(self.coalition_value(&with));
            let marginal = v_with - v_without;
            let s = subset.len();
            let weight = factorial(s) * factorial(n - s - 1) / factorial(n);
            shapley += weight * marginal;
        }
        Some(shapley as f32)
    }

    /// 三元分解奖励（设计 §12.1 `decompose`）：
    /// `reward_i = team_reward×0.3 + φ_i×0.5 + process×0.2`
    ///
    /// - **global 通道**（0.3）：团队总奖励均分——协作共识红利
    /// - **shapley 通道**（0.5）：个体边际贡献归因——Ω₅ Credit
    /// - **process 通道**（0.2）：验证阶段得分——质量反馈
    pub fn decompose(&self, team_reward: f32, outcome: &Outcome) -> AgentRewards {
        let global = team_reward * 0.3;
        let process = outcome.verification.process_score();
        let all: Vec<String> = AgentRole::ALL
            .iter()
            .map(|r| r.as_str().to_string())
            .collect();
        let mut rewards = AgentRewards {
            skeptic: global,
            security: global,
            execution: global,
        };
        for (role, slot) in [
            (AgentRole::Skeptic, &mut rewards.skeptic),
            (AgentRole::Security, &mut rewards.security),
            (AgentRole::Execution, &mut rewards.execution),
        ] {
            let phi = self.compute_shapley(role.as_str(), &all).unwrap_or(0.0);
            *slot += phi * 0.5 + process * 0.2;
        }
        rewards
    }
}

/// 阶乘（f64 中间精度；n ≤ 8 时 40320，无溢出风险）
fn factorial(n: usize) -> f64 {
    let mut r = 1.0f64;
    for i in 2..=n {
        r *= i as f64;
    }
    r
}
