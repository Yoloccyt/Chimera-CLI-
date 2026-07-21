//! ActionRegistry — Action 单一事实源(ADR-029,v3.1 §4.2)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **单一事实源**:斜杠命令 / 命令面板 / 面板上下文动作三入口全部从本注册表
//!   派生(经 `codegen`),杜绝三处手写清单漂移。
//! - **只做索引,不做上帝对象**:Action 按域在 `domains/` 分包声明,注册表仅
//!   聚合 + 建立 id→下标索引 + 提供查询/模糊匹配。
//! - **40 项熔断线**(§十二):注册项突破 `MAX_ACTIONS` 视为交互粒度失控信号,
//!   `is_over_budget()` 供 CI/评审检测,提示回到"核心功能 ≤3 键可达"重新裁剪。

use std::collections::HashMap;

use crate::actions::descriptor::{ActionDescriptor, ActionDomain};
use crate::actions::domains;

/// Action 注册项熔断上限 — 突破即视为交互粒度失控(§十二)
///
/// WHY 40:经验阈值。交互式 TUI 的核心功能约 10 个,加扩展功能余量至 40;
/// 超过通常意味着把本应是参数的选项拆成了独立 Action,应合并为子命令。
pub const MAX_ACTIONS: usize = 40;

/// 动作注册表 — 索引全部可交互动作
#[derive(Debug, Clone, Default)]
pub struct ActionRegistry {
    /// 有序动作列表(保留注册顺序,供 palette/help 稳定展示)
    descriptors: Vec<ActionDescriptor>,
    /// id → descriptors 下标索引,提供 O(1) 精确查询
    index: HashMap<&'static str, usize>,
}

impl ActionRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建并装入六个内建域的全部动作(生产入口)
    ///
    /// WHY 独立构造器:测试可用 `new()` + `register` 精确控制内容,
    /// 生产用 `with_builtin_domains()` 一次性纳入 §五 动作基线。
    pub fn with_builtin_domains() -> Self {
        let mut reg = Self::new();
        for desc in domains::all_builtin_descriptors() {
            reg.register(desc);
        }
        reg
    }

    /// 注册一个动作;若 id 重复则忽略并返回 `false`(防止覆盖既有描述)
    ///
    /// WHY 拒绝重复而非覆盖:重复 id 通常是复制粘贴错误,静默覆盖会导致
    /// 难以察觉的行为漂移;返回 `false` 便于调用方(测试)断言唯一性。
    pub fn register(&mut self, desc: ActionDescriptor) -> bool {
        if self.index.contains_key(desc.id) {
            return false;
        }
        let idx = self.descriptors.len();
        self.index.insert(desc.id, idx);
        self.descriptors.push(desc);
        true
    }

    /// 按 id 精确查询
    pub fn get(&self, id: &str) -> Option<&ActionDescriptor> {
        self.index.get(id).map(|&i| &self.descriptors[i])
    }

    /// 返回全部动作(注册顺序)
    pub fn all(&self) -> &[ActionDescriptor] {
        &self.descriptors
    }

    /// 动作总数
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    /// 注册表是否为空
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// 是否超过熔断上限(§十二)
    pub fn is_over_budget(&self) -> bool {
        self.descriptors.len() > MAX_ACTIONS
    }

    /// 返回指定域的全部动作
    pub fn by_domain(&self, domain: ActionDomain) -> Vec<&ActionDescriptor> {
        self.descriptors
            .iter()
            .filter(|d| d.domain == domain)
            .collect()
    }

    /// 返回全部核心动作(is_core,可达性验收基线)
    pub fn core_actions(&self) -> Vec<&ActionDescriptor> {
        self.descriptors.iter().filter(|d| d.is_core).collect()
    }

    /// 模糊搜索 — 供命令面板检索(§8.3 可发现性)
    ///
    /// 匹配规则(大小写不敏感):query 命中 id / 斜杠词 / 解析后的标题任一即返回。
    /// WHY 三路匹配:用户可能记得功能 id(quest.pause)、斜杠词(quest pause)
    /// 或界面标题(暂停 Quest / Pause Quest),任一命中都应可达,保证可发现性。
    /// 空 query 返回全部(palette 打开时展示完整列表)。
    pub fn fuzzy_search(&self, query: &str) -> Vec<&ActionDescriptor> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.descriptors.iter().collect();
        }
        self.descriptors
            .iter()
            .filter(|d| Self::matches(d, &q))
            .collect()
    }

    /// 判断单个动作是否命中查询(id/slash/标题三路)
    fn matches(desc: &ActionDescriptor, query_lower: &str) -> bool {
        if desc.id.to_lowercase().contains(query_lower) {
            return true;
        }
        if let Some(slash) = desc.slash {
            if slash.to_lowercase().contains(query_lower) {
                return true;
            }
        }
        // 标题经 i18n 解析后按当前 locale 匹配(中/英均可发现)
        crate::i18n::tr(desc.title_key)
            .to_lowercase()
            .contains(query_lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_registers_baseline_actions() {
        let reg = ActionRegistry::with_builtin_domains();
        // §五 首期基线:六域动作已装入,数量应 > 0 且远低于熔断线
        assert!(!reg.is_empty());
        assert!(!reg.is_over_budget(), "内建动作不应超过 40 熔断线");
        assert!(reg.len() < MAX_ACTIONS);
    }

    #[test]
    fn action_ids_are_unique() {
        let reg = ActionRegistry::with_builtin_domains();
        // 重复注册同 id 应被拒绝
        let mut reg2 = ActionRegistry::new();
        let all = reg.all().to_vec();
        for d in &all {
            assert!(reg2.register(*d), "首次注册 {} 应成功", d.id);
        }
        for d in &all {
            assert!(!reg2.register(*d), "重复注册 {} 应被拒绝", d.id);
        }
    }

    #[test]
    fn core_actions_present_and_reachable() {
        let reg = ActionRegistry::with_builtin_domains();
        let core = reg.core_actions();
        // 核心功能存在(agent.chat / task.create / export.run / system.toggle_locale 等)
        assert!(reg.get("agent.chat").is_some());
        assert!(reg.get("export.run").is_some());
        assert!(reg.get("system.toggle_locale").is_some());
        assert!(!core.is_empty(), "应存在核心动作");
    }

    #[test]
    fn every_action_is_discoverable_by_id() {
        // §8.3:每个动作都能被命令面板按 id 检索命中
        let reg = ActionRegistry::with_builtin_domains();
        for d in reg.all() {
            let hits = reg.fuzzy_search(d.id);
            assert!(
                hits.iter().any(|h| h.id == d.id),
                "动作 {} 应能被自身 id 搜索命中",
                d.id
            );
        }
    }

    #[test]
    fn fuzzy_search_matches_slash_and_empty_returns_all() {
        let reg = ActionRegistry::with_builtin_domains();
        // 空 query 返回全部
        assert_eq!(reg.fuzzy_search("").len(), reg.len());
        // 斜杠词片段命中
        let hits = reg.fuzzy_search("export");
        assert!(hits.iter().any(|d| d.id == "export.run"));
    }

    #[test]
    fn by_domain_partitions_actions() {
        let reg = ActionRegistry::with_builtin_domains();
        let task = reg.by_domain(ActionDomain::Task);
        assert!(task.iter().all(|d| d.domain == ActionDomain::Task));
        assert!(task.iter().any(|d| d.id == "task.create"));
    }
}
