//! FTS5 全文索引 — 替代 LIKE 全表扫描,实现 O(log n) 全文检索
//!
//! 对应架构层:L5 Knowledge
//! 对应 Task 2 / N15: repo-wiki FTS5 全文索引(v1.2.0)
//! 对应 Task S3: FTS5 trigram tokenizer 升级(v1.3.0)
//!
//! # 设计要点
//! - **运行时检测 FTS5 tokenizer 能力**(v1.3.0 三级降级):
//!   `init_fts_table` 优先尝试 `trigram`(SQLite 3.34+),失败降级 `unicode61`,
//!   再失败标记 `Unavailable`。`FtsCapability` 三值枚举记录检测结果,
//!   后续 `search_fulltext` 据此选择最优查询路径。
//! - **standalone FTS5 虚拟表**:`entries_fts(entry_id UNINDEXED, title, content)`,
//!   与 `entries` 表通过 `entry_id` 关联,insert/delete 时显式同步
//! - **索引同步**:insert 时 DELETE+INSERT(FTS5 不支持 INSERT OR REPLACE),
//!   delete 时 DELETE,保证 entries 表与 FTS5 索引一致性
//! - **查询优先级**(v1.3.0 三级降级链):
//!   1. `AvailableTrigram` → trigram MATCH(直接命中 CJK 三字以上子串)
//!   2. `AvailableUnicode61` → unicode61 MATCH + 空结果降级 LIKE(CJK 子串不命中)
//!   3. `Unavailable` → LIKE 全表扫描
//! - **查询安全化**:`sanitize_fts5_query` 将用户输入转为安全的 FTS5 phrase 表达式,
//!   防止特殊字符触发 MATCH 语法错误
//!
//! # WHY trigram 而非 icu(v1.3.0 升级)
//! v1.2.0 使用 `unicode61` tokenizer,对 CJK 子串检索有固有局限:
//! 连续 CJK 字符被视为单 token,"性能分析报告" 是一个整体 token,
//! 子串 "分析" MATCH 不命中(v1.2.0 依赖 LIKE 降级保证召回)。
//!
//! v1.3.0 升级为 `trigram` tokenizer,将文本按 3 字符滑窗分词,
//! "性能分析报告" 会生成 "性能分"、"能分析"、"分析报"、"析报告" 等 trigram,
//! CJK 三字以上子串可直接 MATCH 命中,无需降级 LIKE。
//!
//! trigram vs icu 的选择:
//! - **trigram**:无 libicu 编译依赖(bundled SQLite 自带),跨平台一致行为,
//!   适合 CJK 三字以上子串检索;但对 < 3 字符查询无优势(无法生成 trigram token)。
//! - **icu**:需要 libicu 编译依赖(增加 binary 体积与构建复杂度),
//!   但分词更精细(支持中文分词边界检测)。
//!
//! 选择 trigram 是 trade-off:无 libicu 依赖(简化构建)+ CJK 三字以上子串检索
//! 改进(主要 use case),代价是 < 3 字符查询仍需 LIKE 降级(可接受,LIKE 对
//! 短查询性能足够,且子串匹配语义更宽松)。
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
//! trigram tokenizer 在 SQLite 3.34+(2021-12)引入,bundled SQLite 3.43+ 支持。
//! 但运行时检测仍保留,因为:
//! 1. 跨平台/非 bundled rusqlite 可能行为不同(旧版 SQLite 无 trigram)
//! 2. 已有数据库文件可能 schema 损坏(unicode61 表已存在,trigram 创建冲突)
//! 3. FTS5 虚拟表创建可能因磁盘/权限失败
//! 4. trigram 创建成功但 MATCH 不工作的边界(SQLite 编译选项差异)
//!
//! 运行时检测 + verify_match 是系统边界校验,符合"只在系统边界做校验"的约束。

use rusqlite::{params, Connection};

use crate::error::WikiError;
use crate::store::row_to_entry;
use crate::types::WikiEntry;

/// FTS5 虚拟表名 — `entries` 表的全文索引镜像
pub const FTS_TABLE: &str = "entries_fts";

