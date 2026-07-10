//! 能力替代候选注册表 — 并发安全的语义向量存储与 Top-K 查询
//!
//! 对应架构层:L10 Interface
//!
//! ## 设计要点
//! - 基于 `DashMap<String, CapabilityDescriptor>` 实现并发安全 O(1) 查找
//! - `find_substitutes` 使用 `select_nth_unstable_by` 实现 O(n) Top-K 选择(降序)
//! - 候选 tier 按相似度排名分配:0=primary, 1=secondary, 2=tertiary(封顶)
//! - 命中/未命中计数用原子操作,无锁监控指标
//!
//! ## 性能目标
//! - 100 能力 × 50 维 in-memory
//! - 单次替代查询 p95 ≤ 30ms(纯内存计算,实际远低于此)

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::error::CsnError;
use crate::similarity::cosine_similarity;
use crate::types::{CapabilityDescriptor, SubstitutionCandidate};

// P0-14:复用mlc-engine的LSH-ANN索引
use mlc_engine::LshIndex;

/// 替代候选注册表统计 — 监控指标快照
///
/// 用于运行时监控注册表健康度(命中率、容量利用率)。
#[derive(Debug, Clone, Default)]
pub struct SubstitutionRegistryStats {
    /// 当前注册能力数
    pub len: usize,
    /// 注册表容量上限
    pub capacity: usize,
    /// 命中次数(查询命中已注册能力)
    pub hits: usize,
    /// 未命中次数(查询未注册能力)
    pub misses: usize,
}

impl SubstitutionRegistryStats {
    /// 计算命中率 [0.0, 1.0]
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f32 / total as f32
    }
}

/// 能力替代候选注册表 — 并发安全的能力语义向量存储
///
/// 基于 `DashMap<String, CapabilityDescriptor>`,支持:
/// - O(1) 能力注册与查找(分片锁)
/// - O(n) Top-K 替代候选选择(n = 注册能力数)
/// - 命中/未命中原子计数(无锁监控)
///
/// ## 并发安全
/// DashMap 采用分片锁,不同 key 的读写互不阻塞。
/// `find_substitutes` 遍历快照后计算,遍历期间的插入不影响一致性。
pub struct SubstitutionCandidateRegistry {
    /// 能力描述符表(capability_id → descriptor)
    capabilities: DashMap<String, CapabilityDescriptor>,
    /// 容量上限
    capacity: usize,
    /// 命中计数(原子操作,无锁监控)
    hits: AtomicUsize,
    /// 未命中计数
    misses: AtomicUsize,
    /// register() 串行化锁 — 修复 B-Maj-1 TOCTOU 竞态
    ///
    /// WHY: DashMap 的 `contains_key`/`len()` 与 `insert` 之间存在 check-then-act 窗口,
    /// 高并发下会导致容量超限或重复条目。此锁将 "检查存在性 + 检查容量 + 插入"
    /// 原子化为单一临界区。register 是冷路径(启动/配置加载),序列化开销可接受。
    register_lock: Mutex<()>,
    /// P0-14:LSH-ANN索引(可选,能力数≥阈值时启用)
    ///
    /// WHY:当注册能力数≥lsh_enable_threshold时启用LSH索引,
    /// 将查询复杂度从O(n)降至O(1)哈希查找+O(k)精确计算。
    lsh_index: Mutex<Option<LshIndex>>,
    /// P0-14:LSH启用阈值与配置
    lsh_enable_threshold: usize,
}

impl SubstitutionCandidateRegistry {
    /// 创建指定容量的注册表
    pub fn new(capacity: usize) -> Self {
        Self::with_lsh_config(capacity, 20, 8, 16)
    }

    /// P0-14:创建带LSH配置的注册表
    ///
    /// # 参数
    /// - `capacity`:注册表容量上限
    /// - `lsh_enable_threshold`:启用LSH的阈值(能力数≥此值时启用)
    /// - `lsh_num_tables`:LSH哈希表数
    /// - `lsh_hash_bits`:每表哈希bit数
    pub fn with_lsh_config(
        capacity: usize,
        lsh_enable_threshold: usize,
        lsh_num_tables: usize,
        lsh_hash_bits: usize,
    ) -> Self {
        Self {
            capabilities: DashMap::new(),
            capacity,
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
            register_lock: Mutex::new(()),
            lsh_index: Mutex::new(None),
            lsh_enable_threshold,
        }
    }

