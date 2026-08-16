//! RL 预留钩子契约集成测试 — 统计学习 → RLTrajectory 数据流（v3.4.0 §5.7 + §17）
//!
//! 覆盖: 铁律6 轨迹导出 / 策略可替换（load_policy）/ 全格式序列化 /
//! RLHook 与既有 rl_types（RLAction/RLState）的语义对齐 / proptest 轨迹属性

#![forbid(unsafe_code)]

use nexus_contracts::{
    rl_types::RLAction, DensityTier, PolicyFormat, RLActionVector, RLHook, RLStateVector,
    RLTrajectory, SerializedPolicy,
};
use proptest::prelude::*;

// ----------------------------------------------------------
// 铁律6: 统计学习历史 → RLTrajectory 导出
// ----------------------------------------------------------

/// 模拟统计学习器（EMA 风格）— 验证可导出轨迹
struct MockStatLearner {
    episode: String,
    decisions: Vec<(RLStateVector, RLActionVector, f32, u64)>,
    current_policy: SerializedPolicy,
}

impl MockStatLearner {
    fn new(episode: &str) -> Self {
        Self {
            episode: episode.to_string(),
            decisions: Vec::new(),
            current_policy: SerializedPolicy::new(PolicyFormat::Json, vec![1, 2, 3], "0.1.0", "L6"),
        }
    }

    fn record(&mut self, state: RLStateVector, action: RLActionVector, reward: f32, ts: u64) {
        self.decisions.push((state, action, reward, ts));
    }
}

impl RLHook for MockStatLearner {
    fn export_trajectory(&self) -> RLTrajectory {
        // 手动拆分（Iterator::unzip 仅支持 2 元组，4 序列需显式循环）
        let mut states = Vec::with_capacity(self.decisions.len());
        let mut actions = Vec::with_capacity(self.decisions.len());
        let mut rewards = Vec::with_capacity(self.decisions.len());
        let mut timestamps = Vec::with_capacity(self.decisions.len());
        for (s, a, r, t) in &self.decisions {
            states.push(s.clone());
            actions.push(a.clone());
            rewards.push(*r);
            timestamps.push(*t);
        }
        RLTrajectory::new(&self.episode, states, actions, rewards, timestamps)
    }

    fn load_policy(&mut self, policy: SerializedPolicy) {
        self.current_policy = policy;
    }

    fn report_reward(&self, _reward: f32) {
        // 统计学习器: 奖励进入本地 EMA（此处 mock 不实现）
    }
}

#[test]
fn statistical_history_exports_to_trajectory() {
    // 铁律6: 所有统计学习机制必须可导出为 RLTrajectory（v4.0 数据流）
    let mut learner = MockStatLearner::new("episode-stats-1");
    learner.record(
        RLStateVector::zeros(),
        RLActionVector::new("S1", 1, vec![0.5]),
        0.2,
        1_000,
    );
    learner.record(
        RLStateVector::zeros(),
        RLActionVector::new("S1", 2, vec![0.6]),
        0.8,
        2_000,
    );

    let traj = learner.export_trajectory();
    assert_eq!(traj.episode_id.as_ref(), "episode-stats-1");
    assert_eq!(traj.len(), 2);
    assert_eq!(traj.actions[1].action_code, 2);
    assert!((traj.total_reward() - 1.0).abs() < f32::EPSILON);
}

#[test]
fn policy_replaceable_for_v4_upgrade() {
    // 铁律2/§17.2: RulePolicyFallback 可在 v4.0 无缝替换为 GrpcRLClient
    let mut learner = MockStatLearner::new("episode-policy-1");
    // v3.x: JSON 规则策略
    let rule_policy = SerializedPolicy::new(PolicyFormat::Json, vec![], "0.1.0", "L6");
    learner.load_policy(rule_policy);
    assert_eq!(learner.current_policy.format, PolicyFormat::Json);
    // v4.0: ONNX 策略网络（同接口替换）
    let onnx_policy = SerializedPolicy::new(PolicyFormat::Onnx, vec![7; 64], "1.0.0", "L6");
    learner.load_policy(onnx_policy.clone());
    assert_eq!(learner.current_policy, onnx_policy);
    assert_eq!(learner.current_policy.byte_len(), 64);
}

