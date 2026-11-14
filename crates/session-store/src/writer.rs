//! CbmrWriter — CBMR 微批写器（手册 §10.4 / ADR-108 / T-07 核心）
//!
//! 对应架构层: **L3 Storage**（session-store,Phase 2 新增,ADR-141）
//! 对应任务: **P2-T2**（手册 W9 §10.4 CbmrWriter 骨架:≤64/2ms/spawn_blocking）
//!
//! # 微批写语义（≤64 条 / 2ms 自适应窗口）
//!
//! - pending 队列攒批:事件先入内存队列（`append` 只入队,不碰磁盘）
//! - **满 64 立即刷**:队列长度达到 `batch_size` 即触发 flush,不等窗口
//! - **窗口到期刷**:后台循环每窗口时长检查一次,有 pending 即 flush
//! - **自适应窗口**:窗口按近期批大小在 1-4ms 间调整——批大（吞吐高）缩窗
//!   至 1ms 降低延迟,批小（吞吐低）扩窗至 4ms 攒批减少 IO 次数
//!   （"自适应"语义 = 窗口随吞吐负反馈,文档注明）
//!
//! # 红线 3:SQLite 只经 spawn_blocking
//!
//! - `flush` 的全部同步工作（段文件追加 + SQLite 单事务写入）封装为
//!   `flush_sync`,经 `run_blocking` 在 `tokio::task::spawn_blocking`
//!   阻塞池执行——async runtime 内绝不直接调用 SQLite
//! - **无 runtime 降级同步**:`run_blocking` 检测 `Handle::try_current()`,
//!   无 tokio runtime 环境（同步测试 / `futures::executor::block_on` 驱动）
//!   直接同步执行闭包,不 spawn（文档注明降级路径;同步测试走 `flush_sync`）
//!
//! # 禁持锁跨 await
//!
//! 所有 `std::sync::Mutex` 锁（pending / sessions / ema）在同步段内获取并
//! 立即释放,锁从不跨 `await` 点持有;`await` 只发生在 spawn_blocking join。
//! 锁序固定:sessions → SQLite conn（read_events 只持 conn）,无循环等待。
//!
//! # 读写分区（ADR-108）
//!
//! - 写路径:微批（段追加一次 fsync + SQLite 单事务）
//! - 读路径:直接走 SQLite 树索引（`read_events` 同样经 spawn_blocking,
//!   与写并发互不阻塞——WAL 模式读-写并发）

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::{spawn_blocking, JoinHandle};
use tracing::{debug, warn};

use crate::error::StoreError;
use crate::segment::{
    list_segment_files, segment_path, SegmentFileReader, SegmentMeta, SegmentWriter,
};
use crate::tree::{ContentProbeReport, EventRow, IndexRepairEntry, StoredEvent, TreeIndex};
use crate::types::{Offset, SegmentId, SessionEvent, SessionId, StoreConfig};

/// CBMR 微批写器 — 会话事件的攒批写入口（Clone 廉价,跨任务共享）
#[derive(Clone)]
pub struct CbmrWriter {
    inner: Arc<CbmrInner>,
}

struct CbmrInner {
    config: StoreConfig,
    /// SQLite 树索引（读/写共用,WAL 模式读-写并发）
    tree: TreeIndex,
    /// 会话 → 当前段写者（惰性创建:首次 flush 时 open_or_create 段 0）
    sessions: Mutex<HashMap<SessionId, SessionState>>,
    /// 攒批队列（快照即释放,不跨 await 持锁）
    pending: Mutex<PendingBatch>,
    /// 近期批大小 EMA（自适应窗口的输入;初始 0 = 冷启动按低吞吐扩窗）
    ema_batch: Mutex<f64>,
    /// 会话最近一次 flush 的末位 Offset（断点续读/回放定位;仅本实例视角）
    ///
    /// WHY（P2-T3 续写幂等）：flush 成功后更新,失败不推进（未落盘不算
    /// 已确认）;重开后首个 flush 前为空 → [`CbmrWriter::last_offset`] 返回
    /// None,调用方应先 flush 再查询。
    last_offset_by_session: Mutex<HashMap<SessionId, Offset>>,
    /// 关闭标志（M50）——置位后后台刷写循环在下一个窗口退出
    ///
    /// WHY 与 `Drop` 共用:Drop 不能 await,所以它只能“置标志 + abort”;
    /// 想保证「最后一批也落盘」必须走可 await 的 [`CbmrWriter::shutdown`]。
    closing: AtomicBool,
    /// 后台刷写循环句柄（M50）——仅构造时写入一次,shutdown/Drop 取走并 abort
    ///
    /// WHY 放 `CbmrInner` 而不是 `CbmrWriter`:
    /// `CbmrWriter` 是 `Clone`（廉价 Arc 克隆）,若把句柄/Drop 挂在它上面,
    /// 任一副本析构就会误杀共享循环。挂在 `CbmrInner` 上则只有**最后一个**
    /// Arc 释放时才会跑 Drop——与所有权的真实语义一致。
    flush_loop: Mutex<Option<JoinHandle<()>>>,
    /// 打开时索引尾部探针的自愈报告(R11 W3a / ADR-295 D4;仅构造时写入一次)
    startup_repair: Mutex<Vec<IndexRepairEntry>>,
}

/// 单会话的段写状态
struct SessionState {
    /// 当前段写者
    current: SegmentWriter,
    /// 当前段的 SQLite 主键（滚动时重新生成）
    current_segment_id: SegmentId,
}

/// 攒批队列（pending）
#[derive(Default)]
struct PendingBatch {
    events: Vec<PendingEvent>,
}

/// 队列中的待写事件
struct PendingEvent {
    session_id: SessionId,
    event: SessionEvent,
}

/// 在阻塞池执行同步闭包;无 tokio runtime 时降级同步执行
///
/// # WHY（红线 3 + 降级语义）
/// - 有 runtime:SQLite 写入与段文件 IO 必须在 `spawn_blocking` 阻塞池
///   （绝不占用 async worker 线程）
/// - 无 runtime（同步测试 / block_on 驱动的异步上下文）:`try_current()`
///   返回 Err,直接同步执行——文档注明此降级路径,调用方须保证自身不在
///   async runtime 内（否则应走 spawn_blocking）
async fn run_blocking<F, T>(f: F) -> Result<T, StoreError>
where
    F: FnOnce() -> Result<T, StoreError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(_) => Ok(spawn_blocking(f).await.map_err(|e| StoreError::Join {
            reason: format!("spawn_blocking join 错误(阻塞池任务 panic): {e}"),
        })??),
        // 无 runtime 降级同步（测试/非异步环境）
        Err(_) => f(),
    }
}