    /// 注册能力描述符 — 若 capability_id 已存在则覆盖(更新)
    ///
    /// # 错误
    /// - `InvalidCapability`:capability_id 为空
    /// - `RegistryFull`:注册表已满(且 key 不存在)
    ///
    /// # 注意
    /// 语义向量维度校验由调用方(CsnSubstitutor)在配置层完成,
    /// 注册表本身不强制维度一致性(支持运行时动态维度调整)。
    pub fn register(&self, cap: CapabilityDescriptor) -> Result<(), CsnError> {
        // 校验 capability_id 非空(系统边界校验)
        if cap.capability_id.is_empty() {
            return Err(CsnError::InvalidCapability {
                reason: "capability_id 不能为空".into(),
            });
        }

        // capacity == 0 表示禁用注册表,拒绝所有注册
        if self.capacity == 0 {
            return Err(CsnError::RegistryFull { capacity: 0 });
        }

        let key = cap.capability_id.clone();

        // WHY: 持有 register_lock 将 "检查存在性 + 检查容量 + 插入" 原子化,
        // 修复 B-Maj-1 TOCTOU 竞态。原先 contains_key + insert 与 len + insert
        // 是无锁 check-then-act 模式,高并发下会突破容量上限。
        // unwrap_or_else 处理中毒锁(前任持有者 panic),恢复访问内部数据。
        let _guard = self.register_lock.lock().unwrap_or_else(|e| e.into_inner());

        // WHY: 容量检查必须在 entry() 之前完成。entry() 持有 shard 写锁,
        // 此时调用 len() 会自死锁(len 需读所有分片,包括 entry 持有写锁的分片)。
        // 在 register_lock 保护下,contains_key/len 读取准确,无 TOCTOU 窗口
        // (只有 register() 能插入,而它已被锁串行化)。
        let key_exists = self.capabilities.contains_key(&key);
        if !key_exists && self.capabilities.len() >= self.capacity {
            return Err(CsnError::RegistryFull {
                capacity: self.capacity,
            });
        }

        // entry API 原子化 key 检查:Occupied → 覆盖,Vacant → 插入(容量已预检)
        match self.capabilities.entry(key.clone()) {
            Entry::Occupied(mut e) => {
                // key 已存在 → 覆盖(更新能力描述符),不改变 len
                e.insert(cap);
                Ok(())
            }
            Entry::Vacant(e) => {
                // 新 key → 容量已在 entry() 之前检查通过,直接插入
                e.insert(cap);
                // P0-14:更新LSH索引
                self.rebuild_lsh_index();
                Ok(())
            }
        }
    }

    /// P0-14:重建LSH索引
    ///
    /// 当能力数达到阈值时,重建LSH索引以加速查询。
    /// 重建在register_lock保护下执行,避免并发冲突。
    fn rebuild_lsh_index(&self) {
        let count = self.capabilities.len();
        if count < self.lsh_enable_threshold {
            return; // 未达阈值,不启用LSH
        }

        if let Ok(mut lsh_guard) = self.lsh_index.lock() {
            let mut lsh = LshIndex::new(8, 16, self.lsh_enable_threshold);
            for (idx, entry) in self.capabilities.iter().enumerate() {
                lsh.insert(idx, &entry.value().semantic_vector);
            }
            *lsh_guard = Some(lsh);
        }
    }

