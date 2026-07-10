//! 200K上下文分层管理 — Kimi长上下文窗口管理
//!
//! 对应架构层:L2 Memory
//! 对应创新点:P2-5 Kimi 200K上下文分层管理
//!
//! # 核心机制
//! 当上下文超过128K(L2窗口)时,启用分层管理策略:
//! - **Summarize**:对历史上下文进行摘要压缩
//! - **Hierarchical**:分层存储(近期全文+远期摘要)
//! - **Selective**:选择性加载(基于相关性评分只加载重要部分)
//!
//! # 设计决策
//! - 纯Rust实现,不依赖外部摘要服务(降级路径)
//! - 基于关键词重叠的简单相关性评分(作为Embedding的降级)
//! - 分层阈值可配置(默认:近期32K全文+剩余摘要)

use std::collections::HashSet;

/// 200K上下文管理器
#[derive(Debug, Clone)]
pub struct LongContextManager {
    /// 全文保留的token数(默认32K)
    full_text_threshold: usize,
    /// 摘要压缩比(默认10x)
    summary_ratio: usize,
    /// 选择性加载的Top-K比例(默认20%)
    selective_top_k: f32,
}

impl Default for LongContextManager {
    fn default() -> Self {
        Self {
            full_text_threshold: 32_768,
            summary_ratio: 10,
            selective_top_k: 0.2,
        }
    }
}

impl LongContextManager {
    /// 创建管理器
    pub fn new(full_text_threshold: usize, summary_ratio: usize, selective_top_k: f32) -> Self {
        Self {
            full_text_threshold,
            summary_ratio,
            selective_top_k: selective_top_k.clamp(0.0, 1.0),
        }
    }

    /// 分层管理上下文
    ///
    /// 输入:上下文条目列表(每个条目携带token数和内容)
    /// 输出:管理后的上下文(近期全文+远期摘要)
    pub fn manage<'a>(&self, entries: &'a [ContextEntry]) -> ManagedContext<'a> {
        let total_tokens: usize = entries.iter().map(|e| e.token_count).sum();

        if total_tokens <= self.full_text_threshold {
            // 小于阈值,全部保留全文
            return ManagedContext {
                full_text_entries: entries.to_vec(),
                summary_entries: vec![],
                strategy: ManagementStrategy::FullText,
            };
        }

        // 分层管理:近期全文 + 远期摘要
        let mut full_text = Vec::new();
        let mut summaries = Vec::new();
        let mut accumulated = 0usize;

        // 从最新(末尾)开始,优先保留全文
        for entry in entries.iter().rev() {
            if accumulated + entry.token_count <= self.full_text_threshold {
                full_text.push(entry.clone());
                accumulated += entry.token_count;
            } else {
                // 超出阈值的部分生成摘要
                let summary = self.summarize(entry);
                summaries.push(summary);
            }
        }

        // 反转回原始顺序
        full_text.reverse();
        summaries.reverse();

        ManagedContext {
            full_text_entries: full_text,
            summary_entries: summaries,
            strategy: ManagementStrategy::Hierarchical,
        }
    }

    /// 选择性加载 — 基于查询相关性只加载最相关的部分
    ///
    /// 输入:上下文条目和查询文本
    /// 输出:按相关性排序的Top-K条目
    pub fn selective_load<'a>(
        &self,
        entries: &'a [ContextEntry],
        query: &str,
    ) -> Vec<&'a ContextEntry> {
        let query_keywords = extract_keywords(query);
        let mut scored: Vec<(f32, &ContextEntry)> = entries
            .iter()
            .map(|e| {
                let entry_keywords = extract_keywords(&e.content);
                let score = jaccard_similarity(&query_keywords, &entry_keywords);
                (score, e)
            })
            .collect();

        // 按相关性降序排序
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let k = (entries.len() as f32 * self.selective_top_k).ceil() as usize;
        scored.into_iter().take(k.max(1)).map(|(_, e)| e).collect()
    }

    /// 简单摘要:取前N个token(作为真实摘要的降级)
    fn summarize(&self, entry: &ContextEntry) -> SummaryEntry {
        let summary_len = (entry.token_count / self.summary_ratio).max(1);
        let summary_content = if entry.content.len() > summary_len * 4 {
            // 粗略估计:1 token ≈ 4 bytes
            format!("{}...(truncated)", &entry.content[..summary_len * 4])
        } else {
            entry.content.clone()
        };

        SummaryEntry {
            original_id: entry.id.clone(),
            summary: summary_content,
            original_tokens: entry.token_count,
            summary_tokens: summary_len,
        }
    }
}

