//! 推理客户端 — MTPE 真实模型推理(P0-9)
//!
//! 对应架构层:L7 Execution
//!
//! ## 设计决策
//! 不直接依赖 `mcp-mesh` crate(L10 Interface)以避免跨层依赖违反 §2.2 铁律。
//! 通过独立 HTTP JSON-RPC 客户端调用外部推理服务(如 vLLM/TensorRT-LLM)。
//!
//! ## 降级路径
//! - 真实模式:HTTP POST 到推理服务端点,获取多步预测 token
//! - Mock 模式:保留 FNV-1a 哈希伪预测,用于 CI/无网络环境

use serde::{Deserialize, Serialize};

use crate::error::MtpeError;
use crate::types::{PredictionContext, Token};

/// 推理请求 — 发送给外部推理服务
#[derive(Debug, Clone, Serialize)]
pub struct InferenceRequest {
    /// 上下文文本(由 PredictionContext 编码)
    pub context: String,
    /// 预测步数 N
    pub n: usize,
    /// 请求 ID(用于 tracing)
    pub request_id: String,
}

/// 推理响应 — 外部推理服务返回的预测 token 列表
#[derive(Debug, Clone, Deserialize)]
pub struct InferenceResponse {
    /// 预测的 token 列表
    pub tokens: Vec<InferenceToken>,
    /// 推理延迟(毫秒)
    pub latency_ms: f32,
}

/// 推理服务返回的单个 token
#[derive(Debug, Clone, Deserialize)]
pub struct InferenceToken {
    /// token 文本
    pub text: String,
    /// 模型置信度(logit probability)
    pub confidence: f32,
}

/// 推理客户端 — 支持真实网络与 Mock 两种模式
#[derive(Debug, Clone)]
pub struct InferenceClient {
    /// HTTP 客户端(真实模式)
    http: Option<reqwest::Client>,
    /// 推理服务端点(如 "http://203.0.113.1:8080/v1/predict")
    endpoint: Option<String>,
    /// Mock 模式:直接返回伪预测 token
    mock: bool,
    /// 请求超时(毫秒)
    timeout_ms: u64,
}

impl InferenceClient {
    /// 创建 Mock 推理客户端(无网络,本地伪预测)
    pub fn mock() -> Self {
        Self {
            http: None,
            endpoint: None,
            mock: true,
            timeout_ms: 5000,
        }
    }

    /// 创建真实推理客户端
    ///
    /// # 参数
    /// - `endpoint`:推理服务 HTTP 端点
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

    /// 从配置创建推理客户端
    pub fn from_config(mock: bool, endpoint: Option<String>, timeout_ms: u64) -> Self {
        if mock || endpoint.is_none() {
            Self::mock()
        } else {
            Self::new(endpoint.unwrap(), timeout_ms)
        }
    }

    /// 发送推理请求,获取多步预测 token
    ///
    /// Mock 模式:使用 FNV-1a 哈希生成确定性伪预测 token。
    /// 真实模式:HTTP POST 到推理服务端点,返回模型生成的 token 列表。
    pub async fn infer(&self, request: InferenceRequest) -> Result<InferenceResponse, MtpeError> {
        if self.mock {
            return Ok(self.mock_infer(&request));
        }

        let http = self.http.as_ref().ok_or_else(|| MtpeError::ConfigError {
            reason: "HTTP client 初始化失败".into(),
        })?;
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or_else(|| MtpeError::ConfigError {
                reason: "推理服务端点未配置".into(),
            })?;

        let resp = http
            .post(endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|e| MtpeError::ConfigError {
                reason: format!("推理请求失败: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(MtpeError::ConfigError {
                reason: format!("推理服务返回 HTTP {}", resp.status()),
            });
        }

        resp.json::<InferenceResponse>()
            .await
            .map_err(|e| MtpeError::ConfigError {
                reason: format!("推理响应解析失败: {e}"),
            })
    }

    /// Mock 推理 — FNV-1a 哈希生成确定性伪预测 token
    fn mock_infer(&self, request: &InferenceRequest) -> InferenceResponse {
        let context_hash = compute_context_hash(&request.context, request.n);
        let tokens = (0..request.n)
            .map(|i| {
                let confidence = (1.0 - (i as f32 * 0.05)).max(0.0);
                InferenceToken {
                    text: format!("pred_{}_{}", i, context_hash),
                    confidence,
                }
            })
            .collect();
        InferenceResponse {
            tokens,
            latency_ms: 0.05, // 50μs 模拟延迟
        }
    }
}

/// 计算上下文哈希 — Mock 推理的确定性种子
///
/// 与 predictor.rs 的 compute_context_hash 相同算法,独立提取以支持 Mock 模式。
fn compute_context_hash(context_str: &str, n: usize) -> u32 {
    let mut hash: u32 = 0x4D54_5045; // "MTPE" ASCII
    for byte in context_str.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // 混入 n,使不同步数产生不同输出
    hash ^= n as u32;
    hash = hash.wrapping_mul(0x0100_0193);
    hash
}

/// 将 InferenceToken 转换为 MTPE 内部 Token 类型
pub fn to_mtpe_tokens(inference_tokens: Vec<InferenceToken>) -> Vec<Token> {
    inference_tokens
        .into_iter()
        .map(|t| Token {
            text: t.text,
            confidence: t.confidence,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_infer_returns_tokens() {
        let client = InferenceClient::mock();
        let req = InferenceRequest {
            context: "hello".into(),
            n: 5,
            request_id: "req-1".into(),
        };
        let resp = client.infer(req).await.expect("Mock 推理应成功");
        assert_eq!(resp.tokens.len(), 5);
        assert!((resp.tokens[0].confidence - 1.0).abs() < f32::EPSILON);
        assert!((resp.tokens[4].confidence - 0.8).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_mock_infer_deterministic() {
        let client = InferenceClient::mock();
        let req1 = InferenceRequest {
            context: "hello".into(),
            n: 3,
            request_id: "req-1".into(),
        };
        let req2 = InferenceRequest {
            context: "hello".into(),
            n: 3,
            request_id: "req-1".into(),
        };
        let resp1 = client.infer(req1).await.unwrap();
        let resp2 = client.infer(req2).await.unwrap();
        assert_eq!(resp1.tokens.len(), resp2.tokens.len());
        for (a, b) in resp1.tokens.iter().zip(resp2.tokens.iter()) {
            assert_eq!(a.text, b.text);
            assert!((a.confidence - b.confidence).abs() < f32::EPSILON);
        }
    }

    #[tokio::test]
    async fn test_mock_infer_different_context() {
        let client = InferenceClient::mock();
        let req1 = InferenceRequest {
            context: "hello".into(),
            n: 3,
            request_id: "req-1".into(),
        };
        let req2 = InferenceRequest {
            context: "world".into(),
            n: 3,
            request_id: "req-1".into(),
        };
        let resp1 = client.infer(req1).await.unwrap();
        let resp2 = client.infer(req2).await.unwrap();
        // 不同上下文应产生不同 token(概率极高)
        assert_ne!(resp1.tokens[0].text, resp2.tokens[0].text);
    }

    #[test]
    fn test_to_mtpe_tokens_conversion() {
        let inference = vec![
            InferenceToken {
                text: "token1".into(),
                confidence: 0.9,
            },
            InferenceToken {
                text: "token2".into(),
                confidence: 0.8,
            },
        ];
        let mtpe = to_mtpe_tokens(inference);
        assert_eq!(mtpe.len(), 2);
        assert_eq!(mtpe[0].text, "token1");
        assert!((mtpe[0].confidence - 0.9).abs() < f32::EPSILON);
    }
}
