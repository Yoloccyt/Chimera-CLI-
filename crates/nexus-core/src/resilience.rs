//! AERA 自适应错误恢复分配（P2-T6，手册 T-11 + v4.0 WI-19 前置）
//!
//! 对应架构层: **L1 Core**（nexus-core）
//! 对应任务: **P2-T6**（手册 W12-13）
//!
//! # 公式（手册 §10.2 骨架 + T-11）
//! `effort = 0.20·quota_pressure + 0.45·criticality + 0.35·error_ewma`（α = 0.3）
//!
//! effort ∈ [0,1] 决定：重试预算、回退层级、是否升级人工。
//! - `error_ewma`：近期错误率指数加权（α=0.3，历史占 0.7）
//! - 非对称迟滞（Hysteresis）：**升档快、降档慢**——错误上升立即反映，
//!   恢复后保持高位一段时间，防抖（避免 effort 在阈值边界震荡）
//!
//! # 确定性（Ω₂）
//! 纯函数 + 显式状态推进，同输入序列同输出；无时间/随机依赖。

/// 非对称迟滞器 — 升档快（raw 上穿阈值立即跟随）、降档慢（下穿需持续）
///
/// WHY 非对称：错误场景的代价是不对称的——低估 effort（错误升级时重试不足）
/// 比高估（多等一次恢复）代价更高。降档需连续 `down_ticks` 次低于阈值。
#[derive(Debug, Clone)]
pub struct Hysteresis {
    /// 升档阈值（raw 高于此立即跟随）
    pub rise_threshold: f64,
    /// 降档阈值（raw 低于此开始计数降档）
    pub fall_threshold: f64,
    /// 连续低于降档阈值多少次才降档（防抖）
    pub down_ticks_required: u32,
    /// 当前连续低计数
    down_ticks: u32,
}

impl Default for Hysteresis {
    fn default() -> Self {
        Self {
            rise_threshold: 0.55,
            fall_threshold: 0.35,
            down_ticks_required: 3,
            down_ticks: 0,
        }
    }
}

impl Hysteresis {
    /// 应用迟滞到原始 effort 值
    ///
    /// - raw > rise_threshold：立即升档（跟随 raw），清零降档计数
    /// - raw < fall_threshold：累计降档计数，达阈值才降档（否则保持上一档）
    /// - 中间区：保持上一档（迟滞带，防抖）
    #[must_use]
    pub fn apply(&mut self, raw: f64, current: f64) -> f64 {
        if raw > self.rise_threshold {
            self.down_ticks = 0;
            return raw;
        }
        if raw < self.fall_threshold {
            self.down_ticks = self.down_ticks.saturating_add(1);
            if self.down_ticks >= self.down_ticks_required {
                self.down_ticks = 0;
                return raw;
            }
            return current;
        }
        // 迟滞带（fall ≤ raw ≤ rise）：保持当前档
        self.down_ticks = 0;
        current
    }
}

/// AERA 自适应错误恢复分配器
#[derive(Debug, Clone)]
pub struct Aera {
    /// 近期错误率指数加权（α = 0.3）
    error_ewma: f64,
    /// 非对称迟滞（升档快、降档慢）
    hysteresis: Hysteresis,
    /// 当前 effort（上一轮输出，迟滞基准；None = 首次调用无历史）
    current_effort: Option<f64>,
}

impl Default for Aera {
    fn default() -> Self {
        Self::new()
    }
}

impl Aera {
    /// 新建分配器（初始 effort 0，错误 EWMA 0）
    #[must_use]
    pub fn new() -> Self {
        Self {
            error_ewma: 0.0,
            hysteresis: Hysteresis::default(),
            current_effort: None,
        }
    }

