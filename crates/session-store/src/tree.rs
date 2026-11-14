//! SQLite 树索引 — segments / events 两表 + fork 零拷贝（v4.0 WI-18 存储面）
//!
//! 对应架构层: **L3 Storage**（session-store,Phase 2 新增,ADR-141）
//! 对应任务: **P2-T2**（v4.0 WI-18:append-only JSONL 段 + SQLite 树索引 + fork 零拷贝）
//!
//! # Schema（WI-18）
//!
//! ```sql
//! segments(segment_id PK, session_id, segment_index, parent_segment_id,
//!          start_offset, end_offset)
//! events(offset, session_id, segment_id, payload BLOB)  -- 复合主键 (session_id, offset)
//! ```
//!
//! - `events.payload` = rmp-serde `to_vec_named`（map 编码,归 ADR-213 决策 R8.1 的
//!   Class A「长生命周期持久 envelope」）序列化的
//!   `SessionEvent`;紧凑索引格式,与段文件 JSONL 人读格式双轨并存。
//!   WHY named:EventMetadata.correlation_id 带 skip_serializing_if,array 位置编码
//!   下字段错位（该 hazard 的实测出处是 nexus-contracts/tests/wire_format_compat.rs,
//!   非 ADR-004——ADR-004 只裁「MessagePack 而非 JSON」与 LHQP 检查点持久化,
//!   从未裁 array/map 形态,旧注把它当依据属过度归属,已在 ADR-214 登记 D-17 关闭）,
//!   map 按字段名匹配不受 skip 影响
//! - `parent_segment_id` 非 NULL 的行 = fork 复制行,指向父会话的段
//! - 复合索引 `(session_id, offset)`:P2-T3 k-way 归并回放的按会话顺序扫描路径
//!
//! # WAL 模式（项目红线）
//!
//! `PRAGMA journal_mode=WAL` 必须经 `Connection::pragma_update` 设置
//! （对齐 scc-cache wal.rs 先例）;WAL 模式允许读-写并发,微批写入不阻塞
//! 读取路径（ADR-108 读写分区放大）。
//!
//! # fork 零拷贝（WI-18）
//!
//! `fork(session_id, from_offset)` 复制**前缀段元数据行**到新会话
//! （新行 `segment_id` 重新分配,`parent_segment_id` 指向父段原行）,
//! **零事件数据拷贝**:
//! - `events` 表零行插入（事件 payload 仍在父段名下,回查经引用链）
//! - JSONL 段文件零复制（无新文件;仅元数据行复制）
//!
//! 最小实现选「仅元数据 + 引用」而非硬链接/引用计数:跨平台（Windows 硬链接
//! 需 SeCreateSymbolicLinkPrivilege）、无权限依赖、语义最简——引用链天然
//! 支持「父段事件对新会话可见」（树索引回查）。
//!
//! # 线程安全
//!
//! `Arc<Mutex<rusqlite::Connection>>`（`Connection` 非 Sync）;所有方法为
//! **同步**签名,由上层经 `tokio::task::spawn_blocking` 调用（红线 3:
//! SQLite 写入只经 spawn_blocking;同步方法仅供阻塞池/无 runtime 环境使用）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{params, OptionalExtension};
use tracing::{debug, warn};

use crate::error::StoreError;
use crate::segment::{list_segment_files, segment_path, SegmentFileReader, SegmentMeta};
use crate::types::{SegmentId, SessionEvent, SessionId};

/// SQLite 树索引 — segments / events 两表的封装
#[derive(Clone)]
pub struct TreeIndex {
    conn: Arc<Mutex<rusqlite::Connection>>,
    /// SQLite 写事务次数（基准指标:单条直写 = N 次,微批 = ceil(N/64) 次）
    transaction_count: Arc<AtomicU64>,
    /// 段文件所在数据目录（`None` = 内存库/无父目录）
    ///
    /// WHY 记它:内容指纹（R12 W3-1）的「段文件字节数」分量只能从段文件面取,
    /// 而树库固定落在 `data_dir/sessions.sqlite3`（writer.rs 组合根约定）⇒
    /// [`TreeIndex::open`] 的父目录即数据目录,写路径（`insert_flush_batch`/
    /// `rebuild_index`）据此在**同一写事务内**补记指纹,零额外写事务。
    data_dir: Option<PathBuf>,
    /// 内容一致性探针触发重建的会话次数（R12 W3-1 计数面/测试断言）
    content_probe_rebuilds: Arc<AtomicU64>,
}

/// 待写入索引的事件行（payload 由 TreeIndex 内部 rmp-serde 序列化）
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    /// 全局序列号（段内 Offset.seq）
    pub offset: u64,
    /// 所属会话
    pub session_id: SessionId,
    /// 所属段（segments 表主键）
    pub segment_id: SegmentId,
    /// 事件本体
    pub event: SessionEvent,
}

/// 从索引读回的事件（offset + 反序列化事件）
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvent {
    /// 全局序列号
    pub offset: u64,
    /// 事件本体
    pub event: SessionEvent,
}

/// 段节点 — 会话树的最小单元（WI-18:节点 = segment,`parent_segment_id` 指针）
///
/// # WHY 自包含快照
/// 与 `segments` 表行同构的纯数据快照,树查询（tree/fork_tree/ancestors）
/// 一次性读入,避免逐节点往返 SQLite。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentNode {
    /// 段主键
    pub segment_id: SegmentId,
    /// 所属会话
    pub session_id: SessionId,
    /// 会话内段索引
    pub segment_index: u32,
    /// 父段引用（fork 复制行为非 NULL,指向父会话段;非 fork 段为 None）
    pub parent_segment_id: Option<SegmentId>,
    /// 段内起始全局序列号
    pub start_offset: u64,
    /// 段内结束全局序列号（含;fork 截断段 = fork 点 - 1）
    pub end_offset: u64,
}

/// 会话树（WI-18）— 会话的段节点集合 + parent 指针形成的树
///
/// 节点均为**该会话的 segments 行**;fork 复制行的 `parent_segment_id`
/// 指向父会话段（跨会话边）,父段节点本身不在本树内（经
/// [`TreeIndex::fork_tree`] 可取得包含祖先节点的完整视图）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTree {
    /// 所属会话
    pub session_id: SessionId,
    /// 该会话的全部段节点（按段索引升序）
    pub nodes: Vec<SegmentNode>,
}

impl SessionTree {
    /// 根节点 — parent_segment_id 为 NULL 的段（非 fork 复制段）
    #[must_use]
    pub fn roots(&self) -> Vec<&SegmentNode> {
        self.nodes
            .iter()
            .filter(|n| n.parent_segment_id.is_none())
            .collect()
    }

    /// 指定父段（segment_id）的直接子节点（本会话内）
    #[must_use]
    pub fn children_of(&self, parent: &SegmentId) -> Vec<&SegmentNode> {
        self.nodes
            .iter()
            .filter(|n| n.parent_segment_id.as_ref() == Some(parent))
            .collect()
    }

    /// 按段 ID 查节点
    #[must_use]
    pub fn node(&self, segment_id: &SegmentId) -> Option<&SegmentNode> {
        self.nodes.iter().find(|n| &n.segment_id == segment_id)
    }

    /// 节点数
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// 会话可见段的来源解析结果 — 定位物理段文件（k-way 归并回放的输入）
///
/// # WHY 物理解析
/// fork 复制行的 session_id 是子会话（无物理文件）,事件数据实际在父会话
/// 段名下;回放必须沿 parent 引用链解析到**物理写入会话**才能打开文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSource {
    /// 段主键（本会话视角）
    pub segment_id: SegmentId,
    /// 物理文件所属会话（沿 parent 链解析到最底层物理段）
    pub file_session: SessionId,
    /// 物理段文件段索引
    pub segment_index: u32,
    /// 父段引用（None = 物理段）
    pub parent_segment_id: Option<SegmentId>,
    /// 段起始全局序列号
    pub start_offset: u64,
    /// 会话可见段尾（fork 截断段 = fork 点 - 1）
    pub end_offset: u64,
}

/// 索引重建统计 — 崩溃自愈的观察结果（测试断言幂等用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildStats {
    /// 扫描的段文件数
    pub segments_scanned: u64,
    /// 新插入的 events 行数
    pub rows_inserted: u64,
    /// 已存在而跳过的 events 行数（去重;幂等性来源）
    pub rows_skipped: u64,
}

/// 索引尾部探针的单会话报告条目(R11 W3a / ADR-295 D4)
#[derive(Debug, Clone)]
pub struct IndexRepairEntry {
    /// 会话 id
    pub session_id: String,
    /// 探针判定:索引与段文件尾部不一致
    pub damaged: bool,
    /// 是否执行了 rebuild_index(仅「索引缺尾行」方向执行——登记失败模式;
    /// 「索引越前」方向只报告不重建,孤儿行留给 ADR-290 D5 审计器)
    pub repaired: bool,
    /// 重建统计(repaired = true 时在场)
    pub stats: Option<RebuildStats>,
    /// 人类可读判定说明(诊断)
    pub detail: String,
}

/// `index_meta` 中内容指纹的键前缀 — 每会话一条 `content_hash:<session_id>`
///
/// WHY 按会话分域而不是单条全局指纹:段文件本身就是按会话分域的,一致性判定必须
/// **逐会话**做——一条全局指纹会让「某会话段文件坏了」牵连全库重建,噪音与代价
/// 同时放大;逐会话还能让报告指出到底哪个会话需要重建。
const META_KEY_CONTENT_HASH_PREFIX: &str = "content_hash:";

/// 关闭启动期内容一致性探针的环境变量名（R12 W3-1 逃生开关）
///
/// 取值口径见 [`content_probe_enabled`]:`off` / `0` / `false` / `no`（大小写
/// 不敏感）关闭;未设置或其它值 ⇒ 默认开启。
pub const INDEX_PROBE_ENV: &str = "CHIMERA_INDEX_PROBE";

/// FNV-1a 64 位初始值（公开常量便于测试独立复算,不引新依赖）
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64 位质数
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 索引内容指纹（R12 W3-1）— 「段文件（权威源）与索引（加速面）是否同步」的判据
///
/// # 口径（三个分量,两侧可各自独立算出）
/// - `bytes`:该会话**全部段文件的字节数之和**（`fs::metadata().len()`,含尾部半条）
/// - `rows`:完整记录数（实况侧 = `SegmentFileReader` 读出的完整记录数;索引侧 =
///   `events` 表该会话行数）
/// - `hash`:稳定 64 位摘要（FNV-1a over 会话 id + 逐段 `(段索引, 字节数)` + 行数
///   + 最大 offset）——把三个分量绑成一个不可分离的字,避免「单看行数相等就放行」
///
/// # WHY 不做「文件内容全量哈希」
/// 指纹必须在**写路径的同一个 SQLite 写事务内**补记（否则每次 flush 都要多一次
/// 写事务,CBMR 微批写的写放大收益被吃掉）。全量内容哈希需要在写路径重读全库字节
/// （O(全库)）;而上面三个分量能用「文件元数据 + 索引聚合」O(段数) 算出,
/// 且已覆盖登记失败模式（尾部半条/截断/整段消失/索引中段缺行/索引整会话缺行——
/// 见 [`TreeIndex::probe_content_consistency`] 的覆盖声明）。**未覆盖**「等长原地
/// 改字节」（bit rot / 手工等长改写）,登记为已知缺口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexContentFingerprint {
    /// 段文件字节数之和（含未确认的尾部半条——正是它让「写入中断」可判）
    pub bytes: u64,
    /// 完整记录数
    pub rows: u64,
    /// 稳定 64 位摘要（FNV-1a;无随机种子,跨进程可复现）
    pub hash: u64,
}

impl IndexContentFingerprint {
    /// 编码为 `index_meta.value` 的稳定文本形态 `v1:<bytes>:<rows>:<hash:016x>`
    #[must_use]
    pub fn encode(&self) -> String {
        format!("v1:{}:{}:{:016x}", self.bytes, self.rows, self.hash)
    }

    /// 解析文本形态;`None` = 不可解析/版本未知 ⇒ 调用方按「字段缺失」处理
    ///
    /// WHY 解析失败也走「缺失」通道（而不是报错）:老库、手工改过的库、未来
    /// 版本写下的新形态都属于「我不知道它说什么」,未知的自愈路径是重建一次索引,
    /// 报错只会把可自愈的问题升级成打不开。
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.split(':');
        if parts.next()? != "v1" {
            return None;
        }
        let bytes = parts.next()?.parse::<u64>().ok()?;
        let rows = parts.next()?.parse::<u64>().ok()?;
        let hash = u64::from_str_radix(parts.next()?, 16).ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self { bytes, rows, hash })
    }
}

