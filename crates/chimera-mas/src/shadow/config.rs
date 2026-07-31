//! 影子模式治理配置 — 签署强制 + fail-closed 构造
//!
//! 对应架构层: L9 Quest(chimera-mas shadow 子模块)
//! 对应 ADR: ADR-053-rev4(s_min 治理签署档)+ ADR-053-rev3 决策 3A″-P(w_min/N_probe)
//!
//! # 核心职责:把"治理签署"从文档承诺升级为构造期强制
//!
//! [`ShadowModeConfig`] 是 ShadowModeOrchestrator 的唯一构造入口参数:
//!
//! - **无签署即无实例**(fail-closed):[`GovernanceSignoff`] 三字段
//!   (signed_by / adr_ref / signed_at)去空白后非空强制,构造期校验——
//!   仿 decay-engine `ResetAuthorization` 的问责凭证设计(评审 S-2.1)
//! - **参数只能收紧不能放松**:s_min 各维不得低于 rev4 治理签署的锚点档
//!   (A≥0.9 / B≥0.5 / C≥0.5);N_probe 不得低于 100 固定下限
//!   (rev4 决策 3A″-P2 删除"升 300"悬空口子后的唯一下限);
//!   权重各 ∈ [0.20, 0.60] 且和为 1(rev3 决策 3A″-P w_min + 单维上限)
//! - **无 `Default` 实现**:杜绝绕过签署的默认档
//!
//! # s_min 锚点依据(rev4 决策 3A″-P2,治理签署 2026-07-29)
//!
//! | 维 | 锚点 | 依据 |
//! |----|------|------|
//! | A 执行面 | ≥0.9 | pvl-layer `VERIFY_PASS_RATE_THRESHOLD`(执行硬门前提) |
//! | B 变异分 | ≥0.5 | pvl ProcessScorer `LOW_QUALITY_THRESHOLD`(低于半数变异杀伤=测试套件不合格) |
//! | C held-out | ≥0.5 | 未见任务至少半数胜(低于即过拟合信号,与 B 同档) |
//!
//! 最终档位仍待用户在阶段③ 前确认;调整只影响配置数值,硬门语义
//! (绝对分 < s_min 即非胜)不变。

use crate::error::{MasError, Result};

// ============================================================
// 治理锚点常量(rev4 治理签署档,配置校验的不可放松下限)
// ============================================================

/// A 维执行面 s_min 锚点(锚定 pvl `VERIFY_PASS_RATE_THRESHOLD` = 0.9)
pub const S_MIN_EXECUTION_ANCHOR: f32 = 0.9;

/// B 维变异分 s_min 锚点(锚定 pvl ProcessScorer `LOW_QUALITY_THRESHOLD` = 0.5)
pub const S_MIN_MUTATION_ANCHOR: f32 = 0.5;

/// C 维 held-out s_min 锚点(与 B 同档,rev4 决策 3A″-P2)
pub const S_MIN_HELD_OUT_ANCHOR: f32 = 0.5;

/// AHIRT 探测总数固定下限(rev4 决策 3A″-P2:N_probe=100,"升 300"已删除)
pub const N_PROBE_FLOOR: u32 = 100;

/// AHIRT 每攻击类别探测数下限(rev3 决策 3A″-P:每类 ≥25)
pub const PER_CATEGORY_FLOOR: u32 = 25;

/// 权重下界 w_min = 0.20(rev3 决策 3A″-P:防权重归零)
pub const WEIGHT_MIN: f32 = 0.20;

/// 单维权重上限 = 0.60(rev3 决策 3A″-P:防权重集中)
pub const WEIGHT_MAX: f32 = 0.60;

/// ε_win 平局带下限 = 0.02(rev3 决策 3C-P:ε_win = max(0.02, 2σ̂_d/√m))
pub const EPSILON_WIN_FLOOR: f64 = 0.02;

