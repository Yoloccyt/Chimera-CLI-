//! DualExperienceBank — 双层经验库(案例级 + 全局蒸馏,MemoHarness,文档 §10.2 问题 4)
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:Ω₂-Compress(全局蒸馏压缩案例经验)
//!
//! # 双层语义(文档 §22 结语)
//!
//! - **案例级经验**:具体任务实例的完整记录,即现有 `WikiEntry`(零新存储,
//!   通过 `case_level_search` 复用 WikiStore 检索)
//! - **全局蒸馏经验**:从多条案例中提炼的跨任务洞察(`DistilledInsight`),
//!   持久化于 `distilled_insights` 表,通过 `global_search` 检索
//!
//! # 蒸馏算法(规则式,WHY 无 LLM)
//!
//! R2 冻结(ADR-042)下禁止引入学习/训练路径,蒸馏必须是确定性规则算法:
//! 1. 标签频次统计:支持度 ≥ `min_support` 的标签产生"社区标签"洞察
//! 2. 标签共现对统计:共现次数 ≥ `min_support` 的标签对产生"共现"洞察
//! 3. 置信度 = 支持度 / 案例总数(可复现、可测试、可审计,Ω₈-Assess)
//!
//! 蒸馏是可重复操作(UPSERT 幂等):相同输入必产生相同洞察集。
//!
//! # 依赖方向(§2.2)
//!
//! 本模块是 repo-wiki(L5)内部模块,仅依赖 crate 内 WikiStore 与
//! L0/L1 已有依赖;不触碰 L2 mlc-engine(记忆层职责边界)。

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::WikiError;
use crate::store::WikiStore;
use crate::types::WikiEntry;

/// 全局蒸馏洞察 — 从多条案例条目提炼的跨任务经验
///
/// 与 `WikiEntry`(案例级)互补:洞察是压缩后的共性规律,案例是原始实例。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistilledInsight {
    /// 洞察唯一标识(确定性:如 `single:rust` / `pair:async+rust`,保证 UPSERT 幂等)
    pub insight_id: String,
    /// 洞察内容(自然语言,规则模板生成)
    pub content: String,
    /// 关联标签(单标签洞察 1 个,共现洞察 2 个)
    pub tags: Vec<String>,
    /// 嵌入向量(源案例条目 embedding 的均值)
    pub embedding: Vec<f32>,
    /// 置信度 = 支持度 / 案例总数,∈ [0, 1]
    pub confidence: f32,
    /// 蒸馏来源案例条目数(支持度)
    pub source_count: u32,
    /// 创建时间(UTC)
    pub created_at: DateTime<Utc>,
}

/// 双层经验库 — 案例级检索 + 全局蒸馏的统一入口
pub struct DualExperienceBank {
    /// Wiki 存储(案例级条目的持久化与检索复用)
    wiki: Arc<WikiStore>,
}

impl DualExperienceBank {
    /// 创建双层经验库
    pub fn new(wiki: Arc<WikiStore>) -> Self {
        Self { wiki }
    }

