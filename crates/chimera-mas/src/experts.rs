//! 精英专家团队编制 (§6 CHIMERA-MAS-Q / ADR-027 决策 5)
//!
//! 把设计文档 §6 的 8 位精英专家(SubAgent 层, 10+ 年经验)编码为静态注册表:
//! 每位专家含角色、对应子代理类型、归口 MainAgent 域、主责象限、工具白名单
//! (带三级权限)、准入条件、退出条件。
//!
//! ## 三级权限模型 (§11.2)
//!
//! - **L0 ReadOnly**: 只读(Read/Grep/Glob/LSP、只读 Bash、WebSearch)—— 默认开放。
//! - **L1 LimitedWrite**: 受限写(Edit/Write、构建/测试/格式化)—— 需任务上下文授权。
//! - **L2 HighRiskApproval**: 高危需审批(删除、发布打 tag、部署、外发)—— 双人确认 + 审计。
//!
//! ## 象限映射 (§6.1)
//!
//! | 专家 | 主责象限 |  | 专家 | 主责象限 |
//! |------|:--------:|--|------|:--------:|
//! | E01 发布分析 | Q4 |  | E05 TDD 守护 | Q3 |
//! | E02 架构优化 | Q2 |  | E06 安全审计 | Q4 |
//! | E03 Rust 架构 | Q1 |  | E07 性能优化 | Q4 |
//! | E04 代码审查 | Q3 |  | E08 DevOps/可观测 | Q4 |
//!
//! WHY 静态注册表: 专家编制是恒定配置, 用 `static` 存储零运行时开销, 可查询、
//! 可校验、可审计(最小权限原则落地)。

use crate::quadrant::{Quadrant, QuadrantSelector};
use std::sync::Arc;

/// 工具授权级别 (§11.2) —— 排序: ReadOnly < LimitedWrite < HighRiskApproval。
///
/// WHY 派生 Ord(按声明序): 支持 `highest_tier()` 求专家所需最高权限, 用于审批决策。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum PermissionTier {
    /// L0 只读 —— 默认开放, 无需审批。
    ReadOnly,
    /// L1 受限写 —— 需任务上下文授权且属专家白名单。
    LimitedWrite,
    /// L2 高危 —— 需显式审批 + 双人确认 + 审计记录。
    HighRiskApproval,
}

impl PermissionTier {
    /// 权限级别标签(L0/L1/L2)。
    pub fn label(&self) -> &'static str {
        match self {
            PermissionTier::ReadOnly => "L0",
            PermissionTier::LimitedWrite => "L1",
            PermissionTier::HighRiskApproval => "L2",
        }
    }
}

/// 单条工具授权 —— 工具名 + 其所需权限级别。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolPermission {
    /// 工具名(如 "Read" / "Bash(cargo)" / "Agent(CodeReview)")。
    pub tool: &'static str,
    /// 该工具所需权限级别。
    pub tier: PermissionTier,
}

impl ToolPermission {
    /// 构造工具授权(const 以支持静态注册表)。
    pub const fn new(tool: &'static str, tier: PermissionTier) -> Self {
        Self { tool, tier }
    }
}

/// 专家编制档案 (§6.2) —— 静态、不可变的专家定义。
///
/// 所有字段为 `&'static`, 使整表可置于 `static` 存储, 零分配、可全程共享。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpertProfile {
    /// 专家编号(E01..E08)。
    pub id: &'static str,
    /// 专家角色名(如 "发布分析专家")。
    pub role_name: &'static str,
    /// 对应子代理类型标识(如 "chimera-release-analyst")。
    pub sub_agent_type: &'static str,
    /// 归口 MainAgent 域(如 "测试发布域")。
    pub main_domain: &'static str,
    /// 主责四象限(§6.1)。
    pub primary_quadrant: Quadrant,
    /// 工具白名单(带三级权限, 遵循最小权限原则)。
    pub tools: &'static [ToolPermission],
    /// 准入条件 —— 满足时被 MainAgent 激活(按需多智能体)。
    pub admit_condition: &'static str,
    /// 退出条件 —— 该专家对本任务的验收门槛。
    pub exit_condition: &'static str,
}