/// 探针判定（纯函数产物）— 三个指纹的相互关系
///
/// WHY 三向而不是两向:内容指纹必须能同时发现「段文件侧变化」与「索引侧变化」。
/// 只比「meta 里记的 vs 从段文件重算的」时,删掉索引中段某行既不改字节数也不改
/// 段文件记录数 ⇒ 两侧仍然相等 ⇒ 漏判。所以探针另外从**索引面**按同一合成函数
/// 重算一次;三向一致才算健康（详见 [`content_probe_verdict`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentProbeVerdict {
    /// 三向一致 ⇒ 索引与权威源同步,不干预
    Healthy,
    /// meta 无该字段/不可解析（老库）⇒ 视为未知,按不一致处理
    MissingField,
    /// 指纹不符（段文件侧变化,或索引缺行/偏移）⇒ 重建
    Mismatch,
    /// 索引越前（索引行数多于段文件完整记录数 = 孤儿行）⇒ **只报告不重建**
    ///
    /// WHY 不重建:`rebuild_index` 是补入式（`INSERT OR IGNORE`）,删不掉孤儿行;
    /// 重建既不会修好它,又会让每次启动都白扫一遍段文件。孤儿行的处置归
    /// ADR-290 D5 审计器（与 `verify_and_repair_tails` 的「索引越前只报告」同口径）。
    IndexAhead,
    /// 无法判定（实况不可读/该会话无段文件）⇒ 不干预;仅 IO 包装会产生
    Undecidable,
}

/// 单会话探针处置结果（报告/测试断言用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentProbeOutcome {
    /// 指纹一致,未干预
    Healthy,
    /// 判不一致 ⇒ 已重建索引并按实况刷新指纹
    Rebuilt,
    /// 索引越前（孤儿行）⇒ 只报告不重建,处置归 ADR-290 D5 审计器
    ReportedOnly,
    /// 判不一致但重建/读实况失败 ⇒ fail-open（保留现有索引继续服务）
    FailedOpen,
    /// 无可比实况（该会话无段文件:fork 未追加 / 段已清理）⇒ 跳过
    Skipped,
}

/// 单会话内容探针记录 — 与同文本的 `tracing::warn!` 一一对应
#[derive(Debug, Clone)]
pub struct ContentProbeEntry {
    /// 会话 id
    pub session_id: String,
    /// 判定(Healthy/MissingField/Mismatch/Undecidable)
    pub verdict: ContentProbeVerdict,
    /// 实况重算的指纹（`Undecidable` 时为空）
    pub live: Option<IndexContentFingerprint>,
    /// 处置结果
    pub outcome: ContentProbeOutcome,
    /// 人类可读判定说明（诊断面）
    pub detail: String,
}

/// 启动期内容一致性探针的汇总报告（R12 W3-1）
#[derive(Debug, Clone, Default)]
pub struct ContentProbeReport {
    /// 环境开关是否关闭了探针（true ⇒ 一条检查也没做）
    pub disabled: bool,
    /// 逐会话记录（`disabled` 时为空）
    pub entries: Vec<ContentProbeEntry>,
}

impl ContentProbeReport {
    /// 已重建的会话数
    #[must_use]
    pub fn rebuilt_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.outcome == ContentProbeOutcome::Rebuilt)
            .count()
    }

    /// fail-open 的会话数（重建失败/实况不可读,索引保持原状）
    #[must_use]
    pub fn failed_open_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.outcome == ContentProbeOutcome::FailedOpen)
            .count()
    }
}

