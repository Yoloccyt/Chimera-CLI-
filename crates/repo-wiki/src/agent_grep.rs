//! Agent Grep — 面向 Agent 的结构化代码搜索(polish-v2.7 closure Stage B-9)
//!
//! 对应架构层: L5 Knowledge(repo-wiki 子模块)
//! 对应 ADR: ADR-049 决策 1(agent-grep 落点 repo-wiki)
//! 对应设计源: `chimera_ultimate_polish_v2.7.md` §3.2(jcode agent-grep:面向 Agent 的代码搜索)
//!
//! # 核心思想(jcode)
//!
//! Agent 的代码搜索与人类不同:人类要"匹配行列表",Agent 要**结构化、
//! 分层、带出处**的检索结果——知识层命中(Wiki 条目,回答"是什么/为什么")
//! 与代码层命中(代码单元,回答"在哪里改"),一次查询双通道返回。
//!
//! # 设计决策(WHY)
//!
//! - **组合而非新建检索**: 知识通道复用 `fts` 模块(FTS5 MATCH → LIKE 降级),
//!   代码通道复用 `behavior_localization::BehaviorLocalizer`(BGPD 三级披露),
//!   本模块零新增索引/零新增依赖——只做结果合成(ADR-049 "与 ISCM 检索融合"方向)
//! - **同步 rusqlite 签名**: 与 fts 模块一致,由调用方负责 `spawn_blocking`
//!   包装(§4.4 红线 2:rusqlite 不得在 async 上下文直接调用)
//! - **命中上限**: 双通道各截断 Top-N(默认 10),防止大库检索撑爆 Agent 上下文
//!   (Ω₁ Sparse:检索结果也要稀疏化)
//!
//! # 使用示例
//!
//! ```no_run
//! use repo_wiki::agent_grep::{AgentGrep, AgentGrepConfig};
//! use repo_wiki::behavior_localization::{BehaviorLocalizer, HarnessHandbook};
//! use rusqlite::Connection;
//!
//! let conn = Connection::open_in_memory().unwrap();
//! let localizer = BehaviorLocalizer::new(HarnessHandbook::default());
//! let grep = AgentGrep::new(localizer, AgentGrepConfig::default());
//!
//! // 调用方在 spawn_blocking 中执行(rusqlite 同步约束)
//! let report = grep.search(&conn, "verifier timeout").unwrap();
//! println!("knowledge={} code={}", report.knowledge_hits.len(), report.code_hits.len());
//! ```

use crate::behavior_localization::BehaviorLocalizer;
use crate::error::WikiError;
use crate::fts;
use crate::types::WikiEntry;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

// ============================================================
// 配置与结果类型
// ============================================================

/// 双通道各自的默认命中上限(Ω₁ Sparse:防检索结果撑爆 Agent 上下文)
pub const DEFAULT_MAX_HITS_PER_CHANNEL: usize = 10;

/// Agent Grep 配置
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentGrepConfig {
    /// 知识通道命中上限
    pub max_knowledge_hits: usize,
    /// 代码通道命中上限
    pub max_code_hits: usize,
}

impl Default for AgentGrepConfig {
    fn default() -> Self {
        Self {
            max_knowledge_hits: DEFAULT_MAX_HITS_PER_CHANNEL,
            max_code_hits: DEFAULT_MAX_HITS_PER_CHANNEL,
        }
    }
}

/// 知识通道命中 — Wiki 条目摘要(不携带全文,Agent 需要时按 entry_id 取)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeHit {
    /// 条目 ID(供二次取全文)
    pub entry_id: String,
    /// 条目标题
    pub title: String,
    /// 内容摘要(前 200 字符,Ω₂ Compress:命中列表只给摘要)
    pub snippet: String,
}

/// 代码通道命中 — BGPD 定位的代码单元
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeHit {
    /// 代码单元标识(如 "pvl-layer::verifier::verify")
    pub unit_id: String,
    /// 源文件路径
    pub file_path: String,
}

