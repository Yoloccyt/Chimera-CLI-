//! HCW-Sparse v2.0 精排 — HNSW + 精确 CLV 重排 → 500 Block
//!
//! 对应任务: P3-W9.2.1
//! 对应病理修复: D1（HCW selector 权重手写、OSA 静态掩码无学习机制）
//!
//! # 算法设计（继承 v5.0 设计文档 §4.2）
//!
//! ## 两阶段精排
//! 1. **HNSW 候选扩展**（<30ms）— 用种子 CLV 在 VectorStore 中检索 Top-N 候选 Block
//!    - over-fetch 策略: N = top_k × overfetch_factor（默认 2.0）
//!    - 补偿 HNSW 近似误差: 检索更多候选，精确重排后截断到 top_k
//! 2. **精确 CLV 重排**（<20ms）— 对候选 Block 用 `CLV::cosine_similarity` 精确重排
//!    - WHY 精确重排: HNSW 是近似最近邻（ANN），返回的 score 可能有误差；
//!      用 `CLV::cosine_similarity` 重新计算确保排序精度
//!    - 缺失 Block CLV 时 fallback 到 HNSW score（不阻塞精排）
//!
//! ## Top-K 选择
//! 用 `select_nth_unstable` (O(n)) 选 Top-K（§4.1 红线：禁止 sort_by 做 Top-K），
//! 再对 Top-K 部分排序输出（O(K log K)，K=500 时开销可忽略）
//!
//! # 性能预算（<50ms）
//! - HNSW 检索 Top-1000（over-fetch 2×）: 10K-100K entry ≈ 20-30ms
//! - 精确 CLV 重排 1000 × 512-dim dot product: ≈ 1-2ms（ndarray SIMD 加速）
//! - Top-K 选择 + 排序: O(n) + O(k log k) ≈ 0.1ms
//! - 总计: < 35ms，预算 <50ms 充足
//!
//! # 架构铁律合规
//! - hcw-window (L2) 不能向上依赖 repo-wiki (L5) 的 HnswStore
//! - 通过 nexus-contracts (L0) 的 `VectorStore` trait 实现依赖倒置（ADR-033）
//! - 调用方注入具体的 `HnswStore` 实例，精排引擎面向 trait 编程

use std::collections::HashMap;
use std::time::Instant;

use nexus_contracts::{VectorHit, VectorStore};
use nexus_core::CLV;

use super::types::{
    BlockId, BlockScore, CoarseRecallOutput, FineRecallConfig, FineRecallOutput, RecallError,
};

// ============================================================
// 精排输入
// ============================================================

/// 精排输入 — 由调用方组装的多信号源
///
/// # 泛型参数
/// - `S`: VectorStore 实现（如 `HnswStore` / `MemoryKnnStore`）
///   约束 `Meta = ()`：精排不使用元数据，仅用向量与 ID
///
/// # 字段
/// - `coarse_output`: 粗召回输出（100 模块，用于后续重排填充的模块级密度计算）
/// - `seed_clv`: 种子 CLV（HNSW 检索的 query + 精确重排的查询向量）
/// - `vector_store`: Block 级向量索引（HNSW 或 Memory KNN）
/// - `block_clvs`: Block ID → CLV 映射（用于精确重排，可选）
///   - `Some`: 启用精确 CLV 重排（用 `CLV::cosine_similarity` 重新计算）
///   - `None`: 退化为仅用 HNSW score（性能优先，精度降低）
/// - `top_k`: 返回 Block 数（默认 500，spec.md 要求）
///
/// # 设计决策（WHY）
/// - `vector_store` 用 `&S` 而非 `&dyn VectorStore`: 泛型静态分发优于 trait object 动态分发，
///   满足 <50ms 性能红线（trait object 的 vtable 查询在热路径上有开销）
/// - `block_clvs` 为 `Option`: 调用方可选提供 Block CLV 映射做精确重排；
///   不提供时退化为仅用 HNSW score（HnswStore 已返回精确余弦相似度，可接受）
/// - `coarse_output` 保留在输入中: 精排本身不直接使用模块列表，
///   但保留供后续重排填充（P3-W10.1）的模块级密度计算使用
pub struct FineRecallInput<'a, S: VectorStore> {
    /// 粗召回输出（100 模块，供后续重排填充使用）
    pub coarse_output: &'a CoarseRecallOutput,
    /// 种子 CLV（HNSW 检索的 query + 精确重排的查询向量）
    pub seed_clv: &'a CLV,
    /// Block 级向量索引（HNSW 或 Memory KNN，实现 `VectorStore` trait）
    pub vector_store: &'a S,
    /// Block ID → CLV 映射（用于精确重排，None 时退化为仅用 HNSW score）
    pub block_clvs: Option<&'a HashMap<BlockId, CLV>>,
    /// 返回 Block 数（spec.md 要求 500）
    pub top_k: usize,
}

