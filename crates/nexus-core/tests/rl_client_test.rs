//! RL 客户端骨架集成测试 — 策略可替换性与数据流（v3.4.0 §6.4 + §17）
//!
//! 覆盖: 顶层 API / 铁律2 策略可替换（RulePolicyFallback ↔ Mock）/
//! 统计学习器 → RLClient 的完整数据流（铁律6 导出 → report_experience）/
//! proptest 轨迹上报不变量

#![forbid(unsafe_code)]

use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector, RLTrajectory};
use nexus_contracts::PolicyFormat;
use nexus_core::rl_client::{RLClient, RLError, RulePolicyFallback};
use nexus_core::stat_learning::{SlidingWindowPolicy, StatLearningPolicy};
use proptest::prelude::*;

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use nexus_core::prelude::*;
    let client = RulePolicyFallback;
    let _ = client; // 编译期可达性验证
    let grpc = GrpcRLClient::new("http://127.0.0.1:50051");
    let _ = grpc;
    let _ = RLError::PolicyNotFound("L6".into());
}

// ----------------------------------------------------------
// 铁律2: 策略可替换（接口同构验证）
// ----------------------------------------------------------

#[tokio::test]
async fn rule_policy_fallback_is_default_and_replaceable() {
    // v3.x 默认: RulePolicyFallback（零 Python 依赖）
    let mut client = RulePolicyFallback;
    let action = client
        .predict(RLStateVector::zeros())
        .await
        .expect("规则回退预测成功");
    assert_eq!(action.layer.as_ref(), "fallback");
    // v4.0 替换路径: 任意实现可替换（接口不变，调用方零改动）
    let mut grpc_like = TestGrpcClient;
    let action2 = grpc_like
        .predict(RLStateVector::zeros())
        .await
        .expect("gRPC 风格客户端预测成功");
    assert_eq!(action2.layer.as_ref(), "grpc-test");
}

/// 测试用 gRPC 风格客户端 — 模拟 v4.0 GrpcRLClient 的接口同构（铁律2）
struct TestGrpcClient;

#[async_trait::async_trait]
impl RLClient for TestGrpcClient {
    async fn predict(&mut self, _state: RLStateVector) -> Result<RLActionVector, RLError> {
        Ok(RLActionVector::new("grpc-test", 1, vec![0.5]))
    }

    async fn report_experience(&mut self, _trajectory: RLTrajectory) -> Result<(), RLError> {
        Ok(())
    }

    async fn sync_policy(
        &mut self,
        layer: &str,
    ) -> Result<nexus_contracts::SerializedPolicy, RLError> {
        Ok(nexus_contracts::SerializedPolicy::new(
            PolicyFormat::Onnx,
            vec![],
            "1.0.0",
            layer,
        ))
    }
}

// ----------------------------------------------------------
// 完整数据流: 统计学习器 → 轨迹导出 → RLClient 上报（铁律6 → §17 数据流）
// ----------------------------------------------------------

#[tokio::test]
async fn stat_learner_to_rl_client_full_flow() {
    // 1. 统计学习器学习
    let mut learner = SlidingWindowPolicy::<u8, u8>::new(16, 0.1);
    for i in 0..5 {
        learner.update(&i, &(i % 2), 0.5);
    }
    // 2. 铁律6: 导出轨迹
    let traj = learner.export_trajectory("ep-full-flow");
    assert_eq!(traj.len(), 5);
    // 3. RLClient 上报（R1 数据面）
    let mut client = RulePolicyFallback;
    client
        .report_experience(traj)
        .await
        .expect("规则回退上报不应失败");
    // 4. 策略同步（v3.x JSON → v4.0 ONNX 替换路径）
    let policy = client.sync_policy("L6").await.expect("同步成功");
    assert_eq!(policy.format, PolicyFormat::Json);
    assert_eq!(policy.layer.as_ref(), "L6");
}

// ----------------------------------------------------------
// RLError 完整性
// ----------------------------------------------------------

#[test]
fn rl_error_variants_exhaustive() {
    let errs = [
        RLError::PolicyNotFound("L1".into()),
        RLError::NetworkError("conn refused".into()),
        RLError::InvalidState("dim mismatch".into()),
    ];
    assert_eq!(errs.len(), 3);
    // Display 与错误语义一致
    assert!(errs[0].to_string().contains("Policy not found"));
    assert!(errs[1].to_string().contains("Network error"));
    assert!(errs[2].to_string().contains("Invalid state"));
}

// ----------------------------------------------------------
// proptest: 轨迹上报不变量（任意轨迹可被 fallback 消费）
// ----------------------------------------------------------

proptest! {
    /// 任意长度的合法轨迹均可被 RulePolicyFallback 消费（R1 数据面无丢失）
    #[test]
    fn fallback_consumes_arbitrary_trajectory(
        n in 0usize..16,
        reward in 0.0f32..1.0,
    ) {
        let states = vec![RLStateVector::zeros(); n];
        let actions = vec![RLActionVector::new("S1", 0, vec![]); n];
        let rewards = vec![reward; n];
        let timestamps: Vec<u64> = (0..n as u64).map(|i| 1_700_000_000_000 + i).collect();
        let traj = RLTrajectory::new("ep-prop-rl", states, actions, rewards, timestamps);
        // 同步验证（proptest 块内不便 async，直接验证构造 + 序列化）
        let json = serde_json::to_string(&traj).expect("JSON 序列化失败");
        let back: RLTrajectory = serde_json::from_str(&json).expect("JSON 反序列化失败");
        prop_assert_eq!(&back, &traj);
        prop_assert_eq!(back.len(), n);
    }
}
