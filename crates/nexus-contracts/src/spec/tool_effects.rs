//! 副作用真值源统一契约 — 判定纯函数 + 声明查表索引（L0 纯契约）。
//!
//! # 背景（W3 / L2 工具层重构方案 §6.4，ADR-32X 待落档）
//!
//! W2 前,副作用判定的三级优先序散在 [`crate::spec::tool_plan`] 的消费方
//! （PlanRunner `effective_side_effect`）且查表是 O(n) clone+find；LLM 面
//! 另有独立判定（descriptor 表 + 未知恒 Write），三源互不汇合。
//! 本模块把**判定逻辑**收敛为单一纯函数、**声明查表**收敛为 O(1) 索引。
//!
//! # 四级真值源优先序（方案 §6.4 裁决）
//!
//! 1. **计划节点显式声明**（`ToolNode.side_effect`）——计划作者意图，最高，
//!    不可被下级推翻（执行器自报只读换取并发 = 让被审计方给自己发许可）；
//! 2. **执行器能力自报**（`capability_of`，内置工具 const 表）；
//! 3. **用户显式配置**（`omega.yaml declared_side_effects`，经
//!    [`SideEffectIndex`] 供给）；
//! 4. **descriptor 运行时声明**（tool_plane 注册表，`trust_readonly_hints`
//!    判定结果——server 自报 hints，弱信号，故列末位）。
//!
//! 皆无 → `None` → 调用方按「可能写」保守（fail-closed，既有语义不变）。
//!
//! # 消费面（W3/W3b）
//!
//! - PlanRunner：前三级（本波，判定结果逐位不变，仅查表 O(n)→O(1)）；
//! - LLM tool_loop：3/4 级汇合（W3b，前置 = `llm_answer_with` 配置流打通）。

use std::collections::HashMap;

use crate::tool_plan::SideEffectDecl;

/// 副作用声明查表索引 — O(n) 构建一次 + O(1) 每节点查询。
///
/// WHY 单独类型而非裸 HashMap 参数:装配点语义自文档（键格式 = 计划面点号
/// `mcp.<server>.<tool>`，与 `omega.yaml` 声明格式一致），并给 W3b 的 LLM 面
/// 复用同一数据结构（四级同源）。
///
/// `#[non_exhaustive]`（G-115 open 类纪律）：随 W3b LLM 面演进（descriptor
/// 源/来源标注字段）；外部构造必经 [`from_declarations`] / [`empty`]。
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SideEffectIndex {
    declarations: HashMap<String, SideEffectDecl>,
}

impl SideEffectIndex {
    /// 从 (工具名, 声明) 对构建索引（键为完整工具名；重复键**后到者胜**——
    /// 与 Vec+find 的「首个命中」语义不同，登记：配置重复声明属配置错误，
    /// 后到者胜让最后写入的用户意图生效，与 HashMap 惯例一致）。
    #[must_use]
    pub fn from_declarations(decls: impl IntoIterator<Item = (String, SideEffectDecl)>) -> Self {
        Self {
            declarations: decls.into_iter().collect(),
        }
    }

    /// 空索引（fail-closed：查表恒 None）。
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// 查表（O(1)；未声明 → None）。
    #[must_use]
    pub fn lookup(&self, tool_name: &str) -> Option<SideEffectDecl> {
        self.declarations.get(tool_name).copied()
    }

    /// 索引规模（诊断/测试断言）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.declarations.len()
    }

    /// 索引是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.declarations.is_empty()
    }
}

