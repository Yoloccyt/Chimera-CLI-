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

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::formal_props::VerificationResult;
use thiserror::Error;
use tracing::warn;

/// 复位授权构造错误(评审 S-2.1)
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResetAuthError {
    /// 授权方或理由为空 — 复位必须留下可追溯的问责记录
    #[error("复位授权无效: authorized_by 与 reason 均不可为空(复位须可问责)")]
    EmptyField,
}

/// 熔断器复位授权凭证(评审 S-2.1)
///
/// # WHY 需要授权凭证(安全边界的诚实声明)
///
/// 评审 S-2.1 指出:裸 `reset()` 使任何持 `&mut` 者可静默复位,"永久跳闸"名不副实。
/// 本凭证把复位从"无参数动作"改为"必须携带非空问责记录的动作":
///
/// - **强制问责**:`authorized_by`(授权方,应为治理法定人数如 "E01+E02")+
///   `reason`(复位理由)均不可为空,构造期即校验——杜绝"无记录静默复位"。
/// - **审计留存**:凭证被熔断器留存(`last_reset`),复位事实与授权者可事后追溯。
///
/// # 边界诚实声明
///
/// 单进程 Rust 库层面**无法密码学阻止**持 `&mut` 的代码构造本凭证并复位——
/// 真正的密码学授权需外部签名基础设施(超出本层)。本凭证提供的是**可问责性
/// 防线**:复位不可能匿名/意外发生,必留下 who+why 记录。密码学级授权由上层
/// 特权审批流(阶段③ 编排器)在此凭证之上叠加。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetAuthorization {
    /// 授权方标识(应代表治理法定人数,如 "E01+E02")
    authorized_by: String,
    /// 复位理由(审计追溯,如已排查的跳闸根因与修复措施)
    reason: String,
}

impl ResetAuthorization {
    /// 构造复位授权凭证,`authorized_by` 与 `reason` 均不可为空(去空白后非空)
    ///
    /// # 错误
    /// 任一字段去除首尾空白后为空 → [`ResetAuthError::EmptyField`]。
    pub fn new(
        authorized_by: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, ResetAuthError> {
        let authorized_by = authorized_by.into();
        let reason = reason.into();
        if authorized_by.trim().is_empty() || reason.trim().is_empty() {
            return Err(ResetAuthError::EmptyField);
        }
        Ok(Self {
            authorized_by,
            reason,
        })
    }

    /// 授权方标识
    #[must_use]
    pub fn authorized_by(&self) -> &str {
        &self.authorized_by
    }

    /// 复位理由
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

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
/// 纯状态机,不执行 RL 训练(L4 深度优化 P1-1:跳闸时经可选 EventBus
/// 发布 ShadowBreakerTripped 事件)。
///
/// L4 深度优化:Debug derive 改手动实现(EventBus 不含 Debug,字段显示
/// 占位 "Some(EventBus)"/"None"),Clone 保持 EventBus Arc 语义——
/// 公共 API 形态不变(chimera-mas 依赖 Debug 格式化)。
#[derive(Clone)]
pub struct ShadowModeCircuitBreaker {
    /// 当前状态
    state: BreakerState,
    /// 跳闸原因(首次触发跳闸的违规反例;未跳闸为 None)
    trip_cause: Option<String>,
    /// 累计观测周期数(审计用)
    observations: u64,
    /// 最近一次复位的授权凭证(评审 S-2.1,审计留存;从未复位为 None)
    last_reset: Option<ResetAuthorization>,
    /// 可选事件总线(L4 深度优化 P1-1):跳闸时发布 ShadowBreakerTripped
    /// 事件供订阅方派生熔断状态;None = 不发布(new() 兼容路径,测试与
    /// 无总线装配场景零行为变化)
    bus: Option<EventBus>,
}

impl Default for ShadowModeCircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ShadowModeCircuitBreaker {
    /// L4 深度优化:EventBus 不实现 Debug,手动实现保持公共 Debug 形态;
    /// bus 字段以 Some/None 占位显示(不泄露总线内部状态)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShadowModeCircuitBreaker")
            .field("state", &self.state)
            .field("trip_cause", &self.trip_cause)
            .field("observations", &self.observations)
            .field("last_reset", &self.last_reset)
            .field(
                "bus",
                &if self.bus.is_some() {
                    "Some(EventBus)"
                } else {
                    "None"
                },
            )
            .finish()
    }
}

impl ShadowModeCircuitBreaker {
    /// 创建武装态熔断器(初始许可监控,尚未观测,无事件总线)
    pub fn new() -> Self {
        Self {
            state: BreakerState::Armed,
            trip_cause: None,
            observations: 0,
            last_reset: None,
            bus: None,
        }
    }

