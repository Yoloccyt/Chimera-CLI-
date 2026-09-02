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

    /// 构造 one-hot 基向量 `e(index)`:第 `index` 维为 1.0,其余 511 维为 0.0
    ///
    /// # 参数语义(易误读,务必注意)
    /// `index` 是**非零分量的下标**(取值域 `0 .. DIMENSION`),**不是**向量维度。
    /// 返回的向量长度恒为 [`DIMENSION`](Self::DIMENSION)。
    /// 命名取 `basis`(线性代数"基向量")而非 `unit_vector(dim)`,
    /// 就是为了避免 `unit`/`dim` 被误读成"维度可配"。
    ///
    /// # 返回值
    /// - `Some(e)`:下标合法。不变量:`e[index] == 1.0`、非零分量恰有 1 个、
    ///   L2 范数为 1.0、与任意 `j != index` 的基向量余弦相似度为 0.0。
    /// - `None`:`index >= DIMENSION`。
    ///
    /// # WHY 返回 Option 而不是 panic
    /// 该构造器的调用方绝大多数是测试夹具(见下"收敛来源"),它们本就期望
    /// "夹具构造失败 ⇒ 该测试无效"并自行 `expect`。库层若直接 panic,等于把
    /// 一个可静态判定的边界输入升级为进程级失败;返回 `None` 让调用方决定强度。
    ///
    /// # 收敛来源
    /// hcw-window / mlc-engine / osa-coordinator / repo-wiki 四个 crate 曾各自
    /// 复制一份逐字节相同的 `fn unit_clv(dim) -> CLV`(共 11 处),且越界一律 panic。
    /// 收敛到 L1 后,one-hot 语义与越界行为在此处一次性钉死。
    ///
    /// # 示例
    /// ```
    /// use nexus_core::CLV;
    ///
    /// let e7 = CLV::basis(7).expect("7 是合法下标");
    /// assert_eq!(e7.as_slice()[7], 1.0);
    /// assert_eq!(e7.as_slice().iter().filter(|&&v| v != 0.0).count(), 1);
    ///
    /// // 越界下标返回 None,不会 panic
    /// assert!(CLV::basis(CLV::DIMENSION).is_none());
    /// ```
    pub fn basis(index: usize) -> Option<Self> {
        if index >= Self::DIMENSION {
            return None;
        }
        let mut v = vec![0.0_f32; Self::DIMENSION];
        v[index] = 1.0;
        // 长度由上一行构造保证为 DIMENSION,无需再走 from_vec 的维度校验
        Some(Self(Array1::from_vec(v)))
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

    /// 发布 CLV 快照事件(WS-4A)— 供 TUI ClvVector 面板消费
    ///
    /// 构造 [`event_bus::NexusEvent::ClvSnapshotReported`] 并**同步发布**
    /// (`publish_blocking`,sync 模式,无需 tokio runtime)。字段语义:
    /// - `metadata.source = "nexus-core"`;
    /// - `clv_summary` 由 [`event_bus::ClvSummary::from_clv_slice`] 从本向量计算;
    /// - `content_hash` 为向量字节的 SHA-256 十六进制摘要(确定性,供去重/检索)。
    ///
    /// 发布失败(理论罕见)仅告警不 panic。
    pub fn report_snapshot(&self, bus: &event_bus::EventBus) {
        use sha2::Digest;

        let mut hasher = sha2::Sha256::new();
        for &v in self.as_slice() {
            hasher.update(v.to_bits().to_le_bytes());
        }
        let content_hash = hex::encode(hasher.finalize());
        let summary = event_bus::ClvSummary::from_clv_slice(self.as_slice());
        let ev = event_bus::NexusEvent::ClvSnapshotReported {
            metadata: event_bus::EventMetadata::new("nexus-core"),
            modality: "Text".to_string(),
            content_hash,
            clv_summary: summary,
        };
        if let Err(e) = bus.publish_blocking(ev) {
            tracing::warn!(error = %e, "CLV 快照事件发布失败");
        }
    }
}

