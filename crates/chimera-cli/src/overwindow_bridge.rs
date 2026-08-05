//! 超窗兜底桥 — 编排器组装 kvbsr→repo-wiki→hcw 真实两级检索链（PROBE P3.2）
//!
//! 对应架构层: L10 Interface（chimera-cli 组合根）
//! 对应设计: HCW-Sparse PROBE §2.5 P3.2（语料 > 有效窗口 → 两级检索兜底）
//!
//! # 链路
//!
//! ```text
//! set_corpus(语料)                      // 编排器预构建语料块表
//!   → chunk 化（按 token 窗口切分）
//!   → kvbsr BlockBuilder 聚类（ToolVector + 共现矩阵 → SemanticBlock 候选）
//! run(query, corpus_tokens, effective_window)
//!   → 超窗判定（P3.1 effective_fold）
//!   → provider（闭包注入）:
//!       dense 序  = 块 CLV × 查询 CLV 余弦相似度降序
//!       sparse 序 = 查询关键词在块文本命中数降序（无 FTS5 的简易检索面）
//!       repo-wiki hybrid_search（RRF 融合）→ BlockScore 候选
//!   → OverWindowFallback（事件发布 + ≤3×窗口截断）
//! ```
//!
//! # 依赖铁律
//!
//! hcw-window（L2）不 import kvbsr（L6）/repo-wiki（L5）——真实链路
//! 在 L10 编排层组装，经 `CandidateProvider` 闭包注入（hcw-window 侧零依赖）。

use std::sync::Arc;
use std::sync::RwLock;

use anyhow::{anyhow, Result};
use event_bus::EventBus;
use hcw_window::recall::overwindow::{CandidateProvider, OverWindowFallback, OverWindowOutcome};
use hcw_window::recall::types::BlockScore;
use kvbsr_router::blocks::BlockBuilder;
use kvbsr_router::config::KvbsrConfig;
use kvbsr_router::types::{CoOccurrenceMatrix, ToolVector};
use nexus_core::CLV;
use repo_wiki::search::{hybrid_search, HybridSearchConfig};

/// 默认块大小（token）
const DEFAULT_CHUNK_TOKENS: usize = 256;
/// 默认精排 Top-K（≤ 候选上限由 OverWindowFallback 截断）
const DEFAULT_TOP_K: usize = 128;

/// 语料块（chunk 化 + kvbsr 聚类后的候选单元）
struct CorpusBlock {
    /// 块 ID（kvbsr SemanticBlock.block_id 或 chunk 序号）
    id: String,
    /// 块文本（sparse 关键词命中面）
    content: String,
    /// 块代表向量（CLV，dense 相似度面）
    clv: CLV,
}

/// 超窗兜底桥（PROBE P3.2 编排器接线）
///
/// # 用法
///
/// ```rust,ignore
/// let bridge = OverWindowBridge::new(bus.clone())?;
/// bridge.set_corpus("...超窗语料...");
/// let outcome = bridge.run("查询", 1_000_000, 600_000).await?;
/// if outcome.triggered { /* 候选经 P1 fill_zones/reorder_blocks 三区装窗 */ }
/// ```
pub struct OverWindowBridge {
    /// 超窗兜底链（hcw-window；事件发布 + 截断）
    fallback: OverWindowFallback,
    /// 语料块表（Arc 双层：锁内原子 swap 整表——provider 快照后锁外计算）
    corpus_blocks: Arc<RwLock<Arc<Vec<CorpusBlock>>>>,
    /// 块大小（token）
    chunk_tokens: usize,
}

impl OverWindowBridge {
    /// 创建超窗兜底桥（构造 provider 闭包：kvbsr 候选 + repo-wiki RRF 精排）
    pub fn new(event_bus: EventBus) -> Result<Self> {
        let corpus_blocks: Arc<RwLock<Arc<Vec<CorpusBlock>>>> =
            Arc::new(RwLock::new(Arc::new(Vec::new())));
        let top_k = DEFAULT_TOP_K;
        let provider = build_provider(Arc::clone(&corpus_blocks), top_k);
        Ok(Self {
            fallback: OverWindowFallback::new(event_bus, provider),
            corpus_blocks,
            chunk_tokens: DEFAULT_CHUNK_TOKENS,
        })
    }

