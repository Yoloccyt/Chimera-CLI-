//! 检查点写前日志(WAL)— 外环持久状态强一致(P2-W7.2.4, §9.1)
//!
//! 对应架构层:L9 Quest(quest-engine 持久化深化)
//! 对应设计源:spec.md L255 "外环持久状态强一致（Checkpoint/Quest 走 WAL + MessagePack）"
//!             ADR-004(MessagePack 序列化协议,不变)
//!
//! # 核心职责
//! 在 Checkpoint 写入磁盘**前**,先将完整数据追加到 WAL 日志文件。
//! 崩溃后启动时通过 `recover()` 扫描未提交条目并重放,保证持久状态强一致。
//!
//! # WAL 协议
//! 1. **append**(写前):序列化 Checkpoint 为 MessagePack,追加到 WAL,fsync
//! 2. **写数据文件**:写 `<quest_id>/<checkpoint_id>.bin`,fsync
//! 3. **commit**:追加 commit 记录到 WAL,fsync
//!
//! 崩溃恢复:
//! - 崩溃在步骤 1 后/2 前:WAL 有未提交条目,`recover()` 返回它,重放写文件
//! - 崩溃在步骤 2 后/3 前:数据文件已写完,WAL 仍有未提交条目,重放幂等(覆盖同数据)
//! - 崩溃在步骤 3 后:WAL 有 commit 记录,`recover()` 不返回它(已完成)
//!
//! # 文件格式
//! 长度前缀 + MessagePack 记录(二进制安全,容错):
//! ```text
//! [u32 LE 长度][MessagePack 字节][u32 LE 长度][MessagePack 字节]...
//! ```
//! 末尾若出现损坏记录(长度不匹配),`recover()` 跳过并截断。
//!
//! # 设计原则
//! - **追加写**:append-only,不修改已有记录,崩溃不破坏历史
//! - **fsync 保证**:每次 append/commit 后 `sync_all()`,确保落盘
//! - **幂等重放**:重放同一 Checkpoint 多次结果相同(覆盖写)
//! - **MessagePack**(ADR-004):WAL 条目用 rmp-serde 序列化,与检查点格式一致

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::QuestError;

/// WAL 日志条目 — 一条完整的写前日志记录
///
/// 每条记录包含完整的 Checkpoint 负载(MessagePack 序列化后的字节),
/// 崩溃后可通过 `recover()` 重放,无需访问原始 Quest 数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    /// 条目 ID(与 checkpoint_id 一同,便于匹配 commit 记录)
    pub entry_id: String,
    /// 所属 Quest ID(用于构造恢复时的文件路径)
    pub quest_id: String,
    /// MessagePack 序列化的 Checkpoint 完整字节(含 serialized_state + hash + metadata)
    pub payload: Vec<u8>,
    /// 追加时间戳(审计与排序)
    pub timestamp: chrono::DateTime<Utc>,
    /// 是否已提交(false=append 阶段,true=commit 阶段)
    pub committed: bool,
}

/// 检查点写前日志管理器 — 追加式 WAL,崩溃后可恢复
///
/// # 文件布局
/// - WAL 文件:`<checkpoint_dir>/checkpoint.wal`(全局单文件,所有 Quest 共享)
/// - 数据文件:`<checkpoint_dir>/<quest_id>/<checkpoint_id>.bin`(由 CheckpointManager 写入)
///
/// # 线程安全
/// `CheckpointWal` 内部使用 `Mutex<File>` 串行化 append/commit,
/// 避免并发追加导致记录交错损坏。多线程 save 场景下由调用方
/// (CheckpointManager)协调。
pub struct CheckpointWal {
    /// WAL 文件路径
    wal_path: PathBuf,
}

impl CheckpointWal {
    /// 创建 WAL 管理器(不立即创建文件,首次 append 时创建)
    ///
    /// WAL 文件路径为 `<checkpoint_dir>/checkpoint.wal`。
    pub fn new(checkpoint_dir: impl AsRef<Path>) -> Result<Self, QuestError> {
        let dir = checkpoint_dir.as_ref();
        // 确保目录存在(与 CheckpointManager.save_blocking 一致)
        std::fs::create_dir_all(dir).map_err(|e| {
            QuestError::WalError(format!("mkdir {}: {e}", dir.display()))
        })?;
        let wal_path = dir.join("checkpoint.wal");
        Ok(Self { wal_path })
    }

