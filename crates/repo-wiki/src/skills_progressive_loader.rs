//! Skills 渐进加载器 — PenguinHarness "Index First, Body on Demand"（设计文档 §11.1）
//!
//! 对应架构层: **L5 Knowledge**（repo-wiki 子模块，用户确认落点 D-4）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §11.1
//! 对应论文: PenguinHarness（Index First, Body on Demand 渐进加载）
//! 对应 ADR: ADR-049 决策 1（skills-progressive-loader 落点 repo-wiki，内嵌模块）
//!
//! # 核心职责
//!
//! 技能的两阶段渐进加载，避免全量加载的内存/上下文膨胀：
//! - **Index First**: 仅注册轻量 [`SkillMetadata`] 索引（CLV 嵌入 + 描述 + 标签）
//! - **Body on Demand**: 任务 CLV 相似度门控后，仅对 Top-K 高相似度技能
//!   加载完整 [`SkillBody`]（代码/示例/测试/文档），其余返回"仅索引"占位
//! - **缓存**: body 加载结果缓存于 `skill_bodies`，重复任务不重复加载
//!
//! # 设计约束（铁律）
//!
//! - **铁律5**: 懒加载不阻塞——仅高相似度 Top-K 加载全文，未命中阈值仅返回索引
//! - **红线 R8 说明**: 索引排序规模为 max_index_count 受限集合（小规模式排序可接受，
//!   大规模索引应前置 CLV 近似检索，文档如实声明规模边界）
//! - **body_provider 注入**: body 内容由调用方注入（避免本模块与 WikiStore
//!   循环依赖），默认规则占位（铁律1 零运行时外部依赖）
//! - **L5→L1 向下合规**: CLV 来自 nexus-core

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use nexus_core::CLV;
use tokio::sync::Mutex;

use crate::skill_graph::SkillGraph;

/// 技能元数据索引 — Index First 阶段的轻量索引条目
#[derive(Clone, Debug)]
pub struct SkillMetadata {
    /// 技能唯一标识
    pub skill_id: String,
    /// 技能名称
    pub name: String,
    /// 技能描述（索引可见，无需加载 body）
    pub description: String,
    /// 技能语义嵌入（CLV 512 维，相似度门控用）
    pub embedding: CLV,
    /// 技能标签
    pub tags: Vec<String>,
    /// body 体积（字节，内存节省估算用）
    pub body_size: usize,
    /// 最近使用时间（None = 从未使用）
    pub last_used: Option<DateTime<Utc>>,
}

/// 技能全文 — Body on Demand 阶段按需加载的完整内容
#[derive(Clone, Debug)]
pub struct SkillBody {
    /// 所属技能 ID
    pub skill_id: String,
    /// 技能代码
    pub code: String,
    /// 使用示例
    pub examples: Vec<String>,
    /// 测试用例
    pub tests: Vec<String>,
    /// 完整文档
    pub documentation: String,
}

/// 加载结果 — 元数据 + body（body 可能为"仅索引"占位）
#[derive(Clone, Debug)]
pub struct LoadedSkill {
    /// 技能元数据索引
    pub metadata: SkillMetadata,
    /// 技能全文（未 full-load 时为占位 body）
    pub body: SkillBody,
}

/// 加载器统计 — 可观测性
#[derive(Clone, Debug)]
pub struct LoaderStats {
    /// 索引总数
    pub total_indexed: usize,
    /// 已加载 body 数
    pub bodies_loaded: usize,
    /// 内存节省比例 = 1 - bodies_loaded / total_indexed
    pub memory_saved_ratio: f32,
}

/// body 内容提供者类型 — 注入式 body 来源（避免本模块与 WikiStore 循环依赖）
pub type BodyProvider = Arc<dyn Fn(&str) -> SkillBody + Send + Sync>;

/// Skills 渐进加载器 — Index First, Body on Demand
///
/// `Clone` 派生（Arc 共享 body 缓存），所有副本共享缓存。
#[derive(Clone)]
pub struct ProgressiveSkillLoader {
    /// 技能索引（Index First 注册）
    skill_index: Vec<SkillMetadata>,
    /// body 缓存（Body on Demand 加载结果，Arc 共享）
    skill_bodies: Arc<Mutex<HashMap<String, SkillBody>>>,
    /// 相似度门控阈值（钳制 [0.5, 0.95]）
    similarity_threshold: f32,
    /// body 内容提供者（注入式；None = 默认规则占位）
    body_provider: Option<BodyProvider>,
}

