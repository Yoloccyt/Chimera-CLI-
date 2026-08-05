//! 超窗兜底链 — 语料超窗时经两级检索装窗（PROBE P3.2）
//!
//! 对应架构层: L2 Memory（hcw-window 内部）
//! 对应设计: HCW-Sparse PROBE §2.5 P3.2（kvbsr→repo-wiki→hcw 检索链）
//!
//! # 链路（编排器注入，L2 不向上依赖）
//!
//! ```text
//! 语料 > 有效窗口（P3.1 effective_fold 折减）
//!   → provider(query, cap)           // 编排器闭包：kvbsr 块路由候选（≤3×窗口）
//!   → repo-wiki hybrid_search 精排    // FTS5 + CLV 近似 RRF 融合（provider 内部）
//!   → 返回 BlockScore 候选集           // 上层复用 P1 fill_zones/reorder_blocks 三区装窗
//!   → 发布 OverWindowFallbackTriggered // Normal 级（观测/审计）
//! ```
//!
//! # 依赖铁律
//!
//! hcw-window（L2）不 import kvbsr（L6）/repo-wiki（L5）——候选生成经
//! `Arc<dyn Fn>` 闭包由编排器注入（L0 风格依赖注入，先例 `recall/fine.rs`
//! VectorStore trait，ADR-033）；事件链走 EventBus（跨层通信唯一通道）。

use std::sync::Arc;

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::recall::types::BlockScore;

/// 候选上限系数（≤3× 有效窗口，P3.2 设计）
pub const CANDIDATE_CAP_FACTOR: usize = 3;

/// 候选生成器类型（编排器注入：kvbsr 块路由 → repo-wiki 精排 → BlockScore）
///
/// 签名：`Fn(&str query, usize cap) -> Vec<BlockScore>`——query 用于精排，
/// cap 为候选上限（3× 有效窗口）
///
/// WHY type 别名: 复杂闭包类型集中定义，避免散布在签名中（clippy::type_complexity）
pub type CandidateProvider = dyn Fn(&str, usize) -> Vec<BlockScore> + Send + Sync;

/// 超窗兜底结果
#[derive(Debug, Clone, PartialEq)]
pub struct OverWindowOutcome {
    /// 是否触发兜底（语料 > 有效窗口）
    pub triggered: bool,
    /// 候选集规模（kvbsr 块路由产出，已截断 ≤3× 窗口）
    pub candidate_count: usize,
    /// 精排后装窗候选（上层复用 P1 三区装窗）
    pub candidates: Vec<BlockScore>,
}

/// 超窗兜底链（PROBE P3.2）
///
/// # 参数注入
/// - `provider`: 候选生成闭包 `Fn(&str query, usize cap) -> Vec<BlockScore>`——
///   编排器构造（内部调 kvbsr build_blocks 分段 + repo-wiki hybrid_search 精排）
///
/// # 用法
///
/// ```rust,ignore
/// let fallback = OverWindowFallback::new(bus, provider);
/// let outcome = fallback.run("查询", corpus_tokens, effective_window);
/// if outcome.triggered { /* 复用 fill_zones/reorder_blocks 装窗 */ }
/// ```
pub struct OverWindowFallback {
    /// 事件总线（发布 OverWindowFallbackTriggered，Normal 级）
    event_bus: EventBus,
    /// 候选生成器（编排器注入；Send + Sync 满足 spawn 约束）
    provider: Arc<CandidateProvider>,
}

impl OverWindowFallback {
    /// 创建兜底链（注入候选生成器）
    ///
    /// # 参数
    /// - `event_bus`: 事件总线
    /// - `provider`: 候选生成闭包（kvbsr 分段 → repo-wiki 精排 → BlockScore）
    pub fn new(event_bus: EventBus, provider: Arc<CandidateProvider>) -> Self {
        Self {
            event_bus,
            provider,
        }
    }