// ----------------------------------------------------------
// 与既有 rl_types 的语义对齐（S1-S9 接缝）
// ----------------------------------------------------------

#[test]
fn rl_hook_vectors_aligned_with_rl_types() {
    // RLActionVector（rl_hooks）与 RLAction（rl_types）语义对齐：
    // rl_types 的 S1-S9 接缝动作可投影为层动作向量（action_code 编码）
    use nexus_contracts::SeamId;
    let seam_action = RLAction::Density(DensityTier::default());
    assert_eq!(seam_action.seam_id(), Some(SeamId::S1Density));
    // 投影示例: 层动作向量承载层 + 动作码（L6 路由层的密度档位选择）
    let vector = RLActionVector::new("S1", 1, vec![0.5]);
    assert_eq!(vector.layer.as_ref(), "S1");
    assert_eq!(vector.action_code, 1);
    // SeamId 为 repr(u8) 枚举，S1Density 实际序号为 1（0 号位为保留/其他接缝）
    assert_eq!(SeamId::S1Density as u8, 1);
}

// ----------------------------------------------------------
// 全格式序列化
// ----------------------------------------------------------

#[test]
fn policy_all_formats_roundtrip() {
    for format in [
        PolicyFormat::Onnx,
        PolicyFormat::SafeTensors,
        PolicyFormat::Json,
    ] {
        let policy = SerializedPolicy::new(format, vec![1, 2, 3, 4], "1.0.0", "L1");
        let json = serde_json::to_string(&policy).expect("JSON 序列化失败");
        let back: SerializedPolicy = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(back, policy);
    }
}

#[test]
fn trajectory_msgpack_roundtrip() {
    // 训练数据面: 轨迹以 MsgPack 形态上传（大数组高效编码）
    let states = vec![RLStateVector::zeros(); 4];
    let actions = (0..4)
        .map(|i| RLActionVector::new("S1", i, vec![i as f32 / 4.0]))
        .collect();
    let traj = RLTrajectory::new(
        "episode-binary",
        states,
        actions,
        vec![0.1, 0.2, 0.3, 0.4],
        vec![1, 2, 3, 4],
    );
    let bytes = rmp_serde::to_vec(&traj).expect("MsgPack 序列化失败");
    let back: RLTrajectory = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
    assert_eq!(back, traj);
    assert_eq!(back.len(), 4);
}

// ----------------------------------------------------------
// proptest 轨迹属性
// ----------------------------------------------------------

proptest! {
    /// 轨迹不变量: 任意等长序列可构造轨迹并保持长度一致
    #[test]
    fn trajectory_length_preserved(
        n in 1usize..8,
        reward in 0.0f32..1.0,
    ) {
        let states = vec![RLStateVector::zeros(); n];
        let actions = vec![RLActionVector::new("S1", 0, vec![]); n];
        let rewards = vec![reward; n];
        let timestamps = (0..n as u64).collect();
        let traj = RLTrajectory::new("episode-prop", states, actions, rewards, timestamps);
        prop_assert_eq!(traj.len(), n);
        // f32 累加顺序差异: sum() 逐元素累加与乘法的舍入不同，用容差比较
        prop_assert!((traj.total_reward() - reward * n as f32).abs() < 1e-4);
    }

    /// 策略序列化属性: 任意字节负载可无损往返
    #[test]
    fn serialized_policy_bytes_roundtrip(
        len in 0usize..128,
        version in "[0-9]+\\.[0-9]+\\.[0-9]+",
    ) {
        let bytes = vec![0xAB; len];
        let policy = SerializedPolicy::new(PolicyFormat::Onnx, bytes.clone(), &version, "L2");
        let json = serde_json::to_string(&policy).expect("JSON 序列化失败");
        let back: SerializedPolicy = serde_json::from_str(&json).expect("JSON 反序列化失败");
        prop_assert_eq!(back.bytes.as_ref(), bytes.as_slice());
        prop_assert_eq!(back.version.as_ref(), version.as_str());
        prop_assert_eq!(back.byte_len(), len);
    }
}