/// FNV-1a 64 位累积（纯函数:稳定、无 std 哈希随机种子、无分配）
fn fnv1a_update(mut state: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        state ^= u64::from(*b);
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

/// 纯函数:由「逐段 (段索引, 字节数) + 行数 + 最大 offset」合成内容指纹
///
/// 输入相同 ⇒ 结果恒相同（无时间戳、无 HashMap 迭代序、无随机种子）,因此同一段
/// 集合重复计算结果相等（`hash_roundtrip_stable` 锁的不变量）。实况侧与索引侧
/// **必须调同一个函数**,否则两侧口径漂移会让探针误判。
#[must_use]
pub fn compose_content_fingerprint(
    session_id: &str,
    segments: &[(u32, u64)],
    rows: u64,
    max_offset: Option<u64>,
) -> IndexContentFingerprint {
    let mut h = fnv1a_update(FNV_OFFSET_BASIS, session_id.as_bytes());
    h = fnv1a_update(h, &(segments.len() as u64).to_le_bytes());
    for (idx, len) in segments {
        h = fnv1a_update(h, &idx.to_le_bytes());
        h = fnv1a_update(h, &len.to_le_bytes());
    }
    h = fnv1a_update(h, &rows.to_le_bytes());
    h = fnv1a_update(h, &max_offset.map_or(u64::MAX, |v| v).to_le_bytes());
    IndexContentFingerprint {
        bytes: segments.iter().map(|(_, len)| *len).sum(),
        rows,
        hash: h,
    }
}

/// 纯函数:`stored`（meta 里记的）vs `live`（段文件实况）vs `index_side`（索引面
/// 按同一合成函数重算）⇒ 判定
///
/// # 三向比较为什么缺一不可
/// - `stored ≠ live`:段文件侧的字节数/行数/最大 seq 自上次记录以来变了
///   ——尾部半条、截断、整段文件被删/增都落在这里;
/// - `index_side ≠ live`:索引侧的行数/最大 offset 与段文件不符 ——
///   **索引中段随机缺行**落在这里（字节数分量两侧都取自段文件,故它只能靠行数/
///   最大 offset 的差异暴露）;
/// - `stored` 缺失:老库从没记过 ⇒ 未知 ⇒ 与「不一致」同通道（幂等重建 + 补记）。
///
/// `index_side` 行数**多于** `live` 时是孤儿行（索引越前）:补入式重建修不掉,
/// 按 [`ContentProbeVerdict::IndexAhead`] 只报告,避免每次启动都白重建一次。
///
/// WHY 缺字段算不一致:缺字段的唯一含义是「从未记录过」（老库/首写前崩溃）,而
/// 探针的处置恰好是幂等重建 + 补记指纹 ⇒ 把「未知」并入「不一致」通道,老库第一次
/// 打开就自动完成迁移,不需要迁移脚本、不需要读侧分支。
#[must_use]
pub fn content_probe_verdict(
    stored: Option<&IndexContentFingerprint>,
    live: &IndexContentFingerprint,
    index_side: &IndexContentFingerprint,
) -> ContentProbeVerdict {
    if stored.is_none() {
        return ContentProbeVerdict::MissingField;
    }
    if index_side != live {
        // 索引行数多于段文件完整记录数 ⇒ 孤儿行(只报告);否则是索引缺行/偏移
        return if index_side.rows > live.rows {
            ContentProbeVerdict::IndexAhead
        } else {
            ContentProbeVerdict::Mismatch
        };
    }
    match stored {
        Some(s) if s == live => ContentProbeVerdict::Healthy,
        _ => ContentProbeVerdict::Mismatch,
    }
}

/// 纯函数:环境变量取值 ⇒ 内容探针是否启用（默认启用）
#[must_use]
pub fn content_probe_enabled(raw: Option<&str>) -> bool {
    let normalized = raw.map(str::trim).map(str::to_ascii_lowercase);
    !matches!(normalized.as_deref(), Some("off" | "0" | "false" | "no"))
}

impl TreeIndex {
    /// 打开或创建 SQLite 树索引（WAL 模式 + 建表 + 建索引）
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = rusqlite::Connection::open(path).map_err(|e| StoreError::Sqlite {
            reason: format!("打开树索引数据库 {} 失败: {e}", path.display()),
        })?;
        // 数据目录 = 树库父目录（组合根约定 `data_dir/sessions.sqlite3`）;
        // 无父目录（裸文件名）时不猜,置 None ⇒ 写路径不补记内容指纹。
        let data_dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf);
        Self::init(conn, data_dir)
    }

    /// 打开内存树索引（测试 / 基准用,不落盘）
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = rusqlite::Connection::open_in_memory().map_err(|e| StoreError::Sqlite {
            reason: format!("打开内存树索引失败: {e}"),
        })?;
        // 内存库无数据目录 ⇒ 内容指纹写入路径全程 no-op（行为与 R11 完全一致）
        Self::init(conn, None)
    }

    /// 公共初始化:WAL pragma（红线）+ 性能 pragma + 建表 + 建索引
    fn init(conn: rusqlite::Connection, data_dir: Option<PathBuf>) -> Result<Self, StoreError> {
        // 红线:journal_mode=WAL 必须 pragma_update(而非 execute 拼 SQL)
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("设置 journal_mode=WAL 失败: {e}"),
            })?;
        // WHY synchronous=NORMAL:WAL 模式标准安全档——事务提交只写 WAL 文件,不等
        // 主库 fsync;断电最多丢最后未 checkpoint 的事务,库文件不会损坏(SQLite 官方
        // 文档 WAL 模式 NORMAL 语义)。本 crate 崩溃恢复语义以段文件为权威源(lib.rs),
        // SQLite 仅索引面且可 rebuild,索引丢最后一笔可接受。FULL 档每次 COMMIT 的
        // fsync 在 Windows FlushFileBuffers 上成本显著(R7 实测固有成本 ~2.9ms/次)。
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("设置 synchronous=NORMAL 失败: {e}"),
            })?;
        // WHY busy_timeout:写锁被并发读者短暂持有时等待而非立即返回 SQLITE_BUSY
        conn.pragma_update(None, "busy_timeout", 5000)
            .map_err(|e| StoreError::Sqlite {
                reason: format!("设置 busy_timeout 失败: {e}"),
            })?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS segments (
                segment_id        TEXT    PRIMARY KEY,
                session_id        TEXT    NOT NULL,
                segment_index     INTEGER NOT NULL,
                parent_segment_id TEXT,
                start_offset      INTEGER NOT NULL,
                end_offset        INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_segments_session
                ON segments(session_id, segment_index);
            CREATE TABLE IF NOT EXISTS events (
                offset     INTEGER NOT NULL,
                session_id TEXT    NOT NULL,
                segment_id TEXT    NOT NULL,
                payload    BLOB    NOT NULL,
                PRIMARY KEY (session_id, offset)
            );
            -- 复合索引:按会话 + offset 的顺序扫描(P2-T3 回放路径)
            CREATE INDEX IF NOT EXISTS idx_events_session_offset
                ON events(session_id, offset);
            -- schema 版本面(R11 W3b,ADR-213 Checkpoint F3 同款模式):
            -- 未来迁移的分支判据。legacy 库(无此表)经 CREATE IF NOT EXISTS
            -- 补建后首次打开即补记 v1——两表现状即 v1 形态,无需数据迁移。
            CREATE TABLE IF NOT EXISTS index_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .map_err(|e| StoreError::Sqlite {
            reason: format!("初始化树索引表结构失败: {e}"),
        })?;
        conn.execute(
            "INSERT OR IGNORE INTO index_meta (key, value) VALUES ('schema_version', '1')",
            [],
        )
        .map_err(|e| StoreError::Sqlite {
            reason: format!("登记 schema 版本失败: {e}"),
        })?;
        // 内容指纹（R12 W3-1）不建表、不加列:`index_meta` 已是 key/value 面,
        // 每会话一条 `content_hash:<session_id>`。WHY 不改 schema:R11 W3b 的
        // `schema_version` 是迁移分支判据,新增一张表/一列会把老库推进"需要迁移"
        // 分支;而 key/value 面天然支持**字段可缺省**——老库没有这些 key ⇒
        // 读回 None ⇒ 探针按未知处理（见 `content_fingerprint` 文档）。
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            transaction_count: Arc::new(AtomicU64::new(0)),
            data_dir,
            content_probe_rebuilds: Arc::new(AtomicU64::new(0)),
        })
    }

    /// 插入或更新段元数据行（upsert:保留首次 start_offset,更新 end_offset）
    ///
    /// WHY upsert:同一段跨多次微批 flush,end_offset 递增推进;start_offset
    /// 必须保持段首事件 seq（重建语义,覆盖会破坏 fork 覆盖校验）。
    pub fn insert_segment(&self, meta: &SegmentMeta) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("segments 表锁中毒: {p}"),
        })?;
        Self::upsert_segment_row(&conn, meta)
    }

    /// 批量插入事件行（单事务,写操作计数 +1）——微批写索引面入口
    ///
    /// # 计数语义（bench 门禁）
    /// 每次调用 = 一次 SQLite 写事务:单条直写 N 次调用 = N 次写;
    /// 微批写 N 条 = ceil(N/batch_size) 次调用。`transaction_count()` 供
    /// bench 计算 `syscall_reduction_pct`。
    pub fn insert_events(&self, rows: &[EventRow]) -> Result<(), StoreError> {
        if rows.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("events 表锁中毒: {p}"),
        })?;
        // 单事务批量:微批的 SQLite 面收益(一次 BEGIN/COMMIT = 一次写事务)
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("开启写事务失败: {e}"),
            })?;
        let result: Result<(), StoreError> = Self::insert_events_rows(&conn, rows);
        // 无论成败都提交/回滚,释放事务(不回滚会滞留写锁)
        match result {
            Ok(()) => conn
                .execute_batch("COMMIT")
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("提交写事务失败: {e}"),
                })?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        self.transaction_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// 单事务完成一次 flush 的全部 SQLite 写(段元数据 upsert + 事件批量)——flush 专用入口
    ///
    /// WHY 合并:原 flush 路径逐段调 insert_segment(每段一次 autocommit 写事务),
    /// N 段 = N+1 次写事务(段元数据 N 次 + 事件批量 1 次)。段元数据与事件同属
    /// 一次 flush 的原子单元,合并为单 BEGIN IMMEDIATE 后每次 flush 恒 1 次写事务
    /// (配合 synchronous=NORMAL,WAL 下 COMMIT 仅写 WAL 不 fsync 主库),写放大直接下降。
    /// 既有 insert_segment/insert_events 保留(fork/rebuild 等其他调用方兼容)。
    ///
    /// # 计数语义(bench 门禁)
    /// 与 insert_events 同口径:每次调用 = 一次 SQLite 写事务,transaction_count +1。
    ///
    /// # 内容指纹（R12 W3-1）
    /// 同一事务内为**本批涉及的会话**补记 `index_meta` 内容指纹（见
    /// [`IndexContentFingerprint`]）。WHY 放进同一事务而不是提交后另起一次写:
    /// 另起一次就是每次 flush 多一次 SQLite 写事务,CBMR 微批写的「一次 flush 一次
    /// 写事务」口径（bench 门禁）会被直接破掉。段文件字节数在事务**外**采集
    /// （调用方已 append+fsync,长度已定;避免持写锁做文件 IO）,事务内只做 SQL
    /// 聚合 + meta upsert。指纹写入失败**不回滚 flush**（簿记面不该弄丢已确认
    /// 事件）:只 warn,下次启动探针会重算并补记 ⇒ fail-open。
    pub fn insert_flush_batch(
        &self,
        segments: &[SegmentMeta],
        rows: &[EventRow],
    ) -> Result<(), StoreError> {
        if segments.is_empty() && rows.is_empty() {
            return Ok(());
        }
        // 指纹输入（无锁 IO;内存库/无数据目录 ⇒ 空 ⇒ 全程 no-op,旧行为不变）
        let fingerprint_inputs = match self.data_dir.as_deref() {
            Some(dir) => Self::collect_segment_file_lens(dir, segments, rows),
            None => Vec::new(),
        };
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("flush 批量写表锁中毒: {p}"),
        })?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("开启 flush 写事务失败: {e}"),
            })?;
        let result = (|| -> Result<(), StoreError> {
            for meta in segments {
                Self::upsert_segment_row(&conn, meta)?;
            }
            Self::insert_events_rows(&conn, rows)?;
            // 指纹是索引面的簿记:失败只 warn,绝不让它回滚已写入的事件行
            if let Err(e) = Self::upsert_content_fingerprints(&conn, &fingerprint_inputs) {
                warn!(error = %e, "flush 后补记内容指纹失败(fail-open;下次启动探针会重建并补记)");
            }
            Ok(())
        })();
        // 无论成败都提交/回滚,释放事务(不回滚会滞留写锁)
        match result {
            Ok(()) => conn
                .execute_batch("COMMIT")
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("提交 flush 写事务失败: {e}"),
                })?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        self.transaction_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// 段元数据 upsert(私有:insert_segment 与 insert_flush_batch 共用)
    fn upsert_segment_row(
        conn: &rusqlite::Connection,
        meta: &SegmentMeta,
    ) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO segments
                (segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(segment_id) DO UPDATE SET
                end_offset = excluded.end_offset",
            params![
                meta.segment_id.as_str(),
                meta.session_id.as_str(),
                meta.segment_index,
                meta.parent_segment_id.as_ref().map(SegmentId::as_str),
                meta.start_offset,
                meta.end_offset,
            ],
        )
        .map_err(|e| StoreError::Sqlite {
            reason: format!("插入段元数据 {} 失败: {e}", meta.segment_id),
        })?;
        Ok(())
    }

    /// 在已开启的事务中批量插入事件行(私有:insert_events 与 insert_flush_batch 共用)
    fn insert_events_rows(
        conn: &rusqlite::Connection,
        rows: &[EventRow],
    ) -> Result<(), StoreError> {
        let mut stmt = conn
            .prepare(
                "INSERT INTO events (offset, session_id, segment_id, payload)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("预编译事件插入语句失败: {e}"),
            })?;
        for row in rows {
            // WHY to_vec_named(map 编码)而非 to_vec(array 位置编码):
            // L0 EventMetadata.correlation_id 带 skip_serializing_if,array
            // 编码下字段错位(形态裁决 ADR-213 R8.1——非 ADR-004,它只裁「MessagePack
            // 而非 JSON」,不裁形态;event-bus 先例 shard.rs/bus.rs 均用 to_vec_named);
            // named 按字段名匹配,skip 不影响反序列化
            let payload =
                rmp_serde::to_vec_named(&row.event).map_err(|e| StoreError::Serialization {
                    reason: format!("SessionEvent rmp 序列化失败: {e}"),
                })?;
            stmt.execute(params![
                row.offset,
                row.session_id.as_str(),
                row.segment_id.as_str(),
                payload,
            ])
            .map_err(|e| StoreError::Sqlite {
                reason: format!("插入事件 offset={} 失败: {e}", row.offset),
            })?;
        }
        Ok(())
    }

    /// 采集「会话 → 逐段 (段索引, 文件字节数)」（无锁 IO,失败降级跳过该会话）
    ///
    /// WHY 只在有数据目录时才做:内存索引库没有段文件面,内容指纹无从谈起——
    /// 这条路径必须保持与 R11 行为**逐字节一致**（内存库测试/基准不受影响）。
    fn collect_segment_file_lens(
        data_dir: &Path,
        segments: &[SegmentMeta],
        rows: &[EventRow],
    ) -> Vec<(SessionId, Vec<(u32, u64)>)> {
        let mut ids: Vec<SessionId> = Vec::new();
        for meta in segments {
            if !ids.contains(&meta.session_id) {
                ids.push(meta.session_id.clone());
            }
        }
        for row in rows {
            if !ids.contains(&row.session_id) {
                ids.push(row.session_id.clone());
            }
        }
        let mut out = Vec::with_capacity(ids.len());
        for sid in ids {
            let files = match Self::segment_files(data_dir, &sid) {
                Ok(files) => files,
                Err(e) => {
                    // 目录不可读:跳过该会话的指纹(下次启动探针会重算)
                    warn!(session = %sid, error = %e, "枚举段文件失败,跳过内容指纹补记");
                    continue;
                }
            };
            let mut lens: Vec<(u32, u64)> = Vec::with_capacity(files.len());
            let mut ok = true;
            for (idx, path) in files {
                match std::fs::metadata(&path) {
                    Ok(m) => lens.push((idx, m.len())),
                    Err(e) => {
                        warn!(session = %sid, path = %path.display(), error = %e, "stat 段文件失败,跳过内容指纹补记");
                        ok = false;
                        break;
                    }
                }
            }
            // 无段文件的会话不写指纹:否则会写下一个「bytes=0」的假指纹,让探针
            // 每次启动都判不一致(fork 未追加/段已清理的会话即此形态)。
            if ok && !lens.is_empty() {
                out.push((sid, lens));
            }
        }
        out
    }

    /// 事务内按「索引聚合 + 预采集的段字节数」补写内容指纹（无额外写事务）
    fn upsert_content_fingerprints(
        conn: &rusqlite::Connection,
        inputs: &[(SessionId, Vec<(u32, u64)>)],
    ) -> Result<(), StoreError> {
        for (sid, lens) in inputs {
            let (rows, max_offset) = Self::index_row_stats(conn, sid)?;
            let fingerprint = compose_content_fingerprint(sid.as_str(), lens, rows, max_offset);
            Self::upsert_fingerprint_row(conn, sid, &fingerprint)?;
        }
        Ok(())
    }

    /// 索引面聚合:该会话 `events` 行数 + 最大 offset（走 `(session_id, offset)` 索引）
    ///
    /// WHY 用 COUNT 而不是「上次行数 + 本批行数」的增量算术:增量算术会与
    /// `rebuild_index`（按段文件实况补入）以及外部审计删行互相漂移,漂移后探针
    /// 会把健康库判成不一致;COUNT 每次都是精确值,代价是每次 flush 一次索引区间
    /// 扫描（登记为已知成本,见报告「可疑点」）。
    fn index_row_stats(
        conn: &rusqlite::Connection,
        session_id: &SessionId,
    ) -> Result<(u64, Option<u64>), StoreError> {
        let (count, max): (i64, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(offset) FROM events WHERE session_id = ?1",
                params![session_id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("统计会话 {} 行数/最大 offset 失败: {e}", session_id),
            })?;
        Ok((count as u64, max.map(|v| v.max(0) as u64)))
    }

    /// upsert 内容指纹行（幂等;`store_content_fingerprint` 与写路径共用）
    fn upsert_fingerprint_row(
        conn: &rusqlite::Connection,
        session_id: &SessionId,
        fingerprint: &IndexContentFingerprint,
    ) -> Result<(), StoreError> {
        let key = format!("{META_KEY_CONTENT_HASH_PREFIX}{}", session_id.as_str());
        conn.execute(
            "INSERT INTO index_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, fingerprint.encode()],
        )
        .map_err(|e| StoreError::Sqlite {
            reason: format!("写入会话 {} 内容指纹失败: {e}", session_id),
        })?;
        Ok(())
    }

    /// 在册会话 id 去重枚举（尾部探针与内容探针共用同一条取数口径）
    fn registered_sessions(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("segments 表锁中毒: {p}"),
        })?;
        let mut stmt = conn
            .prepare("SELECT DISTINCT session_id FROM segments")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("枚举在册会话失败: {e}"),
            })?;
        let rows =
            stmt.query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("读取在册会话失败: {e}"),
                })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Sqlite {
                reason: format!("遍历在册会话失败: {e}"),
            })
    }

    /// fork 会话 — 复制前缀段元数据到新会话（零事件数据拷贝）
    ///
    /// # 参数
    /// - `session_id`:父会话
    /// - `from_offset`:fork 点(全局 seq);前缀 = `[0, from_offset)` 的事件
    ///
    /// # 语义
    /// - 新会话事件 seq 从 `from_offset` 起(调用方以 `from_offset` 作为
    ///   新会话首段 start_seq),回放时前缀事件 + 新事件按 seq 无缝拼接
    /// - 复制段 = `start_offset < from_offset` 的段;跨 fork 点的段被截断
    ///   （end_offset = from_offset - 1）;覆盖校验失败 → `ForkViolation`
    /// - 零数据拷贝:events 表零插入、JSONL 段文件零创建（见模块文档）
    ///
    /// # 返回
    /// 新会话 ID（UUIDv7）
    pub fn fork(&self, session_id: &SessionId, from_offset: u64) -> Result<SessionId, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("fork 锁中毒: {p}"),
        })?;

        // 1. 读父会话全部段(按段索引升序)
        let mut stmt = conn
            .prepare(
                "SELECT segment_id, segment_index, parent_segment_id, start_offset, end_offset
                 FROM segments WHERE session_id = ?1 ORDER BY segment_index ASC",
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("预编译父段查询失败: {e}"),
            })?;
        let parent_rows = stmt
            .query_map(params![session_id.as_str()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询父会话 {} 段失败: {e}", session_id),
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Sqlite {
                reason: format!("读取父会话段行失败: {e}"),
            })?;

        // 2. 过滤前缀段 + 部分截断
        let mut copied: Vec<(String, i64, Option<String>, i64, i64)> = Vec::new();
        for (seg_id, idx, parent, start, end) in parent_rows {
            if (start as u64) < from_offset {
                let mut end_adj = end as u64;
                if (end as u64) >= from_offset {
                    end_adj = from_offset - 1; // 跨 fork 点:截断
                }
                copied.push((seg_id, idx, parent, start, end_adj as i64));
            }
        }

        // 3. 覆盖校验:前缀必须覆盖 [0, from_offset)
        if from_offset > 0 {
            if copied.is_empty() {
                return Err(StoreError::ForkViolation {
                    reason: format!(
                        "父会话 {} 在 fork 点 {from_offset} 前无任何段(可能 fork 点超出历史)",
                        session_id
                    ),
                });
            }
            let first_start = copied.first().map(|c| c.3 as u64).unwrap_or(0);
            let last_end = copied.last().map(|c| c.4 as u64).unwrap_or(0);
            if first_start != 0 {
                return Err(StoreError::ForkViolation {
                    reason: format!("前缀段覆盖不连续:首个复制段 start_offset={first_start} != 0"),
                });
            }
            if last_end != from_offset - 1 {
                return Err(StoreError::ForkViolation {
                    reason: format!(
                        "前缀段未覆盖到 fork 点:最后复制段 end_offset={last_end}, 期望 {}",
                        from_offset - 1
                    ),
                });
            }
        }

        // 4. 插入复制行(新 segment_id + parent 引用)+ 新会话 ID
        let new_session = SessionId::generate();
        {
            let mut stmt = conn
                .prepare(
                    "INSERT INTO segments
                        (segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("预编译 fork 复制行插入失败: {e}"),
                })?;
            for (parent_seg, idx, _, start, end) in &copied {
                stmt.execute(params![
                    SegmentId::generate().as_str(),
                    new_session.as_str(),
                    idx,
                    Some(parent_seg.as_str()),
                    start,
                    end,
                ])
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("插入 fork 复制段行失败: {e}"),
                })?;
            }
        }
        debug!(
            "fork 会话 {} -> {}: 复制 {} 个前缀段元数据(零事件数据拷贝), fork 点 {}",
            session_id,
            new_session,
            copied.len(),
            from_offset
        );
        Ok(new_session)
    }

    /// 按会话读回事件（前缀引用段 + 自己段,按 seq 升序）——树索引回读路径
    ///
    /// # 参数
    /// - `session_id`:目标会话（可为 fork 产生的新会话）
    /// - `from`:可选 seq 下界（断点续传,None = 从头）
    ///
    /// # 合并语义
    /// 前缀事件（经 parent_segment_id 引用,offset < fork 点）天然排在
    /// 自己段事件（offset >= fork 点）之前,直接拼接即为全序（ADR-109 归并预演）。
    pub fn read_events(
        &self,
        session_id: &SessionId,
        from: Option<u64>,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("读回锁中毒: {p}"),
        })?;
        let from = from.unwrap_or(0);

        // 前缀引用段的 end 上限(= fork 点 - 1;无前缀段为 NULL)
        let prefix_end: Option<i64> = conn
            .query_row(
                "SELECT MAX(end_offset) FROM segments
                 WHERE session_id = ?1 AND parent_segment_id IS NOT NULL",
                params![session_id.as_str()],
                |r| r.get(0),
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询前缀段 end 上限失败: {e}"),
            })?;

        // 前缀事件(来自父段引用)
        let mut prefix: Vec<StoredEvent> = Vec::new();
        if let Some(pe) = prefix_end {
            if from <= (pe as u64) {
                let mut stmt = conn
                    .prepare(
                        "SELECT e.offset, e.payload FROM events e
                         WHERE e.segment_id IN (
                            SELECT parent_segment_id FROM segments
                            WHERE session_id = ?1 AND parent_segment_id IS NOT NULL
                         )
                         AND e.offset >= ?2 AND e.offset <= ?3
                         ORDER BY e.offset ASC",
                    )
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("预编译前缀事件查询失败: {e}"),
                    })?;
                let rows = stmt
                    .query_map(params![session_id.as_str(), from, pe], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
                    })
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("查询前缀事件失败: {e}"),
                    })?;
                for row in rows {
                    let (off, payload) = row.map_err(|e| StoreError::Sqlite {
                        reason: format!("读取前缀事件行失败: {e}"),
                    })?;
                    let event: SessionEvent =
                        rmp_serde::from_slice(&payload).map_err(|e| StoreError::Serialization {
                            reason: format!("前缀事件 rmp 反序列化失败: {e}"),
                        })?;
                    prefix.push(StoredEvent {
                        offset: off as u64,
                        event,
                    });
                }
            }
        }

        // 自己段事件
        let mut stmt = conn
            .prepare(
                "SELECT offset, payload FROM events
                 WHERE session_id = ?1 AND offset >= ?2
                 ORDER BY offset ASC",
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("预编译本段事件查询失败: {e}"),
            })?;
        let rows = stmt
            .query_map(params![session_id.as_str(), from], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询本段事件失败: {e}"),
            })?;
        let mut own: Vec<StoredEvent> = Vec::new();
        for row in rows {
            let (off, payload) = row.map_err(|e| StoreError::Sqlite {
                reason: format!("读取本段事件行失败: {e}"),
            })?;
            let event: SessionEvent =
                rmp_serde::from_slice(&payload).map_err(|e| StoreError::Serialization {
                    reason: format!("本段事件 rmp 反序列化失败: {e}"),
                })?;
            own.push(StoredEvent {
                offset: off as u64,
                event,
            });
        }

        // 拼接:前缀(< fork 点) + 自己(>= fork 点),天然全序
        prefix.extend(own);
        Ok(prefix)
    }

    /// 会话树（WI-18）— 该会话全部段节点 + parent 指针
    pub fn tree(&self, session_id: &SessionId) -> Result<SessionTree, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("会话树锁中毒: {p}"),
        })?;
        let nodes = query_segment_nodes(&conn, session_id)?;
        Ok(SessionTree {
            session_id: session_id.clone(),
            nodes,
        })
    }

    /// 按段 ID 查段节点（fork_tree 跨会话收集祖先节点用）
    pub fn segment_node(&self, segment_id: &SegmentId) -> Result<SegmentNode, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("段节点查询锁中毒: {p}"),
        })?;
        let node: Option<SegmentNode> = conn
            .query_row(
                "SELECT segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset
                 FROM segments WHERE segment_id = ?1",
                params![segment_id.as_str()],
                row_to_node,
            )
            .optional()
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询段节点 {} 失败: {e}", segment_id),
            })?;
        node.ok_or_else(|| StoreError::NotFound {
            what: format!("段 {} 不存在", segment_id),
        })
    }

    /// fork 树视图（WI-18）— fork 子会话的完整树:自己会话节点 + 沿 parent
    /// 引用链收集的祖先段节点（跨会话,去重）
    ///
    /// # WHY 完整视图
    /// `tree()` 只含本会话 segments 行,parent 指针指向外部节点（父段）;
    /// fork_tree 沿指针把祖先节点也拉入 nodes,使树从根到叶完整可遍历。
    pub fn fork_tree(&self, session_id: &SessionId) -> Result<SessionTree, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("fork 树锁中毒: {p}"),
        })?;
        let mut nodes = query_segment_nodes(&conn, session_id)?;
        // 沿 parent 链收集祖先节点（跨会话）;seen 防环（fork 链理论无环,
        // 防御性去重避免异常数据死循环）
        let mut seen: std::collections::HashSet<SegmentId> =
            nodes.iter().map(|n| n.segment_id.clone()).collect();
        let mut frontier: Vec<SegmentId> = nodes
            .iter()
            .filter_map(|n| n.parent_segment_id.clone())
            .collect();
        while let Some(pid) = frontier.pop() {
            if !seen.insert(pid.clone()) {
                continue;
            }
            let parent_node = query_segment_node(&conn, &pid)?;
            if let Some(gp) = parent_node.parent_segment_id.clone() {
                frontier.push(gp);
            }
            nodes.push(parent_node);
        }
        // 祖先节点按段索引升序稳定化输出顺序（原顺序为 BFS 收集序）
        nodes.sort_by_key(|n| n.segment_index);
        Ok(SessionTree {
            session_id: session_id.clone(),
            nodes,
        })
    }

    /// 祖先链（WI-18）— 会话的祖先段 ID,从直接父层到根层（每层内按
    /// segment_id 字典序保证确定性输出）
    ///
    /// # 示例
    /// 链式 fork A → B → C:ancestors(C) = [B 的段, A 的段]
    pub fn ancestors(&self, session_id: &SessionId) -> Result<Vec<SegmentId>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("祖先链锁中毒: {p}"),
        })?;
        let nodes = query_segment_nodes(&conn, session_id)?;
        let mut out: Vec<SegmentId> = Vec::new();
        let mut seen: std::collections::HashSet<SegmentId> = std::collections::HashSet::new();
        // 分层 BFS:layer = 当前层的直接父段集合,处理完再推入祖父层
        let mut layer: std::collections::HashSet<SegmentId> = nodes
            .iter()
            .filter_map(|n| n.parent_segment_id.clone())
            .collect();
        while !layer.is_empty() {
            let mut next: std::collections::HashSet<SegmentId> = std::collections::HashSet::new();
            let mut ordered: Vec<SegmentId> = layer.iter().cloned().collect();
            ordered.sort();
            for pid in ordered {
                if !seen.insert(pid.clone()) {
                    continue;
                }
                out.push(pid.clone());
                let node = query_segment_node(&conn, &pid)?;
                if let Some(gp) = node.parent_segment_id {
                    next.insert(gp);
                }
            }
            layer = next;
        }
        Ok(out)
    }

    /// 会话可见段来源解析 — k-way 归并回放的段文件定位（ADR-109）
    ///
    /// 对会话每段（含 fork 复制行）沿 parent 链解析到物理文件;fork 复制行
    /// 的 `end_offset` 保持会话视角（截断段 = fork 点 - 1）,回放按此过滤。
    pub fn segment_sources(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SegmentSource>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("段来源解析锁中毒: {p}"),
        })?;
        let nodes = query_segment_nodes(&conn, session_id)?;
        let mut out = Vec::with_capacity(nodes.len());
        for node in nodes {
            let (file_session, file_idx) = resolve_physical(&conn, &node.segment_id, 0)?;
            out.push(SegmentSource {
                segment_id: node.segment_id,
                file_session,
                segment_index: file_idx,
                parent_segment_id: node.parent_segment_id,
                start_offset: node.start_offset,
                end_offset: node.end_offset,
            });
        }
        Ok(out)
    }

    /// 索引重建（崩溃自愈,幂等）— 段文件完整但 SQLite 索引缺行时扫描段文件
    /// 重建 segments/events 行
    ///
    /// # 场景（T2 审查 Important #2 承接）
    /// `flush_sync` 中段文件 fsync 与索引 insert 非原子:崩溃窗口可能出现
    /// 「段文件已 fsync、索引缺行」。本方法以段文件为权威源自愈。
    ///
    /// # 幂等性
    /// events 行用 `INSERT OR IGNORE`（已存在 offset 跳过）;segments 行
    /// upsert 保留 start_offset。重复调用无副作用（第二次 rows_inserted = 0,
    /// rows_skipped = 全量）。
    ///
    /// # 范围
    /// 只重建该会话**物理拥有**的段文件（`<stem>.<idx>.jsonl`）;fork 复制行
    /// 不重建（其 events 行本来就零拷贝,事件经父段引用可见）。
    ///
    /// # 内容指纹（R12 W3-1）
    /// 重建是「索引与权威源重新对齐」的时刻,顺带在**同一事务内**把内容指纹刷成
    /// 段文件实况（`bytes/rows/max_offset` 全部来自本次扫描,零额外 IO）。
    /// 口径要点:补入式重建不删孤儿行（索引越前方向）⇒ 指纹按**段文件实况**记,
    /// 而不是按索引行数记——否则孤儿行会让每次启动都判不一致,陷入重建循环。
    pub fn rebuild_index(
        &self,
        data_dir: &Path,
        session_id: &SessionId,
    ) -> Result<RebuildStats, StoreError> {
        // 1. 枚举段文件（无锁 IO;与尾部探针共享同一枚举规则）
        let files = Self::segment_files(data_dir, session_id)?;

        // 2. 读全部段文件记录（无锁 IO;半条停止——已确认记录才重建）
        //    同时采集逐段字节数（内容指纹的 bytes 分量;含尾部半条,与实况侧同源）
        let mut scanned: Vec<(u32, Vec<crate::segment::SegmentRecord>)> = Vec::new();
        let mut lens: Vec<(u32, u64)> = Vec::with_capacity(files.len());
        let mut live_rows: u64 = 0;
        let mut live_max: Option<u64> = None;
        for (idx, path) in &files {
            let len = std::fs::metadata(path)
                .map_err(|e| StoreError::Io {
                    context: format!("stat 段文件 {} 失败(重建指纹)", path.display()),
                    source: e,
                })?
                .len();
            lens.push((*idx, len));
            let mut reader = SegmentFileReader::open(path)?;
            let mut records = Vec::new();
            while let Some(rec) = reader.next_record()? {
                live_rows += 1;
                live_max = Some(live_max.map_or(rec.seq, |m: u64| m.max(rec.seq)));
                records.push(rec);
            }
            // 空段文件跳过:无记录无法推断 start_seq,后续 writer flush 会补齐
            if !records.is_empty() {
                scanned.push((*idx, records));
            }
        }
        let live_fingerprint =
            compose_content_fingerprint(session_id.as_str(), &lens, live_rows, live_max);

        // 3. 锁内批量写库（单事务;upsert segments + INSERT OR IGNORE events）
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("rebuild 锁中毒: {p}"),
        })?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| StoreError::Sqlite {
                reason: format!("rebuild 开启事务失败: {e}"),
            })?;
        let result = (|| -> Result<RebuildStats, StoreError> {
            let mut stats = RebuildStats {
                segments_scanned: scanned.len() as u64,
                rows_inserted: 0,
                rows_skipped: 0,
            };
            {
                let mut stmt_seg = conn
                    .prepare(
                        "INSERT INTO segments
                            (segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset)
                         VALUES (?1, ?2, ?3, NULL, ?4, ?5)
                         ON CONFLICT(segment_id) DO UPDATE SET end_offset = excluded.end_offset",
                    )
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("rebuild 预编译段 upsert 失败: {e}"),
                    })?;
                let mut stmt_ev = conn
                    .prepare(
                        "INSERT OR IGNORE INTO events (offset, session_id, segment_id, payload)
                         VALUES (?1, ?2, ?3, ?4)",
                    )
                    .map_err(|e| StoreError::Sqlite {
                        reason: format!("rebuild 预编译事件插入失败: {e}"),
                    })?;
                for (idx, records) in &scanned {
                    // 段 ID:复用已有（索引残留行）或生成新（崩溃缺行）
                    let seg_id = query_segment_id(&conn, session_id, *idx)?
                        .unwrap_or_else(SegmentId::generate);
                    let start =
                        records
                            .first()
                            .map(|r| r.seq)
                            .ok_or_else(|| StoreError::InvalidInput {
                                reason: "rebuild 空段文件不可达(已在上层过滤)".into(),
                            })?;
                    let end = records.last().map(|r| r.seq).unwrap_or(start);
                    stmt_seg
                        .execute(params![
                            seg_id.as_str(),
                            session_id.as_str(),
                            idx,
                            start,
                            end,
                        ])
                        .map_err(|e| StoreError::Sqlite {
                            reason: format!("rebuild upsert 段 {} 失败: {e}", seg_id),
                        })?;
                    for rec in records {
                        // WHY to_vec_named:与 insert_events 同源（map 编码;形态裁决
                        // ADR-213 R8.1,非 ADR-004）
                        let payload = rmp_serde::to_vec_named(&rec.event).map_err(|e| {
                            StoreError::Serialization {
                                reason: format!("rebuild 事件 rmp 序列化失败: {e}"),
                            }
                        })?;
                        let inserted = stmt_ev
                            .execute(params![
                                rec.seq,
                                session_id.as_str(),
                                seg_id.as_str(),
                                payload,
                            ])
                            .map_err(|e| StoreError::Sqlite {
                                reason: format!("rebuild 插入事件 offset={} 失败: {e}", rec.seq),
                            })?;
                        // 去重统计:INSERT OR IGNORE 返回 0 = 已存在跳过
                        if inserted == 0 {
                            stats.rows_skipped += 1;
                        } else {
                            stats.rows_inserted += 1;
                        }
                    }
                }
            }
            // 指纹随重建一并落库(同事务):下次启动探针读到的就是「已对齐」态
            Self::upsert_fingerprint_row(&conn, session_id, &live_fingerprint)?;
            Ok(stats)
        })();
        match result {
            Ok(stats) => conn
                .execute_batch("COMMIT")
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("rebuild 提交事务失败: {e}"),
                })
                .map(|_| stats),
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// 枚举某会话的段文件（无锁 IO）:`<stem>.<idx>.jsonl`,按 idx 升序
    ///
    /// rebuild_index 与尾部探针共享同一枚举规则——直接复用 segment.rs 的
    /// `list_segment_files`(writer 重开恢复同源),只补路径映射,三处零重复。
    fn segment_files(
        data_dir: &Path,
        session_id: &SessionId,
    ) -> Result<Vec<(u32, PathBuf)>, StoreError> {
        let idxs = list_segment_files(data_dir, session_id)?;
        Ok(idxs
            .into_iter()
            .map(|idx| (idx, segment_path(data_dir, session_id, idx)))
            .collect())
    }

    /// 树索引 schema 版本(R11 W3b,ADR-213 Checkpoint F3 同款模式)
    ///
    /// 0 = index_meta 面缺失(理论上不可达——init 恒补建;保守口径防手工删表);
    /// 1 = 当前两表 + index_meta 形态。未来 schema 变更以此为迁移分支判据。
    pub fn schema_version(&self) -> Result<u16, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("index_meta 表锁中毒: {p}"),
        })?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM index_meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| StoreError::Sqlite {
                reason: format!("读取 schema 版本失败: {e}"),
            })?;
        Ok(raw.and_then(|v| v.parse::<u16>().ok()).unwrap_or(0))
    }

    /// 会话在索引中的最大 offset(尾部探针判据;None = 索引零行)
    pub fn max_offset(&self, session_id: &SessionId) -> Result<Option<u64>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("events 表锁中毒: {p}"),
        })?;
        let raw: Option<i64> = conn
            .query_row(
                "SELECT MAX(offset) FROM events WHERE session_id = ?1",
                params![session_id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询 max_offset 失败: {e}"),
            })?;
        Ok(raw.map(|v| v.max(0) as u64))
    }

    /// 打开时索引尾部一致性探针 + 自愈(ADR-295 D4;`StoreConfig::
    /// repair_index_on_open` 门控,调用方为 `CbmrWriter::open_blocking`)
    ///
    /// # 判据(覆盖登记失败模式,廉价不整扫)
    /// 对 segments 表在册的每个会话:取其**最后一个段文件**的尾部完整记录 seq
    /// (半条丢弃,与 recover_tail 同语义)vs 索引 `MAX(offset)`:
    /// - 索引 < 段尾 ⇒ **索引缺尾行**(登记失败模式:段文件已 fsync、索引 insert
    ///   前崩溃)⇒ 调 `rebuild_index` 幂等补行;
    /// - 索引 > 段尾 ⇒ 索引越前(非登记模式,反常)⇒ 只报告不重建;
    /// - 相等 ⇒ 健康。
    ///
    /// # 覆盖范围诚实声明
    /// 中段随机缺行/索引整库丢失不在本探针判据内(前者属全量一致性 =
    /// `scripts/audit_memory_consistency.py` 诊断面;后者 session_id 无法从
    /// 段记录反查——SessionEvent 不携带 session_id,恢复走运维面)。
    /// fork 会话只有段元数据行、无段文件时跳过(无尾部可比)。
    ///
    /// > R12 W3-1 补充:`中段随机缺行` 这一条现已由**内容指纹探针**
    /// > [`TreeIndex::probe_content_consistency`] 覆盖(比对 meta 指纹与段文件
    /// > 实况,不一致即重建);本方法仍只管尾部缺口,两者判据不重叠、可独立开关。
    pub fn verify_and_repair_tails(
        &self,
        data_dir: &Path,
    ) -> Result<Vec<IndexRepairEntry>, StoreError> {
        let mut report = Vec::new();
        let sessions = self.registered_sessions()?;
        for sid_str in sessions {
            let session_id = SessionId::new(sid_str.as_str());
            let files = Self::segment_files(data_dir, &session_id)?;
            let Some((_, last_path)) = files.last().cloned() else {
                // 只有段元数据行、无段文件(fork 未追加)——无尾部可比
                report.push(IndexRepairEntry {
                    session_id: sid_str,
                    damaged: false,
                    repaired: false,
                    stats: None,
                    detail: "无段文件(fork 未追加或段已清理),跳过尾部探针".to_string(),
                });
                continue;
            };
            // 读最后段文件的尾部完整记录(顺序迭代,O(单文件);半条自然丢弃)
            let mut reader = SegmentFileReader::open(&last_path)?;
            let mut last_seq: Option<u64> = None;
            while let Some(rec) = reader.next_record()? {
                last_seq = Some(rec.seq);
            }
            let Some(tail_seq) = last_seq else {
                report.push(IndexRepairEntry {
                    session_id: sid_str,
                    damaged: false,
                    repaired: false,
                    stats: None,
                    detail: "最后段文件无完整记录(空段/半条),跳过尾部探针".to_string(),
                });
                continue;
            };
            let index_max = self.max_offset(&session_id)?;
            match index_max {
                Some(mx) if mx >= tail_seq => {
                    report.push(IndexRepairEntry {
                        session_id: sid_str,
                        damaged: mx > tail_seq,
                        repaired: false,
                        stats: None,
                        detail: format!(
                            "尾部一致(mx={mx} ≥ tail={tail_seq})或索引越前,无需/不做重建"
                        ),
                    });
                }
                _ => {
                    // 索引缺尾行(None 或 mx < tail_seq)——登记失败模式,幂等重建
                    let stats = self.rebuild_index(data_dir, &session_id)?;
                    debug!(
                        session = %sid_str,
                        inserted = stats.rows_inserted,
                        skipped = stats.rows_skipped,
                        "索引尾部缺口已自愈重建(ADR-295 D4)"
                    );
                    report.push(IndexRepairEntry {
                        session_id: sid_str,
                        damaged: true,
                        repaired: true,
                        stats: Some(stats),
                        detail: format!(
                            "索引缺尾行(index_max={index_max:?} < tail={tail_seq}),已重建"
                        ),
                    });
                }
            }
        }
        Ok(report)
    }

    /// 读 `index_meta` 里的内容指纹（每会话一条;`None` = 无该字段/不可解析）
    ///
    /// # WHY 缺字段是 `None` 而不是错误（字段可缺省的根据）
    /// 内容指纹是 R12 新增的**纯记账面**,老库（R11 及更早）里根本不存在这些 key,
    /// 手工删过 `index_meta` 行的库也没有。缺字段的语义只有「从未记录过」= 未知,
    /// 不构成「库坏了」的证据;读侧把它当 `None` 返回,写路径与探针各自决定处置
    /// （探针:按不一致通道走一次幂等重建并补记,见
    /// [`TreeIndex::probe_content_consistency`]）。因此老库不需要迁移脚本、
    /// 不需要 schema 版本跳变,也不需要任何读路径分支——这正是把指纹放进
    /// key/value 的 `index_meta` 而不是新列/新表的原因。
    pub fn content_fingerprint(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<IndexContentFingerprint>, StoreError> {
        let key = format!("{META_KEY_CONTENT_HASH_PREFIX}{}", session_id.as_str());
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("index_meta 表锁中毒: {p}"),
        })?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM index_meta WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| StoreError::Sqlite {
                reason: format!("读取会话 {} 内容指纹失败: {e}", session_id),
            })?;
        Ok(raw.as_deref().and_then(IndexContentFingerprint::parse))
    }

    /// 写（覆盖）`index_meta` 里的内容指纹 — upsert,幂等
    pub fn store_content_fingerprint(
        &self,
        session_id: &SessionId,
        fingerprint: &IndexContentFingerprint,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("index_meta 表锁中毒: {p}"),
        })?;
        Self::upsert_fingerprint_row(&conn, session_id, fingerprint)
    }

    /// 实况指纹（薄 IO 包装）— 扫该会话全部段文件,取字节数 + 完整记录数 + 最大 seq
    ///
    /// 读侧规则与 `rebuild_index`/`verify_and_repair_tails` 同源
    /// （`SegmentFileReader`,尾部半条自然丢弃）:半条**不计行**,但**计字节**
    /// ——「写入中断残留半条」正是靠字节数分量被判出来的。
    pub fn live_content_fingerprint(
        &self,
        data_dir: &Path,
        session_id: &SessionId,
    ) -> Result<IndexContentFingerprint, StoreError> {
        let files = Self::segment_files(data_dir, session_id)?;
        Ok(Self::live_content_fingerprint_from_files(session_id, &files)?.0)
    }

    /// 由已枚举的段文件列表算实况指纹 + 逐段 (段索引, 字节数)（探针复用,免重复 readdir）
    fn live_content_fingerprint_from_files(
        session_id: &SessionId,
        files: &[(u32, PathBuf)],
    ) -> Result<(IndexContentFingerprint, Vec<(u32, u64)>), StoreError> {
        let mut lens: Vec<(u32, u64)> = Vec::with_capacity(files.len());
        let mut rows: u64 = 0;
        let mut max_offset: Option<u64> = None;
        for (idx, path) in files {
            let len = std::fs::metadata(path)
                .map_err(|e| StoreError::Io {
                    context: format!("stat 段文件 {} 失败(实况指纹)", path.display()),
                    source: e,
                })?
                .len();
            lens.push((*idx, len));
            let mut reader = SegmentFileReader::open(path)?;
            while let Some(rec) = reader.next_record()? {
                rows += 1;
                max_offset = Some(max_offset.map_or(rec.seq, |m: u64| m.max(rec.seq)));
            }
        }
        Ok((
            compose_content_fingerprint(session_id.as_str(), &lens, rows, max_offset),
            lens,
        ))
    }

    /// 索引面指纹 — 与实况侧**同一合成函数**,只是把 `rows`/`max_offset` 换成
    /// 索引里的聚合值（字节数分量仍取段文件:索引不存段文件长度）
    ///
    /// WHY 需要它:索引中段缺行不会改变段文件任何分量,只有把索引自身的聚合值
    /// 代进同一个函数才暴露得出来（见 [`content_probe_verdict`] 的三向说明）。
    fn index_side_fingerprint(
        &self,
        session_id: &SessionId,
        lens: &[(u32, u64)],
    ) -> Result<IndexContentFingerprint, StoreError> {
        let (rows, max_offset) = {
            let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
                reason: format!("events 表锁中毒: {p}"),
            })?;
            Self::index_row_stats(&conn, session_id)?
        };
        Ok(compose_content_fingerprint(
            session_id.as_str(),
            lens,
            rows,
            max_offset,
        ))
    }

    /// 立即按段文件实况重算并写入内容指纹（运维/测试/自愈入口,幂等）
    pub fn refresh_content_fingerprint(
        &self,
        data_dir: &Path,
        session_id: &SessionId,
    ) -> Result<IndexContentFingerprint, StoreError> {
        let live = self.live_content_fingerprint(data_dir, session_id)?;
        self.store_content_fingerprint(session_id, &live)?;
        Ok(live)
    }

    /// 内容一致性探针已触发重建的会话次数（R12 W3-1 计数面）
    #[must_use]
    pub fn content_probe_rebuild_count(&self) -> u64 {
        self.content_probe_rebuilds.load(Ordering::Relaxed)
    }

    /// 启动期内容一致性探针（R12 W3-1）— 纯判定 + 薄 IO 的启动自检/自愈
    ///
    /// # 判据（三向比较,详见 [`content_probe_verdict`]）
    /// 对 `segments` 表在册的每个**有段文件**的会话:
    /// 1. 从段文件（权威源）重算实况指纹 `live`;
    /// 2. 从**索引面**按同一合成函数重算 `index_side`（行数/最大 offset 取索引）;
    /// 3. 读 `index_meta` 里上次写入的 `stored`（缺字段 ⇒ `None`）;
    /// - 三者一致 ⇒ 健康;
    /// - `index_side` 行数**多于** `live` ⇒ 孤儿行（索引越前）⇒ 只报告不重建;
    /// - 其余不一致（含 `stored` 缺失）⇒ 调 [`TreeIndex::rebuild_index`] 重建、
    ///   按实况刷新指纹、计数 +1、`warn!` 记一条（本 crate 不持事件总线,且仓内
    ///   **不存在** `IndexDegraded` 事件变体 ⇒ 按既有约定用 warn 日志 + 返回报告
    ///   作可观测面,**不新增事件类型**）。
    ///
    /// # WHY 不一致就重建,而不是报错退出（权威源语义,ADR-290 D2）
    /// 段文件是权威源、树索引只是加速面。索引不一致的正确处置是「照权威源补齐
    /// 加速面」——数据本身一条没丢,报错只会把「可自愈的索引脏读」升级成
    /// 「进程起不来」,把可用性事故凭空造出来。`rebuild_index` 是
    /// `INSERT OR IGNORE` 的幂等补齐（重复调用 rows_inserted=0）,所以「多重建一次」
    /// 的代价只是 O(段文件字节) 的扫描,而「误报错退出」的代价是整机不可用。
    ///
    /// # WHY fail-open（重建/读实况失败不向上抛、不 panic）
    /// 探针属于自愈优化面,不能成为打开路径的单点失败（与 N-4 降级姿态、
    /// `verify_and_repair_tails` 的降级口径一致）:单会话失败 ⇒ 记 `FailedOpen` +
    /// warn,继续用**现有索引**服务（它至多是陈旧,不是不可用）,下次启动再试。
    /// 只有「枚举在册会话」这一步（= 整个 SQLite 不可用）才返回 `Err`,由调用方
    /// 决定是否降级——本函数自身不 panic、不做 IO 重试。
    ///
    /// # 覆盖范围（诚实声明）
    /// - 覆盖:尾部半条/截断、整段文件消失、索引中段随机缺行、老库缺指纹字段、
    ///   flush 崩溃窗口（段已落盘、索引缺行）;
    /// - **不覆盖**:索引整库丢失（`session_id` 无法从段记录反查——与尾部探针同一
    ///   限制）、等长原地改字节（bit rot;指纹只有字节数/行数/摘要,无全量内容哈希）;
    /// - 索引越前的孤儿行:判 `IndexAhead` 只报告不重建（补入式重建删不掉,
    ///   处置归 ADR-290 D5 审计器）,因此不会陷入「每次启动都重建」;
    /// - fork 复制行无物理文件 ⇒ `Skipped`（无实况可比）,与尾部探针同口径。
    ///
    /// # 开关
    /// 环境变量 [`INDEX_PROBE_ENV`] = `off`/`0`/`false`/`no` ⇒ 整体跳过
    /// （`disabled = true`,零重建、零告警）。默认开启。
    pub fn probe_content_consistency(
        &self,
        data_dir: &Path,
    ) -> Result<ContentProbeReport, StoreError> {
        self.probe_content_consistency_with(data_dir, |_| {})
    }

    /// 同 [`TreeIndex::probe_content_consistency`],但把每条记录交给观察者
    ///
    /// WHY 观察者而不是全局日志断言:本 crate 无 `tracing-subscriber` 依赖
    /// （不新增 crate 的硬约束）,进程内捕获日志不可行;把「记录了什么」作为
    /// 数据交出去,既能让测试断言通知路径真的被调用,也给上层（健康探针/组合根）
    /// 一个零依赖的接入点。
    pub fn probe_content_consistency_with<F>(
        &self,
        data_dir: &Path,
        mut observer: F,
    ) -> Result<ContentProbeReport, StoreError>
    where
        F: FnMut(&ContentProbeEntry),
    {
        let raw_env = std::env::var(INDEX_PROBE_ENV).ok();
        if !content_probe_enabled(raw_env.as_deref()) {
            warn!(
                env = INDEX_PROBE_ENV,
                "内容一致性探针被环境变量关闭,本次启动跳过(不影响索引可用性)"
            );
            return Ok(ContentProbeReport {
                disabled: true,
                entries: Vec::new(),
            });
        }
        let mut report = ContentProbeReport::default();
        for sid_str in self.registered_sessions()? {
            let session_id = SessionId::new(sid_str.as_str());
            // 段文件枚举失败 ⇒ fail-open 跳过本会话(索引照旧服务)
            let files = match Self::segment_files(data_dir, &session_id) {
                Ok(files) => files,
                Err(e) => {
                    let entry = ContentProbeEntry {
                        session_id: sid_str,
                        verdict: ContentProbeVerdict::Undecidable,
                        live: None,
                        outcome: ContentProbeOutcome::FailedOpen,
                        detail: format!("枚举段文件失败(fail-open,保留现有索引): {e}"),
                    };
                    warn!(session = %entry.session_id, error = %e, "内容探针枚举段文件失败,跳过本会话");
                    observer(&entry);
                    report.entries.push(entry);
                    continue;
                }
            };
            if files.is_empty() {
                // 只有段元数据行、无段文件(fork 未追加 / 段已清理)——无实况可比
                let entry = ContentProbeEntry {
                    session_id: sid_str,
                    verdict: ContentProbeVerdict::Undecidable,
                    live: None,
                    outcome: ContentProbeOutcome::Skipped,
                    detail: "无段文件(fork 未追加或段已清理),跳过内容探针".to_string(),
                };
                observer(&entry);
                report.entries.push(entry);
                continue;
            }
            // 实况重算失败 ⇒ fail-open(段文件损坏到读不出记录时,索引仍是可用面)
            let (live, lens) = match Self::live_content_fingerprint_from_files(&session_id, &files)
            {
                Ok(pair) => pair,
                Err(e) => {
                    let entry = ContentProbeEntry {
                        session_id: sid_str,
                        verdict: ContentProbeVerdict::Undecidable,
                        live: None,
                        outcome: ContentProbeOutcome::FailedOpen,
                        detail: format!("实况指纹不可读(fail-open,保留现有索引): {e}"),
                    };
                    warn!(session = %entry.session_id, error = %e, "内容探针重算实况指纹失败,跳过本会话");
                    observer(&entry);
                    report.entries.push(entry);
                    continue;
                }
            };
            // 索引面按同一合成函数重算(行数/最大 offset 取索引)⇒ 才能发现「索引中段缺行」
            let index_side = match self.index_side_fingerprint(&session_id, &lens) {
                Ok(fp) => fp,
                Err(e) => {
                    let entry = ContentProbeEntry {
                        session_id: sid_str,
                        verdict: ContentProbeVerdict::Undecidable,
                        live: Some(live),
                        outcome: ContentProbeOutcome::FailedOpen,
                        detail: format!("索引面指纹不可算(fail-open,保留现有索引): {e}"),
                    };
                    warn!(session = %entry.session_id, error = %e, "内容探针计算索引面指纹失败,跳过本会话");
                    observer(&entry);
                    report.entries.push(entry);
                    continue;
                }
            };
            // meta 读失败 ⇒ 按「缺字段」处理(仍走重建通道,不阻断)
            let stored = match self.content_fingerprint(&session_id) {
                Ok(stored) => stored,
                Err(e) => {
                    warn!(session = %sid_str, error = %e, "读取内容指纹失败,按缺失处理");
                    None
                }
            };
            let verdict = content_probe_verdict(stored.as_ref(), &live, &index_side);
            let entry = match verdict {
                ContentProbeVerdict::Healthy => ContentProbeEntry {
                    session_id: sid_str,
                    verdict,
                    live: Some(live),
                    outcome: ContentProbeOutcome::Healthy,
                    detail: format!(
                        "指纹一致(bytes={}, rows={}),索引与段文件同步",
                        live.bytes, live.rows
                    ),
                },
                // 孤儿行:补入式重建修不掉,只报告(处置归 ADR-290 D5 审计器)
                ContentProbeVerdict::IndexAhead => {
                    let detail = format!(
                        "索引越前(index_rows={} > file_rows={}),只报告不重建(孤儿行归 ADR-290 D5 审计器)",
                        index_side.rows, live.rows
                    );
                    warn!(session = %sid_str, "索引内容一致性探针:{}", detail);
                    ContentProbeEntry {
                        session_id: sid_str,
                        verdict,
                        live: Some(live),
                        outcome: ContentProbeOutcome::ReportedOnly,
                        detail,
                    }
                }
                _ => {
                    let why = match verdict {
                        ContentProbeVerdict::MissingField => "内容指纹字段缺失",
                        _ => "内容指纹不符",
                    };
                    match self.rebuild_index(data_dir, &session_id) {
                        Ok(stats) => {
                            self.content_probe_rebuilds.fetch_add(1, Ordering::Relaxed);
                            // 按实况刷新指纹(重建是补入式,故以段文件实况为准)
                            if let Err(e) = self.store_content_fingerprint(&session_id, &live) {
                                warn!(session = %sid_str, error = %e, "内容指纹刷新失败,下次启动会再判不一致(不影响可用性)");
                            }
                            let detail = format!(
                                "{why} ⇒ 已重建索引(inserted={}, skipped={};bytes={}, rows={})",
                                stats.rows_inserted, stats.rows_skipped, live.bytes, live.rows
                            );
                            warn!(
                                session = %sid_str,
                                verdict = ?verdict,
                                inserted = stats.rows_inserted,
                                skipped = stats.rows_skipped,
                                "索引内容一致性探针:{}", detail
                            );
                            ContentProbeEntry {
                                session_id: sid_str,
                                verdict,
                                live: Some(live),
                                outcome: ContentProbeOutcome::Rebuilt,
                                detail,
                            }
                        }
                        Err(e) => {
                            let detail = format!("{why} 但重建失败(fail-open,保留现有索引): {e}");
                            warn!(session = %sid_str, error = %e, "索引内容一致性探针重建失败,降级为不自愈");
                            ContentProbeEntry {
                                session_id: sid_str,
                                verdict,
                                live: Some(live),
                                outcome: ContentProbeOutcome::FailedOpen,
                                detail,
                            }
                        }
                    }
                }
            };
            observer(&entry);
            report.entries.push(entry);
        }
        Ok(report)
    }

    /// 事件行总数（`session_id` 为 None 时全库统计;测试断言零拷贝用）
    pub fn event_count(&self, session_id: Option<&SessionId>) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("计数锁中毒: {p}"),
        })?;
        let (sql, sid): (&str, Option<&str>) = match session_id {
            Some(s) => (
                "SELECT COUNT(*) FROM events WHERE session_id = ?1",
                Some(s.as_str()),
            ),
            None => ("SELECT COUNT(*) FROM events", None),
        };
        let count = match sid {
            Some(s) => conn
                .query_row(sql, params![s], |r| r.get::<_, i64>(0))
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("统计事件数失败: {e}"),
                })?,
            None => conn
                .query_row(sql, [], |r| r.get::<_, i64>(0))
                .map_err(|e| StoreError::Sqlite {
                    reason: format!("统计事件数失败: {e}"),
                })?,
        };
        Ok(count as u64)
    }

    /// 段元数据行数（测试断言 fork 复制行数量）
    pub fn segment_count(&self, session_id: &SessionId) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("段计数锁中毒: {p}"),
        })?;
        let count = conn
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE session_id = ?1",
                params![session_id.as_str()],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("统计段数失败: {e}"),
            })?;
        Ok(count as u64)
    }

    /// SQLite 写事务计数（bench 门禁:syscall_reduction_pct 的分母/分子）
    #[must_use]
    pub fn transaction_count(&self) -> u64 {
        self.transaction_count.load(Ordering::Relaxed)
    }

    /// 按 (session_id, segment_index) 查既有段 ID（续写幂等复用残留行用）
    ///
    /// # WHY（P2-T3 续写幂等承接）
    /// writer 重开后续写同一物理段时,若重新 `generate()` 段 ID 会产生
    /// **重复段行**（旧行 end_offset 定格、新行重复覆盖）→ segment_sources
    /// 按段索引返回多行 → 回放重复打开同一文件、事件重复输出。复用既有
    /// ID 使 insert_segment 走 upsert（end_offset 推进）路径,索引行唯一。
    /// None = 会话尚无该段的行（新段 / 索引缺行待 rebuild）。
    pub fn segment_id_for(
        &self,
        session_id: &SessionId,
        segment_index: u32,
    ) -> Result<Option<SegmentId>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("段 ID 查询锁中毒: {p}"),
        })?;
        query_segment_id(&conn, session_id, segment_index)
    }

    /// fork 会话的续写起点 — 前缀段最大 end_offset + 1（重开续写恢复用）
    ///
    /// # WHY（P2-T3 续写幂等承接）
    /// fork 会话零拷贝无自己的段文件,重开续写无法从文件推断起点;fork 点
    /// 信息仅存于前缀复制行的 end_offset（fork 时截断到 fork 点-1）→ 恢复
    /// 点 = max(end_offset) + 1。None = 会话无前缀复制行（非 fork 会话/新
    /// 会话,续写点应为 0）。
    pub fn fork_point(&self, session_id: &SessionId) -> Result<Option<u64>, StoreError> {
        let conn = self.conn.lock().map_err(|p| StoreError::LockPoisoned {
            reason: format!("fork 点查询锁中毒: {p}"),
        })?;
        let prefix_end: Option<i64> = conn
            .query_row(
                "SELECT MAX(end_offset) FROM segments
                 WHERE session_id = ?1 AND parent_segment_id IS NOT NULL",
                params![session_id.as_str()],
                |r| r.get(0),
            )
            .map_err(|e| StoreError::Sqlite {
                reason: format!("查询会话 {} 前缀段 end 上限失败: {e}", session_id),
            })?;
        Ok(prefix_end.map(|e| e as u64 + 1))
    }
}