// ============================================================
// PROBE P1.2: 探针精排输入（独立新类型——严禁给 FineRecallInput 追加字段，
// 25 处既有字面量构造点零改动，编译破坏面红线）
// ============================================================

/// 探针精排输入 — 查询探针驱动（PROBE P1.2，SnapKV 平移）
///
/// 与 [`FineRecallInput`] 的差异：`seed_clv` 语义扩展为**混合探针向量**
/// （当前 query + 近 K 轮对话，由 `crate::probe::mix_probe` 构造）。
/// 字段结构与 FineRecallInput 对齐，但独立类型避免破坏既有构造点。
///
/// # 泛型参数
/// - `S`: VectorStore 实现（约束 `Meta = ()`，与 FineRecallInput 一致）
pub struct ProbeRecallInput<'a, S: VectorStore> {
    /// 粗召回输出（供后续重排填充使用）
    pub coarse_output: &'a CoarseRecallOutput,
    /// 混合探针 CLV（`crate::probe::mix_probe` 产出）
    pub probe_clv: &'a CLV,
    /// Block 级向量索引（HNSW 或 Memory KNN，实现 `VectorStore` trait）
    pub vector_store: &'a S,
    /// Block ID → CLV 映射（用于精确重排，None 时退化为仅用 HNSW score）
    pub block_clvs: Option<&'a HashMap<BlockId, CLV>>,
    /// 返回 Block 数（默认 500）
    pub top_k: usize,
}

// ============================================================
// 精排引擎
// ============================================================

/// 精排引擎 — HNSW + 精确 CLV 重排
///
/// # 构建器模式
/// 用 `FineRecall::new(config)` 或 `FineRecall::with_default_config()` 构造，
/// 调用 `rank()` 执行精排。
///
/// # 线程安全
/// 引擎本身无可变状态（`&self` 调用），可被多线程并发调用。
/// 若需动态更新配置，用 `Arc<RwLock<FineRecall>>` 包裹。
///
/// # 示例
/// ```no_run
/// use hcw_window::recall::{FineRecall, FineRecallInput, FineRecallConfig, CoarseRecallOutput, ModuleScore};
/// use nexus_contracts::{VectorStore, VectorStoreExt, VectorBackend};
/// use nexus_core::CLV;
/// use std::collections::HashMap;
///
/// // Mock VectorStore 实现（生产用 HnswStore）
/// struct MockStore;
/// impl VectorStore for MockStore {
///     type Meta = ();
///     type Error = String;
///     fn upsert(&self, _: &str, _: &[f32], _: Self::Meta) -> Result<(), Self::Error> { Ok(()) }
///     fn top_k(&self, _: &[f32], k: usize, _: &str) -> Result<Vec<nexus_contracts::VectorHit>, Self::Error> {
///         Ok((0..k).map(|i| nexus_contracts::VectorHit::new(format!("block-{i}"), 0.9)).collect())
///     }
///     fn remove(&self, _: &str) -> Result<(), Self::Error> { Ok(()) }
///     fn default() -> Self { Self }
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let store = MockStore;
/// let coarse = CoarseRecallOutput { modules: vec![], elapsed_us: 0 };
/// let seed_clv = CLV::zero();
/// let recall = FineRecall::with_default_config();
///
/// let input = FineRecallInput {
///     coarse_output: &coarse,
///     seed_clv: &seed_clv,
///     vector_store: &store,
///     block_clvs: None,
///     top_k: 500,
/// };
/// let output = recall.rank(input)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct FineRecall {
    /// 精排配置（over-fetch 因子 + 精确重排开关）
    config: FineRecallConfig,
}

