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
//!
//! # 热路径范数预计算(专家 Agent 优化 2026-08-11)
//! - `compute_gate_value_with_norms`:激活循环专用变体。调用方一次算好
//!   task 前缀范数与专家范数(注册时缓存),内层只做 SIMD 友好的点积累加,
//!   省去 `cosine_similarity_slices` 每次调用重复计算 2× 范数 + sqrt
//! - `prefix_l2_norm`:与 `cosine_similarity_slices` 内部范数累加逐位一致
//!   (4 路累加器 + chunks_exact(4) + 尾部补算),保证剪枝/预计算边界无歧义
//! - 维度结构不匹配时(异常场景)自动回退 `compute_gate_value` 精确路径

use crate::config::GeaConfig;
use crate::types::{ExpertProfile, TaskProfile};

/// Sigmoid 函数:委托至 nexus_contracts::util::sigmoid(全程 f32,项目红线 §6.2 #6)
use nexus_contracts::util::sigmoid;

/// 计算向量前缀 `[0..len)` 的 L2 范数
///
/// WHY 与 `cosine_similarity_slices` 内部范数累加逐位一致(4 路累加器 +
/// chunks_exact(4) + 尾部补算 + 合并顺序相同):预计算范数用于门控热路径
/// 与冲突剪枝边界时,与精确路径的范数位级一致,无舍入歧义。
/// 被 conflict.rs 复用(重叠检测范数预计算)。
pub(crate) fn prefix_l2_norm(v: &[f32], len: usize) -> f32 {
    let prefix = &v[..len.min(v.len())];
    let (mut n0, mut n1, mut n2, mut n3) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    for chunk in prefix.chunks_exact(4) {
        n0 += chunk[0] * chunk[0];
        n1 += chunk[1] * chunk[1];
        n2 += chunk[2] * chunk[2];
        n3 += chunk[3] * chunk[3];
    }
    let mut norm = n0 + n1 + n2 + n3;
    let processed = (prefix.len() / 4) * 4;
    for vi in &prefix[processed..] {
        norm += *vi * *vi;
    }
    norm.sqrt()
}

/// 标签归一化 — casefold + 分隔符消除（W6,ADR-084）
///
/// "Code-Gen" / "code_gen" / "codeGen" → "codegen":消除命名风格差异
/// 对匹配的干扰（纯函数,proptest 锁定等价类）。
fn normalize_tag(tag: &str) -> String {
    tag.chars()
        .filter(|c| *c != '-' && *c != '_')
        .collect::<String>()
        .to_lowercase()
}

/// 计算能力标签亲和度 — 两级归一化匹配（W6 占位升级,ADR-084 决策 3）
///
/// 匹配策略（对原"task_type 精确等值"占位的升级,保持纯函数）:
/// - **精确匹配**（归一化后相等）= 1.0
/// - **前缀包含**（任一方向前缀,如 "codegen" ↔ "codegen-advanced"）= 0.5
/// - 分母用专家标签数（非任务标签数）,保证亲和度反映
///   "专家能力覆盖任务需求"的程度;结果 clamp [0,1]。
///
/// 诚实边界: 语义相似度（embedding 级）属 GSOE 进化侧职责,本层不引入
/// 臆造 tag embedding 基建（记档 ADR-084/执行报告）。
fn compute_affinity(task: &TaskProfile, expert: &ExpertProfile) -> f32 {
    if expert.capability_tags.is_empty() {
        return 0.0;
    }
    let task_tag = normalize_tag(&task.task_type);
    let mut score = 0.0f32;
    for tag in &expert.capability_tags {
        let normalized = normalize_tag(tag);
        if normalized == task_tag {
            score += 1.0; // 精确匹配（归一化等价类内）
        } else if !task_tag.is_empty()
            && !normalized.is_empty()
            && (normalized.starts_with(&task_tag) || task_tag.starts_with(&normalized))
        {
            score += 0.5; // 前缀包含（词族近邻）
        }
    }
    (score / expert.capability_tags.len() as f32).clamp(0.0, 1.0)
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
    // 专家历史成功率反馈:仅 w4 启用时计算 confidence(热路径分支,
    // 默认 w4=0 时跳过方法调用,保持与旧版门控性能持平)
    let confidence = if config.w4_confidence > 0.0 {
        expert.confidence()
    } else {
        0.0
    };

    let raw = config.w1 * complexity
        + config.w2 * relevance
        + config.w3 * affinity
        + config.w4_confidence * confidence
        - config.bias;
    let gate = sigmoid(raw);

    // clamp 防止浮点误差导致的微小越界(sigmoid 理论输出 (0,1))
    gate.clamp(0.0, 1.0)
}