/// 权重和校验容差(f32 三数相加的浮点误差余量)
const WEIGHT_SUM_TOLERANCE: f32 = 1e-4;

// ============================================================
// 治理签署凭证
// ============================================================

/// 治理签署凭证 — 影子模式配置的问责记录(fail-closed 前置)
///
/// # WHY 需要签署凭证
///
/// ADR-053 系列历轮声明"虚拟专家复评 ≠ 治理签署"。本凭证把"配置由谁
/// 依据哪份 ADR 于何时批准"固化为构造期强制字段,杜绝无签署配置启动
/// 影子模式编排。与 decay-engine `ResetAuthorization` 同为可问责性防线:
/// 单进程库层面无法密码学阻止伪造,但保证配置来源不可能匿名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceSignoff {
    /// 签署方标识(治理权威,如 "user")
    signed_by: String,
    /// 签署依据的 ADR 引用(如 "ADR-053-rev4 + audit/ADR-053-governance-signoff-2026-07-29.md")
    adr_ref: String,
    /// 签署日期(ISO 8601,如 "2026-07-29")
    signed_at: String,
}

impl GovernanceSignoff {
    /// 构造签署凭证,三字段去除首尾空白后均不可为空
    ///
    /// # 错误
    /// 任一字段为空 → [`MasError::ShadowGovernanceConfigInvalid`]
    pub fn new(
        signed_by: impl Into<String>,
        adr_ref: impl Into<String>,
        signed_at: impl Into<String>,
    ) -> Result<Self> {
        let signed_by = signed_by.into();
        let adr_ref = adr_ref.into();
        let signed_at = signed_at.into();
        if signed_by.trim().is_empty() || adr_ref.trim().is_empty() || signed_at.trim().is_empty() {
            return Err(MasError::ShadowGovernanceConfigInvalid {
                reason: "治理签署凭证三字段(signed_by/adr_ref/signed_at)均不可为空".into(),
            });
        }
        Ok(Self {
            signed_by,
            adr_ref,
            signed_at,
        })
    }

    /// 签署方标识
    #[must_use]
    pub fn signed_by(&self) -> &str {
        &self.signed_by
    }

    /// 签署依据的 ADR 引用
    #[must_use]
    pub fn adr_ref(&self) -> &str {
        &self.adr_ref
    }

    /// 签署日期
    #[must_use]
    pub fn signed_at(&self) -> &str {
        &self.signed_at
    }
}

// ============================================================
// 外部证据门权重
// ============================================================

/// A/B/C 三维加权权重 — 构造期强制 rev3 约束(w_min/上限/归一)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvidenceWeights {
    /// A 维执行面权重
    pub execution: f32,
    /// B 维变异分权重
    pub mutation: f32,
    /// C 维 held-out 权重
    pub held_out: f32,
}

impl EvidenceWeights {
    /// 构造并校验权重(各 ∈ [0.20, 0.60],和 = 1.0)
    ///
    /// # 错误
    /// 违反任一约束 → [`MasError::ShadowGovernanceConfigInvalid`]
    pub fn new(execution: f32, mutation: f32, held_out: f32) -> Result<Self> {
        for (name, w) in [
            ("execution", execution),
            ("mutation", mutation),
            ("held_out", held_out),
        ] {
            if !(WEIGHT_MIN..=WEIGHT_MAX).contains(&w) {
                return Err(MasError::ShadowGovernanceConfigInvalid {
                    reason: format!(
                        "权重 {name}={w} 越界:须 ∈ [{WEIGHT_MIN}, {WEIGHT_MAX}](rev3 决策 3A″-P)"
                    ),
                });
            }
        }
        let sum = execution + mutation + held_out;
        if (sum - 1.0).abs() > WEIGHT_SUM_TOLERANCE {
            return Err(MasError::ShadowGovernanceConfigInvalid {
                reason: format!("权重和 {sum} ≠ 1.0(容差 {WEIGHT_SUM_TOLERANCE})"),
            });
        }
        Ok(Self {
            execution,
            mutation,
            held_out,
        })
    }
}

