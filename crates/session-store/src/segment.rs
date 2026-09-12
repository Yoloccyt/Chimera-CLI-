//! append-only 段 — 会话事件追加到 JSONL 段文件（含 WAL 意向 + 崩溃恢复）
//!
//! 对应架构层: **L3 Storage**（session-store,Phase 2 新增,ADR-141）
//! 对应任务: **P2-T2**（手册 W9 T-07 / v4.0 WI-18 存储面:append-only JSONL 段）
//!
//! # 段文件布局
//!
//! 每个会话按「每 Thread 一段」组织,文件名为 `<session_id>.<segment_index>.jsonl`
//! （`session_id` 含 `-`/`:` 等字符在 Windows 上不合法,故文件名用 `session_id` 的
//! URL 安全编码,详见 [`file_stem`]）。每条记录二进制布局:
//!
//! ```text
//! [u32 LE 长度前缀][JSON 字节][\n]
//! ```
//!
//! JSON 字节 = [`SegmentRecord`] 序列化（`{ seq, row, event }`,行内嵌
//! Offset 双键）——崩溃恢复逐条反序列化并校验 seq/row 连续性,不依赖
//! 调用方 start_seq 的恒等式重建（旧版按 start_seq 递增重建再比对是恒等式,
//! 校验形同虚设）。
//!
//! - 长度前缀 = JSON 字节长度（不含前缀与 \n 自身）——**WAL 意向记录**:
//!   追加先写前缀再写数据,崩溃时凭前缀检测「半条」
//! - `\n` 结尾保持 JSONL 可读性（去掉前缀即标准 JSONL,每行一条 JSON）
//!
//! # 崩溃恢复（不丢已确认事件）
//!
//! - `append` / `append_batch` 返回 Ok 前必须 `sync_all`（fsync 段尾部）,
//!   已确认事件在崩溃后不丢失
//! - 启动（`open_or_create`）时顺序扫描文件:长度前缀完整但数据不足
//!   （剩余字节 < 前缀指示长度,或不足 4 字节前缀）→ 截断到最后一个完整记录
//!   末尾（未完成尾部被丢弃——半条从未确认,丢弃不违反「不丢已确认事件」）
//! - 截断后重建 `row`（段内行号）与 `next_seq`（全局序列号）;调用方传入的
//!   `start_seq` 与恢复出的序列号不一致时返回 `StoreError::OffsetMismatch`
//!
//! # Offset 双键
//!
//! 每事件分配 `Offset { seq, row }`:seq 从 `start_seq` 起按会话全局单调
//! （跨段连续,滚动时由上层把旧段 `next_seq` 传给新段）,row 段内 0-based
//! 单调。严格单调性由追加路径保证（单写者）。

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::types::{Offset, SegmentId, SessionEvent, SessionId};

/// 单条记录 = 4 字节长度前缀 + JSON 字节 + 1 字节换行
const HEADER_LEN: u64 = 4;
/// 换行分隔符字节
const NEWLINE: u8 = b'\n';

/// 段文件中的单条记录 — JSON 行内嵌 Offset 双键
///
/// 序列化布局（JSONL 每行一条）:记录本身携带 seq/row,崩溃恢复逐条
/// 反序列化并校验连续性——不依赖调用方 `start_seq` 的恒等式重建;
/// 同时也是 P2-T3 k-way 归并回放的排序键来源（ADR-109）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[must_use]
pub struct SegmentRecord {
    /// 全局序列号（会话内单调,跨段连续）
    pub seq: u64,
    /// 段内行号（0-based,段内单调）
    pub row: u64,
    /// 事件本体
    pub event: SessionEvent,
}

/// 将 SessionId 编码为 URL 安全文件名字符串（Windows 文件名合法化）
///
/// WHY:UUID 短横线合法,但 session_id 可能来自外部协议（ThreadId 编码
/// goal_id+run_id 可能含 `:` `/` `\` 等 Windows 非法字符）,统一替换为 `~`。
#[must_use]
pub fn file_stem(session_id: &SessionId) -> String {
    let mut s = session_id.as_str().to_string();
    for ch in ['/', '\\', ':', '*', '?', '"', '<', '>', '|'] {
        s = s.replace(ch, "~");
    }
    s
}

/// 由会话 + 段索引推导段文件路径
#[must_use]
pub fn segment_path(data_dir: &Path, session_id: &SessionId, index: u32) -> PathBuf {
    data_dir.join(format!("{}.{}.jsonl", file_stem(session_id), index))
}

