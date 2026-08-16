//! RL 客户端骨架 — 策略推理接口 + 规则回退（设计文档 §6.4 + §17）
//!
//! 对应架构层: **L1 Core**（nexus-core 新增模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §6.4 / §17.2
//! 对应规划: RL 架构预留（Rust 侧接口设计 · Python 侧 v4.0 计划）
//!
//! # 核心职责
//!
//! 承载 v4.0 RL 升级的策略推理客户端接口：
//!
//! | 组件 | v3.x 实现 | v4.0 升级路径 |
//! |------|-----------|-------------|
//! | [`RLClient`] | 规则策略回退（RulePolicyFallback） | gRPC 调用 Python RL Service |
//! | [`RulePolicyFallback`] | 规则策略（零 Python 依赖，铁律1） | 替换为 GrpcRLClient |
//! | [`RLError`] | 三类错误（策略缺失/网络/状态非法） | 保持 |
//!
//! # 设计约束
//!
//! - **铁律1**: 零运行时 Python 依赖——`RulePolicyFallback` 为纯 Rust 规则策略
//! - **铁律2**: 实现方可替换（RulePolicyFallback 为默认），接口同构
//! - **异步接口**: `#[async_trait]`（L1 允许异步 trait；L0 的 `RLHook` 保持
//!   同步为最小公共子集，本模块提供异步策略面）
//! - **RL 开发闸门（2026-08-16 治理决策）**: Python 侧训练服务**禁止实施**，
//!   `GrpcRLClient` 仅保留结构占位与升级路径文档；Rust 侧接口先行
//! - **依赖方向**: nexus-core（L1）→ nexus-contracts（L0）合规；
//!   本模块不引入 tonic/prost（gRPC 依赖留待 v4.0 闸门解除）

use async_trait::async_trait;
use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector, RLTrajectory, SerializedPolicy};
use thiserror::Error;

/// RL 客户端错误 — 策略推理失败的三类原因
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RLError {
    /// 策略不存在（层未注册策略）
    #[error("Policy not found: {0}")]
    PolicyNotFound(String),
    /// 网络错误（v4.0 gRPC 通道）
    #[error("Network error: {0}")]
    NetworkError(String),
    /// 状态非法（维度不匹配/数值越界）
    #[error("Invalid state: {0}")]
    InvalidState(String),
}

/// RL 客户端 — 策略推理的统一接口（v4.0 升级路径）
///
/// # v4.0 替换路径
///
/// | 方法 | v3.x（统计/规则） | v4.0（RL） |
/// |------|-----------------|-----------|
/// | `predict` | 规则策略回退 | gRPC 调用 Python RL Service |
/// | `report_experience` | 本地记录（no-op） | 发送到 Python 训练服务 |
/// | `sync_policy` | 加载本地 JSON 配置 | 从 Python 服务拉取 ONNX 模型 |
#[async_trait]
pub trait RLClient: Send + Sync {
    /// 预测动作（State → Action）
    async fn predict(&mut self, state: RLStateVector) -> Result<RLActionVector, RLError>;

    /// 上报经验轨迹（R1 数据面收集）
    async fn report_experience(&mut self, trajectory: RLTrajectory) -> Result<(), RLError>;

    /// 同步策略（从策略源加载/拉取）
    async fn sync_policy(&mut self, layer: &str) -> Result<SerializedPolicy, RLError>;
}

/// 规则策略回退 — 默认实现（铁律1: 零 Python 依赖）
///
/// v3.x 阶段所有层使用本回退；v4.0 闸门解除后由 `GrpcRLClient` 替换，
/// 接口不变、调用方零改动（铁律2: 策略可替换）。
#[derive(Debug, Clone, Copy, Default)]
pub struct RulePolicyFallback;

#[async_trait]
impl RLClient for RulePolicyFallback {
    async fn predict(&mut self, _state: RLStateVector) -> Result<RLActionVector, RLError> {
        // 规则回退: 返回固定保守动作（action_code=0, 单参数 0.1）
        // WHY 固定值: 无学习信号时保守默认（0 号动作通常为"维持现状"）
        Ok(RLActionVector::new("fallback", 0, vec![0.1]))
    }

    async fn report_experience(&mut self, _trajectory: RLTrajectory) -> Result<(), RLError> {
        // R1 数据面: 本地记录（此处 no-op——真实收集由调用方完成）
        // v4.0: 上传到训练集群
        Ok(())
    }

    async fn sync_policy(&mut self, layer: &str) -> Result<SerializedPolicy, RLError> {
        // 返回 JSON 格式的规则策略占位（版本随基线递增）
        Ok(SerializedPolicy::new(
            nexus_contracts::PolicyFormat::Json,
            vec![],
            "rule-fallback-v3.4.0",
            layer,
        ))
    }
}

// ============================================================
// GrpcRLClient 预留（v4.0）
// ============================================================

