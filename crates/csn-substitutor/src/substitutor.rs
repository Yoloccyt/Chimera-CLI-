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

use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::CsnError;
use crate::similarity::cosine_similarity;
use crate::types::{CapabilityDescriptor, SubstitutionCandidate};

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
}

impl SubstitutionCandidateRegistry {
    /// 创建指定容量的注册表
    pub fn new(capacity: usize) -> Self {
        Self {
            capabilities: DashMap::new(),
            capacity,
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
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

        // 已存在则覆盖(更新能力描述符)
        if self.capabilities.contains_key(&key) {
            self.capabilities.insert(key, cap);
            return Ok(());
        }

        // 新增:检查容量上限
        if self.capabilities.len() >= self.capacity {
            return Err(CsnError::RegistryFull {
                capacity: self.capacity,
            });
        }

        self.capabilities.insert(key, cap);
        Ok(())
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
    /// # 流程
    /// 1. 获取目标能力的语义向量(未找到则返回空 Vec,不计入未命中)
    /// 2. 遍历所有其他能力,计算余弦相似度
    /// 3. 使用 `select_nth_unstable_by` 选 Top-K(降序,O(n))
    /// 4. 对 Top-K 内部排序(降序,O(K log K),K << n)
    /// 5. 按 rank 分配 tier:0=primary, 1=secondary, 2=tertiary(封顶)
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

        // 收集所有其他能力的 (similarity, candidate_id)
        let mut scored: Vec<(f32, String)> = self
            .capabilities
            .iter()
            .filter(|r| r.key() != capability_id)
            .map(|r| {
                let score = cosine_similarity(&target_vector, &r.value().semantic_vector);
                (score, r.key().clone())
            })
            .collect();

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
}
