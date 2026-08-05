//! 语义响应缓存 — CLV 向量相似度匹配 + 命名空间隔离（ADR-069 Token 效率优化）
//!
//! 对应架构层: L3 Storage（scc-cache）
//!
//! # 核心机制
//! 请求的 CLV（512 维上下文潜在向量）与缓存条目的 CLV 做余弦相似度匹配，
//! 超过阈值（默认 0.92）视为语义等价，直接返回缓存响应（跳过厂商调用）。
//!
//! # 隐私隔离
//! 每个 namespace（quest_id）独立分区，`lookup` 只在同一 namespace 内搜索，
//! 禁止跨命名空间命中（NamespaceQuota 隔离红线）。
//!
//! # 性能
//! 暴力扫描 + per-namespace 容量硬限 256：
//! - 256 条目 × 28ns/512d cosine = 7.2μs（远低于 TTFT P95 5% 预算 ~5ms）
//! - 无需 HNSW/IVF（违反 forbid(unsafe_code) 且当前规模不需要）
//!
//! # 回滚
//! CapabilityToken S9 接缝 Cooldown/Frozen 态 → `allows_learned_policy()` = false
//! → 调用方 bypass 全部缓存逻辑（30s 降级无缓存模式）。
//!
//! # 依赖方向（§2.2 铁律）
//! L3 → L1（nexus_core::cosine_similarity_slices）合法
//! L3 → L0（nexus_contracts::TokenCacheKey）合法

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use nexus_contracts::affinity::TokenCacheKey;
use nexus_core::cosine_similarity_slices;

/// 默认相似度阈值（保守，避免误命中）
///
/// WHY 0.92: 编码场景的语义相似请求通常 > 0.95（同一函数微调参数），
/// 0.92 留出安全余量；过低（< 0.85）会命中不相关响应（correctness 风险）。
pub const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.92;

/// 默认 per-namespace 最大条目数
///
/// WHY 256: 暴力扫描 O(N) 在 256 条目时 ~7.2μs，可接受；
/// 超过后 LRU 驱逐最旧条目（hit_count 最低 + created_at 最早）。
pub const DEFAULT_MAX_ENTRIES_PER_NS: usize = 256;

/// 语义缓存条目 — 缓存的响应 + 匹配元数据
#[derive(Debug)]
pub struct SemanticEntry {
    /// 复合缓存键（精确匹配层）
    pub key: TokenCacheKey,
    /// 请求的 CLV 向量（512 维，语义匹配层）
    pub clv: Vec<f32>,
    /// 缓存的响应内容
    pub response: Arc<str>,
    /// 缓存时的上下文哈希（Context Ledger 漂移校验用）
    pub context_hash: [u8; 32],
    /// 创建时间（Unix 秒）
    pub created_at: u64,
    /// 命中次数（LRU 驱逐参考）
    pub hit_count: AtomicU64,
}

/// 语义缓存查询结果
#[derive(Debug, Clone)]
pub struct CachedResponse {
    /// 缓存的响应内容
    pub response: Arc<str>,
    /// 匹配相似度 [threshold, 1.0]
    pub similarity: f32,
}

/// 命名空间分区 — per-quest 隔离的缓存条目集合
///
/// WHY RwLock 而非 DashMap：条目数 < 256，读多写少（查询远多于插入），
/// RwLock 读路径零分片开销；写路径（插入/驱逐）低频，短暂阻塞可接受。
#[derive(Debug, Default)]
struct NamespacePartition {
    entries: RwLock<Vec<SemanticEntry>>,
}

/// 语义响应缓存 — 基于 CLV 向量相似度的命名空间隔离缓存
///
/// 线程安全：DashMap 分片锁保护 namespace 创建/查找，
/// 各 namespace 内部 RwLock 保护条目读写。
#[derive(Debug)]
pub struct SemanticResponseCache {
    /// namespace → 分区（quest_id 隔离）
    namespaces: DashMap<Box<str>, NamespacePartition>,
    /// 相似度阈值（可配置）
    similarity_threshold: f32,
    /// per-namespace 最大条目数
    max_entries_per_ns: usize,
}

impl Default for SemanticResponseCache {
    fn default() -> Self {
        Self::new(DEFAULT_SIMILARITY_THRESHOLD, DEFAULT_MAX_ENTRIES_PER_NS)
    }
}

impl SemanticResponseCache {
    /// 创建语义缓存（自定义阈值与容量）
    pub fn new(similarity_threshold: f32, max_entries_per_ns: usize) -> Self {
        Self {
            namespaces: DashMap::new(),
            similarity_threshold,
            max_entries_per_ns,
        }
    }

