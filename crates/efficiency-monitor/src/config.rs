//! 监控配置定义
//!
//! 控制采集间隔、默认告警冷却时间与 Critical 事件立即告警开关。
//! 配置项默认值经过权衡,适合大多数 L9 Quest 层监控场景。

use serde::{Deserialize, Serialize};

/// 监控配置 — 控制效率监控器的采集与告警行为
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorConfig {
    /// 采集间隔(毫秒)
    ///
    /// 默认 1000ms(1 秒),平衡监控实时性与系统开销。
    /// WHY 1000:效率监控不需要亚秒级实时性,1 秒间隔可将开销控制在可忽略水平;
    /// 对延迟敏感的场景(如 Critical 事件)走立即告警通道,不依赖采集间隔。
    pub collect_interval_ms: u64,

    /// 默认告警冷却时间(秒)
    ///
    /// 默认 60 秒,避免同一规则在短时间内重复触发告警风暴。
    /// WHY 60:Critical 事件走立即告警(无 cooldown),Warning/Info 走规则引擎
    /// 检查,60 秒 cooldown 可防止指标抖动导致的告警风暴。
    pub default_cooldown_secs: u64,

    /// 是否启用 Critical 事件立即告警
    ///
    /// 默认 true。启用后,SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded
    /// 四个 Critical 事件将绕过规则引擎,直接触发告警并发布 EfficiencyAlertTriggered。
    /// WHY true:这些事件代表安全/预算红线,必须立即告警,不应受采集间隔或规则引擎延迟影响。
    pub critical_instant_alert: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            collect_interval_ms: 1000,
            default_cooldown_secs: 60,
            critical_instant_alert: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MonitorConfig::default();
        assert_eq!(config.collect_interval_ms, 1000);
        assert_eq!(config.default_cooldown_secs, 60);
        assert!(config.critical_instant_alert);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = MonitorConfig {
            collect_interval_ms: 500,
            default_cooldown_secs: 30,
            critical_instant_alert: false,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: MonitorConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.collect_interval_ms, 500);
        assert_eq!(restored.default_cooldown_secs, 30);
        assert!(!restored.critical_instant_alert);
    }

    #[test]
    fn test_config_clone() {
        let config = MonitorConfig::default();
        let cloned = config.clone();
        assert_eq!(config.collect_interval_ms, cloned.collect_interval_ms);
    }
}