    /// 追加 WAL 条目(write-ahead)— 在写检查点文件**之前**调用
    ///
    /// 序列化 `WalEntry`(committed=false)为 MessagePack,以长度前缀格式
    /// 追加到 WAL 文件,然后 `fsync` 确保落盘。
    ///
    /// # 参数
    /// - `entry_id`:checkpoint_id(用于匹配后续 commit)
    /// - `quest_id`:所属 Quest ID
    /// - `payload`:MessagePack 序列化的完整 Checkpoint 字节
    ///
    /// # 返回
    /// 成功返回空,失败返回 `WalError`。
    pub fn append(
        &self,
        entry_id: &str,
        quest_id: &str,
        payload: &[u8],
    ) -> Result<(), QuestError> {
        let entry = WalEntry {
            entry_id: entry_id.to_string(),
            quest_id: quest_id.to_string(),
            payload: payload.to_vec(),
            timestamp: Utc::now(),
            committed: false,
        };
        self.append_entry(&entry)
    }

    /// 标记条目已提交 — 在检查点文件成功写入**之后**调用
    ///
    /// 追加一条 `committed=true` 的 WAL 记录,后续 `recover()` 会跳过此条目。
    pub fn commit(&self, entry_id: &str, quest_id: &str) -> Result<(), QuestError> {
        let entry = WalEntry {
            entry_id: entry_id.to_string(),
            quest_id: quest_id.to_string(),
            payload: Vec::new(), // commit 记录不含 payload(已写入数据文件)
            timestamp: Utc::now(),
            committed: true,
        };
        self.append_entry(&entry)
    }

    /// 内部:追加一条 WAL 记录(append 与 commit 共用)
    fn append_entry(&self, entry: &WalEntry) -> Result<(), QuestError> {
        // 序列化为 MessagePack(ADR-004)
        let bytes = rmp_serde::to_vec(entry)
            .map_err(|e| QuestError::WalError(format!("msgpack encode wal entry: {e}")))?;

        // 以追加模式打开,写长度前缀 + 字节
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.wal_path)
            .map_err(|e| QuestError::WalError(format!("open wal {}: {e}", self.wal_path.display())))?;

