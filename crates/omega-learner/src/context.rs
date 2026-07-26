//! 上下文特征向量 — LinUCB 的输入
//!
//! 对应任务: **P4-W13.1.3**（LinUCB 算法核心）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//!
//! # 设计动机
//!
//! v5.0 设计文档 §7.3 六接缝的上下文均为有限维实数向量:
//! - S1: (任务类型 one-hot, DAG 深度, 内存压力) ~ 8-16 维
//! - S2: (任务阶段 one-hot, 进度指标) ~ 5-10 维
//! - S3: (编辑历史签名, 调用图特征) ~ 12-20 维
//! - S4: (块类型, 访问时序, 错误关联) ~ 6-12 维
//! - S5: (risk_level, 只读性, 历史模式) ~ 4-8 维
//! - S6: (操作类型, 风险信号密度) ~ 3-6 维
//!
//! 本模块封装上下文向量为 `SeamContext`,确保维度一致性并提供归一化校验。
//!
//! # 设计约束
//!
//! - 内部用 `Array1<f32>`(与 CLV 一致,避免 f64 转换开销)
//! - 但 LinUCB 算法内部用 `f64`(精度需求),接口层做转换
//! - 维度 `d` 在构造时固定,运行时不可变(避免矩阵维度不匹配)

use crate::error::{LearnerError, Result};
use ndarray::Array1;
use serde::{Deserialize, Serialize};

/// 上下文特征向量 — LinUCB 输入
///
/// # 设计决策(WHY)
///
/// - **`Array1<f32>` 而非 `Vec<f32>`**: 与 `nexus-core::CLV` 一致,避免转换开销
/// - **`d: usize` 显式存储**: 防止维度漂移,构造后不可变
/// - **`Copy` 未实现**: `Array1` 含堆分配,Copy 语义会浅拷贝,故 Clone 即可
/// - **归一化由调用方负责**: LinUCB 假设上下文有界(`||x|| ≤ 1`),
///   调用方应在构造前归一化,本类型不做强制(避免双重归一化)
///
/// # 示例
///
/// ```
/// use omega_learner::context::SeamContext;
///
/// let ctx = SeamContext::new(vec![0.1, 0.2, 0.3, 0.4]).unwrap();
/// assert_eq!(ctx.dim(), 4);
/// assert!((ctx.as_array()[0] - 0.1).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeamContext {
    /// 特征向量(f32 与 CLV 一致)
    features: Array1<f32>,
}

impl SeamContext {
    /// 创建上下文向量
    ///
    /// # 参数
    /// - `features`: 特征向量(必须非空)
    ///
    /// # 错误
    /// - `InvalidDimension`: features 为空
    pub fn new<F: Into<Vec<f32>>>(features: F) -> Result<Self> {
        let features = features.into();
        if features.is_empty() {
            return Err(LearnerError::InvalidDimension);
        }
        Ok(Self {
            features: Array1::from(features),
        })
    }

    /// 从 Array1 构造上下文
    ///
    /// WHY 提供: 调用方可能已持 `Array1<f32>`(如 CLV),避免 Vec 中转
    pub fn from_array(features: Array1<f32>) -> Result<Self> {
        if features.is_empty() {
            return Err(LearnerError::InvalidDimension);
        }
        Ok(Self { features })
    }

    /// 返回上下文维度 d
    pub fn dim(&self) -> usize {
        self.features.len()
    }

    /// 返回 f32 数组视图(供外部消费,如序列化)
    pub fn as_array(&self) -> &Array1<f32> {
        &self.features
    }

    /// 返回 f64 数组(供 LinUCB 内部使用,精度转换)
    ///
    /// WHY 转换 f64: LinUCB 矩阵运算需要 f64 精度(f32 累加易漂移),
    /// 上下文向量通常 ≤ 20 维,转换开销可忽略(< 100ns)。
    /// 每次调用都新建 Array1<f64> 避免持有转换缓存(简化生命周期)。
    pub fn as_f64_array(&self) -> Array1<f64> {
        self.features.mapv(|x| x as f64)
    }

    /// 消费并返回内部 Array1
    pub fn into_inner(self) -> Array1<f32> {
        self.features
    }

    /// 返回索引位置的元素(便于调试)
    pub fn get(&self, idx: usize) -> Option<f32> {
        self.features.get(idx).copied()
    }

    /// 计算向量 L2 范数(归一化校验用)
    ///
    /// WHY 提供此方法: 调用方在构造前可校验 `||x|| ≤ 1`,
    /// LinUCB regret 上界假设上下文有界。
    pub fn l2_norm(&self) -> f64 {
        let sum_sq: f64 = self.features.iter().map(|&x| (x as f64).powi(2)).sum();
        sum_sq.sqrt()
    }

    /// 校验上下文是否归一化(L2 范数 ≤ 1.0 + 容忍度)
    ///
    /// WHY: LinUCB regret 上界证明假设 `||x||_2 ≤ 1`,
    /// 调用方应在归一化后调用 `is_normalized()` 校验,避免模型退化。
    /// 容忍度 1e-3 防止浮点漂移误报。
    pub fn is_normalized(&self) -> bool {
        let norm = self.l2_norm();
        norm <= 1.0 + 1e-3
    }
}

