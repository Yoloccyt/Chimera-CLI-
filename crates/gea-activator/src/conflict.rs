//! GEA 专家冲突消解 — 功能重叠检测与 Top-K 选择
//!
//! 对应架构层:L6 Router
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 设计决策(WHY)
//! - 综合评分 = gate_value × expert_priority:门控值反映任务匹配度,
//!   优先级反映专家本身的能力权重,两者相乘得到最终排序依据
//! - 功能重叠检测基于 CLV 余弦相似度:复用 `nexus_core::cosine_similarity_slices`,
//!   重叠度 > overlap_threshold 时仅保留评分更高者,避免冗余激活
//! - Top-K 排序用 `select_nth_unstable`:Top-K 选择 O(n),
//!   优于全排序 `sort_by` 的 O(n log n)(继承 Week 3 经验)

use std::collections::HashMap;

use crate::config::GeaConfig;
use crate::error::GeaError;
use crate::types::{ActivationResult, ExpertId, ExpertProfile};

/// 候选专家条目:(ExpertId, gate_value)
pub type Candidate = (ExpertId, f32);

/// 综合评分条目:(ExpertId, gate_value, composite_score)
///
/// `composite_score = gate_value × expert_priority`
type ScoredCandidate = (ExpertId, f32, f32);

/// 解决专家冲突:综合评分排序 + 功能重叠检测 + Top-K 选择
///
/// # 算法步骤
/// 1. 计算每个候选的综合评分:`gate_value × expert_priority`
/// 2. 按综合评分降序排序
/// 3. 贪心遍历:对每个候选,检查与已激活专家的重叠度,
///    重叠度 > `overlap_threshold` 则抑制(仅保留评分更高者)
/// 4. 取 Top-K 作为最终激活列表,其余为抑制列表
///
/// # 参数
/// - `candidates`:候选专家列表 `(ExpertId, gate_value)`
/// - `expert_profiles`:专家画像表,用于查询优先级与向量
/// - `config`:配置(含 overlap_threshold、top_k)
///
/// # 错误
/// - `ExpertNotFound`:候选专家不在 `expert_profiles` 中
/// - `ConflictResolutionFailed`:所有候选均被抑制(理论上不会发生,防御性返回)
pub fn resolve_conflicts(
    candidates: Vec<Candidate>,
    expert_profiles: &HashMap<ExpertId, ExpertProfile>,
    config: &GeaConfig,
) -> Result<ActivationResult, GeaError> {
    if candidates.is_empty() {
        return Ok(ActivationResult::empty());
    }

    // 步骤 1:计算综合评分,校验专家存在性
    let mut scored: Vec<ScoredCandidate> = Vec::with_capacity(candidates.len());
    for (expert_id, gate_value) in candidates {
        let profile = expert_profiles
            .get(&expert_id)
            .ok_or_else(|| GeaError::ExpertNotFound {
                expert_id: expert_id.to_string(),
            })?;
        let composite = gate_value * profile.priority;
        scored.push((expert_id, gate_value, composite));
    }

    // 步骤 2:按综合评分降序排序(全排序,因为后续需贪心遍历全部)
    // WHY 全排序而非 select_nth_unstable:冲突检测需按评分从高到低贪心遍历,
    // select_nth_unstable 仅保证 Top-K 在前 K 位但内部无序,无法满足贪心顺序要求。
    // Top-K 选择优化在步骤 4 之后对"已通过冲突检测的列表"使用。
    scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    // 步骤 3:贪心冲突检测 — 重叠度 > threshold 则抑制
    let mut activated_with_score: Vec<ScoredCandidate> = Vec::new();
    let mut suppressed: Vec<ExpertId> = Vec::new();

    for candidate in scored {
        let profile =
            expert_profiles
                .get(&candidate.0)
                .ok_or_else(|| GeaError::ExpertNotFound {
                    expert_id: candidate.0.to_string(),
                })?;

        // 检查与所有已激活专家的重叠度
        let mut conflict = false;
        for activated in &activated_with_score {
            let activated_profile =
                expert_profiles
                    .get(&activated.0)
                    .ok_or_else(|| GeaError::ExpertNotFound {
                        expert_id: activated.0.to_string(),
                    })?;
            // 复用 nexus_core 余弦相似度,取最小长度(兼容维度差异)
            let overlap = nexus_core::cosine_similarity_slices(
                &profile.expert_vector,
                &activated_profile.expert_vector,
            );
            if overlap > config.overlap_threshold {
                conflict = true;
                break;
            }
        }

        if conflict {
            suppressed.push(candidate.0);
        } else {
            activated_with_score.push(candidate);
        }
    }

    // 步骤 4:Top-K 选择 — 使用 select_nth_unstable 优化(O(n))
    // WHY:已通过冲突检测的列表可能超过 top_k,只需前 K 个,无需全排序
    let top_gate_value = activated_with_score.first().map(|c| c.1).unwrap_or(0.0);

    let (activated, extra_suppressed) = select_top_k(activated_with_score, config.top_k);

    // 未进入 Top-K 的也加入 suppressed
    let mut all_suppressed = suppressed;
    all_suppressed.extend(extra_suppressed);

    Ok(ActivationResult {
        activated,
        suppressed: all_suppressed,
        top_gate_value,
    })
}

