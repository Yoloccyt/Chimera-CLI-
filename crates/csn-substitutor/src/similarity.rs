//! 余弦相似度计算 — 委托至 nexus-core 权威实现
//!
//! 对应架构层:L10 Interface
//!
//! ## 设计要点
//! - 统一使用 `nexus_core::cosine_similarity_slices` 权威实现,避免多副本优化不一致
//! - 零向量保护、长度不匹配保护、clamp 行为均由权威实现保证
//!
//! ## 公式
//! cos(a, b) = (a · b) / (||a|| * ||b||)

// 统一使用 nexus-core 权威实现,避免多副本优化不一致
use nexus_core::cosine_similarity_slices;

/// 计算两个向量的余弦相似度
///
/// # 参数
/// - `a`:向量 A(任意维度)
/// - `b`:向量 B(任意维度)
///
/// # 返回
/// 余弦相似度得分,范围 [-1.0, 1.0]。
///
/// # 边界处理
/// - 任一向量为零向量(模长为 0):返回 `0.0`(避免除零导致 NaN)
/// - 空向量:返回 `0.0`
///
/// # 实现说明
/// 委托至 `nexus_core::cosine_similarity_slices` 权威实现,行为完全一致。
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    cosine_similarity_slices(a, b)
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

    // === 4. 长度不匹配(权威实现取 min 长度计算) ===

    #[test]
    fn test_cosine_similarity_length_mismatch() {
        // 权威实现取 min 长度计算:a=[1.0, 0.0, 0.0], b=[1.0, 0.0]
        // 仅用前 2 个元素:dot=1.0, norm_a=1.0, norm_b=1.0 → 1.0
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![1.0_f32, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!(
            (score - 1.0).abs() < 1e-6,
            "不等长输入取 min 长度,预期 1.0, got {score}"
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
