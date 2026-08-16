//! RL 预留钩子契约 — v4.0 升级路径（设计文档 §5.7 + §17）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §5.7 / §17
//! 对应规划: RL 架构预留（Rust 侧接口设计 · Python 侧 v4.0 计划）
//!
//! # 核心职责
//!
//! 承载 v4.0 RL 升级的 Rust 侧预留接口与共享数据类型：
//!
//! | 类型 | 职责 | 消费层 |
//! |------|------|--------|
//! | [`RLHook`] | 统计学习 ↔ RL 的统一钩子 trait（铁律6 导出 RLTrajectory） | L1 rl-client / L1 nexus-core stat-learning |
//! | [`SerializedPolicy`] | 序列化策略载荷（ONNX/SafeTensors/JSON） | L1 rl-client sync_policy |
//! | [`RLTrajectory`] | 状态-动作-奖励序列（统计学习历史导出形态） | L1 rl-client / 训练导出 |
//! | [`RLStateVector`] | 层状态向量（CLV 512 维 + 层特征 128 维） | L2 memory-pyramid / L1 状态编码器 |
//! | [`RLActionVector`] | 层动作向量（层标识 + 动作码 + 参数） | L2 memory-pyramid / L1 动作解码器 |
//!
//! # 设计约束（ADR-033 + 铁律）
//!
//! - **纯类型 + 接口定义**: trait 仅声明契约，不含实现逻辑
//! - **零 crate 依赖**: 仅 `serde` derive；**不引入 `async-trait`**
//!   （L0 零依赖铁律）——`RLHook` 为同步 trait（`Send + Sync`），
//!   异步包装由 L1 `rl-client` 的 `GrpcRLClient` 提供，与既有
//!   `MemoryStrategyProvider` / `CommandValidator` trait 模式一致
//! - **f32 字段仅 `PartialEq`**: logprobs/rewards/parameters 为浮点字段
//! - **铁律6**: `export_trajectory` 使所有统计学习历史可导出为 `RLTrajectory`
//! - **铁律2**: 接口同构——Rust 侧 `StatLearningPolicy` 与 RL `Policy`
//!   接口同构（State→Action），`RLHook` 为数据面预留
//! - **`Box<[T]>` 优化**: 轨迹序列为写后只读大载荷，用堆切片承载
//! - **固定数组布局**: `RLStateVector` 使用 `[f32; 512]` / `[f32; 128]`
//!   固定数组（2.5KB 栈内安全），为未来 FFI/ONNX 互操作预留
//!
//! # v4.0 升级路径
//!
//! | 组件 | v3.x 实现 | v4.0 升级 |
//! |------|-----------|----------|
//! | `RLHook` 实现方 | 规则策略回退（RulePolicyFallback） | gRPC 调用 Python RL Service |
//! | `SerializedPolicy` | 本地 JSON 配置 | 从 Python 服务拉取 ONNX 模型 |
//! | `RLTrajectory` | 本地序列化 | 上传到训练集群 |

use serde::{Deserialize, Serialize};

/// 固定数组 serde 辅助 — 大数组字段的序列化/反序列化
///
/// WHY 自实现: serde 内建 `[T; N]` 的 `Deserialize` 仅支持 N ≤ 32，
/// CLV(512)/层特征(128) 超出；L0 零依赖铁律禁止引入 `serde_arrays` 依赖。
/// 序列化委托内建 `Serialize`（任意 N 支持），反序列化收集 Vec 后
/// 长度校验转数组（长度不匹配即拒绝——训练证据完整性）。
mod fixed_array {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, const N: usize>(
        arr: &[f32; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        arr.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        deserializer: D,
    ) -> Result<[f32; N], D::Error> {
        let v = Vec::<f32>::deserialize(deserializer)?;
        let actual = v.len();
        v.try_into().map_err(|_| {
            // String 不实现 Expected，用 custom 承载动态长度错误信息
            serde::de::Error::custom(format!("期望 {N} 个 f32，实际 {actual}"))
        })
    }
}

// ============================================================
// 策略格式与序列化策略
// ============================================================

/// 策略序列化格式 — 支持 ONNX/SafeTensors/JSON 三态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyFormat {
    /// ONNX 模型（v4.0 策略网络默认格式）
    Onnx,
    /// SafeTensors（HuggingFace 生态格式）
    SafeTensors,
    /// JSON 配置（v3.x 规则策略格式）
    Json,
}

