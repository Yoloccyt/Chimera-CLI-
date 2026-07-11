//! CLV 投影器 — 将 512-dim CLV 投影到 block_vector_dim(默认 64)
//!
//! 对应架构层:L6 Router
//! 对应创新点:KVBSR PCA 降维(P1-6)
//!
//! # 核心职责
//! - `ClvProjector`:将 512-dim CLV 投影到 N-dim(默认 64)
//! - `ProjectionMethod::Truncate`:简单截取前 N 维(原实现,零成本)
//! - `ProjectionMethod::Pca`:主成分分析投影(数据驱动,区分度更高)
//! - `ProjectionMethod::RandomProjection`:随机投影(快速,无需训练)
//!
//! # 设计决策(WHY)
//! - **PCA 替代截取**:截取前 N 维假设前 N 维最重要,但 CLV 的 512 维是
//!   神经网络嵌入的输出,各维度重要性不均匀且非前 N 维必然最优。
//!   PCA 通过数据驱动找到最大方差方向,投影后区分度更高。
//! - **随机投影作为冷启动**:无需训练数据,即时可用,适合系统初始化。
//! - **可配置**:通过 `KvbsrConfig::projection_method` 切换,支持 A/B 测试。
//! - **纯 Rust 实现**:不依赖外部 BLAS,512×64 矩阵乘法计算量小
//!   (32768 次乘加),Rust 层面完全高效。

use ndarray::{Array1, Array2};
use nexus_core::CLV;
use serde::{Deserialize, Serialize};

/// CLV 投影方法
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProjectionMethod {
    /// 简单截取前 N 维 — 零成本,确定性高
    #[default]
    Truncate,
    /// 主成分分析投影 — 数据驱动,区分度最高
    Pca,
    /// 随机投影 — 快速,无需训练
    RandomProjection,
}

/// CLV 投影器 — 将 512-dim CLV 投影到目标维度
///
/// # 使用方式
/// ```
/// # use kvbsr_router::clv_projector::{ClvProjector, ProjectionMethod};
/// # use nexus_core::CLV;
/// // 创建截断投影器(零成本)
/// let projector = ClvProjector::new_truncate(64);
///
/// // 投影 CLV
/// let clv = CLV::zero();
/// let projected = projector.project(&clv);
/// assert_eq!(projected.len(), 64);
/// ```
#[derive(Debug, Clone)]
pub struct ClvProjector {
    /// 投影矩阵:original_dim × target_dim (PCA/RandomProjection 用)
    projection_matrix: Option<Array2<f32>>,
    /// 均值向量(PCA 中心化处理用)
    mean: Option<Array1<f32>>,
    /// 目标维度
    target_dim: usize,
    /// 投影方法
    method: ProjectionMethod,
}

impl ClvProjector {
    /// 创建截断投影器 — 零成本,取前 target_dim 维
    pub fn new_truncate(target_dim: usize) -> Self {
        Self {
            projection_matrix: None,
            mean: None,
            target_dim,
            method: ProjectionMethod::Truncate,
        }
    }

    /// 创建随机投影器 — 无需训练,即时可用
    ///
    /// 投影矩阵元素从 N(0, 1/√target_dim) 采样。
    pub fn new_random_projection(target_dim: usize) -> Self {
        let matrix = Self::generate_random_matrix(target_dim);
        Self {
            projection_matrix: Some(matrix),
            mean: None,
            target_dim,
            method: ProjectionMethod::RandomProjection,
        }
    }

    /// 从预训练 PCA 矩阵创建投影器
    ///
    /// # 参数
    /// - `projection_matrix`:512 × target_dim 的投影矩阵
    /// - `mean`:512-dim 均值向量(中心化处理用)
    /// - `target_dim`:目标维度
    pub fn from_pca_matrix(
        projection_matrix: Array2<f32>,
        mean: Array1<f32>,
        target_dim: usize,
    ) -> Result<Self, String> {
        let shape = projection_matrix.shape();
        if shape.len() != 2 || shape[1] != target_dim {
            return Err(format!(
                "投影矩阵列数应为 {},实际为 {:?}",
                target_dim, shape
            ));
        }
        if mean.len() != CLV::DIMENSION {
            return Err(format!(
                "均值向量长度应为 {},实际为 {}",
                CLV::DIMENSION,
                mean.len()
            ));
        }
        Ok(Self {
            projection_matrix: Some(projection_matrix),
            mean: Some(mean),
            target_dim,
            method: ProjectionMethod::Pca,
        })
    }