// ============================================================
// 影子模式配置
// ============================================================

/// 影子模式编排配置 — 构造期完成全部治理校验(fail-closed)
///
/// 字段全私有:构造后不可变(数值调整须重新构造 = 重新过签署校验),
/// 经 getter 只读访问。**刻意不实现 `Default`**,杜绝绕过签署的默认档。
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowModeConfig {
    /// 治理签署凭证
    signoff: GovernanceSignoff,
    /// A 维执行面 s_min 硬门(≥ 0.9 锚点)
    s_min_execution: f32,
    /// B 维变异分 s_min 硬门(≥ 0.5 锚点)
    s_min_mutation: f32,
    /// C 维 held-out s_min 硬门(≥ 0.5 锚点)
    s_min_held_out: f32,
    /// A/B/C 加权权重
    weights: EvidenceWeights,
    /// AHIRT 探测总数下限(≥ 100)
    n_probe_min: u32,
    /// AHIRT 每类探测数下限(≥ 25)
    per_category_min: u32,
    /// 平局带 σ̂_d(须实跑标定,None 时 ε_win 取下限 0.02)
    sigma_d: Option<f64>,
    /// 每批配对数 m(须实跑标定,与 σ̂_d 联用计算 ε_win)
    pairs_per_batch: Option<u32>,
}

