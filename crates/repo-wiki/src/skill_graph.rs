//! SkillGraph — 技能依赖图与复用率优先推荐(polish-v2.7 P4-6)
//!
//! 对应架构层:L5 Knowledge(repo-wiki 子模块)
//! 对应 ADR:ADR-049 决策 1(skill-graph 落点 repo-wiki)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §9.2(SkillGraph 联合进化)
//!
//! # 核心思想(Ω₆ Reuse 复用率优先)
//!
//! 技能是"被验证过的可复用能力单元"。技能图维护技能间依赖关系与
//! 使用统计,推荐时五因子加权(语义相似 + 上下文匹配 + 成功率 +
//! 复用频率 + 依赖满足度),Top-K 用 `select_nth_unstable`(§4.1 红线)。
//!
//! # R2 冻结声明(ADR-042)
//! 图拓扑演进为纯规则(频次阈值发现新技能 + 低频淘汰),
//! 无策略网络更新;方案 §9.2 的 evolution_policy.update 推迟至 R2 解冻。

use std::collections::HashMap;

use nexus_core::CLV;
use serde::{Deserialize, Serialize};

/// 新技能发现的最低出现频次(方案 §9.2 NEW_SKILL_THRESHOLD)
const NEW_SKILL_THRESHOLD: u32 = 3;

/// 推荐返回的最大条数
const RECOMMEND_TOP_K: usize = 10;

/// 技能节点
#[derive(Debug, Clone)]
pub struct SkillNode {
    /// 技能唯一标识
    pub skill_id: String,
    /// 技能语义嵌入
    pub embedding: CLV,
    /// 历史成功率 [0.0, 1.0]
    pub success_rate: f32,
    /// 复用次数
    pub reuse_count: u32,
    /// 依赖的前置技能
    pub dependencies: Vec<String>,
}

/// 技能推荐条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecommendation {
    /// 技能标识
    pub skill_id: String,
    /// 五因子综合相关度
    pub relevance: f32,
    /// 依赖的前置技能(调用方按序装配)
    pub dependencies: Vec<String>,
}

/// 技能使用模式 — 从轨迹提取的技能序列观测
#[derive(Debug, Clone)]
pub struct SkillUsagePattern {
    /// 候选技能标识
    pub skill_id: String,
    /// 技能嵌入(新技能入图时使用)
    pub embedding: CLV,
    /// 本批次出现频次
    pub frequency: u32,
    /// 本批次成功率
    pub success_rate: f32,
    /// 观测到的技能调用序列(相邻两技能建依赖边)
    pub sequence: Vec<String>,
}

/// 技能图
#[derive(Debug, Default)]
pub struct SkillGraph {
    nodes: HashMap<String, SkillNode>,
}

/// 安全约束违规（Milestone B-3a，九层防御 L5 补齐）
///
/// 技能图执行前闸门：调用方应先 `check_security()` 再 `recommend()`，
/// 防止悬空依赖（依赖技能不存在）与循环依赖（执行死锁）。
#[derive(Debug, Clone, PartialEq)]
pub struct SecurityViolation {
    /// 违规技能 ID
    pub skill_id: String,
    /// 违规原因（含依赖技能名，便于定位）
    pub reason: String,
}

impl SkillGraph {
    /// 创建空技能图
    pub fn new() -> Self {
        Self::default()
    }

    /// 技能数
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 图是否为空
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 查询技能
    pub fn get(&self, skill_id: &str) -> Option<&SkillNode> {
        self.nodes.get(skill_id)
    }

    /// 图节点只读迭代（W3）— 供渐进加载索引映射（skill_metadata_from_graph）
    pub fn iter(&self) -> impl Iterator<Item = &SkillNode> {
        self.nodes.values()
    }

