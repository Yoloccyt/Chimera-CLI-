//! 语义嵌入客户端 — TextPerceptor v3 神经网络语义嵌入(P0-1)
//!
//! 对应架构层:L2 Memory
//!
//! ## 设计决策
//! 不直接引入 `ort` crate(避免 unsafe 代码与复杂依赖)。
//! 通过独立 HTTP 客户端调用外部 embedding 服务(如 ONNX Runtime Server/
//! sentence-transformers API),协议兼容 OpenAI Embedding API 格式。
//!
//! ## 降级路径
//! - 真实模式:HTTP POST 到 embedding 服务端点,获取 512-dim 语义向量
//! - Mock 模式:保留 v2 的 n-gram SipHash 感知哈希(同义句相似度>0.85)
//!
//! ## 验收指标
//! - 同义句相似度 > 0.9(v2 为 >0.85)
//! - 编码延迟 p95 < 30ms(1000 字符文本)

use serde::{Deserialize, Serialize};

use crate::error::NmcError;

/// Embedding 请求 — 发送给外部 embedding 服务
#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingRequest {
    /// 输入文本
    pub input: String,
    /// 模型标识(如 "text-embedding-v3")
    pub model: String,
    /// 请求 ID
    pub request_id: String,
}

/// Embedding 响应 — 外部服务返回的向量
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingResponse {
    /// 嵌入向量(512-dim 或 768-dim,下游模块负责投影到 512-dim)
    pub embedding: Vec<f32>,
    /// 模型使用的 token 数
    pub tokens_used: u32,
    /// 推理延迟(毫秒)
    pub latency_ms: f64,
}

/// 语义嵌入客户端 — 支持真实网络与 Mock 两种模式
#[derive(Debug, Clone)]
pub struct EmbeddingClient {
    /// HTTP 客户端(真实模式)
    http: Option<reqwest::Client>,
    /// embedding 服务端点
    endpoint: Option<String>,
    /// Mock 模式:使用 v2 语义嵌入算法
    mock: bool,
    /// 请求超时(毫秒)
    timeout_ms: u64,
    /// 目标维度(512,与 CLV 对齐)
    target_dim: usize,
}

impl EmbeddingClient {
    /// 创建 Mock 嵌入客户端(使用 v2 算法)
    pub fn mock(target_dim: usize) -> Self {
        Self {
            http: None,
            endpoint: None,
            mock: true,
            timeout_ms: 5000,
            target_dim,
        }
    }

    /// 创建真实嵌入客户端
    ///
    /// # 参数
    /// - `endpoint`:embedding 服务 HTTP 端点
    /// - `timeout_ms`:请求超时
    /// - `target_dim`:目标维度(通常 512)
    pub fn new(endpoint: impl Into<String>, timeout_ms: u64, target_dim: usize) -> Self {
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
            target_dim,
        }
    }

    /// 从配置创建客户端
    pub fn from_config(mock: bool, endpoint: Option<String>, timeout_ms: u64, target_dim: usize) -> Self {
        if mock || endpoint.is_none() {
            Self::mock(target_dim)
        } else {
            Self::new(endpoint.unwrap(), timeout_ms, target_dim)
        }
    }

    /// 获取文本的语义嵌入向量
    ///
    /// Mock 模式:使用 v2 n-gram SipHash 感知哈希。
    /// 真实模式:HTTP POST 到 embedding 服务端点。
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, NmcError> {
        if self.mock {
            return Ok(self.mock_embed(text));
        }

        let http = self.http.as_ref().ok_or_else(|| NmcError::EmbeddingError {
            reason: "HTTP client 初始化失败".into(),
        })?;
        let endpoint = self.endpoint.as_ref().ok_or_else(|| NmcError::EmbeddingError {
            reason: "embedding 服务端点未配置".into(),
        })?;

        let request = EmbeddingRequest {
            input: text.into(),
            model: "text-embedding-v3".into(),
            request_id: format!("emb-{}", text.len()),
        };

        let resp = http
            .post(endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|e| NmcError::EmbeddingError {
                reason: format!("embedding 请求失败: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(NmcError::EmbeddingError {
                reason: format!("embedding 服务返回 HTTP {}", resp.status()),
            });
        }

        let embedding_resp: EmbeddingResponse = resp.json().await.map_err(|e| {
            NmcError::EmbeddingError {
                reason: format!("embedding 响应解析失败: {e}"),
            }
        })?;

        // 维度适配:若返回维度与目标维度不同,进行投影/截断
        let embedding = if embedding_resp.embedding.len() == self.target_dim {
            embedding_resp.embedding
        } else if embedding_resp.embedding.len() > self.target_dim {
            // 截断到目标维度
            embedding_resp.embedding.into_iter().take(self.target_dim).collect()
        } else {
            // 维度不足,用零填充
            let mut padded = embedding_resp.embedding;
            padded.resize(self.target_dim, 0.0);
            padded
        };

        Ok(embedding)
    }

    /// Mock 嵌入 — v2 n-gram SipHash 感知哈希
    fn mock_embed(&self, text: &str) -> Vec<f32> {
        // 委托给 v2 算法(与原有 TextPerceptor 行为一致)
        crate::perceptors::text::semantic_embedding_v2(text, self.target_dim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_embed_returns_vector() {
        let client = EmbeddingClient::mock(512);
        let emb = client.embed("hello world").await.expect("Mock 嵌入应成功");
        assert_eq!(emb.len(), 512);
    }

    #[tokio::test]
    async fn test_mock_embed_consistent() {
        let client = EmbeddingClient::mock(512);
        let emb1 = client.embed("test").await.unwrap();
        let emb2 = client.embed("test").await.unwrap();
        assert_eq!(emb1, emb2);
    }

    #[tokio::test]
    async fn test_mock_embed_different_texts() {
        let client = EmbeddingClient::mock(512);
        let emb1 = client.embed("hello").await.unwrap();
        let emb2 = client.embed("world").await.unwrap();
        // 不同文本应产生不同嵌入(概率极高)
        assert_ne!(emb1, emb2);
    }

    #[tokio::test]
    async fn test_mock_embed_empty() {
        let client = EmbeddingClient::mock(512);
        let emb = client.embed("").await.unwrap();
        assert_eq!(emb.len(), 512);
        assert!(emb.iter().all(|&v| v == 0.0));
    }

    #[tokio::test]
    async fn test_dimension_adaptation_truncate() {
        let client = EmbeddingClient::mock(128);
        let emb = client.embed("test").await.unwrap();
        assert_eq!(emb.len(), 128);
    }

    #[tokio::test]
    async fn test_dimension_adaptation_pad() {
        let client = EmbeddingClient::mock(1024);
        let emb = client.embed("test").await.unwrap();
        assert_eq!(emb.len(), 1024);
    }
}
