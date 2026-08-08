//! 平台接地规格 — 将平台/环境约束固化为可审计契约（Milestone B-4）
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §5.1 P2 / §6 B-4
//! 对应设计: 根目录设计文档 §4.2（北大 NL2Pipeline gap 解）
//!
//! # 职责
//!
//! 承载平台接地契约：环境变量 / 工具链 / 路径 / 权限 / 配置五类要求，
//! 供 L9 efficiency-monitor RuntimeAuditor 第 0 维度（契约遵守）审计消费。
//! 纯类型 + 纯函数（ADR-033 L0 契约层），无 IO、无平台逻辑。

use serde::{Deserialize, Serialize};

/// 接地要求分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum GroundingCategory {
    /// 环境变量（如 CARGO_HOME 指向项目工具链）
    Env = 0,
    /// 工具链（如 GNU stable-x86_64-pc-windows-gnu）
    Toolchain = 1,
    /// 路径（如 msys64/mingw64/bin 在 PATH 中）
    Path = 2,
    /// 权限（如可写目录、执行权限）
    Permission = 3,
    /// 配置（如 .cargo/config.toml 固化 linker）
    Config = 4,
}

impl GroundingCategory {
    /// 从文档标记解析（"PG-ENV" → Env）；未知标记返回 None
    pub fn from_marker(marker: &str) -> Option<Self> {
        match marker {
            "ENV" => Some(Self::Env),
            "TOOLCHAIN" => Some(Self::Toolchain),
            "PATH" => Some(Self::Path),
            "PERMISSION" => Some(Self::Permission),
            "CONFIG" => Some(Self::Config),
            _ => None,
        }
    }
}

/// 单条接地要求
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundingRequirement {
    /// 要求 ID（文档行序号派生，如 "r0"）
    pub requirement_id: String,
    /// 要求描述（自然语言，观测按包含匹配）
    pub description: String,
    /// 分类
    pub category: GroundingCategory,
}

/// 平台接地规格 — 平台约束的可审计契约
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformGroundingSpec {
    /// 规格 ID（"pg-" 前缀）
    pub spec_id: String,
    /// 平台标识（如 "windows-gnu" / "linux" / "darwin"）
    pub platform: String,
    /// 接地要求清单
    pub requirements: Vec<GroundingRequirement>,
}

/// 接地校验结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroundingCheckOutcome {
    /// 全部要求满足（或规格为空——无要求即无违反）
    Grounded,
    /// 存在未满足要求
    Violated {
        /// 未满足要求的描述列表
        missing: Vec<String>,
    },
}

impl PlatformGroundingSpec {
    /// 从平台约束文档提取骨架
    ///
    /// 解析格式：`PG-<CATEGORY>: <描述>` 行（每行一条要求）；
    /// 其他行（标题/注释/叙述）忽略。ID 按出现序派生（r0, r1, ...）。
    pub fn from_doc(spec_id: impl Into<String>, platform: impl Into<String>, doc: &str) -> Self {
        let mut requirements = Vec::new();
        for (idx, line) in doc.lines().enumerate() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("PG-") else {
                continue;
            };
            let Some((marker, description)) = rest.split_once(':') else {
                continue;
            };
            let Some(category) = GroundingCategory::from_marker(marker.trim()) else {
                continue;
            };
            requirements.push(GroundingRequirement {
                requirement_id: format!("r{idx}"),
                description: description.trim().to_string(),
                category,
            });
        }
        Self {
            spec_id: spec_id.into(),
            platform: platform.into(),
            requirements,
        }
    }

    /// 接地校验 — 观测集合必须覆盖全部要求（包含匹配）
    ///
    /// 与 `BehaviorContract::enforce` 同款观测语义：观测条目为
    /// "已满足约束"的自然语言描述，要求按子串匹配判定覆盖。
    pub fn check(&self, observed: &[String]) -> GroundingCheckOutcome {
        let missing: Vec<String> = self
            .requirements
            .iter()
            .filter(|r| !observed.iter().any(|o| o.contains(&r.description)))
            .map(|r| r.description.clone())
            .collect();
        if missing.is_empty() {
            GroundingCheckOutcome::Grounded
        } else {
            GroundingCheckOutcome::Violated { missing }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_marker_roundtrip() {
        assert_eq!(
            GroundingCategory::from_marker("ENV"),
            Some(GroundingCategory::Env)
        );
        assert_eq!(
            GroundingCategory::from_marker("CONFIG"),
            Some(GroundingCategory::Config)
        );
        assert_eq!(GroundingCategory::from_marker("UNKNOWN"), None);
    }
}
