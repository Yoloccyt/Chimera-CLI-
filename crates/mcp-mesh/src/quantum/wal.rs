//! WAL（Write-Ahead Log）— 2PC 协调者崩溃恢复持久化
//!
//! 对应架构层:L10 Interface
//! 对应任务:Task 0.7 v2.9.0-omega(SubTask 0.7.2 / 0.7.5)
//!
//! # 设计原理(Gray & Reuter《Transaction Processing》第 7 章)
//!
//! 2PC 协调者在每阶段切换前先写 WAL,确保崩溃后可重建事务状态:
//! 1. Prepare 成功 → 写 Prepare entry(参与者已 ACK)
//! 2. Commit 成功 → 写 Commit entry(事务终结)
//! 3. Rollback 完成 → 写 Rollback entry(事务终结)
//!
//! 崩溃后 `recover_from_wal` 扫描所有 entry:
//! - 仅有 Prepare entry 的事务:重新发起 Commit(因参与者已 ACK,必须提交)
//! - 有 Commit/Rollback entry 的事务:已完成,跳过
//!
//! # 文件格式
//!
//! 采用 JSONL（newline-delimited JSON）而非 MessagePack 二进制:
//! - 人类可读,便于排查崩溃现场
//! - 天然按行分割,`BufReader::lines()` 即可解析
//! - truncate 时不需要重新编码
//! - 体积稍大可忽略(WAL 单条 entry < 200 字节)
//!
//! # async 反模式防御(§4.4)
//!
//! - 文件 IO 全部 `spawn_blocking` 包装,避免阻塞 async runtime
//! - fsync 必须在 blocking 上下文调用(同步且慢,~ms 级)
//! - WAL 文件路径不可变,避免运行时迁移引入竞态

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use tokio::task;
use tracing::{debug, warn};

use crate::error::McpError;
use crate::quantum::transaction::WalEntry;

/// 默认 WAL 文件路径(相对用户家目录)
///
/// WHY `~/.chimera/mcp_mesh.wal`:与项目其他持久化(配置/缓存)统一在 `~/.chimera/`,
/// 避免散落各处。生产环境可通过 `MeshConfig::wal_path` 覆盖。
pub const DEFAULT_WAL_FILENAME: &str = "mcp_mesh.wal";

/// WAL 持久化管理器 — append-only 文件 + spawn_blocking IO
///
/// 所有 IO 操作通过 `spawn_blocking` 在 blocking 线程池执行,避免阻塞 async runtime。
/// 单实例线程安全(`Send + Sync`),内部无锁(每次操作独立打开文件)。
///
/// # 性能权衡
///
/// - 每次 `append` 都 `fsync`,确保崩溃不丢数据(代价:~1-5ms 延迟)
/// - 不做写合并,因 2PC 事务串行执行,WAL 写入频率 = 事务频率,通常 < 100 TPS
/// - 如未来需高吞吐,可引入 group commit(批量 fsync)
pub struct WalStore {
    /// WAL 文件绝对路径
    path: PathBuf,
}