/// append-only 段写者 — 单会话单段的追加句柄
///
/// # 线程安全
/// 非 `Sync`（持有 `std::fs::File`）;由上层 `CbmrWriter` 经 `Mutex` 串行访问
/// （对齐 scc-cache `SqliteWal` 的 `Arc<Mutex<Connection>>` 范式）。
#[derive(Debug)]
pub struct SegmentWriter {
    session_id: SessionId,
    segment_index: u32,
    path: PathBuf,
    file: File,
    /// 段内下一行号（0-based,= 已写完整记录数）
    row: u64,
    /// 段内下一全局序列号（起始 = 构造时 `start_seq`,随追加递增）
    next_seq: u64,
    /// 段起始全局序列号（元数据 start_offset 用）
    first_seq: u64,
    /// 最近一次追加的 fsync 结果缓存（诊断用,避免重复 syscall）
    last_fsync_ok: bool,
    /// 测试注入:下一次 append_batch 的 write_all 模拟失败（仅 cfg(test) 编译）
    #[cfg(test)]
    fail_write: bool,
    /// 测试注入:下一次 append_batch 的 sync_all 模拟失败（仅 cfg(test) 编译）
    #[cfg(test)]
    fail_sync: bool,
}

impl SegmentWriter {
    /// 打开或创建段文件（`<session_id>.<segment_index>.jsonl`）,含崩溃恢复
    ///
    /// # 参数
    /// - `start_seq`:该段首个事件的全局序列号（新会话第 0 段为 0;
    ///   滚动段为旧段 `next_seq`）
    ///
    /// # 崩溃恢复行为
    /// 文件已存在 → 扫描截断未完成尾部,逐条反序列化校验 seq/row 连续性;
    /// 文件内首条记录 seq 与 `start_seq` 不一致 → `OffsetMismatch`
    /// （调用方传错起始序列号,属编程错误而非磁盘损坏）。
    pub fn open_or_create(
        data_dir: &Path,
        session_id: &SessionId,
        segment_index: u32,
        start_seq: u64,
    ) -> Result<Self, StoreError> {
        let path = segment_path(data_dir, session_id, segment_index);
        // WHY create(true) 而非 create_new:崩溃后重启必须复用既有文件
        // （不截断,交给 recover_tail 决定截断点）;create_new 会因文件存在失败。
        // truncate(false) 显式声明保留旧文件（append-only 追加语义,不覆盖）
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| StoreError::Io {
                context: format!("打开段文件 {} 失败", path.display()),
                source: e,
            })?;

