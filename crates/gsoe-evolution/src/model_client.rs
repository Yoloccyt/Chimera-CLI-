//! 模型采样客户端 — GSOE 真实模型接入(P0-6)
//!
//! 对应架构层:L5 Knowledge
//!
//! ## 设计决策
//! 不直接依赖 `mcp-mesh` crate(L10 Interface)以避免跨层依赖违反 §2.2 铁律。
//! 而是通过独立 HTTP JSON-RPC 客户端调用外部模型服务,协议与 MCP Mesh 兼容。
//!
//! ## 降级路径
//! - 真实模式:HTTP POST 到模型服务端点,获取 logits/embedding 作为动作向量
//! - Mock 模式:保留 LCG 伪随机采样,用于 CI/无网络环境

use serde::{Deserialize, Serialize};

use crate::error::GsoeError;

/// 模型采样请求 — 发送给外部模型服务
#[derive(Debug, Clone, Serialize)]
pub struct ModelSampleRequest {
    /// 提示词/上下文(由策略参数编码)
    pub prompt: String,
    /// 采样温度(由 mutation_rate 映射)
    pub temperature: f32,
    /// 请求动作维度(GRPO 动作向量长度)
    pub action_dim: usize,
    /// 采样轨迹 ID
    pub trajectory_id: String,
}

/// 模型采样响应 — 外部模型返回的动作向量
#[derive(Debug, Clone, Deserialize)]
pub struct ModelSampleResponse {
    /// 动作向量(logits 或 embedding 值)
    pub actions: Vec<f32>,
    /// 模型评估的奖励估计(可选,若无则由 GSOE 本地计算)
    pub estimated_reward: Option<f32>,
    /// 模型推理延迟(毫秒)
    pub latency_ms: u64,
}

/// 模型采样客户端 — 支持真实网络与 Mock 两种模式
#[derive(Debug, Clone)]
pub struct ModelSampler {
    /// HTTP 客户端(真实模式)
    http: Option<reqwest::Client>,
    /// 模型服务端点(如 "http://203.0.113.1:8080/v1/sample")
    endpoint: Option<String>,
    /// Mock 模式:直接返回伪随机动作
    mock: bool,
    /// 请求超时(毫秒)
    timeout_ms: u64,
}

impl ModelSampler {
    /// 创建 Mock 采样器(无网络,本地 LCG 伪随机)
    pub fn mock() -> Self {
        Self {
            http: None,
            endpoint: None,
            mock: true,
            timeout_ms: 5000,
        }
    }

    /// 创建真实模型采样器
    ///
    /// # 参数
    /// - `endpoint`:模型服务 HTTP 端点
    /// - `timeout_ms`:请求超时
    pub fn new(endpoint: impl Into<String>, timeout_ms: u64) -> Self {
        let endpoint = endpoint.into();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .ok();
        Self {
            http,
            endpoint: Some(endpoint),
            mock: false,
            timeout_ms,
        }
    }

    /// 从配置创建采样器(mock 模式或真实模式)
    pub fn from_config(mock: bool, endpoint: Option<String>, timeout_ms: u64) -> Self {
        if mock || endpoint.is_none() {
            Self::mock()
        } else {
            Self::new(endpoint.unwrap(), timeout_ms)
        }
    }

    /// 发送采样请求,获取动作向量
    ///
    /// Mock 模式:使用 LCG 生成伪随机动作向量(与原有行为一致)。
    /// 真实模式:HTTP POST 到模型服务端点,返回模型生成的动作向量。
    pub async fn sample(&self, request: ModelSampleRequest) -> Result<ModelSampleResponse, GsoeError> {
        if self.mock {
            return Ok(self.mock_sample(&request));
        }

        let http = self.http.as_ref().ok_or_else(|| GsoeError::ConfigError {
            reason: "HTTP client 初始化失败".into(),
        })?;
        let endpoint = self.endpoint.as_ref().ok_or_else(|| GsoeError::ConfigError {
            reason: "模型服务端点未配置".into(),
        })?;

        let resp = http
            .post(endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|e| GsoeError::ConfigError {
                reason: format!("模型采样请求失败: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(GsoeError::ConfigError {
                reason: format!("模型服务返回 HTTP {}", resp.status()),
            });
        }

        resp.json::<ModelSampleResponse>()
            .await
            .map_err(|e| GsoeError::ConfigError {
                reason: format!("模型响应解析失败: {e}"),
            })
    }