/// 序列化策略载荷 — v4.0 `sync_policy` 的传输/存储形态
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerializedPolicy {
    /// 策略格式
    pub format: PolicyFormat,
    /// 策略字节（ONNX/SafeTensors/JSON 原始字节，定长只读）
    pub bytes: Box<[u8]>,
    /// 策略版本（如 "1.0.0"）
    pub version: Box<str>,
    /// 所属架构层（如 "L6" / "S2"）
    pub layer: Box<str>,
}

impl SerializedPolicy {
    /// 创建序列化策略载荷
    pub fn new(format: PolicyFormat, bytes: Vec<u8>, version: &str, layer: &str) -> Self {
        Self {
            format,
            bytes: bytes.into_boxed_slice(),
            version: Box::from(version),
            layer: Box::from(layer),
        }
    }

    /// 策略字节长度（完整性校验便捷访问）
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}

// ============================================================
// 状态/动作向量
// ============================================================

/// 层状态向量 — RL 状态的空间表示
///
/// `clv` 为 512 维上下文潜在向量（与 `nexus-core::CLV` 语义对齐，L0 不依赖 L1），
/// `layer_features` 为 128 维层特征（各层自定义编码）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RLStateVector {
    /// 上下文潜在向量（512 维 f32）
    #[serde(with = "fixed_array")]
    pub clv: [f32; 512],
    /// 层特征向量（128 维 f32）
    #[serde(with = "fixed_array")]
    pub layer_features: [f32; 128],
}

impl RLStateVector {
    /// 创建全零状态向量（默认/未知状态）
    pub fn zeros() -> Self {
        Self {
            clv: [0.0; 512],
            layer_features: [0.0; 128],
        }
    }
}

/// 层动作向量 — RL 动作的空间表示
///
/// `action_code` 为层内动作编号（如 S1 接缝的密度档位编码），
/// `parameters` 为动作的连续参数（定长只读）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RLActionVector {
    /// 所属架构层（如 "L6" / "S8"）
    pub layer: Box<str>,
    /// 动作码（层内离散动作编号）
    pub action_code: u32,
    /// 动作连续参数（定长只读）
    pub parameters: Box<[f32]>,
}

impl RLActionVector {
    /// 创建层动作向量
    pub fn new(layer: &str, action_code: u32, parameters: Vec<f32>) -> Self {
        Self {
            layer: Box::from(layer),
            action_code,
            parameters: parameters.into_boxed_slice(),
        }
    }
}

// ============================================================
// 轨迹
// ============================================================

/// RLTrajectory — 状态-动作-奖励完整轨迹（铁律6 导出形态）
///
/// 所有统计学习机制（EMA/UCB/Softmax）的历史可导出为轨迹，
/// 为 v4.0 离线训练提供数据流。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RLTrajectory {
    /// 轨迹 ID（episode 标识）
    pub episode_id: Box<str>,
    /// 状态序列（定长只读）
    pub states: Box<[RLStateVector]>,
    /// 动作序列（定长只读）
    pub actions: Box<[RLActionVector]>,
    /// 奖励序列（定长只读）
    pub rewards: Box<[f32]>,
    /// 时间戳序列（Unix 毫秒，定长只读）
    pub timestamps: Box<[u64]>,
}