// ============================================================
// 模块级私有辅助（行映射 / 物理解析 / 段 ID 查询）
// ============================================================

/// segments 行 → SegmentNode 映射（query_row 回调）
fn row_to_node(r: &rusqlite::Row<'_>) -> rusqlite::Result<SegmentNode> {
    Ok(SegmentNode {
        segment_id: SegmentId::new(r.get::<_, String>(0)?),
        session_id: SessionId::new(r.get::<_, String>(1)?),
        segment_index: r.get::<_, i64>(2)? as u32,
        parent_segment_id: r.get::<_, Option<String>>(3)?.map(SegmentId::new),
        start_offset: r.get::<_, i64>(4)? as u64,
        end_offset: r.get::<_, i64>(5)? as u64,
    })
}

/// 查询会话全部段节点（按段索引升序）
fn query_segment_nodes(
    conn: &rusqlite::Connection,
    session_id: &SessionId,
) -> Result<Vec<SegmentNode>, StoreError> {
    let mut stmt = conn
        .prepare(
            "SELECT segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset
             FROM segments WHERE session_id = ?1 ORDER BY segment_index ASC",
        )
        .map_err(|e| StoreError::Sqlite {
            reason: format!("预编译会话段查询失败: {e}"),
        })?;
    let rows = stmt
        .query_map(params![session_id.as_str()], row_to_node)
        .map_err(|e| StoreError::Sqlite {
            reason: format!("查询会话 {} 段失败: {e}", session_id),
        })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| StoreError::Sqlite {
            reason: format!("读取会话段行失败: {e}"),
        })
}