impl ProgressiveSkillLoader {
    /// 创建加载器
    ///
    /// - `similarity_threshold`: 相似度门控阈值（钳制 [0.5, 0.95]，
    ///   过低 → 噪声技能涌入，过高 → 漏加载）
    pub fn new(similarity_threshold: f32) -> Self {
        Self {
            skill_index: Vec::new(),
            skill_bodies: Arc::new(Mutex::new(HashMap::new())),
            similarity_threshold: similarity_threshold.clamp(0.5, 0.95),
            body_provider: None,
        }
    }

    /// 注入 body 内容提供者（Body on Demand 的真实内容来源）
    ///
    /// WHY 注入式: 本模块不直接依赖 WikiStore（避免 repo-wiki 内部循环依赖），
    /// 调用方（L9/L10）接线真实内容来源。
    pub fn with_body_provider<F>(mut self, provider: F) -> Self
    where
        F: Fn(&str) -> SkillBody + Send + Sync + 'static,
    {
        self.body_provider = Some(Arc::new(provider));
        self
    }

    /// 注册技能索引（Index First）— 替换既有索引
    pub fn register_index(&mut self, metadata: Vec<SkillMetadata>) {
        self.skill_index = metadata;
    }

    /// 渐进加载技能（Body on Demand）
    ///
    /// 流程（规范 §11.1）:
    /// 1. CLV 相似度门控（≥ similarity_threshold）
    /// 2. 相似度降序取 Top-`max_index_count` 索引
    /// 3. 前 `max_full_load` 个加载完整 body（缓存命中直接返回）
    /// 4. 其余返回"仅索引"占位 body（铁律5 懒加载）
    ///
    /// # 红线 R8(W3)
    ///
    /// Top-N 用 `select_nth_unstable_by` O(n) 部分排序定位前 n 个最高分,
    /// 再对前 n 局部排序（O(n + k log k)）保持降序输出契约——
    /// 替代原全量 `sort_by` 的 O(n log n)。
    pub async fn load_skills(
        &self,
        task_embedding: &CLV,
        max_index_count: usize,
        max_full_load: usize,
    ) -> Vec<LoadedSkill> {
        // 1: 相似度门控
        let mut scored: Vec<(SkillMetadata, f32)> = self
            .skill_index
            .iter()
            .map(|meta| {
                let similarity = meta.embedding.cosine_similarity(task_embedding);
                (meta.clone(), similarity)
            })
            .filter(|(_, score)| *score >= self.similarity_threshold)
            .collect();
        // 2: Top-N(红线 R8: select_nth O(n) 部分排序 + 前 n 局部排序保持降序)
        if max_index_count < scored.len() {
            scored.select_nth_unstable_by(
                max_index_count,
                |a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal),
            );
            scored.truncate(max_index_count);
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // 3+4: 前 max_full_load 加载 body，其余占位
        let mut loaded = Vec::with_capacity(scored.len());
        for (idx, (meta, score)) in scored.into_iter().enumerate() {
            let body = if idx < max_full_load {
                self.load_body(&meta.skill_id).await
            } else {
                // 仅索引占位: 不加载全文，保留描述供调用方决策
                SkillBody {
                    skill_id: meta.skill_id.clone(),
                    code: format!("// Not loaded (sim: {score:.2})"),
                    examples: Vec::new(),
                    tests: Vec::new(),
                    documentation: meta.description.clone(),
                }
            };
            loaded.push(LoadedSkill {
                metadata: meta,
                body,
            });
        }
        loaded
    }