        let mut writer = Self {
            session_id: session_id.clone(),
            segment_index,
            path,
            file,
            row: 0,
            next_seq: start_seq,
            first_seq: start_seq,
            last_fsync_ok: true,
            #[cfg(test)]
            fail_write: false,
            #[cfg(test)]
            fail_sync: false,
        };
        // WHY 校验内化:recover_tail 逐条反序列化并校验 seq/row 连续性——
        // 文件非空时首条记录 seq 必须等于 start_seq;空文件 next_seq ==
        // start_seq 恒成立,无需后置比对（旧版按 start_seq 递增重建再比对
        // 是恒等式,任何错误起始值都检查不出）
        writer.recover_tail()?;
        Ok(writer)
    }

    /// 追加单条事件 → Offset（严格单调）
    ///
    /// # 持久化保证
    /// 返回 Ok 前已 `sync_all`（fsync 段尾部）——已确认事件崩溃不丢。
    pub fn append(&mut self, ev: &SessionEvent) -> Result<Offset, StoreError> {
        let offsets = self.append_batch(std::slice::from_ref(ev))?;
        offsets
            .into_iter()
            .next()
            .ok_or_else(|| StoreError::InvalidInput {
                reason: "append_batch 返回空 Offset 列表".into(),
            })
    }

    /// 批量追加事件 → 每个事件的 Offset（微批写核心:一次 write_all + 一次 fsync）
    ///
    /// # WHY 批量
    /// N 条事件共享一次 `write_all` 与一次 `sync_all`:syscall 从 2N 降至 2,
    /// 是 CBMR 微批写（ADR-108）的段面收益——配合 SQLite 单事务写入,
    /// 全链路写放大从「每事件 2 次 syscall」降为「每批 2 次 syscall」。
    pub fn append_batch(&mut self, events: &[SessionEvent]) -> Result<Vec<Offset>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        // 失败回滚:批次开始快照 (start_seq, start_row)。WHY:若不回滚,
        // write_all / sync_all 失败后内存状态已推进而磁盘未落,调用方忽略
        // Err 继续用同一 writer 追加会产生 offset 跳跃(磁盘上 seq/row 断档)。
        // 回滚到批次前状态后,追加继续从快照处分配 offset,保持严格单调。
        let start_seq = self.next_seq;
        let start_row = self.row;
        let mut buf = Vec::with_capacity(events.len() * 64);
        let mut offsets = Vec::with_capacity(events.len());
        for ev in events {
            let offset = Offset::new(self.next_seq, self.row);
            offsets.push(offset);
            // JSON 行内嵌 Offset 双键(seq/row):崩溃恢复可校验连续性,
            // 不依赖调用方 start_seq 的「恒等式重建」
            let record = SegmentRecord {
                seq: self.next_seq,
                row: self.row,
                event: ev.clone(),
            };
            let json = serde_json::to_vec(&record).map_err(|e| StoreError::Serialization {
                reason: format!("SegmentRecord JSON 序列化失败: {e}"),
            })?;
            let len = u32::try_from(json.len()).map_err(|_| StoreError::InvalidInput {
                reason: format!("事件 JSON 超长 ({} bytes > u32::MAX)", json.len()),
            })?;
            // WAL 意向记录:先写长度前缀,再写数据
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&json);
            buf.push(NEWLINE);
            self.next_seq += 1;
            self.row += 1;
        }
        // 测试注入:write_all 模拟失败（等价于真实 IO 错误,验证回滚路径）
        #[cfg(test)]
        if std::mem::take(&mut self.fail_write) {
            self.rollback_batch(start_seq, start_row);
            return Err(StoreError::Io {
                context: format!(
                    "注入 write_all 失败(测试):追加 {} 条事件到 {}",
                    events.len(),
                    self.path.display()
                ),
                source: std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "测试注入的写入失败",
                ),
            });
        }
        if let Err(e) = self.file.write_all(&buf) {
            self.rollback_batch(start_seq, start_row);
            return Err(StoreError::Io {
                context: format!(
                    "追加 {} 条事件到 {} 失败",
                    events.len(),
                    self.path.display()
                ),
                source: e,
            });
        }
        // 测试注入:sync_all 模拟失败（验证回滚路径）
        #[cfg(test)]
        if std::mem::take(&mut self.fail_sync) {
            self.rollback_batch(start_seq, start_row);
            return Err(StoreError::Io {
                context: format!(
                    "注入 sync_all 失败(测试):fsync 段文件 {}",
                    self.path.display()
                ),
                source: std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "测试注入的 fsync 失败",
                ),
            });
        }
        // fsync 段尾部:append 返回 Ok 前必须落盘（不丢已确认事件）
        if let Err(e) = self.file.sync_all() {
            self.rollback_batch(start_seq, start_row);
            return Err(StoreError::Io {
                context: format!("fsync 段文件 {} 失败", self.path.display()),
                source: e,
            });
        }
        self.last_fsync_ok = true;
        Ok(offsets)
    }

    /// 失败回滚 — 恢复 append_batch 批次前内存状态
    ///
    /// 仅回滚内存状态（next_seq/row/fsync 标志）:磁盘未写入该批次,offset
    /// 连续性由内存快照保证;真实 write_all 部分写入的极端场景由崩溃恢复
    /// （[`Self::recover_tail`] 半条截断）兜底,调用方应在写失败后停止使用
    /// 该 writer（或以 Err 传播中止本次写路径）。
    fn rollback_batch(&mut self, start_seq: u64, start_row: u64) {
        self.next_seq = start_seq;
        self.row = start_row;
        self.last_fsync_ok = false;
    }

    /// 测试注入:设置下一次 append_batch 的失败点（仅测试编译,生产不可见）
    #[cfg(test)]
    pub fn inject_failure(&mut self, fail_write: bool, fail_sync: bool) {
        self.fail_write = fail_write;
        self.fail_sync = fail_sync;
    }

    /// 段内已写行数
    #[must_use]
    pub fn row_count(&self) -> u64 {
        self.row
    }

    /// 段内下一全局序列号（滚动段时传给新段作为 start_seq）
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// 段起始全局序列号（元数据 start_offset）
    #[must_use]
    pub fn first_seq(&self) -> u64 {
        self.first_seq
    }

    /// 段索引
    #[must_use]
    pub fn segment_index(&self) -> u32 {
        self.segment_index
    }

    /// 所属会话
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// 段文件路径
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 最近一次 fsync 是否成功（诊断用）
    #[must_use]
    pub fn last_fsync_ok(&self) -> bool {
        self.last_fsync_ok
    }

    /// 段元数据（SQLite segments 表行）
    ///
    /// `end_offset` = 段内最后一条事件的 seq（无事件时等于 next_seq - 1 的
    /// 饱和语义,调用方保证只对非空段 upsert）。
    #[must_use]
    pub fn meta(&self, segment_id: SegmentId, parent_segment_id: Option<SegmentId>) -> SegmentMeta {
        let end = if self.row == 0 {
            self.first_seq
        } else {
            self.next_seq - 1
        };
        SegmentMeta {
            segment_id,
            session_id: self.session_id.clone(),
            segment_index: self.segment_index,
            parent_segment_id,
            start_offset: self.first_seq,
            end_offset: end,
        }
    }

    // ============================================================
    // 崩溃恢复（内部）
    // ============================================================

    /// 扫描段文件,截断未完成尾部,重建 row/next_seq
    ///
    /// # 算法
    /// 顺序读取:每读到 4 字节长度前缀 `n`:
    /// - 剩余字节 >= n → 该条完整,跳过（含 \n）,row/next_seq 递增
    /// - 剩余字节 < n → 半条（前缀写了但数据没写全）→ 截断到该条起始
    /// - 读前缀不足 4 字节（文件尾残留 1-3 字节）→ 截断到该条起始
    ///
    /// 截断目标 = 最后一个完整记录的结束偏移;截断后重建的 row/next_seq
    /// 与已确认事件一致（半条从未确认）。
    fn recover_tail(&mut self) -> Result<(), StoreError> {
        let meta = self.file.metadata().map_err(|e| StoreError::Io {
            context: format!("读取段文件 {} 元数据失败", self.path.display()),
            source: e,
        })?;
        let file_len = meta.len();
        // WHY seek 到 0:句柄由 append 使用,恢复前必须回到文件头
        self.file
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| StoreError::Io {
                context: format!("seek 段文件 {} 失败", self.path.display()),
                source: e,
            })?;

        // 期望的连续性:首条 seq = first_seq(= 调用方 start_seq),row 从 0 起
        let mut expect_seq: u64 = self.first_seq;
        let mut complete_end: u64 = 0;
        let mut row: u64 = 0;
        let mut next_seq: u64 = self.first_seq;
        let mut header = [0u8; HEADER_LEN as usize];

        loop {
            // 读长度前缀
            let read = match self.file.read(&mut header) {
                Ok(0) => 0, // EOF:干净结束
                Ok(n) => {
                    if n < HEADER_LEN as usize {
                        // 前缀本身半条（崩溃发生在写前缀中途）
                        break;
                    }
                    n
                }
                Err(e) => {
                    return Err(StoreError::Io {
                        context: format!("扫描段文件 {} 读长度前缀失败", self.path.display()),
                        source: e,
                    });
                }
            };
            if read == 0 {
                break;
            }
            let len = u32::from_le_bytes(header) as u64;
            let data_start = complete_end + HEADER_LEN;
            // 数据 + \n 是否完整
            if data_start + len + 1 > file_len {
                // 半条:长度前缀写全但数据（或 \n）未写全 → 截断到本条起始
                warn!(
                    "段文件 {} 尾部含半条记录 (offset={data_start}, 期望 {len}+1 字节, 实际剩余 {}),截断",
                    self.path.display(),
                    file_len - data_start
                );
                break;
            }
            // 读数据并反序列化(校验 seq/row 连续性)
            let mut body = vec![0u8; len as usize];
            self.file
                .read_exact(&mut body)
                .map_err(|e| StoreError::Io {
                    context: format!("读取段文件 {} 记录体失败", self.path.display()),
                    source: e,
                })?;
            let record: SegmentRecord =
                serde_json::from_slice(&body).map_err(|e| StoreError::WalCorrupt {
                    path: self.path.display().to_string(),
                    reason: format!("记录 JSON 解析失败: {e}"),
                })?;
            if record.seq != expect_seq || record.row != row {
                // 序列号不连续:文件被外部篡改 / 调用方 start_seq 传错
                return Err(StoreError::OffsetMismatch {
                    expected: expect_seq,
                    actual: record.seq,
                });
            }
            // 跳过 \n
            self.file
                .seek(std::io::SeekFrom::Current(1))
                .map_err(|e| StoreError::Io {
                    context: format!("跳过段文件 {} 记录换行失败", self.path.display()),
                    source: e,
                })?;
            complete_end = data_start + len + 1;
            row += 1;
            next_seq += 1;
            expect_seq += 1;
        }

        // 截断未完成尾部（若存在）
        if file_len > complete_end {
            self.file
                .set_len(complete_end)
                .map_err(|e| StoreError::Io {
                    context: format!(
                        "截断层文件 {} 到 {} 字节失败",
                        self.path.display(),
                        complete_end
                    ),
                    source: e,
                })?;
            info!(
                "段文件 {} 崩溃恢复:截断 {} 字节半条尾部,保留 {row} 条完整记录",
                self.path.display(),
                file_len - complete_end
            );
        }

        // 重建状态 + 句柄定位到文件尾
        self.file
            .seek(std::io::SeekFrom::Start(complete_end))
            .map_err(|e| StoreError::Io {
                context: format!("seek 段文件 {} 到文件尾失败", self.path.display()),
                source: e,
            })?;
        self.row = row;
        self.next_seq = next_seq;
        Ok(())
    }
}

