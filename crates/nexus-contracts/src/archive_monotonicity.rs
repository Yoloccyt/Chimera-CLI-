//! 归档单调性契约 — INV-8 独立公共 API(P0-2 修复)
//!
//! 对应架构层: **L0 Contracts**(nexus-contracts)
//! 对应审计项: **P0-2 — INV-8 归档单调性独立公共 API**
//!
//! # 背景(WHY 本模块存在)
//!
//! INV-8(归档单调性:Hot→Warm→Cold→Ice 单向降级,禁止回升)的属性测试
//! 此前仅在 L9 `chimera-mas`(`InvariantChecker::check_inv8_archive_monotonicity`)
//! 实现。但 `mlc-engine`(L2,四级神经形态)与 `cmt-tiering`(L3,热温冰冷)
//! 自身未导出归档单调性公共 API——任何第三方直接使用 `mlc-engine::demote` /
//! `cmt-tiering::TierMigrator` 时,可绕过 `chimera_mas::InvariantChecker`,
//! 导致 INV-8 失效。
//!
//! 本模块将 INV-8 判定下沉至 L0,作为**独立公共 API**:
//! - L0 是 mlc-engine(L2) 与 cmt-tiering(L3) 唯一可共同依赖的契约层
//!   (依赖铁律 §2.2:`L(N) → L(0)` 恒允许),调用方无需引入 L9 chimera-mas
//! - 判定为纯函数,无状态、无副作用,可在任意归档/迁移入口直接执行
//!
//! # ADR-033 例外声明(与 `test_scale` 同类)
//!
//! nexus-contracts 的 ADR-033 约束为"纯类型 + 零逻辑"。本模块是继
//! `test_scale` 之后的第二个明确例外:承载 INV-8 判定逻辑(单函数,
//! 无业务依赖),因为归档单调性必须由 L2/L3 的公共入口**独立执行**,
//! 无法下沉为纯类型定义。
//!
//! # 语义(与 chimera-mas 内部实现的差异,务必注意)
//!
//! - **合法**: `to.level() >= from.level()`(降级归档 + 同层保持)
//! - **非法**: `to.level() < from.level()`(回升 / 逆向膨胀)
//!
//! ⚠️ 与 `chimera_mas::InvariantChecker::check_inv8_archive_monotonicity` 的差异:
//! chimera-mas 将同层操作(`Hot→Hot`)也判定为非法(要求严格升 level);
//! 本 L0 API 按审计修复要求将同层视为合法——归档到自身层级为无操作(no-op),
//! 不构成"回升"。两处为**独立实现**(语义镜像约定):调用方按场景选择,
//! 修改任一实现时须同步评审另一处,防止语义漂移。
//!
//! # 层级映射(供 L2/L3 接线参考)
//!
//! | L0 ArchiveTier | cmt-tiering::Tier | mlc-engine::MemoryTier |
//! |----------------|-------------------|------------------------|
//! | Hot(0)         | Hot               | L0Working              |
//! | Warm(1)        | Warm              | L1Episodic             |
//! | Cold(2)        | Cold              | L2Semantic             |
//! | Ice(3)         | Ice               | L3Procedural           |

use std::fmt;

use serde::{Deserialize, Serialize};

/// 归档层级 — 四级归档的层级标识
///
/// 顺序严格单调递增:Hot(0) < Warm(1) < Cold(2) < Ice(3)。
/// 与 `cmt-tiering::Tier`(L3)/`mlc-engine::MemoryTier`(L2) 语义等价,
/// 但定义在 L0,避免 L0 → L3/L2 向上依赖(依赖铁律 §2.2)。
///
/// # serde derive 说明（2026-08-16 审计修复 A1）
///
/// 归档层级可随归档事件/检查点序列化传输（如 EventBus 归档事件载荷、
/// LHQP 检查点持久化），故派生 `Serialize`/`Deserialize`，与 crate 内
/// 其他 30+ 公开枚举的 serde 惯例保持一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveTier {
    /// 热层 — 工作/高频访问(等价 cmt Hot、mlc L0Working)
    Hot,
    /// 温层 — 情节/中频访问(等价 cmt Warm、mlc L1Episodic)
    Warm,
    /// 冷层 — 语义/低频访问(等价 cmt Cold、mlc L2Semantic)
    Cold,
    /// 冰层 — 归档/持久化只读(等价 cmt Ice、mlc L3Procedural)
    Ice,
}

impl ArchiveTier {
    /// 返回层级数值(0=Hot, 1=Warm, 2=Cold, 3=Ice)
    ///
    /// 数值严格递增,INV-8 单调性判定依据:
    /// - 合法: `to.level() >= from.level()`(降级或同层保持)
    /// - 非法: `to.level() < from.level()`(回升)
    ///
    /// ## 示例
    ///
    /// ```
    /// use nexus_contracts::ArchiveTier;
    /// assert_eq!(ArchiveTier::Hot.level(), 0);
    /// assert_eq!(ArchiveTier::Ice.level(), 3);
    /// ```
    pub const fn level(self) -> u8 {
        match self {
            Self::Hot => 0,
            Self::Warm => 1,
            Self::Cold => 2,
            Self::Ice => 3,
        }
    }
}