impl ShadowModeConfig {
    /// 构造配置并执行全部治理校验(fail-closed)
    ///
    /// # 校验规则(违反任一 → [`MasError::ShadowGovernanceConfigInvalid`])
    /// - s_min 各维不低于治理签署锚点(只能收紧不能放松)且 ≤ 1.0
    /// - `n_probe_min ≥ 100`(固定下限)、`per_category_min ≥ 25`
    /// - 权重约束由 [`EvidenceWeights::new`] 前置保证
    /// - `sigma_d`(若提供)必须为有限正数;`pairs_per_batch`(若提供)≥ 1
    ///
    /// WHY #[allow(too_many_arguments)]:9 个参数均为独立的治理旋钮(s_min 三维/
    /// 权重/N_probe/每类下限/σ̂_d/m),各自有 fail-closed 校验语义,强行聚合成
    /// 参数结构反而障蔽校验职责;日常调用走 [`anchor_profile`](Self::anchor_profile)
    /// 单参入口,`new` 仅供全控制场景。与 repo-wiki/osa-coordinator 同类构造器先例一致。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        signoff: GovernanceSignoff,
        s_min_execution: f32,
        s_min_mutation: f32,
        s_min_held_out: f32,
        weights: EvidenceWeights,
        n_probe_min: u32,
        per_category_min: u32,
        sigma_d: Option<f64>,
        pairs_per_batch: Option<u32>,
    ) -> Result<Self> {
        for (name, value, anchor) in [
            ("A 执行面", s_min_execution, S_MIN_EXECUTION_ANCHOR),
            ("B 变异分", s_min_mutation, S_MIN_MUTATION_ANCHOR),
            ("C held-out", s_min_held_out, S_MIN_HELD_OUT_ANCHOR),
        ] {
            if value < anchor || value > 1.0 {
                return Err(MasError::ShadowGovernanceConfigInvalid {
                    reason: format!(
                        "s_min {name}={value} 越界:须 ∈ [{anchor}, 1.0](锚点为治理签署档,只能收紧)"
                    ),
                });
            }
        }
        if n_probe_min < N_PROBE_FLOOR {
            return Err(MasError::ShadowGovernanceConfigInvalid {
                reason: format!(
                    "n_probe_min={n_probe_min} 低于固定下限 {N_PROBE_FLOOR}(rev4 决策 3A″-P2)"
                ),
            });
        }
        if per_category_min < PER_CATEGORY_FLOOR {
            return Err(MasError::ShadowGovernanceConfigInvalid {
                reason: format!(
                    "per_category_min={per_category_min} 低于下限 {PER_CATEGORY_FLOOR}(rev3 决策 3A″-P)"
                ),
            });
        }
        if let Some(sd) = sigma_d {
            if !sd.is_finite() || sd <= 0.0 {
                return Err(MasError::ShadowGovernanceConfigInvalid {
                    reason: format!("sigma_d={sd} 非法:须为有限正数(实跑标定值)"),
                });
            }
        }
        if let Some(m) = pairs_per_batch {
            if m == 0 {
                return Err(MasError::ShadowGovernanceConfigInvalid {
                    reason: "pairs_per_batch 不可为 0".into(),
                });
            }
        }
        Ok(Self {
            signoff,
            s_min_execution,
            s_min_mutation,
            s_min_held_out,
            weights,
            n_probe_min,
            per_category_min,
            sigma_d,
            pairs_per_batch,
        })
    }

    /// 以 rev4 治理签署锚点档构造(s_min 取锚点值,权重均分归一)
    ///
    /// WHY 提供便捷构造:锚点档是 rev4 签署的初始档(A≥0.9/B≥0.5/C≥0.5),
    /// 权重均分 (0.34/0.33/0.33) 满足 w_min 与单维上限;σ̂_d/m 留 None
    /// (须实跑标定,ε_win 落回下限 0.02)。签署凭证仍强制传入,不豁免问责。
    pub fn anchor_profile(signoff: GovernanceSignoff) -> Result<Self> {
        Self::new(
            signoff,
            S_MIN_EXECUTION_ANCHOR,
            S_MIN_MUTATION_ANCHOR,
            S_MIN_HELD_OUT_ANCHOR,
            EvidenceWeights::new(0.34, 0.33, 0.33)?,
            N_PROBE_FLOOR,
            PER_CATEGORY_FLOOR,
            None,
            None,
        )
    }

    /// 平局带 ε_win = max(0.02, 2·σ̂_d/√m)(rev3 决策 3C-P)
    ///
    /// σ̂_d 或 m 缺失(未实跑标定)时落回下限 0.02——下限是预注册的
    /// 保守值,标定后只会更严不会更松(2σ̂_d/√m 取 max)。
    #[must_use]
    pub fn epsilon_win(&self) -> f64 {
        match (self.sigma_d, self.pairs_per_batch) {
            (Some(sd), Some(m)) => EPSILON_WIN_FLOOR.max(2.0 * sd / f64::from(m).sqrt()),
            _ => EPSILON_WIN_FLOOR,
        }
    }

    /// 治理签署凭证
    #[must_use]
    pub fn signoff(&self) -> &GovernanceSignoff {
        &self.signoff
    }

    /// A 维执行面 s_min 硬门
    #[must_use]
    pub fn s_min_execution(&self) -> f32 {
        self.s_min_execution
    }

    /// B 维变异分 s_min 硬门
    #[must_use]
    pub fn s_min_mutation(&self) -> f32 {
        self.s_min_mutation
    }

    /// C 维 held-out s_min 硬门
    #[must_use]
    pub fn s_min_held_out(&self) -> f32 {
        self.s_min_held_out
    }

    /// A/B/C 加权权重
    #[must_use]
    pub fn weights(&self) -> &EvidenceWeights {
        &self.weights
    }

    /// AHIRT 探测总数下限
    #[must_use]
    pub fn n_probe_min(&self) -> u32 {
        self.n_probe_min
    }

    /// AHIRT 每类探测数下限
    #[must_use]
    pub fn per_category_min(&self) -> u32 {
        self.per_category_min
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn signoff() -> GovernanceSignoff {
        GovernanceSignoff::new("user", "ADR-053-rev4", "2026-07-29").expect("合法签署")
    }

    /// 空签署字段拒绝构造(fail-closed 核心)
    #[test]
    fn test_empty_signoff_rejected() {
        assert!(GovernanceSignoff::new("", "ADR-053-rev4", "2026-07-29").is_err());
        assert!(GovernanceSignoff::new("user", "  ", "2026-07-29").is_err());
        assert!(GovernanceSignoff::new("user", "ADR-053-rev4", "").is_err());
    }

    /// 锚点档构造成功且 ε_win 落回下限
    #[test]
    fn test_anchor_profile_valid() {
        let config = ShadowModeConfig::anchor_profile(signoff()).expect("锚点档应合法");
        assert_eq!(config.s_min_execution(), S_MIN_EXECUTION_ANCHOR);
        assert_eq!(config.n_probe_min(), N_PROBE_FLOOR);
        assert!((config.epsilon_win() - EPSILON_WIN_FLOOR).abs() < f64::EPSILON);
    }

    /// s_min 低于锚点拒绝(只能收紧不能放松)
    #[test]
    fn test_s_min_below_anchor_rejected() {
        let weights = EvidenceWeights::new(0.34, 0.33, 0.33).expect("合法权重");
        let result = ShadowModeConfig::new(
            signoff(),
            0.8, // < 0.9 锚点
            0.5,
            0.5,
            weights,
            100,
            25,
            None,
            None,
        );
        assert!(matches!(
            result,
            Err(MasError::ShadowGovernanceConfigInvalid { .. })
        ));
    }

    /// N_probe 低于 100 固定下限拒绝
    #[test]
    fn test_n_probe_below_floor_rejected() {
        let weights = EvidenceWeights::new(0.34, 0.33, 0.33).expect("合法权重");
        let result = ShadowModeConfig::new(signoff(), 0.9, 0.5, 0.5, weights, 99, 25, None, None);
        assert!(result.is_err());
    }

    /// 权重越界与不归一拒绝
    #[test]
    fn test_weights_constraints() {
        assert!(
            EvidenceWeights::new(0.7, 0.2, 0.1).is_err(),
            "单维 >0.60 拒绝"
        );
        assert!(
            EvidenceWeights::new(0.1, 0.5, 0.4).is_err(),
            "单维 <0.20 拒绝"
        );
        assert!(EvidenceWeights::new(0.4, 0.4, 0.4).is_err(), "和 ≠1 拒绝");
        assert!(EvidenceWeights::new(0.4, 0.3, 0.3).is_ok());
    }

    /// σ̂_d 标定后 ε_win = max(下限, 2σ̂_d/√m)
    #[test]
    fn test_epsilon_win_calibrated() {
        let weights = EvidenceWeights::new(0.34, 0.33, 0.33).expect("合法权重");
        let config = ShadowModeConfig::new(
            signoff(),
            0.9,
            0.5,
            0.5,
            weights,
            100,
            25,
            Some(0.05),
            Some(25),
        )
        .expect("标定配置应合法");
        // 2×0.05/√25 = 0.02 = 下限 → 取 max 仍 0.02
        assert!((config.epsilon_win() - 0.02).abs() < 1e-12);

        let config2 = ShadowModeConfig::new(
            signoff(),
            0.9,
            0.5,
            0.5,
            weights,
            100,
            25,
            Some(0.2),
            Some(16),
        )
        .expect("标定配置应合法");
        // 2×0.2/4 = 0.1 > 0.02
        assert!((config2.epsilon_win() - 0.1).abs() < 1e-12);
    }

    /// 非法 σ̂_d 拒绝
    #[test]
    fn test_invalid_sigma_d_rejected() {
        let weights = EvidenceWeights::new(0.34, 0.33, 0.33).expect("合法权重");
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                ShadowModeConfig::new(signoff(), 0.9, 0.5, 0.5, weights, 100, 25, Some(bad), None)
                    .is_err(),
                "sigma_d={bad} 应被拒绝"
            );
        }
    }
}
