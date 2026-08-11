//! ApprovalMode 审批模式三态状态机(Concord W4 · T4.1,ADR-074)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! 方案文档 P3:CapabilityToken 四态存在于能力层,但 TUI 无一键模式切换,
//! 用户无法在会话中翻转"只读规划 ↔ 自动执行"。对标五家主流 Agent CLI
//! (Claude Code Shift+Tab 三态、Codex /mode、yottacode banner 等)引入
//! 交互层审批模式:
//!
//! - `Normal` — 执行前确认(默认态)
//! - `Plan`   — 只读规划:orchestrated/agent 层命令被交互层诚实拦截,
//!   仅 instant 层可执行(执行层门控归 L4/L9 能力层,本层仅表达)
//! - `Auto`   — 自动执行档;Critical 9 类 mpsc 旁路事件**不豁免**
//!   (语义声明见 ADR-074,TUI 不越层伪造门控)
//!
//! # 红线合规
//! ADR-034:ApprovalMode 是会话运行策略字段(TuiState),非编译期/运行时
//! feature flag;与 RouterMode(输入语法状态机)、ViewMode(视图状态机)正交。

use serde::{Deserialize, Serialize};

/// 审批模式三态(Normal/Plan/Auto)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ApprovalMode {
    /// 执行前确认(默认态)
    #[default]
    Normal,
    /// 只读规划:orchestrated/agent 命令交互层拦截
    Plan,
    /// 自动执行档(Critical 9 类事件不豁免,见 ADR-074)
    Auto,
}

impl ApprovalMode {
    /// 正向循环:Normal → Plan → Auto → Normal(Shift+Tab 语义)
    pub fn cycle(self) -> Self {
        match self {
            Self::Normal => Self::Plan,
            Self::Plan => Self::Auto,
            Self::Auto => Self::Normal,
        }
    }

    /// 标签 i18n 键(statusline 徽标文本)
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Normal => "approval.mode.normal",
            Self::Plan => "approval.mode.plan",
            Self::Auto => "approval.mode.auto",
        }
    }

    /// 是否拦截 orchestrated/agent 层命令(仅 Plan 态)
    pub fn blocks_execution_tiers(self) -> bool {
        matches!(self, Self::Plan)
    }

    /// 能力策略标签(表达层映射,不触达能力层;ADR-074 映射表)
    ///
    /// WHY 纯表达:依赖铁律禁止 L10 直接操作 L4 能力机制;本函数仅提供
    /// 面向操作员的策略说明文本键,真实门控由能力层执行。
    pub fn capability_policy_label(self) -> &'static str {
        match self {
            Self::Normal => "approval.policy.normal",
            Self::Plan => "approval.policy.plan",
            Self::Auto => "approval.policy.auto",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_order_normal_plan_auto() {
        assert_eq!(ApprovalMode::Normal.cycle(), ApprovalMode::Plan);
        assert_eq!(ApprovalMode::Plan.cycle(), ApprovalMode::Auto);
        assert_eq!(ApprovalMode::Auto.cycle(), ApprovalMode::Normal);
    }

    #[test]
    fn cycle_three_steps_returns_to_start() {
        let mut m = ApprovalMode::default();
        for _ in 0..3 {
            m = m.cycle();
        }
        assert_eq!(m, ApprovalMode::Normal, "三态循环三步回到起点");
    }

    #[test]
    fn default_is_normal() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::Normal);
    }

    #[test]
    fn label_keys_distinct_and_stable() {
        let keys = [
            ApprovalMode::Normal.label_key(),
            ApprovalMode::Plan.label_key(),
            ApprovalMode::Auto.label_key(),
        ];
        // 三键互不相同(避免徽标文案串台)
        assert_ne!(keys[0], keys[1]);
        assert_ne!(keys[1], keys[2]);
        assert_ne!(keys[0], keys[2]);
    }

    #[test]
    fn only_plan_blocks_execution_tiers() {
        assert!(!ApprovalMode::Normal.blocks_execution_tiers());
        assert!(ApprovalMode::Plan.blocks_execution_tiers());
        assert!(!ApprovalMode::Auto.blocks_execution_tiers());
    }

    #[test]
    fn capability_policy_labels_distinct() {
        let keys = [
            ApprovalMode::Normal.capability_policy_label(),
            ApprovalMode::Plan.capability_policy_label(),
            ApprovalMode::Auto.capability_policy_label(),
        ];
        assert_ne!(keys[0], keys[1]);
        assert_ne!(keys[1], keys[2]);
        assert_ne!(keys[0], keys[2]);
    }

    #[test]
    fn serde_roundtrip_all_variants() {
        for m in [ApprovalMode::Normal, ApprovalMode::Plan, ApprovalMode::Auto] {
            let yaml = serde_yaml::to_string(&m).expect("serialize");
            let back: ApprovalMode = serde_yaml::from_str(&yaml).expect("deserialize");
            assert_eq!(back, m);
        }
    }

    #[test]
    fn serde_missing_field_defaults_normal() {
        // 旧状态文件无 approval_mode 字段时的兼容默认
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default)]
            approval_mode: ApprovalMode,
        }
        let h: Holder = serde_yaml::from_str("{}").expect("empty yaml");
        assert_eq!(h.approval_mode, ApprovalMode::Normal);
    }
}
