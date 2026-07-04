//! CLV(Context Latent Vector)— 512 维潜在语言向量
//!
//! 对应架构层:L1 Core
//! 对应创新点:CLV — 所有上下文、记忆、意图的统一潜在表示
//!
//! # 设计决策(WHY)
//! - **512 维**:平衡表达力与计算成本,与主流嵌入模型(MiniLM、BGE)对齐
//! - **ndarray::Array1**:提供零成本向量运算(dot/sqrt),优于 `Vec<f32>` 手写循环
//! - **零向量边界**:cosine_similarity 对零向量返回 0.0,避免除零 panic
//!
//! # 使用场景
//! - NMC 编码:UserIntent → CLV
//! - 语义路由:KVBSR/FaaE 按 CLV 余弦相似度路由
//! - 记忆检索:MLC 按 CLV 相似度召回

use ndarray::Array1;
use serde::{Deserialize, Serialize};

use crate::error::NexusError;

/// CLV — 512 维 f32 潜在向量,NEXUS-OMEGA 的统一语义表示
///
/// 所有实例维度严格为 512,通过 `zero()` 或 `from_vec()` 构造。
/// `from_vec()` 做维度校验,防止外部输入构造错误维度向量。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CLV(Array1<f32>);

impl CLV {
    /// CLV 固定维度:512
    pub const DIMENSION: usize = 512;

    /// 创建零向量 — 所有维度为 0.0
    pub fn zero() -> Self {
        Self(Array1::zeros(Self::DIMENSION))
    }

    /// 从 `Vec<f32>` 构造 CLV — 维度必须为 512
    ///
    /// # 错误
    /// - `InvalidClvDimension`:传入向量长度不等于 512
    pub fn from_vec(v: Vec<f32>) -> Result<Self, NexusError> {
        if v.len() != Self::DIMENSION {
            return Err(NexusError::InvalidClvDimension {
                expected: Self::DIMENSION,
                actual: v.len(),
            });
        }
        Ok(Self(Array1::from_vec(v)))
    }

    /// 计算与另一个 CLV 的余弦相似度
    ///
    /// 公式:dot(a, b) / (|a| * |b|)
    ///
    /// # 零向量边界
    /// 若任一向量为零向量(|a|==0 或 |b|==0),返回 0.0 而非 NaN。
    /// WHY:零向量无方向,余弦相似度无定义;返回 0.0 表示"无相似性",
    /// 避免下游 NaN 污染(如路由评分 NaN 导致排序异常)。
    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        let dot = self.0.dot(&other.0);
        let norm_self = self.0.dot(&self.0).sqrt();
        let norm_other = other.0.dot(&other.0).sqrt();

        if norm_self == 0.0 || norm_other == 0.0 {
            return 0.0;
        }

        dot / (norm_self * norm_other)
    }

    /// 返回 CLV 固定维度(512)
    pub fn dimension() -> usize {
        Self::DIMENSION
    }

    /// 只读访问内部 f32 切片
    ///
    /// WHY:Array1::from_vec/zeros 产生的数组总是连续内存布局,
    /// as_slice() 必返回 Some。用 unwrap_or(&[]) 作为不可能 None 的防御。
    pub fn as_slice(&self) -> &[f32] {
        self.0.as_slice().unwrap_or(&[])
    }
}