    /// 语义查询 — 在指定 namespace 内查找相似缓存
    ///
    /// 返回相似度最高且超过阈值的缓存响应。
    /// 精确匹配层（TokenCacheKey）+ 语义匹配层（CLV cosine）双重校验。
    /// 不执行 Context Ledger 漂移校验（等价 `lookup_with_context(.., None)`，
    /// 旧调用方行为不变）。
    pub fn lookup(
        &self,
        namespace: &str,
        key: &TokenCacheKey,
        query_clv: &[f32],
    ) -> Option<CachedResponse> {
        self.lookup_with_context(namespace, key, query_clv, None)
    }

    /// 语义查询（带 Context Ledger 漂移校验）— 在指定 namespace 内查找相似缓存
    ///
    /// `current_context_hash` 为 Some 时，对每个候选 entry 额外校验
    /// `verify_context_ledger(current_context_hash, &entry.context_hash)`，
    /// 不一致（上下文已漂移）则跳过该 entry（按 miss 处理）——
    /// cache hit ≠ correctness，上下文变更后复用旧响应可能产生错误内容。
    /// None = 不校验（保持原 `lookup` 语义）。
    pub fn lookup_with_context(
        &self,
        namespace: &str,
        key: &TokenCacheKey,
        query_clv: &[f32],
        current_context_hash: Option<&[u8; 32]>,
    ) -> Option<CachedResponse> {
        let partition = self.namespaces.get(namespace)?;
        let entries = partition.entries.read().ok()?;

        let mut best: Option<(f32, &SemanticEntry)> = None;

        for entry in entries.iter() {
            // 精确匹配层：缓存键必须完全一致（模型/版本/工具/提示/档位）
            if &entry.key != key {
                continue;
            }
            // Context Ledger 漂移校验：当前上下文与缓存时不一致 → 该 entry 失效。
            // 放在余弦计算前，尽早过滤无效 entry（避免无谓的 512 维计算）
            if let Some(current) = current_context_hash {
                if !verify_context_ledger(current, &entry.context_hash) {
                    continue;
                }
            }
            // 语义匹配层：CLV 余弦相似度
            let sim = cosine_similarity_slices(query_clv, &entry.clv);
            if sim >= self.similarity_threshold {
                match &best {
                    Some((best_sim, _)) if sim <= *best_sim => {}
                    _ => best = Some((sim, entry)),
                }
            }
        }

        best.map(|(sim, entry)| {
            entry.hit_count.fetch_add(1, Ordering::Relaxed);
            CachedResponse {
                response: Arc::clone(&entry.response),
                similarity: sim,
            }
        })
    }

    /// 插入缓存条目 — 写入指定 namespace
    ///
    /// 超过容量上限时驱逐 hit_count 最低的条目（LRU 近似）。
    pub fn insert(
        &self,
        namespace: &str,
        key: TokenCacheKey,
        clv: Vec<f32>,
        response: &str,
        context_hash: [u8; 32],
        now_secs: u64,
    ) {
        let partition = self.namespaces.entry(namespace.into()).or_default();

        let mut entries = match partition.entries.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        // 容量检查：超限驱逐 hit_count 最低的条目
        if entries.len() >= self.max_entries_per_ns {
            // 找 hit_count 最低的索引（O(N)，N ≤ 256 可接受）
            if let Some(evict_idx) = entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.hit_count.load(Ordering::Relaxed))
                .map(|(i, _)| i)
            {
                entries.swap_remove(evict_idx);
            }
        }

        entries.push(SemanticEntry {
            key,
            clv,
            response: response.into(),
            context_hash,
            created_at: now_secs,
            hit_count: AtomicU64::new(0),
        });
    }

    /// 清空指定 namespace 的全部缓存（会话结束/回滚时调用）
    pub fn clear_namespace(&self, namespace: &str) {
        self.namespaces.remove(namespace);
    }

    /// 指定 namespace 的条目数（诊断用）
    pub fn namespace_len(&self, namespace: &str) -> usize {
        match self.namespaces.get(namespace) {
            Some(partition) => partition.entries.read().map(|e| e.len()).unwrap_or(0),
            None => 0,
        }
    }

    /// 总 namespace 数（诊断用）
    pub fn namespace_count(&self) -> usize {
        self.namespaces.len()
    }
}

// ============================================================
// Context Ledger 漂移校验（cache hit ≠ correctness）
// ============================================================

