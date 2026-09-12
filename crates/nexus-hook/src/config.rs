//! Hook 配置 — TOML 挂载（P3-T3，v4.0 WI-24）
//!
//! 对应架构层: L9 Quest（nexus-hook，ADR-146）
//!
//! TOML 格式（挂载文件示例）:
//! ```toml
//! trust = "ask"   # trusted / ask / untrusted
//!
//! [[pre_tool_use]]
//! command = "git stash"
//! timeout_ms = 5000
//!
//! [[post_tool_use]]
//! command = "notify-send done"
//! ```
//!
//! 环境变量注入（执行时）: `$TOOL_NAME` / `$SESSION_ID` / `$GOAL_ID` 由执行器注入。

use std::collections::HashMap;

use crate::lifecycle::LifecycleEvent;
use serde::Deserialize;

/// 项目信任级别 — 安全门第一道（WI-24:信任提示）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustLevel {
    /// 信任项目:全部 hook 可执行
    Trusted,
    /// 询问:hook 执行前需确认（默认;未确认 = 不执行）
    #[default]
    Ask,
    /// 不信任:hook 一律不执行（fail-closed）
    Untrusted,
}

impl TrustLevel {
    /// 解析 TOML 字符串（未知值 fail-closed → Untrusted）
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "trusted" => Self::Trusted,
            "ask" => Self::Ask,
            _ => Self::Untrusted,
        }
    }

    /// 是否允许执行（Untrusted 一律拒绝）
    #[must_use]
    pub const fn allows_execution(self) -> bool {
        matches!(self, Self::Trusted | Self::Ask)
    }
}

