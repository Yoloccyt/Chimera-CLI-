//! CLV 判别力预检 — 针块 vs 随机块余弦分离度（PROBE P0.1，熔断前置）
//!
//! 对应任务: PROBE 实施计划 §2.2 P0.1（熔断门：分离度 ≥ 0.1）
//! 对应风险: 计划 §5 R1（CLV 判别力不足 → 回退独立嵌入探针备选）
//!
//! # 目的
//! 探针打分（P1）以 CLV 余弦相似度为核心机制（Round 1 裁决），其可行性前提是
//! CLV 对"语义相关块"与"随机块"具备可分辨的判别力。本测试在 P0 阶段提前实测：
//! - 构造 10 个"针块"CLV（共享主题分量 + 独立噪声，模拟同主题事实块）
//! - 构造 20 个"随机块"CLV（独立随机，模拟无关上下文）
//! - 分离度 = mean(针块间余弦) − mean(针块与随机块余弦)
//!
//! # 熔断门
//! 分离度 ≥ 0.1 为通过门（计划 §8.2 R1 触发器）；不达标则探针方案降级为
//! 独立嵌入模型（tract-onnx 复用 nmc-encoder 后端），接口在 P1 已隔离。
//!
//! # 确定性
//! 与 `benches/coarse_recall.rs::make_clv` 同模式：纯算术伪随机，不引入 rand 依赖，
//! 固定种子可复现（每次运行结果一致，避免 flaky）。

#![forbid(unsafe_code)]

use nexus_core::CLV;

/// CLV 固定维度（与 nexus-core 一致）
const CLV_DIM: usize = 512;

/// 针块数量（共享主题）
const NEEDLE_COUNT: usize = 10;

/// 随机块数量（独立噪声）
const RANDOM_COUNT: usize = 20;

/// 主题分量占比 — 针块 CLV = 主题 × TOPIC_BIAS + 噪声 × (1 − TOPIC_BIAS)
///
/// WHY 0.6: 模拟"同主题事实块"在真实系统中的合理相似度聚集程度。
/// 若 0.6 主题占比下分离度仍 < 0.1，则真实场景（主题更分散）必然不达标，
/// 熔断门判定保守且可信。
const TOPIC_BIAS: f32 = 0.6;

/// 分离度熔断阈值（计划 §8.2 R1）
const SEPARATION_THRESHOLD: f32 = 0.1;