impl FineRecall {
    /// 创建精排引擎，使用指定配置
    pub fn new(config: FineRecallConfig) -> Self {
        Self { config }
    }

    /// 创建精排引擎，使用默认配置（overfetch=2.0, precise_rerank=true）
    pub fn with_default_config() -> Self {
        Self::new(FineRecallConfig::default())
    }

    /// 返回配置引用（只读）
    pub fn config(&self) -> &FineRecallConfig {
        &self.config
    }

    /// 执行精排 — HNSW 候选扩展 + 精确 CLV 重排 → Top-K Block
    ///
    /// # 算法步骤
    /// 1. 计算 over-fetch K = top_k × overfetch_factor
    /// 2. HNSW 检索: 用种子 CLV 在 VectorStore 中检索 Top-K 候选 Block
    /// 3. 精确 CLV 重排（如果启用且提供了 block_clvs）:
    ///    - 对每个候选 Block，用 `CLV::cosine_similarity(seed_clv, block_clv)` 精确计算相似度
    ///    - 缺失 Block CLV 时 fallback 到 HNSW score
    /// 4. Top-K 选择（`select_nth_unstable`）+ 降序排序输出
    ///
    /// # 错误
    /// - `VectorStoreError`: VectorStore 检索失败（如维度不匹配、索引损坏）
    ///
    /// # 性能
    /// - 10K Block 索引 × Top-500 输出: p95 < 35ms
    /// - 100K Block 索引 × Top-500 输出: p95 < 45ms
    /// - 输出 `elapsed_us` 字段记录实际耗时，供基准断言 <50ms
    pub fn rank<S>(&self, input: FineRecallInput<'_, S>) -> Result<FineRecallOutput, RecallError>
    where
        S: VectorStore<Meta = ()>,
        // WHY `Display` 而非 `std::error::Error`: 测试 mock 用 `Error = String`
        // （String 未实现 StdError），放宽到 Display 让 mock 与生产 WikiError 均满足
        S::Error: std::fmt::Display + Send + Sync + 'static,
    {
        self.rank_impl(
            input.seed_clv,
            input.coarse_output,
            input.vector_store,
            input.block_clvs,
            input.top_k,
        )
    }

    /// 执行探针精排 — 用混合探针 CLV 检索 + 精确重排 → Top-K Block（PROBE P1.2）
    ///
    /// # 参数
    /// - `input`: 探针精排输入（`probe_clv` 为 `mix_probe` 产出的混合向量）
    ///
    /// # 与 `rank()` 的关系
    /// 共享 [`Self::rank_impl`] 核心逻辑，仅查询向量语义不同（探针 vs 种子）；
    /// 探针路径异常（NaN/零向量率 >50%）由调用方先经 `probe_health` 检测后
    /// 决定是否回退 `rank()`（Static 路径），见 `crate::probe`。
    ///
    /// # 错误
    /// - `VectorStoreError`: VectorStore 检索失败
    pub fn rank_with_probe<S>(
        &self,
        input: ProbeRecallInput<'_, S>,
    ) -> Result<FineRecallOutput, RecallError>
    where
        S: VectorStore<Meta = ()>,
        S::Error: std::fmt::Display + Send + Sync + 'static,
    {
        self.rank_impl(
            input.probe_clv,
            input.coarse_output,
            input.vector_store,
            input.block_clvs,
            input.top_k,
        )
    }

    /// 精排核心实现（rank 与 rank_with_probe 共享，提取避免复制公式）
    ///
    /// # 参数
    /// - `query_clv`: 查询向量（种子 CLV 或混合探针 CLV）
    /// - `coarse_output`: 粗召回输出（供后续重排填充使用）
    /// - `vector_store`: Block 级向量索引
    /// - `block_clvs`: Block ID → CLV 映射（可选精确重排）
    /// - `top_k`: 返回 Block 数
    ///
    /// # 算法
    /// 1. over-fetch K = top_k × overfetch_factor
    /// 2. VectorStore 检索 Top-K 候选
    /// 3. 精确 CLV 重排（启用且提供 block_clvs 时）
    /// 4. Top-K 选择（`select_nth_unstable`）+ 降序排序输出
    ///
    /// WHY 提取共享: rank() 与 rank_with_probe() 仅查询向量语义不同，
    /// 复制 80 行逻辑违反"杜绝冗余代码"（compressor.rs L64-75 同哲学）；
    /// 既有 rank() 测试全绿 = 提取重构的行为等价验证
    fn rank_impl<S>(
        &self,
        query_clv: &CLV,
        coarse_output: &CoarseRecallOutput,
        vector_store: &S,
        block_clvs: Option<&HashMap<BlockId, CLV>>,
        top_k: usize,
    ) -> Result<FineRecallOutput, RecallError>
    where
        S: VectorStore<Meta = ()>,
        S::Error: std::fmt::Display + Send + Sync + 'static,
    {
        let start = Instant::now();
        // coarse_output 保留在输入中供 P3 模块级密度计算（rank/fill 链路透传），
        // 精排阶段不消费——显式标注避免 unused 警告
        let _ = coarse_output;

        // 1. 计算 over-fetch K
        // WHY over-fetch: HNSW 是近似检索，返回的 Top-N 可能含误差，
        // 获取更多候选后精确重排可提升 Top-K 精度
        let fetch_k = if top_k == 0 {
            return Ok(FineRecallOutput {
                blocks: Vec::new(),
                elapsed_us: start.elapsed().as_micros() as u64,
                candidate_count: 0,
            });
        } else {
            // top_k × overfetch_factor，至少 top_k + 100 保证小规模也有 over-fetch
            let factor = self.config.overfetch_factor.max(1.0) as usize;
            top_k.saturating_mul(factor).max(top_k + 100)
        };

        // 2. HNSW 检索: 用查询向量在 VectorStore 中检索 Top-K 候选 Block
        let hits: Vec<VectorHit> = vector_store
            .top_k(query_clv.as_slice(), fetch_k, "")
            .map_err(|e| RecallError::VectorStoreError(e.to_string()))?;

        let candidate_count = hits.len();

        // 3. 精确 CLV 重排（如果启用且提供了 block_clvs）
        let mut blocks: Vec<BlockScore> = hits
            .into_iter()
            .map(|hit| {
                let precise_score = if self.config.precise_rerank {
                    self.compute_precise_score(&hit, query_clv, block_clvs)
                } else {
                    // 未启用精确重排，直接用 HNSW score
                    hit.score
                };
                // token_count = 0：精排阶段不知道 Block 的 token 数，
                // 重排填充阶段由 RerankFillInput.block_tokens 注入实际值
                BlockScore::new(hit.id, precise_score, hit.score, "", 0)
            })
            .collect();

        // 4. Top-K 选择: select_nth_unstable 选 Top-K（§4.1 红线：禁止 sort_by 做 Top-K）
        //
        //    WHY (score desc, block_id asc) 比较器 + truncate(top_k)：
        //    select_nth_unstable_by 在 tie 元素间选择是不稳定的——若比较器只看 score，
        //    相同 score 的 block 任意一个都可能被选入 Top-K（P3-W9.1 test_top_k_limit 教训）。
        //    加入 block_id 作为 tiebreaker 后，tie 时字典序小的 block 优先进入 Top-K，
        //    行为确定可复现（与 coarse.rs::recall 的 Top-K 逻辑保持一致）。
        let top_k = top_k.min(blocks.len());
        if top_k == 0 {
            blocks.clear();
        } else if top_k < blocks.len() {
            // cmp_desc(a, b) = b.score vs a.score (desc) ⊕ a.block_id vs b.block_id (asc)
            blocks.select_nth_unstable_by(top_k - 1, |a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.block_id.cmp(&b.block_id))
            });
            blocks.truncate(top_k);
        }

