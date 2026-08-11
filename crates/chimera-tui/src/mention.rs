//! @ 引用候选补全(Concord W4 T4.5,Claude Code/Codex 对齐)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! composer Insert 态输入 `@` 后按 Tab 补全引用。候选**零管道派生**自
//! 既有状态(面板名 + Quest ID),不引入文件系统索引——无候选时字面保留
//! '@'(诚实降级,不伪造文件引用能力)。

use crate::types::{PanelId, TuiState};

/// @ 引用候选最大数量(防候选列表过长)
pub const MAX_MENTION_CANDIDATES: usize = 8;

/// 从状态派生 @ 引用候选(面板名 + Quest ID),按前缀过滤
///
/// # 参数
/// - `state`:当前 TUI 状态
/// - `prefix`:`@` 之后已输入的前缀(空 = 全部候选)
///
/// # 返回
/// 补全后的完整引用文本列表(含 `@` 前缀),面板名在前、Quest ID 在后,
/// 各自保持注册序/事件序;截断至 MAX_MENTION_CANDIDATES。
pub fn mention_candidates(state: &TuiState, prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // 面板名候选(REGISTERED_FOCUS_ORDER 单一事实源,W1 资产复用)
    for panel in PanelId::REGISTERED_FOCUS_ORDER {
        let name = panel.as_str();
        if name.to_lowercase().starts_with(&prefix.to_lowercase()) {
            out.push(format!("@{name}"));
        }
    }
    // Quest ID 候选(quest_list 事件序)
    for quest in &state.quest_list {
        if quest
            .quest_id
            .to_lowercase()
            .starts_with(&prefix.to_lowercase())
        {
            out.push(format!("@{}", quest.quest_id));
        }
    }
    out.truncate(MAX_MENTION_CANDIDATES);
    out
}

/// 提取输入缓冲末尾的 @ 引用片段
///
/// # 返回
/// Some((起始字节偏移, '@' 后的前缀))——末尾词以 '@' 起始时;否则 None。
///
/// # 边界
/// 末尾词定义为最后一个空白符之后的片段;空缓冲/末尾空白返回 None。
pub fn extract_mention_tail(buffer: &str) -> Option<(usize, String)> {
    if buffer.is_empty() || buffer.ends_with(char::is_whitespace) {
        return None;
    }
    let tail = buffer.rsplit_once(char::is_whitespace);
    let (start, word) = match tail {
        Some((_, w)) => (buffer.len() - w.len(), w),
        None => (0, buffer),
    };
    word.strip_prefix('@')
        .map(|prefix| (start, prefix.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::Quest;
    use proptest::prelude::*;

    #[test]
    fn empty_prefix_lists_panels_first() {
        let state = TuiState::new();
        let cands = mention_candidates(&state, "");
        assert!(!cands.is_empty(), "面板名候选应存在");
        assert!(cands.len() <= MAX_MENTION_CANDIDATES);
        assert!(cands.iter().all(|c| c.starts_with('@')));
    }

    #[test]
    fn prefix_filters_case_insensitive() {
        let mut state = TuiState::new();
        state.quest_list.push(Quest {
            quest_id: "q-Alpha".into(),
            ..Default::default()
        });
        let cands = mention_candidates(&state, "q-al");
        assert!(cands.contains(&"@q-Alpha".to_string()), "大小写不敏感匹配");
    }

    #[test]
    fn no_match_returns_empty() {
        let state = TuiState::new();
        assert!(mention_candidates(&state, "zzz-no-such").is_empty());
    }

    #[test]
    fn extract_mention_tail_variants() {
        assert_eq!(extract_mention_tail(""), None);
        assert_eq!(extract_mention_tail("hello "), None);
        assert_eq!(extract_mention_tail("hello"), None);
        assert_eq!(
            extract_mention_tail("hello @qu"),
            Some((6, "qu".to_string()))
        );
        assert_eq!(extract_mention_tail("@"), Some((0, String::new())));
    }

    proptest! {
        /// 候选数恒不超上限;全部以 @ 开头
        #[test]
        fn candidates_bounded_and_prefixed(n_quests in 0usize..30) {
            let mut state = TuiState::new();
            for i in 0..n_quests {
                state.quest_list.push(Quest {
                    quest_id: format!("q-{i}"),
                    ..Default::default()
                });
            }
            let cands = mention_candidates(&state, "");
            prop_assert!(cands.len() <= MAX_MENTION_CANDIDATES);
            for c in &cands {
                prop_assert!(c.starts_with('@'));
            }
        }
    }
}
