//! GEA 门控值计算 — Sigmoid 门控
//!
//! 对应架构层:L6 Router
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 设计决策(WHY)选择 Sigmoid 门控
//! - 连续可导,输出 ∈ (0, 1),适合门控值计算
//! - 连续 `[0,1]` 允许"部分激活",对应 OMEGA 的 Ω-Sparse 稀疏化理念
//! - 相比 hard gate(0/1),sigmoid 提供平滑梯度,利于后续 GSOE 在线进化
//!
//! # 公式
//! `gate = sigmoid(w1 × complexity + w2 × relevance + w3 × affinity - bias)`
//! - `complexity` = task.complexity_score
//! - `relevance` = cosine_similarity(task.clv, expert.expert_vector)
//! - `affinity` = 能力标签匹配度(匹配标签数 / 总标签数)

use crate::config::GeaConfig;
use crate::types::{ExpertProfile, TaskProfile};

/// Sigmoid 函数:`1.0 / (1.0 + exp(-x))`
///
/// WHY 标准库 f32::exp:无需引入额外依赖,精度足够
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// 计算能力标签亲和度:匹配标签数 / 专家总标签数
///
/// WHY 占位实现:当前用简单集合匹配,后续可替换为语义相似度。
/// 分母用专家标签数(非任务标签数),保证亲和度反映"专家能力覆盖任务需求"的程度。
fn compute_affinity(task: &TaskProfile, expert: &ExpertProfile) -> f32 {
    if expert.capability_tags.is_empty() {
        return 0.0;
    }
    // 任务类型作为隐式标签参与匹配
    let task_tag = &task.task_type;
    let matched = expert
        .capability_tags
        .iter()
        .filter(|tag| tag.as_str() == task_tag)
        .count();
    matched as f32 / expert.capability_tags.len() as f32
}

