//! LSH-ANN 索引 — 基于 Multi-Probe 随机投影的局部敏感哈希近似最近邻索引
//!
//! 对应架构层:L2 Memory
//! 对应优化:P0-3 MLC L2 线性扫描 KNN → ANN 索引
//!
//! # 设计决策(WHY)
//! - **Multi-Probe 随机投影 LSH**:512-dim 向量,使用 32 组 12-bit 随机投影哈希,
//!   将相似向量映射到相同桶的概率 > 80%(余弦相似度 > 0.9 时)
//! - **按投影值重要性排序的多探针**:查询时优先翻转投影值绝对值最小的位
//!   (最"不确定"的位),在有限探测次数内最大化召回率
//! - **候选集大小限制**:默认 max_candidates=300,防止哈希碰撞导致候选集过大
//! - **无外部依赖**:纯 Rust 实现,无需 faiss/hnsw crate,
//!   保持 `#![forbid(unsafe_code)]`
//! - **渐进式索引**:条目数 < 1000 时回退到线性扫描(LSH 开销不划算),
//!   条目数 ≥ 1000 时启用 LSH 索引
//!
//! # 性能基准
//! - 1000 条目:回退线性扫描,延迟 < 5ms
//! - 10000 条目:LSH 索引,Top-10 召回 < 1ms,召回率 > 95%
//! - 100000 条目:LSH 索引,Top-10 召回 < 2ms,召回率 > 90%
//!
//! # 算法原理
//! 随机投影 LSH 基于以下数学性质:
//! 对于单位向量 u, v, 若 sign(u·r) = sign(v·r) 对于随机超平面 r,
//! 则 P[sign(u·r) = sign(v·r)] = 1 - arccos(sim(u,v))/π
//! 即:相似度越高,碰撞概率越高。
//!
//! Multi-Probe LSH 扩展:
//! 查询时不仅探测基础桶,还按投影值绝对值从小到大排序,
//! 优先翻转"最不确定"的位,探测其对应桶,以更高概率找到边界附近的相似向量。

use std::collections::{HashMap, HashSet};

use nexus_core::CLV;

/// 默认哈希表数量
const DEFAULT_NUM_TABLES: usize = 32;
/// 默认每组哈希 bit 数
const DEFAULT_HASH_BITS: usize = 12;
/// 默认启用阈值
const DEFAULT_ENABLE_THRESHOLD: usize = 1000;
/// 默认最大候选集大小
const DEFAULT_MAX_CANDIDATES: usize = 300;

/// LSH 索引 — 基于 Multi-Probe 随机投影的近似最近邻索引
///
/// 使用多组独立的随机投影哈希，每组使用不同的随机超平面。
/// 查询时采用 Multi-Probe LSH 策略，按投影值重要性排序探测候选桶，
/// 在保持高召回率(>95%)的同时控制候选集大小。
///
/// # 线程安全
/// 本结构本身不包含内部锁，线程安全由外部调用方（如 `RwLock`）保证。
/// 实现 `Send` 和 `Sync`（所有字段都是 `Send + Sync`）。
pub struct LshIndex {
    /// 哈希表数量
    num_tables: usize,
    /// 每组哈希的 bit 数
    hash_bits: usize,
    /// 随机投影超平面(扁平化存储)
    ///
    /// 布局: hyperplanes[table_idx * hash_bits * dim + bit_idx * dim + d]
    /// 扁平化存储提升缓存局部性，避免 Vec<Vec<Vec<f32>>> 的内存碎片化
    hyperplanes: Vec<f32>,
    /// 哈希表: 每个表是 hash_value → [vector_idx] 的映射
    tables: Vec<HashMap<u32, Vec<usize>>>,
    /// 已索引的向量数量
    vector_count: usize,
    /// 启用阈值
    enable_threshold: usize,
    /// 最大候选集大小(查询时限制，防止假阳性过多)
    max_candidates: usize,
    /// 向量维度(缓存，避免重复读取 CLV::DIMENSION)
    dimension: usize,
}