/// 副作用有效判定 — 单一真值源（消费方不得自行实现优先序）。
///
/// # 参数（按优先级降序）
///
/// - `node_decl`:计划节点显式声明（源 1；`None` = 未声明）；
/// - `capability`:执行器能力自报（源 2，`capability_of` 结果）；
/// - `declared`:声明索引查表结果（源 3，[`SideEffectIndex::lookup`]）。
///
/// 源 4（descriptor 声明）不在此函数——PlanRunner 无 descriptor 视图，其
/// 三级判定结果必须与本函数逐位一致（proptest 锁）；源 4 仅 LLM 面（W3b）
/// 以 `resolve_with_descriptor` 组合。
///
/// # 返回
///
/// `None` = 全部未声明 ⇒ 调用方按「可能写」保守（fail-closed 不扩散）。
#[must_use]
pub fn resolve_effect(
    node_decl: Option<SideEffectDecl>,
    capability: Option<SideEffectDecl>,
    declared: Option<SideEffectDecl>,
) -> Option<SideEffectDecl> {
    node_decl.or(capability).or(declared)
}

/// 四级判定（LLM 面专用，W3b 接线）：源 3（配置）**优先于**源 4（descriptor）。
///
/// WHY 源 3 > 源 4:用户显式声明是更强的意图信号，descriptor 的
/// `trust_readonly_hints` 是 server 自报 hints（弱信号，ADR-235 D1 默认 false）。
#[must_use]
pub fn resolve_effect_with_descriptor(
    capability: Option<SideEffectDecl>,
    declared: Option<SideEffectDecl>,
    descriptor: Option<SideEffectDecl>,
) -> Option<SideEffectDecl> {
    capability.or(declared).or(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;

    // === SideEffectIndex ===

    #[test]
    fn index_lookup_and_size() {
        let index = SideEffectIndex::from_declarations(vec![
            ("mcp.a.search".to_string(), SideEffectDecl::ReadOnly),
            ("mcp.b.deploy".to_string(), SideEffectDecl::Write),
        ]);
        assert_eq!(index.len(), 2);
        assert!(!index.is_empty());
        assert_eq!(index.lookup("mcp.a.search"), Some(SideEffectDecl::ReadOnly));
        assert_eq!(index.lookup("mcp.b.deploy"), Some(SideEffectDecl::Write));
        assert_eq!(index.lookup("mcp.unknown.t"), None);
    }

    #[test]
    fn empty_index_is_fail_closed() {
        let index = SideEffectIndex::empty();
        assert!(index.is_empty());
        assert_eq!(index.lookup("mcp.any.t"), None);
    }

    // === resolve_effect 三级优先序（PlanRunner 等价锁） ===

    #[test]
    fn node_declaration_has_highest_priority() {
        // 节点 Write 不可被下级只读推翻（被审计方不得给自己发许可）
        assert_eq!(
            resolve_effect(
                Some(SideEffectDecl::Write),
                Some(SideEffectDecl::ReadOnly),
                Some(SideEffectDecl::ReadOnly),
            ),
            Some(SideEffectDecl::Write)
        );
        // 节点 ReadOnly 同理最高
        assert_eq!(
            resolve_effect(
                Some(SideEffectDecl::ReadOnly),
                Some(SideEffectDecl::Write),
                None,
            ),
            Some(SideEffectDecl::ReadOnly)
        );
    }

    #[test]
    fn capability_beats_declared() {
        assert_eq!(
            resolve_effect(
                None,
                Some(SideEffectDecl::ReadOnly),
                Some(SideEffectDecl::Write)
            ),
            Some(SideEffectDecl::ReadOnly)
        );
    }

    #[test]
    fn all_none_is_fail_closed() {
        assert_eq!(resolve_effect(None, None, None), None);
    }

    // === proptest（PlanRunner effective_side_effect 等价性） ===

    proptest::proptest! {
        /// 优先序不变量:任一级 Some 即短路下级——与 PlanRunner 三级
        /// `node.or(capability).or(declared)` 形态逐位等价。
        #[test]
        fn priority_order_short_circuits(
            n in 0u8..3,
            c in 0u8..3,
            d in 0u8..3,
        ) {
            let to_decl = |v: u8| match v {
                0 => None,
                1 => Some(SideEffectDecl::ReadOnly),
                _ => Some(SideEffectDecl::Write),
            };
            proptest::prop_assert_eq!(
                resolve_effect(to_decl(n), to_decl(c), to_decl(d)),
                to_decl(n).or(to_decl(c)).or(to_decl(d))
            );
        }
    }
}