    /// 查找能力描述符 — O(1),返回 clone,更新命中/未命中计数
    pub fn get(&self, capability_id: &str) -> Option<CapabilityDescriptor> {
        match self.capabilities.get(capability_id) {
            Some(entry) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(entry.clone())
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// 查找替代候选 — 基于余弦相似度选 Top-K(排除自身)
    ///
    /// P0-14:智能路由 — 能力数 < lsh_enable_threshold 时线性扫描O(n),
    /// 能力数 ≥ lsh_enable_threshold 时LSH-ANN索引查询O(1)+O(k)。
    ///
    /// # 流程
    /// 1. 获取目标能力的语义向量(未找到则返回空 Vec,不计入未命中)
    /// 2. 若LSH索引已启用,使用LSH-ANN获取候选集,再精确计算相似度
    /// 3. 若LSH未启用,遍历所有其他能力计算余弦相似度(线性扫描)
    /// 4. 使用 `select_nth_unstable_by` 选 Top-K(降序,O(n))
    /// 5. 对 Top-K 内部排序(降序,O(K log K),K << n)
    /// 6. 按 rank 分配 tier:0=primary, 1=secondary, 2=tertiary(封顶)
    ///
    /// # 返回
    /// 按 `similarity_score` 降序排列的 Top-K 候选列表;若 `capability_id`
    /// 未注册,返回空 Vec。
    pub fn find_substitutes(
        &self,
        capability_id: &str,
        top_k: usize,
    ) -> Vec<SubstitutionCandidate> {
        // 获取目标向量(未注册则返回空)
        let target_vector = match self.capabilities.get(capability_id) {
            Some(entry) => entry.semantic_vector.clone(),
            None => return Vec::new(),
        };

        // P0-14:智能路由 — LSH索引启用且能力数足够时使用ANN查询
        let mut scored: Vec<(f32, String)> = if let Ok(lsh_guard) = self.lsh_index.lock() {
            if let Some(lsh) = lsh_guard.as_ref() {
                if lsh.is_enabled() {
                    // LSH-ANN路径:获取候选集,再精确计算相似度
                    let candidates = lsh.query(&target_vector, 2); // 2 probes
                    candidates
                        .into_iter()
                        .filter_map(|idx| {
                            self.capabilities.iter().nth(idx).map(|entry| {
                                if entry.key() != capability_id {
                                    let score = cosine_similarity(
                                        &target_vector,
                                        &entry.value().semantic_vector,
                                    );
                                    Some((score.clamp(0.0, 1.0), entry.key().clone()))
                                } else {
                                    None
                                }
                            })?
                        })
                        .collect()
                } else {
                    Vec::new() // LSH未启用,回退到线性扫描
                }
            } else {
                Vec::new() // LSH未初始化,回退到线性扫描
            }
        } else {
            Vec::new() // 锁获取失败,回退到线性扫描
        };

        // 若LSH路径未返回结果(未启用/空候选),回退到线性扫描
        if scored.is_empty() {
            scored = self
                .capabilities
                .iter()
                .filter(|r| r.key() != capability_id)
                .map(|r| {
                    let score = cosine_similarity(&target_vector, &r.value().semantic_vector);
                    (score, r.key().clone())
                })
                .collect();
        }

        if scored.is_empty() || top_k == 0 {
            return Vec::new();
        }

        // Top-K 选择:使用 select_nth_unstable_by(O(n) 平均复杂度)
        let k = top_k.min(scored.len());
        let top = select_top_k_desc(&mut scored, k);

        // Top-K 内部降序排序(O(K log K),K << n,可接受)
        // WHY:select_nth_unstable 仅保证前 K 个是最大的,但不保证顺序;
        // 返回前需排序以便调用方按相似度降序消费
        top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // 映射为 SubstitutionCandidate,按 rank 分配 tier
        top.iter()
            .enumerate()
            .map(|(rank, (score, id))| SubstitutionCandidate {
                candidate_id: id.clone(),
                similarity_score: *score,
                // tier 封顶在 2(tertiary):rank 0→tier 0, rank 1→tier 1, rank 2+→tier 2
                tier: rank.min(2) as u32,
            })
            .collect()
    }

    /// 当前注册能力数
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// 缓存容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 命中次数(监控指标)
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }

    /// 未命中次数(监控指标)
    pub fn misses(&self) -> usize {
        self.misses.load(Ordering::Relaxed)
    }

    /// 获取监控统计快照
    pub fn stats(&self) -> SubstitutionRegistryStats {
        SubstitutionRegistryStats {
            len: self.capabilities.len(),
            capacity: self.capacity,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }
}

/// Top-K 降序选择 — 使用 `select_nth_unstable_by` 实现 O(n) 平均复杂度
///
/// 返回前 `k` 个相似度最高的元素(未完全排序,但保证是最大的 K 个)。
/// 调用方负责对返回的切片进行排序以获得降序顺序。
fn select_top_k_desc(scored: &mut [(f32, String)], k: usize) -> &mut [(f32, String)] {
    if k >= scored.len() {
        return scored;
    }
    let idx = k - 1;
    // 降序:b.0 vs a.0(大的在前)
    scored.select_nth_unstable_by(idx, |a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    &mut scored[..k]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    // === 辅助函数 ===

    fn make_descriptor(id: &str, vector: Vec<f32>) -> CapabilityDescriptor {
        CapabilityDescriptor::new(id, vector)
    }

    fn make_registry_with_caps(caps: Vec<(&str, Vec<f32>)>) -> SubstitutionCandidateRegistry {
        let registry = SubstitutionCandidateRegistry::new(100);
        for (id, v) in caps {
            registry.register(make_descriptor(id, v)).expect("注册失败");
        }
        registry
    }

    // === 1. 注册与查找基础 ===

    #[test]
    fn test_register_and_get() {
        let registry = SubstitutionCandidateRegistry::new(16);
        let cap = make_descriptor("cap-1", vec![1.0; 50]);
        registry.register(cap.clone()).expect("注册失败");

        let found = registry.get("cap-1").expect("应找到能力");
        assert_eq!(found.capability_id, "cap-1");
        assert_eq!(registry.hits(), 1);
    }

    #[test]
    fn test_get_not_found_updates_misses() {
        let registry = SubstitutionCandidateRegistry::new(16);
        assert!(registry.get("missing").is_none());
        assert_eq!(registry.misses(), 1);
        assert_eq!(registry.hits(), 0);
    }

    // === 2. 注册校验 ===

    #[test]
    fn test_register_empty_id_returns_error() {
        let registry = SubstitutionCandidateRegistry::new(16);
        let cap = make_descriptor("", vec![1.0; 50]);
        let result = registry.register(cap);
        assert!(matches!(result, Err(CsnError::InvalidCapability { .. })));
    }

    #[test]
    fn test_register_capacity_full_returns_error() {
        let registry = SubstitutionCandidateRegistry::new(2);
        registry
            .register(make_descriptor("cap-1", vec![1.0; 50]))
            .expect("注册 cap-1 失败");
        registry
            .register(make_descriptor("cap-2", vec![1.0; 50]))
            .expect("注册 cap-2 失败");

        let result = registry.register(make_descriptor("cap-3", vec![1.0; 50]));
        assert!(matches!(result, Err(CsnError::RegistryFull { .. })));
    }

    #[test]
    fn test_register_overwrite_existing() {
        let registry = SubstitutionCandidateRegistry::new(16);
        registry
            .register(make_descriptor("cap-1", vec![1.0; 50]))
            .expect("首次注册失败");
        // 覆盖:更新向量
        registry
            .register(make_descriptor("cap-1", vec![0.5; 50]))
            .expect("覆盖注册失败");

        let found = registry.get("cap-1").expect("应找到能力");
        assert_eq!(registry.len(), 1, "覆盖不应增加条目数");
        assert!((found.semantic_vector[0] - 0.5).abs() < 1e-6);
    }

    // === 3. find_substitutes Top-K 选择 ===

    #[test]
    fn test_find_substitutes_excludes_self() {
        let registry =
            make_registry_with_caps(vec![("cap-1", vec![1.0; 50]), ("cap-2", vec![0.9; 50])]);

        let candidates = registry.find_substitutes("cap-1", 5);
        assert_eq!(candidates.len(), 1, "仅 cap-2 是候选(排除自身)");
        assert_eq!(candidates[0].candidate_id, "cap-2");
    }

    #[test]
    fn test_find_substitutes_unregistered_returns_empty() {
        let registry = SubstitutionCandidateRegistry::new(16);
        let candidates = registry.find_substitutes("missing", 5);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_find_substitutes_top_k_ordering() {
        // 三个候选:cap-2 最相似(0.99),cap-3 次之(0.9),cap-4 最远(0.5)
        let registry = make_registry_with_caps(vec![
            ("cap-1", vec![1.0; 50]),
            ("cap-2", vec![0.99; 50]),
            ("cap-3", vec![0.9; 50]),
            ("cap-4", vec![0.5; 50]),
        ]);

        let candidates = registry.find_substitutes("cap-1", 3);
        assert_eq!(candidates.len(), 3);
        // 应按相似度降序排列
        assert_eq!(candidates[0].candidate_id, "cap-2");
        assert_eq!(candidates[1].candidate_id, "cap-3");
        assert_eq!(candidates[2].candidate_id, "cap-4");
        // 相似度应递减
        assert!(candidates[0].similarity_score >= candidates[1].similarity_score);
        assert!(candidates[1].similarity_score >= candidates[2].similarity_score);
    }

    #[test]
    fn test_find_substitutes_top_k_limit() {
        let registry = make_registry_with_caps(vec![
            ("cap-1", vec![1.0; 50]),
            ("cap-2", vec![0.9; 50]),
            ("cap-3", vec![0.8; 50]),
            ("cap-4", vec![0.7; 50]),
        ]);

        let candidates = registry.find_substitutes("cap-1", 2);
        assert_eq!(candidates.len(), 2, "应只返回 Top-2");
    }

    #[test]
    fn test_find_substitutes_top_k_zero() {
        let registry =
            make_registry_with_caps(vec![("cap-1", vec![1.0; 50]), ("cap-2", vec![0.9; 50])]);

        let candidates = registry.find_substitutes("cap-1", 0);
        assert!(candidates.is_empty(), "top_k=0 应返回空");
    }

    #[test]
    fn test_find_substitutes_top_k_exceeds_available() {
        let registry =
            make_registry_with_caps(vec![("cap-1", vec![1.0; 50]), ("cap-2", vec![0.9; 50])]);

        let candidates = registry.find_substitutes("cap-1", 100);
        assert_eq!(candidates.len(), 1, "仅 1 个候选可用");
    }

    // === 4. tier 分配 ===

    #[test]
    fn test_find_substitutes_tier_assignment() {
        let registry = make_registry_with_caps(vec![
            ("cap-1", vec![1.0; 50]),
            ("cap-2", vec![0.99; 50]),
            ("cap-3", vec![0.9; 50]),
            ("cap-4", vec![0.8; 50]),
            ("cap-5", vec![0.7; 50]),
        ]);

        let candidates = registry.find_substitutes("cap-1", 5);
        assert_eq!(candidates.len(), 4);
        // tier 分配:rank 0→0, rank 1→1, rank 2→2, rank 3→2(封顶)
        assert_eq!(candidates[0].tier, 0, "rank 0 → tier 0 (primary)");
        assert_eq!(candidates[1].tier, 1, "rank 1 → tier 1 (secondary)");
        assert_eq!(candidates[2].tier, 2, "rank 2 → tier 2 (tertiary)");
        assert_eq!(candidates[3].tier, 2, "rank 3 → tier 2 (capped)");
    }

    // === 5. select_top_k_desc 单元测试 ===

    #[test]
    fn test_select_top_k_desc() {
        let mut scored = vec![
            (0.3_f32, "a".into()),
            (0.9_f32, "b".into()),
            (0.5_f32, "c".into()),
            (0.1_f32, "d".into()),
        ];
        let top = select_top_k_desc(&mut scored, 2);
        assert_eq!(top.len(), 2);
        // 前 2 个应是最大的两个(0.9 和 0.5),但顺序不保证
        let scores: Vec<f32> = top.iter().map(|(s, _)| *s).collect();
        assert!(scores.contains(&0.9));
        assert!(scores.contains(&0.5));
    }

    #[test]
    fn test_select_top_k_desc_k_exceeds_len() {
        let mut scored = vec![(0.5_f32, "a".into())];
        let top = select_top_k_desc(&mut scored, 10);
        assert_eq!(top.len(), 1, "k > len 时返回全部");
    }

    // === 6. 监控统计 ===

    #[test]
    fn test_stats_snapshot() {
        let registry =
            make_registry_with_caps(vec![("cap-1", vec![1.0; 50]), ("cap-2", vec![0.9; 50])]);
        let _ = registry.get("cap-1"); // hit
        let _ = registry.get("missing"); // miss

        let stats = registry.stats();
        assert_eq!(stats.len, 2);
        assert_eq!(stats.capacity, 100);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_stats_hit_rate_zero_when_no_queries() {
        let registry = SubstitutionCandidateRegistry::new(16);
        let stats = registry.stats();
        assert_eq!(stats.hit_rate(), 0.0);
    }

    // === 7. 并发安全 ===

    #[test]
    fn test_concurrent_register_and_find() {
        let registry = std::sync::Arc::new(SubstitutionCandidateRegistry::new(100));
        let mut handles = Vec::new();

        // 多线程并发注册不同能力
        for i in 0..20 {
            let reg = std::sync::Arc::clone(&registry);
            handles.push(thread::spawn(move || {
                let id = format!("cap-{i}");
                let v: Vec<f32> = (0..50).map(|_| (i as f32) * 0.05).collect();
                reg.register(make_descriptor(&id, v)).expect("并发注册失败");
            }));
        }
        for h in handles {
            h.join().expect("线程 panic");
        }

        assert_eq!(registry.len(), 20);

        // 并发查找替代
        let mut read_handles = Vec::new();
        for i in 0..20 {
            let reg = std::sync::Arc::clone(&registry);
            read_handles.push(thread::spawn(move || {
                let id = format!("cap-{i}");
                reg.find_substitutes(&id, 5)
            }));
        }
        for h in read_handles {
            let candidates = h.join().expect("线程 panic");
            assert!(candidates.len() <= 5);
        }
    }

    // === 8. TOCTOU 竞态修复验证(B-Maj-1)===
    //
    // 验证 register() 在高并发下:
    // 1. 不同 key 不超过容量上限(len <= capacity)
    // 2. 相同 key 不产生重复条目(len == 1)
    //
    // 使用 Barrier 同步所有线程同时启动,最大化竞态窗口。

    #[test]
    fn test_concurrent_register_unique_keys_respects_capacity() {
        // 100 线程并发注册不同 key,容量上限 50
        // 验证:注册表条目数严格不超过容量上限
        let capacity = 50;
        let registry = std::sync::Arc::new(SubstitutionCandidateRegistry::new(capacity));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(100));
        let mut handles = Vec::new();

        for i in 0..100 {
            let reg = std::sync::Arc::clone(&registry);
            let b = std::sync::Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                // Barrier 确保所有线程同时开始,最大化竞态窗口
                b.wait();
                let id = format!("cap-{i}");
                // 忽略 RegistryFull 错误(容量满后预期返回)
                let _ = reg.register(make_descriptor(&id, vec![1.0; 50]));
            }));
        }
        for h in handles {
            h.join().expect("线程 panic");
        }

        assert!(
            registry.len() <= capacity,
            "注册表不应超过容量上限: {} > {}",
            registry.len(),
            capacity
        );
        assert_eq!(
            registry.len(),
            capacity,
            "应注册满容量(100 线程竞争 50 槽位)"
        );
    }

    #[test]
    fn test_concurrent_register_same_key_no_duplicate() {
        // 100 线程并发注册相同 key
        // 验证:最终只有 1 个条目(无重复插入)
        let registry = std::sync::Arc::new(SubstitutionCandidateRegistry::new(100));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(100));
        let mut handles = Vec::new();

        for _ in 0..100 {
            let reg = std::sync::Arc::clone(&registry);
            let b = std::sync::Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                b.wait();
                reg.register(make_descriptor("same-cap", vec![1.0; 50]))
                    .expect("注册失败");
            }));
        }
        for h in handles {
            h.join().expect("线程 panic");
        }

        assert_eq!(registry.len(), 1, "相同 key 并发注册不应产生重复条目");
    }
}