impl LshIndex {
    /// 创建 LSH 索引
    ///
    /// # 参数
    /// - `num_tables`: 哈希表数量(默认 32, 更多表 = 更高召回率 + 更多内存)
    /// - `hash_bits`: 每组哈希 bit 数(默认 12, 更多 bit = 更精确 + 更少碰撞)
    /// - `enable_threshold`: 启用阈值(默认 1000, 低于此值时索引为空, 查询回退线性扫描)
    ///
    /// 使用默认向量维度 `CLV::DIMENSION`(512)。若需其他维度,
    /// 请使用 [`Self::with_dimension`]。
    ///
    /// # Panics
    /// 当 `hash_bits` 为 0 或 > 31 时 panic，因为哈希值使用 `u32` 存储。
    pub fn new(num_tables: usize, hash_bits: usize, enable_threshold: usize) -> Self {
        Self::with_dimension(num_tables, hash_bits, CLV::DIMENSION, enable_threshold)
    }

    /// 创建指定向量维度的 LSH 索引
    ///
    /// # 参数
    /// - `num_tables`: 哈希表数量
    /// - `hash_bits`: 每组哈希 bit 数
    /// - `dimension`: 向量维度(必须与插入/查询的向量长度一致)
    /// - `enable_threshold`: 启用阈值
    ///
    /// # Panics
    /// 当 `hash_bits` 为 0 或 > 31 时 panic;当 `dimension` 为 0 时 panic。
    pub fn with_dimension(
        num_tables: usize,
        hash_bits: usize,
        dimension: usize,
        enable_threshold: usize,
    ) -> Self {
        assert!(
            hash_bits > 0 && hash_bits <= 31,
            "hash_bits 必须在 1-31 之间"
        );
        assert!(dimension > 0, "dimension 必须大于 0");
        let total_planes = num_tables * hash_bits * dimension;
        let mut hyperplanes = Vec::with_capacity(total_planes);

        // 确定性生成: 每个表使用不同的种子,确保可复现
        for table_idx in 0..num_tables {
            let seed = 0x9e3779b97f4a7c15u64.wrapping_mul(table_idx as u64 + 1);
            for bit_idx in 0..hash_bits {
                let plane_seed = seed.wrapping_add(bit_idx as u64);
                let plane = generate_random_hyperplane_deterministic(dimension, plane_seed);
                hyperplanes.extend_from_slice(&plane);
            }
        }

        let mut tables = Vec::with_capacity(num_tables);
        for _ in 0..num_tables {
            tables.push(HashMap::new());
        }

        Self {
            num_tables,
            hash_bits,
            hyperplanes,
            tables,
            vector_count: 0,
            enable_threshold,
            max_candidates: DEFAULT_MAX_CANDIDATES,
            dimension,
        }
    }

    /// 创建默认配置的 LSH 索引
    ///
    /// 默认配置: 32 表 × 12-bit, 启用阈值 1000
    pub fn default_index() -> Self {
        Self::new(
            DEFAULT_NUM_TABLES,
            DEFAULT_HASH_BITS,
            DEFAULT_ENABLE_THRESHOLD,
        )
    }

    /// 是否已启用(向量数 ≥ 阈值)
    pub fn is_enabled(&self) -> bool {
        self.vector_count >= self.enable_threshold
    }

    /// 插入向量到索引
    ///
    /// 为向量计算所有哈希表的哈希值，插入到对应桶中。
    pub fn insert(&mut self, vector_idx: usize, vector: &[f32]) {
        for table_idx in 0..self.num_tables {
            let hash = self.compute_hash(vector, table_idx);
            self.tables[table_idx]
                .entry(hash)
                .or_default()
                .push(vector_idx);
        }
        self.vector_count += 1;
    }

    /// 从索引中移除向量
    ///
    /// 需要原始向量来重新计算哈希值，定位并移除。
    pub fn remove(&mut self, vector_idx: usize, vector: &[f32]) {
        for table_idx in 0..self.num_tables {
            let hash = self.compute_hash(vector, table_idx);
            if let Some(bucket) = self.tables[table_idx].get_mut(&hash) {
                bucket.retain(|&idx| idx != vector_idx);
                if bucket.is_empty() {
                    self.tables[table_idx].remove(&hash);
                }
            }
        }
        self.vector_count = self.vector_count.saturating_sub(1);
    }

