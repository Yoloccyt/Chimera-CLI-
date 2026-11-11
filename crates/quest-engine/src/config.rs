//! Quest Engine 配置类型 — 任务分解与检查点策略参数
//!
//! 对应架构层:L9 Quest
//!
//! # 设计决策(WHY)
//! - `max_tasks_per_quest`:防止分解器生成过细任务图导致调度开销激增
//!   (架构红线:单 Quest 任务数 ≤ 16,与 GQEP 批处理窗口对齐)
//! - `checkpoint_interval`:每 N 个 Task 完成触发检查点,平衡持久化开销
//!   与恢复成本;过小则 IO 频繁,过大则崩溃后重做工作多

use serde::{Deserialize, Serialize};

use crate::decompose::DependencyMode;

/// Quest Engine 配置 — 控制任务分解上限与检查点频率
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestConfig {
    /// 单个 Quest 允许的最大任务数(默认 16)
    ///
    /// WHY:超过此值的分解会被截断,防止恶意或异常输入导致任务爆炸
    pub max_tasks_per_quest: u32,

    /// 检查点触发间隔:每 N 个 Task 完成保存一次检查点(默认 3)
    ///
    /// WHY:0 表示禁用自动检查点;Week 2 阶段仅配置,实际持久化在后续阶段实现
    pub checkpoint_interval: u32,

    /// 依赖推断模式(WP-E 方案 C 用户裁决,默认 Linear = 历史行为)
    ///
    /// WHY serde default:旧配置文件(无此字段)反序列化时回填 Linear,
    /// 存量配置零兼容性问题;Linear 路径与旧硬编码逐字节等价,
    /// 由 `decompose::tests::linear_matches_legacy` 锁定。
    #[serde(default)]
    pub dependency_mode: DependencyMode,
}

impl Default for QuestConfig {
    fn default() -> Self {
        Self {
            max_tasks_per_quest: 16,
            checkpoint_interval: 3,
            dependency_mode: DependencyMode::default(),
        }
    }
}

impl QuestConfig {
    /// 创建自定义配置
    pub fn new(max_tasks_per_quest: u32, checkpoint_interval: u32) -> Self {
        Self {
            max_tasks_per_quest,
            checkpoint_interval,
            dependency_mode: DependencyMode::default(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = QuestConfig::default();
        assert_eq!(cfg.max_tasks_per_quest, 16);
        assert_eq!(cfg.checkpoint_interval, 3);
    }

    #[test]
    fn test_custom_config() {
        let cfg = QuestConfig::new(8, 1);
        assert_eq!(cfg.max_tasks_per_quest, 8);
        assert_eq!(cfg.checkpoint_interval, 1);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let cfg = QuestConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let de: QuestConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.max_tasks_per_quest, cfg.max_tasks_per_quest);
        assert_eq!(de.checkpoint_interval, cfg.checkpoint_interval);
        assert_eq!(de.dependency_mode, cfg.dependency_mode);
    }

    #[test]
    fn test_config_missing_dependency_mode_defaults_linear() {
        // WP-E 方案 C:旧配置文件无 dependency_mode 字段,必须回填 Linear
        // (存量配置零兼容性问题——默认配置下行为零变化的配置面保证)
        let json = r#"{"max_tasks_per_quest":16,"checkpoint_interval":3}"#;
        let de: QuestConfig = serde_json::from_str(json).unwrap();
        assert_eq!(de.dependency_mode, DependencyMode::Linear);
        assert_eq!(de.max_tasks_per_quest, 16);
        assert_eq!(de.checkpoint_interval, 3);
    }
}