    /// 按需加载 body — 缓存命中直接返回，未命中经 body_provider 生成并缓存
    async fn load_body(&self, skill_id: &str) -> SkillBody {
        // 缓存命中快路径
        let cache = self.skill_bodies.lock().await;
        if let Some(body) = cache.get(skill_id) {
            return body.clone();
        }
        drop(cache);
        // 未命中: 经注入提供者生成（默认规则占位）
        let body = match &self.body_provider {
            Some(provider) => provider(skill_id),
            None => SkillBody {
                skill_id: skill_id.to_string(),
                code: format!("// Loaded {skill_id}"),
                examples: Vec::new(),
                tests: Vec::new(),
                documentation: "Loaded on demand".to_string(),
            },
        };
        let mut cache = self.skill_bodies.lock().await;
        cache.insert(skill_id.to_string(), body.clone());
        body
    }

    /// 加载器统计 — 索引数 / 已加载 body 数 / 内存节省比例
    pub async fn get_stats(&self) -> LoaderStats {
        let cache = self.skill_bodies.lock().await;
        let total = self.skill_index.len();
        LoaderStats {
            total_indexed: total,
            bodies_loaded: cache.len(),
            memory_saved_ratio: 1.0 - (cache.len() as f32 / total.max(1) as f32),
        }
    }

    /// 相似度阈值只读访问（可观测性）
    pub fn similarity_threshold(&self) -> f32 {
        self.similarity_threshold
    }

    /// 索引总数只读访问（可观测性）
    pub fn index_count(&self) -> usize {
        self.skill_index.len()
    }

    /// 索引快照只读访问（W3）— 供 L6 编排层消费（osa skill_plan 规划输入）
    ///
    /// WHY 只读切片: 避免克隆整个索引;调用方按需投影为 L6 轻量条目。
    pub fn index_snapshot(&self) -> &[SkillMetadata] {
        &self.skill_index
    }

    /// 按任务相关性预取 body（W3,铁律5 非阻塞）— 后台缓存填充
    ///
    /// 对相似度 Top-`n` 的技能预加载 body（等价 `load_skills(task, n, n)`）:
    /// - **幂等**: 缓存命中直接返回,重复预取无副作用
    /// - **非阻塞**: `tokio::spawn` 后台执行,失败仅影响后续缓存 miss,
    ///   不影响主流程（§4.4-7 幂等缓存填充 fire-and-forget 可接受）
    /// - 与 L6 规划协同: L6 `plan_skill_load` 产出 full-load 集后,调用方
    ///   可先 prefetch 预热缓存,再走 load_skills 主路径（命中快路径）
    pub fn prefetch(&self, task_embedding: CLV, n: usize) -> tokio::task::JoinHandle<()> {
        let loader = self.clone(); // Arc 共享 body 缓存,副本即同缓存
        tokio::spawn(async move {
            let _ = loader.load_skills(&task_embedding, n, n).await;
        })
    }
}

