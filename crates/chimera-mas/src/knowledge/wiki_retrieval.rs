//! WikiRetriever — Wiki 检索器(Task 18 §18.7,§18.10;P1-2 检索管道收敛)
//!
//! 架构层归属:L9 Quest(chimera-mas knowledge 子模块,复用 L5 repo-wiki)
//! 核心职责:封装 `repo_wiki::WikiStore` 的混合检索(`hybrid_query`),
//! HNSW dense(可选 NMC 编码) + FTS5 sparse 由 L5 侧 RRF 融合(P1-2),
//! 本层不再自实现粗排——检索能力收敛到 L5 单一实现(Ω₆ Reuse)。
//!
//! ## 风险阈值(§18.7)
//!
//! Wiki 条目 > 10000 时登记 `RiskLevel::High`(检索延迟可能超过 10ms 基线,
//! 参考 §20.5 `wiki_knn@1000 < 10ms` criterion benchmark)。低于阈值返回
//! `RiskLevel::Low`(在此场景表"未达风险阈值",等价于 §18.7 描述的 Unknown)。
//!
//! ## 关键约束
//!
//! - rusqlite 调用必须 `spawn_blocking`(§4.4 反模式 2,Wiki 检索涉及)
//!   注:`WikiStore::hybrid_query` 内部已用 `with_read_conn` 包装 `spawn_blocking`,
//!   外层 `WikiRetriever::search` 不需要重复包装
//! - 单函数 ≤ 200 行(§6.1 红线)

use std::sync::Arc;

use nmc_encoder::{PerceptionInput, Perceptor};
use osa_coordinator::RiskLevel;
use repo_wiki::{WikiEntry, WikiError, WikiStore};

use crate::error::{MasError, Result};

/// 默认风险阈值 — Wiki 条目超过此值登记 RiskLevel::High(§18.7)
///
/// WHY 10000:参考 §20.5 criterion benchmark `wiki_knn@1000 < 10ms`,
/// 1000 条目延迟 10ms,10000 条目线性外推 100ms 已影响用户体验,
/// 触发 High 风险登记让 PDCA 介入(§20.11 闭环告警)。
pub const DEFAULT_WIKI_RISK_THRESHOLD: usize = 10000;

/// Wiki 检索器 — 封装 WikiStore 的 hybrid_query(RRF 融合,P1-2)
///
/// ## 设计要点
///
/// - **复用 WikiStore**:不自实现 SQLite / FTS5 / 向量检索 / RRF(ADR-026 决策 4/5)
/// - **可选 NMC 编码器**:`with_text_encoder` 注入后 dense 通道启用;
///   无编码器时检索退化 sparse-only(行为与历史版本一致)
/// - **风险阈值**:条目 > 10000 登记风险,供 PDCA §20.11 告警
#[derive(Clone)]
pub struct WikiRetriever {
    /// Wiki 存储(复用,hybrid_query 双通道检索)
    wiki: Arc<WikiStore>,
    /// 风险阈值(默认 10000)
    risk_threshold: usize,
    /// 可选的 NMC 文本编码器(P1-2)— None 时 dense 通道退化 sparse-only
    ///
    /// WHY Arc 包装:TextPerceptor 内部持有 tokenizer/ONNX 计划,可能不可 Clone;
    /// Arc 共享使 WikiRetriever 保持 Clone(多 Agent 并发检索)。
    encoder: Option<Arc<nmc_encoder::TextPerceptor>>,
}

impl WikiRetriever {
    /// 创建 Wiki 检索器(无编码器,sparse-only 模式)
    ///
    /// ## 参数
    /// - `wiki`:WikiStore 实例(Arc 共享,支持多 Agent 并发检索)
    /// - `risk_threshold`:风险阈值(条目数超过此值登记 High,默认 10000)
    pub fn new(wiki: Arc<WikiStore>, risk_threshold: usize) -> Self {
        Self {
            wiki,
            risk_threshold,
            encoder: None,
        }
    }

    /// 用默认风险阈值(10000)创建 Wiki 检索器(无编码器)
    pub fn with_default_threshold(wiki: Arc<WikiStore>) -> Self {
        Self::new(wiki, DEFAULT_WIKI_RISK_THRESHOLD)
    }

    /// 创建注入 NMC 文本编码器的 Wiki 检索器(P1-2 dense 通道启用)
    ///
    /// ## 参数
    /// - `wiki`:WikiStore 实例
    /// - `risk_threshold`:风险阈值
    /// - `encoder`:NMC 文本感知器(查询文本 → 嵌入向量)
    pub fn with_text_encoder(
        wiki: Arc<WikiStore>,
        risk_threshold: usize,
        encoder: nmc_encoder::TextPerceptor,
    ) -> Self {
        Self {
            wiki,
            risk_threshold,
            encoder: Some(Arc::new(encoder)),
        }
    }