    /// 投影 CLV 到目标维度
    ///
    /// 根据 method 选择投影方式:
    /// - `Truncate`:截取前 target_dim 维
    /// - `Pca`/`RandomProjection`:矩阵乘法投影
    pub fn project(&self, clv: &CLV) -> Vec<f32> {
        match self.method {
            ProjectionMethod::Truncate => self.project_truncate(clv),
            ProjectionMethod::Pca | ProjectionMethod::RandomProjection => self.project_matrix(clv),
        }
    }

    /// 截断投影 — 取前 target_dim 维
    fn project_truncate(&self, clv: &CLV) -> Vec<f32> {
        let slice = clv.as_slice();
        slice[..self.target_dim.min(slice.len())].to_vec()
    }

    /// 矩阵投影 — projection_matrix^T × (clv - mean)
    fn project_matrix(&self, clv: &CLV) -> Vec<f32> {
        let original = Array1::from_vec(clv.as_slice().to_vec());
        let centered = if let Some(ref mean) = self.mean {
            &original - mean
        } else {
            original
        };

        if let Some(ref matrix) = self.projection_matrix {
            let projected = matrix.t().dot(&centered);
            projected.to_vec()
        } else {
            // 无投影矩阵,降级到截断
            self.project_truncate(clv)
        }
    }

    /// 从样本数据训练 PCA 投影矩阵
    ///
    /// 流程:
    /// 1. 计算样本均值
    /// 2. 中心化处理
    /// 3. 计算协方差矩阵
    /// 4. 幂迭代法求前 target_dim 个特征向量
    /// 5. 更新投影矩阵和 mean
    ///
    /// # 参数
    /// - `samples`:CLV 样本列表(每个 512-dim)
    pub fn train_pca(&mut self, samples: &[CLV]) {
        if samples.len() < self.target_dim {
            tracing::warn!(
                "PCA 训练样本不足:需要至少 {} 个,实际 {} 个,保持当前方法",
                self.target_dim,
                samples.len()
            );
            return;
        }

        // 1. 计算均值
        let mut mean = Array1::zeros(CLV::DIMENSION);
        for sample in samples {
            let slice = sample.as_slice();
            for i in 0..CLV::DIMENSION {
                mean[i] += slice[i];
            }
        }
        mean /= samples.len() as f32;

        // 2. 中心化数据矩阵
        let mut data = Array2::zeros((samples.len(), CLV::DIMENSION));
        for (row_idx, sample) in samples.iter().enumerate() {
            let slice = sample.as_slice();
            for col_idx in 0..CLV::DIMENSION {
                data[[row_idx, col_idx]] = slice[col_idx] - mean[col_idx];
            }
        }

        // 3. 协方差矩阵
        let cov = data.t().dot(&data) / (samples.len() - 1).max(1) as f32;

        // 4. 幂迭代求特征向量
        let eigenvectors = Self::power_iteration_top_k(&cov, self.target_dim, 100);

        // 5. 更新状态
        self.projection_matrix = Some(eigenvectors);
        self.mean = Some(mean);
        self.method = ProjectionMethod::Pca;
    }

    /// 返回当前投影方法
    pub fn method(&self) -> ProjectionMethod {
        self.method
    }

    /// 返回目标维度
    pub fn target_dim(&self) -> usize {
        self.target_dim
    }

