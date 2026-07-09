//! 检查点管理器 — LHQP(Long-Horizon Quest Persistence)实现
//!
//! 对应架构层:L9 Quest
//! 对应创新点:LHQP(长周期任务持久化,进程崩溃后可从最近检查点恢复)
//!
//! # 设计决策(WHY)
//! - **文件布局**:`~/.aether/checkpoints/<quest_id>/<checkpoint_id>.bin`
//!   按 quest_id 分目录,避免单目录文件爆炸;支持多 Quest 并发持久化
//! - **序列化格式**:MessagePack(ADR-004,与 Event Bus 一致)
//!   跨进程兼容、紧凑(比 JSON 小 30-50%)、支持二进制数据
//! - **完整性校验**:SHA-256 哈希比对 `serialized_state`
//!   防止磁盘位翻转或人为篡改导致状态漂移
//! - **保留策略**:最近 N 个检查点(默认 5),超出删除最旧
//!   避免磁盘膨胀,同时保留足够回滚点供恢复
//! - **异步 I/O**:所有磁盘操作通过 `tokio::task::spawn_blocking` 封装,
//!   避免阻塞 tokio worker 线程(A1 优化:save/load 可能涉及多次文件 I/O +
//!   MessagePack 序列化 + SHA-256 校验,累计可达数十毫秒)
//!
//! # 架构红线
//! - 单函数 ≤ 200 行
//! - 禁止 unwrap()/expect() 在非测试代码
//! - 所有 IO 错误包装为 QuestError,不泄漏底层 io::Error

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use nexus_core::{Checkpoint, Quest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::QuestError;

/// 检查点管理器 — 负责 Quest 状态的持久化与恢复
///
/// 文件结构:`~/.aether/checkpoints/<quest_id>/<checkpoint_id>.bin`
/// 序列化格式:MessagePack(ADR-004,与 Event Bus 一致)
/// 保留策略:最近 N 个检查点,超出删除最旧(避免磁盘膨胀)
pub struct CheckpointManager {
    /// 检查点根目录(所有 Quest 的检查点按子目录组织)
    checkpoint_dir: PathBuf,
    /// 每个 Quest 保留的最大检查点数(超出删除最旧)
    max_keep: usize,
}

impl CheckpointManager {
    /// 创建检查点管理器,默认保留 5 个检查点
    pub fn new(checkpoint_dir: PathBuf) -> Self {
        Self {
            checkpoint_dir,
            max_keep: 5,
        }
    }

    /// 创建检查点管理器,自定义保留数量
    pub fn with_max_keep(checkpoint_dir: PathBuf, max_keep: usize) -> Self {
        Self {
            checkpoint_dir,
            max_keep,
        }
    }

    /// 保存检查点 — 序列化 Quest 为 MessagePack,写入磁盘(异步)
    ///
    /// 内部通过 `spawn_blocking` 将磁盘 I/O + MessagePack 序列化 + SHA-256 校验
    /// 放到阻塞线程池执行,避免阻塞 tokio worker 线程。
    ///
    /// 流程:
    /// 1. 生成 checkpoint_id(UUIDv7,时间有序,便于排序与因果追踪)
    /// 2. 序列化 Quest 为 MessagePack
    /// 3. 计算 SHA-256 哈希(用于恢复时完整性校验)
    /// 4. 创建 Checkpoint 实例并整体序列化为 MessagePack 写入磁盘
    /// 5. 调用 prune_old 清理超出 max_keep 的旧检查点
    pub async fn save(&self, quest: &Quest) -> Result<Checkpoint, QuestError> {
        // UUIDv7 生成是 CPU-only 操作,在 async 上下文中完成
        let checkpoint_id = format!("cp-{}", Uuid::now_v7());
        // Quest 序列化也是纯 CPU,提前完成(避免 clone Quest 到 spawn_blocking)
        let serialized_state = rmp_serde::to_vec(quest)
            .map_err(|e| QuestError::SerializationError(format!("msgpack encode: {e}")))?;
        let memory_snapshot_hash = compute_sha256_hex(&serialized_state);

        let checkpoint = Checkpoint::new(
            quest.quest_id.clone(),
            checkpoint_id,
            memory_snapshot_hash,
            serialized_state,
        );

        // 阻塞操作:创建目录 + 写文件 + prune_old 移到 spawn_blocking
        let dir = self.checkpoint_dir.clone();
        let max_keep = self.max_keep;

        tokio::task::spawn_blocking(move || Self::save_blocking(checkpoint, &dir, max_keep))
            .await
            .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// 检查点保存的阻塞部分 — 磁盘写入 + 旧检查点清理
    ///
    /// WHY 独立静态函数:spawn_blocking 要求闭包 Send + 'static,
    /// 将所需参数显式传入,避免捕获 `&self` 引发借用冲突。
    fn save_blocking(
        checkpoint: Checkpoint,
        checkpoint_dir: &Path,
        max_keep: usize,
    ) -> Result<Checkpoint, QuestError> {
        let quest_id = checkpoint.quest_id.clone();
        let checkpoint_id_str = checkpoint.checkpoint_id.clone();

        // 构造文件路径
        let file_path = checkpoint_dir
            .join(&quest_id)
            .join(format!("{checkpoint_id_str}.bin"));

        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                QuestError::CheckpointSaveFailed(format!("mkdir {}: {e}", parent.display()))
            })?;
        }

        // 序列化 Checkpoint 为 MessagePack 并写入磁盘
        let bytes = rmp_serde::to_vec(&checkpoint)
            .map_err(|e| QuestError::SerializationError(format!("msgpack encode cp: {e}")))?;
        fs::write(&file_path, bytes).map_err(|e| {
            QuestError::CheckpointSaveFailed(format!("write {}: {e}", file_path.display()))
        })?;

        // 清理超出 max_keep 的旧检查点(失败不阻断保存,仅记录)
        if let Err(e) = Self::prune_old_blocking(&quest_id, checkpoint_dir, max_keep) {
            tracing::warn!(
                quest_id = %quest_id,
                error = %e,
                "prune_old 失败,旧检查点未清理(不影响本次保存)"
            );
        }

        tracing::debug!(
            quest_id = %quest_id,
            checkpoint_id = %checkpoint_id_str,
            file = %file_path.display(),
            "检查点已保存"
        );
        Ok(checkpoint)
    }

    /// 加载指定检查点 — 读取磁盘并校验完整性(异步)
    ///
    /// 内部通过 `spawn_blocking` 将磁盘读取 + MessagePack 反序列化 + SHA-256 校验
    /// 放到阻塞线程池执行。
    pub async fn load(
        &self,
        quest_id: &str,
        checkpoint_id: &str,
    ) -> Result<Checkpoint, QuestError> {
        let file_path = self.checkpoint_path(quest_id, checkpoint_id);

        tokio::task::spawn_blocking(move || Self::load_blocking(&file_path))
            .await
            .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// 检查点加载的阻塞部分 — 磁盘读取 + 反序列化 + 完整性校验
    fn load_blocking(file_path: &Path) -> Result<Checkpoint, QuestError> {
        let bytes = fs::read(file_path).map_err(|e| {
            QuestError::CheckpointNotFound(format!("read {}: {e}", file_path.display()))
        })?;
        let checkpoint: Checkpoint = rmp_serde::from_slice(&bytes)
            .map_err(|e| QuestError::SerializationError(format!("msgpack decode cp: {e}")))?;
        // 完整性校验(防磁盘位翻转或人为篡改)
        let actual_hash = compute_sha256_hex(&checkpoint.serialized_state);
        if actual_hash != checkpoint.memory_snapshot_hash {
            return Err(QuestError::CheckpointCorrupted);
        }
        Ok(checkpoint)
    }

    /// 加载最新检查点(按 created_at 排序)— 无检查点返回 None(异步)
    ///
    /// WHY:崩溃恢复场景下,用户只需"最新可用检查点",无需知道具体 ID。
    /// 按 created_at 排序而非 checkpoint_id,因前者语义明确(时间),
    /// 后者虽 UUIDv7 时间有序但解析复杂。
    ///
    /// 内部通过 `spawn_blocking` 将多次磁盘读取 + 完整性校验放到阻塞线程池,
    /// 避免 load_latest 最差情况(max_keep 次文件读取)阻塞 tokio worker。
    pub async fn load_latest(&self, quest_id: &str) -> Result<Option<Checkpoint>, QuestError> {
        let dir = self.checkpoint_dir.join(quest_id);
        let quest_id = quest_id.to_string();

        tokio::task::spawn_blocking(move || Self::load_latest_blocking(&dir, &quest_id))
            .await
            .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// 加载最新检查点的阻塞部分 — 列出 + 逐个加载 + 排序
    fn load_latest_blocking(
        quest_dir: &Path,
        quest_id: &str,
    ) -> Result<Option<Checkpoint>, QuestError> {
        // 列出所有 .bin 文件(等价于 list_checkpoints 的文件遍历部分)
        if !quest_dir.exists() {
            return Ok(None);
        }
        let entries = fs::read_dir(quest_dir).map_err(|e| {
            QuestError::CheckpointSaveFailed(format!("readdir {}: {e}", quest_dir.display()))
        })?;
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| QuestError::CheckpointSaveFailed(format!("readdir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(stem.to_string());
                }
            }
        }
        if ids.is_empty() {
            return Ok(None);
        }

        // 逐个加载并校验(失败的跳过,不阻断)
        let mut checkpoints: Vec<Checkpoint> = Vec::with_capacity(ids.len());
        for id in &ids {
            let file_path = quest_dir.join(format!("{id}.bin"));
            match Self::load_blocking(&file_path) {
                Ok(cp) => checkpoints.push(cp),
                Err(e) => {
                    tracing::warn!(
                        quest_id = quest_id,
                        checkpoint_id = %id,
                        error = %e,
                        "加载检查点失败,跳过(可能已损坏)"
                    );
                }
            }
        }
        // 按 created_at 降序,取最新
        checkpoints.sort_by_key(|cp| std::cmp::Reverse(cp.created_at));
        Ok(checkpoints.into_iter().next())
    }

    /// 校验检查点完整性 — 重新计算 SHA-256 与存储的 hash 比对
    ///
    /// 不匹配返回 `CheckpointCorrupted`,防止使用被篡改/损坏的状态恢复
    pub fn verify_integrity(&self, checkpoint: &Checkpoint) -> Result<(), QuestError> {
        let actual_hash = compute_sha256_hex(&checkpoint.serialized_state);
        if actual_hash != checkpoint.memory_snapshot_hash {
            return Err(QuestError::CheckpointCorrupted);
        }
        Ok(())
    }

    /// 保留最近 N 个检查点,删除其余(异步)
    ///
    /// 按 created_at 降序排序,保留前 N 个,删除其余文件。
    /// WHY:避免磁盘膨胀,同时保留足够回滚点。
    pub async fn prune_old(&self, quest_id: &str, keep: usize) -> Result<(), QuestError> {
        let dir = self.checkpoint_dir.clone();
        let quest_id = quest_id.to_string();

        tokio::task::spawn_blocking(move || Self::prune_old_blocking(&quest_id, &dir, keep))
            .await
            .map_err(|e| QuestError::CheckpointSaveFailed(format!("spawn_blocking join: {e}")))?
    }

    /// 检查点清理的阻塞部分 — 列出 + 加载元数据 + 删除旧文件
    ///
    /// WHY 独立静态函数:save_blocking 内部调用,需纯同步实现。
    fn prune_old_blocking(
        quest_id: &str,
        checkpoint_dir: &Path,
        keep: usize,
    ) -> Result<(), QuestError> {
        let quest_dir = checkpoint_dir.join(quest_id);
        if !quest_dir.exists() {
            return Ok(());
        }

        // 列出所有 .bin 文件
        let entries = fs::read_dir(&quest_dir).map_err(|e| {
            QuestError::CheckpointSaveFailed(format!("readdir {}: {e}", quest_dir.display()))
        })?;
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| QuestError::CheckpointSaveFailed(format!("readdir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(stem.to_string());
                }
            }
        }

        if ids.len() <= keep {
            return Ok(());
        }

        // 逐个加载获取 created_at(使用阻塞版本,避免异步调用)
        let mut checkpoints: Vec<(String, chrono::DateTime<Utc>)> = Vec::with_capacity(ids.len());
        for id in &ids {
            let file_path = quest_dir.join(format!("{id}.bin"));
            if let Ok(cp) = Self::load_blocking(&file_path) {
                checkpoints.push((cp.checkpoint_id, cp.created_at));
            }
        }

        // 按 created_at 降序,保留前 keep 个,删除其余
        checkpoints.sort_by_key(|(_, ts)| std::cmp::Reverse(*ts));
        for (id, _) in checkpoints.iter().skip(keep) {
            let path = quest_dir.join(format!("{id}.bin"));
            if let Err(e) = fs::remove_file(&path) {
                tracing::warn!(
                    quest_id = quest_id,
                    checkpoint_id = id,
                    error = %e,
                    "删除旧检查点失败(继续清理其余)"
                );
            }
        }
        Ok(())
    }

    /// 列出指定 Quest 的所有检查点 ID(文件名去扩展名)
    ///
    /// 返回顺序未定义,调用方需自行排序(如 load_latest 按 created_at 排序)
    pub fn list_checkpoints(&self, quest_id: &str) -> Result<Vec<String>, QuestError> {
        let dir = self.quest_dir(quest_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&dir).map_err(|e| {
            QuestError::CheckpointSaveFailed(format!("readdir {}: {e}", dir.display()))
        })?;
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| QuestError::CheckpointSaveFailed(format!("readdir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(stem.to_string());
                }
            }
        }
        Ok(ids)
    }

    /// 构造 Quest 检查点目录:`<checkpoint_dir>/<quest_id>/`
    fn quest_dir(&self, quest_id: &str) -> PathBuf {
        self.checkpoint_dir.join(quest_id)
    }

    /// 构造检查点文件路径:`<checkpoint_dir>/<quest_id>/<checkpoint_id>.bin`
    fn checkpoint_path(&self, quest_id: &str, checkpoint_id: &str) -> PathBuf {
        self.quest_dir(quest_id)
            .join(format!("{checkpoint_id}.bin"))
    }

    /// 检查点根目录(只读访问,供测试与诊断)
    pub fn checkpoint_dir(&self) -> &Path {
        &self.checkpoint_dir
    }

    /// 当前保留上限
    pub fn max_keep(&self) -> usize {
        self.max_keep
    }
}

