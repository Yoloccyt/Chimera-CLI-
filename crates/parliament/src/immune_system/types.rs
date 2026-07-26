//! ImmuneSystem 类型定义 — 不可进化面（ADR-046 决策 9）
//!
//! 对应架构层:L8 Parliament
//! 对应 ADR:ADR-046 决策 1/9
//!
//! # 不可进化面清单（决策 9 附录 C）
//! - `ParadoxProbe` trait 签名
//! - `ProbeType` enum 变体集（`#[non_exhaustive]`）
//! - `ParadoxReport` 数据结构
//! - `ParadoxRiskReport` 数据结构
//! - `ImmuneSystemError` enum
//!
//! # 可进化面（决策 9 附录 C,允许 GSOE/AutoDPO 演化）
//! - 滑动窗口大小 N（各探针内部）
//! - 阈值 0.3/0.7（各探针内部）
//! - 级联风险权重 0.5/0.3/0.2（`compute_cascade_risk()`）

use thiserror::Error;

// ============================================================
// ProbeType — 探针类型枚举（不可进化面,决策 9）
// ============================================================

/// 探针类型 — 固定 3 变体（ADR-046 决策 9）
///
/// # 不可进化面
/// - `#[non_exhaustive]` 标注强制外部走 `probe_type()` 访问器,禁止外部 match
/// - 新增探针需 ADR + major 版本（决策 9）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProbeType {
    /// 记忆悖论探针（TemporalFilter + INV-8 单调归档,§8.2）
    MemoryParadox,
    /// 推理悖论探针（Fast Path 80% 跳过 + 自白通道,§8.2）
    ReasoningTrap,
    /// 进化悖论探针（RHI-CG 双通道 + 不可进化面 + R2 冻结线,§8.2）
    EvolutionHack,
}

impl ProbeType {
    /// 返回字符串标识（用于日志与序列化）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MemoryParadox => "memory_paradox",
            Self::ReasoningTrap => "reasoning_trap",
            Self::EvolutionHack => "evolution_hack",
        }
    }

    /// 返回所有探针类型（固定顺序,用于遍历）
    pub fn all() -> [ProbeType; 3] {
        [
            Self::MemoryParadox,
            Self::ReasoningTrap,
            Self::EvolutionHack,
        ]
    }
}

impl std::fmt::Display for ProbeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// Severity — 严重级别（与 §6.3 四档对齐）
// ============================================================

/// 探针报告严重级别（ADR-046 决策 2-4 阈值映射）
///
/// # 阈值映射（可进化面,决策 9）
/// - `paradox_rate > 0.3` → `Warning`
/// - `paradox_rate > 0.7` → `Critical`
/// - 其他 → `Normal`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// 正常（paradox_rate <= 0.3）
    Normal,
    /// 告警（0.3 < paradox_rate <= 0.7）
    Warning,
    /// 关键（paradox_rate > 0.7,触发膜增厚 + 熔断）
    Critical,
}

impl Severity {
    /// 根据 paradox_rate 推断严重级别（可进化面）
    pub fn from_paradox_rate(rate: f32) -> Self {
        // WHY f32 全程：§4.4 #6 红线禁止 f32 隐式转 f64 比较
        if rate > 0.7 {
            Self::Critical
        } else if rate > 0.3 {
            Self::Warning
        } else {
            Self::Normal
        }
    }

    /// 返回字符串标识
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

// ============================================================
// ParadoxReport — 单探针报告（不可进化面,决策 9）
// ============================================================

/// 单探针报告（ADR-046 决策 2-4 输出）
///
/// # 字段语义
/// - `probe_type`: 探针类型（与探针一一对应）
/// - `paradox_rate`: 悖论率 [0.0, 1.0]
/// - `severity`: 严重级别（由 `paradox_rate` 推断）
/// - `details`: 人类可读详情（用于审计日志）
/// - `insufficient_data`: 数据不足标记（事件未扩展时返回 true）
#[derive(Debug, Clone)]
pub struct ParadoxReport {
    /// 探针类型
    pub probe_type: ProbeType,
    /// 悖论率 [0.0, 1.0]
    pub paradox_rate: f32,
    /// 严重级别
    pub severity: Severity,
    /// 人类可读详情
    pub details: String,
    /// 数据不足标记（事件未扩展时为 true,ADR-046 决策 2）
    pub insufficient_data: bool,
}

impl ParadoxReport {
    /// 创建数据不足的报告（ADR-046 决策 2:`temporal_meta` 未扩展时返回）
    pub fn insufficient_data(probe_type: ProbeType) -> Self {
        Self {
            probe_type,
            paradox_rate: 0.0,
            severity: Severity::Normal,
            details: "insufficient data: event variants not extended".to_string(),
            insufficient_data: true,
        }
    }

