//! 影子模式熔断开关 — R2 解冻阶段③ 前置 3(fail-closed 安全护栏)
//!
//! 对应架构层: L4 Security(decay-engine,复用 Freeze 不可逆哲学)
//! 对应 ADR: ADR-052 待办 3(影子模式回滚熔断)+ ADR-042(R2 冻结)
//! 对应计划: `IMPLEMENTATION_PLAN_phase8_formal_verifier_m2.md` R2 解冻阶段③
//!
//! # 职责:影子模式的 fail-closed 安全护栏
//!
//! ADR-042 定义 R2 解冻阶段③ 为"影子模式运行 2 周,无异常方可正式解冻"。
//! 本熔断器是该阶段的**安全网**:它消费 7 个 FormalVerifier 验证器的输出
//! (`VerificationResult` 流),门控影子模式下的 RL 更新是否被许可。
//!
//! ## 为什么是安全强化而非解冻(WHY 本模块不违反 R2 冻结)
//!
//! 本熔断器**不执行任何 RL 训练、不含 5 个 R2 扫描关键词**。它只做一件事:
//! 在形式化验证器报告任一属性 `Violated` 时,**永久跳闸**(fail-closed),
//! 拒绝后续所有 RL 更新直至人工复位。这是解冻前必须先就位的护栏——
//! 构建它**收紧**而非放松安全约束。
//!
//! ## fail-closed 三态语义(安全考量)
//!
//! 熔断器的核心安全原则是"无正面验证证据即拒绝":
//!
//! 1. **任一 `Violated`** → 永久跳闸(`Tripped`),Denied,不可逆直至 `reset()`
//!    —— 复用 decay-engine `Freeze` 的不可逆哲学:安全违规后权限不残留
//! 2. **≥1 `Satisfied` 且 0 `Violated`** → 许可(`Armed`),RL 更新放行
//!    —— 必须有**正面**验证证据才放行,非"没发现问题就放行"
//! 3. **全 `Skipped` 或空** → 瞬态拒绝(`Armed` 但本周期不许可)
//!    —— 无验证证据 ≠ 通过验证;fail-closed 下拒绝,但不永久跳闸
//!    (Skipped 是"未验证"非"验证失败",不构成违规)
//!
//! # 风险控制
//!
//! - **不可逆跳闸**:一旦跳闸,即使后续观测全 Satisfied 也保持跳闸,
//!   必须人工 `reset()`(调用方须审计记录复位原因)——防止瞬时抖动
//!   自动恢复掩盖真实安全问题
//! - **跳闸原因留档**:`trip_cause` 记录首次触发跳闸的违规反例,供审计追溯

use nexus_contracts::formal_props::VerificationResult;

/// 熔断器状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// 武装态:监控中,满足条件时许可 RL 更新
    Armed,
    /// 跳闸态:检测到形式化属性违规,永久拒绝 RL 更新直至人工复位
    Tripped,
}

/// RL 更新门控裁决
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RlGateVerdict {
    /// 许可:形式化验证有正面证据(≥1 Satisfied 且 0 Violated)
    Permitted,
    /// 拒绝:携带拒绝原因(违规反例 / 证据不足)
    Denied {
        /// 拒绝原因(人类可读,供审计)
        reason: String,
    },
}

impl RlGateVerdict {
    /// 是否许可 RL 更新
    #[must_use]
    pub fn is_permitted(&self) -> bool {
        matches!(self, Self::Permitted)
    }
}

/// 影子模式熔断开关 — fail-closed RL 更新门控状态机
///
/// 消费 FormalVerifier 验证结果流,门控影子模式下 RL 更新的许可。
/// 纯状态机,不执行 RL 训练,不发布事件(Critical 事件由调用方发布,§6.2)。
#[derive(Debug, Clone)]
pub struct ShadowModeCircuitBreaker {
    /// 当前状态
    state: BreakerState,
    /// 跳闸原因(首次触发跳闸的违规反例;未跳闸为 None)
    trip_cause: Option<String>,
    /// 累计观测周期数(审计用)
    observations: u64,
}

