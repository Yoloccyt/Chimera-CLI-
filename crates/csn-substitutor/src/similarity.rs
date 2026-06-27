//! 余弦相似度计算 — 无外部依赖的纯 Rust 实现
//!
//! 对应架构层:L10 Interface
//!
//! ## 设计要点
//! - 纯 Rust 实现,不依赖 ndarray/cosine 等外部 crate(降低构建依赖)
//! - 零向量保护:任一向量为零向量时返回 0.0,避免 NaN 污染
//! - 长度不匹配保护:返回 0.0,避免 panic(系统边界校验)
//! - 单次遍历同时累加点积与模长,提升缓存局部性
//!
//! ## 公式
//! cos(a, b) = (a · b) / (||a|| * ||b||)

/// 计算两个向量的余弦相似度
///
/// # 参数
/// - `a`:向量 A(任意维度)
/// - `b`:向量 B(任意维度)
///
/// # 返回
/// 余弦相似度得分,范围 [-1.0, 1.0]:
/// - `1.0`:方向完全相同
/// - `0.0`:正交或任一向量为零向量
/// - `-1.0`:方向完全相反
///
/// # 边界处理
/// - 任一向量为零向量(模长为 0):返回 `0.0`(避免除零导致 NaN)
/// - 长度不匹配:返回 `0.0`(系统边界校验,不 panic)
/// - 空向量:返回 `0.0`
///
/// # 性能
/// 单次遍历 O(n),n = 向量维度。同时累加点积与模长平方,
/// 避免多次遍历,提升缓存局部性。
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    // 长度不匹配:返回 0.0(系统边界校验)
    if a.len() != b.len() {
        return 0.0;
    }

    // 单次遍历累加点积与模长平方
    let mut dot_product: f32 = 0.0;
    let mut norm_a_sq: f32 = 0.0;
    let mut norm_b_sq: f32 = 0.0;

    for (xa, xb) in a.iter().zip(b.iter()) {
        dot_product += xa * xb;
        norm_a_sq += xa * xa;
        norm_b_sq += xb * xb;
    }

    let norm_product = (norm_a_sq * norm_b_sq).sqrt();

    // 零向量保护:任一模长为 0 时返回 0.0,避免 NaN
    // WHY:NaN 会污染后续 Top-K 排序,导致候选选择异常
    if norm_product == 0.0 {
        return 0.0;
    }

    dot_product / norm_product
}

#[cfg(test)]
mod tests {
    use super::*;

    // === 1. 零向量保护 ===

    #[test]
    fn test_cosine_similarity_zero_vector_a() {
        let a = vec![0.0_f32; 50];
        let b = vec![1.0_f32; 50];
        let score = cosine_similarity(&a, &b);
        assert!((score - 0.0).abs() < 1e-6, "零向量应返回 0.0, got {score}");
    }

    #[test]
    fn test_cosine_similarity_zero_vector_b() {
        let a = vec![1.0_f32; 50];
        let b = vec![0.0_f32; 50];
        let score = cosine_similarity(&a, &b);
        assert!((score - 0.0).abs() < 1e-6, "零向量应返回 0.0, got {score}");
    }

    #[test]
    fn test_cosine_similarity_both_zero_vectors() {
        let a = vec![0.0_f32; 50];
        let b = vec![0.0_f32; 50];
        let score = cosine_similarity(&a, &b);
        assert!(
            (score - 0.0).abs() < 1e-6,
            "双零向量应返回 0.0, got {score}"
        );
    }

    // === 2. 单位向量(完全相同/相反)===

