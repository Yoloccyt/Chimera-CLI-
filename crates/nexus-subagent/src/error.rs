//! SubAgent 错误类型 — 库层 thiserror enum（§4.1 规范）
//!
//! # 背景（架构方向 F-c，2026-09-12）
//!
//! 此前 `nexus-subagent` 是 L7 唯一的"错误洼地":任务体与
//! [`SubAgentRuntime::join_next`](crate::runtime::SubAgentRuntime::join_next)
//! 均以 `Result<String, String>` 传递失败——错误被**拍平成文案**,调用方只能打印,
//! 无法区分"预期内取消" / "实现缺陷(panic)" / "业务执行失败"三类语义,
//! 因而无法给出不同的降级策略(取消=忽略、panic=告警、业务失败=重试/上报)。
//!
//! 本模块把三类来源显式化为枚举变体,使调用方可 `match` 判定。
//! 这是 v4.0 WI-25 / ADR-148 遗留 TODO 的落地（见本 crate Cargo.toml 注释）:
//! 「错误类型与事件面实现时再声明 thiserror」——错误类型部分已落地,
//! 事件面仍按原计划以 `optional = true` + feature 门控接入(GATED 轨道)。
//!
//! # 三类来源
//! | 变体 | 来源 | 调用方建议处置 |
//! |---|---|---|
//! | [`SubAgentError::Cancelled`] | 任务执行前已被取消(四因取消之一) | 预期内终止,降级/忽略 |
//! | [`SubAgentError::Panicked`] | 任务体 panic(`spawn_blocking` 返回 JoinError) | 实现缺陷,告警 + 不重试 |
//! | [`SubAgentError::Execution`] | 任务体返回业务失败 | 业务错误,按类型重试或上报 |

use thiserror::Error;

/// SubAgent 任务失败 — 按**语义分类**而非拍平文案
///
/// WHY 枚举而非 `String`:调用方需要区分"取消"(预期内)与"panic"(实现缺陷),
/// 两者的降级策略完全不同;字符串无法承载这一判定(只能靠子串匹配,脆弱且易漂移)。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SubAgentError {
    /// 任务在执行前已被取消(四因:用户取消 / 超时 / 配额耗尽 / 父级撤销)
    #[error("subagent cancelled: {reason}")]
    Cancelled {
        /// 取消原因(取自 [`CancelReason`](crate::cancel::CancelReason) 的稳定文案)
        reason: String,
    },

    /// 任务体 panic — `spawn_blocking` 的 `JoinError` 转换而来
    ///
    /// 属**实现缺陷**,不应静默重试(重试通常再次 panic)。
    #[error("subagent panicked: {detail}")]
    Panicked {
        /// panic 详情(`JoinError` 的 Display)
        detail: String,
    },

    /// 任务体返回的业务失败(接入方主动 `Err`)
    #[error("subagent failed: {detail}")]
    Execution {
        /// 业务失败详情(接入方提供)
        detail: String,
    },
}

/// 从字符串文案构造(迁移便利)
///
/// 映射为 [`SubAgentError::Execution`]——字符串错误无分类信息,按"业务失败"
/// 处理是最保守的语义(不是取消、不是 panic)。
///
/// WHY 提供本转换:既有接入方以 `Err(format!(...))` 返回失败;有了 `From<String>`,
/// 其 `Err("...".into())` / `Err(format!(...))` 可零改动继续编译(无感升级)。
/// 新代码仍应显式构造变体以获得精确分类。
impl From<String> for SubAgentError {
    fn from(detail: String) -> Self {
        Self::Execution { detail }
    }
}

impl SubAgentError {
    /// 是否为预期内的取消(非故障)
    ///
    /// # 返回
    /// `true` = 取消导致,调用方通常可静默降级;`false` = 真实失败。
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled { .. })
    }

    /// 是否为实现缺陷(panic)
    ///
    /// # 返回
    /// `true` = 任务体 panic,建议告警而非重试。
    #[must_use]
    pub fn is_panic(&self) -> bool {
        matches!(self, Self::Panicked { .. })
    }

    /// 是否为业务执行失败
    ///
    /// # 返回
    /// `true` = 任务体主动返回的错误,可按业务语义重试或上报。
    #[must_use]
    pub fn is_execution(&self) -> bool {
        matches!(self, Self::Execution { .. })
    }

    /// 兼容视图:还原为旧的 `Result<String, String>` 错误文案
    ///
    /// WHY 存在:旧 [`join_next`](crate::runtime::SubAgentRuntime::join_next)
    /// 签名以 `String` 暴露错误;本方法为兼容壳提供**与分类一致**的文案,
    /// 避免兼容路径与新类型两条文案分叉(单一事实源 = 本枚举的 `Display`)。
    ///
    /// # 返回
    /// 与 `Display` 相同的错误文案。
    #[must_use]
    pub fn as_message(&self) -> String {
        self.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_carries_detail() {
        let e = SubAgentError::Cancelled {
            reason: "user_abort".into(),
        };
        assert_eq!(e.to_string(), "subagent cancelled: user_abort");
        let p = SubAgentError::Panicked {
            detail: "boom".into(),
        };
        assert_eq!(p.to_string(), "subagent panicked: boom");
        let x = SubAgentError::Execution {
            detail: "tool missing".into(),
        };
        assert_eq!(x.to_string(), "subagent failed: tool missing");
    }

    #[test]
    fn classification_is_mutually_exclusive() {
        let variants = [
            SubAgentError::Cancelled {
                reason: "r".into(),
            },
            SubAgentError::Panicked {
                detail: "d".into(),
            },
            SubAgentError::Execution {
                detail: "d".into(),
            },
        ];
        for v in &variants {
            // 每个变体恰命中一个分类谓词
            let hits =
                v.is_cancelled() as u8 + v.is_panic() as u8 + v.is_execution() as u8;
            assert_eq!(hits, 1, "变体 {v} 应恰归属一个分类");
        }
    }

    #[test]
    fn as_message_matches_display() {
        let e = SubAgentError::Execution {
            detail: "x".into(),
        };
        assert_eq!(e.as_message(), e.to_string());
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// 属性:任意详情文案都完整出现在 Display 中,且分类谓词与变体一致
        /// (防止后续新增变体时漏接分类谓词或文案模板)。
        #[test]
        fn prop_detail_preserved_and_classified(detail in ".{0,120}") {
            let c = SubAgentError::Cancelled { reason: detail.clone() };
            proptest::prop_assert!(c.to_string().contains(&detail));
            proptest::prop_assert!(c.is_cancelled() && !c.is_panic() && !c.is_execution());

            let p = SubAgentError::Panicked { detail: detail.clone() };
            proptest::prop_assert!(p.to_string().contains(&detail));
            proptest::prop_assert!(p.is_panic() && !p.is_cancelled() && !p.is_execution());

            let x = SubAgentError::Execution { detail: detail.clone() };
            proptest::prop_assert!(x.to_string().contains(&detail));
            proptest::prop_assert!(x.is_execution() && !x.is_cancelled() && !x.is_panic());
        }
    }
}