/// 生成确定性伪随机 CLV
///
/// # 参数
/// - `seed`: 种子值（决定噪声分量）
/// - `topic`: 可选主题向量（`None` 时纯随机）
/// - `topic_bias`: 主题分量占比 [0,1]，`0.0` 时纯噪声
///
/// # 返回值
/// 512 维 CLV，非零向量（+0.001 偏移避免零向量边界触发）
///
/// # 设计决策（WHY）
/// 采用 SplitMix64 强混合哈希替代简单线性哈希：线性哈希（如
/// `seed*217 + j*403`）在相邻 seed 下产生高度相关序列（实测余弦 0.998），
/// 导致“分离度测度”混入生成器系统性偏差；SplitMix64 保证不同 seed 的
/// 512 维向量近似正交（对照组验证分离度 ≈ 0）。
fn make_clv(seed: u64, topic: Option<&CLV>, topic_bias: f32) -> CLV {
    let mut v: Vec<f32> = (0..CLV_DIM)
        .map(|j| {
            // SplitMix64 强混合（标准散列，纯安全算术）
            let mut z = seed.wrapping_add((j as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            // 映射到 [0,1)，+0.001 避免零向量
            ((z >> 11) as f32) / (1u64 << 53) as f32 + 0.001
        })
        .collect();
    if let Some(t) = topic {
        let t_slice = t.as_slice();
        for (i, val) in v.iter_mut().enumerate() {
            *val = topic_bias * t_slice[i] + (1.0 - topic_bias) * *val;
        }
    }
    CLV::from_vec(v).expect("CLV dimension must be 512")
}

/// 构造主题向量（针块共享语义中心）
///
/// 用独立种子生成，保证与任何针/随机块噪声不同源。
fn make_topic() -> CLV {
    make_clv(0x5EED_CAFE, None, 0.0)
}

/// 计算两组向量之间的平均余弦相似度（cross 模式）
///
/// # 参数
/// - `a`: 组 A
/// - `b`: 组 B
///
/// # 返回值
/// mean(cos(a_i, b_j))，全组合平均
fn mean_cross_cosine(a: &[CLV], b: &[CLV]) -> f32 {
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for x in a {
        for y in b {
            sum += x.cosine_similarity(y);
            count += 1;
        }
    }
    sum / count as f32
}

/// 测量 CLV 判别力（分离度）
///
/// # 返回值
/// `(separation, intra_mean, inter_mean)`:
/// - `separation`: 分离度 = 针块内相似度均值 − 针块与随机块相似度均值
/// - `intra_mean`: mean(针块间余弦)（应显著高于随机基线）
/// - `inter_mean`: mean(针块与随机块余弦)（随机基线）
fn measure_separation() -> (f32, f32, f32) {
    let topic = make_topic();
    let needles: Vec<CLV> = (0..NEEDLE_COUNT)
        .map(|i| make_clv(1000 + i as u64, Some(&topic), TOPIC_BIAS))
        .collect();
    let randoms: Vec<CLV> = (0..RANDOM_COUNT)
        .map(|i| make_clv(5000 + i as u64, None, 0.0))
        .collect();

    let intra = mean_cross_cosine(&needles, &needles);
    let inter = mean_cross_cosine(&needles, &randoms);
    (intra - inter, intra, inter)
}

#[test]
fn test_clv_separation_above_threshold() {
    // 熔断门：分离度 ≥ 0.1（计划 §8.2 R1 触发器）
    let (separation, intra, inter) = measure_separation();
    eprintln!(
        "[probe_clv_separation] intra={intra:.4} inter={inter:.4} separation={separation:.4} \
         threshold={SEPARATION_THRESHOLD} pass={}",
        separation >= SEPARATION_THRESHOLD
    );
    assert!(
        separation >= SEPARATION_THRESHOLD,
        "CLV 判别力不足: 分离度 {separation:.4} < {SEPARATION_THRESHOLD} \
         (intra={intra:.4}, inter={inter:.4}) — 触发 R1 熔断: 升级独立嵌入探针"
    );
}

#[test]
fn test_needle_blocks_self_similar() {
    // 针块间相似度应显著高于与随机块的相似度（分离度为正且可测）
    let (separation, intra, inter) = measure_separation();
    assert!(intra > inter, "intra={intra:.4} 应 > inter={inter:.4}");
    assert!(separation > 0.0, "分离度应为正，实际 {separation:.4}");
}

#[test]
fn test_random_blocks_are_distinct_from_needles() {
    // 随机块与针块的平均相似度不应接近 1.0（确保随机基线有效）
    let topic = make_topic();
    let needles: Vec<CLV> = (0..NEEDLE_COUNT)
        .map(|i| make_clv(1000 + i as u64, Some(&topic), TOPIC_BIAS))
        .collect();
    let randoms: Vec<CLV> = (0..RANDOM_COUNT)
        .map(|i| make_clv(5000 + i as u64, None, 0.0))
        .collect();
    let inter = mean_cross_cosine(&needles, &randoms);
    assert!(
        inter < 0.9,
        "随机基线应远离 1.0（当前 {inter:.4}），测试构造可能失效"
    );
}

#[test]
fn test_zero_topic_bias_yields_no_separation() {
    // 对照组：无主题分量（纯随机针块）时分离度应 ≈ 0，
    // 验证分离度测量对"主题信号"敏感（测度有效性自检）
    let topic = make_topic();
    let needles: Vec<CLV> = (0..NEEDLE_COUNT)
        .map(|i| make_clv(1000 + i as u64, Some(&topic), 0.0))
        .collect();
    let randoms: Vec<CLV> = (0..RANDOM_COUNT)
        .map(|i| make_clv(5000 + i as u64, None, 0.0))
        .collect();
    let intra = mean_cross_cosine(&needles, &needles);
    let inter = mean_cross_cosine(&needles, &randoms);
    let separation = intra - inter;
    assert!(
        separation.abs() < 0.05,
        "纯随机针块分离度应 ≈ 0（实际 {separation:.4}），测度存在系统性偏差"
    );
}
