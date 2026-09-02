//! CheapIndex — TIE-SWA 廉价索引全量打分 + 共享复用 + Shadow 预热（P3-T13，v4.0 WI-26）
//!
//! 对应架构层: **L6 Router**（osa-coordinator，ADR-151 裁决：挂既有 crate 增强）
//! 对应任务: **P3-T13a**（手册 W19，WI-26：TIE-SWA 两级评估阶段一）
//!
//! # 设计（v4.0 WI-26 规格）
//! - **廉价索引全量打分 → 深查 top-k** 统一范式:CheapIndex 对全部候选做浅层
//!   打分（O(n)）,top-k 进入深查（Selective 深打分）;
//! - **共享缓存**:相邻高相似查询共享一次选择结果（TTL + 相似度双闸——
//!   查询向量与缓存查询相似度 ≥ 闸值 且 未过期 → 直接复用,深查成本归零）;
//! - **Shadow 预热**:新打分器 Shadow 双跑（分桶一致率 ≥85% 且样本 ≥1000
//!   才转正;否则保持旧打分器）。
//!
//! # 门禁（WI-26）
//! 复用命中 ≥35%;命中率 <20% 自动退化逐查（回退路径）。

use std::time::{Duration, Instant};

/// 复用相似度闸值 — 查询相似度 ≥ 该值才复用缓存结果
pub const REUSE_SIMILARITY_GATE: f64 = 0.9;
/// Shadow 预热一致率门槛 — 分桶一致率 ≥85% 才可转正
pub const SHADOW_AGREEMENT_GATE: f64 = 0.85;
/// Shadow 预热最小样本 — ≥1000 才可转正（防小样本巧合）
pub const SHADOW_MIN_SAMPLES: u64 = 1_000;
/// 复用命中率退化阈值 — <20% 自动退化逐查（回退路径）
pub const REUSE_DEGRADE_RATE: f64 = 0.2;

/// 索引条目 — 廉价打分输入
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    /// 候选键
    pub key: String,
    /// 廉价分数（浅层特征,如前缀/哈希/统计）
    pub cheap_score: f64,
}

/// 查询 — 廉价打分 + 深查
#[derive(Debug, Clone, PartialEq)]
pub struct TieswaQuery {
    /// 查询键（去重/缓存命中判定）
    pub query_key: String,
    /// 查询特征向量（相似度判定;空 = 不可复用）
    pub features: Vec<f64>,
    /// 候选清单（深查输入）
    pub candidates: Vec<IndexEntry>,
}

/// 深查结果 — 选中 top-k（含深打分）
#[derive(Debug, Clone, PartialEq)]
pub struct DeepResult {
    /// 查询键
    pub query_key: String,
    /// 选中键（深打分降序 top-k）
    pub selected: Vec<String>,
    /// 是否缓存复用（未深查）
    pub cache_hit: bool,
}

/// 缓存条目 — 查询结果 + 特征 + TTL
#[derive(Debug, Clone)]
struct CacheEntry {
    /// 结果
    result: DeepResult,
    /// 查询特征（相似度判定）
    features: Vec<f64>,
    /// 过期时刻
    expires_at: Instant,
    /// 命中计数（退化判定）
    hits: u64,
}

/// 共享结果缓存 — TTL + 相似度双闸
#[derive(Debug)]
pub struct SharedResultCache {
    /// query_key → 缓存条目
    entries: std::collections::HashMap<String, CacheEntry>,
    /// TTL（默认 60s;0 = 不过期）
    ttl: Duration,
    /// 相似度闸值
    similarity_gate: f64,
    /// 查询总数（命中率统计）
    total_queries: u64,
    /// 缓存命中数（命中率统计）
    cache_hits: u64,
}

impl Default for SharedResultCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(60), REUSE_SIMILARITY_GATE)
    }
}

