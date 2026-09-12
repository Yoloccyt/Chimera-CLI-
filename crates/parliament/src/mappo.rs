//! MAPPO：多智能体 PPO（Milestone C-6，设计 §12.1 目标形态）
//!
//! 规则式占位实现（项目先例：MTPE 伪预测 / GSOE 规则式进化策略——替换为生产
//! 实现时不得破坏既有接口契约）：
//! - `ActorNetwork`：特征加权线性打分 → sigmoid 置信度（确定性策略，不训练）
//! - `CentralizedCritic`：Welford 在线统计（baseline=均值 / std=样本标准差）
//! - `compute_advantages`：Agent-wise 归一化 `(r − baseline) / (std + 1e-8)`（Dr.MAS 修复）
//!
//! R2 冻结（ADR-042）：本模块不触神经网络训练面；Critic 观察仅更新统计量，
//! 不做梯度更新。生产替换方向 = 真实 Actor/Critic 网络 + PPO 损失更新。

/// 议会三角色 agent 身份（MAPPO actor 索引与 SHARP 归因共用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentRole {
    /// 质疑者：挑战提案、否决检查
    Skeptic,
    /// 安全官：安全边界放行/拦截
    Security,
    /// 执行者：效率与执行路径选择
    Execution,
}

impl AgentRole {
    /// 固定三人组（设计 §12.1 的三元联合决策）
    pub const ALL: [AgentRole; 3] = [
        AgentRole::Skeptic,
        AgentRole::Security,
        AgentRole::Execution,
    ];

    /// 角色名（SHARP 归因键与 coalition 成员名）
    pub fn as_str(self) -> &'static str {
        match self {
            AgentRole::Skeptic => "Skeptic",
            AgentRole::Security => "Security",
            AgentRole::Execution => "Execution",
        }
    }
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 二值动作：approve（批准/放行/执行）+ 置信度（规则式策略输出）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Action {
    /// 决策方向：true=通过，false=否决/拦截/暂停
    pub approve: bool,
    /// 置信度 ∈ [0,1]（sigmoid 打分）
    pub confidence: f32,
}

/// 规则式 Actor 网络（占位）：`score = Σ weights·obs`，`approve = score ≥ 0`
///
/// 特征权重由外部注入（配置/专家先验），当前为确定性打分——不训练。
#[derive(Debug, Clone)]
pub struct ActorNetwork {
    weights: Vec<f32>,
}

impl ActorNetwork {
    /// 构造规则式策略；权重维度须与观测特征维度一致
    pub fn new(weights: Vec<f32>) -> Self {
        Self { weights }
    }

    /// 观测 → 动作（线性打分 + sigmoid 置信度）
    pub fn predict(&self, obs: &[f32]) -> Action {
        let score = self
            .weights
            .iter()
            .zip(obs.iter())
            .map(|(w, o)| w * o)
            .sum::<f32>();
        Action {
            approve: score >= 0.0,
            confidence: sigmoid(score),
        }
    }
}

/// 逻辑斯蒂函数:委托至 nexus_contracts::util::sigmoid(全程 f32,项目红线 §6.2 #6)
use nexus_contracts::util::sigmoid;

/// 三元组观测：Skeptic/Security/Execution 各自特征向量
#[derive(Debug, Clone)]
pub struct ParliamentState {
    /// Skeptic 观测（如提案风险特征）
    pub skeptic_obs: Vec<f32>,
    /// Security 观测（如边界威胁特征）
    pub security_obs: Vec<f32>,
    /// Execution 观测（如效率/资源特征）
    pub execution_obs: Vec<f32>,
}

/// 三元联合动作（设计 §12.1 `JointAction`）
#[derive(Debug, Clone)]
pub struct JointAction {
    /// Skeptic 动作
    pub skeptic: Action,
    /// Security 动作
    pub security: Action,
    /// Execution 动作
    pub execution: Action,
}

/// 三元奖励（SHARP 分解输出，设计 §12.1 `AgentRewards`）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentRewards {
    /// Skeptic 通道奖励
    pub skeptic: f32,
    /// Security 通道奖励
    pub security: f32,
    /// Execution 通道奖励
    pub execution: f32,
}

/// 三元优势（Agent-wise 归一化后，设计 §12.1 `AgentAdvantages`）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentAdvantages {
    /// Skeptic 归一化优势
    pub skeptic: f32,
    /// Security 归一化优势
    pub security: f32,
    /// Execution 归一化优势
    pub execution: f32,
}