/// FTS5 tokenizer 能力等级 — 运行时检测的结果(v1.3.0 三级降级链)
///
/// `Copy` 语义:在 `WikiStore` 中作为只读字段缓存,clone 时复制即可。
/// 运行时不变(检测一次后缓存),后续查询据此选择最优路径。
///
/// # 三级降级链
/// 1. `AvailableTrigram` — trigram tokenizer 可用(SQLite 3.34+),
///    CJK 三字以上子串 MATCH 直接命中,无需降级 LIKE
/// 2. `AvailableUnicode61` — 仅 unicode61 可用(trigram 创建失败或不工作),
///    CJK 子串检索需依赖空结果降级 LIKE(unicode61 将连续 CJK 视为单 token)
/// 3. `Unavailable` — FTS5 完全不可用,所有查询降级 LIKE
///
/// # WHY 三值而非二值(v1.2.0 升级)
/// v1.2.0 二值(`Available`/`Unavailable`)无法区分 trigram 与 unicode61,
/// 导致 trigram 不可用时仍需在每次查询时尝试 trigram MATCH 再降级(浪费一次
/// 失败查询)。三值在初始化时一次检测并缓存能力,后续查询直接走对应路径,
/// 避免运行时重复探测开销。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsCapability {
    /// trigram tokenizer 可用(SQLite 3.34+),CJK 三字以上子串 MATCH 直接命中。
    AvailableTrigram,
    /// 仅 unicode61 可用(trigram 创建失败或不工作),CJK 子串需空结果降级 LIKE。
    AvailableUnicode61,
    /// FTS5 完全不可用(扩展未编译或虚拟表创建失败),所有查询降级 LIKE。
    Unavailable,
}

impl FtsCapability {
    /// 是否可用 — 调用方据此选择 FTS5 或 LIKE 路径
    ///
    /// `AvailableTrigram` 和 `AvailableUnicode61` 都返回 `true`(FTS5 可用,
    /// 仅 tokenizer 不同);`Unavailable` 返回 `false`。
    ///
    /// WHY 保持 `is_available()` 兼容:v1.2.0 调用方(如 `writer_insert` /
    /// `writer_delete` 的 FTS5 索引同步)只关心 FTS5 是否可用,不关心 tokenizer
    /// 类型。三值改造后 `is_available()` 仍正确反映"FTS5 是否可同步索引"。
    pub fn is_available(self) -> bool {
        matches!(self, Self::AvailableTrigram | Self::AvailableUnicode61)
    }
}

/// 初始化 FTS5 虚拟表,返回 tokenizer 能力等级(v1.3.0 三级降级链)。
///
/// # 降级链
/// 1. 尝试创建 trigram 虚拟表 + `verify_trigram_match` 验证 MATCH 实际工作
/// 2. trigram 创建失败或 MATCH 不工作 → DROP 表 + 创建 unicode61 虚拟表
/// 3. unicode61 也失败 → `Unavailable`(所有查询降级 LIKE)
///
/// 每级创建成功后回填 `entries` 表已有数据到 FTS5 索引,
/// 保证重新打开已有数据库时检索完整性(旧库无 FTS5 表,首次启用需回填)。
///
/// # WHY verify_trigram_match 而非仅检查创建成功
/// SQLite 版本可能支持 trigram 创建(`CREATE VIRTUAL TABLE` 不报错)但
/// MATCH 实际不工作(编译选项差异、tokenizer 注册问题)。仅检查创建成功
/// 会导致 `AvailableTrigram` 标记错误,后续 CJK 子串查询空结果(因 MATCH
/// 不工作)而 v1.3.0 不会降级 LIKE(trigram 路径空结果直接返回)。
/// `verify_trigram_match` 插入测试数据 + 执行 MATCH + 清理,确保 trigram
/// 实际可用才标记 `AvailableTrigram`,否则降级 unicode61(空结果降级 LIKE)。
///
/// # WHY 回填用 NOT IN 而非全量重建
/// FTS5 不支持 `INSERT OR REPLACE`,全量重建需先清空再全量插入,开销大。
/// `WHERE entry_id NOT IN (SELECT entry_id FROM entries_fts)` 只插入缺失行,
/// 增量回填,适用于"已有数据的库首次启用 FTS5"场景。回填失败不阻断
/// (记录为 `_`),因为回填是补救措施,失败时新插入的文档仍会正常索引。
pub fn init_fts_table(conn: &Connection) -> FtsCapability {
    // 路径 1:尝试 trigram tokenizer
    if try_init_trigram(conn) {
        return FtsCapability::AvailableTrigram;
    }

    // 路径 2:trigram 失败,降级 unicode61
    if try_init_unicode61(conn) {
        return FtsCapability::AvailableUnicode61;
    }

    // 路径 3:都失败,Unavailable
    FtsCapability::Unavailable
}

