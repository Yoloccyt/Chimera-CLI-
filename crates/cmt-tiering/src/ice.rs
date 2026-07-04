//! Ice 层 — 归档只读文件存储
//!
//! 对应架构层:L3 Storage(Ice tier)
//!
//! # 设计决策(WHY)
//! - **文件存储而非 SQLite**:Ice 层是归档层,访问频率极低(延迟 < 500ms),
//!   文件存储足够满足需求,且避免 SQLite 连接开销
//! - **每个能力一个 `.bin` 文件**:路径形如 `<ice_dir>/<cap_id>.bin`,
//!   简单直观,便于备份与迁移(直接复制文件)
//! - **JSON 序列化**:使用 `serde_json` 序列化 `CapabilityEntry`,
//!   可读性与兼容性好(未来可升级为 MessagePack 提升性能)
//! - **spawn_blocking 包装文件 I/O**:文件读写可能阻塞异步运行时,
//!   使用 `tokio::task::spawn_blocking` 放到阻塞线程池
//! - **archive 语义而非 insert**:Ice 层是归档层,使用 `archive` 方法名
//!   更准确地表达"归档"语义(与 Hot/Warm/Cold 的 insert 区分)
//! - **无容量上限**:Ice 层是最终归档层,无容量限制(磁盘空间是唯一约束)
//!
//! # 文件格式
//! ```text
//! <ice_dir>/<cap_id>.bin
//!   内容:serde_json 序列化的 CapabilityEntry(JSON 格式)
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::task::spawn_blocking;
use tracing::{debug, trace};

use crate::error::CmtError;
use crate::types::{CapabilityEntry, CapabilityId, Tier};

/// Ice 层 — 归档只读文件存储
///
/// 每个能力存储为一个 `.bin` 文件,路径形如 `<ice_dir>/<cap_id>.bin`。
/// 所有 async 方法通过 `spawn_blocking` 在阻塞线程池中执行文件 I/O。
///
/// # 线程安全
/// `Arc<PathBuf>` 包装目录路径(不可变,无需 Mutex)。
/// 文件操作由文件系统保证原子性(同一文件不会并发写入)。
#[derive(Clone)]
pub struct IceTier {
    /// Ice 层目录路径(Arc 共享,Clone 廉价)
    ice_dir: Arc<PathBuf>,
}

impl IceTier {
    /// 创建 Ice 层,指定目录路径
    ///
    /// 路径的父目录应已存在(调用方负责创建)。
    pub fn new(ice_dir: impl Into<PathBuf>) -> Self {
        Self {
            ice_dir: Arc::new(ice_dir.into()),
        }
    }

    /// 返回 Ice 层目录路径
    pub fn dir(&self) -> &Path {
        &self.ice_dir
    }

    /// 构造指定能力 ID 的文件路径
    ///
    /// 路径形如 `<ice_dir>/<cap_id>.bin`
    fn file_path(&self, cap_id: &str) -> PathBuf {
        self.ice_dir.join(format!("{cap_id}.bin"))
    }

