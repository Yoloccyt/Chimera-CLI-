//! `aether wiki <query>` — Wiki 查询骨架
//!
//! 后续接入 `repo-wiki` crate 的语义检索(ISCM 跨层共享索引)。
//! 当前仅打印查询占位信息。

use anyhow::Result;

use crate::config::ChimeraConfig;

/// 执行 wiki 查询命令
pub async fn execute(query: &str, config: &ChimeraConfig) -> Result<()> {
    tracing::info!(query = %query, "Wiki 查询");
    println!("[wiki] 查询语句:{}", query);
    println!("[wiki] 知识库路径:{}", config.repo_wiki.db_path);
    println!("[wiki] 嵌入维度:{}", config.repo_wiki.embedding_dim);
    println!("[wiki] (骨架:待 repo-wiki crate 实现后接入,Week 2)");
    Ok(())
}
