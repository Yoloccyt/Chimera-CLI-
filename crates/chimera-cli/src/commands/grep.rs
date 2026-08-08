//! Agent Grep CLI 命令 — `chimera grep <pattern>`（Milestone B-5）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §5.1 P2 / §6 B-5）：
//! 库层 `repo_wiki::agent_grep::AgentGrep` 已落地，本模块接线 CLI 入口。
//! 双通道检索：知识通道（FTS5/LIKE 降级）+ 代码通道（BGPD 三级披露）。

use std::path::PathBuf;

use anyhow::Result;
use repo_wiki::agent_grep::{AgentGrep, AgentGrepConfig};
use repo_wiki::behavior_localization::{BehaviorLocalizer, HarnessHandbook};
use repo_wiki::WikiStore;

use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;

/// 知识命中摘要最大字符数
const SNIPPET_CHARS: usize = 120;

/// 执行 agent grep 命令 — 双通道检索知识库与代码行为定位
///
/// `pattern` 为检索模式，`config` 提供数据库路径，`json` 控制输出格式。
/// rusqlite 同步调用经 `spawn_blocking` 包裹（§4.4 红线 2）。
pub async fn execute(pattern: &str, config: &ChimeraConfig, json: bool) -> Result<()> {
    tracing::info!(pattern = %pattern, "Agent Grep 检索");

    // 1. 解析数据库路径（展开 ~）
    let db_path = expand_db_path(&config.repo_wiki.db_path)?;

    // 2. 打开 WikiStore（连接池 + WAL + FTS5）
    let store = WikiStore::open(&db_path)
        .map_err(|e| ChimeraCliError::EngineError(format!("WikiStore 打开失败: {e}")))?;

    // 3. 双通道检索（同步库 API 在 spawn_blocking 中执行）
    let store = store.clone();
    let pattern_display = pattern.to_string(); // 输出阶段回显（闭包 move 后仍可用）
    let pattern = pattern.to_string();
    let report = tokio::task::spawn_blocking(move || {
        store.with_read_conn_sync(|conn| {
            let grep = AgentGrep::new(
                BehaviorLocalizer::new(HarnessHandbook::default()),
                AgentGrepConfig::default(),
            );
            grep.search(conn, &pattern)
        })
    })
    .await
    .map_err(|e| ChimeraCliError::EngineError(format!("Agent Grep 任务失败: {e}")))?
    .map_err(|e| ChimeraCliError::EngineError(format!("Agent Grep 检索失败: {e}")))?;

    // 4. 输出
    if json {
        output::print_json(&report)?;
    } else if report.is_empty() {
        output::print_info(&format!("未找到匹配「{pattern_display}」的知识或代码"));
    } else {
        // 知识通道命中
        if !report.knowledge_hits.is_empty() {
            let rows: Vec<Vec<String>> = report
                .knowledge_hits
                .iter()
                .enumerate()
                .map(|(i, hit)| {
                    let snippet = truncate(&hit.snippet, SNIPPET_CHARS);
                    vec![(i + 1).to_string(), hit.title.clone(), snippet]
                })
                .collect();
            output::print_table(&["#", "知识条目", "摘要"], &rows);
        }
        // 代码通道命中
        if !report.code_hits.is_empty() {
            let rows: Vec<Vec<String>> = report
                .code_hits
                .iter()
                .enumerate()
                .map(|(i, hit)| {
                    vec![
                        (i + 1).to_string(),
                        hit.unit_id.clone(),
                        hit.file_path.clone(),
                    ]
                })
                .collect();
            output::print_table(&["#", "单元", "位置"], &rows);
        }
        if report.knowledge_degraded {
            output::print_warning("知识通道已降级为 LIKE 检索（FTS5 不可用）");
        }
    }

    Ok(())
}

/// 展开 `~` 为 home 目录（与 wiki 命令同款，避免重复实现）
fn expand_db_path(raw: &str) -> Result<PathBuf> {
    if raw == "~" {
        return Ok(home_dir());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return Ok(home_dir().join(rest));
    }
    Ok(PathBuf::from(raw))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 截断摘要（字符级，保留完整性）
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundary() {
        let s = "中文内容".repeat(40);
        let t = truncate(&s, 100);
        assert!(t.chars().count() <= 101, "应截断到 100 字符 + 省略号");
        assert!(t.ends_with('…'));
    }

    #[test]
    fn expand_db_path_handles_tilde() {
        let p = expand_db_path("~/wiki.db").unwrap();
        assert!(p.ends_with("wiki.db"));
        let p2 = expand_db_path("/abs/path.db").unwrap();
        assert_eq!(p2, PathBuf::from("/abs/path.db"));
    }
}
