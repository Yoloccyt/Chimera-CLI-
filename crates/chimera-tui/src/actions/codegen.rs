//! codegen — 从 ActionRegistry 生成三入口内容(ADR-029,v3.1 §4.2)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **单一事实源的工程保证**:斜杠命令表 / 命令面板条目 / ? 帮助内容全部由本
//!   模块从 `ActionRegistry` 生成,任何入口都不手写功能清单——这是"三入口行为
//!   一致、永不漂移"(§4.2 铁律)的落地点。
//! - **标题在生成时解析 i18n**:`ActionDescriptor` 只存 key,codegen 在生成
//!   展示条目时经 `i18n::tr` 解析为当前 locale 文案,locale 切换即时反映。

use crate::actions::descriptor::ActionDomain;
use crate::actions::registry::ActionRegistry;

/// 生成的斜杠命令项 — Chat 面板斜杠命令入口
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// 斜杠触发词(含前导 `/`,如 "/quest pause")
    pub command: String,
    /// 关联的动作 id
    pub action_id: &'static str,
}

/// 生成的命令面板条目 — 命令面板(Ctrl+P)入口
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    /// 关联的动作 id
    pub action_id: &'static str,
    /// 已解析为当前 locale 的标题
    pub title: String,
    /// 副标题(斜杠词或域名,辅助辨识)
    pub subtitle: String,
}

/// 生成的帮助行 — ? 帮助面板入口
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpLine {
    /// 快捷键(无则为斜杠词)
    pub key: String,
    /// 已解析为当前 locale 的动作标题
    pub title: String,
}

/// 生成全部斜杠命令(仅含声明了 `slash` 的动作)
pub fn slash_commands(reg: &ActionRegistry) -> Vec<SlashCommand> {
    reg.all()
        .iter()
        .filter_map(|d| {
            d.slash.map(|s| SlashCommand {
                command: format!("/{s}"),
                action_id: d.id,
            })
        })
        .collect()
}

/// 生成命令面板条目(按 query 模糊过滤;空 query 返回全部)
pub fn palette_entries(reg: &ActionRegistry, query: &str) -> Vec<PaletteEntry> {
    reg.fuzzy_search(query)
        .into_iter()
        .map(|d| PaletteEntry {
            action_id: d.id,
            title: crate::i18n::tr(d.title_key).to_string(),
            // 副标题优先展示斜杠词,无斜杠则展示域名,辅助用户辨识来源
            subtitle: match d.slash {
                Some(s) => format!("/{s}"),
                None => d.domain.as_str().to_string(),
            },
        })
        .collect()
}

/// 生成 ? 帮助面板行(可选按域过滤;`None` 返回全部)
///
/// WHY 按域过滤:面板上下文帮助只展示当前焦点面板相关域的动作,
/// 契合 §4.6 原则 4"渐进披露"——帮助按上下文动态生成,不平铺全部。
pub fn help_lines(reg: &ActionRegistry, domain: Option<ActionDomain>) -> Vec<HelpLine> {
    reg.all()
        .iter()
        .filter(|d| domain.is_none_or(|dom| d.domain == dom))
        .map(|d| HelpLine {
            // 快捷键优先;无快捷键则回退到斜杠词;两者皆无则空
            key: d
                .default_key
                .map(str::to_string)
                .or_else(|| d.slash.map(|s| format!("/{s}")))
                .unwrap_or_default(),
            title: crate::i18n::tr(d.title_key).to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_commands_generated_from_registry() {
        let reg = ActionRegistry::with_builtin_domains();
        let cmds = slash_commands(&reg);
        // 声明了斜杠词的动作都应出现,且带前导 /
        assert!(cmds
            .iter()
            .any(|c| c.command == "/export" && c.action_id == "export.run"));
        assert!(cmds.iter().all(|c| c.command.starts_with('/')));
    }

    #[test]
    fn palette_entries_resolve_titles_and_filter() {
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let reg = ActionRegistry::with_builtin_domains();
        // 空 query 返回全部
        assert_eq!(palette_entries(&reg, "").len(), reg.len());
        // 中文标题解析:export.run → "导出数据"
        let export = palette_entries(&reg, "export")
            .into_iter()
            .find(|e| e.action_id == "export.run")
            .expect("应命中 export.run");
        assert_eq!(export.title, "导出数据");
    }

    #[test]
    fn help_lines_filter_by_domain() {
        let reg = ActionRegistry::with_builtin_domains();
        let task_help = help_lines(&reg, Some(ActionDomain::Task));
        assert!(!task_help.is_empty());
        // 全量帮助行数 = 动作总数
        assert_eq!(help_lines(&reg, None).len(), reg.len());
    }
}