/// 从已通过冲突检测的列表中选择 Top-K
///
/// 使用 `select_nth_unstable_by` 实现 O(n) 的 Top-K 选择,
/// 然后对前 K 个元素排序得到降序排列的激活列表。
///
/// 返回 (activated_top_k, suppressed_extra)
///
/// WHY pivot 处理:`select_nth_unstable_by(k, ...)` 返回 (left, pivot, right),
/// left 有 k 个元素(索引 0..k),pivot 是第 k 个元素(索引 k),right 是剩余。
/// pivot 不属于 Top-K,必须加入 suppressed,否则会丢失条目。
fn select_top_k(mut scored: Vec<ScoredCandidate>, k: usize) -> (Vec<ExpertId>, Vec<ExpertId>) {
    if scored.len() <= k {
        // 全部激活,无额外抑制
        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        let activated: Vec<ExpertId> = scored.into_iter().map(|c| c.0).collect();
        return (activated, Vec::new());
    }

    // select_nth_unstable_by:第 k 个元素就位,前 k 个为 Top-K(无序)
    // WHY unwrap_or(sorted):partial_cmp 对 NaN 返回 None,但门控值经 clamp 不会为 NaN
    let (top_k, pivot, rest) = scored.select_nth_unstable_by(k, |a, b| {
        b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal)
    });

    // 对 Top-K 排序得到降序
    let mut top_k_sorted: Vec<ScoredCandidate> = top_k.to_vec();
    top_k_sorted.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let activated: Vec<ExpertId> = top_k_sorted.into_iter().map(|c| c.0).collect();

    // pivot 和 rest 均不属于 Top-K,加入抑制列表
    let mut suppressed: Vec<ExpertId> = Vec::with_capacity(rest.len() + 1);
    suppressed.push(pivot.0.clone());
    suppressed.extend(rest.iter().map(|c| c.0.clone()));
    (activated, suppressed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_profile(id: &str, vector: Vec<f32>, priority: f32) -> (ExpertId, ExpertProfile) {
        (
            ExpertId::new(id),
            ExpertProfile::new(id, vector, priority, vec![]),
        )
    }

    fn make_profiles(items: Vec<(&str, Vec<f32>, f32)>) -> HashMap<ExpertId, ExpertProfile> {
        items
            .into_iter()
            .map(|(id, v, p)| make_profile(id, v, p))
            .collect()
    }

    #[test]
    fn test_no_conflicts_basic() {
        // 三个专家向量正交,无冲突
        let mut v1 = vec![0.0; 64];
        v1[0] = 1.0;
        let mut v2 = vec![0.0; 64];
        v2[1] = 1.0;
        let mut v3 = vec![0.0; 64];
        v3[2] = 1.0;

        let profiles = make_profiles(vec![("e-1", v1, 0.5), ("e-2", v2, 0.5), ("e-3", v3, 0.5)]);

        let candidates: Vec<Candidate> = vec![
            (ExpertId::new("e-1"), 0.8),
            (ExpertId::new("e-2"), 0.7),
            (ExpertId::new("e-3"), 0.6),
        ];

        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        // 无冲突,Top-3 全部激活
        assert_eq!(result.activated.len(), 3);
        assert!(result.suppressed.is_empty());
        // 按综合评分降序:0.8*0.5=0.4 > 0.7*0.5=0.35 > 0.6*0.5=0.3
        assert_eq!(result.activated[0], ExpertId::new("e-1"));
        assert_eq!(result.activated[1], ExpertId::new("e-2"));
        assert_eq!(result.activated[2], ExpertId::new("e-3"));
        assert!((result.top_gate_value - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_conflict_high_overlap() {
        // 两个专家向量高度重叠(相同向量),重叠度 = 1.0 > 0.8
        let v = vec![1.0; 64];
        let profiles = make_profiles(vec![("e-1", v.clone(), 0.5), ("e-2", v, 0.5)]);

        let candidates: Vec<Candidate> =
            vec![(ExpertId::new("e-1"), 0.8), (ExpertId::new("e-2"), 0.7)];

        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        // e-1 评分更高(0.8 > 0.7),e-2 被抑制
        assert_eq!(result.activated.len(), 1);
        assert_eq!(result.activated[0], ExpertId::new("e-1"));
        assert_eq!(result.suppressed.len(), 1);
        assert_eq!(result.suppressed[0], ExpertId::new("e-2"));
    }

    #[test]
    fn test_top_k_boundary() {
        // 5 个无冲突专家,top_k = 3,应激活 3 个,抑制 2 个
        let profiles = make_profiles(vec![
            ("e-1", make_orthogonal(0), 0.5),
            ("e-2", make_orthogonal(1), 0.5),
            ("e-3", make_orthogonal(2), 0.5),
            ("e-4", make_orthogonal(3), 0.5),
            ("e-5", make_orthogonal(4), 0.5),
        ]);

        let candidates: Vec<Candidate> = vec![
            (ExpertId::new("e-1"), 0.9),
            (ExpertId::new("e-2"), 0.8),
            (ExpertId::new("e-3"), 0.7),
            (ExpertId::new("e-4"), 0.6),
            (ExpertId::new("e-5"), 0.5),
        ];

        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        assert_eq!(result.activated.len(), 3);
        assert_eq!(result.suppressed.len(), 2);
        // Top-3 按评分降序
        assert_eq!(result.activated[0], ExpertId::new("e-1"));
        assert_eq!(result.activated[1], ExpertId::new("e-2"));
        assert_eq!(result.activated[2], ExpertId::new("e-3"));
    }

    #[test]
    fn test_empty_candidates() {
        let profiles = HashMap::new();
        let config = GeaConfig::default();
        let result = resolve_conflicts(vec![], &profiles, &config).unwrap();
        assert!(!result.has_activated());
        assert!((result.top_gate_value - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_expert_not_found() {
        let profiles = HashMap::new();
        let config = GeaConfig::default();
        let candidates: Vec<Candidate> = vec![(ExpertId::new("missing"), 0.8)];
        let result = resolve_conflicts(candidates, &profiles, &config);
        assert!(matches!(result, Err(GeaError::ExpertNotFound { .. })));
    }

    #[test]
    fn test_priority_influence() {
        // e-2 门控值更低但优先级更高,综合评分可能超过 e-1
        let profiles = make_profiles(vec![
            ("e-1", make_orthogonal(0), 0.1), // 0.8 * 0.1 = 0.08
            ("e-2", make_orthogonal(1), 1.0), // 0.7 * 1.0 = 0.70
        ]);

        let candidates: Vec<Candidate> =
            vec![(ExpertId::new("e-1"), 0.8), (ExpertId::new("e-2"), 0.7)];

        let config = GeaConfig::default();
        let result = resolve_conflicts(candidates, &profiles, &config).unwrap();

        // e-2 综合评分更高(0.70 > 0.08),应排第一
        assert_eq!(result.activated[0], ExpertId::new("e-2"));
        assert!((result.top_gate_value - 0.7).abs() < 1e-6);
    }

    /// 生成正交向量:仅第 idx 维为 1.0,其余为 0.0
    fn make_orthogonal(idx: usize) -> Vec<f32> {
        let mut v = vec![0.0; 64];
        if idx < 64 {
            v[idx] = 1.0;
        }
        v
    }
}