/// SkillGraph 节点 → 加载器索引条目映射（W3,L5 内部协同）
///
/// 复用率图（SkillGraph）供给渐进加载索引（ProgressiveSkillLoader）:
/// success_rate / reuse_count 沉淀进 description（索引可见,无需加载 body）,
/// body_size 未知置 0（真实体积由 body_provider 侧计量）。
pub fn skill_metadata_from_graph(graph: &SkillGraph) -> Vec<SkillMetadata> {
    graph
        .iter()
        .map(|node| SkillMetadata {
            skill_id: node.skill_id.clone(),
            name: node.skill_id.clone(),
            description: format!(
                "success_rate={:.3}, reuse_count={}",
                node.success_rate, node.reuse_count
            ),
            embedding: node.embedding.clone(),
            tags: node.dependencies.clone(),
            body_size: 0,
            last_used: None,
        })
        .collect()
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造指定维度方向为 1.0 的单位 CLV（其余 0）
    fn unit_clv(dim: usize) -> CLV {
        let mut v = vec![0.0f32; CLV::DIMENSION];
        v[dim] = 1.0;
        CLV::from_vec(v).expect("512 维合法")
    }

    fn meta(id: &str, dim: usize) -> SkillMetadata {
        SkillMetadata {
            skill_id: id.to_string(),
            name: format!("name-{id}"),
            description: format!("desc-{id}"),
            embedding: unit_clv(dim),
            tags: vec![id.to_string()],
            body_size: 100,
            last_used: None,
        }
    }

    #[tokio::test]
    async fn threshold_clamped_to_range() {
        assert_eq!(ProgressiveSkillLoader::new(0.1).similarity_threshold(), 0.5);
        assert_eq!(
            ProgressiveSkillLoader::new(0.99).similarity_threshold(),
            0.95
        );
        assert_eq!(ProgressiveSkillLoader::new(0.7).similarity_threshold(), 0.7);
    }

    #[tokio::test]
    async fn similarity_gating_filters_low_similarity() {
        let mut loader = ProgressiveSkillLoader::new(0.9);
        // dim 0 与任务完全相关（cos=1.0），dim 5 正交（cos=0.0）
        loader.register_index(vec![meta("rel", 0), meta("orth", 5)]);
        let loaded = loader.load_skills(&unit_clv(0), 10, 10).await;
        assert_eq!(loaded.len(), 1, "正交技能应被相似度门控过滤");
        assert_eq!(loaded[0].metadata.skill_id, "rel");
    }

    #[tokio::test]
    async fn top_k_index_with_full_load_boundary() {
        let mut loader = ProgressiveSkillLoader::new(0.5);
        // 三个近似技能（同维度扰动近似——用相同方向 cos=1.0）
        loader.register_index(vec![meta("a", 0), meta("b", 0), meta("c", 0)]);
        // Top-2 索引，仅 full-load 1 个
        let loaded = loader.load_skills(&unit_clv(0), 2, 1).await;
        assert_eq!(loaded.len(), 2);
        // 第一个 full-load（body 非占位）
        assert!(!loaded[0].body.code.starts_with("// Not loaded"));
        // 第二个仅索引占位
        assert!(loaded[1].body.code.starts_with("// Not loaded"));
    }

    #[tokio::test]
    async fn body_cache_hit_no_reload() {
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&call_count);
        let loader = ProgressiveSkillLoader::new(0.5).with_body_provider(move |id| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            SkillBody {
                skill_id: id.to_string(),
                code: format!("real-{id}"),
                examples: Vec::new(),
                tests: Vec::new(),
                documentation: "real".to_string(),
            }
        });
        let mut loader = loader;
        loader.register_index(vec![meta("a", 0)]);
        // 两次加载同一技能 → provider 仅调用一次（缓存命中）
        loader.load_skills(&unit_clv(0), 1, 1).await;
        loader.load_skills(&unit_clv(0), 1, 1).await;
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        let stats = loader.get_stats().await;
        assert_eq!(stats.bodies_loaded, 1);
    }

    #[tokio::test]
    async fn memory_saved_ratio_math() {
        let mut loader = ProgressiveSkillLoader::new(0.5);
        loader.register_index(vec![meta("a", 0), meta("b", 0), meta("c", 0), meta("d", 0)]);
        // 仅 full-load 1 个 → saved = 1 - 1/4 = 0.75
        loader.load_skills(&unit_clv(0), 4, 1).await;
        let stats = loader.get_stats().await;
        assert_eq!(stats.total_indexed, 4);
        assert_eq!(stats.bodies_loaded, 1);
        assert!((stats.memory_saved_ratio - 0.75).abs() < 1e-6);
    }

    #[tokio::test]
    async fn empty_index_returns_empty() {
        let loader = ProgressiveSkillLoader::new(0.5);
        let loaded = loader.load_skills(&unit_clv(0), 10, 10).await;
        assert!(loaded.is_empty());
        let stats = loader.get_stats().await;
        assert_eq!(stats.total_indexed, 0);
        // 空索引 ratio 不除零（max(1) 保护）
        assert!(stats.memory_saved_ratio.is_finite());
    }

    #[tokio::test]
    async fn body_provider_injection_used() {
        let loader = ProgressiveSkillLoader::new(0.5).with_body_provider(|id| SkillBody {
            skill_id: id.to_string(),
            code: format!("injected-{id}"),
            examples: vec!["ex".into()],
            tests: Vec::new(),
            documentation: "injected".into(),
        });
        let mut loader = loader;
        loader.register_index(vec![meta("a", 0)]);
        let loaded = loader.load_skills(&unit_clv(0), 1, 1).await;
        assert_eq!(loaded[0].body.code, "injected-a");
    }
}