impl PartialEq for SeamContext {
    /// 相等性比较(逐元素 f32 比较,精确匹配)
    ///
    /// WHY 手动实现而非派生: 测试断言需要精确比较,但生产代码应避免依赖相等性
    /// (浮点比较应使用近似比较,如 `abs_diff_eq`)
    fn eq(&self, other: &Self) -> bool {
        self.features == other.features
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // SeamContext 构造与基础测试
    // ============================================================

    #[test]
    fn test_context_new_basic() {
        let ctx = SeamContext::new(vec![0.1, 0.2, 0.3, 0.4]).unwrap();
        assert_eq!(ctx.dim(), 4);
    }

    #[test]
    fn test_context_new_single_dim() {
        let ctx = SeamContext::new(vec![0.5]).unwrap();
        assert_eq!(ctx.dim(), 1);
    }

    #[test]
    fn test_context_new_empty_fails() {
        let result = SeamContext::new(vec![]);
        assert!(matches!(result, Err(LearnerError::InvalidDimension)));
    }

    #[test]
    fn test_context_from_array() {
        let arr = Array1::from(vec![0.1, 0.2, 0.3]);
        let ctx = SeamContext::from_array(arr).unwrap();
        assert_eq!(ctx.dim(), 3);
    }

    #[test]
    fn test_context_from_array_empty_fails() {
        let arr = Array1::zeros(0);
        let result = SeamContext::from_array(arr);
        assert!(matches!(result, Err(LearnerError::InvalidDimension)));
    }

    // ============================================================
    // 访问与转换测试
    // ============================================================

    #[test]
    fn test_context_as_array() {
        let ctx = SeamContext::new(vec![0.1, 0.2, 0.3]).unwrap();
        let arr = ctx.as_array();
        assert_eq!(arr.len(), 3);
        assert!((arr[0] - 0.1).abs() < 1e-6);
        assert!((arr[1] - 0.2).abs() < 1e-6);
        assert!((arr[2] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_context_as_f64_array() {
        let ctx = SeamContext::new(vec![0.1, 0.2, 0.3]).unwrap();
        let f64_arr = ctx.as_f64_array();
        assert_eq!(f64_arr.len(), 3);
        assert!((f64_arr[0] - 0.1).abs() < 1e-6);
        assert!((f64_arr[1] - 0.2).abs() < 1e-6);
        assert!((f64_arr[2] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_context_into_inner() {
        let ctx = SeamContext::new(vec![0.1, 0.2]).unwrap();
        let arr = ctx.into_inner();
        assert_eq!(arr.len(), 2);
        assert!((arr[0] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn test_context_get() {
        let ctx = SeamContext::new(vec![0.1, 0.2, 0.3]).unwrap();
        assert_eq!(ctx.get(0), Some(0.1));
        assert_eq!(ctx.get(2), Some(0.3));
        assert_eq!(ctx.get(3), None);
    }

    // ============================================================
    // 范数与归一化测试
    // ============================================================

    #[test]
    fn test_context_l2_norm_unit_vector() {
        let ctx = SeamContext::new(vec![1.0, 0.0, 0.0]).unwrap();
        let norm = ctx.l2_norm();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_context_l2_norm_general() {
        let ctx = SeamContext::new(vec![3.0, 4.0]).unwrap();
        let norm = ctx.l2_norm();
        assert!((norm - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_context_l2_norm_zero_vector() {
        let ctx = SeamContext::new(vec![0.0, 0.0, 0.0]).unwrap();
        let norm = ctx.l2_norm();
        assert!((norm - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_context_is_normalized_true() {
        let ctx = SeamContext::new(vec![1.0, 0.0, 0.0]).unwrap();
        assert!(ctx.is_normalized());
    }

    #[test]
    fn test_context_is_normalized_false() {
        let ctx = SeamContext::new(vec![3.0, 4.0]).unwrap();
        assert!(!ctx.is_normalized()); // norm = 5.0
    }

    #[test]
    fn test_context_is_normalized_tolerance() {
        // 1.0001 接近 1.0,误差 < 1e-3 容忍
        let ctx = SeamContext::new(vec![1.0001, 0.0]).unwrap();
        assert!(ctx.is_normalized());
    }

    // ============================================================
    // 相等性与序列化测试
    // ============================================================

    #[test]
    fn test_context_equality() {
        let ctx1 = SeamContext::new(vec![0.1, 0.2, 0.3]).unwrap();
        let ctx2 = SeamContext::new(vec![0.1, 0.2, 0.3]).unwrap();
        let ctx3 = SeamContext::new(vec![0.1, 0.2, 0.4]).unwrap();
        assert_eq!(ctx1, ctx2);
        assert_ne!(ctx1, ctx3);
    }

    #[test]
    fn test_context_clone() {
        let ctx1 = SeamContext::new(vec![0.1, 0.2, 0.3]).unwrap();
        let ctx2 = ctx1.clone();
        assert_eq!(ctx1, ctx2);
    }

    #[test]
    fn test_context_serialize_json() {
        let ctx = SeamContext::new(vec![0.1, 0.2, 0.3]).unwrap();
        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: SeamContext = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, deserialized);
    }
}