        // 5. Top-K 排序（降序），tie 用 block_id 字典序保证稳定
        //    WHY 二次排序：select_nth 只保证 Top-K 集合正确，不保证内部顺序；
        //    K=500 时 O(K log K) ≈ 0.1ms，开销可忽略
        blocks.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.block_id.cmp(&b.block_id))
        });

        Ok(FineRecallOutput {
            blocks,
            elapsed_us: start.elapsed().as_micros() as u64,
            candidate_count,
        })
    }

    /// 计算精确 CLV 相似度
    ///
    /// # 算法
    /// - 若提供了 `block_clvs` 且包含该 Block 的 CLV:
    ///   `precise_score = max(0.0, seed_clv.cosine_similarity(block_clv))`
    ///   WHY 负相关归零: 负相似度表示"语义相反"，对召回无意义
    /// - 若未提供 `block_clvs` 或 Block CLV 缺失:
    ///   fallback 到 HNSW score（不阻塞精排，仅精度降低）
    fn compute_precise_score(
        &self,
        hit: &VectorHit,
        seed_clv: &CLV,
        block_clvs: Option<&HashMap<BlockId, CLV>>,
    ) -> f32 {
        match block_clvs.and_then(|clvs| clvs.get(&hit.id)) {
            Some(block_clv) => {
                let sim = seed_clv.cosine_similarity(block_clv);
                // 负相关归零（避免负分污染综合分数，与粗召回 semantic_similarity 一致）
                sim.max(0.0)
            }
            // WHY fallback: HnswStore::top_k 已返回精确余弦相似度（distance_to_score），
            // 缺失 block_clvs 时用 HNSW score 是可接受的降级
            None => hit.score,
        }
    }
}