/// 单条 hook 规格
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HookSpec {
    /// shell 命令（可含占位符;执行器按 token 拆分 program/args）
    pub command: String,
    /// 超时（ms,默认 5000;超时熔断不阻主流程）
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

/// 默认超时 5s
const fn default_timeout_ms() -> u64 {
    5_000
}

/// TOML 挂载文件（serde 直解析）
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookFile {
    /// 信任级别（trusted/ask/untrusted）
    #[serde(default)]
    pub trust: Option<String>,
    /// session_start 挂载
    #[serde(default)]
    pub session_start: Vec<HookSpec>,
    /// session_end 挂载
    #[serde(default)]
    pub session_end: Vec<HookSpec>,
    /// quest_start 挂载
    #[serde(default)]
    pub quest_start: Vec<HookSpec>,
    /// quest_end 挂载
    #[serde(default)]
    pub quest_end: Vec<HookSpec>,
    /// pre_quest_turn 挂载
    #[serde(default)]
    pub pre_quest_turn: Vec<HookSpec>,
    /// post_quest_turn 挂载
    #[serde(default)]
    pub post_quest_turn: Vec<HookSpec>,
    /// pre_tool_use 挂载
    #[serde(default)]
    pub pre_tool_use: Vec<HookSpec>,
    /// post_tool_use 挂载
    #[serde(default)]
    pub post_tool_use: Vec<HookSpec>,
    /// pre_request 挂载
    #[serde(default)]
    pub pre_request: Vec<HookSpec>,
    /// post_request 挂载
    #[serde(default)]
    pub post_request: Vec<HookSpec>,
    /// pre_command 挂载
    #[serde(default)]
    pub pre_command: Vec<HookSpec>,
    /// post_command 挂载
    #[serde(default)]
    pub post_command: Vec<HookSpec>,
    /// error 挂载
    #[serde(default)]
    pub error: Vec<HookSpec>,
    /// stop 挂载
    #[serde(default)]
    pub stop: Vec<HookSpec>,
}

impl HookFile {
    /// 事件 → 挂载规格查询（未挂载返回空）
    #[must_use]
    pub fn specs_for(&self, event: LifecycleEvent) -> &[HookSpec] {
        match event {
            LifecycleEvent::SessionStart => &self.session_start,
            LifecycleEvent::SessionEnd => &self.session_end,
            LifecycleEvent::QuestStart => &self.quest_start,
            LifecycleEvent::QuestEnd => &self.quest_end,
            LifecycleEvent::PreQuestTurn => &self.pre_quest_turn,
            LifecycleEvent::PostQuestTurn => &self.post_quest_turn,
            LifecycleEvent::PreToolUse => &self.pre_tool_use,
            LifecycleEvent::PostToolUse => &self.post_tool_use,
            LifecycleEvent::PreRequest => &self.pre_request,
            LifecycleEvent::PostRequest => &self.post_request,
            LifecycleEvent::PreCommand => &self.pre_command,
            LifecycleEvent::PostCommand => &self.post_command,
            LifecycleEvent::Error => &self.error,
            LifecycleEvent::Stop => &self.stop,
        }
    }

    /// 已挂载事件计数（诊断）
    #[must_use]
    pub fn mounted_count(&self) -> usize {
        LifecycleEvent::ALL
            .iter()
            .map(|e| self.specs_for(*e).len())
            .sum()
    }
}

/// Hook 配置 — 挂载文件 + 信任级别的运行时形态
#[derive(Debug, Clone, Default)]
pub struct HookConfig {
    /// 信任级别
    pub trust: TrustLevel,
    /// 事件 → 挂载规格
    pub hooks: HashMap<LifecycleEvent, Vec<HookSpec>>,
}

impl HookConfig {
    /// 从 TOML 文本构建（解析失败返回 None,调用方按空配置 fail-closed）
    #[must_use]
    pub fn from_toml(text: &str) -> Option<Self> {
        let file: HookFile = toml::from_str(text).ok()?;
        let trust = file
            .trust
            .as_deref()
            .map(TrustLevel::parse)
            .unwrap_or_default();
        let mut hooks = HashMap::new();
        for event in LifecycleEvent::ALL {
            let specs = file.specs_for(event);
            if !specs.is_empty() {
                hooks.insert(event, specs.to_vec());
            }
        }
        Some(Self { trust, hooks })
    }

    /// 事件挂载数（诊断）
    #[must_use]
    pub fn specs_for(&self, event: LifecycleEvent) -> &[HookSpec] {
        self.hooks.get(&event).map(Vec::as_slice).unwrap_or(&[])
    }

    /// 空配置判定（空载 = 现状,回退路径）
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TrustLevel 解析 — 合法值映射 + 未知 fail-closed
    #[test]
    fn trust_level_parse() {
        assert_eq!(TrustLevel::parse("trusted"), TrustLevel::Trusted);
        assert_eq!(TrustLevel::parse("ask"), TrustLevel::Ask);
        assert_eq!(TrustLevel::parse("untrusted"), TrustLevel::Untrusted);
        assert_eq!(
            TrustLevel::parse("garbage"),
            TrustLevel::Untrusted,
            "未知必须 fail-closed"
        );
        assert_eq!(TrustLevel::parse(""), TrustLevel::Untrusted);
    }

    /// TOML 解析 — 多事件挂载 + 信任级别
    #[test]
    fn toml_parse_full() {
        let text = r#"
trust = "ask"

[[pre_tool_use]]
command = "git stash"
timeout_ms = 3000

[[pre_tool_use]]
command = "echo pre"

[[post_tool_use]]
command = "notify-send done"
"#;
        let cfg = HookConfig::from_toml(text).expect("TOML 必须可解析");
        assert_eq!(cfg.trust, TrustLevel::Ask);
        assert_eq!(cfg.specs_for(LifecycleEvent::PreToolUse).len(), 2);
        assert_eq!(cfg.specs_for(LifecycleEvent::PostToolUse).len(), 1);
        // 默认超时 5s
        assert_eq!(
            cfg.specs_for(LifecycleEvent::PreToolUse)[1].timeout_ms,
            5_000
        );
        // 未挂载事件为空
        assert!(cfg.specs_for(LifecycleEvent::SessionStart).is_empty());
        assert!(!cfg.is_empty());
    }

    /// 非法 TOML — 返回 None（fail-closed 空配置）
    #[test]
    fn invalid_toml_none() {
        assert!(HookConfig::from_toml("not [valid").is_none());
    }

    /// 空 TOML — 空配置（无 hooks,trust 默认 Ask）
    #[test]
    fn empty_toml_empty_config() {
        let cfg = HookConfig::from_toml("").expect("空文本应解析为默认");
        assert!(cfg.is_empty());
        assert_eq!(cfg.trust, TrustLevel::Ask);
    }

    /// 14 节全部可挂载 — ALL 遍历 specs_for 不 panic
    #[test]
    fn all_sections_mountable() {
        let file = HookFile::default();
        for e in LifecycleEvent::ALL {
            assert!(file.specs_for(e).is_empty());
        }
    }

    /// mounted_count — 挂载计数正确
    #[test]
    fn mounted_count_ok() {
        let text = r#"
[[error]]
command = "echo err"
[[stop]]
command = "echo stop"
"#;
        let cfg = HookConfig::from_toml(text).expect("解析成功");
        assert_eq!(cfg.specs_for(LifecycleEvent::Error).len(), 1);
        assert_eq!(cfg.specs_for(LifecycleEvent::Stop).len(), 1);
    }
}
