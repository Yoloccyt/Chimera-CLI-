//! SubAgent 类型 — 3 类型 + 规格 + 规模上限（P3-T9，v4.0 WI-25）
//!
//! 对应架构层: L7 Execution（nexus-subagent，ADR-148）

use serde::{Deserialize, Serialize};

/// Swarm 规模上限 — 8（ADR-148 门禁;远低于 K3 的 300,Rust 单进程求稳）
pub const SWARM_LIMIT: usize = 8;

/// SubAgent 类型 — 同一执行引擎换参数（WI-25:类型化）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentKind {
    /// 编码（模型/工具集/写权限 + worktree）
    Coder,
    /// 探索（只读检索/调研）
    Explore,
    /// 规划（计划生成,不执行）
    Plan,
}

impl SubAgentKind {
    /// 全部类型（诊断/注册遍历）
    pub const ALL: [SubAgentKind; 3] = [
        SubAgentKind::Coder,
        SubAgentKind::Explore,
        SubAgentKind::Plan,
    ];

    /// 能力标签（Auction 匹配依据）
    #[must_use]
    pub const fn capability_tag(self) -> &'static str {
        match self {
            SubAgentKind::Coder => "code",
            SubAgentKind::Explore => "explore",
            SubAgentKind::Plan => "plan",
        }
    }
}

/// SubAgent 规格 — 同引擎换参数的参数集（WI-25）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentSpec {
    /// 类型
    pub kind: SubAgentKind,
    /// 模型标识（空 = 默认旗舰）
    #[serde(default)]
    pub model: Option<String>,
    /// 工具集白名单（空 = 类型默认集）
    #[serde(default)]
    pub tools: Vec<String>,
    /// 权限上下文（execpolicy 六模式;空 = 默认 Default）
    #[serde(default)]
    pub permission: Option<String>,
    /// worktree 路径（coder 专用;空 = 主工作区）
    #[serde(default)]
    pub worktree: Option<String>,
}

impl SubAgentSpec {
    /// 新建规格（默认参数）
    #[must_use]
    pub fn new(kind: SubAgentKind) -> Self {
        Self {
            kind,
            model: None,
            tools: Vec::new(),
            permission: None,
            worktree: None,
        }
    }
}

/// SubAgent 档案 — 注册表条目（Auction 报价依据）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentProfile {
    /// 档案 ID（注册键）
    pub profile_id: String,
    /// 类型
    pub kind: SubAgentKind,
    /// 能力标签（可多标签,逗号分隔）
    #[serde(default)]
    pub capabilities: String,
    /// 单位成本（报价基数;低 = 更便宜）
    pub unit_cost: f64,
    /// 当前负载（0.0-1.0;负载平滑用）
    #[serde(default)]
    pub load: f64,
}

impl SubAgentProfile {
    /// 新建档案
    #[must_use]
    pub fn new(profile_id: impl Into<String>, kind: SubAgentKind, unit_cost: f64) -> Self {
        Self {
            profile_id: profile_id.into(),
            kind,
            capabilities: kind.capability_tag().into(),
            unit_cost,
            load: 0.0,
        }
    }

    /// 能力匹配度（0.0-1.0:标签交叠比例）
    #[must_use]
    pub fn match_ratio(&self, required: &str) -> f64 {
        let req: Vec<&str> = required
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if req.is_empty() {
            return 1.0;
        }
        let mine: Vec<&str> = self.capabilities.split(',').map(str::trim).collect();
        let hit = req.iter().filter(|r| mine.contains(r)).count();
        hit as f64 / req.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3 类型能力标签 — 唯一且语义正确
    #[test]
    fn kind_tags_unique() {
        let mut set = std::collections::HashSet::new();
        for k in SubAgentKind::ALL {
            assert!(set.insert(k.capability_tag()));
        }
        assert_eq!(SubAgentKind::ALL.len(), 3);
    }

    /// 匹配度 — 标签交叠比例
    #[test]
    fn match_ratio_semantics() {
        let p = SubAgentProfile::new("p1", SubAgentKind::Coder, 1.0);
        assert!((p.match_ratio("code") - 1.0).abs() < 1e-9);
        assert!((p.match_ratio("code,explore") - 0.5).abs() < 1e-9);
        assert!((p.match_ratio("explore") - 0.0).abs() < 1e-9);
        assert!((p.match_ratio("") - 1.0).abs() < 1e-9, "空需求 = 全匹配");
    }

    /// 序列化往返 — 规格/档案可编解码
    #[test]
    fn serde_roundtrip() {
        let spec = SubAgentSpec::new(SubAgentKind::Coder);
        let json = serde_json::to_string(&spec).expect("编码成功");
        let back: SubAgentSpec = serde_json::from_str(&json).expect("解码成功");
        assert_eq!(back, spec);
    }
}