/// 上下文条目
#[derive(Debug, Clone)]
pub struct ContextEntry {
    /// 条目ID
    pub id: String,
    /// 内容
    pub content: String,
    /// Token数
    pub token_count: usize,
}

impl ContextEntry {
    /// 创建上下文条目
    pub fn new(id: impl Into<String>, content: impl Into<String>, token_count: usize) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            token_count,
        }
    }
}

/// 摘要条目
#[derive(Debug, Clone)]
pub struct SummaryEntry {
    /// 原始条目ID
    pub original_id: String,
    /// 摘要内容
    pub summary: String,
    /// 原始token数
    pub original_tokens: usize,
    /// 摘要token数
    pub summary_tokens: usize,
}

/// 管理后的上下文
#[derive(Debug, Clone)]
pub struct ManagedContext<'a> {
    /// 保留全文的条目
    pub full_text_entries: Vec<ContextEntry>,
    /// 摘要条目
    pub summary_entries: Vec<SummaryEntry>,
    /// 管理策略
    pub strategy: ManagementStrategy,
}

impl<'a> ManagedContext<'a> {
    /// 总token数(全文+摘要)
    pub fn total_tokens(&self) -> usize {
        let full: usize = self.full_text_entries.iter().map(|e| e.token_count).sum();
        let summary: usize = self.summary_entries.iter().map(|s| s.summary_tokens).sum();
        full + summary
    }

    /// 压缩比(原始/压缩后)
    pub fn compression_ratio(&self) -> f32 {
        let original: usize = self.full_text_entries.iter().map(|e| e.token_count).sum()
            + self.summary_entries.iter().map(|s| s.original_tokens).sum();
        let compressed = self.total_tokens();
        if compressed == 0 {
            return 1.0;
        }
        original as f32 / compressed as f32
    }
}

/// 管理策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagementStrategy {
    /// 全部保留全文
    FullText,
    /// 分层管理(近期全文+远期摘要)
    Hierarchical,
    /// 选择性加载
    Selective,
}

/// 提取关键词(简单实现)
fn extract_keywords(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|s| s.to_lowercase())
        .filter(|s| s.len() > 2)
        .collect()
}

/// Jaccard相似度
fn jaccard_similarity(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let intersection: usize = a.intersection(b).count();
    let union: usize = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f32 / union as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entries(count: usize, tokens_each: usize) -> Vec<ContextEntry> {
        (0..count)
            .map(|i| {
                ContextEntry::new(
                    format!("e-{i}"),
                    format!("Content of entry {i} with some keywords"),
                    tokens_each,
                )
            })
            .collect()
    }

    #[test]
    fn test_full_text_when_under_threshold() {
        let manager = LongContextManager::default();
        let entries = make_entries(10, 100); // 1000 tokens total
        let managed = manager.manage(&entries);
        assert_eq!(managed.strategy, ManagementStrategy::FullText);
        assert_eq!(managed.full_text_entries.len(), 10);
    }

    #[test]
    fn test_hierarchical_when_over_threshold() {
        let manager = LongContextManager::new(1000, 10, 0.2);
        let entries = make_entries(20, 100); // 2000 tokens total
        let managed = manager.manage(&entries);
        assert_eq!(managed.strategy, ManagementStrategy::Hierarchical);
        // 近期全文条目token数应 <= 1000
        let full_tokens: usize = managed
            .full_text_entries
            .iter()
            .map(|e| e.token_count)
            .sum();
        assert!(full_tokens <= 1000, "全文部分应 <= 阈值");
        // 应有摘要条目
        assert!(!managed.summary_entries.is_empty());
    }

    #[test]
    fn test_compression_ratio() {
        let manager = LongContextManager::new(1000, 10, 0.2);
        let entries = make_entries(20, 100);
        let managed = manager.manage(&entries);
        let ratio = managed.compression_ratio();
        assert!(ratio >= 1.0, "压缩比应 >= 1.0");
    }

    #[test]
    fn test_selective_load() {
        let manager = LongContextManager::new(1000, 10, 0.3);
        let entries = vec![
            ContextEntry::new("e1", "database schema design", 10),
            ContextEntry::new("e2", "frontend UI components", 10),
            ContextEntry::new("e3", "API endpoint documentation", 10),
            ContextEntry::new("e4", "database query optimization", 10),
        ];
        let selected = manager.selective_load(&entries, "database");
        // 应返回与"database"相关的条目
        assert!(!selected.is_empty());
        assert!(selected.iter().any(|e| e.id == "e1" || e.id == "e4"));
    }
}
