//! FTS5 全文索引 — 替代 LIKE 全表扫描,实现 O(log n) 全文检索
//!
//! 对应架构层:L5 Knowledge
//! 对应 Task 2 / N15: repo-wiki FTS5 全文索引
//!
//! # 设计要点
//! - **运行时检测 FTS5 可用性**:`init_fts_table` 尝试创建虚拟表,失败则标记
//!   `Unavailable`,不中断初始化,后续查询走 LIKE 降级
//! - **standalone FTS5 虚拟表**:`entries_fts(entry_id UNINDEXED, title, content)`,
//!   与 `entries` 表通过 `entry_id` 关联,insert/delete 时显式同步
//! - **索引同步**:insert 时 DELETE+INSERT(FTS5 不支持 INSERT OR REPLACE),
//!   delete 时 DELETE,保证 entries 表与 FTS5 索引一致性
//! - **查询优先级**:`search_fts` 用 MATCH;`search_like` 为降级路径;
//!   `WikiStore::search_fulltext` 优先 FTS5,失败降级 LIKE
//! - **查询安全化**:`sanitize_fts5_query` 将用户输入转为安全的 FTS5 phrase 表达式,
//!   防止特殊字符触发 MATCH 语法错误
//!
//! # WHY standalone 而非 external content
//! external content 模式(`content='entries'`)需配合触发器同步,逻辑复杂且
//! FTS5 external content 的 DELETE 语义在 UPSERT 场景下易出错。standalone 模式
//! 虽多存一份文本(FTS5 倒排索引体积约为原文 50%),但同步逻辑清晰可控,
//! 在 1000+ 文档规模下存储开销可接受,换取的是代码可维护性与正确性。
//!
//! # WHY entry_id UNINDEXED
//! `entry_id` 仅用于 JOIN 关联和 DELETE WHERE 同步,不参与全文检索。
//! `UNINDEXED` 标记使该列不进入倒排索引,节省索引体积与写入开销,
//! 同时仍可作为普通列读取(JOIN/DELETE 可用)。
//!
//! # WHY 运行时检测而非编译时假设
//! `libsqlite3-sys 0.30.1` bundled 的 `build.rs` 硬编码 `-DSQLITE_ENABLE_FTS5`
//! (见 libsqlite3-sys 源码 build.rs:129),FTS5 在当前编译中默认可用。
//! 但运行时检测仍保留,因为:
//! 1. 跨平台/非 bundled rusqlite 可能行为不同
//! 2. 已有数据库文件可能 schema 损坏
//! 3. FTS5 虚拟表创建可能因磁盘/权限失败
//!
//! 运行时检测是系统边界校验,符合"只在系统边界做校验"的约束。

use rusqlite::{params, Connection};

use crate::error::WikiError;
use crate::store::row_to_entry;
use crate::types::WikiEntry;

/// FTS5 虚拟表名 — `entries` 表的全文索引镜像
pub const FTS_TABLE: &str = "entries_fts";

/// FTS5 可用性状态 — 运行时检测的结果
///
/// `Copy` 语义:在 `WikiStore` 中作为只读字段缓存,clone 时复制即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsCapability {
    /// FTS5 扩展可用,`entries_fts` 虚拟表已就绪,可走 MATCH 路径
    Available,
    /// FTS5 不可用(扩展未编译或虚拟表创建失败),降级到 LIKE
    Unavailable,
}

impl FtsCapability {
    /// 是否可用 — 调用方据此选择 FTS5 或 LIKE 路径
    pub fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }
}

