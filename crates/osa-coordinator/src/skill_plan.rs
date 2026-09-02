//! Skills 渐进加载 L6 编排 — D2 契约驱动的纯函数规划器（规范 §11.1，W3）
//!
//! 对应架构层: **L6 Router**（osa-coordinator 内嵌，ADR-084 决策 5）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §11.1
//! 协同落点: 加载机制（Index First / Body on Demand / 双检锁缓存）在 L5
//! repo-wiki `ProgressiveSkillLoader`；本模块承载 **L6 编排决策**——
//! 消费 L0 `ToolInteractionContract`(D2) 契约与 SkillGraph 推荐提权，
//! 产出加载规划（哪些技能全量加载、哪些仅索引、哪些跳过）。
//!
//! # 职责边界（ADR-084 决策 5）
//!
//! - **本模块（L6）**: 纯函数规划——相似度门控 + boost 提权 + R8 Top-K 预算切分;
//!   不依赖 repo-wiki（避免 rusqlite 依赖链污染 osa 全部消费方）
//! - **repo-wiki（L5）**: `ProgressiveSkillLoader` 执行加载（缓存/prefetch）+
//!   `skill_metadata_from_graph` 供给索引
//! - **装配层（L9/L10）**: SkillGraph → 索引 → L6 规划 → prefetch → load_skills
//!
//! # 打分公式（决策记录）
//!
//! `score = sim(task, embedding) × 0.7 + 0.3 × is_boosted`
//!
//! 相似度为主信号（0.7），SkillGraph 推荐项提权（0.3）——推荐项即使相似度
//! 平庸也保底 0.3 进候选（SkillGraph 五因子推荐含成功率/复用/依赖结构，
//! 是独立于当前任务向量的知识信号）。权重为可调常量，调参须经 bench 证据。
//!
//! # 设计约束（铁律）
//!
//! - **铁律4**: `plan_skill_load` 纯函数（同输入同输出，proptest 锁定不变量）
//! - **铁律5**: 规划零 IO;执行侧 prefetch 后台幂等（repo-wiki 侧已实现）
//! - **红线 R8**: Top-`max_full_skill_load` 用 `select_nth_unstable_by` O(n)
//!   部分排序 + 前 k 局部排序（保持降序输出契约）

use std::collections::HashSet;

use nexus_contracts::harness_dimensions::ToolInteractionContract;
use nexus_core::CLV;

/// 相似度主信号权重（打分公式可调常量，调参须经 bench 证据）
pub const SIMILARITY_WEIGHT: f32 = 0.7;
/// SkillGraph 推荐提权权重
pub const BOOST_WEIGHT: f32 = 0.3;
/// 默认门控阈值（与 L5 loader 钳制下界 0.5 对齐）
pub const DEFAULT_PLAN_THRESHOLD: f32 = 0.5;

/// 技能索引条目 — L6 轻量投影（免 repo-wiki 生产依赖）
///
/// 装配层/测试从 repo-wiki `SkillMetadata` 投影（embedding 为共享 CLV 类型）。
#[derive(Clone, Debug)]
pub struct SkillIndexEntry {
    /// 技能唯一标识
    pub skill_id: String,
    /// 语义嵌入（相似度门控用）
    pub embedding: CLV,
    /// body 体积（字节，进度估算用;未知置 0）
    pub body_size: u32,
    /// 最近使用纪元（None = 从未使用）
    pub last_used_epoch: Option<u64>,
}

impl SkillIndexEntry {
    /// 构造索引条目
    pub fn new(skill_id: impl Into<String>, embedding: CLV, body_size: u32) -> Self {
        Self {
            skill_id: skill_id.into(),
            embedding,
            body_size,
            last_used_epoch: None,
        }
    }
}

/// Skills 加载规划 — L6 编排决策产物
#[derive(Clone, Debug)]
pub struct SkillLoadPlan {
    /// 全量加载的技能 ID（降序得分排列,受 D2.max_full_skill_load 预算约束）
    pub full_load_ids: Vec<String>,
    /// 仅索引的技能 ID（通过门控但未获 body 预算——Body on Demand 占位）
    pub index_only_ids: Vec<String>,
    /// 门控以下跳过的条数（不进上下文）
    pub skipped_count: usize,
    /// 规划时的门控阈值
    pub threshold: f32,
    /// 渐进加载是否启用（false = 全量加载语义）
    pub progressive_enabled: bool,
}

/// 加载进度追踪 — 规划 + 索引规模合成（W3）
#[derive(Clone, Debug)]
pub struct SkillLoadProgress {
    /// 索引总条数
    pub indexed_total: usize,
    /// 通过门控的候选数（full + index_only）
    pub candidates: usize,
    /// 全量加载数（body 进入上下文）
    pub full_loaded: usize,
    /// 仅索引数（占位 body）
    pub index_only: usize,
    /// 全量加载的预估 body 字节（body_size 求和;未知条目贡献 0）
    pub est_full_body_bytes: u64,
}