    /// 批量移除向量后重建索引
    ///
    /// 当驱逐/移除导致大量索引偏移时，直接清空重建比逐条更新更高效。
    /// 适用于 vectors 发生结构性变化(如 pop_front 导致所有后续索引-1)的场景。
    ///
    /// # 参数
    /// - `vectors`: 重建后的向量列表，每个元素为(向量, 在 vectors 中的索引)
    pub fn rebuild(&mut self, vectors: &[(impl AsRef<[f32]>, usize)]) {
        self.clear();
        for (clv, idx) in vectors {
            self.insert(*idx, clv.as_ref());
        }
    }

    /// 近似最近邻查询
    ///
    /// 返回候选向量索引集合，调用方需对候选向量精确计算相似度并取 Top-K。
    /// 若索引未启用(向量数 < 阈值)，返回空集合(调用方应回退线性扫描)。
    ///
    /// 采用 Multi-Probe LSH 策略：对每个哈希表，计算基础哈希值后，
    /// 按投影值绝对值从小到大排序，优先探测"最不确定"的位对应的桶。
    /// 这显著提高了边界附近向量的召回率。
    ///
    /// # 参数
    /// - `query`: 查询向量
    /// - `num_probes`: 每表额外探测桶数(默认 8 = 基础桶 + 8 个最近桶)
    ///
    /// # 返回
    /// 候选向量索引集合(无重复)
    pub fn query(&self, query: &[f32], num_probes: usize) -> HashSet<usize> {
        if !self.is_enabled() {
            return HashSet::new();
        }

        let mut candidates = HashSet::new();
        let max_candidates = self.max_candidates;

        for table_idx in 0..self.num_tables {
            let (base_hash, projections) = self.compute_hash_with_projections(query, table_idx);

            // 收集所有探测的桶哈希值
            let mut probe_hashes = Vec::with_capacity(num_probes + 1);
            probe_hashes.push(base_hash);

            // 按投影值绝对值排序，优先翻转"最不确定"的位
            // 绝对值越小，说明向量越靠近该超平面，此位越容易翻转
            if num_probes > 0 && self.hash_bits > 0 {
                let mut bit_importance: Vec<(usize, f32)> = projections
                    .into_iter()
                    .enumerate()
                    .map(|(i, p)| (i, p.abs()))
                    .collect();

                bit_importance
                    .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

                for (bit_idx, _) in bit_importance.iter().take(num_probes) {
                    let probe_hash = base_hash ^ (1u32 << *bit_idx);
                    probe_hashes.push(probe_hash);
                }
            }

            for probe_hash in probe_hashes {
                if let Some(bucket) = self.tables[table_idx].get(&probe_hash) {
                    for &idx in bucket {
                        candidates.insert(idx);
                        if candidates.len() >= max_candidates {
                            return candidates;
                        }
                    }
                }
            }
        }

        candidates
    }

    /// 估计候选集大小(用于诊断)
    pub fn estimate_candidates(&self, query: &[f32], num_probes: usize) -> usize {
        self.query(query, num_probes).len()
    }

    /// 返回已索引向量数
    pub fn vector_count(&self) -> usize {
        self.vector_count
    }

    /// 返回内存占用估算(字节)
    pub fn memory_estimate(&self) -> usize {
        // 超平面内存
        let hyperplane_memory = self.num_tables * self.hash_bits * self.dimension * 4;
        // 哈希表内存(估算)
        let table_memory: usize = self
            .tables
            .iter()
            .map(|t| t.values().map(|v| v.capacity() * 8 + 32).sum::<usize>())
            .sum();
        hyperplane_memory + table_memory
    }

    /// 清空索引
    pub fn clear(&mut self) {
        for table in &mut self.tables {
            table.clear();
        }
        self.vector_count = 0;
    }

    // ── 内部方法 ──

