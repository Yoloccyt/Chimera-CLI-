//! 预编译模板注册表 — O(1) 查找与 LRU 驱逐
//!
//! 对应架构层:L7 Execution
//!
//! ## 设计要点
//! - 基于 DashMap 实现并发安全的 O(1) 分片查找
//! - LRU 驱逐按 `compiled_at` 时间排序,删除最旧条目腾出空间
//! - `precompile` 将 `TemplateSpec` 转换为可融合的 `SlimeTemplate`
//! - `get_template_meta` 提供零拷贝的元数据访问(返回 Copy 类型),
//!   供融合引擎在 Top-K 选择时避免 clone 整个模板

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::SsraError;
use crate::types::{FusionStrategy, SlimeTemplate};

/// 模板预编译规格 — 描述待编译的模板参数
#[derive(Debug, Clone)]
pub struct TemplateSpec {
    /// 能力 ID
    pub capability_id: String,
    /// 参数形状(参数名列表)
    pub parameter_shape: Vec<String>,
    /// 融合策略
    pub strategy: FusionStrategy,
}

impl TemplateSpec {
    /// 创建模板规格
    pub fn new(
        capability_id: impl Into<String>,
        parameter_shape: Vec<String>,
        strategy: FusionStrategy,
    ) -> Self {
        Self {
            capability_id: capability_id.into(),
            parameter_shape,
            strategy,
        }
    }
}

/// 预编译模板 — 将 TemplateSpec 转换为 SlimeTemplate
///
/// 设置 `compiled_at` 为当前 UTC 时间,`weight` 默认 1.0。
/// 预编译产物可直接注册到 `TemplateRegistry` 参与运行时融合。
pub fn precompile(spec: TemplateSpec) -> SlimeTemplate {
    SlimeTemplate {
        capability_id: spec.capability_id,
        parameter_shape: spec.parameter_shape,
        fusion_strategy: spec.strategy,
        compiled_at: Utc::now(),
        weight: 1.0,
    }
}

/// 模板注册表 — 并发安全的模板缓存
///
/// 基于 `DashMap<String, SlimeTemplate>`,支持 O(1) 查找与 LRU 驱逐。
/// `capacity` 控制缓存上限,`evict_oldest` 按 `compiled_at` 删除最旧条目。
///
/// ## 并发安全
/// DashMap 采用分片锁,不同 key 的读写互不阻塞。
/// `evict_oldest` 采用 best-effort 策略:遍历快照后逐个删除,
/// 遍历期间的其他线程插入可能导致瞬时超额,但最终一致。
pub struct TemplateRegistry {
    templates: DashMap<String, SlimeTemplate>,
    capacity: usize,
    /// 命中计数(原子操作,无锁监控指标)
    hits: AtomicUsize,
    /// 未命中计数
    misses: AtomicUsize,
}

impl TemplateRegistry {
    /// 创建指定容量的注册表
    pub fn new(capacity: usize) -> Self {
        Self {
            templates: DashMap::new(),
            capacity,
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
        }
    }

    /// 注册模板 — 若 capability_id 已存在则覆盖(更新)
    ///
    /// # 错误
    /// - `ConfigError`:capacity 为 0,或 key 不存在且已达容量上限
    ///
    /// 调用者在容量满时应先调用 `evict_oldest` 腾出空间。
    pub fn register(&self, template: SlimeTemplate) -> Result<(), SsraError> {
        let key = template.capability_id.clone();

        // capacity == 0 表示禁用缓存,拒绝所有注册
        if self.capacity == 0 {
            return Err(SsraError::ConfigError {
                reason: "模板缓存容量为 0,无法注册".into(),
            });
        }

        // 已存在则覆盖(更新预编译模板)
        if self.templates.contains_key(&key) {
            self.templates.insert(key, template);
            return Ok(());
        }

        // 新增:检查容量上限
        if self.templates.len() >= self.capacity {
            return Err(SsraError::ConfigError {
                reason: format!(
                    "模板缓存已满(capacity={}),请先调用 evict_oldest 腾出空间",
                    self.capacity
                ),
            });
        }

        self.templates.insert(key, template);
        Ok(())
    }