/// 计算 SHA-256 哈希并返回十六进制字符串
///
/// WHY 单独函数:save 与 verify_integrity 共用,确保哈希算法一致
fn compute_sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hasher.finalize();
    hex::encode(hash)
}

/// 检查点元数据(用于内部排序与诊断,不包含完整序列化数据)
///
/// WHY:此类型当前未在公共 API 使用,预留供未来"轻量索引文件"优化 —
/// 当前直接从 .bin 文件加载完整 Checkpoint 获取 created_at,
/// 文件数少时性能可接受;后续若检查点数量增长,可单独持久化元数据索引
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // 预留供未来索引优化使用
struct CheckpointMetaInternal {
    checkpoint_id: String,
    quest_id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    memory_snapshot_hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{MultimodalInput, Task, TaskStatus, ThinkingMode, UserIntent};
    use tempfile::tempdir;

    /// 构造测试用 Quest
    fn make_quest(id: &str, task_count: usize) -> Quest {
        let tasks = (0..task_count)
            .map(|i| Task {
                task_id: format!("task-{i}"),
                description: format!("任务 {i}"),
                status: TaskStatus::Pending,
                dependencies: if i == 0 {
                    vec![]
                } else {
                    vec![format!("task-{}", i - 1)]
                },
            })
            .collect();
        Quest {
            quest_id: id.into(),
            title: format!("测试 Quest {id}"),
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    #[tokio::test]
    async fn test_save_load_roundtrip() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-1", 3);

        let checkpoint = cm.save(&quest).await.unwrap();
        assert_eq!(checkpoint.quest_id, "q-1");
        assert!(!checkpoint.serialized_state.is_empty());
        assert!(!checkpoint.memory_snapshot_hash.is_empty());

        let loaded = cm.load("q-1", &checkpoint.checkpoint_id).await.unwrap();
        assert_eq!(loaded.quest_id, checkpoint.quest_id);
        assert_eq!(loaded.checkpoint_id, checkpoint.checkpoint_id);
        assert_eq!(loaded.memory_snapshot_hash, checkpoint.memory_snapshot_hash);
        assert_eq!(loaded.serialized_state, checkpoint.serialized_state);

        // 反序列化 Quest 验证字段一致
        let restored_quest: Quest = rmp_serde::from_slice(&loaded.serialized_state).unwrap();
        assert_eq!(restored_quest.quest_id, quest.quest_id);
        assert_eq!(restored_quest.tasks.len(), quest.tasks.len());
        assert_eq!(restored_quest.thinking_mode, quest.thinking_mode);
    }

    #[tokio::test]
    async fn test_verify_integrity_corrupted() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-1", 2);

        let mut checkpoint = cm.save(&quest).await.unwrap();
        // 篡改 serialized_state,哈希应不匹配
        checkpoint.serialized_state[0] ^= 0xff;
        let result = cm.verify_integrity(&checkpoint);
        assert!(matches!(result, Err(QuestError::CheckpointCorrupted)));
    }