impl ExpertProfile {
    /// 判断白名单是否含指定工具(前缀匹配, 兼容 "Bash(cargo)" 这类带参写法)。
    pub fn has_tool(&self, tool: &str) -> bool {
        self.tools
            .iter()
            .any(|t| t.tool == tool || t.tool.starts_with(tool))
    }

    /// 该专家工具白名单中的最高权限级别(用于审批决策与审计)。
    ///
    /// 空白名单返回 `ReadOnly`(最保守), 但注册表保证每位专家白名单非空。
    pub fn highest_tier(&self) -> PermissionTier {
        self.tools
            .iter()
            .map(|t| t.tier)
            .max()
            .unwrap_or(PermissionTier::ReadOnly)
    }
}

// ============================================================
// 8 位专家工具白名单(§6.2, 静态常量)
// ============================================================

/// E01 发布分析 —— 全只读(版本/发布/ADR 分析)。
const E01_TOOLS: &[ToolPermission] = &[
    ToolPermission::new("Read", PermissionTier::ReadOnly),
    ToolPermission::new("Grep", PermissionTier::ReadOnly),
    ToolPermission::new("Glob", PermissionTier::ReadOnly),
    ToolPermission::new("Bash(readonly)", PermissionTier::ReadOnly),
    ToolPermission::new("WebSearch", PermissionTier::ReadOnly),
];

/// E02 架构优化 —— 只读 + 检索子代理。
const E02_TOOLS: &[ToolPermission] = &[
    ToolPermission::new("Read", PermissionTier::ReadOnly),
    ToolPermission::new("Grep", PermissionTier::ReadOnly),
    ToolPermission::new("Glob", PermissionTier::ReadOnly),
    ToolPermission::new("LSP", PermissionTier::ReadOnly),
    ToolPermission::new("Agent(Search)", PermissionTier::ReadOnly),
];

/// E03 Rust 架构 —— 源码受限写 + cargo 构建。
const E03_TOOLS: &[ToolPermission] = &[
    ToolPermission::new("Read", PermissionTier::ReadOnly),
    ToolPermission::new("Edit", PermissionTier::LimitedWrite),
    ToolPermission::new("LSP", PermissionTier::ReadOnly),
    ToolPermission::new("Bash(cargo)", PermissionTier::LimitedWrite),
    ToolPermission::new("skill(rust-best-practices)", PermissionTier::ReadOnly),
];

/// E04 代码审查 —— 只读 + 审查子代理。
const E04_TOOLS: &[ToolPermission] = &[
    ToolPermission::new("Read", PermissionTier::ReadOnly),
    ToolPermission::new("Grep", PermissionTier::ReadOnly),
    ToolPermission::new("Agent(CodeReview)", PermissionTier::ReadOnly),
    ToolPermission::new("GetProblems", PermissionTier::ReadOnly),
];

/// E05 TDD 守护 —— 测试受限写 + cargo test。
const E05_TOOLS: &[ToolPermission] = &[
    ToolPermission::new("Read", PermissionTier::ReadOnly),
    ToolPermission::new("Edit(tests)", PermissionTier::LimitedWrite),
    ToolPermission::new("Bash(cargo test)", PermissionTier::LimitedWrite),
    ToolPermission::new("skill(test-driven-development)", PermissionTier::ReadOnly),
];

/// E06 安全审计 —— 只读 + cargo audit。
const E06_TOOLS: &[ToolPermission] = &[
    ToolPermission::new("Read", PermissionTier::ReadOnly),
    ToolPermission::new("Grep", PermissionTier::ReadOnly),
    ToolPermission::new("Bash(cargo audit)", PermissionTier::ReadOnly),
    ToolPermission::new("skill(security)", PermissionTier::ReadOnly),
];

