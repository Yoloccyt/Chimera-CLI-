//! 参数注册表 — 全局可学习参数管理

use std::sync::Arc;

use dashmap::DashMap;

use crate::error::LearningError;
use crate::learner::{GradientDescent, OnlineLearner};
use crate::types::{FeedbackSignal, LearnableParameter, ParameterValue};

/// 参数注册表 — 全局可学习参数的集中管理器
///
/// 基于 `DashMap` 实现并发安全的参数存储,支持:
/// - 参数注册/注销
/// - 参数查询与更新
/// - 批量持久化/恢复
/// - 反馈驱动的自动更新
///
/// # 线程安全
/// `Clone` 廉价(仅 Arc 引用计数),可在多线程/多任务间共享。
#[derive(Clone)]
pub struct ParameterRegistry {
    /// 参数存储:参数ID → 参数
    params: Arc<DashMap<String, LearnableParameter>>,
    /// 默认学习器
    default_learner: Arc<dyn OnlineLearner>,
}

impl ParameterRegistry {
    /// 创建新的参数注册表
    pub fn new() -> Self {
        Self {
            params: Arc::new(DashMap::new()),
            default_learner: Arc::new(GradientDescent),
        }
    }

    /// 注册参数
    ///
    /// 若参数ID已存在,返回错误。
    pub fn register(&self, param: LearnableParameter) -> Result<(), LearningError> {
        if self.params.contains_key(&param.id) {
            return Err(LearningError::UpdateFailed(format!(
                "parameter {} already registered",
                param.id
            )));
        }
        self.params.insert(param.id.clone(), param);
        Ok(())
    }

    /// 注销参数
    pub fn unregister(&self, id: &str) -> Result<(), LearningError> {
        self.params
            .remove(id)
            .ok_or_else(|| LearningError::ParameterNotFound(id.to_string()))?;
        Ok(())
    }

    /// 获取参数值
    pub fn get_value(&self, id: &str) -> Result<ParameterValue, LearningError> {
        self.params
            .get(id)
            .map(|e| e.value.clone().value)
            .ok_or_else(|| LearningError::ParameterNotFound(id.to_string()))
    }

    /// 获取参数引用(只读)
    pub fn get_param(&self, id: &str) -> Result<LearnableParameter, LearningError> {
        self.params
            .get(id)
            .map(|e| e.value.clone())
            .ok_or_else(|| LearningError::ParameterNotFound(id.to_string()))
    }

    /// 更新参数值(直接设置)
    pub fn set_value(
        &self,
        id: &str,
        value: ParameterValue,
    ) -> Result<(), LearningError> {
        let mut entry = self
            .params
            .get_mut(id)
            .ok_or_else(|| LearningError::ParameterNotFound(id.to_string()))?;
        entry.value = value;
        entry.update_count += 1;
        entry.last_updated = chrono::Utc::now().to_rfc3339();
        entry.clamp_value();
        Ok(())
    }

    /// 基于反馈信号更新参数(使用在线学习)
    ///
    /// 流程:
    /// 1. 查找参数
    /// 2. 调用学习器计算新值
    /// 3. 应用更新并记录
    pub fn update_with_feedback(
        &self,
        id: &str,
        feedback: FeedbackSignal,
    ) -> Result<ParameterValue, LearningError> {
        let mut entry = self
            .params
            .get_mut(id)
            .ok_or_else(|| LearningError::ParameterNotFound(id.to_string()))?;

        let new_value = self
            .default_learner
            .update(&entry.value, feedback, entry.learning_rate);
        entry.value = new_value;
        entry.update_count += 1;
        entry.last_updated = chrono::Utc::now().to_rfc3339();
        entry.clamp_value();

        Ok(entry.value.clone())
    }

    /// 列出所有已注册参数
    pub fn list_params(&self) -> Vec<LearnableParameter> {
        self.params.iter().map(|e| e.value.clone()).collect()
    }

    /// 按crate筛选参数
    pub fn list_by_crate(&self, crate_name: &str) -> Vec<LearnableParameter> {
        self.params
            .iter()
            .filter(|e| e.crate_name == crate_name)
            .map(|e| e.value.clone())
            .collect()
    }

    /// 参数总数
    pub fn count(&self) -> usize {
        self.params.len()
    }

    /// 序列化所有参数到JSON
    pub fn to_json(&self) -> Result<String, LearningError> {
        let params: Vec<LearnableParameter> = self.list_params();
        Ok(serde_json::to_string(&params)?)
    }

    /// 从JSON恢复参数
    pub fn from_json(&self, json: &str) -> Result<(), LearningError> {
        let params: Vec<LearnableParameter> = serde_json::from_str(json)?;
        for param in params {
            self.register(param)?;
        }
        Ok(())
    }
}

impl Default for ParameterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_param(id: &str, value: f32) -> LearnableParameter {
        LearnableParameter::new(id, format!("param-{id}"), "test-crate", ParameterValue::scalar(value))
    }

    #[test]
    fn test_register_and_get() {
        let registry = ParameterRegistry::new();
        let param = make_param("p1", 0.5);
        registry.register(param).unwrap();

        let v = registry.get_value("p1").unwrap();
        assert_eq!(v.as_scalar(), Some(0.5));
    }

    #[test]
    fn test_update_with_feedback() {
        let registry = ParameterRegistry::new();
        let param = make_param("p1", 0.5).with_learning_rate(0.1);
        registry.register(param).unwrap();

        let new_v = registry
            .update_with_feedback("p1", FeedbackSignal::Success)
            .unwrap();
        // 0.5 + 0.1 * 1.0 = 0.6
        assert!((new_v.as_scalar().unwrap() - 0.6).abs() < 1e-5);
    }

    #[test]
    fn test_list_by_crate() {
        let registry = ParameterRegistry::new();
        registry.register(make_param("p1", 0.5)).unwrap();
        registry
            .register(
                LearnableParameter::new("p2", "p2", "other-crate", ParameterValue::scalar(0.3)),
            )
            .unwrap();

        let test_params = registry.list_by_crate("test-crate");
        assert_eq!(test_params.len(), 1);
        assert_eq!(test_params[0].id, "p1");
    }

    #[test]
    fn test_json_roundtrip() {
        let registry = ParameterRegistry::new();
        registry.register(make_param("p1", 0.5)).unwrap();

        let json = registry.to_json().unwrap();
        let registry2 = ParameterRegistry::new();
        registry2.from_json(&json).unwrap();

        assert_eq!(registry2.count(), 1);
        assert_eq!(registry2.get_value("p1").unwrap().as_scalar(), Some(0.5));
    }
}