/// 尝试创建 FTS5 虚拟表并返回可用性。
///
/// 成功创建返回 `Available`;若 SQLite 未编译 FTS5 扩展
/// (`CREATE VIRTUAL TABLE` 失败),捕获错误返回 `Unavailable`,
/// 不中断初始化流程。
///
/// 创建后自动回填 `entries` 表已有数据到 FTS5 索引,
/// 保证重新打开已有数据库时检索完整性(旧库无 FTS5 表,首次启用需回填)。
///
/// # WHY 回填用 NOT IN 而非全量重建
/// FTS5 不支持 `INSERT OR REPLACE`,全量重建需先清空再全量插入,开销大。
/// `WHERE entry_id NOT IN (SELECT entry_id FROM entries_fts)` 只插入缺失行,
/// 增量回填,适用于"已有数据的库首次启用 FTS5"场景。回填失败不阻断
/// (记录为 `_`),因为回填是补救措施,失败时新插入的文档仍会正常索引。
pub fn init_fts_table(conn: &Connection) -> FtsCapability {
    let create_sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {FTS_TABLE} USING fts5(\
         entry_id UNINDEXED, title, content, tokenize='unicode61');"
    );
    match conn.execute_batch(&create_sql) {
        Ok(()) => {
            // WHY:重新打开已有数据库时,若 FTS5 表为空(首次启用)或部分缺失,
            // 需将现有 entries 数据回填到 FTS5 索引,保证检索完整性。
            // 使用 NOT IN 避免重复插入(FTS5 不支持 INSERT OR REPLACE)。
            let backfill = format!(
                "INSERT INTO {FTS_TABLE}(entry_id, title, content) \
                 SELECT entry_id, title, content FROM entries \
                 WHERE entry_id NOT IN (SELECT entry_id FROM {FTS_TABLE});"
            );
            // 回填失败不阻断初始化(回填是补救措施,失败时新插入仍正常索引)
            let _ = conn.execute_batch(&backfill);
            FtsCapability::Available
        }
        // WHY:FTS5 不可用时静默降级,不返回错误 — 保证功能可用性优先于性能
        Err(_) => FtsCapability::Unavailable,
    }
}

/// 同步写入 FTS5 索引(insert/update 时调用)。
///
/// WHY:FTS5 不支持 `INSERT OR REPLACE` 语义(无 PRIMARY KEY 约束),
/// 采用 DELETE + INSERT 保证 UPSERT 幂等性 — 先删除旧索引行再插入新行,
/// 避免重复条目导致 MATCH 返回冗余结果。
pub fn sync_fts_insert(conn: &Connection, entry: &WikiEntry) -> Result<(), WikiError> {
    // 先删除可能存在的旧索引(幂等:无匹配行时 DELETE 不报错)
    conn.execute(
        &format!("DELETE FROM {FTS_TABLE} WHERE entry_id = ?1;"),
        params![entry.entry_id],
    )?;
    // 插入新索引
    conn.execute(
        &format!("INSERT INTO {FTS_TABLE}(entry_id, title, content) VALUES (?1, ?2, ?3);"),
        params![entry.entry_id, entry.title, entry.content],
    )?;
    Ok(())
}

/// 同步删除 FTS5 索引(delete 时调用)。
///
/// 幂等:无匹配行时 DELETE 不报错。
pub fn sync_fts_delete(conn: &Connection, entry_id: &str) -> Result<(), WikiError> {
    conn.execute(
        &format!("DELETE FROM {FTS_TABLE} WHERE entry_id = ?1;"),
        params![entry_id],
    )?;
    Ok(())
}