/// E07 性能优化 —— 只读 + cargo bench(受限写) + perf 剖析。
const E07_TOOLS: &[ToolPermission] = &[
    ToolPermission::new("Read", PermissionTier::ReadOnly),
    ToolPermission::new("Bash(cargo bench)", PermissionTier::LimitedWrite),
    ToolPermission::new("chrome-devtools(perf)", PermissionTier::ReadOnly),
];

/// E08 DevOps/可观测 —— 只读 + 部署/脚本(高危需审批)。
const E08_TOOLS: &[ToolPermission] = &[
    ToolPermission::new("Read", PermissionTier::ReadOnly),
    ToolPermission::new("Bash(docker/scripts)", PermissionTier::HighRiskApproval),
    ToolPermission::new("WebFetch", PermissionTier::ReadOnly),
];

/// 8 位精英专家静态编制表 (§6.1 / §6.2)。
///
/// WHY `static` 而非 `const`: 需返回 `&'static [ExpertProfile]` 供注册表全程共享;
/// 各字段均 `Sync`(引用 + Copy 枚举), 满足 `static` 约束。
static EXPERTS: [ExpertProfile; 8] = [
    ExpertProfile {
        id: "E01",
        role_name: "发布分析专家",
        sub_agent_type: "chimera-release-analyst",
        main_domain: "测试发布域",
        primary_quadrant: Quadrant::Hardening,
        tools: E01_TOOLS,
        admit_condition: "涉及版本/发布/ADR",
        exit_condition: "发布检查清单全绿",
    },
    ExpertProfile {
        id: "E02",
        role_name: "架构优化分析师",
        sub_agent_type: "architecture-optimization-analyst",
        main_domain: "架构设计域",
        primary_quadrant: Quadrant::Integration,
        tools: E02_TOOLS,
        admit_condition: "涉及跨 crate/层级变更",
        exit_condition: "依赖方向零违规",
    },
    ExpertProfile {
        id: "E03",
        role_name: "Rust 架构专家",
        sub_agent_type: "rust-architecture-expert",
        main_domain: "代码实现域",
        primary_quadrant: Quadrant::Implementation,
        tools: E03_TOOLS,
        admit_condition: "涉及 Rust 实现",
        exit_condition: "clippy/fmt 通过",
    },
    ExpertProfile {
        id: "E04",
        role_name: "代码审查专家",
        sub_agent_type: "code-review-refactor-expert",
        main_domain: "代码实现域",
        primary_quadrant: Quadrant::Verification,
        tools: E04_TOOLS,
        admit_condition: "每次 PR/大改后",
        exit_condition: "≥2 人审查通过",
    },
    ExpertProfile {
        id: "E05",
        role_name: "TDD 守护者",
        sub_agent_type: "tdd-guardian",
        main_domain: "测试发布域",
        primary_quadrant: Quadrant::Verification,
        tools: E05_TOOLS,
        admit_condition: "任何功能/修复",
        exit_condition: "覆盖率达标",
    },
    ExpertProfile {
        id: "E06",
        role_name: "安全审计专家",
        sub_agent_type: "security-auditor",
        main_domain: "架构设计域",
        primary_quadrant: Quadrant::Hardening,
        tools: E06_TOOLS,
        admit_condition: "涉及安全面/外部输入",
        exit_condition: "安全扫描无高危",
    },
    ExpertProfile {
        id: "E07",
        role_name: "性能优化专家",
        sub_agent_type: "performance-optimizer",
        main_domain: "代码实现域",
        primary_quadrant: Quadrant::Hardening,
        tools: E07_TOOLS,
        admit_condition: "涉及性能敏感路径",
        exit_condition: "基准不回退",
    },
    ExpertProfile {
        id: "E08",
        role_name: "DevOps/可观测专家",
        sub_agent_type: "devops-observability",
        main_domain: "测试发布域",
        primary_quadrant: Quadrant::Hardening,
        tools: E08_TOOLS,
        admit_condition: "涉及部署/可观测",
        exit_condition: "部署记录可查",
    },
];