// 算法定义已下沉至 L0 `nexus_contracts::util`(第四轮冗余收敛 实施-8)。
// WHY 保留本模块 re-export:`CLV::cosine_similarity` 方法、`lib.rs`/`prelude`
// 的 `pub use clv::{cosine_similarity_slices, CLV}` 以及 30+ 处
// `use nexus_core::cosine_similarity_slices` 全部继续有效,对外 API 零破坏;
// 同时让 `nexus_core::clv::cosine_similarity_slices` 这一路径也保持可用。
pub use nexus_contracts::util::cosine_similarity_slices;

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

    /// WS-4A:report_snapshot 发布 ClvSnapshotReported,字段符合契约
    #[test]
    fn report_snapshot_publishes_event() {
        let bus = event_bus::EventBus::new();
        // 订阅必须先于发布(broadcast 反模式 3:错过则静默丢事件)
        let mut rx = bus.subscribe();
        let clv = CLV::from_vec(vec![1.0f32; CLV::DIMENSION]).unwrap();
        clv.report_snapshot(&bus);

        let ev = rx
            .try_recv()
            .expect("try_recv 不应 Err")
            .expect("应收到事件");
        match ev {
            event_bus::NexusEvent::ClvSnapshotReported {
                metadata,
                modality,
                content_hash,
                clv_summary,
            } => {
                assert_eq!(metadata.source, "nexus-core");
                assert_eq!(modality, "Text");
                assert!(!content_hash.is_empty(), "content_hash 必须非空");
                // 全 1 向量:L2 范数 = sqrt(512)
                let expect_norm = (CLV::DIMENSION as f32).sqrt();
                assert!(
                    (clv_summary.l2_norm - expect_norm).abs() < 1e-3,
                    "l2_norm={} 期望 ~{expect_norm}",
                    clv_summary.l2_norm
                );
            }
            other => panic!("期望 ClvSnapshotReported,收到 {other:?}"),
        }
    }

    // ── CLV::basis ───────────────────────────────────────────────────
    //
    // 收敛动机:hcw-window / mlc-engine / osa-coordinator / repo-wiki 四个 crate 的
    // 11 处测试夹具各自复制了一份逐字节相同的 `unit_clv`(one-hot 向量),
    // 且越界下标一律 panic。收敛到 L1 单一权威构造器后,行为在此处一次性钉死。

    #[test]
    fn basis_is_one_hot_at_index() {
        for idx in [0usize, 1, 5, 256, CLV::DIMENSION - 1] {
            let b = CLV::basis(idx).expect("合法下标应返回 Some");
            let s = b.as_slice();
            assert_eq!(s.len(), CLV::DIMENSION, "basis 必须保持 512 维");
            assert_eq!(s[idx], 1.0, "下标 {idx} 处应为 1.0");
            assert_eq!(
                s.iter().filter(|&&v| v != 0.0).count(),
                1,
                "除下标 {idx} 外不得有非零分量"
            );
        }
    }

    #[test]
    fn basis_out_of_range_returns_none_not_panic() {
        assert!(
            CLV::basis(CLV::DIMENSION).is_none(),
            "512 越界(最大合法下标 511)"
        );
        assert!(CLV::basis(CLV::DIMENSION + 1).is_none());
        assert!(CLV::basis(usize::MAX).is_none());
    }

    #[test]
    fn basis_has_unit_l2_norm() {
        let b = CLV::basis(42).unwrap();
        let sq: f32 = b.as_slice().iter().map(|v| v * v).sum();
        assert!(
            (sq - 1.0).abs() < 1e-6,
            "基向量 L2 范数平方应为 1.0,实为 {sq}"
        );
    }

    #[test]
    fn basis_vectors_are_mutually_orthogonal() {
        let e0 = CLV::basis(0).unwrap();
        let e1 = CLV::basis(1).unwrap();
        assert_eq!(e0.cosine_similarity(&e1), 0.0, "不同基向量必须正交");
        assert!(
            (e0.cosine_similarity(&e0) - 1.0).abs() < 1e-6,
            "自身余弦应为 1.0"
        );
        // 零向量边界不受新构造器影响
        assert_eq!(e0.cosine_similarity(&CLV::zero()), 0.0);
    }

    use proptest::prelude::*;

    proptest! {
        /// 契约:index 为任意 usize 都不得 panic,且 Some ⟺ index 在维度内
        #[test]
        fn basis_never_panics_for_any_index(idx in any::<usize>()) {
            let r = CLV::basis(idx);
            prop_assert_eq!(r.is_some(), idx < CLV::DIMENSION);
        }

        /// 合法下标下 one-hot 不变量恒成立(唯一非零 + 该处为 1.0)
        #[test]
        fn basis_is_one_hot(idx in 0usize..CLV::DIMENSION) {
            let b = CLV::basis(idx).unwrap();
            let s = b.as_slice();
            prop_assert_eq!(s.len(), CLV::DIMENSION);
            prop_assert_eq!(s[idx], 1.0);
            prop_assert_eq!(s.iter().filter(|&&v| v != 0.0).count(), 1);
        }
    }
}
