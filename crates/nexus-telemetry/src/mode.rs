//! 饱和处置策略 — 三态枚举与解析（ADR-197 D2）

/// 饱和处置策略（ADR-197 D2）
///
/// WHY 三态而不是一个布尔:计划的风险缓解要求“先以不改变分支的观测模式上线,
/// 拐点数据充足后再开降级”;单一布尔只能表达“降级 / 报错”两种行为变更,
/// 无法表达“只看不改分支”这个最安全的默认态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaturationMode {
    /// 默认:只记计数与发事件，任务**仍走池**（行为零改变，先测拐点）
    Observe,
    /// 超阈时改走 Inline 执行（变慢不变错，结果逐元素一致）
    Degrade,
    /// 超阈时不执行闭包，直接返回 `ComputeError::Saturated`（调用方自行重试）
    Strict,
}

/// 解析处置策略 —— 纯函数，便于表驱动单测
///
/// 非法值回落 [`SaturationMode::Observe`]（WHY:行为变更必须显式声明才能发生）。
/// 向后兼容:旧 `CHIMERA_COMPUTE_STRICT_POOL=1/true/on/yes` 等价于 `strict`。
#[must_use]
pub fn parse_saturation_mode(raw: Option<&str>, legacy_strict: Option<&str>) -> SaturationMode {
    if let Some(text) = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
    {
        return match text.as_str() {
            "observe" => SaturationMode::Observe,
            "degrade" | "inline" => SaturationMode::Degrade,
            "strict" | "error" => SaturationMode::Strict,
            // 空/非法值不猜意图:回到零行为变更的默认态
            _ => SaturationMode::Observe,
        };
    }
    // 兼容旧开关（只认肯定值,其他一律当未设置）
    match legacy_strict
        .map(str::trim)
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("1") | Some("true") | Some("on") | Some("yes") => SaturationMode::Strict,
        _ => SaturationMode::Observe,
    }
}