impl WalStore {
    /// 创建 WAL 存储,使用指定路径
    ///
    /// 不立即创建文件 — 首次 `append` 时按需创建(避免空 WAL 文件污染)。
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// 解析默认 WAL 路径(`~/.chimera/mcp_mesh.wal`)
    ///
    /// WHY 静态方法:配置默认值在 `MeshConfig::default()` 中需要,不应要求实例化。
    /// 失败时返回 None,调用方降级为禁用 WAL(`MeshConfig::durable = false`)。
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        let mut path = PathBuf::from(home);
        path.push(".chimera");
        path.push(DEFAULT_WAL_FILENAME);
        Some(path)
    }

    /// 追加一条 WAL entry(同步 fsync)
    ///
    /// # 流程
    /// 1. 序列化 entry 为 JSON 单行
    /// 2. `spawn_blocking` 中以 append 模式打开文件
    /// 3. 写入 JSON + 换行符
    /// 4. `sync_all` 确保数据落盘(fsync)
    ///
    /// # 错误处理
    /// - 文件打开失败(权限/路径不存在)→ `WalIoError`
    /// - 写入失败(磁盘满)→ `WalIoError`
    /// - fsync 失败 → `WalIoError`(数据可能未落盘,调用方应告警)
    ///
    /// # 参数
    /// - `entry`:WAL 条目(事务 ID + 状态 + 参与者 + 时间戳)
    pub async fn append(&self, entry: &WalEntry) -> Result<(), McpError> {
        let path = self.path.clone();
        let entry_bytes = serde_json::to_vec(entry).map_err(|e| McpError::WalIoError {
            reason: format!("WAL entry 序列化失败: {e}"),
        })?;
        // WHY clone entry 用于日志(不能借用进 spawn_blocking,因 spawn_blocking 要求 'static)
        let entry_clone = entry.clone();

        // WHY spawn_blocking:文件 open/write/sync_all 都是阻塞系统调用,
        // 在 async 上下文直接调用会阻塞 runtime 工作线程(§4.4 反模式 #2)
        let join_result = task::spawn_blocking(move || -> Result<(), McpError> {
            // 确保父目录存在(首次创建)
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| McpError::WalIoError {
                        reason: format!("创建 WAL 父目录失败: {}: {e}", parent.display()),
                    })?;
                }
            }

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| McpError::WalIoError {
                    reason: format!("打开 WAL 文件失败: {}: {e}", path.display()),
                })?;

            // 写入 JSON + 换行符(JSONL 格式)
            file.write_all(&entry_bytes)
                .map_err(|e| McpError::WalIoError {
                    reason: format!("WAL 写入失败: {e}"),
                })?;
            file.write_all(b"\n").map_err(|e| McpError::WalIoError {
                reason: format!("WAL 写入换行符失败: {e}"),
            })?;

            // fsync — 确保数据落盘,崩溃后可恢复
            // WHY 不用 sync_data(仅数据不含元数据):Linux 上 sync_data 可能不更新
            // 文件大小元数据,导致读取时截断。sync_all 虽稍慢但保证一致。
            file.sync_all().map_err(|e| McpError::WalIoError {
                reason: format!("WAL fsync 失败: {e}"),
            })?;

            debug!(
                path = %path.display(),
                transaction_id = %entry_clone.transaction_id,
                state = %entry_clone.state,
                "WAL entry 写入成功"
            );
            Ok(())
        })
        .await;

        // 处理 JoinError(panic/cancel),解包内层 Result
        match join_result {
            Ok(inner) => inner,
            Err(e) => Err(McpError::WalIoError {
                reason: format!("WAL append 任务 join 失败: {e}"),
            }),
        }
    }

    /// 读取所有 WAL entry(用于崩溃恢复)
    ///
    /// # 流程
    /// 1. `spawn_blocking` 中以只读方式打开文件
    /// 2. `BufReader::lines()` 逐行解析 JSON
    /// 3. 跳过空行与解析失败的行(损坏的尾行可能是崩溃时未写完整)
    ///
    /// # 错误处理
    /// - 文件不存在 → 返回空 Vec(首次启动,无历史 WAL)
    /// - 文件打开失败(权限) → `WalIoError`
    /// - 单行解析失败 → 跳过并告警(不阻塞恢复)
    ///
    /// # 返回
    /// 按写入顺序排列的 WalEntry 列表
    pub async fn read_all(&self) -> Result<Vec<WalEntry>, McpError> {
        let path = self.path.clone();

        let join_result = task::spawn_blocking(move || -> Result<Vec<WalEntry>, McpError> {
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // 文件不存在 = 首次启动或 WAL 已被 truncate,返回空
                    debug!(path = %path.display(), "WAL 文件不存在,无历史 entry");
                    return Ok(Vec::new());
                }
                Err(e) => {
                    return Err(McpError::WalIoError {
                        reason: format!("打开 WAL 文件失败: {}: {e}", path.display()),
                    });
                }
            };

            let reader = BufReader::new(file);
            let mut entries = Vec::new();
            for (line_no, line) in reader.lines().enumerate() {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => {
                        warn!(
                            path = %path.display(),
                            line_no,
                            error = %e,
                            "WAL 读取行失败,跳过"
                        );
                        continue;
                    }
                };
                // 跳过空行(JSONL 容忍尾行空行)
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<WalEntry>(&line) {
                    Ok(entry) => entries.push(entry),
                    Err(e) => {
                        // 尾行可能因崩溃未写完整,跳过而非报错
                        warn!(
                            path = %path.display(),
                            line_no,
                            error = %e,
                            "WAL entry 解析失败,跳过(可能是崩溃时未写完整)"
                        );
                    }
                }
            }
            debug!(
                path = %path.display(),
                count = entries.len(),
                "WAL 读取完成"
            );
            Ok(entries)
        })
        .await;

        match join_result {
            Ok(inner) => inner,
            Err(e) => Err(McpError::WalIoError {
                reason: format!("WAL read_all 任务 join 失败: {e}"),
            }),
        }
    }

    /// 清空 WAL 文件(恢复完成后调用,避免无限增长)
    ///
    /// # 流程
    /// 1. `spawn_blocking` 中以 truncate 模式打开文件(清空内容)
    /// 2. `sync_all` 确保元数据(文件大小=0)落盘
    ///
    /// # 错误处理
    /// - 文件不存在 → 视为成功(无需 truncate)
    /// - truncate 失败 → `WalIoError`(下次启动仍会读取旧 entry,可能重复恢复)
    pub async fn truncate(&self) -> Result<(), McpError> {
        let path = self.path.clone();

        let join_result = task::spawn_blocking(move || -> Result<(), McpError> {
            let file = match OpenOptions::new().write(true).truncate(true).open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // 文件不存在 = 已被 truncate 或从未创建,无操作
                    return Ok(());
                }
                Err(e) => {
                    return Err(McpError::WalIoError {
                        reason: format!("打开 WAL 文件 truncate 失败: {}: {e}", path.display()),
                    });
                }
            };
            file.sync_all().map_err(|e| McpError::WalIoError {
                reason: format!("WAL truncate fsync 失败: {e}"),
            })?;
            debug!(path = %path.display(), "WAL 已 truncate");
            Ok(())
        })
        .await;

        match join_result {
            Ok(inner) => inner,
            Err(e) => Err(McpError::WalIoError {
                reason: format!("WAL truncate 任务 join 失败: {e}"),
            }),
        }
    }

    /// 获取 WAL 文件路径(用于测试与日志)
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantum::transaction::TransactionState;

    /// 测试辅助:创建临时 WAL 文件路径
    fn make_temp_wal() -> (WalStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = dir.path().join("test.wal");
        (WalStore::new(path), dir)
    }

    #[tokio::test]
    async fn test_wal_append_and_read_single_entry() {
        let (store, _dir) = make_temp_wal();
        let entry = WalEntry::new("tx-1", TransactionState::Prepare, vec!["s-1".into()]);

        store.append(&entry).await.expect("append 失败");
        let entries = store.read_all().await.expect("read_all 失败");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].transaction_id, "tx-1");
        assert_eq!(entries[0].state, TransactionState::Prepare);
        assert_eq!(entries[0].participants_ack, vec!["s-1".to_string()]);
    }

    #[tokio::test]
    async fn test_wal_append_multiple_entries_preserves_order() {
        let (store, _dir) = make_temp_wal();
        let entries = vec![
            WalEntry::new("tx-1", TransactionState::Prepare, vec!["s-1".into()]),
            WalEntry::new("tx-1", TransactionState::Commit, vec!["s-1".into()]),
            WalEntry::new("tx-2", TransactionState::Prepare, vec!["s-2".into()]),
        ];

        for entry in &entries {
            store.append(entry).await.expect("append 失败");
        }
        let restored = store.read_all().await.expect("read_all 失败");
        assert_eq!(restored.len(), 3);
        assert_eq!(restored[0].transaction_id, "tx-1");
        assert_eq!(restored[0].state, TransactionState::Prepare);
        assert_eq!(restored[1].state, TransactionState::Commit);
        assert_eq!(restored[2].transaction_id, "tx-2");
    }

    #[tokio::test]
    async fn test_wal_read_empty_when_file_not_exists() {
        let (store, _dir) = make_temp_wal();
        let entries = store.read_all().await.expect("read_all 失败");
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_wal_truncate_clears_file() {
        let (store, _dir) = make_temp_wal();
        store
            .append(&WalEntry::new("tx-1", TransactionState::Prepare, vec![]))
            .await
            .expect("append 失败");
        assert_eq!(
            store.read_all().await.unwrap().len(),
            1,
            "append 后应有 1 条 entry"
        );

        store.truncate().await.expect("truncate 失败");
        assert!(
            store.read_all().await.unwrap().is_empty(),
            "truncate 后应无 entry"
        );
    }

    #[tokio::test]
    async fn test_wal_truncate_when_file_not_exists_succeeds() {
        let (store, _dir) = make_temp_wal();
        // 文件不存在时 truncate 应成功(幂等)
        store.truncate().await.expect("truncate 应成功");
    }

    #[tokio::test]
    async fn test_wal_creates_parent_directory() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let nested = dir.path().join("nested").join("subdir").join("test.wal");
        let store = WalStore::new(nested.clone());

        store
            .append(&WalEntry::new("tx-1", TransactionState::Commit, vec![]))
            .await
            .expect("append 应自动创建父目录");

        let entries = store.read_all().await.expect("read_all 失败");
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn test_wal_default_path_returns_some_on_unix_like() {
        // 在 Windows 上 HOME 不存在,但 USERPROFILE 可能存在
        // 此测试验证 default_path 不 panic,且返回的路径以正确文件名结尾
        if let Some(path) = WalStore::default_path() {
            assert!(path.ends_with(DEFAULT_WAL_FILENAME));
        }
        // 若 HOME/USERPROFILE 都不存在,返回 None 也是合法行为
    }

    #[tokio::test]
    async fn test_wal_recovery_scenario_prepare_only() {
        // 模拟崩溃场景:Prepare 写入后崩溃,Commit 未写入
        let (store, _dir) = make_temp_wal();
        store
            .append(&WalEntry::new(
                "tx-crash",
                TransactionState::Prepare,
                vec!["s-1".into(), "s-2".into()],
            ))
            .await
            .expect("append 失败");

        let entries = store.read_all().await.expect("read_all 失败");
        // 恢复逻辑测试:只有 Prepare entry 的事务需要重新 Commit
        let needs_commit: Vec<_> = entries
            .iter()
            .filter(|e| e.state == TransactionState::Prepare)
            .collect();
        assert_eq!(needs_commit.len(), 1);
        assert_eq!(needs_commit[0].transaction_id, "tx-crash");
        assert_eq!(needs_commit[0].participants_ack.len(), 2);
    }

    #[tokio::test]
    async fn test_wal_recovery_scenario_commit_present() {
        // 完整流程:Prepare + Commit 都已写入,恢复时应跳过
        let (store, _dir) = make_temp_wal();
        store
            .append(&WalEntry::new(
                "tx-done",
                TransactionState::Prepare,
                vec!["s-1".into()],
            ))
            .await
            .expect("Prepare append 失败");
        store
            .append(&WalEntry::new(
                "tx-done",
                TransactionState::Commit,
                vec!["s-1".into()],
            ))
            .await
            .expect("Commit append 失败");

        let entries = store.read_all().await.expect("read_all 失败");
        // 同一事务 ID 有 Prepare + Commit,恢复时应跳过(已完成)
        let tx_ids: Vec<_> = entries
            .iter()
            .filter(|e| e.state == TransactionState::Commit)
            .map(|e| &e.transaction_id)
            .collect();
        assert!(tx_ids.contains(&&"tx-done".to_string()));
    }
}