/// 只读段文件读取器 — 顺序读取长度前缀记录（P2-T3 k-way 归并回放的读取源）
///
/// # WHY 与 SegmentWriter 分离
/// 回放（replay）与索引重建（rebuild_index）需要**并发只读**段文件,不应持有
/// 写句柄;独立的只读游标使 k-way 归并可同时打开 k 个段文件（每段一个游标）。
///
/// # 半条语义（对齐 recover_tail）
/// 崩溃窗口内段文件尾部可能残留半条（长度前缀写全、数据未写全）。半条从未
/// fsync 确认,只读扫描**停止于最后一条完整记录**（不报错、不截断——只读器
/// 无写权限,截断交由 SegmentWriter::open_or_create 的 recover_tail 负责）。
#[derive(Debug)]
pub struct SegmentFileReader {
    file: BufReader<File>,
    path: PathBuf,
    /// 已读完整记录数（诊断/统计用）
    rows: u64,
}

impl SegmentFileReader {
    /// 打开段文件（只读;文件不存在 → `NotFound`）
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let file = File::open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotFound {
                    what: format!("段文件 {} 不存在", path.display()),
                }
            } else {
                StoreError::Io {
                    context: format!("打开段文件 {} 失败", path.display()),
                    source: e,
                }
            }
        })?;
        Ok(Self {
            file: BufReader::new(file),
            path: path.to_path_buf(),
            rows: 0,
        })
    }

    /// 读取下一条完整记录（EOF → None;半条 → warn 并停止返回 None）
    ///
    /// # 返回语义
    /// `Ok(None)` 表示已无更多**已确认**记录（正常 EOF 或半条截断点）。
    pub fn next_record(&mut self) -> Result<Option<SegmentRecord>, StoreError> {
        let mut header = [0u8; HEADER_LEN as usize];
        // WHY 循环读满 header 而非单次 read:BufReader::read 在内部缓冲区
        // 剩余不足时只返回剩余部分（fill_buf 见缓冲区非空不触发底层补充
        // 读取）,单次 read 可能返回 1-3 字节——若按「n < 4 即半条前缀」
        // 判定,记录边界处会提前停止扫描（bench 复现:万条段文件只读出
        // 164 条,且停止位置随记录长度非确定变化）。循环直到读满 4 字节
        // 或 EOF,才能区分「干净 EOF」与「半条前缀」。
        let mut filled = 0usize;
        while filled < HEADER_LEN as usize {
            match self.file.read(&mut header[filled..]) {
                Ok(0) => {
                    if filled == 0 {
                        return Ok(None); // 干净 EOF
                    }
                    // 前缀本身半条（崩溃发生在写前缀中途）→ 停止
                    warn!(
                        "段文件 {} 尾部残留 {} 字节半条前缀,只读扫描停止",
                        self.path.display(),
                        filled
                    );
                    return Ok(None);
                }
                Ok(n) => filled += n,
                Err(e) => {
                    return Err(StoreError::Io {
                        context: format!("读段文件 {} 长度前缀失败", self.path.display()),
                        source: e,
                    });
                }
            }
        }
        let len = u32::from_le_bytes(header) as usize;
        let mut body = vec![0u8; len];
        if let Err(e) = self.file.read_exact(&mut body) {
            // 数据不足:半条（前缀写全、数据未写全）→ 停止
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                warn!(
                    "段文件 {} 尾部半条记录(期望 {len} 字节数据,实际不足),只读扫描停止",
                    self.path.display()
                );
                return Ok(None);
            }
            return Err(StoreError::Io {
                context: format!("读段文件 {} 记录体失败", self.path.display()),
                source: e,
            });
        }
        // 换行分隔符（JSONL 可读性;缺失视为半条）
        let mut nl = [0u8; 1];
        if let Err(e) = self.file.read_exact(&mut nl) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                warn!(
                    "段文件 {} 记录缺少换行结尾(半条),只读扫描停止",
                    self.path.display()
                );
                return Ok(None);
            }
            return Err(StoreError::Io {
                context: format!("读段文件 {} 换行分隔符失败", self.path.display()),
                source: e,
            });
        }
        let record: SegmentRecord =
            serde_json::from_slice(&body).map_err(|e| StoreError::WalCorrupt {
                path: self.path.display().to_string(),
                reason: format!("记录 JSON 解析失败: {e}"),
            })?;
        self.rows += 1;
        Ok(Some(record))
    }

    /// 已读完整记录数（诊断/统计）
    #[must_use]
    pub fn rows_read(&self) -> u64 {
        self.rows
    }
}