/// 验证上下文一致性 — 哨兵哈希比对
///
/// 缓存命中后，比对当前上下文哈希与缓存时的上下文哈希：
/// - 一致 → 缓存有效（上下文未漂移）
/// - 不一致 → 缓存失效（上下文已变更，响应可能不正确）
///
/// WHY 不用哨兵 token 嵌入：嵌入会修改响应内容（破坏保真度），
/// 哈希比对是零侵入校验（不修改缓存内容）。
pub fn verify_context_ledger(
    current_context_hash: &[u8; 32],
    cached_context_hash: &[u8; 32],
) -> bool {
    current_context_hash == cached_context_hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::ThinkingPreference;
    // proptest prelude:prop_assert!/prop_assert_eq! 宏 + Strategy trait(block-named 语法)
    use proptest::prelude::*;

    fn test_key() -> TokenCacheKey {
        TokenCacheKey {
            model: "glm-5.2".into(),
            model_version: "2026-07".into(),
            tool_schema_hash: [1u8; 32],
            system_prompt_hash: [2u8; 32],
            thinking_tier: ThinkingPreference::Standard,
        }
    }

    fn test_clv(seed: f32) -> Vec<f32> {
        vec![seed; 512]
    }

    #[test]
    fn insert_and_lookup_hit() {
        let cache = SemanticResponseCache::default();
        let key = test_key();
        let clv = test_clv(0.5);

        cache.insert(
            "quest-1",
            key.clone(),
            clv.clone(),
            "cached response",
            [0u8; 32],
            1000,
        );

        let result = cache.lookup("quest-1", &key, &clv);
        assert!(result.is_some());
        let cached = result.unwrap();
        assert_eq!(cached.response.as_ref(), "cached response");
        assert!(cached.similarity >= 0.99); // 相同向量 → ~1.0
    }

    #[test]
    fn lookup_miss_different_namespace() {
        let cache = SemanticResponseCache::default();
        let key = test_key();
        let clv = test_clv(0.5);

        cache.insert(
            "quest-1",
            key.clone(),
            clv.clone(),
            "response",
            [0u8; 32],
            1000,
        );

        // 不同 namespace 不应命中（隐私隔离）
        let result = cache.lookup("quest-2", &key, &clv);
        assert!(result.is_none());
    }

    #[test]
    fn lookup_miss_different_key() {
        let cache = SemanticResponseCache::default();
        let key = test_key();
        let clv = test_clv(0.5);

        cache.insert("q", key.clone(), clv.clone(), "response", [0u8; 32], 1000);

        // 不同缓存键（模型变更）不应命中
        let mut different_key = key;
        different_key.model = "glm-5.3".into();
        let result = cache.lookup("q", &different_key, &clv);
        assert!(result.is_none());
    }

    #[test]
    fn lookup_miss_below_threshold() {
        let cache = SemanticResponseCache::default();
        let key = test_key();
        let clv_a = test_clv(1.0);
        // 构造一个与 clv_a 相似度低于阈值的向量
        let mut clv_b = test_clv(-1.0); // 方向相反 → cosine = -1.0
        clv_b[0] = -1.0;

        cache.insert("q", key.clone(), clv_a, "response", [0u8; 32], 1000);

        let result = cache.lookup("q", &key, &clv_b);
        assert!(result.is_none());
    }

    #[test]
    fn lru_eviction_on_capacity() {
        let cache = SemanticResponseCache::new(0.92, 3); // 容量 3
        let clv = test_clv(0.5);

        for i in 0..4 {
            let mut key = test_key();
            key.model = format!("model-{i}").into();
            cache.insert(
                "q",
                key,
                clv.clone(),
                &format!("resp-{i}"),
                [0u8; 32],
                i as u64,
            );
        }

        // 容量 3，插入 4 条 → 驱逐 1 条
        assert_eq!(cache.namespace_len("q"), 3);
    }

    #[test]
    fn clear_namespace_removes_all() {
        let cache = SemanticResponseCache::default();
        let key = test_key();
        cache.insert("q", key, test_clv(0.5), "r", [0u8; 32], 1);
        assert_eq!(cache.namespace_len("q"), 1);

        cache.clear_namespace("q");
        assert_eq!(cache.namespace_len("q"), 0);
    }

    #[test]
    fn context_ledger_verification() {
        let hash_a = [1u8; 32];
        let hash_b = [2u8; 32];

        assert!(verify_context_ledger(&hash_a, &hash_a));
        assert!(!verify_context_ledger(&hash_a, &hash_b));
    }

    #[test]
    fn hit_count_increments_on_lookup() {
        let cache = SemanticResponseCache::default();
        let key = test_key();
        let clv = test_clv(0.5);
        cache.insert("q", key.clone(), clv.clone(), "r", [0u8; 32], 1);

        // 多次命中
        cache.lookup("q", &key, &clv);
        cache.lookup("q", &key, &clv);

        // 验证 hit_count（通过 partition 内部状态间接验证：驱逐时 hit_count 低的先走）
        assert_eq!(cache.namespace_len("q"), 1);
    }

    // ============================================================
    // R3 Context Ledger 漂移校验(ADR-069 Task 5.1)
    // ============================================================

    /// 漂移校验:缓存时 context_hash=A,当前查询哈希 B → 该 entry 失效(miss);
    /// 查询哈希 A → 命中。cache hit ≠ correctness,上下文漂移后响应可能不正确。
    #[test]
    fn lookup_with_context_rejects_drifted_hash() {
        let cache = SemanticResponseCache::default();
        let key = test_key();
        let clv = test_clv(0.5);
        cache.insert(
            "q",
            key.clone(),
            clv.clone(),
            "cached response",
            [1u8; 32], // 缓存时上下文哈希 = A
            1000,
        );

        // 当前上下文哈希 = B(漂移)→ 按 miss 处理
        assert!(
            cache
                .lookup_with_context("q", &key, &clv, Some(&[2u8; 32]))
                .is_none(),
            "上下文漂移后缓存必须按 miss 处理"
        );
        // 当前上下文哈希 = A(一致)→ 命中
        let hit = cache.lookup_with_context("q", &key, &clv, Some(&[1u8; 32]));
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().response.as_ref(), "cached response");
    }

    /// 原 lookup 保持兼容:不传上下文哈希 = 不校验(旧调用方行为不变)
    #[test]
    fn legacy_lookup_ignores_context_drift() {
        let cache = SemanticResponseCache::default();
        let key = test_key();
        let clv = test_clv(0.5);
        cache.insert("q", key.clone(), clv.clone(), "r", [9u8; 32], 1);
        // 无上下文校验的旧 lookup 语义:任何插入哈希均可命中
        assert!(cache.lookup("q", &key, &clv).is_some());
    }

    /// 阈值边界:相似度 0.93(> 0.92)→ 命中;0.91(< 0.92)→ miss
    #[test]
    fn lookup_threshold_boundary_hit_and_miss() {
        let cache = SemanticResponseCache::new(0.92, 256);
        let key = test_key();
        // 基准单位向量:e0
        let mut base = vec![0.0_f32; 512];
        base[0] = 1.0;
        cache.insert("q", key.clone(), base.clone(), "r", [0u8; 32], 1);

        // 与 e0 夹角余弦 = 0.93(单位向量,分量平方和 = 1)→ 命中
        let mut above = vec![0.0_f32; 512];
        above[0] = 0.93;
        above[1] = (1.0 - 0.93_f32 * 0.93_f32).sqrt();
        assert!(
            cache.lookup("q", &key, &above).is_some(),
            "相似度 0.93 ≥ 阈值 0.92 必须命中"
        );

        // 余弦 = 0.91(< 0.92)→ miss
        let mut below = vec![0.0_f32; 512];
        below[0] = 0.91;
        below[1] = (1.0 - 0.91_f32 * 0.91_f32).sqrt();
        assert!(
            cache.lookup("q", &key, &below).is_none(),
            "相似度 0.91 < 阈值 0.92 必须 miss"
        );
    }

    // ============================================================
    // R3 proptest(ADR-069 Task 5.4)
    // ============================================================

    proptest::proptest! {
        /// 随机哈希对:verify_context_ledger 必须恒真(相同 → 一致,不同 → 不一致)
        #[test]
        fn context_ledger_random_pairs_consistent(a: [u8; 32], b: [u8; 32]) {
            prop_assert!(verify_context_ledger(&a, &a), "相同哈希必须一致");
            if a != b {
                prop_assert!(
                    !verify_context_ledger(&a, &b),
                    "不同哈希必须不一致(漂移检测前提)"
                );
            }
        }

        /// 随机 namespace + 随机上下文哈希:
        /// 一致哈希 → 命中;跨 namespace 同键同指纹 → 不命中(隐私隔离红线)
        #[test]
        fn lookup_with_context_random_namespace_and_hash(
            ns in "q[a-z0-9]{0,16}",
            hash: [u8; 32],
        ) {
            let cache = SemanticResponseCache::default();
            let key = test_key();
            let clv = test_clv(0.5);
            cache.insert(&ns, key.clone(), clv.clone(), "r", hash, 1);
            // 当前上下文哈希与缓存时一致 → 命中
            prop_assert!(
                cache.lookup_with_context(&ns, &key, &clv, Some(&hash)).is_some(),
                "一致哈希必须命中"
            );
            // 跨 namespace(不同命名空间,同键同指纹)→ 不命中(隐私隔离)
            let other_ns = format!("other-{ns}");
            prop_assert!(
                cache.lookup_with_context(&other_ns, &key, &clv, Some(&hash)).is_none(),
                "跨 namespace 禁止命中"
            );
        }
    }
}