    /// 创建带事件总线的熔断器(L4 深度优化 P1-1,事件驱动化)
    ///
    /// 跳闸时经 `publish_blocking` 发布 `ShadowBreakerTripped` 事件
    /// (同步方法正确发布模式,§4.4 #8),供 TUI DecayPanel 等订阅方从
    /// 事件流派生熔断状态显示——替代原 shadow_breaker_status() 全局
    /// 函数占位(Ω₄-Event:所有跨层通信走 EventBus)。
    ///
    /// # 装配方式
    /// 上层编排器(chimera-mas shadow/orchestrator)持有熔断器实例时:
    /// ```no_run
    /// # use decay_engine::ShadowModeCircuitBreaker;
    /// # use event_bus::EventBus;
    /// let breaker = ShadowModeCircuitBreaker::with_event_bus(EventBus::new());
    /// ```
    pub fn with_event_bus(bus: EventBus) -> Self {
        Self {
            state: BreakerState::Armed,
            trip_cause: None,
            observations: 0,
            last_reset: None,
            bus: Some(bus),
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
            // L4 深度优化 P1-1:跳闸时发布 ShadowBreakerTripped 事件(事件驱动化)
            // 发布失败仅 warn(熔断状态本身已完成跳闸,事件丢失不影响 fail-closed 语义)
            if let Some(bus) = &self.bus {
                let event = NexusEvent::ShadowBreakerTripped {
                    metadata: EventMetadata::new("decay-engine:shadow_breaker"),
                    reason: cause.clone(),
                };
                if let Err(e) = bus.publish_blocking(event) {
                    warn!(error = %e, "ShadowBreakerTripped 事件发布失败(跳闸已完成)");
                }
            }
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

    /// 人工复位熔断器 → 回到武装态(评审 S-2.1:须携带授权凭证)
    ///
    /// # 安全考量
    ///
    /// 复位是**人工干预动作**:调用方必须先排查跳闸根因(`trip_cause`)、
    /// 确认形式化违规已修复,再复位。复位须携带 [`ResetAuthorization`] 凭证
    /// (非空 who+why),凭证被留存供审计(`last_reset`)——杜绝匿名/意外复位。
    /// 不提供"自动复位"——防止瞬时抖动掩盖真实安全问题。
    ///
    /// # 参数
    /// - `authorization`: 复位授权凭证(非空授权方 + 理由,构造期已校验)
    pub fn reset(&mut self, authorization: ResetAuthorization) {
        self.state = BreakerState::Armed;
        self.trip_cause = None;
        // observations 累计不清零(审计连续性)
        self.last_reset = Some(authorization);
    }

    /// 最近一次复位的授权凭证(从未复位为 None)— 审计查询用
    #[must_use]
    pub fn last_reset(&self) -> Option<&ResetAuthorization> {
        self.last_reset.as_ref()
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
        cb.reset(ResetAuthorization::new("E01+E02", "已排查跳闸根因并修复").unwrap());
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
        cb.reset(ResetAuthorization::new("E01+E02", "测试复位").unwrap());
        cb.observe(&[satisfied()]);
        // 复位不清零观测计数(审计连续性)
        assert_eq!(cb.observations(), 3);
    }

    // ============================================================
    // 评审 S-2.1:复位授权凭证约束
    // ============================================================

    #[test]
    fn test_reset_authorization_rejects_empty_fields() {
        // 空授权方或空理由 → 构造失败(杜绝无问责复位)
        assert_eq!(
            ResetAuthorization::new("", "理由"),
            Err(ResetAuthError::EmptyField)
        );
        assert_eq!(
            ResetAuthorization::new("E01", ""),
            Err(ResetAuthError::EmptyField)
        );
        assert_eq!(
            ResetAuthorization::new("   ", "理由"),
            Err(ResetAuthError::EmptyField),
            "纯空白应视为空"
        );
    }

    #[test]
    fn test_reset_records_authorization_for_audit() {
        let mut cb = ShadowModeCircuitBreaker::new();
        cb.observe(&[violated("属性违反")]);
        assert!(cb.is_tripped());
        assert!(cb.last_reset().is_none(), "复位前无授权记录");

        let auth = ResetAuthorization::new("E01+E02", "根因已修复:XX").unwrap();
        cb.reset(auth);
        // 复位后授权凭证被留存供审计
        let recorded = cb.last_reset().expect("复位后应有授权记录");
        assert_eq!(recorded.authorized_by(), "E01+E02");
        assert!(recorded.reason().contains("根因已修复"));
    }

    #[test]
    fn test_valid_authorization_accessors() {
        let auth = ResetAuthorization::new("E02", "安全复核通过").unwrap();
        assert_eq!(auth.authorized_by(), "E02");
        assert_eq!(auth.reason(), "安全复核通过");
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