impl Default for ShadowModeCircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl ShadowModeCircuitBreaker {
    /// 创建武装态熔断器(初始许可监控,尚未观测)
    pub fn new() -> Self {
        Self {
            state: BreakerState::Armed,
            trip_cause: None,
            observations: 0,
        }
    }

    /// 观测一批验证结果,更新状态并返回 RL 门控裁决
    ///
    /// # fail-closed 逻辑(见模块文档三态语义)
    ///
    /// 1. 已跳闸 → 保持跳闸,Denied(不可逆,忽略本次观测内容)
    /// 2. 任一 Violated → 跳闸,记录反例,Denied
    /// 3. ≥1 Satisfied 且 0 Violated → 保持 Armed,Permitted
    /// 4. 全 Skipped / 空 → 保持 Armed,瞬态 Denied(证据不足)
    ///
    /// # 参数
    /// - `results`: 本周期 FormalVerifier 验证器输出(7 属性验证结果)
    ///
    /// # 返回
    /// RL 更新门控裁决(Permitted / Denied)
    pub fn observe(&mut self, results: &[VerificationResult]) -> RlGateVerdict {
        self.observations += 1;

        // 1. 已跳闸:不可逆,忽略本次观测(fail-closed)
        if self.state == BreakerState::Tripped {
            return RlGateVerdict::Denied {
                reason: format!(
                    "熔断器已跳闸(不可逆直至人工复位);原因: {}",
                    self.trip_cause.as_deref().unwrap_or("未知")
                ),
            };
        }

        // 2. 扫描违规:任一 Violated 即永久跳闸
        if let Some(counterexample) = results.iter().find_map(|r| match r {
            VerificationResult::Violated { counterexample, .. } => Some(counterexample.clone()),
            _ => None,
        }) {
            let cause = format!("形式化属性被违反: {counterexample}");
            self.state = BreakerState::Tripped;
            self.trip_cause = Some(cause.clone());
            return RlGateVerdict::Denied { reason: cause };
        }

        // 3. 需正面证据:至少一个 Satisfied 才放行(非"没问题就放行")
        let has_satisfied = results.iter().any(VerificationResult::is_satisfied);
        if has_satisfied {
            RlGateVerdict::Permitted
        } else {
            // 4. 全 Skipped / 空:证据不足,瞬态拒绝(不跳闸)
            RlGateVerdict::Denied {
                reason: "验证证据不足(无 Satisfied 结果),fail-closed 拒绝".to_string(),
            }
        }
    }

    /// 当前状态
    #[must_use]
    pub fn state(&self) -> BreakerState {
        self.state
    }

    /// 是否已跳闸
    #[must_use]
    pub fn is_tripped(&self) -> bool {
        self.state == BreakerState::Tripped
    }

    /// 跳闸原因(未跳闸为 None)
    #[must_use]
    pub fn trip_cause(&self) -> Option<&str> {
        self.trip_cause.as_deref()
    }

    /// 累计观测周期数
    #[must_use]
    pub fn observations(&self) -> u64 {
        self.observations
    }

    /// 人工复位熔断器 → 回到武装态
    ///
    /// # 安全考量
    ///
    /// 复位是**人工干预动作**:调用方必须先排查跳闸根因(`trip_cause`)、
    /// 确认形式化违规已修复,再复位。调用方须审计记录复位事件(§6.2)。
    /// 不提供"自动复位"——防止瞬时抖动掩盖真实安全问题。
    pub fn reset(&mut self) {
        self.state = BreakerState::Armed;
        self.trip_cause = None;
        // observations 累计不清零(审计连续性)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn satisfied() -> VerificationResult {
        VerificationResult::Satisfied { samples_tested: 10 }
    }

    fn violated(msg: &str) -> VerificationResult {
        VerificationResult::Violated {
            counterexample: msg.to_string(),
            samples_tested: 5,
        }
    }