/// 计算两个 f32 切片的余弦相似度(自由函数,供上层 crate 共享)
///
/// 公式:dot(a, b) / (|a| * |b|)
///
/// # 返回值
/// 返回值 ∈ [-1.0, 1.0],通过 `clamp` 钳制浮点误差导致的微小越界。
///
/// # 零向量处理
/// 若任一向量为零向量(|a|==0 或 |b|==0),返回 0.0 而非 NaN。
/// WHY 统一零向量处理:避免不同 crate 返回 NaN 导致下游计算异常
/// (如路由评分 NaN 导致排序异常)。
///
/// # 不等长输入
/// 取两个切片的最小长度计算(兼容不等长输入,最安全)。
/// 调用方若需严格等长校验,应在调用前自行断言。
///
/// # 设计决策(WHY)
/// SubTask 21.4 — mlc-engine(types.rs)、kvbsr-router(blocks.rs)、
/// repo-wiki(vector.rs)三处重复实现余弦相似度,且零向量处理策略不一致。
/// 提取到 L1 Core 统一行为,消除约 80 行重复代码。
///
/// # 示例
/// ```
/// use nexus_core::cosine_similarity_slices;
///
/// // 相同向量余弦相似度为 1.0
/// let v = vec![1.0, 2.0, 3.0];
/// let sim = cosine_similarity_slices(&v, &v);
/// assert!((sim - 1.0).abs() < 1e-5);
///
/// // 零向量返回 0.0(非 NaN)
/// let zero = vec![0.0, 0.0, 0.0];
/// assert_eq!(cosine_similarity_slices(&zero, &v), 0.0);
/// ```
pub fn cosine_similarity_slices(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let mut dot: f32 = 0.0;
    let mut norm_a: f32 = 0.0;
    let mut norm_b: f32 = 0.0;
    for i in 0..len {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let norm_a = norm_a.sqrt();
    let norm_b = norm_b.sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_dimension() {
        let clv = CLV::zero();
        assert_eq!(clv.as_slice().len(), CLV::DIMENSION);
        assert_eq!(CLV::dimension(), 512);
        assert!(clv.as_slice().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_from_vec_valid() {
        let v = vec![0.5_f32; CLV::DIMENSION];
        let clv = CLV::from_vec(v).unwrap();
        assert_eq!(clv.as_slice().len(), 512);
        assert!(clv.as_slice().iter().all(|&v| v == 0.5));
    }

    #[test]
    fn test_from_vec_invalid_dimension() {
        let v = vec![0.0_f32; 256];
        let result = CLV::from_vec(v);
        assert!(matches!(
            result,
            Err(NexusError::InvalidClvDimension {
                expected: 512,
                actual: 256
            })
        ));
    }

    #[test]
    fn test_cosine_similarity_identical_vectors() {
        let v = vec![1.0_f32; CLV::DIMENSION];
        let clv = CLV::from_vec(v).unwrap();
        let sim = clv.cosine_similarity(&clv);
        // 浮点误差容忍:相同向量余弦相似度应接近 1.0
        assert!((sim - 1.0).abs() < 1e-5, "expected ~1.0, got {sim}");
    }

    #[test]
    fn test_cosine_similarity_orthogonal_vectors() {
        // 构造正交向量:前半非零 vs 后半非零
        let mut v1 = vec![0.0_f32; CLV::DIMENSION];
        let mut v2 = vec![0.0_f32; CLV::DIMENSION];
        for i in 0..256 {
            v1[i] = 1.0;
            v2[256 + i] = 1.0;
        }
        let clv1 = CLV::from_vec(v1).unwrap();
        let clv2 = CLV::from_vec(v2).unwrap();
        let sim = clv1.cosine_similarity(&clv2);
        assert!(sim.abs() < 1e-6, "expected ~0.0, got {sim}");
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let zero = CLV::zero();
        let mut v = vec![1.0_f32; CLV::DIMENSION];
        v[0] = 2.0;
        let nonzero = CLV::from_vec(v).unwrap();

        // 零向量与任意向量:返回 0.0(非 NaN)
        let sim1 = zero.cosine_similarity(&nonzero);
        assert_eq!(sim1, 0.0);

        // 零向量与零向量:返回 0.0
        let sim2 = zero.cosine_similarity(&zero);
        assert_eq!(sim2, 0.0);
    }

    #[test]
    fn test_clv_serde_roundtrip() {
        let mut v = vec![0.0_f32; CLV::DIMENSION];
        for (i, val) in v.iter_mut().enumerate() {
            *val = i as f32 * 0.1;
        }
        let original = CLV::from_vec(v).unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let restored: CLV = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);
    }
}