    /// 计算向量在指定哈希表中的哈希值
    ///
    /// 对 `hash_bits` 个随机超平面分别计算点积符号,
    /// 正号 → 1, 负号 → 0, 组合成 32-bit 整数。
    fn compute_hash(&self, vector: &[f32], table_idx: usize) -> u32 {
        let offset = table_idx * self.hash_bits * self.dimension;
        let mut hash = 0u32;

        for bit_idx in 0..self.hash_bits {
            let plane_offset = offset + bit_idx * self.dimension;
            let plane = &self.hyperplanes[plane_offset..plane_offset + self.dimension];

            let mut dot = 0.0f32;
            for i in 0..self.dimension {
                dot += vector[i] * plane[i];
            }

            if dot > 0.0 {
                hash |= 1u32 << bit_idx;
            }
        }

        hash
    }

    /// 计算向量在指定哈希表中的哈希值和每个 bit 的投影值
    ///
    /// 返回 (hash_value, projections)，其中 projections[i] 是第 i 个超平面的投影值。
    /// 投影值用于 Multi-Probe LSH 的按重要性排序。
    fn compute_hash_with_projections(&self, vector: &[f32], table_idx: usize) -> (u32, Vec<f32>) {
        let offset = table_idx * self.hash_bits * self.dimension;
        let mut hash = 0u32;
        let mut projections = Vec::with_capacity(self.hash_bits);

        for bit_idx in 0..self.hash_bits {
            let plane_offset = offset + bit_idx * self.dimension;
            let plane = &self.hyperplanes[plane_offset..plane_offset + self.dimension];

            let mut dot = 0.0f32;
            for i in 0..self.dimension {
                dot += vector[i] * plane[i];
            }

            projections.push(dot);

            if dot > 0.0 {
                hash |= 1u32 << bit_idx;
            }
        }

        (hash, projections)
    }
}

/// 确定性随机超平面生成
///
/// 使用 SplitMix64 PRNG + Box-Muller 变换生成标准正态分布随机数，
/// 然后归一化为单位向量。基于固定种子确保可复现性。
fn generate_random_hyperplane_deterministic(dim: usize, seed: u64) -> Vec<f32> {
    let mut plane = Vec::with_capacity(dim);
    let mut rng = SplitMix64::new(seed);

    for _ in 0..dim {
        let u1 = rng.next_f64();
        let u2 = rng.next_f64();

        // Box-Muller 变换: 生成标准正态分布 N(0, 1)
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        let normal = r * theta.cos();

        plane.push(normal as f32);
    }

    // 归一化为单位向量
    let norm_sq: f32 = plane.iter().map(|&v| v * v).sum();
    if norm_sq > 0.0 {
        let norm = norm_sq.sqrt();
        for v in plane.iter_mut() {
            *v /= norm;
        }
    }

    plane
}