/// Welford 在线统计（单变量均值/方差，O(1) 更新，数值稳定）
///
/// WHY 不用简单累加：均值漂移场景下 `Σx² − (Σx)²/n` 会灾难性消减。
#[derive(Debug, Clone, Default)]
struct WelfordStat {
    count: u64,
    mean: f64,
    m2: f64,
}

impl WelfordStat {
    fn update(&mut self, x: f32) {
        self.count += 1;
        let x = f64::from(x);
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
    }

    fn mean(&self) -> f32 {
        self.mean as f32
    }

    /// 样本标准差（count < 2 时为 0——单样本无方差信息）
    fn std(&self) -> f32 {
        if self.count < 2 {
            0.0
        } else {
            (self.m2 / (self.count - 1) as f64).sqrt() as f32
        }
    }
}

/// 共享 Critic（全局信息）：每 agent 独立 baseline/std 在线统计
///
/// 设计 §12.1：`CentralizedCritic` 以全局信息为三 agent 提供各自基准。
#[derive(Debug, Clone, Default)]
pub struct CentralizedCritic {
    per_agent: [WelfordStat; 3],
}

impl CentralizedCritic {
    /// 观察一轮三元奖励（只更新统计量，非训练）
    pub fn observe(&mut self, rewards: &AgentRewards) {
        self.per_agent[0].update(rewards.skeptic);
        self.per_agent[1].update(rewards.security);
        self.per_agent[2].update(rewards.execution);
    }

    /// Skeptic baseline（均值）
    pub fn baseline_skeptic(&self) -> f32 {
        self.per_agent[0].mean()
    }
    /// Security baseline（均值）
    pub fn baseline_security(&self) -> f32 {
        self.per_agent[1].mean()
    }
    /// Execution baseline（均值）
    pub fn baseline_execution(&self) -> f32 {
        self.per_agent[2].mean()
    }
    /// Skeptic 标准差
    pub fn std_skeptic(&self) -> f32 {
        self.per_agent[0].std()
    }
    /// Security 标准差
    pub fn std_security(&self) -> f32 {
        self.per_agent[1].std()
    }
    /// Execution 标准差
    pub fn std_execution(&self) -> f32 {
        self.per_agent[2].std()
    }
}

/// MAPPO：多智能体 PPO（规则式占位，设计 §12.1）
///
/// 三个 actor 各自决策 + 共享 critic 提供全局基准；actor 权重由构造注入。
/// 约定：`actor_weights` 必须恰好 3 组（Skeptic/Security/Execution 顺序）。
#[derive(Debug, Clone)]
pub struct MAPPO {
    actors: Vec<ActorNetwork>,
    critic: CentralizedCritic,
}

impl MAPPO {
    /// 构造：3 组 actor 权重（每组与对应观测同维）+ 空 critic
    pub fn new(actor_weights: Vec<Vec<f32>>) -> Self {
        let actors = actor_weights.into_iter().map(ActorNetwork::new).collect();
        Self {
            actors,
            critic: CentralizedCritic::default(),
        }
    }

    /// 联合决策：三 actor 各自 `predict`（设计 §12.1 `joint_decision`）
    pub fn joint_decision(&self, state: &ParliamentState) -> JointAction {
        JointAction {
            skeptic: self.actors[0].predict(&state.skeptic_obs),
            security: self.actors[1].predict(&state.security_obs),
            execution: self.actors[2].predict(&state.execution_obs),
        }
    }

    /// Agent-wise 优势归一化（Dr.MAS 修复）：
    /// `(r − baseline) / (std + 1e-8)`——每 agent 独立尺度，非全局共享统计
    pub fn compute_advantages(&self, rewards: &AgentRewards) -> AgentAdvantages {
        AgentAdvantages {
            skeptic: (rewards.skeptic - self.critic.baseline_skeptic())
                / (self.critic.std_skeptic() + 1e-8),
            security: (rewards.security - self.critic.baseline_security())
                / (self.critic.std_security() + 1e-8),
            execution: (rewards.execution - self.critic.baseline_execution())
                / (self.critic.std_execution() + 1e-8),
        }
    }

    /// Critic 观察一轮三元奖励（只读统计，不触训练面）
    pub fn observe(&mut self, rewards: &AgentRewards) {
        self.critic.observe(rewards);
    }

    /// 共享 critic 引用（外部读取 baseline/std）
    pub fn critic(&self) -> &CentralizedCritic {
        &self.critic
    }
}