    /// 计算本轮 effort ∈ [0,1]
    ///
    /// # 参数
    /// - `quota_pressure`：配额压力 [0,1]（预算余量低 → 高）
    /// - `criticality`：任务关键性 [0,1]（关键任务 → 高）
    /// - `err`：本轮错误指示 [0,1]（0 = 成功，1 = 失败）
    ///
    /// # 语义
    /// effort 决定恢复投入：重试次数、回退层级、是否升级人工。
    /// 高 effort = 更激进恢复（更多重试 + 升级人工）；低 effort = 快速放弃。
    #[must_use]
    pub fn effort(&mut self, quota_pressure: f64, criticality: f64, err: f64) -> f64 {
        let err = err.clamp(0.0, 1.0);
        // EWMA 更新（α = 0.3：近期 30%，历史 70%）
        self.error_ewma = 0.3 * err + 0.7 * self.error_ewma;
        let raw = 0.20 * quota_pressure.clamp(0.0, 1.0)
            + 0.45 * criticality.clamp(0.0, 1.0)
            + 0.35 * self.error_ewma;
        let raw = raw.clamp(0.0, 1.0);
        // 首次调用（无 prior）直接跟随 raw——无历史可保持，迟滞不适用
        let current = self.current_effort.unwrap_or(raw);
        let next = self.hysteresis.apply(raw, current);
        self.current_effort = Some(next);
        next
    }

    /// 当前错误 EWMA（诊断）
    #[must_use]
    pub fn error_ewma(&self) -> f64 {
        self.error_ewma
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_weights_and_clamp() {
        let mut aera = Aera::new();
        // 全零输入 → effort 0
        assert_eq!(aera.effort(0.0, 0.0, 0.0), 0.0);
        // 临界性主导：criticality=1 → raw = 0.45（独立实例验证公式，避免迟滞保持干扰）
        let mut fresh = Aera::new();
        let e = fresh.effort(0.0, 1.0, 0.0);
        assert!((e - 0.45).abs() < 1e-9);
    }

    #[test]
    fn error_ewma_alpha_0_3() {
        let mut aera = Aera::new();
        let _ = aera.effort(0.0, 0.0, 1.0); // err=1 → ewma = 0.3
        assert!((aera.error_ewma() - 0.3).abs() < 1e-9);
        let _ = aera.effort(0.0, 0.0, 0.0); // err=0 → ewma = 0.3*0 + 0.7*0.3 = 0.21
        assert!((aera.error_ewma() - 0.21).abs() < 1e-9);
    }

    #[test]
    fn hysteresis_rise_fast_fall_slow() {
        let mut aera = Aera::new();
        // 错误上升：立即升档
        let e1 = aera.effort(1.0, 1.0, 1.0); // raw = 0.2+0.45+0.35*0.3 = 0.755 > 0.55 → 升
        assert!(e1 > 0.55, "错误场景必须立即升档, 实际 {e1}");
        // 恢复：raw 下降但需 3 次连续低于 0.35 才降档
        let e2 = aera.effort(0.0, 0.0, 0.0); // raw = 0.35*0.21 = 0.0735 < 0.35 → 计 1
        assert_eq!(e2, e1, "第一次低于阈值必须保持（降档慢）");
        let e3 = aera.effort(0.0, 0.0, 0.0); // 计 2
        assert_eq!(e3, e1, "第二次仍保持");
        let e4 = aera.effort(0.0, 0.0, 0.0); // 计 3 → 降档
        assert!(e4 < e1, "第三次连续低于阈值才降档");
    }

    #[test]
    fn hysteresis_band_holds() {
        let mut aera = Aera::new();
        let e1 = aera.effort(0.5, 0.5, 1.0); // raw = 0.1+0.225+0.105 = 0.43（迟滞带）
        let e2 = aera.effort(0.5, 0.5, 0.0);
        assert_eq!(e1, e2, "迟滞带内保持当前档");
    }

    #[test]
    fn deterministic_same_input_sequence() {
        let mut a = Aera::new();
        let mut b = Aera::new();
        for (q, c, e) in [(0.3, 0.7, 0.0), (0.9, 0.2, 1.0), (0.1, 0.1, 0.5)] {
            assert_eq!(
                a.effort(q, c, e),
                b.effort(q, c, e),
                "同序列必须逐位一致(Ω₂)"
            );
        }
    }
}
