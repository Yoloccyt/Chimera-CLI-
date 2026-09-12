//! 供应商健康漂移守卫 — T-08 三维度漂移检测与告警（P4-T5，D-P8 裁决）
//!
//! 对应架构层: **L1 Core**（model-router 层内模块,零新增 crate）
//! 对应任务: **P4-T5**（W21:T-08 供应商守卫上线,ADR-154 登记）
//!
//! # 检测维度（Ch12 W22 门禁:漂移用例 100% 报警 + 正常流量零误报）
//! 1. **延迟漂移**:健康探测延迟相对基线膨胀超过 `latency_ratio`（默认 3×）
//!    且绝对值超过 `latency_floor_ms`（默认 1000ms,排除小基数噪声）
//! 2. **健康翻转**:`healthy: true → false`
//! 3. **能力标签漂移**:context 缩水超 `context_shrink_ratio`（默认 50%）
//!    或任一布尔能力（vision/tools/streaming/effort）丢失
//!
//! # 保守化原则（RK-P22 缓解）
//! 守卫**只报警不熔断**——告警经 [`ProviderDriftAlert`] 产出并由调用方
//! 发布（DynamicEvent 双轨,轨二 `external.*` 命名空间）,路由决策不受影响;
//! 阈值全部显式可配（[`DriftThresholds`]）,默认值保守（高置信才报警）。
//!
//! # 零误报设计
//! 基线快照（[`DriftBaseline`]）取自首次观测;正常波动（比率低于阈值或
//! 绝对值低于地板）不触发。首观测只建基线不报警（无比较对象）。

use nexus_contracts::event_v2::{
    DynamicEvent, EventMetadataV2, EventNamespace, EventTypeId, ImportanceScore,
};

use crate::provider::{Health, ProviderCaps};

/// 漂移阈值 — 全部显式可配,默认保守（ADR-154）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftThresholds {
    /// 延迟膨胀比率告警线（当前/基线 > ratio → 报警）
    pub latency_ratio: f64,
    /// 延迟告警地板（毫秒;低于此绝对值不报,排除小基数噪声）
    pub latency_floor_ms: u64,
    /// 上下文缩水比率告警线（缩水比例 > ratio → 报警）
    pub context_shrink_ratio: f64,
}

impl Default for DriftThresholds {
    fn default() -> Self {
        Self {
            latency_ratio: 3.0,
            latency_floor_ms: 1_000,
            context_shrink_ratio: 0.5,
        }
    }
}

/// 基线快照 — 首次观测固化（健康 + 能力）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftBaseline {
    /// 基线健康探测延迟（毫秒）
    pub latency_ms: u64,
    /// 基线能力快照
    pub caps: ProviderCaps,
}

/// 漂移维度 — 三类检测枚举
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftKind {
    /// 延迟膨胀（当前 ms,基线 ms）
    Latency(u64, u64),
    /// 健康翻转（true → false）
    HealthFlipped,
    /// 上下文缩水（当前 tokens,基线 tokens）
    ContextShrunk(usize, usize),
    /// 布尔能力丢失（能力名）
    CapabilityLost(&'static str),
}

/// 漂移告警 — 单 provider 单维度一条
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDriftAlert {
    /// provider id
    pub provider_id: String,
    /// 漂移维度
    pub kind: DriftKind,
}

impl ProviderDriftAlert {
    /// 告警 JSON 载荷（DynamicEvent serialize 输入）
    #[must_use]
    pub fn payload_json(&self) -> String {
        let detail = match &self.kind {
            DriftKind::Latency(cur, base) => {
                format!("latency {base}ms -> {cur}ms")
            }
            DriftKind::HealthFlipped => "healthy true -> false".to_string(),
            DriftKind::ContextShrunk(cur, base) => {
                format!("context {base} -> {cur} tokens")
            }
            DriftKind::CapabilityLost(name) => format!("capability lost: {name}"),
        };
        format!(
            "{{\"provider\":\"{}\",\"drift\":\"{}\",\"detail\":\"{}\"}}",
            self.provider_id,
            match &self.kind {
                DriftKind::Latency(..) => "latency",
                DriftKind::HealthFlipped => "health",
                DriftKind::ContextShrunk(..) => "context",
                DriftKind::CapabilityLost(_) => "capability",
            },
            detail
        )
    }
}

/// 守卫结果 — 报警列表（空 = 零漂移）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DriftReport {
    /// 全部告警（只报警不熔断;调用方决定上报/记录）
    pub alerts: Vec<ProviderDriftAlert>,
}

impl DriftReport {
    /// 是否零漂移（正常流量判定）
    #[must_use]
    pub fn clean(&self) -> bool {
        self.alerts.is_empty()
    }
}

/// 供应商漂移守卫 — 纯函数检测器（无内部状态,基线由调用方管理）
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderDriftGuard {
    thresholds: DriftThresholds,
}