/// 带预计算范数的门控计算 — activate 热路径专用(专家 Agent 优化 2026-08-11)
///
/// 当 `task.clv.len() >= expert.expert_vector.len()`(项目约定 CLV 512 维 ≥
/// 专家向量 64 维)时,min_len = 专家向量长度,relevance 可用调用方预计算的
/// task_norm(任务前缀范数,一次算好)与 expert_norm(专家注册时缓存)直接求得,
/// 省去每次调用的重复范数计算。
///
/// 维度结构不匹配时(如专家向量长于任务 CLV)回退 `compute_gate_value`
/// 精确路径,保证结果与基线一致。
///
/// ## 与精确路径的浮点差异
/// 点积为顺序累加(非 4 路累加器),relevance 与 `cosine_similarity_slices`
/// 存在 ~1e-7 级差异,经 w2 加权后对门控值影响 < 1e-7,远低于激活阈值判断
/// 的 1e-2 级裕量,不改变任何激活决策。
pub fn compute_gate_value_with_norms(
    task: &TaskProfile,
    task_norm: f32,
    expert: &ExpertProfile,
    expert_norm: f32,
    config: &GeaConfig,
) -> f32 {
    let len = task.clv.len().min(expert.expert_vector.len());
    // 仅当调用方预计算的范数恰为 len 前缀范数时使用预计算路径
    // (等长或专家向量短于任务 CLV 场景;len == expert.len() 保证 expert_norm 完整)
    if len == expert.expert_vector.len() && len > 0 {
        let dot: f32 = task.clv[..len]
            .iter()
            .zip(&expert.expert_vector[..len])
            .map(|(a, b)| a * b)
            .sum();
        let relevance = if task_norm > 0.0 && expert_norm > 0.0 {
            (dot / (task_norm * expert_norm)).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let affinity = compute_affinity(task, expert);
        // 仅 w4 启用时计算 confidence(与 compute_gate_value 行为一致)
        let confidence = if config.w4_confidence > 0.0 {
            expert.confidence()
        } else {
            0.0
        };
        let raw = config.w1 * task.complexity_score
            + config.w2 * relevance
            + config.w3 * affinity
            + config.w4_confidence * confidence
            - config.bias;
        return sigmoid(raw).clamp(0.0, 1.0);
    }
    // 回退:维度结构不匹配,走精确路径
    compute_gate_value(task, expert, config)
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

    // ============================================================
    // W6 占位升级测试（ADR-084 决策 3: 归一化 + 两级匹配）
    // ============================================================

    #[test]
    fn test_normalize_tag_equivalence_classes() {
        // 归一化等价类: 分隔符消除 + casefold
        assert_eq!(normalize_tag("Code-Gen"), "codegen");
        assert_eq!(normalize_tag("code_gen"), "codegen");
        assert_eq!(normalize_tag("codeGen"), "codegen");
        assert_eq!(normalize_tag("CODE-GEN"), normalize_tag("code_gen"));
        assert_eq!(normalize_tag(""), "");
    }

    #[test]
    fn test_affinity_normalized_match_upgrades_gate() {
        // W6 修复核心: 命名风格差异（kebab vs snake）不再错失匹配——
        // 原"精确等值"占位下 task="code_gen" vs tag="code-gen" 亲和度 0
        let config_match = GeaConfig {
            w3: 2.0, // 放大 affinity 通道差异便于断言
            w1: 0.0,
            w2: 0.0,
            w4_confidence: 0.0,
            ..GeaConfig::default()
        };
        let expert = make_expert("e-1", vec![1.0; 64], vec!["code-gen"]);
        let task_snake = make_task(0.5, vec![1.0; 64], "code_gen");
        let task_unrelated = make_task(0.5, vec![1.0; 64], "refactor");

        let gate_snake = compute_gate_value(&task_snake, &expert, &config_match);
        let gate_unrelated = compute_gate_value(&task_unrelated, &expert, &config_match);
        assert!(
            gate_snake > gate_unrelated,
            "归一化等价类内匹配应提升门控（snake/kebab 风格差异免疫）"
        );
    }

    #[test]
    fn test_affinity_prefix_partial_match() {
        // 前缀包含（0.5 权）: "codegen" ↔ "codegen-advanced" 词族近邻
        let config = GeaConfig {
            w3: 2.0,
            w1: 0.0,
            w2: 0.0,
            w4_confidence: 0.0,
            ..GeaConfig::default()
        };
        let expert = make_expert("e-1", vec![1.0; 64], vec!["codegen-advanced"]);
        let task_near = make_task(0.5, vec![1.0; 64], "codegen");
        let task_far = make_task(0.5, vec![1.0; 64], "refactor");

        let gate_near = compute_gate_value(&task_near, &expert, &config);
        let gate_far = compute_gate_value(&task_far, &expert, &config);
        assert!(
            gate_near > gate_far,
            "前缀词族近邻应获得部分亲和度（0.5 权）"
        );
    }

    #[test]
    fn test_affinity_bounded_and_empty_safe() {
        // 边界: 空任务类型不因 starts_with("") 前缀陷阱误匹配——
        // 零亲和（与完全无关任务同值）;结果域 [0,1]
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![1.0; 64], vec!["a", "b"]);
        let empty_task = make_task(0.5, vec![1.0; 64], "");
        let unrelated_task = make_task(0.5, vec![1.0; 64], "zzz");
        let gate_empty = compute_gate_value(&empty_task, &expert, &config);
        let gate_unrelated = compute_gate_value(&unrelated_task, &expert, &config);
        assert!(
            (gate_empty - gate_unrelated).abs() < 1e-6,
            "空任务类型必须零亲和（前缀陷阱免疫）"
        );
        assert!(gate_empty.is_finite() && (0.0..=1.0).contains(&gate_empty));
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

    // ============================================================
    // 热路径范数预计算(专家 Agent 优化 2026-08-11)
    // ============================================================

    #[test]
    fn test_with_norms_matches_exact_for_equal_dim() {
        // 等长场景:with_norms 与精确路径门控值应一致(浮点差异 < 1e-5)
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![0.5; 64], vec!["code-gen"]);
        let task = make_task(0.8, vec![0.5; 64], "code-gen");

        let exact = compute_gate_value(&task, &expert, &config);
        let task_norm = prefix_l2_norm(&task.clv, expert.expert_vector.len());
        let expert_norm = prefix_l2_norm(&expert.expert_vector, expert.expert_vector.len());
        let optimized =
            compute_gate_value_with_norms(&task, task_norm, &expert, expert_norm, &config);
        assert!(
            (optimized - exact).abs() < 1e-5,
            "with_norms 应与精确路径一致: optimized={optimized}, exact={exact}"
        );
    }

    #[test]
    fn test_with_norms_clv_longer_than_expert() {
        // 项目约定场景:CLV 512 维 > 专家向量 64 维,with_norms 走预计算路径
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![0.5; 64], vec!["code-gen"]);
        let task = make_task(0.9, vec![0.5; 512], "code-gen");

        let exact = compute_gate_value(&task, &expert, &config);
        let task_norm = prefix_l2_norm(&task.clv, expert.expert_vector.len());
        let expert_norm = prefix_l2_norm(&expert.expert_vector, expert.expert_vector.len());
        let optimized =
            compute_gate_value_with_norms(&task, task_norm, &expert, expert_norm, &config);
        assert!(
            (optimized - exact).abs() < 1e-5,
            "512d CLV 下 with_norms 应与精确路径一致: optimized={optimized}, exact={exact}"
        );
    }

    #[test]
    fn test_with_norms_falls_back_when_dim_mismatch() {
        // 异常场景:专家向量(128 维)长于任务 CLV(64 维),len != expert.len()
        // → 回退精确路径,结果与 compute_gate_value 完全一致
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![0.5; 128], vec!["code-gen"]);
        let task = make_task(0.8, vec![0.5; 64], "code-gen");

        let exact = compute_gate_value(&task, &expert, &config);
        let optimized = compute_gate_value_with_norms(&task, 1.0, &expert, 1.0, &config);
        assert_eq!(optimized, exact, "维度不匹配应回退精确路径");
    }

    #[test]
    fn test_with_norms_zero_vector_safe() {
        // 零向量:范数为 0 → relevance = 0(与精确路径零向量处理一致)
        let config = GeaConfig::default();
        let expert = make_expert("e-1", vec![0.0; 64], vec![]);
        let task = make_task(0.5, vec![0.0; 64], "test");

        let exact = compute_gate_value(&task, &expert, &config);
        let optimized = compute_gate_value_with_norms(&task, 0.0, &expert, 0.0, &config);
        assert!(optimized.is_finite(), "零向量门控值必须有限");
        assert!(
            (optimized - exact).abs() < 1e-5,
            "零向量下 with_norms 应与精确路径一致"
        );
    }

    #[test]
    fn test_prefix_l2_norm_matches_cosine_internal() {
        // 范数预计算与 cosine_similarity_slices 内部范数位级一致:
        // 同向量余弦应为 1.0(用预计算范数重建)
        let v = vec![0.3f32, 0.7, 0.2, 0.9, 0.5, 0.1, 0.8, 0.4];
        let norm = prefix_l2_norm(&v, v.len());
        let sim = nexus_core::cosine_similarity_slices(&v, &v);
        assert!((sim - 1.0).abs() < 1e-5, "同向量余弦应接近 1.0, got {sim}");
        assert!(norm > 0.0, "非零向量范数应 > 0");
        // 重建:dot = norm² → sim = norm² / norm² = 1.0
        let dot: f32 = v.iter().map(|x| x * x).sum();
        let rebuilt = dot / (norm * norm);
        assert!(
            (rebuilt - sim).abs() < 1e-5,
            "预计算范数重建的余弦应与精确一致: {rebuilt} vs {sim}"
        );
    }
}