    /// 案例级检索 — 复用 WikiStore 混合检索(sparse-only 退化模式)
    ///
    /// 案例级经验即 WikiEntry;本方法不重复实现检索逻辑,
    /// 直接复用 L5 唯一融合入口(Ω₆-Reuse)。
    pub async fn case_level_search(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<WikiEntry>, WikiError> {
        // 案例级检索走 hybrid_query sparse-only(无编码器注入时行为与 FTS5 一致),
        // 再按融合排序回取条目。
        let results = self.wiki.hybrid_query(query, None, top_k).await?;
        let mut entries = Vec::with_capacity(results.len());
        for r in results {
            if let Some(entry) = self.wiki.get(r.doc_id).await? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// 规则式蒸馏 — 高频标签 + 共现对 → 洞察,写入 distilled_insights 表
    ///
    /// # 参数
    /// - `min_support`:最小支持度阈值(标签/共现对计数 ≥ 此值才产生洞察)
    ///
    /// # 返回
    /// 本次蒸馏产生的洞察列表(已持久化;UPSERT 幂等,可重复调用)。
    ///
    /// # 算法(确定性规则,无 LLM)
    /// 1. 读取所有 Current 案例条目
    /// 2. 统计标签频次与标签共现对频次(字母序配对,避免重复)
    /// 3. 满足支持度的标签/共现对生成洞察,embedding 取源条目均值
    pub async fn distill_from_entries(
        &self,
        min_support: usize,
    ) -> Result<Vec<DistilledInsight>, WikiError> {
        // 1. 读取案例条目(仅 Current 状态,归档条目不参与蒸馏)
        let all = self.wiki.list_all().await?;
        let entries: Vec<&WikiEntry> = all.iter().filter(|e| e.is_current()).collect();
        let total = entries.len();
        if total == 0 {
            return Ok(Vec::new());
        }

        // 2. 标签频次统计
        let mut tag_freq: HashMap<String, usize> = HashMap::new();
        for entry in &entries {
            for tag in &entry.tags {
                *tag_freq.entry(tag.clone()).or_insert(0) += 1;
            }
        }

        // 3. 标签共现对统计(字母序配对,保证 (a,b) 与 (b,a) 归并为同一对)
        let mut pair_freq: HashMap<(String, String), usize> = HashMap::new();
        for entry in &entries {
            let mut sorted: Vec<&String> = entry.tags.iter().collect();
            sorted.sort();
            for i in 0..sorted.len() {
                for j in (i + 1)..sorted.len() {
                    let pair = (sorted[i].clone(), sorted[j].clone());
                    *pair_freq.entry(pair).or_insert(0) += 1;
                }
            }
        }

        // 4. 生成洞察(确定性顺序:标签按字母序,共现对按 (a, b) 字母序)
        let mut insights: Vec<DistilledInsight> = Vec::new();

        // 4a. 单标签洞察
        let mut sorted_tags: Vec<(String, usize)> = tag_freq
            .into_iter()
            .filter(|(_, c)| *c >= min_support)
            .collect();
        sorted_tags.sort_by(|a, b| a.0.cmp(&b.0));
        for (tag, count) in &sorted_tags {
            let embedding = Self::mean_embedding_for_tag(&entries, tag);
            insights.push(DistilledInsight {
                insight_id: format!("single:{tag}"),
                content: format!("社区标签「{tag}」:{count} 个案例条目共享此主题"),
                tags: vec![tag.clone()],
                embedding,
                confidence: *count as f32 / total as f32,
                source_count: *count as u32,
                created_at: Utc::now(),
            });
        }

        // 4b. 共现对洞察
        let mut sorted_pairs: Vec<((String, String), usize)> = pair_freq
            .into_iter()
            .filter(|(_, c)| *c >= min_support)
            .collect();
        sorted_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        for ((a, b), count) in &sorted_pairs {
            let embedding = Self::mean_embedding_for_tags(&entries, a, b);
            insights.push(DistilledInsight {
                insight_id: format!("pair:{a}+{b}"),
                content: format!("标签「{a}」与「{b}」共现于 {count} 个案例条目"),
                tags: vec![a.clone(), b.clone()],
                embedding,
                confidence: *count as f32 / total as f32,
                source_count: *count as u32,
                created_at: Utc::now(),
            });
        }

        // 5. 持久化(UPSERT 幂等)
        for insight in &insights {
            self.wiki.insert_distilled_insight(insight.clone()).await?;
        }
        Ok(insights)
    }

    /// 全局蒸馏检索 — 按内容/标签匹配返回洞察
    ///
    /// 规则式匹配:洞察 content 或 tags 包含 query 子串即命中,
    /// 按 source_count 降序(高支持度洞察优先)。
    pub async fn global_search(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<DistilledInsight>, WikiError> {
        let mut all = self.wiki.list_distilled_insights().await?;
        all.retain(|i| i.content.contains(query) || i.tags.iter().any(|t| t.contains(query)));
        // WHY 按支持度降序:高支持度洞察是更普遍的规律,检索价值更高
        // C9(2026-09-04,红线 #8):全量 sort O(n log n)+截断 → L0 xts_top_k_by
        // (select_nth O(n) + 前 k 段二次排序)。返回集合与段内降序与原语义一致;
        // 仅同 source_count 的 tie 相对顺序由稳定全排变为 select 决定(无界洞察
        // 库场景下性能收益随 n 增长,tests/experience_bank_test.rs 既有断言
        // 不依赖 tie 逐位序,已核)。
        nexus_contracts::util::xts_top_k_by(&mut all, top_k, |a, b| {
            b.source_count.cmp(&a.source_count)
        });
        all.truncate(top_k);
        Ok(all)
    }

    /// 导出全部蒸馏洞察 — L3 存储层持久化协同接口（Phase 5 Wave 4，D-6）
    ///
    /// 返回已持久化于 `distilled_insights` 表的全部洞察，按 source_count 降序。
    /// 调用方（L3 `ExperienceCardStorage` 持久化）由上层驱动，本模块不引入
    /// cmt-tiering 依赖（职责边界：记忆层 L2 vs 知识层 L5 vs 存储层 L3）。
    ///
    /// # 返回
    /// 全部蒸馏洞察（按支持度降序），空库返回空 Vec。
    pub async fn export_distilled_insights(&self) -> Result<Vec<DistilledInsight>, WikiError> {
        let mut all = self.wiki.list_distilled_insights().await?;
        // 按支持度降序（高支持度洞察优先持久化）
        all.sort_by_key(|i| std::cmp::Reverse(i.source_count));
        Ok(all)
    }

    /// 计算携带指定标签的源条目 embedding 均值
    ///
    /// WHY 均值聚合:洞察是多个案例的共性压缩,均值向量保留粗粒度语义方向;
    /// 维度以第一个源条目为准,维度不一致的条目跳过(容错)。
    fn mean_embedding_for_tag(entries: &[&WikiEntry], tag: &str) -> Vec<f32> {
        let sources: Vec<&WikiEntry> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == tag))
            .copied()
            .collect();
        Self::mean_embeddings(&sources)
    }

    /// 计算同时携带两个标签的源条目 embedding 均值
    fn mean_embedding_for_tags(entries: &[&WikiEntry], a: &str, b: &str) -> Vec<f32> {
        let sources: Vec<&WikiEntry> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == a) && e.tags.iter().any(|t| t == b))
            .copied()
            .collect();
        Self::mean_embeddings(&sources)
    }

    /// 条目 embedding 均值(维度以第一条为准,不一致条目跳过)
    fn mean_embeddings(sources: &[&WikiEntry]) -> Vec<f32> {
        let Some(first) = sources.first() else {
            return Vec::new();
        };
        let dim = first.embedding.len();
        let mut sum = vec![0.0f64; dim];
        let mut counted = 0usize;
        for entry in sources {
            if entry.embedding.len() == dim {
                for (i, v) in entry.embedding.iter().enumerate() {
                    sum[i] += f64::from(*v);
                }
                counted += 1;
            }
        }
        if counted == 0 {
            return Vec::new();
        }
        sum.into_iter()
            .map(|s| (s / counted as f64) as f32)
            .collect()
    }
}