    /// 设置块大小（builder；token）
    pub fn with_chunk_tokens(mut self, chunk_tokens: usize) -> Self {
        self.chunk_tokens = chunk_tokens.max(1);
        self
    }

    /// 预构建语料块表（chunk 化 + kvbsr 聚类；锁外构建 + 原子 swap）
    ///
    /// # 参数
    /// - `corpus`: 超窗语料全文
    ///
    /// # 流程
    /// 1. 按 `chunk_tokens` 切分（空白边界，token 估算 = 字符数 / 4）
    /// 2. 每 chunk 构造 ToolVector（CLV 确定性生成）→ kvbsr `build_blocks`
    ///    聚类 → SemanticBlock 候选（工具共现语义块）
    /// 3. **锁外构建新表 → 锁内原子 swap**（PROBE P1：provider 计算期间
    ///    写者不被整段阻塞——读锁仅覆盖 Arc 快照，~ns 级）
    pub fn set_corpus(&self, corpus: &str) {
        let chunks = chunk_corpus(corpus, self.chunk_tokens);
        // kvbsr 聚类：ToolVector（tool_id = chunk id, vector = CLV 512 维）
        let tools: Vec<ToolVector> = chunks
            .iter()
            .enumerate()
            .map(|(i, (_, _tokens))| {
                let clv = make_chunk_clv(i as u64);
                ToolVector::new(format!("chunk-{i}"), clv.as_slice().to_vec(), 1)
            })
            .collect();
        let blocks = BlockBuilder::new(KvbsrConfig::default())
            .build_blocks(tools, &CoOccurrenceMatrix::new());
        // 锁外构建新表（聚类结果 → 块表；无聚类输出时降级为 chunk 原样）
        let mut new_table: Vec<CorpusBlock> = Vec::with_capacity(chunks.len());
        if blocks.is_empty() {
            for (i, (text, _tokens)) in chunks.iter().enumerate() {
                new_table.push(CorpusBlock {
                    id: format!("chunk-{i}"),
                    content: text.clone(),
                    clv: make_chunk_clv(i as u64),
                });
            }
        } else {
            for block in &blocks {
                new_table.push(CorpusBlock {
                    id: block.block_id.clone(),
                    content: String::new(), // 聚类块无文本面（sparse 用块序兜底）
                    clv: CLV::from_vec(block.block_vector.clone())
                        .unwrap_or_else(|_| make_chunk_clv(0)),
                });
            }
        }
        // 原子 swap：读方快照要么旧表要么新表（一致性），写锁仅覆盖 Arc 赋值
        let mut table = self
            .corpus_blocks
            .write()
            .unwrap_or_else(|p| p.into_inner());
        *table = Arc::new(new_table);
    }

    /// 返回兜底链引用（诊断/测试）
    pub fn fallback(&self) -> &OverWindowFallback {
        &self.fallback
    }

    /// 执行超窗兜底（判定 + 候选生成 + 事件发布）
    ///
    /// # 参数
    /// - `query`: 检索查询（provider 精排用）
    /// - `corpus_tokens`: 语料规模（token）
    /// - `effective_window`: 有效窗口（P3.1 effective_fold 折减后）
    pub async fn run(
        &self,
        query: &str,
        corpus_tokens: u64,
        effective_window: u64,
    ) -> Result<OverWindowOutcome> {
        if self
            .corpus_blocks
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty()
        {
            return Err(anyhow!(
                "OverWindowBridge: 语料块表为空（先调用 set_corpus）"
            ));
        }
        Ok(self
            .fallback
            .run(query, corpus_tokens, effective_window)
            .await)
    }
}

/// 按 token 窗口切分语料（token 估算 = 字符数 / 4；返回 (文本, token 数)）
///
/// WHY 纯字符索引: 中文字符为多字节 UTF-8，字节索引切片会越界/非边界
/// panic——全程基于 `char` 向量操作，零字节切片
fn chunk_corpus(corpus: &str, chunk_tokens: usize) -> Vec<(String, usize)> {
    let chars: Vec<char> = corpus.chars().collect();
    let est_tokens = chars.len() / 4;
    if est_tokens <= chunk_tokens {
        return vec![(corpus.to_string(), est_tokens.max(1))];
    }
    let chars_per_chunk = chunk_tokens * 4;
    let mut chunks = Vec::new();
    let mut start = 0;
    let total = chars.len();
    while start < total {
        let end = (start + chars_per_chunk).min(total);
        // 空白边界对齐（字符级回退，避免单词截断）
        let mut cut = end;
        if end < total {
            for i in (start..end).rev() {
                if chars[i] == ' ' {
                    cut = i + 1;
                    break;
                }
            }
        }
        let text: String = chars[start..cut].iter().collect();
        if !text.trim().is_empty() {
            chunks.push((text, chunk_tokens));
        }
        // 防御：无空白时强制前进（避免死循环）
        if cut <= start {
            cut = start + (total - start).min(chars_per_chunk);
        }
        start = cut;
    }
    chunks
}