    /// 安全约束校验（Milestone B-3a）— 悬空依赖 + 循环依赖检测
    ///
    /// # 检查项
    /// 1. **悬空依赖**：技能 dependencies 引用的技能不在图中
    /// 2. **循环依赖**：DFS 检测 A→…→A 环（含自依赖 a→a）
    ///
    /// # 返回
    /// 违规列表（空 = 图安全）。纯查询（&self），不改变图状态。
    ///
    /// # 复杂度
    /// O(V + E)（拓扑探测：每个节点至多访问一次）
    pub fn check_security(&self) -> Vec<SecurityViolation> {
        let mut violations = Vec::new();

        // 1. 悬空依赖
        for node in self.nodes.values() {
            for dep in &node.dependencies {
                if !self.nodes.contains_key(dep) {
                    violations.push(SecurityViolation {
                        skill_id: node.skill_id.clone(),
                        reason: format!("悬空依赖: 引用的技能 '{dep}' 不存在于图中"),
                    });
                }
            }
        }

        // 2. 循环依赖（DFS 三色标记：0=未访问 1=访问中 2=已结束）
        // WHY 三色而非简单 visited：只有"访问中"栈上的回边才是环，
        // 已结束节点的回边（如菱形依赖）不是环。
        // WHY 递归后序：节点必须等全部子节点处理完才标记 2，
        // 迭代版"pop 即标 2"会让祖先提前变 2 导致环漏检（B-3a 实测发现）。
        let mut color: HashMap<String, u8> = HashMap::new();
        let mut path: Vec<String> = Vec::new();
        for skill_id in self.nodes.keys() {
            if color.get(skill_id.as_str()) == Some(&2) {
                continue;
            }
            self.dfs_cycle_detect(skill_id, &mut color, &mut path, &mut violations);
        }

        violations
    }

    /// 循环依赖 DFS 辅助（后序三色；技能图规模小，递归安全）
    fn dfs_cycle_detect(
        &self,
        skill_id: &str,
        color: &mut HashMap<String, u8>,
        path: &mut Vec<String>,
        violations: &mut Vec<SecurityViolation>,
    ) {
        color.insert(skill_id.to_string(), 1);
        path.push(skill_id.to_string());
        if let Some(node) = self.nodes.get(skill_id) {
            for dep in &node.dependencies {
                match color.get(dep.as_str()) {
                    Some(1) => {
                        // 回边 → 环；报告环起点（path 中该依赖的位置）
                        let start = path.iter().position(|s| s == dep).unwrap_or(0);
                        let cycle = &path[start..];
                        let mut reason = format!("循环依赖: {}", cycle.join(" → "));
                        reason.push_str(&format!(" → {dep}"));
                        if !violations
                            .iter()
                            .any(|v: &SecurityViolation| v.reason == reason)
                        {
                            violations.push(SecurityViolation {
                                skill_id: cycle[0].clone(),
                                reason,
                            });
                        }
                    }
                    None => self.dfs_cycle_detect(dep, color, path, violations),
                    _ => {} // 已结束节点的回边不是环（菱形依赖）
                }
            }
        }
        path.pop();
        color.insert(skill_id.to_string(), 2);
    }

    /// 规则式联合演进(方案 §9.2 co_evolve 的降级实现,ADR-050 同款哲学)
    ///
    /// 1. 发现新技能:pattern 频次 ≥3 且未入图 → 新增节点
    /// 2. 更新依赖:调用序列相邻技能建依赖边(后者依赖前者)
    /// 3. 更新统计:已有技能累积复用次数,成功率 EWMA(α=0.3)
    pub fn evolve_with_patterns(&mut self, patterns: &[SkillUsagePattern]) {
        for pattern in patterns {
            match self.nodes.get_mut(&pattern.skill_id) {
                Some(node) => {
                    node.reuse_count += pattern.frequency;
                    // EWMA 平滑:新观测权重 0.3,避免单批次抖动覆盖历史
                    node.success_rate = node.success_rate * 0.7 + pattern.success_rate * 0.3;
                }
                None if pattern.frequency >= NEW_SKILL_THRESHOLD => {
                    self.nodes.insert(
                        pattern.skill_id.clone(),
                        SkillNode {
                            skill_id: pattern.skill_id.clone(),
                            embedding: pattern.embedding.clone(),
                            success_rate: pattern.success_rate,
                            reuse_count: pattern.frequency,
                            dependencies: Vec::new(),
                        },
                    );
                }
                // 低频未入图技能:不吸纳(防技能碎片化)
                None => {}
            }

            // 序列相邻依赖:B 紧随 A 出现 → B 依赖 A
            for window in pattern.sequence.windows(2) {
                if let Some(node) = self.nodes.get_mut(&window[1]) {
                    if !node.dependencies.contains(&window[0]) {
                        node.dependencies.push(window[0].clone());
                    }
                }
            }
        }
    }