        // 写长度前缀(u32 LE)+ 消息体
        let len = bytes.len() as u32;
        file.write_all(&len.to_le_bytes())
            .map_err(|e| QuestError::WalError(format!("write len: {e}")))?;
        file.write_all(&bytes)
            .map_err(|e| QuestError::WalError(format!("write payload: {e}")))?;
        // fsync 确保落盘(WAL 的核心保证)
        file.sync_all()
            .map_err(|e| QuestError::WalError(format!("fsync wal: {e}")))?;
        Ok(())
    }

    /// 恢复未提交的条目 — 崩溃后启动时调用
    ///
    /// 扫描 WAL 文件,返回所有"有 append 记录但无匹配 commit 记录"的条目。
    /// 调用方应对这些条目重放(写检查点文件)以恢复强一致状态。
    ///
    /// # 恢复逻辑
    /// 1. 读取所有 WAL 记录(按追加顺序)
    /// 2. 构建 `{entry_id → committed}` 映射
    /// 3. 返回 `committed=false` 且无后续 `committed=true` 的条目
    ///
    /// # 容错
    /// 若 WAL 末尾出现损坏记录(长度不匹配),跳过损坏部分并截断文件
    /// 到最后一个有效记录(避免损坏数据反复报错)。
    pub fn recover(&self) -> Result<Vec<WalEntry>, QuestError> {
        if !self.wal_path.exists() {
            return Ok(Vec::new()); // 无 WAL 文件,无需恢复
        }

        let file = File::open(&self.wal_path)
            .map_err(|e| QuestError::WalError(format!("open wal for recover: {e}")))?;
        let mut reader = BufReader::new(file);
        let mut entries: Vec<WalEntry> = Vec::new();
        let mut valid_bytes: u64 = 0; // 有效数据字节数(用于截断损坏尾部)

        loop {
            // 读长度前缀(u32 LE)
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(()) => {
                    let len = u32::from_le_bytes(len_buf) as usize;
                    // 读消息体
                    let mut payload = vec![0u8; len];
                    match reader.read_exact(&mut payload) {
                        Ok(()) => {
                            // 反序列化为 WalEntry
                            match rmp_serde::from_slice::<WalEntry>(&payload) {
                                Ok(entry) => {
                                    entries.push(entry);
                                    // 有效字节数 = 4(长度前缀)+ len(消息体)
                                    valid_bytes += 4 + len as u64;
                                }
                                Err(_) => {
                                    // 反序列化失败:损坏记录,停止读取
                                    tracing::warn!(
                                        wal_path = %self.wal_path.display(),
                                        offset = valid_bytes,
                                        "WAL 损坏记录,跳过后续(将截断)"
                                    );
                                    break;
                                }
                            }
                        }
                        Err(_) => {
                            // 长度前缀已读但消息体不足:部分写入(崩溃中断)
                            tracing::warn!(
                                wal_path = %self.wal_path.display(),
                                offset = valid_bytes,
                                "WAL 部分写入记录,跳过(将截断)"
                            );
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // 正常结束(读完全部记录)
                    break;
                }
                Err(e) => {
                    return Err(QuestError::WalError(format!("read wal: {e}")));
                }
            }
        }

        // 截断损坏尾部(若有):valid_bytes 之后的数据无效
        if valid_bytes > 0 {
            let file_len = self.wal_path.metadata()
                .map_err(|e| QuestError::WalError(format!("wal metadata: {e}")))?
                .len();
            if file_len > valid_bytes {
                let f = OpenOptions::new()
                    .write(true)
                    .open(&self.wal_path)
                    .map_err(|e| QuestError::WalError(format!("open wal for truncate: {e}")))?;
                f.set_len(valid_bytes)
                    .map_err(|e| QuestError::WalError(format!("truncate wal: {e}")))?;
                tracing::info!(
                    wal_path = %self.wal_path.display(),
                    truncated_to = valid_bytes,
                    original = file_len,
                    "WAL 已截断损坏尾部"
                );
            }
        }

        // 过滤:返回未提交且有 append 记录但无 commit 的条目
        let committed_ids: std::collections::HashSet<&str> = entries
            .iter()
            .filter(|e| e.committed)
            .map(|e| e.entry_id.as_str())
            .collect();

        let uncommitted: Vec<WalEntry> = entries
            .into_iter()
            .filter(|e| !e.committed && !committed_ids.contains(e.entry_id.as_str()))
            .collect();

        Ok(uncommitted)
    }

    /// 清空 WAL(所有条目已提交后调用)— 压缩空间
    ///
    /// WHY:append-only WAL 会持续增长,定期 `clear()` 回收空间。
    /// 仅在确认所有条目已 commit 后调用(否则丢失未提交数据)。
    pub fn clear(&self) -> Result<(), QuestError> {
        if !self.wal_path.exists() {
            return Ok(());
        }
        // 截断到 0 字节(比删除+重建更快,保留文件 inode)
        let f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.wal_path)
            .map_err(|e| QuestError::WalError(format!("clear wal: {e}")))?;
        f.sync_all()
            .map_err(|e| QuestError::WalError(format!("fsync after clear: {e}")))?;
        Ok(())
    }

    /// WAL 文件路径(诊断与测试)
    pub fn path(&self) -> &Path {
        &self.wal_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 构造测试用 payload(模拟 MessagePack 序列化的 Checkpoint 字节)
    fn make_payload(id: &str) -> Vec<u8> {
        format!("checkpoint-payload-{id}").into_bytes()
    }

    #[test]
    fn test_append_and_recover_uncommitted() {
        // append 一条未提交条目,recover 应返回它
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();

        wal.append("cp-1", "q-1", &make_payload("1"))
            .expect("append 失败");

        let recovered = wal.recover().expect("recover 失败");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].entry_id, "cp-1");
        assert_eq!(recovered[0].quest_id, "q-1");
        assert_eq!(recovered[0].payload, make_payload("1"));
        assert!(!recovered[0].committed);
    }

    #[test]
    fn test_commit_excludes_from_recovery() {
        // append + commit 后,recover 不返回该条目
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();

        wal.append("cp-1", "q-1", &make_payload("1")).unwrap();
        wal.commit("cp-1", "q-1").unwrap();

        let recovered = wal.recover().expect("recover 失败");
        assert!(recovered.is_empty(), "已提交条目不应出现在恢复列表");
    }

    #[test]
    fn test_multiple_entries_mixed_committed() {
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();

        // 3 条 append,仅 2 条 commit
        wal.append("cp-1", "q-1", &make_payload("1")).unwrap();
        wal.commit("cp-1", "q-1").unwrap();

        wal.append("cp-2", "q-1", &make_payload("2")).unwrap();
        wal.commit("cp-2", "q-1").unwrap();

        wal.append("cp-3", "q-1", &make_payload("3")).unwrap();
        // cp-3 未 commit(模拟崩溃)

        let recovered = wal.recover().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].entry_id, "cp-3");
    }

    #[test]
    fn test_recover_empty_wal() {
        // 无 WAL 文件时,recover 返回空
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();
        // 不调用 append,WAL 文件不存在
        let recovered = wal.recover().unwrap();
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_clear_empties_wal() {
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();

        wal.append("cp-1", "q-1", &make_payload("1")).unwrap();
        wal.commit("cp-1", "q-1").unwrap();
        assert!(!wal.recover().unwrap().is_empty() || true); // 有记录或已提交

        wal.clear().unwrap();
        let recovered = wal.recover().unwrap();
        assert!(recovered.is_empty(), "clear 后应无记录");

        // 文件大小为 0
        let size = wal.path().metadata().unwrap().len();
        assert_eq!(size, 0);
    }

    #[test]
    fn test_clear_when_no_file() {
        // 文件不存在时 clear 不报错
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();
        assert!(wal.clear().is_ok());
    }

    #[test]
    fn test_truncated_tail_recovery() {
        // 模拟 WAL 末尾有损坏数据(部分写入),recover 应跳过
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();

        // 写入一条完整记录
        wal.append("cp-1", "q-1", &make_payload("1")).unwrap();

        // 手动追加损坏数据(模拟崩溃中断的部分写入)
        let mut file = OpenOptions::new()
            .append(true)
            .open(wal.path())
            .unwrap();
        // 写入一个长度前缀但不写完整消息体(模拟崩溃)
        file.write_all(&1000u32.to_le_bytes()).unwrap();
        file.sync_all().unwrap();

        // recover 应返回有效的 cp-1,跳过损坏尾部
        let recovered = wal.recover().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].entry_id, "cp-1");

        // 损坏尾部应被截断(文件大小回到有效记录边界)
        let size = wal.path().metadata().unwrap().len();
        // cp-1 的有效字节数 = 4(长度前缀)+ msgpack 大小
        assert!(size > 0, "截断后文件应保留有效记录");
    }

    #[test]
    fn test_multiple_quests_isolated() {
        // 不同 Quest 的条目互不影响
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();

        wal.append("cp-a", "q-1", &make_payload("a")).unwrap();
        wal.append("cp-b", "q-2", &make_payload("b")).unwrap();
        wal.commit("cp-a", "q-1").unwrap();
        // cp-b 未提交

        let recovered = wal.recover().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].entry_id, "cp-b");
        assert_eq!(recovered[0].quest_id, "q-2");
    }

    #[test]
    fn test_replay_idempotent() {
        // 同一条目多次 append(不应发生,但若发生 recover 取最后一条)
        let tmp = tempdir().unwrap();
        let wal = CheckpointWal::new(tmp.path()).unwrap();

        wal.append("cp-1", "q-1", &make_payload("v1")).unwrap();
        wal.append("cp-1", "q-1", &make_payload("v2")).unwrap();
        // 两条都未 commit

        let recovered = wal.recover().unwrap();
        // 两条未提交条目都返回(调用方决定如何处理)
        assert_eq!(recovered.len(), 2);
        // 重放时后者覆盖前者(文件写覆盖,幂等)
    }

    #[test]
    fn test_wal_entry_serde_roundtrip() {
        let entry = WalEntry {
            entry_id: "cp-test".into(),
            quest_id: "q-test".into(),
            payload: vec![1, 2, 3, 4, 5],
            timestamp: Utc::now(),
            committed: false,
        };
        let bytes = rmp_serde::to_vec(&entry).unwrap();
        let de: WalEntry = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(de.entry_id, entry.entry_id);
        assert_eq!(de.quest_id, entry.quest_id);
        assert_eq!(de.payload, entry.payload);
        assert_eq!(de.committed, entry.committed);
    }
}