impl CbmrWriter {
    /// 创建微批写器（同步路径）
    ///
    /// # 调用约束（M51/C-62）
    /// 本函数内部是**真阻塞**的：`create_dir_all` + `TreeIndex::open`
    /// （`Connection::open` / `PRAGMA` / `CREATE TABLE`）都在调用线程上执行。
    /// 因此 **async 上下文必须用 [`CbmrWriter::open`]**（它把整段构建灌进
    /// `run_blocking` 的阻塞池），否则会堵住 runtime worker（AGENTS §4.4-2）。
    ///
    /// # 后台循环
    /// `config.spawn_flush_loop` 为 true 且存在 tokio runtime 时启动
    /// 自适应窗口刷写任务;否则仅靠显式 `flush` / 满批触发（测试确定性）。
    pub fn new(config: StoreConfig) -> Result<Self, StoreError> {
        Self::open_blocking(config)
    }

    /// 创建微批写器（async 入口）— 把打开/DDL 整段放进阻塞池（M51/C-62）
    ///
    /// # WHY 需要它
    /// `Connection::open`、`PRAGMA journal_mode=WAL`、`CREATE TABLE ...` 与
    /// `create_dir_all` 都是同步阻塞调用。此前它们直接跑在调用方的 async worker 上，
    /// 与红线「SQLite 只经 spawn_blocking」不一致（该红线原文只写了「写入」，
    /// 覆盖缺口就在打开/DDL 这一段）。
    ///
    /// # 与 `new` 的关系
    /// 两者共用**同一个实现**（`open_blocking`），不存在第二套构建逻辑——
    /// 刻意避开了 L46 双 `MtpeConfig` 那类「双构造面」。区别仅在调用上下文：
    /// async 用 `open().await`，同步/测试用 `new()`。
    pub async fn open(config: StoreConfig) -> Result<Self, StoreError> {
        run_blocking(move || Self::open_blocking(config)).await
    }

