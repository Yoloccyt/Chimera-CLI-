//! rl-client 通道 — Rust ↔ Python 训练服务轻量传输（Milestone C-3）
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §5.1 P4 / §6 C-3
//! 裁决: 模块嵌入 chimera-cli + `rl-client` feature gate（默认 off）,
//! 否决独立 crate 与 tonic/prost 默认引入（ADR-049 决策 1 哲学 +
//! binary <50MB 红线——reqwest 仅 feature 开启时编译）。
//!
//! # 传输协议（轻量 HTTP/JSON）
//!
//! - `POST /experiences`：批量上传 RewardSignal（R1 数据面经验回放）
//! - `GET /health`：训练服务健康检查
//!
//! # 依赖铁律
//!
//! L10 chimera-cli → L0 nexus-contracts（RewardSignal 载荷）合规；
//! 与 Python 训练服务为跨进程通信，不进入 Rust workspace。
//!
//! # R2 冻结声明（ADR-042）
//!
//! 本通道仅传输 R1 数据面信号；R2 训练面解冻后由训练服务消费。

use anyhow::{anyhow, Result};
use nexus_contracts::reward::RewardSignal;

/// 经验批量上传响应（服务端确认）
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PushSummary {
    /// 服务端接收条数
    pub accepted: usize,
    /// 服务端消息（可选）
    pub message: Option<String>,
}

/// rl-client 契约 — 训练服务通信的统一入口
///
/// 真实实现 `HttpRlClient`（feature-gated）；编排器可注入 mock（测试/影子）。
pub trait RlClient: Send + Sync {
    /// 批量上传经验信号（R1 数据面）
    fn push_experiences(&self, batch: &[RewardSignal]) -> Result<PushSummary>;

    /// 训练服务健康检查
    fn health_check(&self) -> Result<bool>;
}

/// HTTP/JSON 轻量实现（feature-gated）
#[cfg(feature = "rl-client")]
#[derive(Debug, Clone)]
pub struct HttpRlClient {
    /// 训练服务基地址（如 http://127.0.0.1:8000）
    base_url: String,
    /// 复用连接池的 blocking 客户端
    http: reqwest::blocking::Client,
}

#[cfg(feature = "rl-client")]
impl HttpRlClient {
    /// 创建客户端（超时 5s，防训练服务挂起阻塞主链路）
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| anyhow!("rl-client HTTP 客户端初始化失败: {e}"))?;
        Ok(Self {
            base_url: base_url.into(),
            http,
        })
    }
}

#[cfg(feature = "rl-client")]
impl RlClient for HttpRlClient {
    fn push_experiences(&self, batch: &[RewardSignal]) -> Result<PushSummary> {
        if batch.is_empty() {
            return Ok(PushSummary {
                accepted: 0,
                message: Some("空批次".into()),
            });
        }
        let url = format!("{}/experiences", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(url)
            .json(batch)
            .send()
            .map_err(|e| anyhow!("rl-client 上传失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(anyhow!("rl-client 上传被拒: HTTP {}", resp.status()));
        }
        resp.json::<PushSummary>()
            .map_err(|e| anyhow!("rl-client 响应解析失败: {e}"))
    }

    fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .get(url)
            .send()
            .map_err(|e| anyhow!("rl-client 健康检查失败: {e}"))?;
        Ok(resp.status().is_success())
    }
}

/// 空实现 — feature off 或未配置训练服务时的安全回退
///
/// WHY 非 Option：调用方（经验收集器）可无条件持有 `Arc<dyn RlClient>`，
/// 未配置时静默 no-op（R1 数据面不阻塞主链路）。
#[derive(Debug, Default)]
pub struct NoopRlClient;

impl RlClient for NoopRlClient {
    fn push_experiences(&self, _batch: &[RewardSignal]) -> Result<PushSummary> {
        Ok(PushSummary {
            accepted: 0,
            message: Some("rl-client 未配置（no-op）".into()),
        })
    }

    fn health_check(&self) -> Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_client_is_safe_fallback() {
        let client = NoopRlClient;
        let summary = client.push_experiences(&[]).expect("no-op 不应失败");
        assert_eq!(summary.accepted, 0);
        assert!(!client.health_check().expect("健康检查不应失败"));
    }
}