    #[test]
    fn test_cosine_similarity_identical_vectors() {
        let a = vec![1.0_f32, 0.0, 0.0, 0.0];
        let b = vec![1.0_f32, 0.0, 0.0, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!(
            (score - 1.0).abs() < 1e-6,
            "完全相同向量应返回 1.0, got {score}"
        );
    }

    #[test]
    fn test_cosine_similarity_opposite_vectors() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![-1.0_f32, 0.0, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!(
            (score - (-1.0)).abs() < 1e-6,
            "完全相反向量应返回 -1.0, got {score}"
        );
    }

    #[test]
    fn test_cosine_similarity_unit_vectors_45_degrees() {
        // 45 度角:cos(45°) ≈ 0.7071
        let a = vec![1.0_f32, 0.0];
        let b = vec![1.0_f32, 1.0_f32]; // 45° 向量
        let score = cosine_similarity(&a, &b);
        let expected = 1.0_f32 / 2.0_f32.sqrt(); // cos(45°) = 1/√2 ≈ 0.7071
        assert!(
            (score - expected).abs() < 1e-6,
            "45° 角应返回 ≈ 0.7071, got {score}"
        );
    }

    // === 3. 正交向量 ===

    #[test]
    fn test_cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![0.0_f32, 1.0, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!(
            (score - 0.0).abs() < 1e-6,
            "正交向量应返回 0.0, got {score}"
        );
    }

    #[test]
    fn test_cosine_similarity_orthogonal_vectors_high_dim() {
        // 50 维正交向量:a 在前 25 维有值,b 在后 25 维有值
        let mut a = vec![0.0_f32; 50];
        let mut b = vec![0.0_f32; 50];
        a.iter_mut().take(25).for_each(|v| *v = 1.0);
        b.iter_mut().skip(25).for_each(|v| *v = 1.0);
        let score = cosine_similarity(&a, &b);
        assert!(
            (score - 0.0).abs() < 1e-6,
            "高维正交向量应返回 0.0, got {score}"
        );
    }

    // === 4. 长度不匹配 ===

    #[test]
    fn test_cosine_similarity_length_mismatch() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![1.0_f32, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!(
            (score - 0.0).abs() < 1e-6,
            "长度不匹配应返回 0.0, got {score}"
        );
    }

    // === 5. 空向量 ===

    #[test]
    fn test_cosine_similarity_empty_vectors() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        let score = cosine_similarity(&a, &b);
        assert!((score - 0.0).abs() < 1e-6, "空向量应返回 0.0, got {score}");
    }

    // === 6. 相似向量(高相似度)===

    #[test]
    fn test_cosine_similarity_high_similarity() {
        // 两个极为相似的向量(仅微小差异)
        let a: Vec<f32> = (0..50).map(|i| (i as f32) * 0.1).collect();
        let b: Vec<f32> = (0..50).map(|i| (i as f32) * 0.1 + 0.001).collect();
        let score = cosine_similarity(&a, &b);
        assert!(score > 0.99, "相似向量应返回 > 0.99, got {score}");
        assert!(score <= 1.0, "相似度不应超过 1.0");
    }

    // === 7. 50 维标准向量(CSN 默认维度)===

    #[test]
    fn test_cosine_similarity_50_dim_standard() {
        // 模拟 CSN 50 维语义向量场景
        let v1 = vec![0.8_f32; 50];
        let v2 = vec![0.6_f32; 50];
        let score = cosine_similarity(&v1, &v2);
        // 同方向向量,余弦相似度应为 1.0(方向相同,大小不影响)
        assert!(
            (score - 1.0).abs() < 1e-6,
            "同方向 50 维向量应返回 1.0, got {score}"
        );
    }

    #[test]
    fn test_cosine_similarity_50_dim_mixed() {
        // 部分维度同向,部分正交
        let mut v1 = vec![0.0_f32; 50];
        let mut v2 = vec![0.0_f32; 50];
        // 前 25 维同向(v1 = v2 = 1.0)
        v1.iter_mut().take(25).for_each(|v| *v = 1.0);
        v2.iter_mut().take(25).for_each(|v| *v = 1.0);
        // 后 25 维:v1 = 1.0, v2 = 0.0(部分正交)
        v1.iter_mut().skip(25).for_each(|v| *v = 1.0);
        let score = cosine_similarity(&v1, &v2);
        // 期望:dot = 25, |v1| = √50, |v2| = √25 = 5
        // cos = 25 / (√50 * 5) = 25 / (7.071 * 5) = 25 / 35.355 ≈ 0.7071
        let expected = 25.0_f32 / (50.0_f32.sqrt() * 25.0_f32.sqrt());
        assert!(
            (score - expected).abs() < 1e-6,
            "混合向量应返回 ≈ {expected}, got {score}"
        );
    }

    // === 8. 对称性验证 ===

    #[test]
    fn test_cosine_similarity_symmetric() {
        let a = vec![1.0_f32, 0.5, 0.3, 0.2];
        let b = vec![0.4_f32, 0.6, 0.8, 0.1];
        let score_ab = cosine_similarity(&a, &b);
        let score_ba = cosine_similarity(&b, &a);
        assert!(
            (score_ab - score_ba).abs() < 1e-6,
            "余弦相似度应对称, score_ab={score_ab}, score_ba={score_ba}"
        );
    }
}