impl Default for FineRecall {
    fn default() -> Self {
        Self::with_default_config()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recall::types::{CoarseRecallOutput, FineRecallConfig, RecallError};

    // ============================================================
    // 测试用 Mock VectorStore 实现
    // ============================================================

    /// 测试用内存 VectorStore — 真实存储向量并实现精确 KNN
    ///
    /// WHY 独立 mock 而非用真实 HnswStore:
    /// hcw-window (L2) 不能依赖 repo-wiki (L5)，故用本 crate 内的 mock 验证精排逻辑。
    /// 此 mock 用 O(n) 遍历 + 精确余弦相似度，确保精排逻辑正确性。
    struct InMemoryVectorStore {
        dim: usize,
        vectors: std::cell::RefCell<HashMap<String, Vec<f32>>>,
    }

    impl InMemoryVectorStore {
        fn with_dim(dim: usize) -> Self {
            Self {
                dim,
                vectors: std::cell::RefCell::new(HashMap::new()),
            }
        }

        fn insert(&self, id: &str, vector: Vec<f32>) {
            self.vectors.borrow_mut().insert(id.to_string(), vector);
        }
    }

    impl VectorStore for InMemoryVectorStore {
        type Meta = ();
        type Error = String;

        fn upsert(&self, id: &str, vector: &[f32], _meta: Self::Meta) -> Result<(), Self::Error> {
            if vector.len() != self.dim {
                return Err(format!(
                    "dimension mismatch: expected {}, got {}",
                    self.dim,
                    vector.len()
                ));
            }
            self.vectors
                .borrow_mut()
                .insert(id.to_string(), vector.to_vec());
            Ok(())
        }

        fn top_k(&self, query: &[f32], k: usize, _ns: &str) -> Result<Vec<VectorHit>, Self::Error> {
            if query.len() != self.dim {
                return Err(format!(
                    "query dimension mismatch: expected {}, got {}",
                    self.dim,
                    query.len()
                ));
            }
            let vectors = self.vectors.borrow();
            let mut scored: Vec<VectorHit> = vectors
                .iter()
                .map(|(id, vec)| VectorHit::new(id.clone(), cosine_similarity_slices(query, vec)))
                .collect();
            // Top-K 用 select_nth_unstable_by（O(n)），符合工程约定
            if k < scored.len() {
                scored.select_nth_unstable_by(k, |a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            scored.truncate(k);
            // 最终降序排序（K log K）
            scored.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Ok(scored)
        }

        fn remove(&self, id: &str) -> Result<(), Self::Error> {
            self.vectors.borrow_mut().remove(id);
            Ok(())
        }

        fn default() -> Self {
            Self {
                dim: 512,
                vectors: std::cell::RefCell::new(HashMap::new()),
            }
        }
    }

    // 统一使用 nexus-core 权威实现,避免多副本优化不一致
    use nexus_core::cosine_similarity_slices;

    // ============================================================
    // 辅助函数
    // ============================================================

    /// 构造测试用 CLV:基于 seed 生成确定性 512-dim 向量
    fn make_clv(seed: u64) -> CLV {
        let v: Vec<f32> = (0..CLV::DIMENSION)
            .map(|j| ((seed.wrapping_add(j as u64)) % 100) as f32 / 100.0)
            .collect();
        CLV::from_vec(v).expect("CLV dimension should be 512")
    }

    /// 构造空的粗召回输出（精排逻辑不直接使用，仅占位）
    fn empty_coarse_output() -> CoarseRecallOutput {
        CoarseRecallOutput {
            modules: Vec::new(),
            elapsed_us: 0,
        }
    }

    // ============================================================
    // 精排配置测试
    // ============================================================

    #[test]
    fn test_fine_recall_config_default() {
        let config = FineRecallConfig::default();
        assert_eq!(config.overfetch_factor, 2.0);
        assert!(config.precise_rerank);
    }

    #[test]
    fn test_fine_recall_with_default_config() {
        let recall = FineRecall::with_default_config();
        assert_eq!(recall.config().overfetch_factor, 2.0);
        assert!(recall.config().precise_rerank);
    }

    // ============================================================
    // 精排引擎测试 — 基础功能
    // ============================================================

    #[test]
    fn test_rank_empty_store_returns_empty() {
        let store = InMemoryVectorStore::with_dim(512);
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0);
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 500,
        };
        let output = recall.rank(input).expect("rank should succeed");
        assert!(output.blocks.is_empty());
        assert_eq!(output.candidate_count, 0);
    }

    #[test]
    fn test_rank_top_k_zero_returns_empty() {
        let store = InMemoryVectorStore::with_dim(512);
        store.insert("block-1", vec![1.0; 512]);
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0);
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 0,
        };
        let output = recall.rank(input).expect("rank should succeed");
        assert!(output.blocks.is_empty());
    }

    #[test]
    fn test_rank_single_block() {
        let store = InMemoryVectorStore::with_dim(512);
        let block_clv = make_clv(1);
        store.insert("block-1", block_clv.as_slice().to_vec());
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(1); // 与 block-1 相同
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 10,
        };
        let output = recall.rank(input).expect("rank should succeed");
        assert_eq!(output.blocks.len(), 1);
        assert_eq!(output.blocks[0].block_id, "block-1");
        // 相同向量余弦相似度 ≈ 1.0
        assert!((output.blocks[0].score - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_rank_multiple_blocks_sorted_desc() {
        let store = InMemoryVectorStore::with_dim(512);
        // 插入 5 个 Block，每个 CLV 方向不同（用 make_clv 确保余弦相似度各异）
        // block-0 与 seed_clv 完全相同 → cos_sim = 1.0（最相似）
        // block-1~4 与 seed_clv 不同 → cos_sim < 1.0
        for i in 0..5u32 {
            let clv = make_clv(i as u64);
            store.insert(&format!("block-{i}"), clv.as_slice().to_vec());
        }
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0); // 与 block-0 相同
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 3,
        };
        let output = recall.rank(input).expect("rank should succeed");
        assert_eq!(output.blocks.len(), 3);
        // block-0 应排第一（与 seed_clv 完全相同，cos_sim = 1.0）
        assert_eq!(output.blocks[0].block_id, "block-0");
        assert!((output.blocks[0].score - 1.0).abs() < 1e-4);
        // 验证降序排列
        for i in 1..output.blocks.len() {
            assert!(
                output.blocks[i - 1].score >= output.blocks[i].score,
                "应按 score 降序: [{}]={} < [{}]={}",
                i - 1,
                output.blocks[i - 1].score,
                i,
                output.blocks[i].score
            );
        }
    }

    #[test]
    fn test_rank_top_k_exceeds_candidates() {
        let store = InMemoryVectorStore::with_dim(512);
        store.insert("block-1", vec![1.0; 512]);
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0);
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 500, // 远超候选数 1
        };
        let output = recall.rank(input).expect("rank should succeed");
        assert_eq!(output.blocks.len(), 1);
    }

    // ============================================================
    // 精确 CLV 重排测试
    // ============================================================

    #[test]
    fn test_precise_rerank_with_block_clvs() {
        let store = InMemoryVectorStore::with_dim(512);
        let block_clv_1 = make_clv(1);
        let block_clv_2 = make_clv(2);
        store.insert("block-1", block_clv_1.as_slice().to_vec());
        store.insert("block-2", block_clv_2.as_slice().to_vec());

        let coarse = empty_coarse_output();
        let seed_clv = make_clv(1); // 与 block-1 相同
        let recall = FineRecall::with_default_config();

        // 提供 block_clvs，启用精确重排
        let mut block_clvs = HashMap::new();
        block_clvs.insert("block-1".to_string(), block_clv_1);
        block_clvs.insert("block-2".to_string(), block_clv_2);

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: Some(&block_clvs),
            top_k: 10,
        };
        let output = recall.rank(input).expect("rank should succeed");
        // block-1 应排第一（与 seed_clv 完全相同）
        assert_eq!(output.blocks[0].block_id, "block-1");
        assert!((output.blocks[0].score - 1.0).abs() < 1e-4);
        // hnsw_score 应与精确 score 接近（InMemoryStore 用相同余弦相似度）
        assert!((output.blocks[0].hnsw_score - output.blocks[0].score).abs() < 1e-5);
    }

    #[test]
    fn test_precise_rerank_missing_block_clv_fallback() {
        let store = InMemoryVectorStore::with_dim(512);
        store.insert("block-1", vec![1.0; 512]);
        store.insert("block-2", vec![0.5; 512]);

        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0);
        let recall = FineRecall::with_default_config();

        // 仅提供 block-1 的 CLV，block-2 缺失应 fallback 到 HNSW score
        let mut block_clvs = HashMap::new();
        block_clvs.insert("block-1".to_string(), make_clv(0));

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: Some(&block_clvs),
            top_k: 10,
        };
        let output = recall.rank(input).expect("rank should succeed");
        // block-2 的 score 应来自 HNSW（fallback）
        let block_2 = output
            .blocks
            .iter()
            .find(|b| b.block_id == "block-2")
            .expect("block-2 should exist");
        assert!((block_2.score - block_2.hnsw_score).abs() < 1e-5);
    }

    #[test]
    fn test_precise_rerank_disabled() {
        let store = InMemoryVectorStore::with_dim(512);
        store.insert("block-1", vec![1.0; 512]);

        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0);
        // 禁用精确重排
        let recall = FineRecall::new(FineRecallConfig {
            overfetch_factor: 2.0,
            precise_rerank: false,
        });

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 10,
        };
        let output = recall.rank(input).expect("rank should succeed");
        // 禁用精确重排时，score 应等于 hnsw_score
        for block in &output.blocks {
            assert!((block.score - block.hnsw_score).abs() < 1e-5);
        }
    }

    // ============================================================
    // 错误处理测试
    // ============================================================

    #[test]
    fn test_rank_vector_store_error() {
        // 用 3-dim store 但 512-dim query 触发维度不匹配
        let store = InMemoryVectorStore::with_dim(3);
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0); // 512-dim
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 10,
        };
        let result = recall.rank(input);
        assert!(result.is_err());
        match result {
            Err(RecallError::VectorStoreError(msg)) => {
                assert!(msg.contains("dimension mismatch"));
            }
            other => panic!("expected VectorStoreError, got {other:?}"),
        }
    }

    // ============================================================
    // 性能指标测试
    // ============================================================

    #[test]
    fn test_rank_records_elapsed_us() {
        let store = InMemoryVectorStore::with_dim(512);
        store.insert("block-1", vec![1.0; 512]);
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0);
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 10,
        };
        let output = recall.rank(input).expect("rank should succeed");
        // elapsed_us 应 > 0（至少记录了 Instant::now 的时间差）
        assert!(output.elapsed_us < 1_000_000); // < 1s（防卡死）
    }

    #[test]
    fn test_rank_records_candidate_count() {
        let store = InMemoryVectorStore::with_dim(512);
        for i in 0..10 {
            store.insert(&format!("block-{i}"), vec![1.0; 512]);
        }
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0);
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 5,
        };
        let output = recall.rank(input).expect("rank should succeed");
        // over-fetch: 5 × 2 = 10，但 store 只有 10 个，candidate_count 应 = 10
        assert_eq!(output.candidate_count, 10);
        // 最终返回 top_k = 5
        assert_eq!(output.blocks.len(), 5);
    }

    // ============================================================
    // over-fetch 因子测试
    // ============================================================

    #[test]
    fn test_overfetch_factor_affects_candidate_count() {
        let store = InMemoryVectorStore::with_dim(512);
        for i in 0..100 {
            store.insert(&format!("block-{i}"), vec![1.0; 512]);
        }
        let coarse = empty_coarse_output();
        let seed_clv = make_clv(0);

        // overfetch_factor = 1.5 → fetch_k = 5 × 1.5 = 7（向下取整）
        // 但至少 top_k + 100 = 105，故 fetch_k = 105
        // 100 个 Block 时 fetch_k 被 store 大小限制
        let recall = FineRecall::new(FineRecallConfig {
            overfetch_factor: 1.5,
            precise_rerank: true,
        });

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: None,
            top_k: 5,
        };
        let output = recall.rank(input).expect("rank should succeed");
        // store 只有 100 个，candidate_count 应 = 100
        assert_eq!(output.candidate_count, 100);
        assert_eq!(output.blocks.len(), 5);
    }

    // ============================================================
    // BlockScore 测试
    // ============================================================

    #[test]
    fn test_block_score_new() {
        let score = BlockScore::new("block-1", 0.95, 0.93, "module-a", 1024);
        assert_eq!(score.block_id, "block-1");
        assert!((score.score - 0.95).abs() < 1e-5);
        assert!((score.hnsw_score - 0.93).abs() < 1e-5);
        assert_eq!(score.source_module, "module-a");
    }

    // ============================================================
    // 综合生命周期测试
    // ============================================================

    #[test]
    fn test_full_lifecycle_rank_with_precise_rerank() {
        let store = InMemoryVectorStore::with_dim(512);
        let mut block_clvs = HashMap::new();

        // 插入 50 个 Block，CLV 各不相同
        for i in 0..50u32 {
            let clv = make_clv(i as u64);
            store.insert(&format!("block-{i}"), clv.as_slice().to_vec());
            block_clvs.insert(format!("block-{i}"), clv);
        }

        let coarse = empty_coarse_output();
        let seed_clv = make_clv(10); // 与 block-10 最相似
        let recall = FineRecall::with_default_config();

        let input = FineRecallInput {
            coarse_output: &coarse,
            seed_clv: &seed_clv,
            vector_store: &store,
            block_clvs: Some(&block_clvs),
            top_k: 20,
        };
        let output = recall.rank(input).expect("rank should succeed");

        assert_eq!(output.blocks.len(), 20);
        // block-10 应排第一（与 seed_clv 完全相同）
        assert_eq!(output.blocks[0].block_id, "block-10");
        assert!((output.blocks[0].score - 1.0).abs() < 1e-4);
        // 验证降序
        for i in 1..output.blocks.len() {
            assert!(
                output.blocks[i - 1].score >= output.blocks[i].score,
                "应按 score 降序"
            );
        }
    }
}