impl SharedResultCache {
    /// 新建缓存
    #[must_use]
    pub fn new(ttl: Duration, similarity_gate: f64) -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            ttl,
            similarity_gate,
            total_queries: 0,
            cache_hits: 0,
        }
    }

    /// 查询 — 相似度双闸命中则复用;未命中返回 None（触发深查）
    #[must_use]
    pub fn lookup(&mut self, query: &TieswaQuery) -> Option<DeepResult> {
        self.total_queries += 1;
        // 过期清理（惰性:只清当前键）
        if let Some(e) = self.entries.get(&query.query_key) {
            if self.ttl > Duration::ZERO && e.expires_at <= Instant::now() {
                self.entries.remove(&query.query_key);
            }
        }
        // 相似度双闸:同键或特征相似 ≥ 闸值
        let hit = self.entries.get(&query.query_key).cloned().or_else(|| {
            self.entries
                .values()
                .find(|e| {
                    !e.features.is_empty()
                        && !query.features.is_empty()
                        && cosine_similarity(&e.features, &query.features) >= self.similarity_gate
                })
                .cloned()
        });
        match hit {
            Some(mut e) => {
                e.hits += 1;
                self.cache_hits += 1;
                // 克隆结果（e.result 移动后 e 仍要回写缓存）
                let mut result = e.result.clone();
                result.cache_hit = true;
                self.entries.insert(query.query_key.clone(), e);
                Some(result)
            }
            None => None,
        }
    }

    /// 写入深查结果（供后续查询复用）
    pub fn store(&mut self, query: &TieswaQuery, result: DeepResult) {
        self.entries.insert(
            query.query_key.clone(),
            CacheEntry {
                result,
                features: query.features.clone(),
                expires_at: Instant::now() + self.ttl,
                hits: 0,
            },
        );
    }

    /// 命中率（诊断;退化判定输入）
    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        if self.total_queries == 0 {
            return 0.0;
        }
        self.cache_hits as f64 / self.total_queries as f64
    }

    /// 缓存条目数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 空判定
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 余弦相似度 — 特征向量（零向量防御:返回 0）
#[must_use]
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na < 1e-12 || nb < 1e-12 {
        return 0.0;
    }
    dot / (na * nb)
}

/// TIE-SWA 两级评估器 — 廉价全量打分 + 共享复用 + 深查
#[derive(Debug)]
pub struct TieswaSelector {
    /// 共享缓存
    cache: SharedResultCache,
    /// Shadow 预热（新打分器验证）
    shadow: ShadowWarmup,
}

impl Default for TieswaSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl TieswaSelector {
    /// 新建评估器
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: SharedResultCache::default(),
            shadow: ShadowWarmup::new(),
        }
    }

    /// 选择 — 缓存命中则复用;未命中廉价打分 → 深查 top-k → 缓存
    ///
    /// # 参数
    /// - `query`:查询
    /// - `k`:top-k 数量
    /// - `deep_scorer`:深打分闭包（廉价 top-k 候选 → 深分数;None 时用廉价分）
    #[must_use]
    pub fn select(
        &mut self,
        query: &TieswaQuery,
        k: usize,
        deep_scorer: Option<&dyn Fn(&str) -> f64>,
    ) -> DeepResult {
        // 1. 缓存复用（TTL + 相似度双闸）
        if let Some(hit) = self.cache.lookup(query) {
            return hit;
        }
        // 2. 廉价全量打分 → top-k（O(n) select_nth_unstable 红线）
        let mut ranked: Vec<(f64, String)> = query
            .candidates
            .iter()
            .map(|c| (c.cheap_score, c.key.clone()))
            .collect();
        let kk = k.min(ranked.len());
        if kk > 0 {
            ranked.select_nth_unstable_by(kk - 1, |a, b| {
                b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            ranked.truncate(kk);
            ranked.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.1.cmp(&b.1))
            });
        }
        // 3. 深查（可选深打分;None = 廉价分即深分）
        let selected: Vec<String> = match deep_scorer {
            Some(scorer) => {
                let mut deep: Vec<(f64, String)> = ranked
                    .iter()
                    .map(|(_, key)| (scorer(key), key.clone()))
                    .collect();
                deep.sort_by(|a, b| {
                    b.0.partial_cmp(&a.0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(a.1.cmp(&b.1))
                });
                deep.into_iter().map(|(_, k)| k).collect()
            }
            None => ranked.into_iter().map(|(_, k)| k).collect(),
        };
        let result = DeepResult {
            query_key: query.query_key.clone(),
            selected,
            cache_hit: false,
        };
        // 4. 缓存复用（后续相似查询零深查）
        self.cache.store(query, result.clone());
        result
    }

    /// 命中率（诊断;命中率 <20% 自动退化逐查——调用方据此回退）
    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        self.cache.hit_rate()
    }

    /// Shadow 预热引用（新打分器验证）
    #[must_use]
    pub fn shadow(&self) -> &ShadowWarmup {
        &self.shadow
    }
}