    /// 生成随机投影矩阵
    fn generate_random_matrix(target_dim: usize) -> Array2<f32> {
        let mut matrix = Array2::zeros((CLV::DIMENSION, target_dim));
        let mut seed: u64 = 0x9e3779b97f4a7c15;

        for col in 0..target_dim {
            for row in 0..CLV::DIMENSION {
                seed = seed.wrapping_add(0x9e3779b97f4a7c15);

                // 中心极限定理近似正态分布
                let mut normal: f32 = 0.0;
                for _ in 0..12 {
                    seed = seed.wrapping_add(0x9e3779b97f4a7c15);
                    let mut w = seed;
                    w = (w ^ (w >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                    w = (w ^ (w >> 27)).wrapping_mul(0x94d049bb133111eb);
                    w = w ^ (w >> 31);
                    normal += ((w as f64) / (u64::MAX as f64)) as f32;
                }
                normal -= 6.0;

                let scale = 1.0 / (target_dim as f32).sqrt();
                matrix[[row, col]] = normal * scale;
            }
        }

        matrix
    }

    /// 幂迭代法求前 k 个特征向量
    fn power_iteration_top_k(matrix: &Array2<f32>, k: usize, max_iterations: usize) -> Array2<f32> {
        let n = matrix.shape()[0];
        let mut eigenvectors = Array2::zeros((n, k));

        for eigen_idx in 0..k {
            let mut v = Array1::zeros(n);
            let mut seed: u64 = 0x123456789abcdef0u64.wrapping_add(eigen_idx as u64);
            for i in 0..n {
                seed = seed
                    .wrapping_mul(0x5851f42d4c957f2d)
                    .wrapping_add(0x14057b7ef767814f);
                v[i] = ((seed as f64) / (u64::MAX as f64)) as f32 - 0.5;
            }

            let norm = v.dot(&v).sqrt();
            if norm > 0.0 {
                v /= norm;
            }

            for _ in 0..max_iterations {
                let mut v_new = matrix.dot(&v);

                // Gram-Schmidt 正交化
                for prev_idx in 0..eigen_idx {
                    let u = eigenvectors.column(prev_idx).to_owned();
                    let proj = v_new.dot(&u);
                    v_new = &v_new - &(proj * &u);
                }

                let new_norm = v_new.dot(&v_new).sqrt();
                if new_norm < 1e-10 {
                    break;
                }
                v = &v_new / new_norm;
            }

            for i in 0..n {
                eigenvectors[[i, eigen_idx]] = v[i];
            }
        }

        eigenvectors
    }
}

impl Default for ClvProjector {
    fn default() -> Self {
        Self::new_truncate(64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_projection() {
        let projector = ClvProjector::new_truncate(64);
        let clv = CLV::from_vec(vec![1.0_f32; CLV::DIMENSION]).unwrap();
        let projected = projector.project(&clv);
        assert_eq!(projected.len(), 64);
        assert!(projected.iter().all(|&v| (v - 1.0).abs() < 1e-6));
    }

    #[test]
    fn test_random_projection_dimensions() {
        let projector = ClvProjector::new_random_projection(64);
        let clv = CLV::from_vec(vec![1.0_f32; CLV::DIMENSION]).unwrap();
        let projected = projector.project(&clv);
        assert_eq!(projected.len(), 64);
        assert_eq!(projector.method(), ProjectionMethod::RandomProjection);
    }

    #[test]
    fn test_pca_training() {
        // 生成 200 个样本
        let mut samples = Vec::new();
        for i in 0..200 {
            let mut v = vec![0.0_f32; CLV::DIMENSION];
            for (j, val) in v.iter_mut().enumerate().take(64) {
                *val = (i as f32 * 0.01 + j as f32 * 0.1).sin();
            }
            samples.push(CLV::from_vec(v).unwrap());
        }

        let mut projector = ClvProjector::new_truncate(64);
        projector.train_pca(&samples);

        assert_eq!(projector.method(), ProjectionMethod::Pca);

        let clv = CLV::from_vec(vec![1.0_f32; CLV::DIMENSION]).unwrap();
        let projected = projector.project(&clv);
        assert_eq!(projected.len(), 64);
    }

    #[test]
    fn test_zero_vector_projection() {
        let projector = ClvProjector::new_random_projection(64);
        let clv = CLV::zero();
        let projected = projector.project(&clv);
        assert_eq!(projected.len(), 64);
    }

    #[test]
    fn test_similarity_preservation() {
        // 构造两个相似向量
        let mut v1 = vec![0.0_f32; CLV::DIMENSION];
        let mut v2 = vec![0.0_f32; CLV::DIMENSION];
        for i in 0..256 {
            v1[i] = 1.0;
            v2[i] = 0.9;
        }
        let clv1 = CLV::from_vec(v1).unwrap();
        let clv2 = CLV::from_vec(v2).unwrap();

        let projector = ClvProjector::new_random_projection(64);
        let p1 = projector.project(&clv1);
        let p2 = projector.project(&clv2);

        // 投影后仍应保持相似(内积同号)
        let dot: f32 = p1.iter().zip(&p2).map(|(a, b)| a * b).sum();
        assert!(dot > 0.0, "相似向量投影后内积应为正,实际 {}", dot);
    }
}