/// 按段 ID 查询单段节点（None → NotFound）
fn query_segment_node(
    conn: &rusqlite::Connection,
    segment_id: &SegmentId,
) -> Result<SegmentNode, StoreError> {
    let node: Option<SegmentNode> = conn
        .query_row(
            "SELECT segment_id, session_id, segment_index, parent_segment_id, start_offset, end_offset
             FROM segments WHERE segment_id = ?1",
            params![segment_id.as_str()],
            row_to_node,
        )
        .optional()
        .map_err(|e| StoreError::Sqlite {
            reason: format!("查询段 {} 失败: {e}", segment_id),
        })?;
    node.ok_or_else(|| StoreError::NotFound {
        what: format!("段 {} 不存在", segment_id),
    })
}

/// 按 (session_id, segment_index) 查既有段 ID（rebuild 复用残留行用）
fn query_segment_id(
    conn: &rusqlite::Connection,
    session_id: &SessionId,
    segment_index: u32,
) -> Result<Option<SegmentId>, StoreError> {
    // WHY 排除 parent_segment_id IS NOT NULL 的复制行:fork 复制行与子会话
    // 自己的段行共享 segment_index（复制行保留父段 index,子段也从 0 起）——
    // 若不过滤,查询可能命中复制行 ID,后续 insert_segment 的 upsert 会把
    // 复制行当真实段行推进 end_offset,污染前缀上界（fork_point 漂移,
    // 回放/read_events 前缀越界多输出）。物理段行的 parent 恒为 NULL。
    let id: Option<String> = conn
        .query_row(
            "SELECT segment_id FROM segments
             WHERE session_id = ?1 AND segment_index = ?2 AND parent_segment_id IS NULL",
            params![session_id.as_str(), segment_index],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| StoreError::Sqlite {
            reason: format!(
                "查询会话 {} 段 {} 的 ID 失败: {e}",
                session_id, segment_index
            ),
        })?;
    Ok(id.map(SegmentId::new))
}