    /// 创建正常报告
    pub fn new(probe_type: ProbeType, paradox_rate: f32, details: impl Into<String>) -> Self {
        let rate_clamped = paradox_rate.clamp(0.0, 1.0);
        Self {
            probe_type,
            paradox_rate: rate_clamped,
            severity: Severity::from_paradox_rate(rate_clamped),
            details: details.into(),
            insufficient_data: false,
        }
    }
}

// ============================================================
// ParadoxRiskReport — 三探针综合报告（不可进化面,决策 9）
// ============================================================

/// 三探针综合报告（ADR-046 决策 1 输出）
///
/// # 字段语义
/// - `reports`: 三探针报告数组（固定长度 3,INV-10）
/// - `cascade_risk`: 级联风险评分 [0.0, 1.0]（INV-12）
/// - `membrane_thickness`: 膜厚度 [0, 7]（INV-11）
/// - `timestamp`: 评估时间戳（Unix 毫秒）
#[derive(Debug, Clone)]
pub struct ParadoxRiskReport {
    /// 三探针报告（固定长度 3,INV-10）
    pub reports: [ParadoxReport; 3],
    /// 级联风险评分 [0.0, 1.0]（INV-12）
    pub cascade_risk: f32,
    /// 膜厚度 [0, 7]（INV-11）
    pub membrane_thickness: u8,
    /// 评估时间戳（Unix 毫秒）
    pub timestamp: u64,
}

impl ParadoxRiskReport {
    /// 返回最大 paradox_rate（用于级联风险评分）
    pub fn max_paradox_rate(&self) -> f32 {
        self.reports
            .iter()
            .map(|r| r.paradox_rate)
            .fold(0.0f32, f32::max)
    }

    /// 是否有任何 Critical 级探针
    pub fn has_critical(&self) -> bool {
        self.reports
            .iter()
            .any(|r| r.severity == Severity::Critical)
    }
}

// ============================================================
// ParadoxProbe trait — 探针统一接口（不可进化面,决策 9）
// ============================================================

/// 探针统一接口（ADR-046 决策 1）
///
/// # 实现契约
/// - 必须 `Send + Sync`（可在 async 任务间共享）
/// - `detect` 方法返回 `ParadoxReport`（异步,允许 IO）
/// - 实现不应 panic（可能导致 ImmuneSystem::scan 失败）
///
/// # 不可进化面（决策 9）
/// - trait 签名禁止 Harness spec 演化
/// - 实现数量固定为 3（INV-10）
pub trait ParadoxProbe: Send + Sync {
    /// 返回探针类型
    fn probe_type(&self) -> ProbeType;

    /// 执行检测,返回报告
    fn detect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ParadoxReport> + Send + 'a>>;
}

// ============================================================
// ImmuneSystemError — 错误类型（不可进化面,决策 9）
// ============================================================

/// ImmuneSystem 错误类型（§4.1：库层 thiserror enum）
///
/// WHY thiserror:库层错误用自定义 enum（§4.1）,应用层才用 anyhow。
#[derive(Debug, Error)]
pub enum ImmuneSystemError {
    /// 探针执行失败
    #[error("probe execution failed: {reason}")]
    ProbeExecutionFailed {
        /// 失败原因
        reason: String,
    },

    /// 事件总线错误
    #[error("event bus error: {reason}")]
    EventBusError {
        /// 失败原因
        reason: String,
    },

