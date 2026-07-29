//! Behavior Localization — L1→L2→L3 行为定位导航(polish-v2.7 P4-8)
//!
//! 对应架构层:L5 Knowledge(repo-wiki 子模块)
//! 对应 ADR:ADR-049 决策 1(behavior-localization 落点 repo-wiki)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §9.4(腾讯 Handbook BGPD:
//! Behavior-Guided Progressive Disclosure)
//!
//! # 核心思想(腾讯 Handbook)
//!
//! 修改请求 → 修改点的定位不应一次性暴露全库,而是三级渐进披露:
//! L1 系统概览(确定相关执行阶段)→ L2 阶段详情(收集候选代码单元)
//! → L3 调用图扩展(候选沿 caller/callee 关系补全影响面)。
//!
//! # 设计决策(WHY)
//!
//! - **Handbook 由调用方构建**:本模块只做导航算法;Handbook 的自动生成
//!   (源码静态分析)依赖 ISCM 索引,作为后续增量(方案 §9.4
//!   generate_handbook 的完整实现需 AST 解析,超出本期规模)
//! - **关键词匹配定位阶段**:确定性规则(与 AEGIS-lite Planner 同款降级哲学),
//!   语义向量定位留待与 repo-wiki KNN 检索融合的后续增量

use std::collections::{HashMap, HashSet};

/// 执行阶段(Handbook L2)— 系统的一个逻辑处理环节
#[derive(Debug, Clone)]
pub struct ExecutionStage {
    /// 阶段名(如 "intent_encoding" / "quest_decomposition")
    pub name: String,
    /// 阶段描述关键词(L1 定位匹配依据)
    pub keywords: Vec<String>,
    /// 阶段内的代码单元 ID 集合
    pub code_units: Vec<String>,
}

/// 代码单元(Handbook L3)— 定位的最小粒度
#[derive(Debug, Clone)]
pub struct CodeUnit {
    /// 单元唯一标识(如 "pvl-layer::verifier::verify")
    pub unit_id: String,
    /// 源文件路径
    pub file_path: String,
}

/// Harness Handbook — L1 概览 / L2 阶段 / L3 单元三级结构
#[derive(Debug, Default)]
pub struct HarnessHandbook {
    /// L2:执行阶段集合
    pub stages: Vec<ExecutionStage>,
    /// L3:代码单元索引(unit_id → 单元)
    pub units: HashMap<String, CodeUnit>,
    /// L3:调用图邻接(unit_id → 直接 caller/callee 集合)
    pub call_edges: HashMap<String, Vec<String>>,
}

/// 行为定位器 — BGPD 三级渐进披露导航
#[derive(Debug, Default)]
pub struct BehaviorLocalizer {
    handbook: HarnessHandbook,
}

impl BehaviorLocalizer {
    /// 用给定 Handbook 创建定位器
    pub fn new(handbook: HarnessHandbook) -> Self {
        Self { handbook }
    }

    /// BGPD 定位:修改请求描述 → 候选代码单元集合(方案 §9.4 localize)
    ///
    /// 1. **L1**:请求描述与阶段关键词匹配,确定相关阶段
    /// 2. **L2**:收集相关阶段的代码单元
    /// 3. **L3**:沿调用图扩展一跳(caller + callee),补全影响面
    ///
    /// 返回去重后的单元引用(命中单元在前,扩展单元在后)。
    pub fn localize(&self, request_description: &str) -> Vec<&CodeUnit> {
        let request_lower = request_description.to_lowercase();

        // L1: 关键词匹配定位相关阶段
        let relevant_stages: Vec<&ExecutionStage> = self
            .handbook
            .stages
            .iter()
            .filter(|stage| {
                stage
                    .keywords
                    .iter()
                    .any(|kw| request_lower.contains(&kw.to_lowercase()))
            })
            .collect();

        // L2: 收集阶段内候选单元
        let mut ordered: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for stage in &relevant_stages {
            for unit_id in &stage.code_units {
                if seen.insert(unit_id.as_str()) {
                    ordered.push(unit_id.as_str());
                }
            }
        }

        // L3: 调用图一跳扩展(caller/callee 双向,call_edges 已双向存储)
        let direct_hits: Vec<&str> = ordered.clone();
        for unit_id in direct_hits {
            if let Some(neighbors) = self.handbook.call_edges.get(unit_id) {
                for neighbor in neighbors {
                    if seen.insert(neighbor.as_str()) {
                        ordered.push(neighbor.as_str());
                    }
                }
            }
        }

        // 验证候选仍存在于单元索引(方案 §9.4 verify_exists:防 Handbook 陈旧)
        ordered
            .into_iter()
            .filter_map(|id| self.handbook.units.get(id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handbook() -> HarnessHandbook {
        let mut units = HashMap::new();
        for (id, path) in [
            ("verifier::verify", "crates/pvl-layer/src/verifier.rs"),
            ("producer::produce", "crates/pvl-layer/src/producer.rs"),
            ("router::route", "crates/model-router/src/router.rs"),
        ] {
            units.insert(
                id.to_string(),
                CodeUnit {
                    unit_id: id.into(),
                    file_path: path.into(),
                },
            );
        }
        let mut call_edges = HashMap::new();
        // producer 调用 verifier(双向边)
        call_edges.insert(
            "verifier::verify".to_string(),
            vec!["producer::produce".to_string()],
        );
        call_edges.insert(
            "producer::produce".to_string(),
            vec!["verifier::verify".to_string()],
        );

        HarnessHandbook {
            stages: vec![
                ExecutionStage {
                    name: "verification".into(),
                    keywords: vec!["verify".into(), "验证".into()],
                    code_units: vec!["verifier::verify".into()],
                },
                ExecutionStage {
                    name: "routing".into(),
                    keywords: vec!["route".into(), "路由".into()],
                    code_units: vec!["router::route".into()],
                },
            ],
            units,
            call_edges,
        }
    }

    #[test]
    fn test_localize_matches_stage_and_expands_call_graph() {
        let localizer = BehaviorLocalizer::new(handbook());
        let hits = localizer.localize("修改验证失败的处理逻辑");
        let ids: Vec<&str> = hits.iter().map(|u| u.unit_id.as_str()).collect();
        // L1/L2:命中 verification 阶段的 verifier;L3:调用图扩展带出 producer
        assert_eq!(ids[0], "verifier::verify");
        assert!(ids.contains(&"producer::produce"), "调用图应扩展 caller");
        // 无关阶段不应出现
        assert!(!ids.contains(&"router::route"));
    }

    #[test]
    fn test_localize_no_match_returns_empty() {
        let localizer = BehaviorLocalizer::new(handbook());
        assert!(localizer.localize("完全无关的请求主题").is_empty());
    }

    #[test]
    fn test_localize_filters_stale_units() {
        let mut hb = handbook();
        // Handbook 陈旧:阶段引用了已不存在的单元
        hb.stages[0].code_units.push("ghost::unit".into());
        let localizer = BehaviorLocalizer::new(hb);
        let hits = localizer.localize("verify");
        assert!(
            hits.iter().all(|u| u.unit_id != "ghost::unit"),
            "陈旧单元应被 verify_exists 过滤"
        );
    }
}