impl RLTrajectory {
    /// 创建轨迹
    ///
    /// # Panics
    ///
    /// 四序列长度不一致时 panic——轨迹完整性不变量：
    /// 每回合必须同时具有状态/动作/奖励/时间戳。
    pub fn new(
        episode_id: &str,
        states: Vec<RLStateVector>,
        actions: Vec<RLActionVector>,
        rewards: Vec<f32>,
        timestamps: Vec<u64>,
    ) -> Self {
        assert_eq!(
            states.len(),
            actions.len(),
            "RLTrajectory 不变量: states 与 actions 必须等长"
        );
        assert_eq!(
            states.len(),
            rewards.len(),
            "RLTrajectory 不变量: states 与 rewards 必须等长"
        );
        assert_eq!(
            states.len(),
            timestamps.len(),
            "RLTrajectory 不变量: states 与 timestamps 必须等长"
        );
        Self {
            episode_id: Box::from(episode_id),
            states: states.into_boxed_slice(),
            actions: actions.into_boxed_slice(),
            rewards: rewards.into_boxed_slice(),
            timestamps: timestamps.into_boxed_slice(),
        }
    }

    /// 轨迹长度（回合数）
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// 是否为空轨迹
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// 累计奖励（轨迹级信用评估）
    pub fn total_reward(&self) -> f32 {
        self.rewards.iter().sum()
    }
}

// ============================================================
// RLHook trait
// ============================================================

/// RL 钩子 — 统计学习 ↔ RL 的统一接口（v4.0 升级路径）
///
/// 同步 trait（`Send + Sync`）：L0 仅定义契约，异步能力由 L1 `rl-client`
/// 的 `GrpcRLClient` 包装（铁律2：实现方可替换，RulePolicyFallback 为默认）。
///
/// # v4.0 替换路径
///
/// | 方法 | v3.x（统计） | v4.0（RL） |
/// |------|-------------|-----------|
/// | `export_trajectory` | 本地序列化 | 上传到训练集群 |
/// | `load_policy` | 加载本地 JSON 配置 | 拉取 ONNX 模型 |
/// | `report_reward` | 本地统计 | 发送到 Python 训练服务 |
pub trait RLHook: Send + Sync {
    /// 导出统计学习历史为 RL 轨迹（铁律6）
    ///
    /// 所有统计学习机制必须可导出，为 v4.0 升级预留数据流。
    fn export_trajectory(&self) -> RLTrajectory;

    /// 加载（替换）策略 — 从序列化载荷恢复策略状态
    fn load_policy(&mut self, policy: SerializedPolicy);

    /// 上报奖励信号 — 反馈给学习器
    fn report_reward(&self, reward: f32);
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 序列化策略 ----------

