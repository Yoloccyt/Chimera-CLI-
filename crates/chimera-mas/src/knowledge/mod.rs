//! Knowledge 子模块 — 三级知识协同检索(Task 18,§18 知识协同)
//!
//! 架构层归属:L9 Quest(chimera-mas 内部子模块)
//! 核心职责:为 Agent 提供专家旗舰咨询、同僚互询、Wiki 检索三级知识链,
//! 支撑 Agent 在本地记忆不足时按"本地 → 同僚 → Wiki"顺序检索知识。
//!
//! ## 三级检索链(§18.1 设计)
//!
//! ```text
//! Agent 提问
//!   ↓
//! 1. 本地 mlc L0/L1(<1ms,§17.1 Task 17)— 短路命中则返回
//!   ↓ miss
//! 2. 同僚互询(脱敏上下文,经 AgentConsultRequested 事件)— 短路命中则返回
//!   ↓ miss
//! 3. Wiki 检索(repo-wiki FTS5 + 内存 KNN,Top-K via select_nth_unstable)
//!   ↓
//! 返回结果(或 KnowledgeRetrievalFailed 错误)
//! ```
//!
//! ## 子模块组织
//!
//! - `expert_consult` — ExpertConsultant 专家旗舰咨询(SLA + 信号量并发控制)
//! - `mutual_inquiry` — MutualInquirer 同僚互询(正则脱敏 + ContextIsolationGuard 协同)
//! - `wiki_retrieval` — WikiRetriever Wiki 检索(FTS5 + Top-K O(n))
//!
//! ## 关键约束(§6.2 红线)
//!
//! - `tokio::broadcast` 先 subscribe 再 spawn(§4.4 反模式 3)
//! - Critical 安全事件用 mpsc(AgentTaskFailed 走 publish_critical,§6.2 红线)
//! - Top-K 用 `select_nth_unstable` (O(n)),禁止 `sort_by`(§4.1)
//! - rusqlite 调用必须 `spawn_blocking`(§4.4 反模式 2,Wiki 检索涉及)
//! - 单函数 ≤ 200 行(§6.1 红线)
//!
//! ## 相关 ADR
//!
//! - ADR-026 决策 5:复用 event-bus / quest-engine / repo-wiki,不新建 AgentMessageBus

pub mod expert_consult;
pub mod mutual_inquiry;
pub mod wiki_retrieval;

// === 关键类型重导出 ===
pub use expert_consult::{ConsultSla, ExpertConsultant};
pub use mutual_inquiry::MutualInquirer;
pub use wiki_retrieval::WikiRetriever;

/// 三级知识检索链 — 顺序执行本地 / 同僚 / Wiki 检索
///
/// 设计参考 §18.1:本地 mlc L0/L1(<1ms)→ 同僚互询(脱敏)→ Wiki(FTS5 + KNN)。
/// 任一级命中即短路返回,避免不必要的下游检索开销。
#[derive(Debug, Clone)]
pub struct KnowledgeChain {
    /// 本地记忆检索结果(可选,由调用方提供,如 mlc L0/L1 命中)
    pub local_result: Option<String>,
    /// 同僚互询器(脱敏 + AgentConsultRequested)
    pub inquirer: Option<MutualInquirer>,
    /// Wiki 检索器(FTS5 + 内存 KNN)
    pub wiki: Option<WikiRetriever>,
}

impl KnowledgeChain {
    /// 构造三级检索链
    pub fn new(
        local_result: Option<String>,
        inquirer: Option<MutualInquirer>,
        wiki: Option<WikiRetriever>,
    ) -> Self {
        Self {
            local_result,
            inquirer,
            wiki,
        }
    }

    /// 顺序执行三级检索 — 本地命中短路,否则依次尝试同僚互询与 Wiki
    ///
    /// ## 参数
    /// - `query`:检索查询字符串
    /// - `top_k`:Wiki 检索的 Top-K 上限
    pub async fn search(&self, query: &str, top_k: usize) -> crate::error::Result<String> {
        // 一级:本地 mlc L0/L1 命中即短路(§18.1,<1ms)
        if let Some(local) = &self.local_result {
            return Ok(local.clone());
        }
        // 二级:同僚互询(经 create_safe_summary 脱敏 + AgentConsultRequested)
        if let Some(inquirer) = &self.inquirer {
            let peer_id = "peer-agent"; // 默认同僚 ID,实际由调用方注入
            if let Ok(answer) = inquirer.inquire(peer_id, query).await {
                return Ok(answer);
            }
        }
        // 三级:Wiki 检索(FTS5 + 内存 KNN,Top-K via select_nth_unstable)
        if let Some(wiki) = &self.wiki {
            let entries = wiki.search(query, top_k).await?;
            if !entries.is_empty() {
                // 拼接 Top-K 条目标题作为检索结果摘要
                let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
                return Ok(titles.join("; "));
            }
        }
        // 三级全 miss:返回 KnowledgeRetrievalFailed
        Err(crate::error::MasError::KnowledgeRetrievalFailed {
            reason: format!("All 3-tier knowledge chain missed for query: {query}"),
        })
    }
}