/// Agent Grep 结构化报告 — 双通道命中 + 查询回显
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentGrepReport {
    /// 原始查询(回显供 Agent 校对)
    pub query: String,
    /// 知识通道命中(Wiki 条目,"是什么/为什么")
    pub knowledge_hits: Vec<KnowledgeHit>,
    /// 代码通道命中(代码单元,"在哪里改")
    pub code_hits: Vec<CodeHit>,
    /// 知识通道是否走了 LIKE 降级(FTS5 不可用/语法错时 true,供诊断)
    pub knowledge_degraded: bool,
}

impl AgentGrepReport {
    /// 双通道是否均无命中
    pub fn is_empty(&self) -> bool {
        self.knowledge_hits.is_empty() && self.code_hits.is_empty()
    }
}

// ============================================================
// AgentGrep — 双通道检索合成器
// ============================================================

/// 摘要截断长度(字符数)
const SNIPPET_CHARS: usize = 200;

/// Agent Grep — 知识 + 代码双通道结构化搜索
///
/// 持有 `BehaviorLocalizer`(代码通道);知识通道每次查询传入 `Connection`
/// (与 fts 模块同款无状态连接使用方式,便于上层连接池管理)。
#[derive(Debug, Default)]
pub struct AgentGrep {
    localizer: BehaviorLocalizer,
    config: AgentGrepConfig,
}

impl AgentGrep {
    /// 创建 Agent Grep(注入行为定位器与配置)
    pub fn new(localizer: BehaviorLocalizer, config: AgentGrepConfig) -> Self {
        Self { localizer, config }
    }

    /// 双通道结构化搜索
    ///
    /// # 通道语义
    /// - **知识通道**: `fts::search_fts`(FTS5 MATCH);失败或空结果时降级
    ///   `fts::search_like`(LIKE 全表扫描),`knowledge_degraded` 标记降级
    /// - **代码通道**: `BehaviorLocalizer::localize`(BGPD 三级披露)
    ///
    /// # 同步约束(§4.4 红线 2)
    /// rusqlite 同步调用,async 上下文必须经 `spawn_blocking` 包装。
    ///
    /// # 错误
    /// - `WikiError`: 知识通道 SQL 执行失败(降级路径也失败时)
    pub fn search(&self, conn: &Connection, query: &str) -> Result<AgentGrepReport, WikiError> {
        // 知识通道:FTS5 优先,失败/空结果降级 LIKE
        // WHY 空结果也降级:FTS5 分词对中文/符号 query 可能零命中,
        // LIKE 子串匹配作为召回兜底(与 WikiStore::search_fulltext 同款语义)
        let (entries, degraded) = match fts::search_fts(conn, query) {
            Ok(hits) if !hits.is_empty() => (hits, false),
            Ok(_) => (fts::search_like(conn, query)?, true),
            Err(_) => (fts::search_like(conn, query)?, true),
        };
        let knowledge_hits = entries
            .iter()
            .take(self.config.max_knowledge_hits)
            .map(entry_to_hit)
            .collect();

        // 代码通道:BGPD 定位
        let code_hits = self
            .localizer
            .localize(query)
            .into_iter()
            .take(self.config.max_code_hits)
            .map(|unit| CodeHit {
                unit_id: unit.unit_id.clone(),
                file_path: unit.file_path.clone(),
            })
            .collect();

        Ok(AgentGrepReport {
            query: query.to_string(),
            knowledge_hits,
            code_hits,
            knowledge_degraded: degraded,
        })
    }
}

