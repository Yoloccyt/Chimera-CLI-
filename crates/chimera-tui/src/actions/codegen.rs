//! codegen — 从 ActionRegistry 生成四入口内容(ADR-029,v3.1 §4.2;Concord T1.3 第四通道)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **单一事实源的工程保证**:斜杠命令表 / 命令面板条目 / ? 帮助内容 / 键位路由表
//!   全部由本模块从 `ActionRegistry` 生成,任何入口都不手写功能清单——这是
//!   "四入口行为一致、永不漂移"(§4.2 铁律 + Concord P5② 收口)的落地点。
//! - **标题在生成时解析 i18n**:`ActionDescriptor` 只存 key,codegen 在生成
//!   展示条目时经 `i18n::tr` 解析为当前 locale 文案,locale 切换即时反映。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

/// 生成的键位绑定 — InputRouter 全局路由表条目(Concord T1.3 第四通道)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    /// 解析后的按键事件(含修饰符)
    pub key: KeyEvent,
    /// 绑定到的动作 id
    pub action_id: &'static str,
}

/// 解析键位声明字符串为 `KeyEvent`(codegen 唯一键串解析点)
///
/// # 支持形式
/// - `"Ctrl+X"`:Ctrl 组合键(字符转小写,与 crossterm 上报一致)
/// - 单字符:含可打印符号(如 `"?"` `"]"` `"\\"`)
/// - 命名键:`"Enter"` / `"Tab"` / `"Esc"` / `"F1"`~`"F12"`
///
/// # 返回值
/// 无法解析时返回 `None`(调用方跳过该绑定并记警告,不中断启动);
/// 声明笔误由双源不变量测试(INV-K-A)在 CI 暴露。
pub fn parse_key_spec(spec: &str) -> Option<KeyEvent> {
    if let Some(rest) = spec.strip_prefix("Ctrl+") {
        let c = rest.chars().next()?.to_ascii_lowercase();
        return Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }
    if let Some(n) = spec.strip_prefix('F') {
        if let Ok(num) = n.parse::<u8>() {
            return Some(KeyEvent::new(KeyCode::F(num), KeyModifiers::NONE));
        }
    }
    let code = match spec {
        "Enter" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "Esc" => KeyCode::Esc,
        one if one.chars().count() == 1 => KeyCode::Char(one.chars().next()?),
        _ => return None,
    };
    Some(KeyEvent::new(code, KeyModifiers::NONE))
}

/// 生成全局键位绑定表(第四通道,Concord T1.3)
///
/// 仅纳入 `default_key` 已声明且 `global_route=true` 的动作;每条绑定的
/// `alias_keys` 同步展开为等价绑定(如 export.run 的 'E' 别名)。
/// InputRouter 消费本表替代手写路由分支(P5② 键位双源收口)。
pub fn key_bindings(reg: &ActionRegistry) -> Vec<KeyBinding> {
    let mut out = Vec::new();
    for d in reg.all() {
        if !d.global_route {
            continue;
        }
        let Some(spec) = d.default_key else {
            continue;
        };
        if let Some(key) = parse_key_spec(spec) {
            out.push(KeyBinding {
                key,
                action_id: d.id,
            });
        } else {
            // 声明笔误:不阻断启动,跳过并告警(INV-K-A 会在 CI 抓住)
            tracing::warn!(
                action = d.id,
                spec,
                "{}",
                crate::t!("actions.codegen.bad_key_skipped")
            );
        }
        for alias in d.alias_keys {
            if let Some(key) = parse_key_spec(alias) {
                out.push(KeyBinding {
                    key,
                    action_id: d.id,
                });
            }
        }
    }
    out
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
        let system_help = help_lines(&reg, Some(ActionDomain::System));
        assert!(!system_help.is_empty());
        // 全量帮助行数 = 动作总数
        assert_eq!(help_lines(&reg, None).len(), reg.len());
    }

    // === Concord T1.3:第四通道 key_bindings 测试 ===

    #[test]
    fn parse_key_spec_handles_all_declared_forms() {
        use crossterm::event::{KeyCode, KeyModifiers};
        // Ctrl 组合(大小写归一)
        assert_eq!(
            parse_key_spec("Ctrl+L"),
            Some(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key_spec("Ctrl+E"),
            Some(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL))
        );
        // 单字符(含符号)
        assert_eq!(
            parse_key_spec("?"),
            Some(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key_spec("\\"),
            Some(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key_spec("]"),
            Some(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE))
        );
        // 命名键与 F 键
        assert_eq!(
            parse_key_spec("Enter"),
            Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key_spec("F5"),
            Some(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE))
        );
        // 无法解析的形式返回 None(不 panic)
        assert_eq!(parse_key_spec("Unknown"), None);
        assert_eq!(parse_key_spec("Ctrl+"), None);
        assert_eq!(parse_key_spec(""), None);
    }

    #[test]
    fn key_bindings_derive_from_declarations_with_aliases() {
        let reg = ActionRegistry::with_builtin_domains();
        let bindings = key_bindings(&reg);
        // 声明键全部派生:Ctrl+L→locale、l→layout、?→help、]→cycle_companion、
        // w→focus_pane、Ctrl+E→export.run(Concord W3:`\` 已复用为 Chat⇄Dashboard
        // 互切,view.toggle_companion 无默认键不再派生)
        let find = |code: KeyCode, mods: KeyModifiers| {
            bindings
                .iter()
                .find(|b| b.key.code == code && b.key.modifiers == mods)
                .map(|b| b.action_id)
        };
        assert_eq!(
            find(KeyCode::Char('l'), KeyModifiers::CONTROL),
            Some("system.toggle_locale")
        );
        assert_eq!(
            find(KeyCode::Char('l'), KeyModifiers::NONE),
            Some("view.switch_layout")
        );
        assert_eq!(
            find(KeyCode::Char('?'), KeyModifiers::NONE),
            Some("system.open_help")
        );
        assert_eq!(
            find(KeyCode::Char('\\'), KeyModifiers::NONE),
            None,
            "Concord W3 T3.4:`\\` 已复用为视图模式互切,不再派生 companion 键位"
        );
        assert_eq!(
            find(KeyCode::Char(']'), KeyModifiers::NONE),
            Some("view.cycle_companion")
        );
        assert_eq!(
            find(KeyCode::Char('w'), KeyModifiers::NONE),
            Some("view.focus_pane")
        );
        assert_eq!(
            find(KeyCode::Char('e'), KeyModifiers::CONTROL),
            Some("export.run")
        );
        // 别名键展开:'E' 与 Ctrl+E 同达 export.run
        assert_eq!(
            find(KeyCode::Char('E'), KeyModifiers::NONE),
            Some("export.run")
        );
    }

    #[test]
    fn key_bindings_skip_non_global_route_actions() {
        // panel.drill_down 声明 Enter 但 global_route=false(面板内消费),不派生
        let reg = ActionRegistry::with_builtin_domains();
        let bindings = key_bindings(&reg);
        assert!(
            !bindings.iter().any(|b| b.action_id == "panel.drill_down"),
            "global_route=false 的动作不得进入全局键位表"
        );
    }
}