/// 规划 Skills 渐进加载（铁律4 纯函数 + 红线 R8）
///
/// - `d2.progressive_skill_loading = false` → 全量加载语义（传统路径，
///   full_load_ids = 全部索引,诚实反映"渐进关闭"的控制面决策）
/// - 否则: 打分（相似度×0.7 + boost×0.3）→ 门控 ≥ threshold →
///   Top-`max_full_skill_load` 全量,其余候选仅索引,门控下跳过
pub fn plan_skill_load(
    d2: &ToolInteractionContract,
    task: &CLV,
    index: &[SkillIndexEntry],
    boost: &[String],
    threshold: f32,
) -> SkillLoadPlan {
    if !d2.progressive_skill_loading {
        return SkillLoadPlan {
            full_load_ids: index.iter().map(|e| e.skill_id.clone()).collect(),
            index_only_ids: Vec::new(),
            skipped_count: 0,
            threshold,
            progressive_enabled: false,
        };
    }
    let boost_set: HashSet<&str> = boost.iter().map(String::as_str).collect();
    // 门控 + 打分（boost 提权见模块文档打分公式）
    let mut scored: Vec<(usize, f32)> = index
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let sim = entry.embedding.cosine_similarity(task);
            let score = if boost_set.contains(entry.skill_id.as_str()) {
                sim * SIMILARITY_WEIGHT + BOOST_WEIGHT
            } else {
                sim
            };
            (i, score)
        })
        .filter(|(_, score)| *score >= threshold)
        .collect();

    // 候选索引集合必须在预算截断前记录（index_only = 候选 − 预算幸存者）
    let candidate_idx: Vec<usize> = scored.iter().map(|(i, _)| *i).collect();
    let skipped_count = index.len() - candidate_idx.len();

    // 红线 R8: Top-预算 O(n) 部分排序 + 前 k 局部排序保持降序
    let budget = d2.max_full_skill_load;
    if budget < scored.len() {
        scored.select_nth_unstable_by(budget, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(budget);
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let full_load_ids: Vec<String> = scored
        .iter()
        .map(|(i, _)| index[*i].skill_id.clone())
        .collect();
    let full_set: HashSet<&str> = full_load_ids.iter().map(String::as_str).collect();
    let index_only_ids: Vec<String> = candidate_idx
        .iter()
        .map(|i| index[*i].skill_id.as_str())
        .filter(|id| !full_set.contains(id))
        .map(String::from)
        .collect();
    SkillLoadPlan {
        full_load_ids,
        index_only_ids,
        skipped_count,
        threshold,
        progressive_enabled: true,
    }
}

/// 由规划合成加载进度（W3 进度追踪）
pub fn progress_from_plan(index: &[SkillIndexEntry], plan: &SkillLoadPlan) -> SkillLoadProgress {
    let est_full_body_bytes = index
        .iter()
        .filter(|e| plan.full_load_ids.contains(&e.skill_id))
        .map(|e| e.body_size as u64)
        .sum();
    SkillLoadProgress {
        indexed_total: index.len(),
        candidates: plan.full_load_ids.len() + plan.index_only_ids.len(),
        full_loaded: plan.full_load_ids.len(),
        index_only: plan.index_only_ids.len(),
        est_full_body_bytes,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 指定维度为 1.0 的单位 CLV（正交技能向量）
    fn unit_clv(dim: usize) -> CLV {
        // 算法体已收敛到 L1 `nexus_core::CLV::basis`(单一权威构造器),
        // 此处仅保留本地签名,避免改动本文件内数十处调用点。
        // basis 越界返回 None;夹具若下标非法则该测试无效,直接 expect 暴露。
        CLV::basis(dim).expect("测试夹具:下标须在 CLV::DIMENSION 内")
    }

    fn entry(id: &str, dim: usize, size: u32) -> SkillIndexEntry {
        SkillIndexEntry::new(id, unit_clv(dim), size)
    }

    fn d2(progressive: bool, budget: usize) -> ToolInteractionContract {
        ToolInteractionContract {
            progressive_skill_loading: progressive,
            max_full_skill_load: budget,
            ..ToolInteractionContract::default_contract()
        }
    }

    #[test]
    fn orthogonal_orthodox_split() {
        // 任务=dim0: 近技能(dim0, sim=1.0)全量; 远技能(dim1, sim=0.0)跳过
        let index = vec![
            entry("near", 0, 100),
            entry("far", 1, 100),
            entry("mid", 2, 100),
        ];
        let plan = plan_skill_load(
            &d2(true, 4),
            &unit_clv(0),
            &index,
            &[],
            DEFAULT_PLAN_THRESHOLD,
        );
        assert_eq!(plan.full_load_ids, vec!["near".to_string()]);
        assert!(plan.index_only_ids.is_empty(), "候选仅 1 个且预算充足");
        assert_eq!(plan.skipped_count, 2, "sim=0 低于门控跳过");
        assert!(plan.progressive_enabled);
    }

    #[test]
    fn budget_cap_enforced() {
        // 5 个同分候选,预算 2 → 仅 2 全量,其余 3 仅索引
        let index: Vec<_> = (0..5).map(|i| entry(&format!("s{i}"), 0, 10)).collect();
        let plan = plan_skill_load(
            &d2(true, 2),
            &unit_clv(0),
            &index,
            &[],
            DEFAULT_PLAN_THRESHOLD,
        );
        assert_eq!(plan.full_load_ids.len(), 2);
        assert_eq!(plan.index_only_ids.len(), 3);
        assert_eq!(plan.skipped_count, 0);
    }

    #[test]
    fn boost_lifts_low_similarity_into_candidates() {
        // sim≈0.4: 无 boost 低于门控 0.5 跳过; boost 提权 0.4×0.7+0.3=0.58 过门控
        let mut v = vec![0.0f32; CLV::DIMENSION];
        v[0] = 0.4;
        v[1] = 0.9165; // 归一化后与 dim0 的余弦 ≈ 0.4
        let mixed = CLV::from_vec(v).expect("合法");
        let index = vec![
            SkillIndexEntry::new("plain", mixed.clone(), 10),
            SkillIndexEntry::new("boosted", mixed, 10),
        ];
        let task = unit_clv(0);
        // 无 boost: 两者 sim≈0.4 < 0.5 → 全跳过
        let no_boost = plan_skill_load(&d2(true, 4), &task, &index, &[], DEFAULT_PLAN_THRESHOLD);
        assert_eq!(no_boost.skipped_count, 2);
        // 有 boost: boosted=0.58 过门控, plain 仍跳过
        let with_boost = plan_skill_load(
            &d2(true, 4),
            &task,
            &index,
            &["boosted".to_string()],
            DEFAULT_PLAN_THRESHOLD,
        );
        assert_eq!(with_boost.full_load_ids, vec!["boosted".to_string()]);
        assert_eq!(with_boost.skipped_count, 1, "plain 仍低于门控");
    }

    #[test]
    fn progressive_disabled_means_full_load() {
        // 渐进关闭 → 全量加载语义（控制面决策的诚实反映）
        let index = vec![entry("a", 1, 10), entry("b", 2, 10)];
        let plan = plan_skill_load(
            &d2(false, 1),
            &unit_clv(0),
            &index,
            &[],
            DEFAULT_PLAN_THRESHOLD,
        );
        assert!(!plan.progressive_enabled);
        assert_eq!(plan.full_load_ids.len(), 2, "关闭渐进 = 全量");
        assert_eq!(plan.skipped_count, 0);
    }

    #[test]
    fn progress_tracking_math() {
        let index = vec![
            entry("a", 0, 100),
            entry("b", 0, 200),
            entry("c", 0, 300),
            entry("far", 1, 50),
        ];
        let plan = plan_skill_load(
            &d2(true, 2),
            &unit_clv(0),
            &index,
            &[],
            DEFAULT_PLAN_THRESHOLD,
        );
        let progress = progress_from_plan(&index, &plan);
        assert_eq!(progress.indexed_total, 4);
        assert_eq!(progress.candidates, 3);
        assert_eq!(progress.full_loaded, 2);
        assert_eq!(progress.index_only, 1);
        // 并列分数下 Top-2 具体成员不定,字节期望从 plan 推导（验证映射逻辑本身）
        let expected_bytes: u64 = index
            .iter()
            .filter(|e| plan.full_load_ids.contains(&e.skill_id))
            .map(|e| e.body_size as u64)
            .sum();
        assert_eq!(progress.est_full_body_bytes, expected_bytes);
        // 全量字节必为所选两者之和,介于 100+200 与 200+300 之间
        assert!(progress.est_full_body_bytes >= 300 && progress.est_full_body_bytes <= 500);
    }

    #[test]
    fn full_load_ids_sorted_descending() {
        // R8 部分排序后前 k 局部排序: 输出保持降序契约
        // 三个不同相似度: cand-a(1.0) > cand-c(0.8) > cand-b(0.5)
        let mut v1 = vec![0.0f32; CLV::DIMENSION];
        v1[0] = 1.0;
        let mut v2 = vec![0.0f32; CLV::DIMENSION];
        v2[0] = 0.5;
        v2[1] = 0.866;
        let mut v3 = vec![0.0f32; CLV::DIMENSION];
        v3[0] = 0.8;
        v3[1] = 0.6;
        let index = vec![
            SkillIndexEntry::new("cand-a", CLV::from_vec(v1).unwrap(), 10),
            SkillIndexEntry::new("cand-b", CLV::from_vec(v2).unwrap(), 10),
            SkillIndexEntry::new("cand-c", CLV::from_vec(v3).unwrap(), 10),
        ];
        let task = unit_clv(0);
        let plan = plan_skill_load(&d2(true, 3), &task, &index, &[], DEFAULT_PLAN_THRESHOLD);
        assert_eq!(plan.full_load_ids.len(), 3);
        assert_eq!(
            plan.full_load_ids,
            vec![
                "cand-a".to_string(),
                "cand-c".to_string(),
                "cand-b".to_string()
            ]
        );
    }
}