/// gRPC RL 客户端 — v4.0 预留结构（**仅占位，禁止实施**）
///
/// # RL 开发闸门（2026-08-16 治理决策）
///
/// Python 侧（RL 版）训练服务仅保留规划（C-4 协议契约不动），
/// Python 服务实体**禁止实施**；待整个 Rust 系统彻底成熟并稳定运行后
/// （R2 解冻 + 稳定性观察期通过）再开启 RL。
///
/// # v4.0 升级路径（闸门解除后）
///
/// 1. 引入 `tonic` + `prost`（gRPC 栈，feature-gated 控制 binary 体积）
/// 2. 实现 `RLClient` for `GrpcRLClient`（predict/report_experience/sync_policy
///    三方法代理到 Python RL Service）
/// 3. 策略源切换: `RulePolicyFallback` → `GrpcRLClient`（接口不变，
///    调用方零改动，铁律2）
///
/// # 当前状态
///
/// 本结构不实现任何方法（编译期占位 + 文档契约），避免死代码警告。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GrpcRLClient {
    /// 训练服务端点（如 http://127.0.0.1:50051）
    endpoint: String,
    /// 策略版本缓存（层 → 版本）
    policy_versions: std::collections::HashMap<String, String>,
}

impl GrpcRLClient {
    /// 预留构造（v4.0 闸门解除后启用）
    #[allow(dead_code)]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            policy_versions: std::collections::HashMap::new(),
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- RulePolicyFallback 三方法 ----------

    #[tokio::test]
    async fn fallback_predict_returns_conservative_action() {
        let mut client = RulePolicyFallback;
        let action = client
            .predict(RLStateVector::zeros())
            .await
            .expect("规则回退预测不应失败");
        assert_eq!(action.layer.as_ref(), "fallback");
        assert_eq!(action.action_code, 0);
        assert_eq!(action.parameters.len(), 1);
        assert!((action.parameters[0] - 0.1).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn fallback_report_experience_is_noop_ok() {
        let mut client = RulePolicyFallback;
        let traj = RLTrajectory::new(
            "ep-test",
            vec![RLStateVector::zeros()],
            vec![RLActionVector::new("fallback", 0, vec![])],
            vec![0.5],
            vec![1_700_000_000_000],
        );
        client
            .report_experience(traj)
            .await
            .expect("规则回退上报不应失败（R1 数据面 no-op）");
    }

    #[tokio::test]
    async fn fallback_sync_policy_returns_json_rule() {
        let mut client = RulePolicyFallback;
        let policy = client
            .sync_policy("L6")
            .await
            .expect("规则回退同步不应失败");
        assert_eq!(policy.format, nexus_contracts::PolicyFormat::Json);
        assert_eq!(policy.version.as_ref(), "rule-fallback-v3.4.0");
        assert_eq!(policy.layer.as_ref(), "L6");
        assert_eq!(policy.byte_len(), 0);
    }

    // ---------- RLError ----------

    #[test]
    fn rl_error_display_messages() {
        assert_eq!(
            RLError::PolicyNotFound("L6".into()).to_string(),
            "Policy not found: L6"
        );
        assert_eq!(
            RLError::NetworkError("timeout".into()).to_string(),
            "Network error: timeout"
        );
        assert_eq!(
            RLError::InvalidState("clv dim 512".into()).to_string(),
            "Invalid state: clv dim 512"
        );
    }

    // ---------- 接口可替换性（铁律2） ----------

    /// Mock 实现 — 验证 RLClient 接口可被任意实现方替换
    struct MockClient {
        predictions: u32,
    }

    #[async_trait]
    impl RLClient for MockClient {
        async fn predict(&mut self, _state: RLStateVector) -> Result<RLActionVector, RLError> {
            self.predictions += 1;
            Ok(RLActionVector::new("mock", 7, vec![0.5]))
        }

        async fn report_experience(&mut self, _trajectory: RLTrajectory) -> Result<(), RLError> {
            Ok(())
        }

        async fn sync_policy(&mut self, _layer: &str) -> Result<SerializedPolicy, RLError> {
            Err(RLError::PolicyNotFound("mock-no-policy".into()))
        }
    }

    #[tokio::test]
    async fn rl_client_trait_is_implementable() {
        // 铁律2: 实现方可替换（RulePolicyFallback 为默认，Mock 为测试替身）
        let mut client: Box<dyn RLClient> = Box::new(MockClient { predictions: 0 });
        let action = client
            .predict(RLStateVector::zeros())
            .await
            .expect("mock 预测成功");
        assert_eq!(action.action_code, 7);
        let err = client.sync_policy("L1").await.expect_err("mock 同步应失败");
        assert!(matches!(err, RLError::PolicyNotFound(_)));
        // 动态分派验证 Send + Sync 约束（编译期）
        let _ = Box::new(RulePolicyFallback) as Box<dyn RLClient>;
    }

    // ---------- Send + Sync ----------

    #[test]
    fn rl_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RulePolicyFallback>();
        assert_send_sync::<GrpcRLClient>();
    }
}