impl ProviderDriftGuard {
    /// 新建（默认保守阈值）
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 自定义阈值（ADR-154 裁决入口）
    #[must_use]
    pub const fn with_thresholds(thresholds: DriftThresholds) -> Self {
        Self { thresholds }
    }

    /// 首次观测 — 固化基线（不检测,无比较对象）
    #[must_use]
    pub fn establish_baseline(&self, health: &Health, caps: &ProviderCaps) -> DriftBaseline {
        DriftBaseline {
            latency_ms: health.latency_ms,
            caps: caps.clone(),
        }
    }

    /// 漂移检测 — 基线 vs 当前观测（三维度逐项检查）
    #[must_use]
    pub fn check(
        &self,
        provider_id: &str,
        baseline: &DriftBaseline,
        health: &Health,
        caps: &ProviderCaps,
    ) -> DriftReport {
        let mut alerts = Vec::new();
        // 维度 1:延迟膨胀（比率超阈值 且 绝对值超地板）
        if baseline.latency_ms > 0 && health.healthy {
            let ratio = health.latency_ms as f64 / baseline.latency_ms as f64;
            if ratio > self.thresholds.latency_ratio
                && health.latency_ms > self.thresholds.latency_floor_ms
            {
                alerts.push(ProviderDriftAlert {
                    provider_id: provider_id.to_string(),
                    kind: DriftKind::Latency(health.latency_ms, baseline.latency_ms),
                });
            }
        }
        // 维度 2:健康翻转（true → false）
        if baseline.caps.context > 0 && !health.healthy {
            alerts.push(ProviderDriftAlert {
                provider_id: provider_id.to_string(),
                kind: DriftKind::HealthFlipped,
            });
        }
        // 维度 3a:上下文缩水（超比率阈值）
        if caps.context < baseline.caps.context {
            let shrink =
                (baseline.caps.context - caps.context) as f64 / baseline.caps.context as f64;
            if shrink > self.thresholds.context_shrink_ratio {
                alerts.push(ProviderDriftAlert {
                    provider_id: provider_id.to_string(),
                    kind: DriftKind::ContextShrunk(caps.context, baseline.caps.context),
                });
            }
        }
        // 维度 3b:布尔能力丢失
        let cap_checks: [(&'static str, bool, bool); 4] = [
            ("vision", baseline.caps.vision, caps.vision),
            ("tools", baseline.caps.tools, caps.tools),
            ("streaming", baseline.caps.streaming, caps.streaming),
            ("effort", baseline.caps.effort, caps.effort),
        ];
        for (name, was, now) in cap_checks {
            if was && !now {
                alerts.push(ProviderDriftAlert {
                    provider_id: provider_id.to_string(),
                    kind: DriftKind::CapabilityLost(name),
                });
            }
        }
        DriftReport { alerts }
    }
}

/// 漂移告警事件 — DynamicEvent 双轨实现（`external.*` 命名空间,轨二）
#[derive(Debug, Clone)]
pub struct ProviderDriftEvent {
    type_id: EventTypeId,
    meta: EventMetadataV2,
    payload_json: String,
}

impl ProviderDriftEvent {
    /// 从告警构造事件实例
    #[must_use]
    pub fn new(alert: &ProviderDriftAlert, session_id: &str) -> Self {
        Self {
            type_id: EventTypeId::new("external.provider_drift_detected"),
            meta: EventMetadataV2::new(session_id),
            payload_json: alert.payload_json(),
        }
    }
}

impl DynamicEvent for ProviderDriftEvent {
    fn event_type(&self) -> EventTypeId {
        self.type_id.clone()
    }
    fn namespace(&self) -> EventNamespace {
        EventNamespace::External
    }
    fn serialize(&self) -> Result<Vec<u8>, String> {
        // payload_json 已是 JSON 文本,直接转字节（禁二次序列化,nexus-hook 先例）
        Ok(self.payload_json.clone().into_bytes())
    }
    fn metadata(&self) -> &EventMetadataV2 {
        &self.meta
    }
    fn importance(&self) -> ImportanceScore {
        // 供应商漂移高重要性（路由降级/熔断决策输入）
        ImportanceScore::new(0.8)
    }
    fn extract_symbols(&self) -> Vec<Box<str>> {
        vec![self.type_id.as_str().into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PID: &str = "provider-a";

    fn health(healthy: bool, latency_ms: u64) -> Health {
        Health {
            provider_id: PID.to_string(),
            healthy,
            latency_ms,
        }
    }

    fn caps() -> ProviderCaps {
        ProviderCaps {
            context: 128_000,
            vision: true,
            tools: true,
            streaming: true,
            effort: true,
            attention_mode: crate::provider::AttentionMode::default(),
        }
    }

    /// 固定用例 1:延迟漂移（100ms → 500ms,5× > 3× 且 >1000ms 地板?500 不超地板）
    /// 地板 1000ms 下 500ms 不报 → 用 1500ms 用例（15×）
    #[test]
    fn latency_drift_alerts() {
        let guard = ProviderDriftGuard::new();
        let base = guard.establish_baseline(&health(true, 100), &caps());
        let report = guard.check(PID, &base, &health(true, 1_500), &caps());
        assert!(!report.clean(), "15× 膨胀必须报警");
        assert!(matches!(
            report.alerts[0].kind,
            DriftKind::Latency(1_500, 100)
        ));
    }

    /// 小基数噪声不误报（100ms → 300ms 3× 但低于地板 1000ms）
    #[test]
    fn latency_below_floor_no_alert() {
        let guard = ProviderDriftGuard::new();
        let base = guard.establish_baseline(&health(true, 100), &caps());
        let report = guard.check(PID, &base, &health(true, 300), &caps());
        assert!(report.clean(), "低于地板的小基数膨胀不报（零误报）");
    }

    /// 固定用例 2:健康翻转
    #[test]
    fn health_flip_alerts() {
        let guard = ProviderDriftGuard::new();
        let base = guard.establish_baseline(&health(true, 100), &caps());
        let report = guard.check(PID, &base, &health(false, 0), &caps());
        assert!(report
            .alerts
            .iter()
            .any(|a| a.kind == DriftKind::HealthFlipped));
    }

    /// 固定用例 3:上下文缩水（128k → 8k,94% > 50%）
    #[test]
    fn context_shrink_alerts() {
        let guard = ProviderDriftGuard::new();
        let base = guard.establish_baseline(&health(true, 100), &caps());
        let mut shrunk = caps();
        shrunk.context = 8_000;
        let report = guard.check(PID, &base, &health(true, 100), &shrunk);
        assert!(report
            .alerts
            .iter()
            .any(|a| matches!(a.kind, DriftKind::ContextShrunk(8_000, 128_000))));
    }

    /// 固定用例 4:布尔能力丢失（tools true → false）
    #[test]
    fn capability_loss_alerts() {
        let guard = ProviderDriftGuard::new();
        let base = guard.establish_baseline(&health(true, 100), &caps());
        let mut reduced = caps();
        reduced.tools = false;
        let report = guard.check(PID, &base, &health(true, 100), &reduced);
        assert!(report
            .alerts
            .iter()
            .any(|a| a.kind == DriftKind::CapabilityLost("tools")));
    }

    /// 零误报 — 正常波动（100ms→120ms,1.2×;能力全保留）零报警
    #[test]
    fn normal_fluctuation_zero_false_positive() {
        let guard = ProviderDriftGuard::new();
        let base = guard.establish_baseline(&health(true, 100), &caps());
        // 多组正常波动用例
        for latency in [100u64, 110, 120, 130, 250] {
            let report = guard.check(PID, &base, &health(true, latency), &caps());
            assert!(
                report.clean(),
                "正常波动 {latency}ms 不得误报: {:?}",
                report.alerts
            );
        }
    }

    /// 门禁:漂移用例矩阵 100% 报警（4/4 维度用例全部触发）
    #[test]
    fn drift_matrix_hundred_percent_alert_rate() {
        let guard = ProviderDriftGuard::new();
        let base = guard.establish_baseline(&health(true, 100), &caps());
        let mut drifted = caps();
        drifted.tools = false;
        let cases: Vec<(&str, Health, ProviderCaps, usize)> = vec![
            ("latency", health(true, 2_000), caps(), 1),
            ("health", health(false, 10), caps(), 1),
            (
                "context",
                health(true, 100),
                {
                    let mut c = caps();
                    c.context = 32_000; // 75% 缩水
                    c
                },
                1,
            ),
            ("capability", health(true, 100), drifted, 1),
        ];
        let mut alerted = 0;
        for (name, h, c, expected) in &cases {
            let report = guard.check(PID, &base, h, c);
            assert_eq!(report.alerts.len(), *expected, "用例 {name} 必须精确报警");
            if !report.clean() {
                alerted += 1;
            }
        }
        assert_eq!(alerted, 4, "漂移矩阵 100% 报警: {alerted}/4");
    }

    /// DynamicEvent 双轨 — 告警事件构造 + 序列化含 provider 与细节
    #[test]
    fn drift_event_dual_track() {
        let alert = ProviderDriftAlert {
            provider_id: PID.to_string(),
            kind: DriftKind::Latency(2_000, 100),
        };
        let event = ProviderDriftEvent::new(&alert, "sess-1");
        assert_eq!(
            event.event_type().as_str(),
            "external.provider_drift_detected"
        );
        assert_eq!(event.namespace(), EventNamespace::External);
        assert_eq!(event.importance().value(), 0.8);
        let bytes = event.serialize().expect("序列化成功");
        let text = String::from_utf8(bytes).expect("JSON 合法");
        assert!(text.contains("provider-a"));
        assert!(text.contains("2000ms"));
        assert!(text.contains("latency"));
    }
}