    /// 镜像状态陈旧
    #[error("stability mirror is stale: last_update_ts={last_update_ms}, now={now_ms}")]
    MirrorStale {
        /// 最后更新时间戳
        last_update_ms: u64,
        /// 当前时间戳
        now_ms: u64,
    },
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probe_type_all_returns_three() {
        let all = ProbeType::all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], ProbeType::MemoryParadox);
        assert_eq!(all[1], ProbeType::ReasoningTrap);
        assert_eq!(all[2], ProbeType::EvolutionHack);
    }

    #[test]
    fn test_probe_type_as_str() {
        assert_eq!(ProbeType::MemoryParadox.as_str(), "memory_paradox");
        assert_eq!(ProbeType::ReasoningTrap.as_str(), "reasoning_trap");
        assert_eq!(ProbeType::EvolutionHack.as_str(), "evolution_hack");
    }

    #[test]
    fn test_probe_type_display() {
        assert_eq!(ProbeType::MemoryParadox.to_string(), "memory_paradox");
    }

    #[test]
    fn test_severity_from_paradox_rate_normal() {
        assert_eq!(Severity::from_paradox_rate(0.0), Severity::Normal);
        assert_eq!(Severity::from_paradox_rate(0.3), Severity::Normal);
    }

    #[test]
    fn test_severity_from_paradox_rate_warning() {
        assert_eq!(Severity::from_paradox_rate(0.31), Severity::Warning);
        assert_eq!(Severity::from_paradox_rate(0.7), Severity::Warning);
    }

    #[test]
    fn test_severity_from_paradox_rate_critical() {
        assert_eq!(Severity::from_paradox_rate(0.71), Severity::Critical);
        assert_eq!(Severity::from_paradox_rate(1.0), Severity::Critical);
    }

    #[test]
    fn test_paradox_report_insufficient_data() {
        let r = ParadoxReport::insufficient_data(ProbeType::MemoryParadox);
        assert_eq!(r.probe_type, ProbeType::MemoryParadox);
        assert!(r.insufficient_data);
        assert!(r.paradox_rate.abs() < 1e-6);
        assert_eq!(r.severity, Severity::Normal);
    }

    #[test]
    fn test_paradox_report_new_clamps_rate() {
        let r = ParadoxReport::new(ProbeType::ReasoningTrap, 1.5, "overflow");
        assert!((r.paradox_rate - 1.0).abs() < 1e-6, "应 clamp 到 1.0");
        assert_eq!(r.severity, Severity::Critical);
        assert!(!r.insufficient_data);
    }

    #[test]
    fn test_paradox_report_new_negative_rate_clamped() {
        let r = ParadoxReport::new(ProbeType::EvolutionHack, -0.5, "negative");
        assert!(r.paradox_rate.abs() < 1e-6, "应 clamp 到 0.0");
        assert_eq!(r.severity, Severity::Normal);
    }

    #[test]
    fn test_paradox_risk_report_max_paradox_rate() {
        let report = ParadoxRiskReport {
            reports: [
                ParadoxReport::new(ProbeType::MemoryParadox, 0.2, "a"),
                ParadoxReport::new(ProbeType::ReasoningTrap, 0.6, "b"),
                ParadoxReport::new(ProbeType::EvolutionHack, 0.4, "c"),
            ],
            cascade_risk: 0.5,
            membrane_thickness: 3,
            timestamp: 1000,
        };
        assert!((report.max_paradox_rate() - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_paradox_risk_report_has_critical() {
        let report = ParadoxRiskReport {
            reports: [
                ParadoxReport::new(ProbeType::MemoryParadox, 0.2, "a"),
                ParadoxReport::new(ProbeType::ReasoningTrap, 0.8, "b"),
                ParadoxReport::new(ProbeType::EvolutionHack, 0.4, "c"),
            ],
            cascade_risk: 0.5,
            membrane_thickness: 3,
            timestamp: 1000,
        };
        assert!(report.has_critical());
    }

    #[test]
    fn test_paradox_risk_report_no_critical() {
        let report = ParadoxRiskReport {
            reports: [
                ParadoxReport::new(ProbeType::MemoryParadox, 0.2, "a"),
                ParadoxReport::new(ProbeType::ReasoningTrap, 0.4, "b"),
                ParadoxReport::new(ProbeType::EvolutionHack, 0.3, "c"),
            ],
            cascade_risk: 0.5,
            membrane_thickness: 3,
            timestamp: 1000,
        };
        assert!(!report.has_critical());
    }

    #[test]
    fn test_immune_system_error_probe_execution_failed_display() {
        let e = ImmuneSystemError::ProbeExecutionFailed {
            reason: "timeout".into(),
        };
        assert!(e.to_string().contains("timeout"));
    }

    #[test]
    fn test_immune_system_error_mirror_stale_display() {
        let e = ImmuneSystemError::MirrorStale {
            last_update_ms: 1000,
            now_ms: 10000,
        };
        let msg = e.to_string();
        assert!(msg.contains("1000"));
        assert!(msg.contains("10000"));
    }
}
