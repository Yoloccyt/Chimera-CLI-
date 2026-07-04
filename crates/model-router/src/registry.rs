//! 模型注册表 — 线程安全的模型元信息存储
//!
//! 对应架构:L1 Core,被 Router 与 Strategies 共享访问
//!
//! # 设计要点
//! - 基于 `DashMap` 提供并发安全的读写(无锁读,细粒度写锁)
//! - `Arc<DashMap>` 使 `ModelRegistry` 可廉价 Clone,跨任务共享
//! - 注册/注销操作返回 `Result`,避免静默覆盖或丢失

use std::sync::Arc;

use dashmap::DashMap;

use crate::error::RouterError;
use crate::types::ModelInfo;

/// 模型注册表 — 持有所有可路由模型的元信息
///
/// Clone 廉价(仅 Arc 引用计数),可在多任务间自由传递,
/// 所有 Clone 共享同一份底层数据。
#[derive(Clone)]
pub struct ModelRegistry {
    models: Arc<DashMap<String, ModelInfo>>,
}

impl ModelRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            models: Arc::new(DashMap::new()),
        }
    }

    /// 从配置加载模型列表,返回新注册表
    pub fn from_config(config: &crate::config::RouterConfig) -> Self {
        let registry = Self::new();
        for model in &config.models {
            // 配置加载阶段静默覆盖重复项(配置错误应在解析时校验)
            registry
                .models
                .insert(model.model_id.clone(), model.clone());
        }
        registry
    }

    /// 注册新模型
    ///
    /// 若 model_id 已存在,返回 `RouterError::ConfigError` 以避免静默覆盖。
    pub fn register(&self, model: ModelInfo) -> Result<(), RouterError> {
        if self.models.contains_key(&model.model_id) {
            return Err(RouterError::ConfigError(format!(
                "model already registered: {}",
                model.model_id
            )));
        }
        self.models.insert(model.model_id.clone(), model);
        Ok(())
    }

    /// 注销模型
    ///
    /// 若 model_id 不存在,返回 `RouterError::ModelNotFound`。
    pub fn unregister(&self, model_id: &str) -> Result<(), RouterError> {
        self.models
            .remove(model_id)
            .map(|_| ())
            .ok_or_else(|| RouterError::ModelNotFound(model_id.into()))
    }

    /// 查询指定模型,返回克隆(避免持锁)
    pub fn get(&self, model_id: &str) -> Option<ModelInfo> {
        self.models.get(model_id).map(|r| r.clone())
    }

    /// 列出所有已注册模型(无序)
    pub fn list(&self) -> Vec<ModelInfo> {
        self.models.iter().map(|r| r.value().clone()).collect()
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
        self.models.len()
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
        // Clone 共享底层 DashMap,因此 cloned 也能看到新注册的模型
        assert_eq!(cloned.count(), 1);
    }
}
