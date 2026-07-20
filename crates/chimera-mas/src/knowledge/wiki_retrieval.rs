//! WikiRetriever — Wiki 检索器(Task 18 §18.7,§18.10)
//!
//! 架构层归属:L9 Quest(chimera-mas knowledge 子模块,复用 L5 repo-wiki)
//! 核心职责:封装 `repo_wiki::WikiStore` 的 FTS5 全文检索 + 内存 KNN,
//! 用 `select_nth_unstable` (O(n)) 取 Top-K,避免 `sort_by` (O(n log n)) 开销。
//!
//! ## 风险阈值(§18.7)
//!
//! Wiki 条目 > 10000 时登记 `RiskLevel::High`(检索延迟可能超过 10ms 基线,
//! 参考 §20.5 `wiki_knn@1000 < 10ms` criterion benchmark)。低于阈值返回
//! `RiskLevel::Low`(在此场景表"未达风险阈值",等价于 §18.7 描述的 Unknown)。
//!
//! ## Top-K 算法(§4.1 规范)
//!
//! 用 `select_nth_unstable` 取 Top-K,时间复杂度 O(n),优于 `sort_by` O(n log n)。
//! - `select_nth_unstable(k)` 将前 k 个元素重排为"小于等于第 k 个"的子集
//! - 再用 `sort_by` 对前 k 个排序(规模已降至 k,可接受 O(k log k))
//!
//! ## 关键约束
//!
//! - rusqlite 调用必须 `spawn_blocking`(§4.4 反模式 2,Wiki 检索涉及)
//!   注:`WikiStore::search_fulltext` 内部已用 `with_read_conn` 包装 `spawn_blocking`,
//!   外层 `WikiRetriever::search` 不需要重复包装
//! - Top-K 用 `select_nth_unstable` (O(n)),禁止 `sort_by` 全排序(§4.1)
//! - 单函数 ≤ 200 行(§6.1 红线)

use std::sync::Arc;

use osa_coordinator::RiskLevel;
use repo_wiki::{WikiEntry, WikiStore};

use crate::error::{MasError, Result};

/// 默认风险阈值 — Wiki 条目超过此值登记 RiskLevel::High(§18.7)
///
/// WHY 10000:参考 §20.5 criterion benchmark `wiki_knn@1000 < 10ms`,
/// 1000 条目延迟 10ms,10000 条目线性外推 100ms 已影响用户体验,
/// 触发 High 风险登记让 PDCA 介入(§20.11 闭环告警)。
pub const DEFAULT_WIKI_RISK_THRESHOLD: usize = 10000;

/// Wiki 检索器 — 封装 WikiStore 的 FTS5 + 内存 KNN,Top-K via select_nth_unstable
///
/// ## 设计要点
///
/// - **复用 WikiStore**:不自实现 SQLite / FTS5 / 向量检索(ADR-026 决策 4/5)
/// - **Top-K O(n)**:`select_nth_unstable` 取前 top_k,避免全排序
/// - **风险阈值**:条目 > 10000 登记风险,供 PDCA §20.11 告警
#[derive(Clone)]
pub struct WikiRetriever {
    /// Wiki 存储(复用,FTS5 + 内存 KNN)
    wiki: Arc<WikiStore>,
    /// 风险阈值(默认 10000)
    risk_threshold: usize,
}

impl WikiRetriever {
    /// 创建 Wiki 检索器
    ///
    /// ## 参数
    /// - `wiki`:WikiStore 实例(Arc 共享,支持多 Agent 并发检索)
    /// - `risk_threshold`:风险阈值(条目数超过此值登记 High,默认 10000)
    pub fn new(wiki: Arc<WikiStore>, risk_threshold: usize) -> Self {
        Self {
            wiki,
            risk_threshold,
        }
    }

    /// 用默认风险阈值(10000)创建 Wiki 检索器
    pub fn with_default_threshold(wiki: Arc<WikiStore>) -> Self {
        Self::new(wiki, DEFAULT_WIKI_RISK_THRESHOLD)
    }

    /// 检索 Wiki — FTS5 全文召回 + Top-K via select_nth_unstable
    ///
    /// ## 参数
    /// - `query`:检索查询字符串
    /// - `top_k`:返回的 Top-K 上限
    ///
    /// ## 返回
    /// - `Ok(Vec<WikiEntry>)`:按相关度降序的 Top-K 条目(长度 ≤ top_k)
    /// - `Err(MasError::KnowledgeRetrievalFailed)`:Wiki 检索失败
    ///
    /// ## 性能
    ///
    /// - FTS5 全文检索:O(log n) ~ O(n)(取决于 SQLite 优化器)
    /// - Top-K 取前 K:O(n) via `select_nth_unstable`(§4.1 规范)
    /// - 前置 K 个排序:O(k log k)(规模已降至 k,可接受)
    ///
    /// ## 错误处理
    ///
    /// `WikiStore::search_fulltext` 内部已用 `with_read_conn` 包装 `spawn_blocking`,
    /// 外层无需重复包装(§4.4 反模式 2)。
    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<WikiEntry>> {
        // 1. 调用 WikiStore::search_fulltext(内部已 spawn_blocking,§4.4 反模式 2)
        let mut entries = self
            .wiki
            .search_fulltext(query.to_string())
            .await
            .map_err(|e| MasError::KnowledgeRetrievalFailed {
                reason: format!("WikiStore::search_fulltext failed: {e}"),
            })?;

        // 2. 为每个条目计算 score(简单相关度:query 在 content 中出现次数)
        //    WHY 简单计数:WikiStore 已通过 FTS5 召回相关条目,Top-K 仅需粗排,
        //    精细排序应由外层 AgentContext 与 NMC CLV 向量完成(§17.1 三级检索)
        let query_lower = query.to_lowercase();
        let mut scored: Vec<(u64, WikiEntry)> = entries
            .drain(..)
            .map(|e| {
                let content_lower = e.content.to_lowercase();
                let score = content_lower.matches(&query_lower).count() as u64;
                (score, e)
            })
            .collect();

        // 3. Top-K via select_nth_unstable(O(n),§4.1 规范)
        //    WHY 不用 sort_by:全排序 O(n log n),Top-K 只需前 K 个 O(n)
        if top_k > 0 && top_k < scored.len() {
            // select_nth_unstable 按 score 降序排列需 reverse 比较
            // partition_point 索引 = top_k - 1(0-based),前 top_k 个为最大值集合
            let partition_idx = top_k - 1;
            // WHY reverse 比较:select_nth_unstable 默认升序,我们要 Top-K 即最大值
            scored.select_nth_unstable_by(partition_idx, |a, b| b.0.cmp(&a.0));
            scored.truncate(top_k);
        }

        // 4. 对前 top_k 个按 score 降序排序(规模已降至 k,O(k log k) 可接受)
        // WHY sort_by_key + Reverse:clippy unnecessary_sort-by 建议,
        // 等价于 sort_by(|a, b| b.0.cmp(&a.0)) 但更符合 Rust 习惯
        scored.sort_by_key(|b| std::cmp::Reverse(b.0));

        // 5. 剥离 score,返回 WikiEntry 列表
        Ok(scored.into_iter().map(|(_, e)| e).collect())
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
            .finish()
    }
}
