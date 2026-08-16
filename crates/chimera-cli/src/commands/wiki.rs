//! `chimera wiki <query>` — Wiki 语义检索,真实接入 L5 repo-wiki crate
//!
//! v2.9.0-omega Task 1.3:替换 NotImplemented 占位,真实调用 WikiStore。
//! P1-2 检索管道收敛:由 search_fulltext 升级为 hybrid_query(RRF 融合入口)。
//!
//! # 流程
//! 1. 从配置读取 `repo_wiki.db_path`,展开 `~` 为 home 目录
//! 2. 打开 WikiStore(SQLite 文件数据库,WAL 模式,自动建表)
//! 3. 调用 `hybrid_query(query, None, top_k)` 执行混合检索
//!    (CLI 无编码器注入,dense 通道退化 sparse-only;条目嵌入接入 NMC 后
//!    可在此处注入 TextPerceptor 启用语义召回)
//! 4. 截取 Top-N 结果(默认 10,`--limit` 控制)
//! 5. 输出:标题 + 相似度分数 + 摘要(人类可读)或 JSON 数组
//!
//! # 设计决策(WHY)
//! - **hybrid_query 而非 search_fulltext**:P1-2 检索能力收敛到 L5 单一融合入口,
//!   CLI 只需传 query 文本;dense 通道依赖条目 embedding 与调用方编码器,
//!   CLI 进程未装配 NMC 模型时自然退化为 sparse-only(行为等价于旧版 FTS5)。
//! - **相似度分数用 RRF 融合分数**:hybrid_query 返回 rrf_score(越高越相关),
//!   直接展示;sparse-only 模式下即由 sparse 排名决定。
//! - **摘要截取前 100 字符**:Wiki 条目内容可能很长,表格模式只显示摘要,
//!   完整内容用 `quest show` 或 TUI 查看。
//!
//! v2.9.0-omega Task 1.7:接受 `json` flag(JSON 数组 envelope 在本命令输出)

use std::path::PathBuf;

use anyhow::Result;
use repo_wiki::WikiStore;

use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;

/// 摘要最大字符数(人类可读模式表格中 content 列的截断长度)
const SUMMARY_MAX_CHARS: usize = 100;

/// 执行 wiki 查询命令 — 真实接入 repo-wiki 语义检索
///
/// `query` 为自然语言查询语句,`config` 提供数据库路径,
/// `json` flag 控制输出格式,`limit` 控制返回结果数(SubTask 1.3.3)。
pub async fn execute(query: &str, config: &ChimeraConfig, json: bool, limit: usize) -> Result<()> {
    tracing::info!(query = %query, limit, "Wiki 查询");

    // 1. 解析数据库路径(展开 ~ 为 home 目录)
    let db_path = expand_db_path(&config.repo_wiki.db_path)?;

    // 2. 打开 WikiStore(自动创建文件与 schema,启用 WAL)
    //    WHY spawn_blocking 不需要:WikiStore::open 是同步但快速(仅打开连接 + 建表),
    //    hybrid_query 内部已用 with_read_conn + spawn_blocking 包装异步执行。
    let store = WikiStore::open(&db_path)
        .map_err(|e| ChimeraCliError::EngineError(format!("WikiStore 打开失败: {e}")))?;

    // 3. 执行混合检索(P1-2:CLI 无编码器,sparse-only 退化模式)
    let results = store
        .hybrid_query(query, None, limit)
        .await
        .map_err(|e| ChimeraCliError::EngineError(format!("Wiki 检索失败: {e}")))?;

    // 4. 按融合排序回取条目(结果数 ≤ limit)
    let mut top_entries: Vec<_> = Vec::with_capacity(results.len());
    for r in results {
        if let Some(entry) = store
            .get(r.doc_id)
            .await
            .map_err(|e| ChimeraCliError::EngineError(format!("Wiki 条目读取失败: {e}")))?
        {
            top_entries.push(entry);
        }
    }

    // 5. 输出
    if json {
        // JSON 模式:输出 WikiEntry 数组 envelope
        // WHY 保留完整 entry(含 content):JSON 消费者(脚本)可能需要全文,
        // 截断会丢失信息。人类可读模式才截断摘要。
        output::print_json(&top_entries)?;
    } else if top_entries.is_empty() {
        output::print_info(&format!("未找到匹配「{query}」的 Wiki 条目"));
    } else {
        // 表格输出:序号 / 标题 / 相似度 / 摘要
        let rows: Vec<Vec<String>> = top_entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                // 相似度用排名近似:RRF 分数已编码在排序中,1/(rank+1)
                // 单调递减映射到 (0, 1] 便于用户理解
                let similarity = 1.0 / (i as f32 + 1.0);
                let summary = truncate_summary(&entry.content, SUMMARY_MAX_CHARS);
                vec![
                    (i + 1).to_string(),
                    entry.title.clone(),
                    format!("{:.2}", similarity),
                    summary,
                ]
            })
            .collect();
        output::print_table(&["#", "标题", "相似度", "摘要"], &rows);
    }

    Ok(())
}

/// 展开 `~` 为 home 目录,返回 PathBuf
///
/// WHY 独立函数:配置默认 `db_path: "~/.aether/wiki.db"` 含 `~`,
/// `WikiStore::open` 直接调用 `Connection::open` 不展开 `~`,
/// 会在当前目录创建 `~/.aether/wiki.db` 字面路径(非预期行为)。
fn expand_db_path(db_path: &str) -> Result<PathBuf, ChimeraCliError> {
    // WHY strip_prefix 而非 starts_with + 手动切片:clippy::manual_strip 建议,
    // strip_prefix 语义更清晰且编译器能保证切片安全。
    if let Some(rest) = db_path.strip_prefix('~') {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| {
                ChimeraCliError::ConfigError(format!(
                    "无法解析 home 目录(HOME/USERPROFILE 均未设置),db_path: {db_path}"
                ))
            })?;
        // 替换首个 `~` 为 home,保留后续路径
        let expanded = format!("{home}{rest}");
        Ok(PathBuf::from(expanded))
    } else {
        Ok(PathBuf::from(db_path))
    }
}

/// 截取摘要到指定字符数(按 Unicode 标量值计数,不截断多字节字符中间)
///
/// 超长时追加 `...` 表示截断,否则原样返回。
fn truncate_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let truncated: String = content.chars().take(max_chars).collect();
    format!("{truncated}...")
}