/// 计算门控值
///
/// 公式:`gate = sigmoid(w1 × complexity + w2 × relevance + w3 × affinity - bias)`
///
/// - `complexity` = `task.complexity_score`
/// - `relevance` = `nexus_core::cosine_similarity_slices(&task.clv, &expert.expert_vector)`
///   (维度不同时取较短长度,由 `cosine_similarity_slices` 处理)
/// - `affinity` = 能力标签匹配度
///
/// 返回值 clamp 到 [0.0, 1.0] 防止浮点误差导致的微小越界。
pub fn compute_gate_value(task: &TaskProfile, expert: &ExpertProfile, config: &GeaConfig) -> f32 {
    let complexity = task.complexity_score;
    // 维度可能不同(clv 512 维 vs expert_vector 64 维),
    // cosine_similarity_slices 内部取最小长度,兼容不等长输入
    let relevance = nexus_core::cosine_similarity_slices(&task.clv, &expert.expert_vector);
    let affinity = compute_affinity(task, expert);

    let raw = config.w1 * complexity + config.w2 * relevance + config.w3 * affinity - config.bias;
    let gate = sigmoid(raw);

    // clamp 防止浮点误差导致的微小越界(sigmoid 理论输出 (0,1))
    gate.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_expert(id: &str, vector: Vec<f32>, tags: Vec<&str>) -> ExpertProfile {
        ExpertProfile::new(
            id,
            vector,
            0.5,
            tags.into_iter().map(String::from).collect(),
        )
    }

    fn make_task(complexity: f32, clv: Vec<f32>, task_type: &str) -> TaskProfile {
        TaskProfile::new(complexity, task_type, 30, clv)
    }

    #[test]
    fn test_gate_value_in_range() {
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![0.5; 64], vec!["code-gen"]);
        let task = make_task(0.8, vec![0.5; 64], "code-gen");

        let gate = compute_gate_value(&task, &expert, &config);
        assert!((0.0..=1.0).contains(&gate), "gate {gate} out of [0,1]");
    }

    #[test]
    fn test_gate_value_boundary_complexity_zero() {
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![1.0; 64], vec![]);
        // complexity = 0, relevance = 1.0(相同向量), affinity = 0(无标签)
        // raw = 0.4*0 + 0.3*1.0 + 0.3*0 - 0.5 = -0.2
        // sigmoid(-0.2) ≈ 0.4502
        let task = make_task(0.0, vec![1.0; 64], "test");
        let gate = compute_gate_value(&task, &expert, &config);
        let expected = sigmoid(-0.2);
        assert!(
            (gate - expected).abs() < 1e-5,
            "expected {expected}, got {gate}"
        );
    }

    #[test]
    fn test_gate_value_boundary_complexity_one() {
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![1.0; 64], vec!["code-gen"]);
        // complexity = 1, relevance = 1.0, affinity = 1.0(标签匹配)
        // raw = 0.4*1 + 0.3*1 + 0.3*1 - 0.5 = 0.5
        // sigmoid(0.5) ≈ 0.6225
        let task = make_task(1.0, vec![1.0; 64], "code-gen");
        let gate = compute_gate_value(&task, &expert, &config);
        let expected = sigmoid(0.5);
        assert!(
            (gate - expected).abs() < 1e-5,
            "expected {expected}, got {gate}"
        );
    }

    #[test]
    fn test_weight_influence_w1() {
        // 提高 w1(复杂度权重),高复杂度任务的门控值应更高
        // WHY 正交向量:使 relevance=0、affinity=0(标签不匹配),
        // 这样门控值仅由 complexity 决定,凸显 w1 的影响
        let config_high_w1 = GeaConfig {
            w1: 0.8,
            w2: 0.1,
            w3: 0.1,
            ..Default::default()
        };
        let config_low_w1 = GeaConfig {
            w1: 0.1,
            w2: 0.45,
            w3: 0.45,
            ..Default::default()
        };
        // 正交向量:expert 在第 0 维,task 在第 1 维,relevance = 0.0
        let mut expert_vec = vec![0.0; 64];
        expert_vec[0] = 1.0;
        let mut task_clv = vec![0.0; 64];
        task_clv[1] = 1.0;
        let expert = make_expert("e-1", expert_vec, vec!["refactor"]);
        let task = make_task(0.9, task_clv, "code-gen");

        let gate_high = compute_gate_value(&task, &expert, &config_high_w1);
        let gate_low = compute_gate_value(&task, &expert, &config_low_w1);
        assert!(
            gate_high > gate_low,
            "higher w1 should yield higher gate for high-complexity task: {gate_high} vs {gate_low}"
        );
    }

    #[test]
    fn test_relevance_zero_vector() {
        let config = GeaConfig::default();
        // 零向量余弦相似度为 0.0(非 NaN)
        let expert = make_expert("e-1", vec![0.0; 64], vec![]);
        let task = make_task(0.5, vec![0.0; 64], "test");
        let gate = compute_gate_value(&task, &expert, &config);
        assert!(gate.is_finite(), "gate must be finite for zero vectors");
        assert!((0.0..=1.0).contains(&gate));
    }

    #[test]
    fn test_dimension_mismatch() {
        // task.clv 512 维,expert.expert_vector 64 维,应取最小长度计算
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![1.0; 64], vec![]);
        let task = make_task(0.5, vec![1.0; 512], "test");
        let gate = compute_gate_value(&task, &expert, &config);
        assert!(gate.is_finite());
        assert!((0.0..=1.0).contains(&gate));
    }

    #[test]
    fn test_affinity_no_tags() {
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![1.0; 64], vec![]);
        let task = make_task(0.5, vec![1.0; 64], "code-gen");
        let gate = compute_gate_value(&task, &expert, &config);
        // 无标签时 affinity = 0
        assert!(gate.is_finite());
    }

    #[test]
    fn test_affinity_tag_match() {
        let config = GeaConfig::default();
        // 标签匹配时门控值应高于不匹配
        let expert_match = make_expert("e-1", vec![1.0; 64], vec!["code-gen"]);
        let expert_no_match = make_expert("e-2", vec![1.0; 64], vec!["refactor"]);
        let task = make_task(0.5, vec![1.0; 64], "code-gen");

        let gate_match = compute_gate_value(&task, &expert_match, &config);
        let gate_no_match = compute_gate_value(&task, &expert_no_match, &config);
        assert!(
            gate_match > gate_no_match,
            "matched tags should yield higher gate"
        );
    }

    #[test]
    fn test_bias_influence() {
        // 更高 bias 使门控值更低(更难激活)
        let config_high_bias = GeaConfig {
            bias: 2.0,
            ..Default::default()
        };
        let config_low_bias = GeaConfig {
            bias: 0.0,
            ..Default::default()
        };
        let expert = make_expert("e-1", vec![1.0; 64], vec!["code-gen"]);
        let task = make_task(0.8, vec![1.0; 64], "code-gen");

        let gate_high_bias = compute_gate_value(&task, &expert, &config_high_bias);
        let gate_low_bias = compute_gate_value(&task, &expert, &config_low_bias);
        assert!(
            gate_high_bias < gate_low_bias,
            "higher bias should yield lower gate"
        );
    }
}