    /// 构建的同步内核 — 建目录 + 开索引库（含 PRAGMA/DDL） + 启动后台循环
    fn open_blocking(config: StoreConfig) -> Result<Self, StoreError> {
        std::fs::create_dir_all(&config.data_dir).map_err(|e| StoreError::Io {
            context: format!("创建数据目录 {} 失败", config.data_dir.display()),
            source: e,
        })?;
        let tree = TreeIndex::open(&config.data_dir.join("sessions.sqlite3"))?;
        // R11 W3a(ADR-295 D4):打开时索引尾部一致性探针 + 自愈(config 门控)。
        // 探针/重建失败**不阻断打开**——索引是加速面(ADR-290 D2),自愈是优化,
        // 打开可用性优先;失败降级 warn + 空报告(诚实可观测,不静默)。
        let startup_repair = if config.repair_index_on_open {
            match tree.verify_and_repair_tails(&config.data_dir) {
                Ok(report) => report,
                Err(e) => {
                    warn!(error = %e, "索引尾部探针失败,降级为不自愈(打开不受阻)");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        // R12 W3-1(ADR-298 D4):**内容**一致性探针 —— 尾部探针只看"末尾缺口"，
        // 其自身文档明载「中段随机缺行/索引整库丢失不在本探针判据内」。本探针比对
        // "索引侧指纹 vs 段文件实况重算"，不一致即按权威源重建（段文件是权威源、
        // 索引是加速面，ADR-290 D2）。
        //
        // 门控与尾部探针**同一开关**（`repair_index_on_open`）：关闭即一次也不跑，
        // 保住既有"禁用 ⇒ 缺口保留、不干预"的语义；探针内部另有
        // `CHIMERA_INDEX_PROBE=off` 的独立逃生（面向只想关它、仍要尾部自愈的场景）。
        //
        // 失败同样不阻断打开：索引是加速面，打开可用性优先于自愈完整性。
        let _startup_content_probe: ContentProbeReport = if config.repair_index_on_open {
            match tree.probe_content_consistency(&config.data_dir) {
                Ok(report) => {
                    log_content_probe(&report);
                    report
                }
                Err(e) => {
                    warn!(error = %e, "索引内容一致性探针失败,降级为不重建(打开不受阻)");
                    ContentProbeReport::default()
                }
            }
        } else {
            ContentProbeReport::default()
        };
        let inner = Arc::new(CbmrInner {
            config,
            tree,
            sessions: Mutex::new(HashMap::new()),
            pending: Mutex::new(PendingBatch::default()),
            ema_batch: Mutex::new(0.0),
            last_offset_by_session: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            flush_loop: Mutex::new(None),
            startup_repair: Mutex::new(startup_repair),
        });
        if inner.config.spawn_flush_loop {
            if tokio::runtime::Handle::try_current().is_ok() {
                spawn_flush_loop(inner.clone());
            } else {
                debug!("无 tokio runtime,后台刷写循环不启动(仅显式 flush/满批触发)");
            }
        }
        Ok(Self { inner })
    }

    /// 打开时索引尾部探针的自愈报告(R11 W3a / ADR-295 D4)
    ///
    /// `repair_index_on_open = false` 或探针失败降级时为空;每条含会话 id、
    /// 是否判定缺口、是否已重建及重建统计。消费方(组合根/健康探针)可据此
    /// 上报可观测性面——本 crate(L3 纯存储)不持事件总线,不发事件。
    #[must_use]
    pub fn startup_repair_report(&self) -> Vec<IndexRepairEntry> {
        self.inner
            .startup_repair
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// 追加会话事件到攒批队列（微批写入口,只入内存）
    ///
    /// # 返回语义
    /// Ok 表示**入队成功**（非 fsync 确认）;持久化确认发生在 flush
    /// （满批 / 窗口到期 / 显式调用）。`SegmentWriter::append` 才是
    /// 「fsync 确认」语义（不丢已确认事件的红线锚点）。
    pub async fn append(
        &self,
        session_id: &SessionId,
        event: SessionEvent,
    ) -> Result<(), StoreError> {
        let should_flush = {
            // 锁段:push + 满批判定,随即释放(不持锁跨 await)
            let mut pending = self
                .inner
                .pending
                .lock()
                .map_err(|p| StoreError::LockPoisoned {
                    reason: format!("pending 队列锁中毒: {p}"),
                })?;
            pending.events.push(PendingEvent {
                session_id: session_id.clone(),
                event,
            });
            pending.events.len() >= self.inner.config.batch_size
        };
        if should_flush {
            // 满 64 立即刷:不等窗口(「≤64 条 / 2ms 窗口」的 64 上界)
            self.flush().await
        } else {
            Ok(())
        }
    }

    /// 优雅关闭 — 置关闭标志→冲刷 pending→回收后台循环（M50）
    ///
    /// # WHY 必须显式提供
    /// 刷写循环原本是「永生任务」:它没有取消通道,也没有任何退出条件,只能随
    /// runtime 整体关闭而消失。而 `append` 已经向调用方返回 Ok（入队成功）,
    /// 一旦进程在最后一个窗口前退出,pending 里的事件**永不落盘**——与
    /// `segment.rs` 红线「append 返回 Ok 前必须落盘」直接相悖。
    ///
    /// `impl Drop` 只能 abort + warn, 兜不住「最后一批还没刷」;
    /// 因此关闭路径必须可 await,否则“不丢已确认事件”无法成立。
    ///
    /// # 幂等性
    /// 重复调用安全:第二次 flush 无 pending 时为 no-op,句柄已取走也只跳过。
    ///
    /// # 返回
    /// - `Ok(())`:pending 已全部尝试落盘且循环已回收
    /// - `Err(StoreError)`:flush 失败（此时循环已回收,但数据风险由错误上报）
    pub async fn shutdown(&self) -> Result<(), StoreError> {
        self.inner.closing.store(true, Ordering::SeqCst);
        let flushed = self.flush().await;
        if let Ok(mut guard) = self.inner.flush_loop.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
        flushed
    }

    /// 显式冲刷 — 立即落盘 pending 全部事件（测试 / 关闭 / 崩溃前调用）
    ///
    /// 无 runtime 环境降级同步执行（见 [`run_blocking`] 文档）。
    pub async fn flush(&self) -> Result<(), StoreError> {
        let this = self.clone();
        run_blocking(move || this.flush_sync()).await
    }

    /// flush 的同步核心 — 段文件追加 + SQLite 单事务写入
    ///
    /// # 调用约束
    /// 仅供:① `flush`（经 spawn_blocking）;② 无 runtime 同步环境
    /// （测试 / 单线程 CLI）。**绝不在 async runtime worker 内直接调用**
    /// （红线 3:SQLite 只经 spawn_blocking）。
    ///
    /// # WHY 双写非原子（P2-T3 归并回放承接）
    /// 段文件 fsync（步骤 3 内 append_batch）与 SQLite 索引 insert（步骤 4）
    /// 之间**非原子**:崩溃窗口内可能出现「段文件已 fsync、索引缺行」的
    /// 状态,且索引本身无自愈。约束登记:段 fsync 与索引 insert 非原子;
    /// 崩溃后索引重建/续写幂等由 **P2-T3 k-way 归并回放** 承接,T2 边界内
    /// 「已确认 = 段 fsync」语义成立（调用方可见的成功以段文件落盘为准）。
    pub fn flush_sync(&self) -> Result<(), StoreError> {
        // 1. 快照 pending(取空即释放锁)
        let batch = {
            let mut pending = self
                .inner
                .pending
                .lock()
                .map_err(|p| StoreError::LockPoisoned {
                    reason: format!("pending 队列锁中毒: {p}"),
                })?;
            std::mem::take(&mut pending.events)
        };
        let batch_len = batch.len();
        if batch_len == 0 {
            return Ok(());
        }

        // 2. 按会话分组(保持批内相对顺序:组内 Vec 原序)
        let mut groups: HashMap<SessionId, Vec<SessionEvent>> = HashMap::new();
        for pe in batch {
            groups.entry(pe.session_id).or_default().push(pe.event);
        }

        // 3. 段追加 + 元数据收集(锁 sessions 的同步段,不跨 await)
        let mut rows: Vec<EventRow> = Vec::with_capacity(batch_len);
        let mut segments: Vec<SegmentMeta> = Vec::new();
        let data_dir = self.inner.config.data_dir.clone();
        let max_rows = self.inner.config.max_rows_per_segment;
        // 本批各会话最后写入的 Offset（flush 成功后更新 last_offset_by_session）
        let mut last_offsets: HashMap<SessionId, Offset> = HashMap::new();
        {
            let mut sessions =
                self.inner
                    .sessions
                    .lock()
                    .map_err(|p| StoreError::LockPoisoned {
                        reason: format!("sessions 表锁中毒: {p}"),
                    })?;
            for (sid, events) in groups {
                // 惰性创建:首次 flush 该会话时恢复续写点（重开场景打开最后
                // 非空段 + 复用既有段 ID——见 [`recover_session_state`] 文档）
                if !sessions.contains_key(&sid) {
                    let (w, seg_id) = recover_session_state(&data_dir, &self.inner.tree, &sid)?;
                    sessions.insert(
                        sid.clone(),
                        SessionState {
                            current: w,
                            current_segment_id: seg_id,
                        },
                    );
                }
                let state = sessions
                    .get_mut(&sid)
                    .ok_or_else(|| StoreError::InvalidInput {
                        reason: format!("会话 {} 状态丢失(插入后不可达)", sid),
                    })?;
                // 批内可能跨段:写满当前段即滚动,剩余事件进新段
                let mut events_left = events;
                loop {
                    let capacity = max_rows.saturating_sub(state.current.row_count());
                    if capacity == 0 {
                        // 滚动:关闭旧段(元数据入列),新段承接剩余
                        let old_meta = state.current.meta(state.current_segment_id.clone(), None);
                        segments.push(old_meta);
                        let start = state.current.next_seq();
                        let idx = state.current.segment_index() + 1;
                        state.current = SegmentWriter::open_or_create(&data_dir, &sid, idx, start)?;
                        state.current_segment_id = SegmentId::generate();
                        continue;
                    }
                    let take = capacity.min(events_left.len() as u64) as usize;
                    if take == 0 {
                        break;
                    }
                    let offsets = state.current.append_batch(&events_left[..take])?;
                    let seg_id = state.current_segment_id.clone();
                    for (off, event) in offsets.iter().zip(events_left.drain(..take)) {
                        rows.push(EventRow {
                            offset: off.seq,
                            session_id: sid.clone(),
                            segment_id: seg_id.clone(),
                            event,
                        });
                    }
                    // 追踪本批该会话最后写入的 Offset:跨段滚动时每轮覆盖,
                    // 末轮值 = 该会话本批全局最后一条（seq 单调最大）
                    if let Some(last) = offsets.last() {
                        last_offsets.insert(sid.clone(), *last);
                    }
                    if events_left.is_empty() {
                        break;
                    }
                    // 写满滚动(继续下一段承接剩余)
                    let old_meta = state.current.meta(state.current_segment_id.clone(), None);
                    segments.push(old_meta);
                    let start = state.current.next_seq();
                    let idx = state.current.segment_index() + 1;
                    state.current = SegmentWriter::open_or_create(&data_dir, &sid, idx, start)?;
                    state.current_segment_id = SegmentId::generate();
                }
                // 当前段 end_offset 推进(upsert 保 start 首值)
                let meta = state.current.meta(state.current_segment_id.clone(), None);
                segments.push(meta);
            }
        } // sessions 锁释放

        // 4. SQLite 写(段元数据 + 事件,均为同步调用;调用方保证经 spawn_blocking)
        // WP-04:单事务合并(原逐段 autocommit = N+1 次写事务/flush → 现 1 次,
        // 配合 synchronous=NORMAL 写放大直接下降)
        self.inner.tree.insert_flush_batch(&segments, &rows)?;

        // 4.5 flush 成功后更新 last_offset 追踪（失败不推进——未落盘不算已确认）
        if !last_offsets.is_empty() {
            let mut last_map =
                self.inner
                    .last_offset_by_session
                    .lock()
                    .map_err(|p| StoreError::LockPoisoned {
                        reason: format!("last_offset 追踪锁中毒: {p}"),
                    })?;
            last_map.extend(last_offsets);
        }

        // 5. EMA 更新(自适应窗口输入:批大 → EMA 升 → 窗口缩)
        {
            let mut ema = self
                .inner
                .ema_batch
                .lock()
                .map_err(|p| StoreError::LockPoisoned {
                    reason: format!("ema_batch 锁中毒: {p}"),
                })?;
            *ema = *ema * 0.7 + batch_len as f64 * 0.3;
        }
        debug!("微批刷写 {batch_len} 条事件 → 段追加 + SQLite 单事务");
        Ok(())
    }

    /// fork 会话（树索引零拷贝元数据复制 + 注册新会话段状态）
    ///
    /// # 参数
    /// - `parent`:父会话
    /// - `from_offset`:fork 点;新会话事件 seq 从该点起（前缀 [0, from_offset)
    ///   经引用链对新会话可见,`read_events` 无缝拼接）
    pub async fn fork_session(
        &self,
        parent: &SessionId,
        from_offset: u64,
    ) -> Result<SessionId, StoreError> {
        let this = self.clone();
        let parent = parent.clone();
        run_blocking(move || {
            let new_sid = this.inner.tree.fork(&parent, from_offset)?;
            let mut sessions =
                this.inner
                    .sessions
                    .lock()
                    .map_err(|p| StoreError::LockPoisoned {
                        reason: format!("sessions 表锁中毒: {p}"),
                    })?;
            let w = SegmentWriter::open_or_create(
                &this.inner.config.data_dir,
                &new_sid,
                0,
                from_offset,
            )?;
            sessions.insert(
                new_sid.clone(),
                SessionState {
                    current: w,
                    current_segment_id: SegmentId::generate(),
                },
            );
            Ok(new_sid)
        })
        .await
    }

    /// 按会话读回事件（SQLite 树索引,读写分区读路径）
    ///
    /// # 参数
    /// - `session_id`:目标会话（含 fork 会话,自动合并前缀引用事件）
    /// - `from`:可选 seq 下界（断点续传）
    pub async fn read_events(
        &self,
        session_id: &SessionId,
        from: Option<u64>,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let this = self.clone();
        let sid = session_id.clone();
        run_blocking(move || this.inner.tree.read_events(&sid, from)).await
    }

    /// 线程级回放 — 纯段文件权威源 k-way 归并回放（ADR-292 D4 桥的存储面入口）
    ///
    /// # WHY 不走 [`CbmrWriter::read_events`]（SQLite 索引读）
    /// 回放桥的消费场景是**线程丢失恢复**（threads 404 / 进程重启后重建）——
    /// 恰是「段文件已 fsync、索引缺行」非原子窗口的受害者面（flush_sync 双写
    /// 非原子,lib.rs 崩溃恢复语义）。`replay`（ADR-109）纯段文件扫描、索引缺失
    /// 不影响正确性,段文件为权威源——恢复路径必须走权威源而非可能陈旧的索引。
    ///
    /// # 参数
    /// - `session_id`:目标会话（含 fork 会话,前缀引用链自动合并）
    /// - `from`:回放下界 Offset（seq 主过滤键;全量重建传 `Offset::new(0, 0)`）
    ///
    /// # 返回
    /// 全局单调回放条目（seq 主序 + row 次序,顺序与写入一致）。
    pub async fn replay_session(
        &self,
        session_id: &SessionId,
        from: Offset,
    ) -> Result<Vec<crate::replay::ReplayItem>, StoreError> {
        let this = self.clone();
        let sid = session_id.clone();
        // WHY 整段进 run_blocking:replay 内含 segment_sources 的 SQLite 读与
        // 段文件 IO,均为阻塞调用——红线 3（阻塞只进 spawn_blocking 阻塞池）
        run_blocking(move || {
            let stream =
                crate::replay::replay(&this.inner.tree, &this.inner.config.data_dir, &sid, from)?;
            stream.collect()
        })
        .await
    }

    /// 当前自适应窗口（ms 整数化,供文档/基准观测）
    ///
    /// 映射:近期批大小 EMA 占比 `ratio`（0=空批,1=满批）→
    /// 窗口 = `4 - 3*ratio` ms（满批 1ms,空批 4ms;基准 = [`StoreConfig::base_window_ms`]）。
    #[must_use]
    pub fn adaptive_window(&self) -> Duration {
        let ema = self.inner.ema_batch.lock().map(|g| *g).unwrap_or(0.0);
        let ratio = (ema / self.inner.config.batch_size as f64).clamp(0.0, 1.0);
        let ms = (4.0 - 3.0 * ratio).clamp(1.0, 4.0);
        Duration::from_millis(ms as u64)
    }

    /// 攒批队列中的待写事件数（测试断言满批触发用）
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.inner
            .pending
            .lock()
            .map(|p| p.events.len())
            .unwrap_or(0)
    }

    /// 最近一次 flush 后该会话最后写入的 Offset（断点续读/回放定位）
    ///
    /// # 语义边界
    /// 仅反映**本 writer 实例**的 flush 历史:重开（drop 后重建）同会话时,
    /// 首个 flush 之前返回 None——调用方应先 flush 再查询（`persist_turn`
    /// 即按 append → flush → last_offset 顺序调用）。会话无事件 → None。
    pub async fn last_offset(&self, session_id: &SessionId) -> Result<Option<Offset>, StoreError> {
        let sid = session_id.clone();
        let last_map =
            self.inner
                .last_offset_by_session
                .lock()
                .map_err(|p| StoreError::LockPoisoned {
                    reason: format!("last_offset 锁中毒: {p}"),
                })?;
        Ok(last_map.get(&sid).copied())
    }

    /// SQLite 写事务计数（bench 门禁:syscall_reduction_pct 的微批分子）
    #[must_use]
    pub fn transactions(&self) -> u64 {
        self.inner.tree.transaction_count()
    }
}

/// 会话续写恢复 — 打开最后一个非空段（段文件为权威源）+ 复用既有段 ID
///
/// # WHY（P2-T3 续写幂等,承接 T2 审查 Important #2）
/// T2 惰性创建固定 `open_or_create(sid, 0, 0)`:新会话正确,但重开已有会话时
/// ① 段 0 满后已滚动,从段 0 续写再滚动到段 1 会 OffsetMismatch（段 1 首条
/// seq 是历史滚动点,与续写后的 next_seq 不一致）;② 重新 generate 段 ID →
/// segments 表重复段行 → segment_sources 按段索引返回多行 → 回放重复输出。
/// 本函数:枚举段文件（权威源,索引缺行也能恢复）→ 从最大索引向下找第一个
/// 非空段（空文件是崩溃残留,与 rebuild_index 跳过语义一致）→ 以文件内首条
/// seq 为 start_seq 重开 → 段 ID 复用既有行（insert_segment 走 upsert）。
fn recover_session_state(
    data_dir: &Path,
    tree: &TreeIndex,
    sid: &SessionId,
) -> Result<(SegmentWriter, SegmentId), StoreError> {
    let idxs = list_segment_files(data_dir, sid)?;
    // 从最大索引向下找第一个非空段（崩溃残留的空段文件无内容可恢复）
    let mut recovered: Option<(u32, u64)> = None; // (段索引, 首条 seq)
    for &idx in idxs.iter().rev() {
        let path = segment_path(data_dir, sid, idx);
        let mut reader = SegmentFileReader::open(&path)?;
        if let Some(rec) = reader.next_record()? {
            recovered = Some((idx, rec.seq));
            break;
        }
    }
    let (idx, start_seq) = match recovered {
        Some(pair) => pair,
        // 无段文件:新会话（续写点 0）或 fork 会话（零拷贝无自己的段文件,
        // 续写点 = fork 点 = 前缀段最大 end_offset + 1——否则新事件 seq
        // 从 0 起会与父段前缀事件 seq 冲突）
        None => (0, tree.fork_point(sid)?.unwrap_or(0)),
    };
    let w = SegmentWriter::open_or_create(data_dir, sid, idx, start_seq)?;
    // 复用既有段 ID（续写走 upsert）;索引缺行（崩溃未 rebuild）则生成新 ID
    let seg_id = tree
        .segment_id_for(sid, idx)?
        .unwrap_or_else(SegmentId::generate);
    Ok((w, seg_id))
}

/// 后台刷写循环 — 自适应窗口到期检查 pending,非空即 flush
///
/// WHY 把句柄存回 `inner.flush_loop`（M50）:循环是构造期启动的,句柄不能只丢在
/// 局部变量里——否则 `shutdown()` 与 `Drop` 无法收掉它,进程只能靠 runtime 关闭。
fn spawn_flush_loop(inner: Arc<CbmrInner>) {
    let writer = CbmrWriter {
        inner: inner.clone(),
    };
    let mut window = Duration::from_millis(inner.config.base_window_ms);
    // WHY 先 clone 一份给任务:`inner` 本体后面还要用来存句柄(E0382);
    //      两个 Arc 指向同一个 CbmrInner,语义不变。
    let loop_inner = inner.clone();
    let handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(window).await;
            // M50:关闭标志优先于刷写——shutdown 已自行 flush 过,
            // 循环只需退出,不再抢一次写
            if loop_inner.closing.load(Ordering::SeqCst) {
                debug!("后台刷写循环收到关闭标志,退出");
                return;
            }
            let has_pending = loop_inner
                .pending
                .lock()
                .map(|p| !p.events.is_empty())
                .unwrap_or(false);
            if has_pending {
                if let Err(e) = writer.flush().await {
                    // WHY 静默失败:后台循环不能因单次失败终止(下次窗口重试);
                    // 上层显式 flush 仍可兜底,错误已留痕
                    warn!("后台微批刷写失败: {e}");
                }
                // flush 后 EMA 已更新,窗口自适应(批大缩窗 / 批小扩窗)
                window = writer.adaptive_window();
            }
        }
    });
    // WHY 句柄存失败不报错:构造期那一次拿锁不可能中毒(刚建的空 Mutex),
    // 真发生中毒也不影响主语义——shutdown 仍能靠 closing 标志让循环退出。
    if let Ok(mut slot) = inner.flush_loop.lock() {
        *slot = Some(handle);
    }
}