    fn skipped() -> VerificationResult {
        VerificationResult::Skipped {
            reason: "前置不满足".to_string(),
        }
    }

    #[test]
    fn test_new_is_armed() {
        let cb = ShadowModeCircuitBreaker::new();
        assert_eq!(cb.state(), BreakerState::Armed);
        assert!(!cb.is_tripped());
        assert_eq!(cb.observations(), 0);
    }

    #[test]
    fn test_all_satisfied_permits() {
        let mut cb = ShadowModeCircuitBreaker::new();
        let verdict = cb.observe(&[satisfied(), satisfied(), satisfied()]);
        assert!(verdict.is_permitted());
        assert_eq!(cb.state(), BreakerState::Armed);
        assert_eq!(cb.observations(), 1);
    }

    #[test]
    fn test_any_violated_trips_and_denies() {
        let mut cb = ShadowModeCircuitBreaker::new();
        let verdict = cb.observe(&[satisfied(), violated("衰减单调性违反"), satisfied()]);
        assert!(!verdict.is_permitted());
        assert!(cb.is_tripped());
        assert!(cb.trip_cause().unwrap().contains("衰减单调性违反"));
    }

    #[test]
    fn test_trip_is_irreversible_until_reset() {
        let mut cb = ShadowModeCircuitBreaker::new();
        cb.observe(&[violated("属性违反")]);
        assert!(cb.is_tripped());

        // 后续即使全 Satisfied 也保持跳闸(不可逆)
        let verdict = cb.observe(&[satisfied(), satisfied()]);
        assert!(!verdict.is_permitted(), "跳闸后即使全通过也应拒绝");
        assert!(cb.is_tripped());

        // 人工复位后恢复武装态
        cb.reset();
        assert_eq!(cb.state(), BreakerState::Armed);
        assert!(cb.trip_cause().is_none());
        // 复位后重新许可
        assert!(cb.observe(&[satisfied()]).is_permitted());
    }

    #[test]
    fn test_all_skipped_denies_without_tripping() {
        let mut cb = ShadowModeCircuitBreaker::new();
        let verdict = cb.observe(&[skipped(), skipped()]);
        // 证据不足:拒绝但不跳闸
        assert!(!verdict.is_permitted());
        assert!(!cb.is_tripped(), "全 Skipped 是证据不足,非违规,不应跳闸");
        assert_eq!(cb.state(), BreakerState::Armed);
    }

    #[test]
    fn test_empty_denies_without_tripping() {
        let mut cb = ShadowModeCircuitBreaker::new();
        let verdict = cb.observe(&[]);
        assert!(!verdict.is_permitted(), "空观测证据不足,fail-closed 拒绝");
        assert!(!cb.is_tripped());
    }

    #[test]
    fn test_mixed_satisfied_and_skipped_permits() {
        let mut cb = ShadowModeCircuitBreaker::new();
        // 有正面证据(Satisfied)且无违规 → 许可
        let verdict = cb.observe(&[satisfied(), skipped()]);
        assert!(verdict.is_permitted());
    }

    #[test]
    fn test_observations_accumulate_across_reset() {
        let mut cb = ShadowModeCircuitBreaker::new();
        cb.observe(&[satisfied()]);
        cb.observe(&[violated("x")]);
        cb.reset();
        cb.observe(&[satisfied()]);
        // 复位不清零观测计数(审计连续性)
        assert_eq!(cb.observations(), 3);
    }

    #[test]
    fn test_violated_denied_reason_carries_counterexample() {
        let mut cb = ShadowModeCircuitBreaker::new();
        let verdict = cb.observe(&[violated("INV-9 委托环")]);
        match verdict {
            RlGateVerdict::Denied { reason } => {
                assert!(reason.contains("INV-9 委托环"));
            }
            RlGateVerdict::Permitted => panic!("违规应拒绝"),
        }
    }
}
