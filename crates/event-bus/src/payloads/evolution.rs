//! 进化 / 回滚诊断辅助类型
//!
//! WHY 独立成模块：本组类型只服务于 `进化 / 回滚诊断辅助类型` 一个领域，
//! 与其它域的载荷无耦合（同文件平铺时改任一领域都要翻找全部类型）。
//!
//! 对外路径不变：全部类型经 `payloads/mod.rs` 的 facade 转出，
//! `event_bus::payloads::X` 逐字不变（ADR-314 W6 / BC-03 域下沉）。
//!
//! 类型：`RollbackTriggerType` / `RollbackDiagnosticContext`（ADR-043 决策 4）。

use serde::{Deserialize, Serialize};

/// R1 影子模式回滚触发条件类型(ADR-043 决策 4)— P2-13 结构化理由记录
///
/// 对应 ADR-043 决策 4 定义的 4 种回滚触发条件 + Unknown 兜底。
///
/// WHY 枚举而非 String:结构化记录便于审计图表按触发条件分类统计,
/// 同时支持 efficiency-monitor 告警规则精确匹配(避免字符串模糊匹配的歧义)。
/// 旧版 `R1ShadowRollbackFailed.reason: String` 保留为人类可读描述,
/// `trigger_type` 字段提供机器可读的分类标签。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackTriggerType {
    /// 连续 3 天 R1 显著差于 L3(决策 4 触发条件 1)
    ///
    /// 对应 `ShadowComparisonReport::ComparisonResult::R1SignificantlyWorse`
    /// 连续出现 3 天的退化模式。
    ConsecutiveRegression,

    /// AsaIntervention 触发(决策 4 触发条件 2)
    ///
    /// R1 接缝触发 AsaIntervention 事件,ADR-037 决策 3 自动置为 Cooldown。
    AsaIntervention,

    /// EWMA 崩塌:24h 内下降 ≥ 0.3(决策 4 触发条件 3)
    ///
    /// 如从 0.7 跌至 0.4 以下,表明 R1 训练策略严重退化。
    EwmaCollapse,

    /// 召回率显著下降:较 L3 基线下降 ≥ 5%(决策 4 触发条件 4)
    ///
    /// 绝对值下降,如从 95% 降至 90%。
    RecallRateDrop,

    /// 未知触发条件(兜底)
    ///
    /// 用于未分类的回滚失败,如 CapabilityTokenRegistry 内部错误导致的
    /// 回滚失败(不属于 4 种退化模式,但回滚操作本身失败)。
    Unknown,
}

impl Default for RollbackTriggerType {
    /// 默认值 = Unknown
    ///
    /// WHY Unknown 而非 ConsecutiveRegression:确保未显式设置 trigger_type 的
    /// 旧版本事件反序列化后不会误归类为某一具体触发条件。
    fn default() -> Self {
        Self::Unknown
    }
}

impl RollbackTriggerType {
    /// 获取触发条件的人类可读描述(用于日志与 TUI 展示)
    pub fn description(&self) -> &'static str {
        match self {
            Self::ConsecutiveRegression => "R1 significantly worse than L3 for 3 consecutive days",
            Self::AsaIntervention => "ASA intervention triggered on R1 seam",
            Self::EwmaCollapse => "EWMA collapsed by >=0.3 within 24h",
            Self::RecallRateDrop => "Recall rate dropped >=5% vs L3 baseline",
            Self::Unknown => "Unknown rollback trigger",
        }
    }
}

/// R1 影子模式回滚诊断上下文(ADR-043 决策 4)— P2-13 结构化理由记录
///
/// 承载回滚失败时的诊断快照,便于专家团队复盘根因。
///
/// # 设计原则
/// - **所有字段 Option**:不同触发条件(`RollbackTriggerType`)只有部分字段有意义。
///   如 `ConsecutiveRegression` 触发时 `regression_streak` 有值但 `recall_rate_drop` 无意义。
/// - **`#[serde(default)]` 全字段**:确保旧版本序列化的事件(无此结构体)能被
///   反序列化为全 None 的默认值,保证向后兼容。
/// - **纯数据类型**:无方法,仅承载快照,便于序列化与持久化到审计日志。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RollbackDiagnosticContext {
    /// 触发回滚时的 EWMA 水平(None 表示未测量)
    ///
    /// 对应 `CapabilityToken.level` 字段快照。`EwmaCollapse` 触发条件必填。
    #[serde(default)]
    pub ewma_level: Option<f32>,

    /// 观察期已进行天数(None 表示未进入观察期)
    ///
    /// 范围 [0, 14],14 天为 ADR-043 决策 3 定义的完整观察期。
    #[serde(default)]
    pub observation_days: Option<u32>,

    /// 连续退化天数(None 表示非 ConsecutiveRegression 触发)
    ///
    /// `ConsecutiveRegression` 触发条件必填,值为 `regression_streak`(≥3 满足触发)。
    #[serde(default)]
    pub regression_streak: Option<u32>,

    /// 召回率下降幅度(绝对值,None 表示非 RecallRateDrop 触发)
    ///
    /// 如 0.05 表示下降 5%。`RecallRateDrop` 触发条件必填,值 ≥ 0.05。
    #[serde(default)]
    pub recall_rate_drop: Option<f32>,

    /// 回滚目标版本号(None 表示无版本号)
    ///
    /// R1 训练策略的版本号,用于追踪具体哪个版本的策略导致退化。
    #[serde(default)]
    pub rollback_target_version: Option<u32>,
}

impl RollbackDiagnosticContext {
    /// 创建空的诊断上下文(所有字段 None)
    ///
    /// 用于触发条件未知或不需要诊断上下文的场景。
    pub fn empty() -> Self {
        Self::default()
    }

    /// builder 模式:设置 EWMA 水平
    pub fn with_ewma_level(mut self, level: f32) -> Self {
        self.ewma_level = Some(level);
        self
    }

    /// builder 模式:设置观察期天数
    pub fn with_observation_days(mut self, days: u32) -> Self {
        self.observation_days = Some(days);
        self
    }

    /// builder 模式:设置连续退化天数
    pub fn with_regression_streak(mut self, streak: u32) -> Self {
        self.regression_streak = Some(streak);
        self
    }

    /// builder 模式:设置召回率下降幅度
    pub fn with_recall_rate_drop(mut self, drop: f32) -> Self {
        self.recall_rate_drop = Some(drop);
        self
    }

    /// builder 模式:设置回滚目标版本号
    pub fn with_rollback_target_version(mut self, version: u32) -> Self {
        self.rollback_target_version = Some(version);
        self
    }
}