/// 安全化 FTS5 MATCH 查询表达式。
///
/// WHY:FTS5 MATCH 语法对特殊字符(`*`, `"`, `:`, `(`, `)`)敏感,
/// 直接透传用户输入可能触发 SQL 语法错误导致查询失败。将每个空白分隔的
/// token 用双引号包裹转为 phrase term(字面量,FTS5 不解析其内特殊字符),
/// 多 token 之间用空格连接(FTS5 隐式 AND),兼顾安全性与召回率。
///
/// # 转义规则
/// - 移除 token 内部的双引号(防止提前闭合 phrase)
/// - 每个 token 包裹为 `"token"`(phrase,字面量匹配)
/// - 多 token 用空格连接(FTS5 隐式 AND 语义)
/// - 空输入或纯空白返回空字符串(调用方应据此跳过 MATCH)
pub(crate) fn sanitize_fts5_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| token.replace('"', ""))
        .filter(|t| !t.is_empty())
        .map(|token| format!("\"{token}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

/// FTS5 MATCH 查询 — 通过 `entry_id` JOIN `entries` 表取回完整 `WikiEntry`。
///
/// # 返回
/// 匹配的 `WikiEntry` 列表,按 FTS5 相关度(`rank`)升序(最相关在前)。
///
/// # 空查询处理
/// `sanitize_fts5_query` 返回空字符串(无有效 token)时,直接返回空 Vec,
/// 不执行 MATCH(避免 FTS5 空 query 报错)。
///
/// # WHY JOIN 而非两次查询
/// FTS5 表只存索引文本(title/content),完整 `WikiEntry` 字段(tags/embedding/
/// 时间戳)在 `entries` 表。通过 `entry_id` JOIN 一次性取回完整数据,避免
/// "先查 FTS5 拿 id 列表再逐条查 entries"的 N+1 查询问题。
pub fn search_fts(conn: &Connection, query: &str) -> Result<Vec<WikiEntry>, WikiError> {
    let sanitized = sanitize_fts5_query(query);
    // 空查询(无有效 token)直接返回空,避免 MATCH 报错
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT e.entry_id, e.title, e.content, e.tags, e.embedding, e.created_at, e.updated_at
         FROM entries e
         JOIN {FTS_TABLE} f ON e.entry_id = f.entry_id
         WHERE {FTS_TABLE} MATCH ?1
         ORDER BY rank;"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![sanitized], row_to_entry)?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// LIKE 全表扫描查询(降级路径)— 保持与原 `search_fulltext` 等价语义。
///
/// 大小写不敏感(SQLite LIKE 默认对 ASCII 不敏感)。在 FTS5 不可用或
/// query 触发 FTS5 语法错误时使用,保证功能可用。
pub fn search_like(conn: &Connection, query: &str) -> Result<Vec<WikiEntry>, WikiError> {
    let pattern = format!("%{query}%");
    let mut stmt = conn.prepare(
        "SELECT entry_id, title, content, tags, embedding, created_at, updated_at
         FROM entries
         WHERE title LIKE ?1 OR content LIKE ?1;",
    )?;
    let rows = stmt.query_map(params![pattern], row_to_entry)?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_simple_query() {
        // 多 token 用空格连接,每个包裹为 phrase(FTS5 隐式 AND)
        let result = sanitize_fts5_query("rust async");
        assert_eq!(result, "\"rust\" \"async\"");
    }

    #[test]
    fn test_sanitize_strips_double_quotes() {
        // 用户输入含双引号时应被移除,防止 FTS5 phrase 提前闭合
        // 每个 token 独立包裹为 phrase,避免 OR 被解析为操作符
        let result = sanitize_fts5_query("rust\" OR \"1");
        assert_eq!(result, "\"rust\" \"OR\" \"1\"");
    }

    #[test]
    fn test_sanitize_empty_query() {
        let result = sanitize_fts5_query("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_sanitize_whitespace_only() {
        let result = sanitize_fts5_query("   \t\n  ");
        assert!(result.is_empty());
    }

    #[test]
    fn test_sanitize_single_token() {
        let result = sanitize_fts5_query("rust");
        assert_eq!(result, "\"rust\"");
    }

    #[test]
    fn test_sanitize_chinese_token() {
        // unicode61 tokenizer 对中文按字符分词,phrase 包裹仍有效
        let result = sanitize_fts5_query("索引 优化");
        assert_eq!(result, "\"索引\" \"优化\"");
    }

    #[test]
    fn test_fts_capability_is_available() {
        assert!(FtsCapability::Available.is_available());
        assert!(!FtsCapability::Unavailable.is_available());
        // Copy 语义:赋值不丢失原值
        let a = FtsCapability::Available;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn test_fts_capability_equality() {
        assert_eq!(FtsCapability::Available, FtsCapability::Available);
        assert_ne!(FtsCapability::Available, FtsCapability::Unavailable);
    }
}