/// 精英专家团队注册表 —— 提供对 8 位专家编制的只读查询。
///
/// 可选支持 `QuadrantSelector`，实现基于任务领域语义的动态象限选择。
/// 未配置选择器时，行为与修改前完全一致（向后兼容）。
#[derive(Debug, Clone)]
pub struct ExpertRegistry {
    /// 静态专家编制表引用。
    experts: &'static [ExpertProfile],
    /// 象限选择器（可选），用于基于任务领域动态选择激活象限。
    /// 使用 `Arc` 包装以支持 `Clone`。
    quadrant_selector: Option<Arc<dyn QuadrantSelector>>,
}

impl Default for ExpertRegistry {
    /// 默认注册表 —— 内置 E01-E08。
    fn default() -> Self {
        Self::new()
    }
}

impl ExpertRegistry {
    /// 创建内置 E01-E08 的注册表（无象限选择器）。
    ///
    /// 默认行为与修改前完全一致：使用 `by_quadrant()` 按固定象限映射查询。
    pub fn new() -> Self {
        Self {
            experts: &EXPERTS,
            quadrant_selector: None,
        }
    }

    /// 全部专家(只读切片, 稳定顺序 E01→E08)。
    pub fn all(&self) -> &'static [ExpertProfile] {
        self.experts
    }

    /// 专家数量(恒为 8)。
    pub fn len(&self) -> usize {
        self.experts.len()
    }

    /// 注册表是否为空(恒为 false, 提供以满足 clippy len/is_empty 惯例)。
    pub fn is_empty(&self) -> bool {
        self.experts.is_empty()
    }

    /// 按编号查询专家(如 "E03"); 不存在返回 `None`。
    pub fn get(&self, id: &str) -> Option<&'static ExpertProfile> {
        self.experts.iter().find(|e| e.id == id)
    }

    /// 按主责象限查询专家(可能多位, 如 Q4 有 E01/E06/E07/E08)。
    ///
    /// 此方法始终使用固定象限映射，不受 `QuadrantSelector` 影响。
    /// 如需动态象限选择，请使用 `select_experts_by_quadrant()`。
    pub fn by_quadrant(&self, quadrant: Quadrant) -> Vec<&'static ExpertProfile> {
        self.experts
            .iter()
            .filter(|e| e.primary_quadrant == quadrant)
            .collect()
    }

    /// 配置象限选择器，支持基于任务领域/语义的动态象限选择。
    ///
    /// 返回 `Self` 以支持链式调用。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// use chimera_mas::experts::ExpertRegistry;
    /// use chimera_mas::quadrant::ConfigurableQuadrantSelector;
    ///
    /// let selector = ConfigurableQuadrantSelector::from_static_mapping();
    /// let registry = ExpertRegistry::new().with_quadrant_selector(Box::new(selector));
    /// ```
    pub fn with_quadrant_selector(mut self, selector: Box<dyn QuadrantSelector>) -> Self {
        self.quadrant_selector = Some(Arc::from(selector));
        self
    }

    /// 基于任务领域和描述选择专家，优先使用象限选择器。
    ///
    /// - 当配置了 `QuadrantSelector` 时：使用选择器动态确定激活象限，
    ///   然后返回这些象限内的专家。
    /// - 未配置选择器时：返回全部专家（降级行为，与修改前一致）。
    pub fn select_experts_by_quadrant(
        &self,
        domain: &str,
        description: &str,
    ) -> Vec<&'static ExpertProfile> {
        match self.quadrant_selector {
            Some(ref selector) => {
                let quadrants = selector.select_quadrants(domain, description);
                let mut result: Vec<&'static ExpertProfile> = quadrants
                    .iter()
                    .flat_map(|q| self.by_quadrant(*q))
                    .collect();
                // 去重（同一专家可能出现在多个象限中）
                result.sort_by_key(|e| e.id);
                result.dedup_by_key(|e| e.id);
                result
            }
            None => self.experts.iter().collect(),
        }
    }
}

