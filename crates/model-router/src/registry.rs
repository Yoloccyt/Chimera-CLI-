//! 模型注册表 — 线程安全的模型元信息存储
//!
//! 对应架构:L1 Core,被 Router 与 Strategies 共享访问
//!
//! # 设计要点
//! - 基于 `RwLock<HashMap>` 提供并发安全的读写(读锁可并发,写锁互斥)
//! - B3 优化:对于小规模注册表(≤10 模型),RwLock 开销(~50ns)远低于
//!   DashMap 分片锁(~200ns),且无哈希分片开销
//! - `Arc<RwLock<HashMap>>` 使 `ModelRegistry` 可廉价 Clone,跨任务共享
//! - 注册/注销操作返回 `Result`,避免静默覆盖或丢失
//! - `register()` 使用 entry API 原子性检查+插入,消除 TOCTOU 竞态

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::error::RouterError;
use crate::types::ModelInfo;

/// 模型注册表 — 持有所有可路由模型的元信息
///
/// Clone 廉价(仅 Arc 引用计数),可在多任务间自由传递,
/// 所有 Clone 共享同一份底层数据。
#[derive(Clone)]
pub struct ModelRegistry {
    models: Arc<RwLock<HashMap<String, ModelInfo>>>,
}

impl ModelRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            models: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 从配置加载模型列表,返回新注册表
    pub fn from_config(config: &crate::config::RouterConfig) -> Self {
        let registry = Self::new();
        // SAFETY:RwLock 刚创建,无其他线程可能 panic 持锁,不可能 poisoned
        // WHY 用作用域限定 guard 生命周期:RwLockWriteGuard 的 Drop 持有对
        // registry.models 的引用,若 guard 仍存活时 move registry 会触发 E0505
        // (cannot move out of borrowed)。作用域结束先 drop guard,再 move registry。
        {
            let mut models = registry
                .models
                .write()
                .expect("fresh RwLock cannot be poisoned");
            for model in &config.models {
                // 配置加载阶段静默覆盖重复项(配置错误应在解析时校验)
                models.insert(model.model_id.clone(), model.clone());
            }
        }
        registry
    }

    /// 注册新模型
    ///
    /// 若 model_id 已存在,返回 `RouterError::ConfigError` 以避免静默覆盖。
    /// 使用 entry API 原子性检查+插入,消除 contains_key + insert 之间的竞态窗口。
    pub fn register(&self, model: ModelInfo) -> Result<(), RouterError> {
        let mut models = self
            .models
            .write()
            .map_err(|_| RouterError::ConfigError("rwlock poisoned".into()))?;
        use std::collections::hash_map::Entry;
        match models.entry(model.model_id.clone()) {
            Entry::Occupied(_) => Err(RouterError::ConfigError(format!(
                "model already registered: {}",
                model.model_id
            ))),
            Entry::Vacant(entry) => {
                entry.insert(model);
                Ok(())
            }
        }
    }

    /// 注销模型
    ///
    /// 若 model_id 不存在,返回 `RouterError::ModelNotFound`。
    pub fn unregister(&self, model_id: &str) -> Result<(), RouterError> {
        let mut models = self
            .models
            .write()
            .map_err(|_| RouterError::ConfigError("rwlock poisoned".into()))?;
        models
            .remove(model_id)
            .map(|_| ())
            .ok_or_else(|| RouterError::ModelNotFound(model_id.into()))
    }

    /// 查询指定模型,返回克隆(避免持锁)
    pub fn get(&self, model_id: &str) -> Option<ModelInfo> {
        let models = self.models.read().ok()?;
        models.get(model_id).cloned()
    }

    /// 列出所有已注册模型(无序)
    pub fn list(&self) -> Vec<ModelInfo> {
        let models = match self.models.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        models.values().cloned().collect()
    }

    /// 按成本升序返回模型列表(Lite 策略使用)
    pub fn list_by_cost(&self) -> Vec<ModelInfo> {
        let mut models = self.list();
        models.sort_by(|a, b| {
            a.cost_per_1k_tokens
                .partial_cmp(&b.cost_per_1k_tokens)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        models
    }

    /// 按延迟升序返回模型列表(Efficient 策略使用)
    pub fn list_by_latency(&self) -> Vec<ModelInfo> {
        let mut models = self.list();
        models.sort_by_key(|m| m.avg_latency_ms);
        models
    }

    /// 已注册模型数量
    pub fn count(&self) -> usize {
        let models = match self.models.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        models.len()
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model(id: &str, cost: f64, latency: u64, quality: f32) -> ModelInfo {
        ModelInfo {
            model_id: id.into(),
            provider: "test".into(),
            cost_per_1k_tokens: cost,
            avg_latency_ms: latency,
            max_context: 8192,
            quality_score: quality,
        }
    }

    #[test]
    fn test_register_and_get() {
        let registry = ModelRegistry::new();
        let model = make_model("m1", 0.001, 100, 0.8);
        registry.register(model.clone()).unwrap();
        assert_eq!(registry.count(), 1);
        assert_eq!(registry.get("m1"), Some(model));
    }

    #[test]
    fn test_register_duplicate_fails() {
        let registry = ModelRegistry::new();
        let model = make_model("m1", 0.001, 100, 0.8);
        registry.register(model.clone()).unwrap();
        let result = registry.register(model);
        assert!(matches!(result, Err(RouterError::ConfigError(_))));
    }

    #[test]
    fn test_unregister() {
        let registry = ModelRegistry::new();
        registry
            .register(make_model("m1", 0.001, 100, 0.8))
            .unwrap();
        assert_eq!(registry.count(), 1);
        registry.unregister("m1").unwrap();
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_unregister_not_found() {
        let registry = ModelRegistry::new();
        let result = registry.unregister("nonexistent");
        assert!(matches!(result, Err(RouterError::ModelNotFound(_))));
    }

    #[test]
    fn test_list_by_cost() {
        let registry = ModelRegistry::new();
        registry
            .register(make_model("expensive", 0.01, 100, 0.9))
            .unwrap();
        registry
            .register(make_model("cheap", 0.001, 200, 0.6))
            .unwrap();
        registry
            .register(make_model("mid", 0.005, 150, 0.75))
            .unwrap();

        let sorted = registry.list_by_cost();
        let ids: Vec<&str> = sorted.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, vec!["cheap", "mid", "expensive"]);
    }

    #[test]
    fn test_list_by_latency() {
        let registry = ModelRegistry::new();
        registry
            .register(make_model("slow", 0.001, 500, 0.9))
            .unwrap();
        registry
            .register(make_model("fast", 0.01, 50, 0.6))
            .unwrap();
        registry
            .register(make_model("mid", 0.005, 200, 0.75))
            .unwrap();

        let sorted = registry.list_by_latency();
        let ids: Vec<&str> = sorted.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, vec!["fast", "mid", "slow"]);
    }

    #[test]
    fn test_from_config() {
        let config = crate::config::RouterConfig::default();
        let registry = ModelRegistry::from_config(&config);
        assert_eq!(registry.count(), 3);
        assert!(registry.get("lite-model").is_some());
        assert!(registry.get("efficient-model").is_some());
        assert!(registry.get("premium-model").is_some());
    }

    #[test]
    fn test_clone_shares_state() {
        let registry = ModelRegistry::new();
        let cloned = registry.clone();
        registry
            .register(make_model("m1", 0.001, 100, 0.8))
            .unwrap();
        // Clone 共享底层 RwLock<HashMap>,因此 cloned 也能看到新注册的模型
        assert_eq!(cloned.count(), 1);
    }
}