/// 尝试初始化 trigram tokenizer 虚拟表并验证 MATCH 实际工作。
///
/// 成功条件(全部满足才返回 `true`):
/// 1. `CREATE VIRTUAL TABLE ... tokenize='trigram'` 执行成功
/// 2. `verify_trigram_match` 验证 trigram MATCH 实际命中测试数据
/// 3. 回填现有 `entries` 数据(失败不阻断,仅记 `_`)
///
/// 失败时清理已创建的表(DROP),避免残留导致后续 unicode61 创建冲突。
fn try_init_trigram(conn: &Connection) -> bool {
    let create_sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {FTS_TABLE} USING fts5(\
         entry_id UNINDEXED, title, content, tokenize='trigram');"
    );
    if conn.execute_batch(&create_sql).is_err() {
        // trigram 创建失败:SQLite 版本不支持 trigram tokenizer,或表已存在但 schema 冲突
        return false;
    }

    // 验证 trigram MATCH 实际工作(创建成功 ≠ 可用)
    if !verify_trigram_match(conn) {
        // WHY DROP 已创建的表:trigram 创建成功但 MATCH 不工作,若不 DROP,
        // 后续 unicode61 创建会因表已存在而失败(IF NOT EXISTS 跳过创建,
        // 但 tokenizer 仍是 trigram)。DROP 后 unicode61 可正常创建。
        let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS {FTS_TABLE};"));
        return false;
    }

    // 回填现有 entries 数据(增量,NOT IN 避免重复)
    let backfill = format!(
        "INSERT INTO {FTS_TABLE}(entry_id, title, content) \
         SELECT entry_id, title, content FROM entries \
         WHERE entry_id NOT IN (SELECT entry_id FROM {FTS_TABLE});"
    );
    // 回填失败不阻断初始化(回填是补救措施,失败时新插入仍正常索引)
    let _ = conn.execute_batch(&backfill);
    true
}

/// 尝试初始化 unicode61 tokenizer 虚拟表(v1.2.0 默认 tokenizer)。
///
/// 成功条件:
/// 1. `CREATE VIRTUAL TABLE ... tokenize='unicode61'` 执行成功
/// 2. 回填现有 `entries` 数据(失败不阻断)
///
/// WHY 不验证 unicode61 MATCH:unicode61 是 SQLite 3.0+ 默认 tokenizer,
/// 创建成功即可用(无 trigram 那种"创建成功但 MATCH 不工作"的边界)。
fn try_init_unicode61(conn: &Connection) -> bool {
    let create_sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {FTS_TABLE} USING fts5(\
         entry_id UNINDEXED, title, content, tokenize='unicode61');"
    );
    if conn.execute_batch(&create_sql).is_err() {
        return false;
    }

    // 回填现有 entries 数据(增量,NOT IN 避免重复)
    let backfill = format!(
        "INSERT INTO {FTS_TABLE}(entry_id, title, content) \
         SELECT entry_id, title, content FROM entries \
         WHERE entry_id NOT IN (SELECT entry_id FROM {FTS_TABLE});"
    );
    let _ = conn.execute_batch(&backfill);
    true
}

