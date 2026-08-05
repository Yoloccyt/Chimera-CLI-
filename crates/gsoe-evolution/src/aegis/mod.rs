//! AEGIS-lite 四阶段进化流水线 — Digester→Planner→Evolver→Critic(polish-v2.7 Phase 2)
//!
//! 对应架构层:L5 Knowledge(gsoe-evolution 子模块)
//! 对应 ADR:ADR-050(AEGIS-lite 降级设计)+ ADR-049 决策 1(落点裁决)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §9.1(小米 HarnessX AEGIS)
//!
//! # R2 冻结声明(ADR-042)
//!
//! 本模块为 AEGIS 的**规则/统计驱动降级实现**(AEGIS-lite):
//! - 全程**无梯度更新、无策略网络、无约束 RL**(FormalVerifier 落地前无条件冻结)
//! - Digester = 纯规则统计聚类;Planner = 静态规则表;
//!   Evolver = 仅生成 HarnessSpec 参数变体(不生成代码);
//!   Critic = CiGate 回归门 + 变异幅度启发式
//! - R2 解冻后 Planner 升级为学习驱动须新 ADR 评审(ADR-050 §3)
//!
//! # 四阶段数据流
//!
//! ```text
//! Vec<TrajectoryOutcome>(上层投喂,L9 → L5 经调用方转换)
//!     │ TrajectoryDigester::digest        [Stage 1: 压缩轨迹,聚类失败模式]
//!     ▼
//! DigestedTrajectories
//!     │ AdaptationPlanner::plan           [Stage 2: 规则表 → 适应方向]
//!     ▼
//! AdaptationPlan
//!     │ SpecEvolver::generate             [Stage 3: 生成 HarnessSpec 变体候选]
//!     ▼
//! Vec<SpecCandidate>
//!     │ AegisCritic::select               [Stage 4: CiGate + 变异幅度守护]
//!     ▼
//! CriticVerdict(接受 ≤1 个变体 → SpecRegistry::register 登记谱系)
//! ```

use nexus_contracts::HarnessSpec;
use serde::{Deserialize, Serialize};

use crate::ci_gate::CiGate;
use crate::error::GsoeError;

pub mod critic;
pub mod digester;
pub mod evolver;
pub mod planner;

pub use critic::{AegisCritic, CriticVerdict, RejectedCandidate};
pub use digester::{DigestedTrajectories, FailurePattern, TrajectoryDigester};
pub use evolver::{SpecCandidate, SpecEvolver};
pub use planner::{AdaptationDirection, AdaptationPlan, AdaptationPlanner};

// ============================================================
// 输入类型 — 轨迹结局摘要
// ============================================================

/// 轨迹结局摘要 — AEGIS 流水线的输入单元
///
/// WHY 本地定义而非复用 L9 类型(ADR-050 决策 3):
/// L5 不得向上依赖 quest-engine/chimera-mas 的轨迹类型(§2.2 依赖铁律),
/// 上层调用方负责将 Quest 轨迹降维转换为本摘要后投喂。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryOutcome {
    /// 轨迹唯一标识(通常为 quest_id 或 task_id)
    pub trajectory_id: String,
    /// 轨迹是否成功结束
    pub success: bool,
    /// 失败错误类别(成功时为 None;如 "timeout" / "verification_failed")
    pub error_kind: Option<String>,
    /// 失败发生位置(成功时为 None;如 "pvl-layer::verifier")
    pub error_location: Option<String>,
    /// 轨迹执行时长(毫秒)
    pub duration_ms: u64,
}

impl TrajectoryOutcome {
    /// 构造成功轨迹摘要
    pub fn succeeded(trajectory_id: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            trajectory_id: trajectory_id.into(),
            success: true,
            error_kind: None,
            error_location: None,
            duration_ms,
        }
    }

    /// 构造失败轨迹摘要
    pub fn failed(
        trajectory_id: impl Into<String>,
        error_kind: impl Into<String>,
        error_location: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            trajectory_id: trajectory_id.into(),
            success: false,
            error_kind: Some(error_kind.into()),
            error_location: Some(error_location.into()),
            duration_ms,
        }
    }
}

// ============================================================
// AegisPipeline — 四阶段编排
// ============================================================

/// AEGIS-lite 流水线 — 串联 Digester→Planner→Evolver→Critic
///
/// # 使用模式
///
/// 上层(L9 编排器)周期性调用 [`run_once`](Self::run_once) 投喂轨迹批次;
/// 返回的变体(若有)由调用方经 `SpecRegistry::register` 登记谱系并走
/// `set_candidate` → `promote_candidate` 灰度(ADR-050 决策 5,
/// AEGIS 不新增回滚机制)。
pub struct AegisPipeline {
    digester: TrajectoryDigester,
    planner: AdaptationPlanner,
    evolver: SpecEvolver,
    critic: AegisCritic,
}

