//! Prompt 压缩 — LLMLingua-2 Python sidecar 接口（ADR-069 Token 效率优化）
//!
//! 对应架构层: L10 Interface（mca-gateway）
//!
//! # WHY sidecar
//! LLMLingua-2 是 Python/PyTorch 生态（transformers + CUDA），Rust 无等价实现。
//! 通过 `tokio::process::Command` 调用 Python sidecar 进程，stdin/stdout JSON 协议。
//! 遵守 `#![forbid(unsafe_code)]`——进程间通信无 unsafe。
//!
//! # 降级策略
//! sidecar 不可用（未安装/超时/崩溃）时 graceful degradation：返回原文不压缩。
//! 不阻塞请求主路径，仅 token 消耗上升（回退到无压缩基线）。
//!
//! # 调用时机
//! 非热路径：仅在首次构造 system prompt 的 Layer 3（repo_context）时调用一次，
//! 压缩结果随 NormalizedPrompt 缓存，后续轮次复用（前缀稳定性保证）。

use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::AffinityError;

/// 默认 sidecar 超时（秒）
///
/// WHY 10s: LLMLingua-2 压缩 4K token 约 2-3s（GPU）/ 5-8s（CPU），
/// 10s 覆盖 CPU 冷启动 + 余量。超时即 fallback 到原文。
const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Prompt 压缩器 — LLMLingua-2 Python sidecar 客户端
///
/// 无状态（每次调用 spawn 新进程），生产环境可升级为进程池预热。
#[derive(Debug, Clone)]
pub struct PromptCompressor {
    /// Python sidecar 命令（如 "python" 或 "python3"）
    sidecar_cmd: String,
    /// sidecar 脚本路径（如 "scripts/llmlingua_compress.py"）
    sidecar_script: String,
    /// 调用超时
    timeout: Duration,
}

impl PromptCompressor {
    /// 创建压缩器（默认超时 10s）
    pub fn new(sidecar_cmd: impl Into<String>, sidecar_script: impl Into<String>) -> Self {
        Self {
            sidecar_cmd: sidecar_cmd.into(),
            sidecar_script: sidecar_script.into(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    /// 覆盖超时（测试用）
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 压缩文本 — 调用 LLMLingua-2 sidecar
    ///
    /// # 协议
    /// stdin 输入 JSON: `{"text": "...", "target_ratio": 0.5}`
    /// stdout 输出 JSON: `{"compressed": "...", "actual_ratio": 0.48}`
    ///
    /// # 降级
    /// 任何失败（进程不存在/超时/JSON 解析错误）均返回原文（Ok(原文)），
    /// 不中断请求主路径。调用方可通过返回值 == 原文判断是否实际压缩。
    pub async fn compress(&self, text: &str, target_ratio: f32) -> Result<String, AffinityError> {
        // 空文本或比率 >= 1.0 无需压缩
        if text.is_empty() || target_ratio >= 1.0 {
            return Ok(text.to_string());
        }

        let input = serde_json::json!({
            "text": text,
            "target_ratio": target_ratio,
        });

        let result = tokio::time::timeout(self.timeout, self.run_sidecar(&input.to_string())).await;

        match result {
            Ok(Ok(compressed)) => Ok(compressed),
            // sidecar 执行失败 → fallback 原文
            Ok(Err(e)) => {
                tracing::warn!(
                    error = %e,
                    "LLMLingua-2 sidecar failed, falling back to uncompressed prompt"
                );
                Ok(text.to_string())
            }
            // 超时 → fallback 原文
            Err(_) => {
                tracing::warn!(
                    timeout_secs = self.timeout.as_secs(),
                    "LLMLingua-2 sidecar timeout, falling back to uncompressed prompt"
                );
                Ok(text.to_string())
            }
        }
    }

    /// 内部：spawn sidecar 进程并通信
    async fn run_sidecar(&self, input_json: &str) -> Result<String, AffinityError> {
        let mut child = Command::new(&self.sidecar_cmd)
            .arg(&self.sidecar_script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| AffinityError::Transport {
                route_key: "llmlingua-sidecar".to_string(),
                reason: format!("failed to spawn sidecar: {e}"),
                retryable: false,
            })?;

        // 写入 stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input_json.as_bytes())
                .await
                .map_err(|e| AffinityError::Transport {
                    route_key: "llmlingua-sidecar".to_string(),
                    reason: format!("failed to write to sidecar stdin: {e}"),
                    retryable: false,
                })?;
            // 关闭 stdin 触发 sidecar 处理
            drop(stdin);
        }

        // 等待输出
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| AffinityError::Transport {
                route_key: "llmlingua-sidecar".to_string(),
                reason: format!("sidecar process error: {e}"),
                retryable: false,
            })?;

        if !output.status.success() {
            return Err(AffinityError::Transport {
                route_key: "llmlingua-sidecar".to_string(),
                reason: format!("sidecar exited with {:?}", output.status),
                retryable: false,
            });
        }

        // 解析 stdout JSON
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout_str).map_err(|e| AffinityError::Transport {
                route_key: "llmlingua-sidecar".to_string(),
                reason: format!("sidecar output parse error: {e}"),
                retryable: false,
            })?;

        parsed["compressed"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| AffinityError::Transport {
                route_key: "llmlingua-sidecar".to_string(),
                reason: "sidecar output missing 'compressed' field".to_string(),
                retryable: false,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_creation() {
        let c = PromptCompressor::new("python3", "scripts/llmlingua_compress.py");
        assert_eq!(c.sidecar_cmd, "python3");
        assert_eq!(c.timeout, Duration::from_secs(10));
    }

    #[test]
    fn compressor_with_timeout() {
        let c = PromptCompressor::new("python", "s.py").with_timeout(Duration::from_secs(5));
        assert_eq!(c.timeout, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn compress_empty_text_returns_empty() {
        let c = PromptCompressor::new("python3", "nonexistent.py");
        let result = c.compress("", 0.5).await.unwrap();
        assert_eq!(result, "");
    }

    #[tokio::test]
    async fn compress_ratio_ge_1_returns_original() {
        let c = PromptCompressor::new("python3", "nonexistent.py");
        let text = "some content";
        let result = c.compress(text, 1.0).await.unwrap();
        assert_eq!(result, text);
    }

    #[tokio::test]
    async fn compress_sidecar_not_found_falls_back_to_original() {
        // sidecar 不存在时应 graceful degradation 到原文
        let c = PromptCompressor::new("nonexistent_python_binary_xyz", "s.py")
            .with_timeout(Duration::from_secs(2));
        let text = "This is a long prompt that would normally be compressed";
        let result = c.compress(text, 0.5).await.unwrap();
        assert_eq!(result, text, "sidecar 不可用时应返回原文");
    }
}