/// Shadow 预热 — 新打分器分桶一致率统计（≥85% 且 ≥1000 样本才转正）
#[derive(Debug)]
pub struct ShadowWarmup {
    /// 一致样本数（新旧打分器同桶）
    agreed: u64,
    /// 总样本数
    total: u64,
}

impl Default for ShadowWarmup {
    fn default() -> Self {
        Self::new()
    }
}

impl ShadowWarmup {
    /// 新建预热器
    #[must_use]
    pub fn new() -> Self {
        Self {
            agreed: 0,
            total: 0,
        }
    }

    /// 记录一次 Shadow 双跑样本（新旧打分器分桶是否一致）
    pub fn record(&mut self, agreed: bool) {
        self.total += 1;
        if agreed {
            self.agreed += 1;
        }
    }

    /// 一致率（无样本 = 0）
    #[must_use]
    pub fn agreement_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.agreed as f64 / self.total as f64
    }

    /// 是否可转正 — 一致率 ≥85% 且样本 ≥1000（WI-26 门禁）
    #[must_use]
    pub fn can_promote(&self) -> bool {
        self.agreement_rate() >= SHADOW_AGREEMENT_GATE && self.total >= SHADOW_MIN_SAMPLES
    }

    /// 样本数
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(key: &str, features: Vec<f64>, n_candidates: usize) -> TieswaQuery {
        TieswaQuery {
            query_key: key.into(),
            features,
            candidates: (0..n_candidates)
                .map(|i| IndexEntry {
                    key: format!("c{i}"),
                    cheap_score: i as f64,
                })
                .collect(),
        }
    }

    /// 两级选择 — 廉价 top-k 正确（无缓存时深查）
    #[test]
    fn select_top_k() {
        let mut s = TieswaSelector::new();
        let q = query("q1", vec![1.0, 0.0], 10);
        let r = s.select(&q, 3, None);
        assert!(!r.cache_hit);
        assert_eq!(r.selected, vec!["c9", "c8", "c7"], "廉价分降序 top-3");
        assert_eq!(s.hit_rate(), 0.0);
    }

    /// 缓存复用 — 同键二次查询零深查（cache_hit=true）
    #[test]
    fn cache_reuse_same_key() {
        let mut s = TieswaSelector::new();
        let q = query("q1", vec![1.0, 0.0], 10);
        let first = s.select(&q, 3, None);
        assert!(!first.cache_hit);
        let second = s.select(&q, 3, None);
        assert!(second.cache_hit, "同键二次必须缓存命中");
        assert_eq!(second.selected, first.selected);
        assert!(s.hit_rate() > 0.0);
    }

    /// 相似度双闸 — 高相似查询复用,低相似不复用
    #[test]
    fn similarity_gate() {
        let mut s = TieswaSelector::new();
        let q1 = query("q1", vec![1.0, 0.0], 5);
        let _ = s.select(&q1, 2, None);
        // 高相似（0.999）→ 复用
        let q_sim = query("q1b", vec![0.999, 0.001], 5);
        let r = s.select(&q_sim, 2, None);
        assert!(r.cache_hit, "高相似必须复用");
        // 低相似（0.5）→ 不复用
        let q_diff = query("q1c", vec![0.5, 0.5], 5);
        let r2 = s.select(&q_diff, 2, None);
        assert!(!r2.cache_hit, "低相似必须深查");
    }

    /// TTL 过期 — 过期后重新深查
    #[test]
    fn ttl_expiry() {
        let mut cache = SharedResultCache::new(Duration::from_millis(20), REUSE_SIMILARITY_GATE);
        let q = query("q1", vec![1.0, 0.0], 5);
        let r = DeepResult {
            query_key: "q1".into(),
            selected: vec!["c4".into()],
            cache_hit: false,
        };
        cache.store(&q, r);
        assert!(cache.lookup(&q).is_some(), "未过期可复用");
        std::thread::sleep(Duration::from_millis(40));
        assert!(cache.lookup(&q).is_none(), "过期必须失效");
    }

    /// Shadow 预热 — 一致率与样本双门槛
    #[test]
    fn shadow_warmup_gates() {
        let mut sw = ShadowWarmup::new();
        assert!(!sw.can_promote(), "无样本不可转正");
        // 100% 一致但样本不足
        for _ in 0..500 {
            sw.record(true);
        }
        assert!(!sw.can_promote(), "样本 <1000 不可转正");
        // 补足样本（保持 100%）
        for _ in 0..500 {
            sw.record(true);
        }
        assert!(sw.can_promote(), "100% 一致 + 1000 样本可转正");
        // 一致率 <85% 拒绝
        let mut sw2 = ShadowWarmup::new();
        for i in 0..1000 {
            sw2.record(i % 2 == 0);
        }
        assert!(!sw2.can_promote(), "50% 一致不可转正");
    }

    /// 命中率退化 — <20% 触发（回退判定输入）
    #[test]
    fn hit_rate_degrade_signal() {
        let mut s = TieswaSelector::new();
        // 交替不同方向特征（余弦 < 0.9）:低命中
        for i in 0..10 {
            let angle = i as f64 * 0.5; // 方向差 > 26° → 余弦 < 0.9
            let q = query(&format!("q{i}"), vec![angle.cos(), angle.sin()], 5);
            let _ = s.select(&q, 2, None);
        }
        assert!(
            s.hit_rate() < REUSE_DEGRADE_RATE,
            "不同方向特征命中率应低（回退信号）: {}",
            s.hit_rate()
        );
    }

    /// 复用命中率 ≥35% — WI-26 门禁（相邻高相似查询共享）
    ///
    /// 20 个高相似查询（同一方向微扰）→ 首查询深查,其余缓存复用
    /// （余弦 ≥0.99 > 0.9 闸值）→ 命中率 ≥95% >> 35% 门禁。
    #[test]
    fn reuse_hit_rate_above_gate() {
        let mut s = TieswaSelector::new();
        let n = 20;
        for i in 0..n {
            // 同方向微扰:余弦 ≈ 0.999+（>> 0.9 闸值）
            let angle = i as f64 * 0.005;
            let q = query(&format!("q{i}"), vec![angle.cos(), angle.sin()], 8);
            let _ = s.select(&q, 3, None);
        }
        let rate = s.hit_rate();
        assert!(
            rate >= 0.35,
            "相邻高相似查询复用命中率必须 ≥35%,实测 {rate:.2}"
        );
        assert!(rate >= 0.9, "同方向微扰命中率应极高: {rate:.2}");
    }

    /// 余弦相似度 — 零向量/不同长度防御
    #[test]
    fn cosine_defensive() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-9);
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-9);
    }
}