impl AegisPipeline {
    /// 创建默认配置的流水线
    pub fn new() -> Self {
        Self {
            digester: TrajectoryDigester::new(),
            planner: AdaptationPlanner::new(),
            evolver: SpecEvolver::new(),
            critic: AegisCritic::new(),
        }
    }

    /// 执行一轮四阶段进化,返回 Critic 裁决
    ///
    /// # 参数
    /// - `trajectories`:本轮观测的轨迹结局批次(空批次直接返回空裁决)
    /// - `base_spec`:当前活跃的基线 HarnessSpec(变体的变异起点)
    /// - `ci_gate`:CI 执行门(生产用 `CargoCiGate`,测试用 `MockCiGate`)
    ///
    /// # 错误
    /// - `GsoeError`:CI 门执行本身失败(如 cargo 不可达)时上抛
    pub async fn run_once(
        &mut self,
        trajectories: &[TrajectoryOutcome],
        base_spec: &HarnessSpec,
        ci_gate: &dyn CiGate,
    ) -> Result<CriticVerdict, GsoeError> {
        // 空批次快速返回:无观测数据不进化(证据纪律,与 RuntimeAuditor 中性评分同理)
        if trajectories.is_empty() {
            return Ok(CriticVerdict::empty());
        }

        // Stage 1: 轨迹消化(统计聚类)
        let digested = self.digester.digest(trajectories);

        // Stage 2: 适应规划(规则表)
        let plan = self.planner.plan(&digested);

        // Stage 3: 变体生成(仅 HarnessSpec 参数变异)
        let candidates = self.evolver.generate(&plan, base_spec);

        // Stage 4: 审查裁决(CiGate + 变异幅度守护)
        self.critic.select(candidates, base_spec, ci_gate).await
    }
}

impl Default for AegisPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci_gate::MockCiGate;
    use nexus_contracts::{HarnessMeta, HarnessSpec, RetryPolicy};

    /// 构造最小合法基线 spec(测试共用)
    pub(crate) fn base_spec() -> HarnessSpec {
        HarnessSpec {
            meta: HarnessMeta {
                name: "aegis-test-spec".into(),
                version: 1,
                immutable: false,
                parent: None,
                task_type: Some("code_fix".into()),
            },
            contracts: vec![],
            hops: vec![],
            retry: RetryPolicy::default(),
            auxiliary: None,
        }
    }

    #[tokio::test]
    async fn test_pipeline_empty_batch_returns_empty_verdict() {
        let mut pipeline = AegisPipeline::new();
        let gate = MockCiGate::with_passing_result();
        let verdict = pipeline
            .run_once(&[], &base_spec(), &gate)
            .await
            .expect("空批次不应报错");
        assert!(verdict.accepted.is_none());
        assert!(verdict.rejected.is_empty());
    }

    #[tokio::test]
    async fn test_pipeline_full_flow_produces_variant_on_timeout_failures() {
        let mut pipeline = AegisPipeline::new();
        let gate = MockCiGate::with_passing_result();
        // 高失败率 + timeout 主导 → 应产出放宽重试的变体
        let trajectories: Vec<TrajectoryOutcome> = (0..10)
            .map(|i| {
                if i < 6 {
                    TrajectoryOutcome::failed(format!("t{i}"), "timeout", "pvl-layer", 5_000)
                } else {
                    TrajectoryOutcome::succeeded(format!("t{i}"), 1_000)
                }
            })
            .collect();

        let verdict = pipeline
            .run_once(&trajectories, &base_spec(), &gate)
            .await
            .expect("流水线不应报错");
        let accepted = verdict.accepted.expect("timeout 主导失败应产出变体");
        // 变体版本递增且 parent 指向基线
        assert_eq!(accepted.meta.version, 2);
        assert_eq!(accepted.meta.parent, Some(1));
        // 放宽重试:max_attempts 高于基线
        assert!(accepted.retry.max_attempts > RetryPolicy::default().max_attempts);
    }

    #[tokio::test]
    async fn test_pipeline_healthy_trajectories_no_variant() {
        let mut pipeline = AegisPipeline::new();
        let gate = MockCiGate::with_passing_result();
        // 全部成功 → NoChange,不产出变体
        let trajectories: Vec<TrajectoryOutcome> = (0..10)
            .map(|i| TrajectoryOutcome::succeeded(format!("t{i}"), 800))
            .collect();

        let verdict = pipeline
            .run_once(&trajectories, &base_spec(), &gate)
            .await
            .expect("流水线不应报错");
        assert!(verdict.accepted.is_none(), "健康轨迹不应触发进化");
    }
}