/// 不变量违反 — INV-8 判定失败的错误载体
///
/// 携带人类可读的 `msg` 字段,调用方可直接记录日志、向上传播或展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantViolation {
    /// 违反描述(含源层级与目标层级名称)
    pub msg: String,
}

impl InvariantViolation {
    /// 构造不变量违反错误
    ///
    /// ## 示例
    ///
    /// ```
    /// use nexus_contracts::InvariantViolation;
    /// let v = InvariantViolation::new("归档层级回升被禁止(INV-8)");
    /// assert_eq!(v.msg, "归档层级回升被禁止(INV-8)");
    /// ```
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

impl fmt::Display for InvariantViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for InvariantViolation {}

/// INV-8 — 归档单调性断言(独立公共 API)
///
/// 验证归档操作沿 Hot→Warm→Cold→Ice 单向降级(禁止回升 / 逆向膨胀):
/// - `Ok(())`: 合法降级(`to.level() > from.level()`)或同层保持
///   (`to.level() == from.level()`,归档到自身层级为无操作)
/// - `Err(InvariantViolation)`: 回升方向(`to.level() < from.level()`),拒绝
///
/// ## 边界场景
///
/// - `Hot → Warm`: Ok(level 0→1)
/// - `Hot → Ice`: Ok(跨级降级,level 0→3)
/// - `Hot → Hot`: Ok(同层保持)
/// - `Ice → Hot`: Err(回升,level 3→0)
///
/// ## 示例
///
/// ```
/// use nexus_contracts::{ArchiveTier, assert_archive_monotonicity};
///
/// assert!(assert_archive_monotonicity(ArchiveTier::Hot, ArchiveTier::Ice).is_ok());
/// assert!(assert_archive_monotonicity(ArchiveTier::Ice, ArchiveTier::Hot).is_err());
/// ```
pub fn assert_archive_monotonicity(
    from: ArchiveTier,
    to: ArchiveTier,
) -> Result<(), InvariantViolation> {
    // 回升方向(to.level() < from.level())即违反 INV-8,拒绝
    if to.level() < from.level() {
        return Err(InvariantViolation::new(format!(
            "归档层级回升被禁止(INV-8): {from:?} -> {to:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全部层级清单 — 供穷举测试复用,避免遗漏新增变体
    const ALL_TIERS: [ArchiveTier; 4] = [
        ArchiveTier::Hot,
        ArchiveTier::Warm,
        ArchiveTier::Cold,
        ArchiveTier::Ice,
    ];

    /// level() 严格递增(0=Hot < 1=Warm < 2=Cold < 3=Ice)
    #[test]
    fn test_archive_tier_level_strictly_increasing() {
        assert_eq!(ArchiveTier::Hot.level(), 0);
        assert_eq!(ArchiveTier::Warm.level(), 1);
        assert_eq!(ArchiveTier::Cold.level(), 2);
        assert_eq!(ArchiveTier::Ice.level(), 3);
        assert!(ArchiveTier::Hot.level() < ArchiveTier::Warm.level());
        assert!(ArchiveTier::Warm.level() < ArchiveTier::Cold.level());
        assert!(ArchiveTier::Cold.level() < ArchiveTier::Ice.level());
    }

    /// 穷举 16 对(4×4):降级方向(6 对)+ 同层保持(4 对)全部 Ok
    ///
    /// WHY 穷举:4 层空间可枚举,穷举比抽样更严格,构成 proptest 的确定性真值源。
    #[test]
    fn test_exhaustive_monotonic_pairs_all_ok() {
        for from in ALL_TIERS {
            for to in ALL_TIERS {
                if to.level() < from.level() {
                    continue; // 回升对由 test_exhaustive_reverse_pairs_all_rejected 覆盖
                }
                let result = assert_archive_monotonicity(from, to);
                assert!(
                    result.is_ok(),
                    "{from:?} → {to:?} 为降级或同层,应返回 Ok,实际: {result:?}"
                );
            }
        }
    }

    /// 穷举回升对(6 对:4 同层以外反向组合):全部 Err,且 msg 含两级名称
    #[test]
    fn test_exhaustive_reverse_pairs_all_rejected() {
        for from in ALL_TIERS {
            for to in ALL_TIERS {
                if to.level() >= from.level() {
                    continue; // 合法对由 test_exhaustive_monotonic_pairs_all_ok 覆盖
                }
                let result = assert_archive_monotonicity(from, to);
                match result {
                    Err(v) => {
                        let expected_from = format!("{from:?}");
                        let expected_to = format!("{to:?}");
                        assert!(
                            v.msg.contains(&expected_from),
                            "错误消息应包含源层级 {expected_from},实际: {}",
                            v.msg
                        );
                        assert!(
                            v.msg.contains(&expected_to),
                            "错误消息应包含目标层级 {expected_to},实际: {}",
                            v.msg
                        );
                    }
                    Ok(()) => panic!("{from:?} → {to:?} 为回升方向,应返回 Err"),
                }
            }
        }
    }

    /// InvariantViolation 的 Display / Error 语义
    #[test]
    fn test_invariant_violation_display_and_error() {
        let v = InvariantViolation::new("归档层级回升被禁止(INV-8): Ice -> Hot");
        assert_eq!(v.to_string(), "归档层级回升被禁止(INV-8): Ice -> Hot");
        // std::error::Error trait 对象可上抛(库层错误协议)
        let boxed: Box<dyn std::error::Error> = Box::new(v.clone());
        assert_eq!(boxed.to_string(), v.to_string());
    }

    // ============================================================
    // proptest 属性(全层级空间的不变量)
    // ============================================================
    //
    // 遵循项目 proptest 纪律(§4.1 + 项目 memory 实证):
    // - block-named 语法: fn test_name(x in 0..100u32) { ... }
    // - 显式传参:格式串不使用 {var} 内联捕获,全部显式位置参数
    //   (proptest prop_assert_eq! 不支持变量捕获)
    //
    // WHY 用普通注释而非 doc comment:proptest! 宏为 #[test] fn 生成包装,
    // 宏外部的 doc comment 无法附着到生成项,会触发 unused_doc_comments 警告
    // (与 budget_tier.rs 同款处理)。

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        /// 不变量 1: from.level() <= to.level() 时必返回 Ok(降级 + 同层保持)
        #[test]
        fn prop_monotonic_or_same_always_ok(
            from_idx in 0u8..=3,
            delta in 0u8..=3
        ) {
            // to_idx = from_idx + delta,保证 to.level() >= from.level()
            let to_idx = from_idx + delta;
            proptest::prop_assume!(to_idx <= 3, "to_idx 不超过 Ice(3)");
            let from = tier_from_idx(from_idx);
            let to = tier_from_idx(to_idx);
            let result = assert_archive_monotonicity(from, to);
            proptest::prop_assert!(
                result.is_ok(),
                "{} -> {} (level {} -> {}) 为降级或同层,应通过,实际: {:?}",
                tier_name(from),
                tier_name(to),
                from.level(),
                to.level(),
                result
            );
        }

        /// 不变量 2: from.level() > to.level() 时必返回 Err(回升被拒绝)
        #[test]
        fn prop_reverse_promotion_always_rejected(
            from_idx in 1u8..=3,
            to_idx in 0u8..=2
        ) {
            // prop_assume 确保 from.level() > to.level()(回升对)
            proptest::prop_assume!(to_idx < from_idx, "本测试仅验证回升方向");
            let from = tier_from_idx(from_idx);
            let to = tier_from_idx(to_idx);
            let result = assert_archive_monotonicity(from, to);
            match result {
                Err(v) => {
                    let expected_from = tier_name(from);
                    let expected_to = tier_name(to);
                    proptest::prop_assert_eq!(
                        v.msg.contains(expected_from),
                        true,
                        "错误消息应包含源层级 {},实际: {}",
                        expected_from,
                        v.msg
                    );
                    proptest::prop_assert_eq!(
                        v.msg.contains(expected_to),
                        true,
                        "错误消息应包含目标层级 {},实际: {}",
                        expected_to,
                        v.msg
                    );
                }
                Ok(()) => proptest::prop_assert!(
                    false,
                    "{} -> {} 为回升方向,应返回 Err",
                    tier_name(from),
                    tier_name(to)
                ),
            }
        }
    }

    /// 索引到 ArchiveTier 的辅助函数(0=Hot, 1=Warm, 2=Cold, 3=Ice)
    fn tier_from_idx(idx: u8) -> ArchiveTier {
        match idx {
            0 => ArchiveTier::Hot,
            1 => ArchiveTier::Warm,
            2 => ArchiveTier::Cold,
            _ => ArchiveTier::Ice,
        }
    }

    /// ArchiveTier 的 Debug 名称 — 供 proptest 消息显式传参使用
    ///
    /// WHY:proptest 断言格式串禁用 {var} 内联捕获(项目纪律),
    /// 用 &'static str 显式传入层级名,避免 `format!("{:?}", tier)` 引入
    /// 额外的分配与闭包捕获。
    fn tier_name(tier: ArchiveTier) -> &'static str {
        match tier {
            ArchiveTier::Hot => "Hot",
            ArchiveTier::Warm => "Warm",
            ArchiveTier::Cold => "Cold",
            ArchiveTier::Ice => "Ice",
        }
    }
}