    /// 五因子技能推荐(Ω₆ 复用率优先)
    ///
    /// score = 语义相似×0.4 + 成功率×0.25 + 复用频率(ln 压缩)×0.2 + 依赖满足度×0.15
    ///
    /// WHY ln 压缩复用频率:线性计数会让高频老技能垄断推荐,
    /// 对数压缩保留"常用优先"信号同时给新技能露出窗口。
    pub fn recommend(&self, task: &CLV, available_skills: &[String]) -> Vec<SkillRecommendation> {
        let mut scored: Vec<SkillRecommendation> = self
            .nodes
            .values()
            .map(|node| {
                let semantic = node.embedding.cosine_similarity(task).max(0.0);
                let success = node.success_rate;
                // ln(1+n)/ln(1+100) 归一化:100 次复用即满分
                let reuse = ((node.reuse_count as f32 + 1.0).ln() / (101.0f32).ln()).min(1.0);
                let deps_satisfied = if node.dependencies.is_empty() {
                    1.0
                } else {
                    node.dependencies
                        .iter()
                        .filter(|d| available_skills.contains(d))
                        .count() as f32
                        / node.dependencies.len() as f32
                };
                SkillRecommendation {
                    skill_id: node.skill_id.clone(),
                    relevance: semantic * 0.4
                        + success * 0.25
                        + reuse * 0.2
                        + deps_satisfied * 0.15,
                    dependencies: node.dependencies.clone(),
                }
            })
            .collect();

        // Top-K 用 select_nth_unstable_by(§4.1 红线:禁止 sort_by 做 Top-K)
        let k = RECOMMEND_TOP_K.min(scored.len());
        if k == 0 {
            return scored;
        }
        if k < scored.len() {
            scored.select_nth_unstable_by(k - 1, |a, b| {
                b.relevance
                    .partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            scored.truncate(k);
        }
        // Top-K 内部再排序输出(K≤10,开销可忽略)
        scored.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_clv(dim: usize) -> CLV {
        // 算法体已收敛到 L1 `nexus_core::CLV::basis`(单一权威构造器),
        // 此处仅保留本地签名,避免改动本文件内数十处调用点。
        // basis 越界返回 None;夹具若下标非法则该测试无效,直接 expect 暴露。
        CLV::basis(dim).expect("测试夹具:下标须在 CLV::DIMENSION 内")
    }

    fn pattern(id: &str, dim: usize, freq: u32, success: f32) -> SkillUsagePattern {
        SkillUsagePattern {
            skill_id: id.into(),
            embedding: unit_clv(dim),
            frequency: freq,
            success_rate: success,
            sequence: vec![],
        }
    }

    #[test]
    fn test_new_skill_discovered_at_frequency_threshold() {
        let mut graph = SkillGraph::new();
        graph.evolve_with_patterns(&[pattern("rare", 0, 2, 0.9)]); // 频次 2 < 3
        assert!(graph.is_empty(), "低频模式不应入图");
        graph.evolve_with_patterns(&[pattern("common", 0, 3, 0.9)]);
        assert_eq!(graph.len(), 1);
    }

    #[test]
    fn test_sequence_builds_dependency_edges() {
        let mut graph = SkillGraph::new();
        graph.evolve_with_patterns(&[pattern("parse", 0, 5, 0.9), pattern("compile", 1, 5, 0.8)]);
        // compile 紧随 parse → compile 依赖 parse
        let mut with_seq = pattern("compile", 1, 1, 0.8);
        with_seq.sequence = vec!["parse".into(), "compile".into()];
        graph.evolve_with_patterns(&[with_seq]);
        assert_eq!(
            graph.get("compile").unwrap().dependencies,
            vec!["parse".to_string()]
        );
    }

    #[test]
    fn test_success_rate_ewma_smoothing() {
        let mut graph = SkillGraph::new();
        graph.evolve_with_patterns(&[pattern("s", 0, 5, 1.0)]);
        graph.evolve_with_patterns(&[pattern("s", 0, 1, 0.0)]);
        // EWMA:1.0×0.7 + 0.0×0.3 = 0.7(单批次失败不清零历史)
        assert!((graph.get("s").unwrap().success_rate - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_recommend_prefers_semantic_match_and_reuse() {
        let mut graph = SkillGraph::new();
        graph.evolve_with_patterns(&[
            pattern("relevant", 0, 50, 0.9),
            pattern("irrelevant", 1, 50, 0.9),
        ]);
        let recs = graph.recommend(&unit_clv(0), &[]);
        assert_eq!(recs[0].skill_id, "relevant", "语义匹配应排首位");
        assert!(recs[0].relevance > recs[1].relevance);
    }

    #[test]
    fn test_recommend_caps_at_top_k() {
        let mut graph = SkillGraph::new();
        let patterns: Vec<SkillUsagePattern> = (0..15)
            .map(|i| pattern(&format!("s{i}"), i % 8, 5, 0.5))
            .collect();
        graph.evolve_with_patterns(&patterns);
        let recs = graph.recommend(&unit_clv(0), &[]);
        assert_eq!(recs.len(), RECOMMEND_TOP_K);
    }
}