    #[test]
    fn serialized_policy_roundtrip() {
        let policy = SerializedPolicy::new(PolicyFormat::Onnx, vec![1, 2, 3, 4], "1.0.0", "L6");
        let json = serde_json::to_string(&policy).expect("JSON 序列化失败");
        let decoded: SerializedPolicy = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, policy);
        assert_eq!(decoded.byte_len(), 4);
    }

    #[test]
    fn serialized_policy_wire_format_frozen() {
        let policy = SerializedPolicy::new(PolicyFormat::Json, vec![], "0.1.0", "S2");
        let json = serde_json::to_string(&policy).expect("JSON 序列化失败");
        assert!(json.contains("\"format\":\"json\""));
        assert!(json.contains("\"version\":\"0.1.0\""));
        assert!(json.contains("\"layer\":\"S2\""));
    }

    #[test]
    fn policy_format_exhaustive() {
        let all = [
            PolicyFormat::Onnx,
            PolicyFormat::SafeTensors,
            PolicyFormat::Json,
        ];
        assert_eq!(all.len(), 3);
    }

    // ---------- 状态/动作向量 ----------

    #[test]
    fn rl_state_vector_layout() {
        // 布局验证: 512 + 128 = 640 f32 = 2560 bytes 固定载荷
        let v = RLStateVector::zeros();
        assert_eq!(v.clv.len(), 512);
        assert_eq!(v.layer_features.len(), 128);
        assert!(v.clv.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn rl_state_vector_msgpack_roundtrip() {
        // 大固定数组的二进制 roundtrip（v4.0 训练数据面核心路径）
        let mut v = RLStateVector::zeros();
        v.clv[0] = 0.5;
        v.clv[511] = -0.25;
        v.layer_features[127] = 1.0;
        let bytes = rmp_serde::to_vec(&v).expect("MsgPack 序列化失败");
        let decoded: RLStateVector = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        assert_eq!(decoded, v);
    }

    #[test]
    fn rl_action_vector_roundtrip() {
        let action = RLActionVector::new("S1", 2, vec![0.1, 0.2, 0.3]);
        let json = serde_json::to_string(&action).expect("JSON 序列化失败");
        let decoded: RLActionVector = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, action);
        assert_eq!(decoded.parameters.len(), 3);
    }

    // ---------- 轨迹 ----------

    fn sample_trajectory() -> RLTrajectory {
        RLTrajectory::new(
            "episode-1",
            vec![RLStateVector::zeros(), RLStateVector::zeros()],
            vec![
                RLActionVector::new("S1", 1, vec![0.5]),
                RLActionVector::new("S2", 0, vec![]),
            ],
            vec![0.1, 0.9],
            vec![1_700_000_000_000, 1_700_000_001_000],
        )
    }

    #[test]
    fn rl_trajectory_roundtrip() {
        let traj = sample_trajectory();
        let json = serde_json::to_string(&traj).expect("JSON 序列化失败");
        let decoded: RLTrajectory = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, traj);
        assert_eq!(decoded.len(), 2);
        assert!(!decoded.is_empty());
    }

    #[test]
    fn rl_trajectory_total_reward() {
        let traj = sample_trajectory();
        assert!((traj.total_reward() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn rl_trajectory_length_invariant_asserted() {
        // 完整性: 序列长度不一致必须 panic
        let result = std::panic::catch_unwind(|| {
            RLTrajectory::new(
                "bad",
                vec![RLStateVector::zeros()],
                vec![],
                vec![0.5],
                vec![0],
            )
        });
        assert!(result.is_err(), "序列长度不一致必须触发断言 panic");
    }

    #[test]
    fn rl_trajectory_empty_semantics() {
        let empty = RLTrajectory::new("empty", vec![], vec![], vec![], vec![]);
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.total_reward(), 0.0);
    }

    // ---------- RLHook trait ----------

    /// 测试用 mock 实现 — 验证 trait 契约可被实现（铁律2: 实现方可替换）
    struct MockHook {
        policy: SerializedPolicy,
    }

    impl RLHook for MockHook {
        fn export_trajectory(&self) -> RLTrajectory {
            sample_trajectory()
        }

        fn load_policy(&mut self, policy: SerializedPolicy) {
            self.policy = policy;
        }

        fn report_reward(&self, reward: f32) {
            // 统计学习器语义: 奖励进入本地 EMA（此处 mock 记录后由测试读取）
            let _ = reward;
        }
    }

    #[test]
    fn rl_hook_trait_implementable() {
        let mut hook = MockHook {
            policy: SerializedPolicy::new(PolicyFormat::Json, vec![], "0.1.0", "L6"),
        };
        // export_trajectory（铁律6）
        let traj = hook.export_trajectory();
        assert_eq!(traj.episode_id.as_ref(), "episode-1");
        // load_policy（策略可替换）
        let new_policy = SerializedPolicy::new(PolicyFormat::Onnx, vec![9; 16], "1.0.0", "L6");
        hook.load_policy(new_policy.clone());
        assert_eq!(hook.policy, new_policy);
        // report_reward
        hook.report_reward(0.8);
    }

    #[test]
    fn rl_hook_is_send_sync() {
        // 编译期验证: RLHook 约束 Send + Sync（异步包装前提）
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MockHook>();
    }
}