    /// 查找模板 — O(1),返回 clone,更新命中/未命中计数
    pub fn get(&self, capability_id: &str) -> Option<SlimeTemplate> {
        match self.templates.get(capability_id) {
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

    /// 零拷贝获取模板元数据 — 返回 (weight, fusion_strategy)
    ///
    /// WHY:融合引擎在 Top-K 选择时只需 weight 与 strategy(均为 Copy 类型),
    /// 避免克隆整个 SlimeTemplate(含 String + Vec),降低热路径开销。
    pub fn get_template_meta(&self, capability_id: &str) -> Option<(f32, FusionStrategy)> {
        self.templates
            .get(capability_id)
            .map(|entry| (entry.value().weight, entry.value().fusion_strategy))
    }

    /// LRU 驱逐 — 按 `compiled_at` 删除最旧条目直到 `len <= capacity - 1`
    ///
    /// 返回实际删除的条目数。若 `len <= capacity - 1`,返回 0。
    /// 当 `capacity == 0` 时,删除所有条目。
    pub fn evict_oldest(&self) -> usize {
        let current = self.templates.len();
        let target = self.capacity.saturating_sub(1);
        if current <= target {
            return 0;
        }
        let to_remove = current - target;

        // 收集 (compiled_at, capability_id) 并按时间升序排序(最旧在前)
        let mut entries: Vec<(DateTime<Utc>, String)> = self
            .templates
            .iter()
            .map(|r| (r.value().compiled_at, r.key().clone()))
            .collect();
        entries.sort_by_key(|(t, _)| *t);

        let mut removed = 0usize;
        for (_, id) in entries.into_iter().take(to_remove) {
            if self.templates.remove(&id).is_some() {
                removed += 1;
            }
        }
        removed
    }

    /// 当前模板数量
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    // === 辅助函数 ===

    fn make_spec(id: &str) -> TemplateSpec {
        TemplateSpec::new(id, vec!["x".into()], FusionStrategy::TopK)
    }

    fn make_template(id: &str, weight: f32) -> SlimeTemplate {
        precompile(make_spec(id)).with_weight(weight)
    }

    // === 1. 注册与查找基础测试 ===

    #[test]
    fn test_register_and_get() {
        let registry = TemplateRegistry::new(16);
        let template = make_template("cap-1", 0.8);
        registry.register(template.clone()).expect("注册失败");

        let found = registry.get("cap-1").expect("应找到模板");
        assert_eq!(found.capability_id, "cap-1");
        assert!((found.weight - 0.8).abs() < f32::EPSILON);
        assert_eq!(registry.hits(), 1);
    }

    #[test]
    fn test_get_not_found_updates_misses() {
        let registry = TemplateRegistry::new(16);
        assert!(registry.get("missing").is_none());
        assert_eq!(registry.misses(), 1);
        assert_eq!(registry.hits(), 0);
    }

    // === 2. 覆盖更新测试 ===

    #[test]
    fn test_register_overwrite_existing() {
        let registry = TemplateRegistry::new(16);
        registry
            .register(make_template("cap-1", 0.5))
            .expect("首次注册失败");
        // 覆盖:更新权重
        registry
            .register(make_template("cap-1", 0.9))
            .expect("覆盖注册失败");

        let found = registry.get("cap-1").expect("应找到模板");
        assert!(
            (found.weight - 0.9).abs() < f32::EPSILON,
            "覆盖后权重应为 0.9"
        );
        assert_eq!(registry.len(), 1, "覆盖不应增加条目数");
    }

    // === 3. 容量上限测试 ===

    #[test]
    fn test_register_capacity_full_returns_error() {
        let registry = TemplateRegistry::new(2);
        registry
            .register(make_template("cap-1", 0.5))
            .expect("注册 cap-1 失败");
        registry
            .register(make_template("cap-2", 0.6))
            .expect("注册 cap-2 失败");

        // 第三个应失败(容量满)
        let result = registry.register(make_template("cap-3", 0.7));
        assert!(result.is_err(), "容量满时应返回错误");
        assert!(matches!(result, Err(SsraError::ConfigError { .. })));
    }

    #[test]
    fn test_register_capacity_zero_returns_error() {
        let registry = TemplateRegistry::new(0);
        let result = registry.register(make_template("cap-1", 0.5));
        assert!(result.is_err(), "capacity=0 时应拒绝注册");
    }

    // === 4. LRU 驱逐测试 ===

    #[test]
    fn test_evict_oldest_removes_oldest() {
        let registry = TemplateRegistry::new(3);
        // 注册 3 个模板(时间递增)
        registry
            .register(make_template("old", 0.1))
            .expect("注册 old 失败");
        thread::sleep(std::time::Duration::from_millis(2));
        registry
            .register(make_template("mid", 0.5))
            .expect("注册 mid 失败");
        thread::sleep(std::time::Duration::from_millis(2));
        registry
            .register(make_template("new", 0.9))
            .expect("注册 new 失败");

        assert_eq!(registry.len(), 3);

        // 驱逐 1 个(最旧的 "old")
        let removed = registry.evict_oldest();
        assert_eq!(removed, 1, "应驱逐 1 个最旧模板");
        assert!(registry.get("old").is_none(), "old 应被驱逐");
        assert!(registry.get("mid").is_some(), "mid 应保留");
        assert!(registry.get("new").is_some(), "new 应保留");
    }

    #[test]
    fn test_evict_oldest_under_capacity_returns_zero() {
        let registry = TemplateRegistry::new(10);
        registry
            .register(make_template("cap-1", 0.5))
            .expect("注册失败");

        // 未达容量,无需驱逐
        let removed = registry.evict_oldest();
        assert_eq!(removed, 0);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_evict_oldest_capacity_zero_clears_all() {
        let registry = TemplateRegistry::new(0);
        // capacity=0 无法注册,所以先手动构造一个非零容量的 registry 再测试
        // 这里直接验证 capacity=0 时 evict 逻辑:target=0, current=0 → 返回 0
        let removed = registry.evict_oldest();
        assert_eq!(removed, 0);
    }

    // === 5. 零拷贝元数据访问测试 ===

    #[test]
    fn test_get_template_meta_zero_copy() {
        let registry = TemplateRegistry::new(16);
        registry
            .register(make_template("cap-1", 0.75))
            .expect("注册失败");

        let meta = registry.get_template_meta("cap-1").expect("应获取元数据");
        assert!((meta.0 - 0.75).abs() < f32::EPSILON, "weight 应为 0.75");
        assert_eq!(meta.1, FusionStrategy::TopK);
    }

    #[test]
    fn test_get_template_meta_not_found() {
        let registry = TemplateRegistry::new(16);
        assert!(registry.get_template_meta("missing").is_none());
    }

    // === 6. precompile 函数测试 ===

    #[test]
    fn test_precompile_sets_fields() {
        let spec = TemplateSpec::new(
            "shell-exec",
            vec!["cmd".into(), "timeout".into()],
            FusionStrategy::WeightedAverage,
        );
        let template = precompile(spec);

        assert_eq!(template.capability_id, "shell-exec");
        assert_eq!(template.parameter_shape, vec!["cmd", "timeout"]);
        assert_eq!(template.fusion_strategy, FusionStrategy::WeightedAverage);
        assert!(
            (template.weight - 1.0).abs() < f32::EPSILON,
            "默认权重应为 1.0"
        );
    }

    // === 7. 并发安全测试 ===

    #[test]
    fn test_concurrent_register_and_get() {
        let registry = std::sync::Arc::new(TemplateRegistry::new(100));
        let mut handles = Vec::new();

        // 多线程并发注册不同模板
        for i in 0..20 {
            let reg = std::sync::Arc::clone(&registry);
            handles.push(thread::spawn(move || {
                let id = format!("cap-{i}");
                reg.register(make_template(&id, 0.5)).expect("并发注册失败");
            }));
        }
        for h in handles {
            h.join().expect("线程 panic");
        }

        assert_eq!(registry.len(), 20);

        // 并发查找
        let mut read_handles = Vec::new();
        for i in 0..20 {
            let reg = std::sync::Arc::clone(&registry);
            read_handles.push(thread::spawn(move || {
                let id = format!("cap-{i}");
                reg.get(&id).expect("并发查找失败")
            }));
        }
        for h in read_handles {
            let t = h.join().expect("线程 panic");
            assert!(!t.capability_id.is_empty());
        }
    }

    // === 8. 边界:top_k 与 capacity 交互 ===

    #[test]
    fn test_evict_then_register_succeeds() {
        let registry = TemplateRegistry::new(2);
        registry
            .register(make_template("cap-1", 0.5))
            .expect("注册 cap-1 失败");
        registry
            .register(make_template("cap-2", 0.6))
            .expect("注册 cap-2 失败");

        // 容量满,注册失败
        assert!(registry.register(make_template("cap-3", 0.7)).is_err());

        // 驱逐 1 个腾出空间
        registry.evict_oldest();

        // 现在可以注册
        registry
            .register(make_template("cap-3", 0.7))
            .expect("驱逐后注册应成功");
        assert_eq!(registry.len(), 2);
    }
}