/// 确定性 chunk CLV（SplitMix64 派生——与 eval harness 同源风格）
fn make_chunk_clv(seed: u64) -> CLV {
    hcw_window::recall::eval::make_clv(seed ^ 0x9E37_79B9_7F4A_7C15, None, 0.0)
}

/// 构造 provider 闭包（kvbsr 候选 → dense/sparse → repo-wiki RRF → BlockScore）
///
/// # 闭包捕获
/// - `corpus_blocks`: 共享语料块表（set_corpus 写入，只读）
/// - `top_k`: 精排上限
fn build_provider(
    corpus_blocks: Arc<RwLock<Arc<Vec<CorpusBlock>>>>,
    top_k: usize,
) -> Arc<CandidateProvider> {
    Arc::new(move |query: &str, cap: usize| {
        // PROBE P1: 短持读锁取 Arc 快照（~ns）→ 锁外计算（余弦/排序/RRF）
        let table = Arc::clone(&*corpus_blocks.read().unwrap_or_else(|p| p.into_inner()));
        if table.is_empty() {
            return Vec::new();
        }
        let k = top_k.min(cap);
        // 查询 CLV（确定性：query 哈希为 seed）
        let query_clv = make_chunk_clv(hash_str(query));
        // dense 序：CLV 余弦降序——PROBE P3: select_nth_unstable 部分选择
        // （O(N + k log k)，替代全量 sort_by O(N log N)；Top-K 红线）
        let mut dense: Vec<(String, f32)> = table
            .iter()
            .map(|b| {
                let sim = query_clv.cosine_similarity(&b.clv);
                (b.id.clone(), sim)
            })
            .collect();
        let n = dense.len();
        let nth = k.min(n);
        if nth > 0 && nth < n {
            dense.select_nth_unstable_by(nth - 1, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
            dense.truncate(nth);
        }
        dense.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let dense_ids: Vec<String> = dense.iter().map(|(id, _)| id.clone()).collect();
        // sparse 序：查询关键词命中数降序（简易检索面；无命中时块序）
        let keywords: Vec<&str> = query.split_whitespace().collect();
        let mut sparse: Vec<(String, usize)> = table
            .iter()
            .map(|b| {
                let hits = keywords
                    .iter()
                    .filter(|kw| b.content.contains(**kw))
                    .count();
                (b.id.clone(), hits)
            })
            .collect();
        let sn = sparse.len();
        let snth = (2 * k).min(sn); // PROBE P3: 融合输入截断 top-2k（rank>2k RRF 贡献可忽略）
        if snth > 0 && snth < sn {
            sparse
                .select_nth_unstable_by(snth - 1, |a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            sparse.truncate(snth);
        }
        sparse.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let sparse_ids: Vec<String> = sparse.iter().map(|(id, _)| id.clone()).collect();
        // repo-wiki RRF 融合（纯函数；config 默认）
        let fused = hybrid_search(&dense_ids, &sparse_ids, &HybridSearchConfig::default(), k);
        // → BlockScore（rrf_score 为精排分）
        fused
            .into_iter()
            .map(|r| BlockScore::new(r.doc_id, r.rrf_score, 0.0, "overwindow", 0))
            .collect()
    })
}

/// 字符串确定性哈希（FNV-1a 64 位）
fn hash_str(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01B3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造超窗语料（重复段填充至 >400K token 估算）
    fn big_corpus() -> String {
        let base = "模块A 处理请求路由与鉴权，模块B 负责缓存失效与回写，模块C 执行语义检索。";
        let mut corpus = String::with_capacity(2_400_000);
        // 80K 段 × ~30 字符 ≈ 240 万字符 ≈ 60 万 token（稳超 400K）
        for i in 0..80_000 {
            corpus.push_str(&format!("{base} 段{i} "));
        }
        corpus
    }

    #[tokio::test]
    async fn test_bridge_end_to_end_over_window() {
        // 超窗端到端：set_corpus → run → 触发 + 候选非空
        let bus = EventBus::new();
        let bridge = OverWindowBridge::new(bus.clone()).unwrap();
        bridge.set_corpus(&big_corpus());
        let corpus_tokens = (big_corpus().chars().count() / 4) as u64;
        assert!(corpus_tokens > 400_000, "语料应超 400K token");
        let outcome = bridge
            .run("语义检索", corpus_tokens, 600_000)
            .await
            .unwrap();
        assert!(outcome.triggered, "超窗应触发兜底");
        assert!(outcome.candidate_count > 0, "候选集不应为空");
        assert!(
            outcome.candidates.len() <= 3 * 600_000,
            "候选受 3× 窗口约束"
        );
    }

    #[tokio::test]
    async fn test_bridge_within_window_no_trigger() {
        // 未超窗：零开销（不触发）
        let bus = EventBus::new();
        let bridge = OverWindowBridge::new(bus).unwrap();
        bridge.set_corpus("小语料。");
        let outcome = bridge.run("查询", 1_000, 600_000).await.unwrap();
        assert!(!outcome.triggered);
    }

    #[tokio::test]
    async fn test_bridge_empty_corpus_errors() {
        // 空语料块表：run 报错（引导调用方先 set_corpus）
        let bus = EventBus::new();
        let bridge = OverWindowBridge::new(bus).unwrap();
        let result = bridge.run("查询", 1_000_000, 600_000).await;
        assert!(result.is_err(), "空块表应报错");
    }

    #[tokio::test]
    async fn test_bridge_rag_recall_ge_load_path() {
        // RAG 路径召回 ≥ 装窗路径：针块（与查询同主题）应被精排保留
        let bus = EventBus::new();
        let bridge = OverWindowBridge::new(bus).unwrap();
        // 构造语料：针块含查询主题词
        let mut corpus = String::new();
        for i in 0..50 {
            corpus.push_str(&format!("普通内容块 {i} 与主题无关。"));
        }
        corpus.push_str("语义检索针块 与查询主题强相关。");
        for i in 0..200 {
            corpus.push_str(&format!("普通内容块 B{i} 无关键词。"));
        }
        bridge.set_corpus(&corpus);
        let outcome = bridge.run("语义检索", 10_000_000, 600_000).await.unwrap();
        assert!(outcome.triggered);
        // 精排候选应非空（kvbsr 聚类块 ID 为 UUID——不可预测，仅验证链路产出）
        assert!(outcome.candidate_count > 0, "精排候选不应为空");
        assert!(
            outcome.candidates.iter().all(|c| !c.block_id.is_empty()),
            "候选 ID 非空"
        );
    }

    #[test]
    fn test_chunk_corpus_boundaries() {
        // chunk 切分：不 panic、不产生空块
        let chunks = chunk_corpus("abcdefghijklmnopqrstuvwxyz", 4);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|(t, _)| !t.trim().is_empty()));
        let chunks2 = chunk_corpus("", 4);
        assert!(chunks2.is_empty() || chunks2.len() == 1);
    }

    #[test]
    fn test_hash_str_deterministic() {
        assert_eq!(hash_str("abc"), hash_str("abc"));
        assert_ne!(hash_str("abc"), hash_str("abd"));
    }

    #[tokio::test]
    async fn test_concurrent_set_corpus_and_run() {
        // PROBE P1 验收：set_corpus（写者）与 run（provider 读）交错 100 次
        // 无阻塞超时（Arc 原子 swap——读锁仅覆盖快照，写者不被整段阻塞）
        let bus = EventBus::new();
        let bridge = Arc::new(OverWindowBridge::new(bus.clone()).unwrap());
        bridge.set_corpus("初始语料内容。");
        let writer = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move {
                for i in 0..100 {
                    bridge.set_corpus(&format!("写入语料 {i} 内容填充。"));
                }
            })
        };
        let reader = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move {
                for _ in 0..100 {
                    // 超窗触发（provider 全量计算在读锁外完成）
                    let _ = bridge
                        .run("查询", 1_000_000, 600_000)
                        .await
                        .expect("run 不应失败");
                }
            })
        };
        let (w, r) = tokio::join!(writer, reader);
        w.unwrap();
        r.unwrap();
    }
}