    /// 异步归档能力条目到 Ice 层
    ///
    /// 将条目序列化为 JSON 并写入 `<ice_dir>/<cap_id>.bin`。
    /// 若文件已存在,覆盖写入(UPSERT 语义)。
    pub async fn archive(&self, mut entry: CapabilityEntry) -> Result<(), CmtError> {
        // 强制设置 tier 为 Ice(防止上层传入错误层级)
        entry.tier = Tier::Ice;

        let file_path = self.file_path(&entry.id);
        let ice_dir = self.ice_dir.clone();

        spawn_blocking(move || {
            // 确保目录存在(幂等操作)
            // WHY:使用 &*ice_dir 解引用 Arc<PathBuf> 为 &Path,
            // 因为 std::fs::create_dir_all 要求 P: AsRef<Path>,
            // 而 &Arc<PathBuf> 不直接实现 AsRef<Path>
            std::fs::create_dir_all(&*ice_dir).map_err(|e| {
                CmtError::StorageError(format!(
                    "创建 Ice 层目录失败: {} - {}",
                    ice_dir.display(),
                    e
                ))
            })?;

            // 序列化为 JSON
            let json = serde_json::to_vec(&entry)?;

            // 写入文件(原子写入:先写临时文件,再重命名)
            // WHY:直接写入可能导致部分写入,读取时 JSON 解析失败;
            // 先写临时文件再重命名,保证文件内容完整
            let tmp_path = file_path.with_extension("bin.tmp");
            std::fs::write(&tmp_path, &json)?;
            std::fs::rename(&tmp_path, &file_path)?;

            debug!(cap_id = %entry.id, file = ?file_path, "Ice 层条目已归档");
            Ok(())
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步获取能力条目(不更新访问时间)
    ///
    /// WHY 不更新访问时间:Ice 层是归档层,条目被访问时通过 `promote_to_hot`
    /// 提升到 Hot 层,而非在 Ice 层更新访问时间。Ice 层保持只读语义。
    ///
    /// 返回条目克隆;若不存在返回 None。
    pub async fn get(&self, cap_id: String) -> Result<Option<CapabilityEntry>, CmtError> {
        let file_path = self.file_path(&cap_id);

        spawn_blocking(move || {
            if !file_path.exists() {
                return Ok(None);
            }

            let bytes = std::fs::read(&file_path)?;
            let entry: CapabilityEntry = serde_json::from_slice(&bytes)?;

            trace!(cap_id = %entry.id, "Ice 层条目已读取");
            Ok(Some(entry))
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步列出所有归档条目的 ID
    ///
    /// 扫描 Ice 层目录,返回所有 `.bin` 文件的文件名(去掉 `.bin` 后缀)。
    pub async fn list(&self) -> Result<Vec<String>, CmtError> {
        let ice_dir = self.ice_dir.clone();

        spawn_blocking(move || {
            if !ice_dir.exists() {
                return Ok(Vec::new());
            }

            let mut ids = Vec::new();
            // WHY:使用 &*ice_dir 解引用 Arc<PathBuf> 为 &Path
            for entry in std::fs::read_dir(&*ice_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        ids.push(stem.to_string());
                    }
                }
            }

            trace!(count = ids.len(), "Ice 层条目列表已获取");
            Ok(ids)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步删除指定条目
    ///
    /// 返回是否删除成功(若文件不存在返回 false)
    ///
    /// WHY 接受 `impl Into<CapabilityId>`:类型安全,与 Warm/Cold 层 delete 签名保持一致,
    /// 调用方可传 `CapabilityId`/`String`/`&str`
    pub async fn delete(&self, cap_id: impl Into<CapabilityId>) -> Result<bool, CmtError> {
        let cap_id = cap_id.into();
        let file_path = self.file_path(cap_id.as_str());

        spawn_blocking(move || {
            if !file_path.exists() {
                return Ok(false);
            }

            std::fs::remove_file(&file_path)?;
            debug!(cap_id = %cap_id, "Ice 层条目已删除");
            Ok(true)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步列出所有归档条目(完整数据,用于迁移或快照)
    pub async fn list_all(&self) -> Result<Vec<CapabilityEntry>, CmtError> {
        let ice_dir = self.ice_dir.clone();

        spawn_blocking(move || {
            if !ice_dir.exists() {
                return Ok(Vec::new());
            }

            let mut entries = Vec::new();
            // WHY:使用 &*ice_dir 解引用 Arc<PathBuf> 为 &Path
            for entry in std::fs::read_dir(&*ice_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("bin") {
                    continue;
                }

                let bytes = std::fs::read(&path)?;
                let cap_entry: CapabilityEntry = serde_json::from_slice(&bytes)?;
                entries.push(cap_entry);
            }

            Ok(entries)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str) -> CapabilityEntry {
        CapabilityEntry::new(id, format!("content-{id}"), Tier::Ice)
    }

    #[tokio::test]
    async fn test_archive_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let ice = IceTier::new(tmp.path());

        let entry = make_entry("cap-1");
        ice.archive(entry).await.unwrap();

        let fetched = ice.get("cap-1".to_string()).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id.as_str(), "cap-1");
        assert_eq!(fetched.content, "content-cap-1");
        assert_eq!(fetched.tier, Tier::Ice);
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let ice = IceTier::new(tmp.path());

        let result = ice.get("nonexistent".to_string()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_archive_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let ice = IceTier::new(tmp.path());

        // 归档第一个版本
        ice.archive(make_entry("cap-1")).await.unwrap();

        // 用相同 ID 但不同内容归档,应覆盖
        let mut entry2 = make_entry("cap-1");
        entry2.content = "updated-content".to_string();
        ice.archive(entry2).await.unwrap();

        let fetched = ice.get("cap-1".to_string()).await.unwrap().unwrap();
        assert_eq!(fetched.content, "updated-content");
    }

    #[tokio::test]
    async fn test_list() {
        let tmp = tempfile::tempdir().unwrap();
        let ice = IceTier::new(tmp.path());

        ice.archive(make_entry("cap-1")).await.unwrap();
        ice.archive(make_entry("cap-2")).await.unwrap();
        ice.archive(make_entry("cap-3")).await.unwrap();

        let mut ids = ice.list().await.unwrap();
        ids.sort();
        assert_eq!(ids, vec!["cap-1", "cap-2", "cap-3"]);
    }

    #[tokio::test]
    async fn test_list_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let ice = IceTier::new(tmp.path());

        let ids = ice.list().await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn test_list_nonexistent_dir() {
        // 目录不存在时应返回空列表(而非错误)
        let ice = IceTier::new("/nonexistent/path/that/does/not/exist");
        let ids = ice.list().await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn test_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let ice = IceTier::new(tmp.path());

        ice.archive(make_entry("cap-1")).await.unwrap();
        let deleted = ice.delete("cap-1".to_string()).await.unwrap();
        assert!(deleted);

        // 删除后应获取不到
        let fetched = ice.get("cap-1".to_string()).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let ice = IceTier::new(tmp.path());

        let deleted = ice.delete("nonexistent".to_string()).await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_list_all() {
        let tmp = tempfile::tempdir().unwrap();
        let ice = IceTier::new(tmp.path());

        ice.archive(make_entry("cap-1")).await.unwrap();
        ice.archive(make_entry("cap-2")).await.unwrap();

        let all = ice.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_persistence_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();

        // 写入数据
        {
            let ice = IceTier::new(tmp.path());
            ice.archive(make_entry("cap-1")).await.unwrap();
        }

        // 重新打开并验证
        {
            let ice = IceTier::new(tmp.path());
            let fetched = ice.get("cap-1".to_string()).await.unwrap().unwrap();
            assert_eq!(fetched.id.as_str(), "cap-1");
            assert_eq!(fetched.content, "content-cap-1");
        }
    }
}