    /// Mock 采样 — LCG 伪随机生成动作向量
    fn mock_sample(&self, request: &ModelSampleRequest) -> ModelSampleResponse {
        let mut rng = Lcg::new(request.trajectory_id.hash_seed());
        let mut actions = Vec::with_capacity(request.action_dim);
        for _ in 0..request.action_dim {
            // 动作 = 基准(1.0) + temperature * 随机扰动
            let action = 1.0 + request.temperature * rng.next_f32();
            actions.push(action);
        }
        ModelSampleResponse {
            actions,
            estimated_reward: None,
            latency_ms: 1,
        }
    }
}

/// 线性同余 PRNG — 零外部依赖的伪随机数生成器
///
/// 与 `policy::grpo::Lcg` 相同实现,提取到 model_client 以支持 Mock 模式。
pub(crate) struct Lcg {
    state: u64,
}

impl Lcg {
    /// 以 seed 构造 PRNG
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(1),
        }
    }

    /// 生成下一个 u32 伪随机数
    pub(crate) fn next_u32(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(1103515245).wrapping_add(12345);
        ((self.state >> 16) & 0x7FFF_FFFF) as u32
    }

    /// 生成 [-1.0, 1.0) 范围的 f32
    pub(crate) fn next_f32(&mut self) -> f32 {
        let raw = self.next_u32() as f32 / (u32::MAX as f32 / 2.0);
        raw - 1.0
    }
}

/// 为字符串生成确定性哈希种子
trait HashSeed {
    fn hash_seed(&self) -> u64;
}

impl HashSeed for str {
    fn hash_seed(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_sampler_returns_actions() {
        let sampler = ModelSampler::mock();
        let req = ModelSampleRequest {
            prompt: "test".into(),
            temperature: 0.1,
            action_dim: 10,
            trajectory_id: "traj-1".into(),
        };
        let resp = sampler.sample(req).await.expect("Mock 采样应成功");
        assert_eq!(resp.actions.len(), 10);
        assert!(resp.estimated_reward.is_none());
    }

    #[tokio::test]
    async fn test_mock_sampler_deterministic() {
        let sampler = ModelSampler::mock();
        let req1 = ModelSampleRequest {
            prompt: "test".into(),
            temperature: 0.1,
            action_dim: 10,
            trajectory_id: "traj-1".into(),
        };
        let req2 = ModelSampleRequest {
            prompt: "test".into(),
            temperature: 0.1,
            action_dim: 10,
            trajectory_id: "traj-1".into(),
        };
        let resp1 = sampler.sample(req1).await.unwrap();
        let resp2 = sampler.sample(req2).await.unwrap();
        assert_eq!(resp1.actions, resp2.actions);
    }

    #[tokio::test]
    async fn test_mock_sampler_different_trajectories() {
        let sampler = ModelSampler::mock();
        let req1 = ModelSampleRequest {
            prompt: "test".into(),
            temperature: 0.1,
            action_dim: 10,
            trajectory_id: "traj-1".into(),
        };
        let req2 = ModelSampleRequest {
            prompt: "test".into(),
            temperature: 0.1,
            action_dim: 10,
            trajectory_id: "traj-2".into(),
        };
        let resp1 = sampler.sample(req1).await.unwrap();
        let resp2 = sampler.sample(req2).await.unwrap();
        // 不同 trajectory_id 应产生不同动作(概率极高)
        assert_ne!(resp1.actions, resp2.actions);
    }

    #[test]
    fn test_lcg_deterministic() {
        let mut rng1 = Lcg::new(42);
        let mut rng2 = Lcg::new(42);
        for _ in 0..100 {
            assert_eq!(rng1.next_u32(), rng2.next_u32());
        }
    }

    #[test]
    fn test_lcg_f32_range() {
        let mut rng = Lcg::new(123);
        for _ in 0..1000 {
            let v = rng.next_f32();
            assert!(v >= -1.0 && v < 1.0, "f32 应在 [-1.0, 1.0) 范围内: {v}");
        }
    }
}