    /// 检索 Wiki — L5 hybrid_query(RRF 融合)+ 按序批量回取条目(P1-2)
    ///
    /// ## 参数
    /// - `query`:检索查询字符串
    /// - `top_k`:返回的 Top-K 上限
    ///
    /// ## 返回
    /// - `Ok(Vec<WikiEntry>)`:按相关度降序的 Top-K 条目(长度 ≤ top_k)
    /// - `Err(MasError::KnowledgeRetrievalFailed)`:Wiki 检索失败
    ///
    /// ## 错误策略(WHY)
    /// 查询编码失败或嵌入维度与存储不匹配时降级 sparse-only 并记录 warning——
    /// 检索永不因编码器/维度契约问题而失败(渐进接入语义,历史行为等价)。
    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<WikiEntry>> {
        // 1. 编码查询向量(可选):编码失败降级 sparse-only
        let query_embedding: Option<Vec<f32>> = match &self.encoder {
            Some(encoder) => {
                let input = PerceptionInput::Text(query.to_string());
                match encoder.perceive(&input) {
                    Ok(element) => Some(element.embedding),
                    Err(err) => {
                        tracing::warn!(error = %err, "NMC 查询编码失败,降级 sparse-only");
                        None
                    }
                }
            }
            None => None,
        };

        // 2. hybrid_query(内部已 spawn_blocking,§4.4 反模式 2)
        let results = match self
            .wiki
            .hybrid_query(query, query_embedding.as_deref(), top_k)
            .await
        {
            Ok(r) => r,
            // 维度不匹配:编码器维度(如 256/384)与存储 vector_dim(如 512)不一致,
            // 降级 sparse-only 保持检索可用(与无编码器行为一致)
            Err(WikiError::EmbeddingDimensionMismatch { expected, actual }) => {
                tracing::warn!(expected, actual, "查询嵌入维度不匹配,降级 sparse-only");
                self.wiki
                    .hybrid_query(query, None, top_k)
                    .await
                    .map_err(|e| MasError::KnowledgeRetrievalFailed {
                        reason: format!("WikiStore::hybrid_query failed: {e}"),
                    })?
            }
            Err(e) => {
                return Err(MasError::KnowledgeRetrievalFailed {
                    reason: format!("WikiStore::hybrid_query failed: {e}"),
                });
            }
        };

        // 3. 按融合排序回取条目(串行 get,top_k 规模小可接受)
        let mut entries = Vec::with_capacity(results.len());
        for r in results {
            if let Some(entry) =
                self.wiki
                    .get(r.doc_id)
                    .await
                    .map_err(|e| MasError::KnowledgeRetrievalFailed {
                        reason: format!("WikiStore::get failed: {e}"),
                    })?
            {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// 检查风险等级 — 条目 > threshold 返回 High,否则 Low(§18.7)
    ///
    /// ## 返回
    /// - `RiskLevel::High`:Wiki 条目数 > risk_threshold(默认 10000)
    /// - `RiskLevel::Low`:条目数 ≤ risk_threshold(表"未达风险阈值")
    ///
    /// ## 注意
    ///
    /// 此方法调用 `wiki.count().await`,涉及 SQLite 查询(内部已 spawn_blocking)。
    /// 频繁调用可能影响性能,建议在 PDCA check 阶段(§20.8)周期性调用。
    pub async fn check_risk(&self) -> RiskLevel {
        match self.wiki.count().await {
            Ok(count) => {
                if (count as usize) > self.risk_threshold {
                    RiskLevel::High
                } else {
                    RiskLevel::Low
                }
            }
            // 查询失败时返回 High(保守策略,触发 PDCA 排查)
            Err(_) => RiskLevel::High,
        }
    }

    /// 获取当前风险阈值
    pub fn risk_threshold(&self) -> usize {
        self.risk_threshold
    }
}

/// 手动实现 Debug — WikiStore 未实现 Debug,显示 Arc 指针地址(§4.1 规范)
impl std::fmt::Debug for WikiRetriever {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WikiRetriever")
            .field("wiki", &"<WikiStore shared Arc>")
            .field("risk_threshold", &self.risk_threshold)
            .field("encoder", &self.encoder.as_ref().map(|_| "<TextPerceptor>"))
            .finish()
    }
}