/// 沿 parent 引用链解析段到物理文件所属会话 + 段索引
///
/// # WHY 递归
/// 链式 fork 下复制行的 session_id 是中间会话（无物理文件）,须逐层向上
/// 回溯到 parent_segment_id IS NULL 的物理段（其 session_id + segment_index
/// 决定段文件路径）。`depth` 为防环上限（fork 链理论无环,异常数据兜底）。
fn resolve_physical(
    conn: &rusqlite::Connection,
    segment_id: &SegmentId,
    depth: u32,
) -> Result<(SessionId, u32), StoreError> {
    if depth > 64 {
        return Err(StoreError::InvalidInput {
            reason: format!("fork 引用链过深或成环(段 {}): 超过 64 层上限", segment_id),
        });
    }
    let node = query_segment_node(conn, segment_id)?;
    match &node.parent_segment_id {
        None => Ok((node.session_id, node.segment_index)),
        Some(parent) => resolve_physical(conn, parent, depth + 1),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use crate::segment::{segment_path, SegmentWriter};
    use crate::types::SessionEvent;

    fn ev(i: u64) -> SessionEvent {
        SessionEvent::with_payload(format!("ev-{i}"), vec![i as u8])
    }

    /// 用 SegmentWriter 写入父会话段 + 索引,返回 (tree, dir, sid)
    fn seed_session(
        tree: &TreeIndex,
        dir: &Path,
        sid: &SessionId,
        n: u64,
        segment_size: u64,
    ) -> Result<(), StoreError> {
        let mut idx = 0u32;
        let mut start_seq = 0u64;
        while start_seq < n {
            let mut w = SegmentWriter::open_or_create(dir, sid, idx, start_seq)?;
            let take = segment_size.min(n - start_seq);
            let events: Vec<SessionEvent> = (start_seq..start_seq + take).map(ev).collect();
            let offsets = w.append_batch(&events)?;
            let seg_id = SegmentId::generate();
            tree.insert_segment(&w.meta(seg_id.clone(), None))?;
            let rows: Vec<EventRow> = offsets
                .iter()
                .zip(events)
                .map(|(off, event)| EventRow {
                    offset: off.seq,
                    session_id: sid.clone(),
                    segment_id: seg_id.clone(),
                    event,
                })
                .collect();
            tree.insert_events(&rows)?;
            start_seq += take;
            idx += 1;
        }
        Ok(())
    }

    #[test]
    fn insert_and_read_events_roundtrip() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("roundtrip");
        seed_session(&tree, dir.path(), &sid, 8, 8).expect("seed");
        assert_eq!(tree.event_count(Some(&sid)).expect("count"), 8);
        assert_eq!(tree.segment_count(&sid).expect("segments"), 1);

        let stored = tree.read_events(&sid, None).expect("read");
        assert_eq!(stored.len(), 8);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
            assert_eq!(s.event.event_type, format!("ev-{i}"));
        }
        // from 过滤(断点续传)
        let tail = tree.read_events(&sid, Some(5)).expect("read from 5");
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].offset, 5);
    }

    #[test]
    fn multiple_segments_read_in_order() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("multi-seg");
        // 3 个段,每段 4 条
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        assert_eq!(tree.segment_count(&sid).expect("segments"), 3);
        let stored = tree.read_events(&sid, None).expect("read");
        assert_eq!(stored.len(), 12);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
        }
    }

    #[test]
    fn fork_copies_metadata_zero_data_copy() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-parent");
        // 父会话 12 条,3 段 × 4
        seed_session(&tree, dir.path(), &parent, 12, 4).expect("seed");
        let files_before = std::fs::read_dir(dir.path()).expect("list dir").count();

        // fork 点 7:前缀 = [0,7) = 段0(0-3) + 段1(4-7 截断为 4-6)
        let child = tree.fork(&parent, 7).expect("fork");
        assert_ne!(child, parent);

        // 元数据复制:新会话有 2 个前缀复制行(parent_segment_id 非 NULL)
        assert_eq!(tree.segment_count(&child).expect("child segs"), 2);
        // 零数据拷贝:全库 events 行数不变
        assert_eq!(tree.event_count(None).expect("all events"), 12);
        // 零数据拷贝:无新 JSONL 段文件
        let files_after = std::fs::read_dir(dir.path()).expect("list dir").count();
        assert_eq!(files_before, files_after, "fork 不得创建段文件");
    }

    #[test]
    fn fork_child_reads_parent_prefix() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-read");
        seed_session(&tree, dir.path(), &parent, 10, 5).expect("seed");

        let child = tree.fork(&parent, 7).expect("fork");
        // 回查:新会话可见前缀 [0,7)
        let stored = tree.read_events(&child, None).expect("read");
        assert_eq!(stored.len(), 7, "fork 后回查父段事件可用");
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
            assert_eq!(s.event.event_type, format!("ev-{i}"));
        }
        // 前缀 + 新事件按 seq 无缝拼接:父会话继续写 7-9 不受影响
        let parent_tail = tree.read_events(&parent, Some(7)).expect("parent tail");
        assert_eq!(parent_tail.len(), 3);
    }

    #[test]
    fn fork_exact_segment_boundary() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-boundary");
        seed_session(&tree, dir.path(), &parent, 8, 4).expect("seed");
        // fork 点 4 = 段1 起点:前缀 = 段0 完整复制(无截断)
        let child = tree.fork(&parent, 4).expect("fork");
        assert_eq!(tree.segment_count(&child).expect("child segs"), 1);
        let stored = tree.read_events(&child, None).expect("read");
        assert_eq!(stored.len(), 4);
        assert_eq!(stored[0].offset, 0);
        assert_eq!(stored[3].offset, 3);
    }

    #[test]
    fn fork_beyond_history_is_rejected() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-oob");
        seed_session(&tree, dir.path(), &parent, 8, 4).expect("seed");
        let err = tree.fork(&parent, 100).expect_err("超出历史必须报错");
        assert!(matches!(err, StoreError::ForkViolation { .. }));
    }

    #[test]
    fn fork_at_zero_has_no_prefix() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = SessionId::new("fork-zero");
        seed_session(&tree, dir.path(), &parent, 8, 4).expect("seed");
        let child = tree.fork(&parent, 0).expect("fork at 0");
        assert_eq!(tree.segment_count(&child).expect("child segs"), 0);
        assert!(tree.read_events(&child, None).expect("read").is_empty());
    }

    #[test]
    fn transaction_count_tracks_writes() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("txn-count");
        let mut w = SegmentWriter::open_or_create(dir.path(), &sid, 0, 0).expect("open");
        let events: Vec<SessionEvent> = (0..8).map(ev).collect();
        let offsets = w.append_batch(&events).expect("append");
        let seg_id = SegmentId::generate();
        tree.insert_segment(&w.meta(seg_id.clone(), None))
            .expect("seg");
        let rows: Vec<EventRow> = offsets
            .iter()
            .zip(events)
            .map(|(off, event)| EventRow {
                offset: off.seq,
                session_id: sid.clone(),
                segment_id: seg_id.clone(),
                event,
            })
            .collect();
        tree.insert_events(&rows).expect("insert");
        assert_eq!(tree.transaction_count(), 1, "一次批量 = 一次事务");
        tree.insert_events(&[]).expect("empty no-op");
        assert_eq!(tree.transaction_count(), 1, "空批不计数");
    }

    #[test]
    fn upsert_segment_keeps_start_updates_end() {
        let tree = TreeIndex::open_in_memory().expect("open");
        let sid = SessionId::new("upsert");
        let m1 = SegmentMeta {
            segment_id: SegmentId::new("seg-1"),
            session_id: sid.clone(),
            segment_index: 0,
            parent_segment_id: None,
            start_offset: 0,
            end_offset: 4,
        };
        tree.insert_segment(&m1).expect("insert");
        let m2 = SegmentMeta {
            segment_id: SegmentId::new("seg-1"),
            session_id: sid.clone(),
            segment_index: 0,
            parent_segment_id: None,
            start_offset: 0,
            end_offset: 9,
        };
        tree.insert_segment(&m2).expect("upsert");
        let conn = tree.conn.lock().expect("lock");
        let (start, end): (i64, i64) = conn
            .query_row(
                "SELECT start_offset, end_offset FROM segments WHERE segment_id = 'seg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("query");
        assert_eq!(start, 0, "start 保留首值");
        assert_eq!(end, 9, "end 更新");
    }

    #[test]
    fn segment_path_matches_naming_convention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("path-naming");
        let p = segment_path(dir.path(), &sid, 3);
        assert_eq!(p.file_name().expect("name"), "path-naming.3.jsonl");
    }

    // ============================================================
    // 会话树（WI-18）:tree / fork_tree / ancestors
    // ============================================================

    /// 链式 fork 辅助:seed 父会话 → fork → 再 fork（返回 [parent, child, grandchild]）
    fn seed_fork_chain(
        tree: &TreeIndex,
        dir: &Path,
    ) -> Result<(SessionId, SessionId, SessionId), StoreError> {
        let a = SessionId::new("tree-fork-a");
        seed_session(tree, dir, &a, 10, 5).expect("seed a");
        let b = tree.fork(&a, 7).expect("fork a->b");
        let c = tree.fork(&b, 7).expect("fork b->c");
        Ok((a, b, c))
    }

    #[test]
    fn tree_lists_session_segments() {
        // tree():3 段全列出,根 = 全部（无 fork 复制行）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("tree-view");
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        let t = tree.tree(&sid).expect("tree");
        assert_eq!(t.node_count(), 3);
        assert_eq!(t.roots().len(), 3, "非 fork 会话全部为根");
        assert!(t.roots().iter().all(|n| n.parent_segment_id.is_none()));
        // 段索引升序
        for (i, n) in t.nodes.iter().enumerate() {
            assert_eq!(n.segment_index, i as u32);
        }
    }

    #[test]
    fn fork_tree_shows_complete_view() {
        // fork 子会话:复制行 + 祖先节点（完整树视图）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b, _c) = seed_fork_chain(&tree, dir.path()).expect("chain");
        // B 的 tree():只含 B 的复制行（parent 指针指向 A 的段）
        let t_b = tree.tree(&b).expect("tree b");
        assert_eq!(
            t_b.node_count(),
            2,
            "B 有 2 个复制段(10 条/5 每段,fork 点 7 截断段 1)"
        );
        assert_eq!(t_b.roots().len(), 0, "B 全部为 fork 复制行,无根");
        assert!(t_b.nodes.iter().all(|n| n.parent_segment_id.is_some()));
        // fork_tree(B):B 节点 + A 祖先节点
        let ft_b = tree.fork_tree(&b).expect("fork_tree b");
        assert_eq!(
            ft_b.node_count(),
            4,
            "B 的 2 节点 + A 的 2 段(段1被截断仍属 A 物理段)"
        );
        // 祖先段（session_id == a）存在
        let a_nodes = ft_b
            .nodes
            .iter()
            .filter(|n| n.session_id == a)
            .collect::<Vec<_>>();
        assert_eq!(a_nodes.len(), 2, "完整视图含 A 的 2 个物理段节点");
    }

    #[test]
    fn ancestors_chain_across_forks() {
        // 链式 fork A→B→C:ancestors(C) = [B 的段, A 的段]（直接父层在前）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let (_a, b, c) = seed_fork_chain(&tree, dir.path()).expect("chain");
        let t_b = tree.tree(&b).expect("tree b");
        let b_segs: std::collections::HashSet<SegmentId> =
            t_b.nodes.iter().map(|n| n.segment_id.clone()).collect();
        let anc = tree.ancestors(&c).expect("ancestors");
        // 祖先链 = B 的 2 段 + A 的 2 段
        assert_eq!(anc.len(), 4, "B 层 2 段 + A 层 2 段");
        // 直接父层在前:B 的段出现在前两位
        assert!(
            b_segs.contains(&anc[0]) && b_segs.contains(&anc[1]),
            "直接父层在前"
        );
        // 全链无重复
        let dedup: std::collections::HashSet<SegmentId> = anc.iter().cloned().collect();
        assert_eq!(dedup.len(), anc.len(), "祖先链无重复");
    }

    #[test]
    fn segment_sources_resolves_physical_file_owner() {
        // 段来源解析:fork 复制段 → 父物理文件（session_id = A,文件可打开）
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b, _c) = seed_fork_chain(&tree, dir.path()).expect("chain");
        let sources = tree.segment_sources(&b).expect("sources");
        assert_eq!(sources.len(), 2, "B 的 2 个复制段");
        for src in &sources {
            assert_eq!(src.file_session, a, "物理文件归属父会话 A");
            // 截断段:B 视角 end_offset = fork 点-1 = 6
            assert!(src.end_offset <= 6, "fork 截断段 end 不超过 fork 点-1");
            // 物理文件必须存在
            let path = segment_path(dir.path(), &src.file_session, src.segment_index);
            assert!(path.exists(), "物理段文件存在: {}", path.display());
        }
        // 段 0 完整复制(0-4),段 1 截断(5-6)
        assert_eq!(sources[0].end_offset, 4);
        assert_eq!(sources[1].end_offset, 6);
    }

    // ============================================================
    // 索引重建（崩溃自愈 + 幂等）:rebuild_index
    // ============================================================

    #[test]
    fn rebuild_index_restores_after_events_deleted() {
        // 模拟崩溃:段文件完整、索引缺 events 行 → rebuild → read_events 完整
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("rebuild-crash");
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        // 崩溃模拟:删除 events 全部行（段文件保留）
        {
            let conn = tree.conn.lock().expect("lock");
            conn.execute("DELETE FROM events", [])
                .expect("delete events");
        }
        assert_eq!(tree.event_count(Some(&sid)).expect("count"), 0);

        let stats = tree.rebuild_index(dir.path(), &sid).expect("rebuild");
        assert_eq!(stats.segments_scanned, 3, "3 个物理段文件");
        assert_eq!(stats.rows_inserted, 12, "12 行重建");
        assert_eq!(stats.rows_skipped, 0);
        // rebuild 后 read_events 完整且顺序正确
        let stored = tree.read_events(&sid, None).expect("read");
        assert_eq!(stored.len(), 12);
        for (i, s) in stored.iter().enumerate() {
            assert_eq!(s.offset, i as u64);
            assert_eq!(s.event.event_type, format!("ev-{i}"));
        }
    }

    #[test]
    fn rebuild_index_is_idempotent() {
        // 幂等:第二次 rebuild 全跳过,无副作用
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("rebuild-idem");
        seed_session(&tree, dir.path(), &sid, 12, 4).expect("seed");
        {
            let conn = tree.conn.lock().expect("lock");
            conn.execute("DELETE FROM events", []).expect("delete");
        }
        let first = tree.rebuild_index(dir.path(), &sid).expect("first");
        assert_eq!(first.rows_inserted, 12);
        let second = tree.rebuild_index(dir.path(), &sid).expect("second");
        assert_eq!(second.rows_inserted, 0, "第二次全去重跳过");
        assert_eq!(second.rows_skipped, 12, "幂等:已存在 offset 全跳过");
        assert_eq!(tree.event_count(Some(&sid)).expect("count"), 12, "行数不变");
    }

    #[test]
    fn rebuild_index_partial_missing_rows_merge() {
        // 部分缺行（索引残留一半）:rebuild 补齐缺失,已存在跳过
        let tree = TreeIndex::open_in_memory().expect("open");
        let dir = tempfile::tempdir().expect("tempdir");
        let sid = SessionId::new("rebuild-partial");
        seed_session(&tree, dir.path(), &sid, 8, 4).expect("seed");
        // 删掉 offset >= 4 的行（后半段索引缺失）
        {
            let conn = tree.conn.lock().expect("lock");
            conn.execute("DELETE FROM events WHERE offset >= 4", [])
                .expect("delete");
        }
        let stats = tree.rebuild_index(dir.path(), &sid).expect("rebuild");
        assert_eq!(stats.rows_inserted, 4, "补齐缺失的 4 行");
        assert_eq!(stats.rows_skipped, 4, "已存在 4 行跳过");
        assert_eq!(tree.event_count(Some(&sid)).expect("count"), 8);
    }
}