/// 验证 trigram tokenizer 实际工作(创建成功 ≠ 可用)。
///
/// 流程:
/// 1. 清理可能残留的测试行(幂等,保证 INSERT 不冲突)
/// 2. 插入测试数据(entry_id=`__trigram_probe__`,content="性能分析报告")
/// 3. 执行 MATCH "分析报告"(4 字 CJK 子串,trigram 应生成 "分析报" + "析报告")
/// 4. 清理测试行(无论 SELECT 成功失败都清理)
///
/// 返回 `true` 表示 MATCH 命中(trigram 工作);`false` 表示 MATCH 报错或
/// 返回 0 行(trigram 不工作,需降级 unicode61)。
///
/// # WHY 测试数据用 "性能分析报告"
/// 4 字 CJK 字符串,生成的 trigram("性能分"、"能分析"、"分析报"、"析报告")
/// 覆盖典型 CJK 子串检索场景。查询 "分析报告" 是其 4 字子串,trigram 应命中。
/// 若用 unicode61 tokenizer(误标记为 trigram),"分析报告" 会被视为单 token,
/// 不匹配 "性能分析报告" 整体 token,MATCH 返回 0 行 → verify 返回 false。
///
/// # WHY 残留测试行不影响生产数据
/// 测试行 `entry_id = "__trigram_probe__"` 不在 `entries` 表中,`search_fts`
/// 通过 `JOIN entries e ON e.entry_id = f.entry_id` 取回完整 `WikiEntry`,
/// 测试行不满足 JOIN 条件,不会出现在任何查询结果中。即便清理失败残留,
/// 也不影响生产查询语义(仅占用极小索引空间)。
fn verify_trigram_match(conn: &Connection) -> bool {
    const TEST_ID: &str = "__trigram_probe__";
    const TEST_TEXT: &str = "性能分析报告";

    // 步骤 1:清理可能残留的测试行(幂等)
    let _ = conn.execute(
        &format!("DELETE FROM {FTS_TABLE} WHERE entry_id = ?1;"),
        params![TEST_ID],
    );

    // 步骤 2:插入测试数据
    let insert_ok = conn
        .execute(
            &format!("INSERT INTO {FTS_TABLE}(entry_id, title, content) VALUES (?1, ?2, ?3);"),
            params![TEST_ID, TEST_TEXT, TEST_TEXT],
        )
        .is_ok();
    if !insert_ok {
        return false;
    }

    // 步骤 3:执行 trigram MATCH — "分析报告" 是 TEST_TEXT 的 4 字 CJK 子串
    // trigram 应生成 "分析报" + "析报告" 两个 trigram,均匹配 TEST_TEXT
    let result: rusqlite::Result<i64> = conn.query_row(
        &format!("SELECT 1 FROM {FTS_TABLE} WHERE {FTS_TABLE} MATCH ?1 LIMIT 1;"),
        params!["分析报告"],
        |row| row.get(0),
    );

    // 步骤 4:清理测试行(无论 SELECT 成功失败都清理)
    let _ = conn.execute(
        &format!("DELETE FROM {FTS_TABLE} WHERE entry_id = ?1;"),
        params![TEST_ID],
    );

    // query_row 返回 Ok 表示命中 1 行(trigram 工作);Err(包括 QueryReturnedNoRows
    // 或 SQL 语法错误)表示 trigram 不工作或不命中
    result.is_ok()
}

/// 同步写入 FTS5 索引(insert/update 时调用)。
///
/// WHY:FTS5 不支持 `INSERT OR REPLACE` 语义(无 PRIMARY KEY 约束),
/// 采用 DELETE + INSERT 保证 UPSERT 幂等性 — 先删除旧索引行再插入新行,
/// 避免重复条目导致 MATCH 返回冗余结果。
///
/// 注:trigram 与 unicode61 tokenizer 的 INSERT/DELETE 语法相同(SQL 层面透明),
/// 此函数无需根据 `FtsCapability` 分流。
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
///
/// # trigram 与 unicode61 兼容性
/// phrase 包裹对两种 tokenizer 都有效:
/// - unicode61:`"分析报告"` 是单 phrase(整体 token 匹配)
/// - trigram:`"分析报告"` 被 trigram 分词为 "分析报" + "析报告"(隐式 AND)
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
///
/// # WHY 不在此函数分流 tokenizer
/// `search_fts` 是底层 MATCH 查询,trigram 与 unicode61 的 SQL 完全相同
/// (tokenizer 在虚拟表创建时固定,查询时透明)。tokenizer 分流在
/// `WikiStore::search_fulltext` 上层处理(根据 `FtsCapability` 决定是否
/// 调用 `search_fts` 以及空结果是否降级 LIKE)。
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
        // v1.3.0:AvailableTrigram 和 AvailableUnicode61 都 is_available == true
        assert!(FtsCapability::AvailableTrigram.is_available());
        assert!(FtsCapability::AvailableUnicode61.is_available());
        assert!(!FtsCapability::Unavailable.is_available());
        // Copy 语义:赋值不丢失原值
        let a = FtsCapability::AvailableTrigram;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn test_fts_capability_equality() {
        // 三值枚举的相等性(v1.3.0 三级降级链区分)
        assert_eq!(
            FtsCapability::AvailableTrigram,
            FtsCapability::AvailableTrigram
        );
        assert_eq!(
            FtsCapability::AvailableUnicode61,
            FtsCapability::AvailableUnicode61
        );
        assert_ne!(
            FtsCapability::AvailableTrigram,
            FtsCapability::AvailableUnicode61
        );
        assert_ne!(FtsCapability::AvailableTrigram, FtsCapability::Unavailable);
        assert_ne!(
            FtsCapability::AvailableUnicode61,
            FtsCapability::Unavailable
        );
    }
}