/// WikiEntry → KnowledgeHit 转换(内容截断为摘要)
fn entry_to_hit(entry: &WikiEntry) -> KnowledgeHit {
    // 按字符边界截断(直接字节切片会截断多字节 UTF-8 字符导致 panic)
    let snippet: String = entry.content.chars().take(SNIPPET_CHARS).collect();
    KnowledgeHit {
        entry_id: entry.entry_id.clone(),
        title: entry.title.clone(),
        snippet,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior_localization::{CodeUnit, ExecutionStage, HarnessHandbook};
    use crate::fts::init_fts_table;
    use std::collections::HashMap;

    /// 构建带 entries 表 + FTS5 索引的内存库(schema 与 store.rs 生产定义对齐)
    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (
                entry_id      TEXT PRIMARY KEY,
                title         TEXT NOT NULL,
                content       TEXT NOT NULL,
                tags          TEXT NOT NULL,
                embedding     BLOB NOT NULL,
                created_at    TEXT NOT NULL,
                updated_at    TEXT NOT NULL,
                temporal_meta TEXT
            );",
        )
        .unwrap();
        init_fts_table(&conn);
        conn
    }

    fn insert_entry(conn: &Connection, id: &str, title: &str, content: &str) {
        // 与 store.rs 生产 schema 对齐:embedding 为非 NULL BLOB(空向量即可,
        // row_to_entry 按 4 字节 f32 LE 块解码,空 BLOB 解码为空向量)
        conn.execute(
            "INSERT INTO entries (entry_id, title, content, tags, embedding, created_at, updated_at)
             VALUES (?1, ?2, ?3, '[]', ?4, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            rusqlite::params![id, title, content, Vec::<u8>::new()],
        )
        .unwrap();
        // 同步 FTS 索引:复用生产写路径公开 API(避免硬编码表名/列名漂移)
        let entry = WikiEntry::new(id, title, content, Vec::new(), Vec::new());
        fts::sync_fts_insert_new(conn, &entry).unwrap();
    }

    /// 构建含一个 verifier 阶段的 Handbook
    fn sample_localizer() -> BehaviorLocalizer {
        let mut units = HashMap::new();
        units.insert(
            "pvl::verify".to_string(),
            CodeUnit {
                unit_id: "pvl::verify".to_string(),
                file_path: "crates/pvl-layer/src/verifier.rs".to_string(),
            },
        );
        BehaviorLocalizer::new(HarnessHandbook {
            stages: vec![ExecutionStage {
                name: "verification".to_string(),
                keywords: vec!["verifier".to_string(), "verify".to_string()],
                code_units: vec!["pvl::verify".to_string()],
            }],
            units,
            call_edges: HashMap::new(),
        })
    }

    #[test]
    fn test_search_dual_channel_hits() {
        let conn = setup_conn();
        insert_entry(
            &conn,
            "e1",
            "verifier timeout root cause",
            "verifier timeout is caused by lock contention",
        );
        let grep = AgentGrep::new(sample_localizer(), AgentGrepConfig::default());
        let report = grep.search(&conn, "verifier timeout").unwrap();

        assert_eq!(report.knowledge_hits.len(), 1);
        assert_eq!(report.knowledge_hits[0].entry_id, "e1");
        assert_eq!(report.code_hits.len(), 1);
        assert_eq!(report.code_hits[0].unit_id, "pvl::verify");
        assert!(!report.is_empty());
    }

    #[test]
    fn test_search_no_hits_is_empty() {
        let conn = setup_conn();
        let grep = AgentGrep::new(
            BehaviorLocalizer::new(HarnessHandbook::default()),
            AgentGrepConfig::default(),
        );
        let report = grep.search(&conn, "nonexistent-topic-xyz").unwrap();
        assert!(report.is_empty());
    }

    #[test]
    fn test_search_respects_channel_limits() {
        let conn = setup_conn();
        for i in 0..15 {
            insert_entry(
                &conn,
                &format!("e{i}"),
                &format!("verifier doc {i}"),
                "verifier related content",
            );
        }
        let config = AgentGrepConfig {
            max_knowledge_hits: 5,
            max_code_hits: 5,
        };
        let grep = AgentGrep::new(sample_localizer(), config);
        let report = grep.search(&conn, "verifier").unwrap();
        assert_eq!(report.knowledge_hits.len(), 5);
    }

    #[test]
    fn test_snippet_truncated_at_char_boundary() {
        let conn = setup_conn();
        // 内容含多字节字符且远超 200 字符:截断必须按字符边界
        let long_content = "验".repeat(500) + " verifier";
        insert_entry(&conn, "e1", "verifier 中文文档", &long_content);
        let grep = AgentGrep::new(sample_localizer(), AgentGrepConfig::default());
        let report = grep.search(&conn, "verifier").unwrap();
        assert_eq!(report.knowledge_hits[0].snippet.chars().count(), 200);
    }

    #[test]
    fn test_query_echoed_in_report() {
        let conn = setup_conn();
        let grep = AgentGrep::new(sample_localizer(), AgentGrepConfig::default());
        let report = grep.search(&conn, "verify").unwrap();
        assert_eq!(report.query, "verify");
    }
}