    /// 执行超窗兜底判定与候选生成
    ///
    /// # 参数
    /// - `query`: 检索查询（传给 provider 精排）
    /// - `corpus_tokens`: 语料规模（token）
    /// - `effective_window`: 有效窗口（P3.1 effective_fold 折减后）
    ///
    /// # 返回
    /// `OverWindowOutcome`：
    /// - `triggered = false`: 未超窗——零开销返回（无 provider 调用、无事件）
    /// - `triggered = true`: 已生成候选（≤3× 有效窗口）并发布事件
    ///
    /// # 红线
    /// - 判定纯算术（无锁无 await）——热路径零影响
    /// - 事件发布 async（publish）——低频（仅超窗时）
    pub async fn run(
        &self,
        query: &str,
        corpus_tokens: u64,
        effective_window: u64,
    ) -> OverWindowOutcome {
        // 超窗判定（P3.1 折减语义）：语料 > 有效窗口才触发
        if corpus_tokens <= effective_window {
            return OverWindowOutcome {
                triggered: false,
                candidate_count: 0,
                candidates: Vec::new(),
            };
        }
        // 候选上限 = 3× 有效窗口（防候选爆炸）
        let cap = (effective_window as usize).saturating_mul(CANDIDATE_CAP_FACTOR);
        // PROBE P2: provider（CPU 密集：余弦/排序/RRF）移出 async worker——
        // spawn_blocking 隔离（闭包 Send+Sync 已满足）；超窗低频路径不阻塞主 runtime
        let provider = Arc::clone(&self.provider);
        let query_owned = query.to_string();
        let mut candidates = tokio::task::spawn_blocking(move || provider(&query_owned, cap))
            .await
            .unwrap_or_else(|_| Vec::new()); // JoinError（task panic）→ 空候选（上层走装窗 fallback）
        if candidates.len() > cap {
            // 截断到上限（provider 异常产出超限时防御）
            candidates.truncate(cap);
        }
        // 发布兜底触发事件（Normal 级——观测/审计，非关键路径）
        let _ = self
            .event_bus
            .publish(NexusEvent::OverWindowFallbackTriggered {
                metadata: EventMetadata::new("hcw-window::overwindow"),
                corpus_tokens,
                effective_window,
                candidate_count: candidates.len() as u32,
                loaded_count: candidates.len() as u32,
            })
            .await;
        OverWindowOutcome {
            triggered: true,
            candidate_count: candidates.len(),
            candidates,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 mock 候选生成器（固定产出）
    fn mock_provider(count: usize) -> Arc<CandidateProvider> {
        Arc::new(move |_query: &str, cap: usize| {
            let n = count.min(cap);
            (0..n)
                .map(|i| {
                    BlockScore::new(
                        format!("block-{i}"),
                        1.0 - (i as f32) * 0.01,
                        0.0,
                        "mock",
                        16,
                    )
                })
                .collect()
        })
    }

    #[tokio::test]
    async fn test_not_triggered_within_window() {
        // 未超窗：零开销返回（不调 provider、不发事件）
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let fallback = OverWindowFallback::new(bus, mock_provider(10));
        let outcome = fallback.run("q", 100_000, 600_000).await;
        assert!(!outcome.triggered);
        assert!(outcome.candidates.is_empty());
        // 无事件发布
        let timeout = tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await;
        assert!(timeout.is_err(), "未超窗不应发布事件");
    }

    #[tokio::test]
    async fn test_triggered_over_window() {
        // 超窗：候选生成 + 事件发布闭环
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let fallback = OverWindowFallback::new(bus, mock_provider(10));
        let outcome = fallback.run("q", 1_000_000, 600_000).await;
        assert!(outcome.triggered);
        assert_eq!(outcome.candidate_count, 10);
        assert_eq!(outcome.candidates.len(), 10);
        // 事件闭环（subscribe 先于 publish ✓）
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
        match timeout {
            Ok(Ok(NexusEvent::OverWindowFallbackTriggered {
                corpus_tokens,
                effective_window,
                candidate_count,
                ..
            })) => {
                assert_eq!(corpus_tokens, 1_000_000);
                assert_eq!(effective_window, 600_000);
                assert_eq!(candidate_count, 10);
            }
            other => panic!("应收到 OverWindowFallbackTriggered，实际: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_candidate_capped_at_3x_window() {
        // 候选上限 = 3× 有效窗口（provider 产出超限被截断）
        let bus = EventBus::new();
        let fallback = OverWindowFallback::new(bus, mock_provider(10_000));
        let outcome = fallback.run("q", 1_000_000, 100).await;
        assert!(outcome.triggered);
        assert_eq!(outcome.candidate_count, 300, "候选应截断到 3× 有效窗口");
    }

    #[tokio::test]
    async fn test_empty_candidates_still_triggered() {
        // provider 空产出：仍触发（事件发布，候选空——上层 fallback 到装窗路径）
        let bus = EventBus::new();
        let fallback = OverWindowFallback::new(bus, mock_provider(0));
        let outcome = fallback.run("q", 1_000_000, 600_000).await;
        assert!(outcome.triggered);
        assert!(outcome.candidates.is_empty());
    }
}