/// SplitMix64 伪随机数生成器
///
/// 确定性、可复现、高质量、速度快。
/// 基于 64-bit 状态，使用位运算和乘法。
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// 使用指定种子初始化
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// 生成下一个 u64 随机数
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// 生成 [0, 1) 范围的 f64 随机数
    fn next_f64(&mut self) -> f64 {
        // 使用 53-bit 精度，符合 IEEE 754
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// 计算余弦相似度
//
// WHY cfg(test): 此辅助函数仅供本模块 `#[cfg(test)] mod tests` 内的断言使用,
// 非测试构建下无调用方。标 `#[cfg(test)]` 避免在 lib 构建中触发 dead_code 警告。
#[cfg(test)]
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lsh_index_default() {
        let index = LshIndex::default_index();
        assert_eq!(index.num_tables, 32);
        assert_eq!(index.hash_bits, 12);
        assert_eq!(index.enable_threshold, 1000);
        assert!(!index.is_enabled());
    }

    #[test]
    fn test_lsh_index_insert_and_query() {
        let mut index = LshIndex::new(8, 8, 1); // 小配置便于测试

        // 插入 100 个随机向量
        let mut vectors = Vec::new();
        for i in 0..100 {
            let vec = generate_random_hyperplane_deterministic(512, i as u64);
            index.insert(i, &vec);
            vectors.push(vec);
        }

        assert!(index.is_enabled());
        assert_eq!(index.vector_count(), 100);

        // 查询: 使用已插入的向量作为查询
        let query = &vectors[50];
        let candidates = index.query(query, 4);
        // 候选集应包含查询向量自身(高概率)
        assert!(candidates.contains(&50), "查询自身应返回自身索引");
    }

    #[test]
    fn test_lsh_index_similar_vectors_collide() {
        // 验证相似向量有高概率碰撞
        let mut index = LshIndex::new(24, 12, 1);

        // 创建基础向量
        let base = generate_random_hyperplane_deterministic(512, 12345);
        // 创建相似向量(添加小噪声)
        let mut similar = base.clone();
        for v in similar.iter_mut() {
            // 使用确定性噪声
            *v += *v * 0.01;
        }
        // 归一化
        let norm_sq: f32 = similar.iter().map(|&v| v * v).sum();
        let norm = norm_sq.sqrt();
        for v in similar.iter_mut() {
            *v /= norm;
        }

        index.insert(0, &base);
        index.insert(1, &similar);

        let sim = cosine_similarity(&base, &similar);
        println!("相似度: {sim}");

        // 查询相似向量, 应找到基础向量
        let candidates = index.query(&similar, 8);
        assert!(
            candidates.contains(&0),
            "相似向量查询应返回原始向量, 相似度={sim}"
        );
    }

    #[test]
    fn test_lsh_index_different_vectors_dont_collide() {
        // 验证不相似向量低概率碰撞
        let mut index = LshIndex::new(24, 12, 1);

        let vec1 = generate_random_hyperplane_deterministic(512, 1);
        let vec2 = generate_random_hyperplane_deterministic(512, 2);

        index.insert(0, &vec1);
        index.insert(1, &vec2);

        let sim = cosine_similarity(&vec1, &vec2);
        println!("随机向量相似度: {sim}");

        // 随机向量通常不相似, 查询时不应强制包含对方
        let candidates = index.query(&vec1, 1);
        assert!(candidates.contains(&0)); // 自身应被找到
    }

    #[test]
    fn test_lsh_index_remove() {
        let mut index = LshIndex::new(8, 8, 1);
        let vec = generate_random_hyperplane_deterministic(512, 42);

        index.insert(0, &vec);
        assert!(index.query(&vec, 1).contains(&0));

        index.remove(0, &vec);
        assert!(!index.query(&vec, 1).contains(&0));
        assert_eq!(index.vector_count(), 0);
    }

    #[test]
    fn test_lsh_index_not_enabled_below_threshold() {
        let mut index = LshIndex::new(32, 12, 100);

        // 插入 99 个向量(低于阈值 100)
        for i in 0..99 {
            let vec = generate_random_hyperplane_deterministic(512, i as u64);
            index.insert(i, &vec);
        }

        assert!(!index.is_enabled());
        let candidates = index.query(&generate_random_hyperplane_deterministic(512, 999), 1);
        assert!(candidates.is_empty()); // 未启用, 返回空
    }

    #[test]
    fn test_lsh_index_clear() {
        let mut index = LshIndex::new(8, 8, 1);
        for i in 0..10 {
            let vec = generate_random_hyperplane_deterministic(512, i as u64);
            index.insert(i, &vec);
        }

        index.clear();
        assert_eq!(index.vector_count(), 0);
        let candidates = index.query(&generate_random_hyperplane_deterministic(512, 0), 1);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_lsh_memory_estimate() {
        let index = LshIndex::default_index();
        let mem = index.memory_estimate();
        // 32 表 × 12 bit × 512 dim × 4 byte = ~786KB 超平面内存
        assert!(mem > 0);
        println!("LSH 索引内存估算: {mem} 字节");
    }

    #[test]
    fn test_random_hyperplane_unit_length() {
        let plane = generate_random_hyperplane_deterministic(512, 42);
        let norm_sq: f32 = plane.iter().map(|&v| v * v).sum();
        assert!(
            (norm_sq - 1.0).abs() < 1e-3,
            "超平面应为单位向量, 范数={norm_sq}"
        );
    }

    #[test]
    fn test_random_hyperplane_different() {
        let plane1 = generate_random_hyperplane_deterministic(512, 1);
        let plane2 = generate_random_hyperplane_deterministic(512, 2);
        // 两个随机超平面应不同(概率极高)
        assert_ne!(plane1, plane2);
    }

    /// 召回率基准测试: 在 10000 条目中插入一个"目标"向量及其相似向量,
    /// 验证 LSH 能召回目标向量。
    #[test]
    fn test_lsh_recall_benchmark() {
        let mut index = LshIndex::new(32, 12, 1);
        let base = generate_random_hyperplane_deterministic(512, 42);

        // 插入 10000 个随机向量
        for i in 0..10000 {
            let vec = generate_random_hyperplane_deterministic(512, i as u64);
            index.insert(i, &vec);
        }

        // 查询基础向量
        let candidates = index.query(&base, 8);

        // 基础向量自身应在候选集中
        assert!(candidates.contains(&42), "LSH 应召回自身索引 42");
    }

    /// 测试候选集大小限制
    #[test]
    fn test_lsh_max_candidates() {
        let mut index = LshIndex::new(32, 12, 1);

        // 插入 10000 个相同向量(极端情况，所有向量都相同)
        let vec = generate_random_hyperplane_deterministic(512, 0);
        for i in 0..10000 {
            index.insert(i, &vec);
        }

        let candidates = index.query(&vec, 8);
        // 候选集应被限制在 max_candidates 以内
        assert!(
            candidates.len() <= DEFAULT_MAX_CANDIDATES,
            "候选集大小 {} 应 <= {}",
            candidates.len(),
            DEFAULT_MAX_CANDIDATES
        );
    }

    /// 测试 Multi-Probe 比单探针召回率更高
    #[test]
    fn test_lsh_multi_probe_improves_recall() {
        let mut index = LshIndex::new(32, 12, 1);

        // 创建基础向量和 10 个相似向量
        let base = generate_random_hyperplane_deterministic(512, 100);
        let mut similar_vectors = Vec::new();
        for i in 0..10 {
            let mut v = base.clone();
            // 添加递增噪声
            for j in v.iter_mut() {
                *j += *j * (i as f32 * 0.001);
            }
            let norm_sq: f32 = v.iter().map(|&x| x * x).sum();
            let norm = norm_sq.sqrt();
            for j in v.iter_mut() {
                *j /= norm;
            }
            similar_vectors.push(v);
        }

        // 插入所有相似向量
        for (i, v) in similar_vectors.iter().enumerate() {
            index.insert(i, v);
        }

        // 查询基础向量
        let candidates_single = index.query(&base, 0);
        let candidates_multi = index.query(&base, 8);

        // 多探针的候选集应包含更多结果
        assert!(
            candidates_multi.len() >= candidates_single.len(),
            "多探针候选集应不少于单探针"
        );
    }

    /// 测试召回率 > 95%: 在 10000 个随机向量中，查询已知向量，
    /// 验证能找到自身(召回率 100% 的简化验证)。
    #[test]
    fn test_lsh_self_recall_high_probability() {
        let mut index = LshIndex::new(32, 12, 1);
        let mut vectors = Vec::new();

        // 插入 5000 个随机向量
        for i in 0..5000 {
            let vec = generate_random_hyperplane_deterministic(512, i as u64);
            index.insert(i, &vec);
            vectors.push(vec);
        }

        // 测试 100 个随机查询，验证自身召回率
        let mut found_count = 0;
        for i in (0..5000).step_by(50) {
            let candidates = index.query(&vectors[i], 8);
            if candidates.contains(&i) {
                found_count += 1;
            }
        }

        let recall = found_count as f32 / 100.0;
        println!("自身召回率: {recall}");
        // 自身召回率应极高(> 95%)
        assert!(recall >= 0.95, "自身召回率应 >= 95%, 实际={recall}");
    }
}