// ============================================================
// 单元测试(公开 API 集成级验证见 tests/experts_test.rs)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_has_exactly_eight_experts() {
        let reg = ExpertRegistry::new();
        assert_eq!(reg.len(), 8);
        assert!(!reg.is_empty());
    }

    #[test]
    fn test_ids_are_e01_to_e08_unique_and_ordered() {
        let reg = ExpertRegistry::new();
        let ids: Vec<&str> = reg.all().iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            ["E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08"]
        );
    }

    #[test]
    fn test_permission_tier_ordering() {
        assert!(PermissionTier::HighRiskApproval > PermissionTier::LimitedWrite);
        assert!(PermissionTier::LimitedWrite > PermissionTier::ReadOnly);
    }

    #[test]
    fn test_every_expert_has_non_empty_toolset() {
        let reg = ExpertRegistry::new();
        for e in reg.all() {
            assert!(!e.tools.is_empty(), "{} 工具白名单不应为空", e.id);
        }
    }

    // ============================================================
    // QuadrantSelector 集成测试
    // ============================================================

    #[test]
    fn test_registry_with_quadrant_selector_uses_selector() {
        use crate::quadrant::ConfigurableQuadrantSelector;
        use std::collections::HashMap;

        let mut map = HashMap::new();
        map.insert("代码实现域".into(), vec![Quadrant::Implementation]);
        let selector = ConfigurableQuadrantSelector::new(map, Quadrant::ALL.to_vec());
        let registry = ExpertRegistry::new().with_quadrant_selector(Box::new(selector));

        // 代码实现域 → Implementation → 只有 E03
        let experts = registry.select_experts_by_quadrant("代码实现域", "");
        let ids: Vec<&str> = experts.iter().map(|e| e.id).collect();
        assert_eq!(ids, ["E03"]);
    }

    #[test]
    fn test_registry_with_selector_uses_static_mapping() {
        use crate::quadrant::ConfigurableQuadrantSelector;

        let selector = ConfigurableQuadrantSelector::from_static_mapping();
        let registry = ExpertRegistry::new().with_quadrant_selector(Box::new(selector));

        // 测试发布域 → Hardening, Verification → E01, E04, E05, E06, E07, E08
        let experts = registry.select_experts_by_quadrant("测试发布域", "");
        let ids: Vec<&str> = experts.iter().map(|e| e.id).collect();
        assert_eq!(ids, ["E01", "E04", "E05", "E06", "E07", "E08"]);
    }

    #[test]
    fn test_registry_without_selector_returns_all_experts() {
        let registry = ExpertRegistry::new();
        // 未配置选择器时，select_experts_by_quadrant 返回全部专家
        let experts = registry.select_experts_by_quadrant("任意领域", "");
        assert_eq!(experts.len(), 8);
    }

    #[test]
    fn test_registry_by_quadrant_unchanged_backward_compat() {
        // 验证 by_quadrant 行为与修改前一致
        let registry = ExpertRegistry::new();
        assert_eq!(registry.by_quadrant(Quadrant::Implementation).len(), 1);
        assert_eq!(registry.by_quadrant(Quadrant::Integration).len(), 1);
        assert_eq!(registry.by_quadrant(Quadrant::Verification).len(), 2);
        assert_eq!(registry.by_quadrant(Quadrant::Hardening).len(), 4);
    }

    #[test]
    fn test_registry_clone_after_selector() {
        // 验证配置了选择器的注册表仍可 Clone
        use crate::quadrant::ConfigurableQuadrantSelector;

        let selector = ConfigurableQuadrantSelector::from_static_mapping();
        let registry = ExpertRegistry::new().with_quadrant_selector(Box::new(selector));
        let cloned = registry.clone();
        assert_eq!(cloned.len(), 8);
    }
}