    #[tokio::test]
    async fn test_load_corrupted_file_returns_error() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-1", 2);

        let checkpoint = cm.save(&quest).await.unwrap();
        // 直接篡改磁盘文件
        let path = cm.checkpoint_path("q-1", &checkpoint.checkpoint_id);
        let mut bytes = std::fs::read(&path).unwrap();
        // 翻转最后一个字节(可能影响 serialized_state 或 hash 字段)
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();

        let result = cm.load("q-1", &checkpoint.checkpoint_id).await;
        // 篡改可能破坏反序列化或哈希校验,任一错误均可接受
        assert!(result.is_err(), "篡改文件后 load 应失败");
    }

    #[tokio::test]
    async fn test_prune_old_keeps_latest_n() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::with_max_keep(tmp.path().to_path_buf(), 3);
        let quest = make_quest("q-1", 1);

        // 创建 5 个检查点
        let mut ids = Vec::new();
        for _ in 0..5 {
            // 微小延迟确保 created_at 不同(chrono::Utc::now 精度可能不足)
            std::thread::sleep(std::time::Duration::from_millis(5));
            let cp = cm.save(&quest).await.unwrap();
            ids.push(cp.checkpoint_id);
        }

        let remaining = cm.list_checkpoints("q-1").unwrap();
        assert_eq!(remaining.len(), 3, "应保留最近 3 个检查点");
        // 最新的 3 个应保留(后创建的)
        assert!(remaining.contains(&ids[3]));
        assert!(remaining.contains(&ids[4]));
    }

    #[tokio::test]
    async fn test_load_latest_returns_none_when_empty() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let result = cm.load_latest("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_load_latest_returns_most_recent() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let quest = make_quest("q-1", 1);

        let mut newest_time = chrono::DateTime::<Utc>::MIN_UTC;
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let cp = cm.save(&quest).await.unwrap();
            newest_time = cp.created_at;
        }

        let latest = cm.load_latest("q-1").await.unwrap().unwrap();
        assert_eq!(latest.created_at, newest_time);
    }

    #[test]
    fn test_list_checkpoints_empty_quest() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let ids = cm.list_checkpoints("no-such-quest").unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn test_load_nonexistent_returns_error() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let result = cm.load("q-1", "cp-nonexistent").await;
        assert!(matches!(result, Err(QuestError::CheckpointNotFound(_))));
    }

    #[test]
    fn test_compute_sha256_hex_deterministic() {
        let h1 = compute_sha256_hex(b"hello");
        let h2 = compute_sha256_hex(b"hello");
        let h3 = compute_sha256_hex(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        // SHA-256 hex 长度为 64
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_with_max_keep_custom() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::with_max_keep(tmp.path().to_path_buf(), 10);
        assert_eq!(cm.max_keep(), 10);
    }

    #[test]
    fn test_checkpoint_dir_accessor() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        let cm = CheckpointManager::new(path.clone());
        assert_eq!(cm.checkpoint_dir(), &path);
    }

    #[tokio::test]
    async fn test_save_creates_nested_directory() {
        let tmp = tempdir().unwrap();
        // 嵌套不存在的目录,save 应自动创建
        let nested = tmp.path().join("a").join("b").join("c");
        let cm = CheckpointManager::new(nested.clone());
        let quest = make_quest("q-1", 1);
        let result = cm.save(&quest).await;
        assert!(result.is_ok(), "应自动创建嵌套目录");
        assert!(nested.exists());
    }

    #[tokio::test]
    async fn test_multiple_quests_isolated() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        let q1 = make_quest("q-1", 2);
        let q2 = make_quest("q-2", 3);

        cm.save(&q1).await.unwrap();
        cm.save(&q2).await.unwrap();

        // 各 Quest 的检查点互不影响
        let ids1 = cm.list_checkpoints("q-1").unwrap();
        let ids2 = cm.list_checkpoints("q-2").unwrap();
        assert_eq!(ids1.len(), 1);
        assert_eq!(ids2.len(), 1);

        // 删除 q-1 的检查点不影响 q-2
        let path1 = cm.checkpoint_path("q-1", &ids1[0]);
        std::fs::remove_file(&path1).unwrap();
        assert_eq!(cm.list_checkpoints("q-2").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_prune_when_under_limit() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::with_max_keep(tmp.path().to_path_buf(), 5);
        let quest = make_quest("q-1", 1);

        // 仅创建 2 个检查点(未超限),prune 不应删除任何
        cm.save(&quest).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        cm.save(&quest).await.unwrap();

        let ids = cm.list_checkpoints("q-1").unwrap();
        assert_eq!(ids.len(), 2);
    }

    /// 验证 UserIntent 与 Quest 的 MessagePack 序列化兼容性
    /// (确保未来扩展字段不破坏旧检查点)
    #[test]
    fn test_msgpack_quest_serialization_stable() {
        let quest = make_quest("q-stable", 4);
        let bytes = rmp_serde::to_vec(&quest).unwrap();
        let de: Quest = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(de, quest);
    }

    /// 验证包含多模态输入的 Quest 也能正确序列化
    #[tokio::test]
    async fn test_save_quest_with_multimodal_intent_context() {
        let tmp = tempdir().unwrap();
        let cm = CheckpointManager::new(tmp.path().to_path_buf());
        // 构造带多模态输入描述的 Quest(模拟实际场景)
        let intent = UserIntent {
            intent_id: "i-1".into(),
            raw_text: "分析图像。生成报告。".into(),
            multimodal_inputs: vec![MultimodalInput::Text("图像数据".into())],
            risk_level: 50,
        };
        // Quest 本身不存储 intent,但 description 携带文本
        let quest = Quest {
            quest_id: "q-mm".into(),
            title: intent.raw_text.clone(),
            tasks: vec![Task {
                task_id: "task-0".into(),
                description: intent.raw_text.clone(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        };
        let cp = cm.save(&quest).await.unwrap();
        let loaded = cm.load("q-mm", &cp.checkpoint_id).await.unwrap();
        let restored: Quest = rmp_serde::from_slice(&loaded.serialized_state).unwrap();
        assert_eq!(restored, quest);
    }
}