/// 枚举会话的段文件索引（升序）— 权威源是段文件,不依赖 SQLite 索引
///
/// # WHY（P2-T3 续写幂等）
/// writer 重开时需恢复**最后一个非空段**（而非固定段 0——段 0 满后滚动过,
/// 重开从段 0 续写会在滚动到已存在段时 OffsetMismatch）。段文件枚举是
/// 权威源:索引缺行（崩溃后未 rebuild）也能正确定位续写段。
/// 返回空 Vec = 会话无任何段文件（新会话）。
pub fn list_segment_files(data_dir: &Path, session_id: &SessionId) -> Result<Vec<u32>, StoreError> {
    let stem = file_stem(session_id);
    let prefix = format!("{stem}.");
    let mut idxs: Vec<u32> = Vec::new();
    let entries = std::fs::read_dir(data_dir).map_err(|e| StoreError::Io {
        context: format!("读取数据目录 {} 失败(枚举段文件)", data_dir.display()),
        source: e,
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| StoreError::Io {
            context: "读取数据目录条目失败(枚举段文件)".into(),
            source: e,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rest = name
            .strip_prefix(&prefix)
            .and_then(|r| r.strip_suffix(".jsonl"));
        if let Some(idx_str) = rest {
            if let Ok(idx) = idx_str.parse::<u32>() {
                idxs.push(idx);
            }
        }
    }
    idxs.sort_unstable();
    Ok(idxs)
}

/// 段元数据 — SQLite `segments` 表行（tree.rs 使用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentMeta {
    /// 段主键（全局唯一）
    pub segment_id: SegmentId,
    /// 所属会话
    pub session_id: SessionId,
    /// 会话内段索引（0-based）
    pub segment_index: u32,
    /// fork 引用:父会话段 ID（非 fork 段为 None）
    pub parent_segment_id: Option<SegmentId>,
    /// 段内起始全局序列号
    pub start_offset: u64,
    /// 段内结束全局序列号（含）
    pub end_offset: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StoreConfig;

    /// 构造测试事件（event_type 带序号便于断言顺序）
    fn ev(i: u64) -> SessionEvent {
        SessionEvent::with_payload(format!("ev-{i}"), vec![i as u8])
    }

    #[test]
    fn file_stem_sanitizes_windows_illegal_chars() {
        let sid = SessionId::new("goal:run/with\\bad*chars");
        let stem = file_stem(&sid);
        assert!(!stem.contains(['/', '\\', ':', '*']));
        assert_eq!(stem, "goal~run~with~bad~chars");
    }

    #[test]
    fn append_offsets_strictly_monotonic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("mono-test");
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
        let mut prev: Option<Offset> = None;
        for i in 0..16 {
            let off = w.append(&ev(i)).expect("append");
            // Offset 严格递增(seq 与 row 同步 +1)
            if let Some(p) = prev {
                assert!(off > p, "Offset 必须严格递增: {off:?} after {p:?}");
                assert_eq!(off.seq, p.seq + 1);
                assert_eq!(off.row, p.row + 1);
            }
            prev = Some(off);
        }
        assert_eq!(w.row_count(), 16);
        assert_eq!(w.next_seq(), 16);
    }

    #[test]
    fn segment_rolls_on_threshold() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = StoreConfig::test_config(dir.path());
        let sid = SessionId::new("roll-test");
        // 第 0 段:max_rows=8,写 8 条后滚动
        let mut seg0 = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open seg0");
        for i in 0..config.max_rows_per_segment {
            seg0.append(&ev(i)).expect("append");
        }
        assert_eq!(seg0.row_count(), config.max_rows_per_segment);
        assert_eq!(seg0.next_seq(), config.max_rows_per_segment);
        // 滚动:新段 start_seq = 旧段 next_seq
        let mut seg1 =
            SegmentWriter::open_or_create(dir.path(), &sid, 1, seg0.next_seq()).expect("open seg1");
        assert_eq!(seg1.row_count(), 0);
        let off = seg1.append(&ev(100)).expect("append seg1");
        assert_eq!(off.seq, config.max_rows_per_segment, "seq 跨段连续");
        assert_eq!(off.row, 0, "row 段内重新编号");
        assert!(seg0.path().exists());
        assert!(seg1.path().exists());
    }

    #[test]
    fn crash_recovery_truncates_half_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("crash-test");
        let path = segment_path(dir.path(), &sid, 0);

        // 写入 3 条完整记录 + 1 条半条(长度前缀写全 + 数据只写一半)
        {
            let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
            for i in 0..3 {
                w.append(&ev(i)).expect("append");
            }
            // 模拟崩溃:直接操作文件追加半条(前缀 + 半截数据)
            use std::io::Write as _;
            let mut f = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open raw");
            let json = serde_json::to_vec(&ev(99)).expect("json");
            f.write_all(&(json.len() as u32).to_le_bytes())
                .expect("prefix");
            let half = &json[..json.len() / 2];
            f.write_all(half).expect("half data");
            f.sync_all().expect("fsync");
        }

        // 重启:恢复截断半条,已确认的 3 条保留
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("reopen");
        assert_eq!(w.row_count(), 3, "半条被截断,仅保留 3 条已确认");
        assert_eq!(w.next_seq(), 3, "seq 回退到 3(半条未确认不占号)");
        // 截断后可继续追加,seq 从 3 继续
        let off = w.append(&ev(3)).expect("append after recover");
        assert_eq!(off.seq, 3);
        assert_eq!(off.row, 3);
        assert_eq!(w.row_count(), 4);
    }

    #[test]
    fn crash_recovery_truncates_half_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("crash-prefix");
        let path = segment_path(dir.path(), &sid, 0);

        {
            let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
            w.append(&ev(0)).expect("append");
            // 崩溃发生在写长度前缀中途:只写 2 字节
            use std::io::Write as _;
            let mut f = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open raw");
            f.write_all(&[0x10, 0x00]).expect("half prefix");
            f.sync_all().expect("fsync");
        }

        let w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("reopen");
        assert_eq!(w.row_count(), 1, "前缀半条被截断");
        assert_eq!(w.next_seq(), 1);
    }

    #[test]
    fn reopen_after_clean_close_preserves_all() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("clean-close");
        {
            let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
            for i in 0..5 {
                w.append(&ev(i)).expect("append");
            }
        } // drop(关闭句柄)
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("reopen");
        assert_eq!(w.row_count(), 5);
        assert_eq!(w.next_seq(), 5);
        let off = w.append(&ev(5)).expect("append");
        assert_eq!(off.seq, 5);
    }

    #[test]
    fn start_seq_mismatch_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("mismatch");
        {
            let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
            w.append(&ev(0)).expect("append");
        }
        // 重开时传错 start_seq(应为 1,传 7)→ OffsetMismatch
        let err = SegmentWriter::open_or_create(dir.path(), &sid, 0, 7)
            .expect_err("必须报 OffsetMismatch");
        match err {
            StoreError::OffsetMismatch { expected, actual } => {
                // 文件内首条记录实际 seq=0,与调用方期望 7 不符
                assert_eq!(expected, 7);
                assert_eq!(actual, 0);
            }
            other => panic!("期望 OffsetMismatch,实际 {other:?}"),
        }
    }

    #[test]
    fn append_batch_offsets_and_file_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("batch-test");
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
        let events: Vec<SessionEvent> = (0..10).map(ev).collect();
        let offsets = w.append_batch(&events).expect("batch append");
        assert_eq!(offsets.len(), 10);
        for (i, off) in offsets.iter().enumerate() {
            assert_eq!(off.seq, i as u64);
            assert_eq!(off.row, i as u64);
        }
        assert_eq!(w.row_count(), 10);
        // 空批是 no-op
        let empty = w.append_batch(&[]).expect("empty batch");
        assert!(empty.is_empty());
        assert_eq!(w.row_count(), 10);
    }

    #[test]
    fn append_batch_write_failure_rolls_back_state() {
        // 模拟 write_all 失败:内存状态必须回滚到批次前,offset 不跳跃
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("rollback-write");
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
        // 先成功追加 2 条:状态推进到 seq=2 / row=2
        let ok = w.append_batch(&[ev(0), ev(1)]).expect("batch ok");
        assert_eq!(ok.len(), 2);
        assert_eq!(w.next_seq(), 2);
        assert_eq!(w.row_count(), 2);
        assert!(w.last_fsync_ok());

        // 注入下一次 write_all 失败
        w.inject_failure(true, false);
        let err = w.append_batch(&[ev(2), ev(3)]).expect_err("write 必须失败");
        match err {
            StoreError::Io { .. } => {}
            other => panic!("期望 StoreError::Io,实际 {other:?}"),
        }
        // 状态回滚到批次前,offset 不跳跃
        assert_eq!(w.next_seq(), 2, "write 失败后 next_seq 必须回滚");
        assert_eq!(w.row_count(), 2, "write 失败后 row 必须回滚");
        assert!(!w.last_fsync_ok(), "失败后 fsync 标志必须复位");

        // 回滚后可继续追加,offset 从快照处连续分配(无断档)
        let off = w.append(&ev(2)).expect("append after rollback");
        assert_eq!(off.seq, 2);
        assert_eq!(off.row, 2);
        assert_eq!(w.next_seq(), 3);
        assert_eq!(w.row_count(), 3);

        // 文件内容只含 3 条(失败的 2 条从未落盘)
        let path = segment_path(dir.path(), &sid, 0);
        let bytes = std::fs::read(&path).expect("read file");
        let mut pos = 0usize;
        let mut rows = 0u32;
        while pos < bytes.len() {
            let len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().expect("len")) as usize;
            pos += 4;
            let record: SegmentRecord =
                serde_json::from_slice(&bytes[pos..pos + len]).expect("record json");
            assert_eq!(record.seq, rows as u64, "文件内 seq 连续无跳跃");
            assert_eq!(record.row, rows as u64);
            pos += len;
            pos += 1; // \n
            rows += 1;
        }
        assert_eq!(rows, 3, "失败批次(2 条)不得出现在文件内");
    }

    #[test]
    fn append_batch_sync_failure_rolls_back_state() {
        // 模拟 sync_all 失败:内存状态必须回滚(已确认=段 fsync 语义的前提)
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("rollback-sync");
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
        w.append(&ev(0)).expect("append");
        assert_eq!(w.next_seq(), 1);

        // 注入下一次 sync_all 失败
        w.inject_failure(false, true);
        let err = w.append_batch(&[ev(1), ev(2)]).expect_err("sync 必须失败");
        match err {
            StoreError::Io { .. } => {}
            other => panic!("期望 StoreError::Io,实际 {other:?}"),
        }
        assert_eq!(w.next_seq(), 1, "sync 失败后 next_seq 必须回滚");
        assert_eq!(w.row_count(), 1, "sync 失败后 row 必须回滚");
        assert!(!w.last_fsync_ok());

        // 回滚后可继续追加,offset 连续
        let off = w.append(&ev(1)).expect("append after rollback");
        assert_eq!(off.seq, 1);
        assert_eq!(off.row, 1);
        assert_eq!(w.next_seq(), 2);
    }

    #[test]
    fn segment_file_is_readable_jsonl() {
        // 验证长度前缀剥离后每行是可解析 JSON(人读审计可读性)
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("jsonl-test");
        let path = segment_path(dir.path(), &sid, 0);
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
        w.append(&ev(1)).expect("append");
        w.append(&ev(2)).expect("append");

        let bytes = std::fs::read(&path).expect("read file");
        // 跳过 4 字节前缀,读到 \n 为止,校验 JSON 可解析
        let mut pos = 0usize;
        let mut rows = 0u32;
        while pos < bytes.len() {
            let len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().expect("len")) as usize;
            pos += 4;
            let json = &bytes[pos..pos + len];
            // 每行 = SegmentRecord JSON(内嵌 seq/row + event)
            let record: SegmentRecord =
                serde_json::from_slice(json).expect("每行必须是合法 SegmentRecord JSON");
            assert_eq!(record.seq, rows as u64, "记录内嵌 seq 与行号一致");
            assert_eq!(record.row, rows as u64, "记录内嵌 row 与行号一致");
            assert_eq!(record.event.event_type, format!("ev-{}", rows + 1));
            pos += len;
            assert_eq!(bytes[pos], NEWLINE, "记录以换行结尾");
            pos += 1;
            rows += 1;
        }
        assert_eq!(rows, 2);
    }

    #[test]
    fn meta_reflects_segment_bounds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("meta-test");
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
        w.append(&ev(0)).expect("append");
        w.append(&ev(1)).expect("append");
        let seg_id = SegmentId::generate();
        let m = w.meta(seg_id.clone(), None);
        assert_eq!(m.segment_id, seg_id);
        assert_eq!(m.session_id, sid);
        assert_eq!(m.segment_index, 0);
        assert_eq!(m.start_offset, 0);
        assert_eq!(m.end_offset, 1);
        assert!(m.parent_segment_id.is_none());
    }
}