/// 析构即回收（M50）——最后一个 Arc 释放时置关闭标志、abort 循环、并对未落盘队列发 warn
///
/// # WHY 必须 warn 而不是静默
/// `append` 返回 Ok 的语义是「入队成功」(非 fsync 确认),所以析构时还有 pending
/// 意味着**调用方已被告知写入成功的事件不会落盘**。静默丢弃会把这种丢数据变成不
/// 可观测;warn 至少让它在日志里可被追。想彻底避免必须显式 [`CbmrWriter::shutdown`]。
impl Drop for CbmrInner {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.flush_loop.lock() {
            if let Some(handle) = guard.take() {
                // WHY abort 而非 join:Drop 不能 await;此时已无合法调用方可 await
                handle.abort();
            }
        }
        let pending = self.pending.lock().map(|p| p.events.len()).unwrap_or(0);
        if pending > 0 {
            warn!(
                pending,
                "CbmrWriter 析构时仍有未落盘事件(append 曾返回 Ok 但未 fsync);\
                 调用方应先 await shutdown() 再释放最后一个引用"
            );
        }
    }
}

/// 数据目录辅助（测试/文档用:段文件命名推导）
#[must_use]
pub fn index_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("sessions.sqlite3")
}

/// 记录一次"内容一致性探针"的结果 —— 打开路径上唯一的可观测面（R12 W3-1）
///
/// # 为什么只记日志、不把报告存起来
/// 本 crate 不依赖 `tracing-subscriber`（不新增 crate 的硬约束），进程内无法断言日志
/// 内容；需要结构化结果的调用方应直接调 [`TreeIndex::probe_content_consistency`]。
/// 打开路径只需要"该告警时告警"：
/// - 有会话被重建 ⇒ `warn`（这是一次真实的修复动作，运维应当看得见）；
/// - 全部健康 ⇒ `debug`（避免每次启动都刷 warn，把噪声变成习惯性忽略）。
///
/// # 参数
/// - `report`:探针汇总报告（只读）
fn log_content_probe(report: &ContentProbeReport) {
    let rebuilt = report.rebuilt_count();
    if rebuilt > 0 {
        warn!(
            sessions = report.entries.len(),
            rebuilt, "索引内容一致性探针:已按段文件（权威源）重建不一致会话"
        );
    } else {
        debug!(
            sessions = report.entries.len(),
            "索引内容一致性探针:未发现需重建的会话"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    fn ev(i: u64) -> SessionEvent {
        SessionEvent::with_payload(format!("ev-{i}"), vec![i as u8])
    }

    fn config(dir: &std::path::Path) -> StoreConfig {
        StoreConfig::test_config(dir)
    }

    #[tokio::test]
    async fn batch_size_threshold_triggers_flush() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        let sid = SessionId::new("thresh");
        // test_config batch_size=4:满 4 条触发,不满不触发
        for i in 0..3 {
            w.append(&sid, ev(i)).await.expect("append");
        }
        assert_eq!(w.pending_len(), 3, "未满批不刷写");
        assert_eq!(w.transactions(), 0, "未满批 0 次 SQLite 写");
        w.append(&sid, ev(3)).await.expect("append 4th");
        assert_eq!(w.transactions(), 1, "满 4 条 = 1 次 SQLite 写事务");
        assert_eq!(w.pending_len(), 0, "满批后队列清空");
        let stored = w.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 4);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
        }
    }

    #[tokio::test]
    async fn window_expiry_triggers_flush() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = config(dir.path());
        cfg.spawn_flush_loop = true;
        cfg.base_window_ms = 1; // 加速窗口到期
        cfg.batch_size = 64; // 不触发满批,只靠窗口
        let w = CbmrWriter::new(cfg).expect("new");
        let sid = SessionId::new("window");
        w.append(&sid, ev(0)).await.expect("append");
        assert_eq!(w.transactions(), 0, "入队后未立即刷写");
        // 轮询等待后台窗口到期刷写(带超时防悬挂)
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while w.transactions() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "窗口到期应触发后台刷写"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(w.pending_len(), 0);
        let stored = w.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 1, "窗口到期后 1 条事件落盘");
    }

    /// M50：shutdown 必须把最后一批刷盘并让后台循环退出
    ///
    /// WHY 窗口拉到 60s：本测试要证的是 shutdown 自己刷（不是窗口到期刷），
    /// 否则就变成在测窗口了。
    #[tokio::test]
    async fn shutdown_flushes_pending_and_stops_loop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = config(dir.path());
        cfg.spawn_flush_loop = true;
        cfg.base_window_ms = 60_000;
        cfg.batch_size = 64;
        let w = CbmrWriter::new(cfg).expect("new");
        let sid = SessionId::new("shutdown");
        for i in 0..3 {
            w.append(&sid, ev(i)).await.expect("append");
        }
        assert_eq!(w.pending_len(), 3, "窗口未到期前事件应全部停在队列");
        assert_eq!(w.transactions(), 0, "未 flush 不得产生 SQLite 事务");

        w.shutdown().await.expect("shutdown");

        assert_eq!(w.pending_len(), 0, "shutdown 后队列必须清空");
        let stored = w.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 3, "shutdown 必须把最后一批落盘");
    }

    /// M50：shutdown 幂等 —— 第二次调用不报错、不多写事务
    #[tokio::test]
    async fn shutdown_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = config(dir.path());
        cfg.spawn_flush_loop = true;
        cfg.base_window_ms = 60_000;
        let w = CbmrWriter::new(cfg).expect("new");
        let sid = SessionId::new("idem");
        w.append(&sid, ev(0)).await.expect("append");

        w.shutdown().await.expect("first shutdown");
        let tx = w.transactions();
        w.shutdown().await.expect("second shutdown");

        assert_eq!(w.transactions(), tx, "无 pending 时不得产生额外事务");
        assert_eq!(
            w.read_events(&sid, None).await.expect("read").len(),
            1,
            "重复 shutdown 不得重复写入"
        );
    }

    /// M51/C-62：async 上下文必须走 `open()`（打开/DDL 在阻塞池里）
    #[tokio::test]
    async fn open_runs_construction_in_blocking_pool() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::open(config(dir.path())).await.expect("open");
        let sid = SessionId::new("open-async");
        w.append(&sid, ev(0)).await.expect("append");
        w.flush().await.expect("flush");
        assert_eq!(w.read_events(&sid, None).await.expect("read").len(), 1);
        assert!(index_path(dir.path()).exists(), "打开路径必须完成建库/DDL");
        w.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn flush_writes_all_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        let sid = SessionId::new("flush-all");
        for i in 0..3 {
            w.append(&sid, ev(i)).await.expect("append");
        }
        assert_eq!(w.pending_len(), 3);
        w.flush().await.expect("flush");
        assert_eq!(w.pending_len(), 0);
        assert_eq!(w.transactions(), 1);
        let stored = w.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 3);
    }

    #[test]
    fn flush_sync_without_runtime_degrades_to_synchronous() {
        // 无 tokio runtime:#[test] 直接调 flush_sync(降级同步路径)
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        let sid = SessionId::new("sync-path");
        // 同步路径:不能 await append,直接塞 pending 再 flush_sync
        {
            let mut pending = w.inner.pending.lock().expect("lock");
            pending.events.push(PendingEvent {
                session_id: sid.clone(),
                event: ev(0),
            });
            pending.events.push(PendingEvent {
                session_id: sid.clone(),
                event: ev(1),
            });
        }
        w.flush_sync().expect("flush_sync");
        assert_eq!(w.transactions(), 1);
        // 同步读回(直接走 tree,不经 async)
        let stored = w.inner.tree.read_events(&sid, None).expect("read");
        assert_eq!(stored.len(), 2);
    }

    #[test]
    fn block_on_drives_flush_without_runtime() {
        // futures::executor::block_on 驱动 async flush:内部 try_current Err
        // → 降级同步执行(覆盖 run_blocking 的 Err 分支)
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        let sid = SessionId::new("blockon-path");
        futures::executor::block_on(async {
            w.append(&sid, ev(0)).await.expect("append");
            w.append(&sid, ev(1)).await.expect("append");
            w.flush().await.expect("flush");
        });
        assert_eq!(w.transactions(), 1);
        let stored = w.inner.tree.read_events(&sid, None).expect("read");
        assert_eq!(stored.len(), 2);
    }

    #[tokio::test]
    async fn segment_rollover_through_writer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = config(dir.path());
        cfg.max_rows_per_segment = 8;
        cfg.batch_size = 4;
        let w = CbmrWriter::new(cfg).expect("new");
        let sid = SessionId::new("roll");
        for i in 0..20 {
            w.append(&sid, ev(i)).await.expect("append");
        }
        // 20 条 = 5 批;段 0 容量 8 → 段 0(8) + 段 1(8) + 段 2(4)
        w.flush().await.expect("final flush");
        assert_eq!(w.inner.tree.segment_count(&sid).expect("segs"), 3);
        let stored = w.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 20);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64, "跨段 seq 连续");
        }
    }

    #[tokio::test]
    async fn fork_session_continues_sequence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        let parent = SessionId::new("fork-parent");
        for i in 0..10 {
            w.append(&parent, ev(i)).await.expect("append");
        }
        w.flush().await.expect("flush");

        // fork 点 7:新会话 seq 从 7 起
        let child = w.fork_session(&parent, 7).await.expect("fork");
        assert_ne!(child, parent);
        w.append(&child, ev(100)).await.expect("child append");
        w.flush().await.expect("child flush");

        // 回查:7 前缀 + 1 自己 = 8 条连续
        let stored = w.read_events(&child, None).await.expect("read");
        assert_eq!(stored.len(), 8);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64, "前缀+新事件无缝拼接");
        }
        assert_eq!(stored[7].event.event_type, "ev-100");
    }

    #[tokio::test]
    async fn adaptive_window_shrinks_with_large_batches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        let sid = SessionId::new("adaptive");
        // 冷启动 EMA=0 → 窗口 4ms(低吞吐扩窗)
        assert_eq!(w.adaptive_window(), Duration::from_millis(4));
        // 满批 flush 后 EMA 上升 → 窗口收缩
        for i in 0..4 {
            w.append(&sid, ev(i)).await.expect("append");
        }
        assert_eq!(w.transactions(), 1);
        // EMA = 0*0.7 + 4*0.3 = 1.2; ratio=1.2/4=0.3 → ms = 4-0.9 = 3.1 → 3ms
        let window = w.adaptive_window();
        assert!(window < Duration::from_millis(4), "批大则缩窗: {window:?}");
        assert!(window >= Duration::from_millis(1));
    }

    #[tokio::test]
    async fn empty_flush_is_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        w.flush().await.expect("empty flush");
        assert_eq!(w.transactions(), 0);
        assert_eq!(w.pending_len(), 0);
    }

    // ============================================================
    // 续写幂等（P2-T3 承接 T2 审查 Important #2）
    // ============================================================

    #[tokio::test]
    async fn writer_reopen_continues_sequence_no_conflict() {
        // 续写幂等:重开后 flush 不产生 UNIQUE 冲突（段 ID 复用 + seq 从尾
        // 部恢复）,且不产生重复段行（否则回放重复输出）
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("reopen-idem");
        let cfg = config(dir.path());

        // 第一生命周期:写 0..9（跨 3 批;段阈值 8 → 段 0 满 8 条后滚动段 1 写 2 条）
        let seg_count_before;
        {
            let w = CbmrWriter::new(cfg.clone()).expect("new");
            for i in 0..10 {
                w.append(&sid, ev(i)).await.expect("append");
            }
            w.flush().await.expect("flush");
            seg_count_before = w.inner.tree.segment_count(&sid).expect("segs");
            assert_eq!(seg_count_before, 2, "10 条 = 段0(8) + 段1(2)");
            // last_offset 追踪生效:最近 flush 的末位 = seq 9
            let lo = w.last_offset(&sid).await.expect("last_offset");
            assert_eq!(lo.expect("offset").seq, 9);
        } // drop = 模拟崩溃后重启

        // 第二生命周期:重开同会话续写 10..19,flush 必须无冲突
        let w2 = CbmrWriter::new(cfg.clone()).expect("reopen");
        // 重开后首个 flush 前 last_offset 为 None（语义边界,文档注明）
        assert!(
            w2.last_offset(&sid).await.expect("lo").is_none(),
            "重开后未 flush 前不追踪"
        );
        for i in 10..20 {
            w2.append(&sid, ev(i)).await.expect("append");
        }
        w2.flush().await.expect("flush 无 UNIQUE 冲突");
        // 段行 = 2 旧段 + 1 新滚动段:既有段 ID 复用（upsert 路径）,不产生重复段行
        assert_eq!(
            w2.inner.tree.segment_count(&sid).expect("segs"),
            seg_count_before + 1,
            "重开续写:既有段复用 + 滚动新增 1 段"
        );
        // 事件 20 条连续
        let stored = w2.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 20);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
            assert_eq!(s.event.event_type, format!("ev-{i}"));
        }
        // last_offset 推进到 19
        assert_eq!(w2.last_offset(&sid).await.expect("lo").expect("o").seq, 19);
        // 回放（纯段文件读取）:顺序逐项一致（门禁一致率 100% 的续写场景）
        let tree = w2.inner.tree.clone();
        let stream =
            crate::replay::replay(&tree, dir.path(), &sid, Offset::new(0, 0)).expect("replay");
        let items = stream.collect().expect("collect");
        assert_eq!(items.len(), 20);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(item.offset.seq, i as u64);
            assert_eq!(item.event.event_type, format!("ev-{i}"));
        }
    }

    #[tokio::test]
    async fn writer_reopen_across_rolled_segments() {
        // 重开前已滚动（3 段）:重开后恢复最后段续写,seq 跨段连续无冲突
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("reopen-roll");
        let mut cfg = config(dir.path());
        cfg.max_rows_per_segment = 8;
        let seg_count_before;
        {
            let w = CbmrWriter::new(cfg.clone()).expect("new");
            for i in 0..20 {
                w.append(&sid, ev(i)).await.expect("append");
            }
            w.flush().await.expect("flush");
            seg_count_before = w.inner.tree.segment_count(&sid).expect("segs");
            assert_eq!(seg_count_before, 3, "20 条 = 段0(8)+段1(8)+段2(4)");
        }

        let w2 = CbmrWriter::new(cfg.clone()).expect("reopen");
        // 续写 10 条:段 2 容量 4 → 段 2 到 8 条 + 滚动段 3 写 6 条
        for i in 20..30 {
            w2.append(&sid, ev(i)).await.expect("append");
        }
        w2.flush().await.expect("flush 无冲突");
        // 段行 = 3 + 1 新段（旧段不重复）
        assert_eq!(
            w2.inner.tree.segment_count(&sid).expect("segs"),
            seg_count_before + 1,
            "重开续写:既有段复用 + 滚动新增 1 段"
        );
        let stored = w2.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 30);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64, "跨段重开续写 seq 连续");
            assert_eq!(s.event.event_type, format!("ev-{i}"));
        }
        assert_eq!(w2.last_offset(&sid).await.expect("lo").expect("o").seq, 29);
    }

    #[tokio::test]
    async fn fork_session_reopen_continues_from_fork_point() {
        // fork 会话零拷贝无自己的段文件:重开后续写点 = fork 点（前缀段最大
        // end_offset + 1）,新事件 seq 从 fork 点继续,与父段前缀无缝拼接
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("reopen-fork-parent");
        let cfg = config(dir.path());

        // 父会话写 10 条,在 fork 点 7 派生子会话
        let child;
        {
            let w = CbmrWriter::new(cfg.clone()).expect("new");
            for i in 0..10 {
                w.append(&parent, ev(i)).await.expect("append");
            }
            w.flush().await.expect("flush");
            child = w.fork_session(&parent, 7).await.expect("fork");
            // fork 后子会话写 1 条（seq=7）落盘
            w.append(&child, ev(100)).await.expect("child append");
            w.flush().await.expect("child flush");
        } // drop = 模拟重启

        // 重开后子会话续写:seq 从 8 继续（7 已写）
        let w2 = CbmrWriter::new(cfg.clone()).expect("reopen");
        for i in 0..3 {
            w2.append(&child, ev(200 + i)).await.expect("append");
        }
        w2.flush()
            .await
            .expect("flush 无冲突(seq 从 fork 点后续写)");
        let stored = w2.read_events(&child, None).await.expect("read");
        assert_eq!(stored.len(), 11, "7 前缀 + 4 自己(7..10)");
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64, "前缀 + 新事件 seq 无缝拼接");
        }
        // 末尾 4 条是自己的事件（seq 7..10）
        assert_eq!(stored[7].event.event_type, "ev-100");
        assert_eq!(stored[10].event.event_type, "ev-202");
    }

    /// R11 W3a(ADR-295 D4):打开时尾部探针自愈——「段已 fsync、索引缺尾行」
    /// 状态(双写非原子崩溃窗口的登记失败模式)在新 writer 打开时被探针发现
    /// 并 rebuild,索引读面恢复完整序列。
    #[tokio::test]
    async fn startup_probe_repairs_missing_index_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        let sid = SessionId::new("repair-probe");
        for i in 0..6 {
            w.append(&sid, ev(i)).await.expect("append");
        }
        w.flush().await.expect("flush");
        w.shutdown().await.expect("shutdown");

        // 模拟双写崩溃窗口:手工删掉索引最后 2 行(段文件保持完整)
        {
            let conn = rusqlite::Connection::open(index_path(dir.path())).expect("open sqlite");
            conn.execute(
                "DELETE FROM events WHERE session_id = 'repair-probe'
                 AND offset >= (SELECT MAX(offset) - 1 FROM events WHERE session_id = 'repair-probe')",
                [],
            )
            .expect("delete tail rows");
        }

        // 重开(repair_index_on_open 默认 true)→ 探针发现缺口 + 幂等重建
        let w2 = CbmrWriter::new(config(dir.path())).expect("reopen");
        let hit = w2
            .startup_repair_report()
            .into_iter()
            .find(|e| e.session_id == "repair-probe")
            .expect("探针报告必须含本会话");
        assert!(
            hit.damaged && hit.repaired,
            "缺口必须被判 damaged 且已重建: {hit:?}"
        );
        assert_eq!(
            hit.stats.as_ref().expect("重建统计在场").rows_inserted,
            2,
            "重建恰好补回被删的 2 行"
        );
        // 读面恢复完整序列(read_events 以索引为读面,缺口即截断)
        let stored = w2.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 6, "自愈后读面必须恢复完整序列");
        w2.shutdown().await.expect("shutdown");
    }

    /// R11 W3a:repair_index_on_open = false ⇒ 不干预旧行为(缺口保留、
    /// 报告为空)——自愈是优化面,逃生开关必须真实生效
    #[tokio::test]
    async fn startup_probe_skipped_when_disabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = CbmrWriter::new(config(dir.path())).expect("new");
        let sid = SessionId::new("no-repair");
        for i in 0..4 {
            w.append(&sid, ev(i)).await.expect("append");
        }
        w.flush().await.expect("flush");
        w.shutdown().await.expect("shutdown");
        {
            let conn = rusqlite::Connection::open(index_path(dir.path())).expect("open sqlite");
            conn.execute(
                "DELETE FROM events WHERE session_id = 'no-repair' AND offset >= 3",
                [],
            )
            .expect("delete tail row");
        }
        let cfg = StoreConfig {
            repair_index_on_open: false,
            ..config(dir.path())
        };
        let w2 = CbmrWriter::new(cfg).expect("reopen");
        assert!(
            w2.startup_repair_report().is_empty(),
            "禁用探针 ⇒ 不出报告也不重建"
        );
        let stored = w2.read_events(&sid, None).await.expect("read");
        assert_eq!(stored.len(), 3, "禁用 ⇒ 缺口保留(诚实非干预旧行为)");
        w2.shutdown().await.expect("shutdown");
    }

    /// R11 W3b:树索引 schema 版本面——新库打开后恒为 v1(ADR-213 Checkpoint
    /// F3 同款模式:u16 版本判据,未来迁移的分支锚点)
    #[test]
    fn tree_index_schema_version_pinned() {
        let t = TreeIndex::open_in_memory().expect("open_in_memory");
        assert_eq!(t.schema_version().expect("version"), 1, "新库 = v1");
    }
}
