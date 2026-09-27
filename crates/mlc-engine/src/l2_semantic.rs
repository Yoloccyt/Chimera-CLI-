//! L2 语义记忆 — 分层检索(精确线性扫描 + HNSW 近似层)
//!
//! 对应架构层:L2 Memory(L2 Semantic tier)
//! 对应 ADR:ADR-192(L2 检索分层)、ADR-005(sqlite-vec 违反 `forbid(unsafe_code)`
//! → 降级内存 KNN 的历史缘由)
//!
//! # 设计决策(WHY)
//! - **分层检索而非单一路线**(ADR-192):条目数 `< HNSW_MIN_ENTRIES` 走线性扫描
//!   精确层(代码路径逐位不变),`≥ HNSW_MIN_ENTRIES` 走 `hnsw_rs` 近似层取候选 +
//!   **候选逐条重算精确余弦**。近似性只体现在候选选取,不体现在分数与排序语义。
//!   背景:100-4096 规模线性扫描本就达标,但 2026-09-25 静默态实测 10k 条目
//!   Top-10 召回 median 4.117ms(已越 4ms redline 入 5ms SLO 警示带)、100k
//!   42.136ms(O(n) 线性 10× 特征),故按规模分层。`sqlite-vec` 仍不在演进路径。
//! - **小输入不回退纪律**:界以下不付建图/近似成本,与 osa-coordinator
//!   `PARALLEL_MIN_ACTIVE_ITEMS` 尺寸自适应同哲学(小输入走串行/精确更快)。
//! - **近似层是主表的派生侧表**:主存储 `entries`/向量队列 `vectors`/CLV 池仍为权威,
//!   图可全量重建;三者在同一把 `RwLock` 内维护,故不变量
//!   「图侧存活点 == entries.keys()」无跨锁竞态(可测)。
//! - **tombstone + 周期重建**:hnsw_rs 不支持删点,`insert`(同 ID 更新)/`remove`/
//!   FIFO 驱逐三条路径一律对旧 dataid 打墓碑(dataid 单调递增永不回收),搜索时 over-fetch
//!   再过滤;死点占比达 25% 则标 stale,下一次召回在 `try_write` 内全量重建。
//!   **重建窗口回退精确扫描** —— 近似层任何异常面收敛到「变慢」而非「变错」。
//! - **相似度 clamp 到 [0.0, 1.0]**:余弦相似度理论范围 [-1.0, 1.0],
//!   负值表示"语义相反",对记忆召回无意义,clamp 到 0.0 表示"无相似性"
//! - **VecDeque<(SharedCLV, MemoryId)> 而非 HashMap**:向量需顺序扫描,Vec 缓存友好;
//!   HashMap 适合精确查找,不适合范围扫描
//! - **RwLock 包装整体**:entries 与 vectors(及近似层侧表)需保持一致性
//! - **SharedCLV + clv_pool 共享**:SubTask 13.1 优化,相同内容的 CLV 通过 `Arc<[f32]>`
//!   共享内存,4096 条目若 CLV 重复则内存从 8MB 降至 k×2KB(k 为不同 CLV 数)
//!
//! # 逃生开关
//! `CHIMERA_NO_HNSW_MLC`(存在即生效,启动期一次读取)全盘关闭近似层 = 回到
//! ADR-005 降级后的纯线性行为;或 `SemanticMemory::with_hnsw_threshold(cap, 0)`。
//!
//! # 使用示例
//! ```
//! # use mlc_engine::{MemoryEntry, MemoryTier, SemanticMemory};
//! # use nexus_core::CLV;
//! # let clv = CLV::from_vec(vec![1.0; CLV::DIMENSION]).unwrap();
//! let mem = SemanticMemory::new(8192);
//! mem.insert(MemoryEntry::new("m-1", "内容", MemoryTier::L2Semantic).with_clv(clv.clone())).unwrap();
//! // 签名与语义不随分层而变:始终返回(相似度降序、长度 ≤ top_k)的 (MemoryId, score)
//! let hits = mem.recall_by_clv(&clv, 10).unwrap();
//! assert_eq!(hits[0].0.as_str(), "m-1");
//! ```
//!
//! # 性能基准（release 实测，`benches/hnsw_tiered.rs` 同批对照，2026-09-26）
//! - 100 条目 Top-10 召回 < 5ms（精确层：线性遍历 100 × 512-dim 向量）
//! - 4096 条目 Top-10 召回 < 200ms（精确层红线，`mlc_l2_knn` 基准守护，ADR-192 不放宽）；实测 1.536ms 不回退
//! - 10k 条目：精确层 4.386ms → 分层 **0.446ms（9.8×）**；CI 门 `hnsw_tiered` 阈值 2ms
//! - 100k 条目：精确层 47.174ms → 分层 **2.110ms（22.4×）**；dev-only 取数（需 `CHIMERA_HNSW_BENCH_FULL=1`）
//! - 召回质量（分数制）：质量比 10k = 1.000 / 100k = 0.9999；分数容差 recall@10 均 ≥ 0.95

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};

use hnsw_rs::hnsw::Hnsw;
use hnsw_rs::prelude::DistCosine;
use nexus_contracts::util::xts_top_k_by;
use nexus_core::CLV;
use tracing::{debug, trace};

use crate::error::MlcError;
use crate::types::{MemoryEntry, MemoryId, MemoryTier, SharedCLV};

/// 近似层切换阈值:条目数 ≥ 该值时 `recall_by_clv` 走 HNSW 近似层
///
/// WHY 4096:精确层既有红线守护的规模(4096 条目 Top-10 < 200ms)。界以下线性
/// 扫描本就达标且逐位精确,界以上才付近似成本(ADR-192 D2)。
/// 注:`MlcConfig::l2_capacity` 默认即 4096,故默认配置下近似层在 L2 满容量饱和时
/// 接管;调用方显式放大容量(10k/100k SLO 场景)即近似层的目标场景。
pub const HNSW_MIN_ENTRIES: usize = 4096;

/// 重建触发:死点占比达 `1 / HNSW_REBUILD_TOMBSTONE_RATIO`(= 25% 存活集)
///
/// WHY 只用比例规则不用绝对墓碑上限:建图成本随规模超线性(release 实测
/// 10k 档秒级、100k 档十秒级),若兼带「墓碑 ≥ 4096 即重建」的绝对上限,
/// 100k 图会每 4%(=4096 次变更)重建一次 —— 重建频率与规模不匹配。
/// 取 25% 是两侧约束的交点:再高则 over-fetch(封顶 2×top_k)不足以凑齐
/// top_k 存活候选而频繁走精确补口,再低则重建过于频繁。
const HNSW_REBUILD_TOMBSTONE_RATIO: usize = 4;

/// HNSW 每层最大连接数(M 参数)
///
/// WHY 32 而非 `memory_graph`/repo-wiki 先例的 16:4096 条 64 簇语料实测
/// (2026-09-26,本仓临时测量夹具取数)建图参数中 **M 是召回质量的决定性旋钮**,
/// 而 `ef_search` 几乎不影响召回(图不可达而非搜索宽度不够):
/// M=16 的 id-recall 在 0.96-0.99 浮动, M=32 提高到 0.97-1.00。
/// 代价:建图 +15%、搜索宽度 +30%(均为同批扫参实测),仍在 ADR-192 §5 延迟目标内。
/// 论文推荐区间 M ∈ [16, 48](Malkov & Yashunin 2016)。
const HNSW_MAX_NB_CONNECTION: usize = 32;

/// HNSW 最大层级(与生产两条先例路径一致)
const HNSW_MAX_LAYER: usize = 16;

/// HNSW 建图时 ef 参数
///
/// WHY 100(低于 `memory_graph`/repo-wiki 先例的 200):建图成本落在
/// **读路径的首次/重建召回**上(release 实测 10k 档 3.56s、100k 档 32.6s,
/// 见 `benches/hnsw_tiered.rs` 输出与 ADR-192 §4 代价 2),而召回质量主要由
/// M 决定(实测 efc 200→400 对 recall 无统计意义提升)→ 取区间下限换建图成本。
const HNSW_EF_CONSTRUCTION: usize = 100;

/// HNSW 搜索宽度基准(实际 ef = `max(adaptive_ef_search(规模), fetch_k)`)
///
/// WHY 50 作小图下限:实测(4096 条 64 簇语料,临时测量夹具)ef 从 50→200
/// **召回几乎不变**(得到 0.97-1.00)而单 query 搜索时间近似线性上升
/// (debug 7.6ms→32ms)→ ef 不是质量旋钮(质量由 M 与建图质量定),
/// 因此取区间下限保延迟(10k 档 ef=100 实测 median 434µs,同批精确层 4.32ms)。
const HNSW_EF_SEARCH: usize = 50;

/// 按图规模自适应搜索宽度
///
/// WHY 阶梯上调(继承 repo-wiki `adaptive_ef_search` 的 v2.9.0-omega 同型决策
/// 与档位边界 10k/100k):图越大、贪心路径越容易错过真 Top-K 成员。
/// 具体阶梯由 `benches/hnsw_tiered.rs` 的 10k/100k recall 采样定档
/// (bench 内置硬断言:质量比 ≥ 0.99 与分数容差 recall ≥ 0.95)。
fn adaptive_ef_search(live_points: usize) -> usize {
    if live_points < 10_000 {
        HNSW_EF_SEARCH
    } else if live_points < 100_000 {
        100
    } else {
        200
    }
}

/// 墓碑过补偿因子 —— 搜索时至少多取 `top_k × 该值` 个候选
///
/// WHY:hnsw_rs 不删点,墓碑仍会被搜索返回,需 over-fetch 再过滤
/// (与 repo-wiki `HnswStore::TOMBSTONE_OVERFETCH_FACTOR` 同口径)。
const TOMBSTONE_OVERFETCH_FACTOR: usize = 2;

/// 近似层逃生开关(`CHIMERA_NO_HNSW_MLC` 存在即关闭)
///
/// WHY 启动期一次读取(`OnceLock`)不入热路径,与
/// `gsoe_evolution::fitness_parallel::parallel_enabled` 同构(红线:禁运行时
/// feature 标志热路径分支);设置该变量即回到 ADR-005 降级后的纯线性行为。
fn hnsw_tier_enabled_by_env() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("CHIMERA_NO_HNSW_MLC").is_err())
}

/// 相似度降序比较器(Top-K 选择与最终输出共用,避免两路语义漂移)
fn sim_desc(a: &(MemoryId, f32), b: &(MemoryId, f32)) -> Ordering {
    b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal)
}

/// HNSW 近似层侧表
///
/// WHY 作为 `SemanticInner` 的侧表而非独立 `RwLock`:图是主表(`vectors`)的派生物,
/// 与主表同锁维护可静态保证「图侧存活点 == entries.keys()」不变量,无需跨锁协调。
struct HnswTier {
    /// 图索引(`'static`:hnsw_rs `insert_slice` 内部 `to_vec` 自持副本,不外借)
    index: Hnsw<'static, f32, DistCosine>,
    /// dataid → (MemoryId, 向量),存活点集(候选精确重算与结果回填的数据源)
    by_dataid: HashMap<usize, (MemoryId, SharedCLV)>,
    /// MemoryId → dataid,反向表(更新/删除/驱逐时定位墓碑)
    dataid_by_id: HashMap<MemoryId, usize>,
    /// 已打墓碑的图点数(图中仍在、侧表已除;重建时归零)
    ///
    /// WHY 只计数不存集合:墓碑 dataid 已从 `by_dataid` 移除即不可命中,
    /// 保留集合只会多一份与主表并行的陈旧名单。
    tombstones: usize,
    /// 下一个可用 dataid(单调递增,永不回收 —— 防墓碑 dataid 被复用误命中)
    next_dataid: usize,
    /// 死点占比达阈值时置位:下一次召回重建;期间回退精确扫描
    stale: bool,
}

impl HnswTier {
    /// 从主表向量队列全量建图(唯一的「建/重建」入口,保证两路径参数一致)
    ///
    /// WHY `parallel_insert_slice`(而非 `parallel_insert`):该变体直接吃 `&[f32]`,
    /// 免于 `Vec<Vec<f32>>` 物化(100k×512×4B ≈ 205MB 峰值复制),同时保留
    /// hnsw_rs 内置 rayon 多核建图(建图是本层主要一次性成本)。
    fn build(vectors: &VecDeque<(SharedCLV, MemoryId)>) -> Self {
        let n = vectors.len();
        let index: Hnsw<'static, f32, DistCosine> = Hnsw::new(
            HNSW_MAX_NB_CONNECTION,
            // max_elements 仅为预分配提示(hnsw_rs 内部 with_capacity),给足当前规模
            n.max(HNSW_MIN_ENTRIES),
            HNSW_MAX_LAYER,
            HNSW_EF_CONSTRUCTION,
            DistCosine,
        );
        let mut tier = Self {
            index,
            by_dataid: HashMap::with_capacity(n),
            dataid_by_id: HashMap::with_capacity(n),
            tombstones: 0,
            next_dataid: n,
            stale: false,
        };
        let insert_data: Vec<(&[f32], usize)> = vectors
            .iter()
            .enumerate()
            .map(|(dataid, (clv, _))| (clv.as_slice(), dataid))
            .collect();
        tier.index.parallel_insert_slice(&insert_data);
        for (dataid, (clv, id)) in vectors.iter().enumerate() {
            tier.by_dataid.insert(dataid, (id.clone(), clv.clone()));
            tier.dataid_by_id.insert(id.clone(), dataid);
        }
        tier
    }

    /// 图侧存活点数(与主表条目数比对即为一致性判据)
    fn live_points(&self) -> usize {
        self.by_dataid.len()
    }

    /// 刷新 stale 标记(死点占比达阈值)
    ///
    /// WHY 每次变更后重算而非只在 `forget` 里置位:增量入图会抬高 `live_points`,
    /// 使原本成立的比例判据可能自然解除;以当前实况为准既不产生不必要重建,
    /// 也不会把已达阈值的情形当作未发生。
    fn refresh_stale(&mut self) {
        self.stale =
            self.tombstones.saturating_mul(HNSW_REBUILD_TOMBSTONE_RATIO) >= self.live_points();
    }

    /// 增量入图(新 dataid 单调分配)
    fn remember(&mut self, id: &MemoryId, clv: &SharedCLV) {
        let dataid = self.next_dataid;
        self.next_dataid += 1;
        self.index.insert((clv.as_slice(), dataid));
        self.by_dataid.insert(dataid, (id.clone(), clv.clone()));
        self.dataid_by_id.insert(id.clone(), dataid);
        self.refresh_stale();
    }

    /// 打墓碑(从侧表移除,图中节点待重建回收)
    fn forget(&mut self, id: &MemoryId) {
        if let Some(dataid) = self.dataid_by_id.remove(id) {
            self.by_dataid.remove(&dataid);
            self.tombstones += 1;
            self.refresh_stale();
        }
    }
}

/// 分层路由决策（ADR-192 D1 的显式化：精确层 / 近似层两档 + 一个重建过渡档）
///
/// WHY 抽为枚举 + 纯函数而不是写在 `recall_by_clv` 的 if 塔里：
/// 路由本身是分层的命脉（错一次就是小输入付近似成本或大输入错过加速），
/// 必须可单独穷举断言，而不得只能间接从结果倒推。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TierRoute {
    /// 精确层：线性扫描（条目数低于阈值 / 近似层被 `CHIMERA_NO_HNSW_MLC` 或阈值 0 关闭）
    Exact,
    /// 近似层：图已在场且未 stale，直接取候选 + 精确重算
    Approx,
    /// 达阈但图缺失或已 stale：先取写锁建图/重建再走近似层；拿不到写锁则本轮回退精确层
    RebuildThenApprox,
}

/// L2 语义记忆内部状态（RwLock 保护，保持 entries / vectors / clv_pool / 近似层一致性）
///
/// WHY RwLock 而非 Mutex:`recall_by_clv`(读)频率远高于 `insert`(写),
/// RwLock 允许多个召回并发,提升高并发场景下的吞吐量
struct SemanticInner {
    /// 条目主存储(MemoryId → Arc<MemoryEntry>)
    ///
    /// WHY Arc<MemoryEntry>:`list_all_arc()` 通过 `Arc::clone` 零拷贝共享,
    /// 避免 `list_all()` 全量深拷贝(4096 条目 ~8MB 分配)
    entries: HashMap<MemoryId, Arc<MemoryEntry>>,
    /// 向量索引((SharedCLV, MemoryId) 队列,顺序扫描)
    ///
    /// WHY VecDeque 而非 Vec:FIFO 驱逐用 `pop_front` O(1),
    /// 原 Vec::remove(0) 为 O(n),高容量下驱逐开销显著
    ///
    /// WHY SharedCLV 而非 CLV:SubTask 13.1 优化,通过 `Arc<[f32]>` 共享
    /// 相同内容 CLV 的内存,避免每条目独立分配 2KB
    vectors: VecDeque<(SharedCLV, MemoryId)>,
    /// CLV 池(内容哈希 → 共享 Arc),用于插入时去重共享
    ///
    /// WHY 池化:4096 条目中若许多 CLV 内容相同(如默认向量、模板向量),
    /// 通过池复用 Arc 将内存从 O(n × 2KB) 降至 O(k × 2KB)。
    /// 驱逐时检查 `Arc::strong_count`,若仅池引用则从池移除,避免池无限增长。
    clv_pool: HashMap<u64, Arc<[f32]>>,
    /// HNSW 近似层侧表(`None` = 尚未惰性建图)
    ///
    /// WHY 放在主表同锁内:图的增量/墓碑/重建与 `vectors`/`entries` 天然同事务,
    /// 不需第二把锁,也不会出现「主表已变、图未变」的窗口。
    hnsw: Option<HnswTier>,
}

impl SemanticInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            vectors: VecDeque::new(),
            clv_pool: HashMap::new(),
            hnsw: None,
        }
    }

    /// 近似层增量入图(图不在场或已 stale 时不动作)
    ///
    /// WHY 仅侧表在场才增量:建图/重建走读路径惰性触发(见 `recall_by_clv`),
    /// 写路径不付一次性建图成本;stale 期间增量无意义(即将被全量重建覆盖)。
    /// 入图向量取 `vectors` 尾元素(与刚 push 的主表同源)。
    fn tier_remember_last(&mut self, id: &MemoryId) {
        let Some(clv) = self.vectors.back().map(|(clv, _)| clv) else {
            return;
        };
        let Some(tier) = self.hnsw.as_mut() else {
            return;
        };
        if !tier.stale {
            tier.remember(id, clv);
        }
    }

    /// 近似层打墓碑(图不在场时为空操作)
    fn tier_forget(&mut self, id: &MemoryId) {
        if let Some(tier) = self.hnsw.as_mut() {
            tier.forget(id);
        }
    }
}

/// L2 语义记忆 — 按 CLV 向量召回的语义关联记忆
///
/// 维护条目主存储与向量索引,按条目数分层实现召回:
/// `< hnsw_threshold` 走线性扫描精确层,`≥` 阈值走 HNSW 近似层(ADR-192)。
///
/// # 线程安全
/// `RwLock<SemanticInner>` 包装,读操作(`recall_by_clv`/`get`)用 `read()`,
/// 写操作(`insert`/`remove`)用 `write()`,允许多个召回并发。
/// 建图/重建发生在 `try_write()` 内(拿不到写锁的本轮召回回退精确扫描)。
pub struct SemanticMemory {
    /// 内部状态(RwLock 保护,读多写少)
    inner: RwLock<SemanticInner>,
    /// 容量上限(超出时按最旧插入顺序驱逐)
    capacity: usize,
    /// 近似层切换阈值(0 = 关闭近似层)
    ///
    /// WHY 实例字段而非只用常量:让分层边界在测试里可验证(小阈值验机制、
    /// 默认 4096 由常量断言固定),同时不把整个索引参数表暴露为公共 API。
    hnsw_threshold: usize,
    /// 运行时是否允许近似层(启动期 `CHIMERA_NO_HNSW_MLC` 一次读取)
    hnsw_enabled: bool,
    /// 累计驱逐次数
    evictions: std::sync::atomic::AtomicU64,
}

impl SemanticMemory {
    /// 创建 L2 语义记忆,指定容量上限
    ///
    /// 近似层使用默认阈值 [`HNSW_MIN_ENTRIES`] 与 `CHIMERA_NO_HNSW_MLC` 逃生开关。
    pub fn new(capacity: usize) -> Self {
        Self::with_hnsw_threshold(capacity, HNSW_MIN_ENTRIES)
    }

    /// 创建 L2 语义记忆并自定义近似层切换阈值
    ///
    /// # 参数
    /// - `capacity`:条目容量上限(超出按 FIFO 驱逐)
    /// - `hnsw_threshold`:≥ 该条目数时启用 HNSW 近似层;`0` = 关闭近似层
    ///   (等价 `CHIMERA_NO_HNSW_MLC`)
    ///
    /// WHY 公开该构造:分层边界需要一个可调旋钮供调用方按实际规模选型
    /// (如 L2 常驻 <4096 可直接关掉近似层),并让机制测试无需构造 4096+ 条目。
    /// 召回结果的类型与排序语义不受该参数影响,故不属于破坏性 API。
    pub fn with_hnsw_threshold(capacity: usize, hnsw_threshold: usize) -> Self {
        Self {
            inner: RwLock::new(SemanticInner::new()),
            capacity,
            hnsw_threshold,
            hnsw_enabled: hnsw_tier_enabled_by_env(),
            evictions: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 本轮召回是否应走近似层
    fn approx_requested(&self, len: usize) -> bool {
        self.hnsw_enabled && self.hnsw_threshold > 0 && len >= self.hnsw_threshold
    }

    /// 返回容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 返回当前条目数
    ///
    /// WHY 返回 Result:消除 `expect("L1 mutex poisoned")`(原注释误写为 L1,
    /// 实际是 L2)。生产代码禁止 expect(),mutex 毒化时返回 `StorageError`
    /// 而非 panic,符合"系统边界做校验"原则。
    pub fn len(&self) -> Result<usize, MlcError> {
        self.inner
            .read()
            .map(|inner| inner.entries.len())
            .map_err(|e| MlcError::StorageError(format!("L2 lock poisoned: {e}")))
    }

    /// 是否为空
    pub fn is_empty(&self) -> Result<bool, MlcError> {
        self.len().map(|n| n == 0)
    }

    /// 返回累计驱逐次数
    pub fn evictions(&self) -> u64 {
        self.evictions.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 插入语义记忆条目(必须携带 CLV)
    ///
    /// - 自动设置 tier 为 L2
    /// - 若条目无 CLV,返回 `InvalidConfig` 错误
    /// - 若容量满,驱逐最旧插入的条目(FIFO,按 vectors 顺序)
    /// - CLV 通过 `clv_pool` 去重共享(SubTask 13.1),相同内容 CLV 复用 `Arc<[f32]>`
    /// - 近似层侧表同步(ADR-192 D3):同 ID 更新与驱逐均打墓碑,新点增量入图;
    ///   但**仅当图已在场**(建图不发生在写路径)
    ///
    /// 返回被驱逐的条目(若有)
    pub fn insert(&self, mut entry: MemoryEntry) -> Result<Option<MemoryEntry>, MlcError> {
        let clv = entry.clv.clone().ok_or_else(|| {
            MlcError::InvalidConfig(format!("L2 语义记忆条目必须携带 CLV: {}", entry.id))
        })?;
        entry.tier = MemoryTier::L2Semantic;

        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;

        // 若更新已存在条目,先移除旧向量(旧 SharedCLV drop 后 Arc 引用计数 -1)
        if inner.entries.contains_key(&entry.id) {
            inner.vectors.retain(|(_, id)| id != &entry.id);
            // 近似层:旧 dataid 打墓碑(图不可删点,ADR-192 D3)
            inner.tier_forget(&entry.id);
        }

        // 容量满且是新条目,驱逐最旧(vectors[0])
        let evicted =
            if inner.entries.len() >= self.capacity && !inner.entries.contains_key(&entry.id) {
                let victim = self.evict_oldest_locked(&mut inner)?;
                self.evictions
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!(
                    entry_id = %entry.id,
                    victim_id = ?victim.as_ref().map(|v| v.id.as_str()),
                    "L2 容量满,驱逐最旧条目"
                );
                victim
            } else {
                None
            };

        // 通过池去重共享 CLV(SubTask 13.1):相同内容 CLV 复用同一个 Arc
        let shared_clv = SharedCLV::intern(&clv, &mut inner.clv_pool);

        // 添加新向量
        inner.vectors.push_back((shared_clv, entry.id.clone()));
        // 近似层增量入图(图不在场/stale 时不动作;向量取 vectors 尾元素与主表同源)
        inner.tier_remember_last(&entry.id);
        // 插入主存储
        inner.entries.insert(entry.id.clone(), Arc::new(entry));

        Ok(evicted)
    }

    /// 本轮召回的路由决策（纯函数，可单独穷举断言）
    ///
    /// WHY 输入取侧表引用而非 `&self` 字段：避免在 `RwLock` 读锁内做二次判断，
    /// 决策与数据同次读取，不会发生“判完又变”的中间态。
    fn route(&self, hnsw: &Option<HnswTier>, entries: usize) -> TierRoute {
        if !self.approx_requested(entries) {
            return TierRoute::Exact;
        }
        match hnsw.as_ref() {
            Some(tier) if !tier.stale => TierRoute::Approx,
            // 未建图（None）或已 stale → 建图/重建窗口
            _ => TierRoute::RebuildThenApprox,
        }
    }

    /// 按 CLV 召回 Top-K 最相似条目（分层路由入口，ADR-192 D1）
    ///
    /// 输出契约与条目数无关:按相似度降序、长度 = `min(top_k, 存活条目数)`,
    /// 分数为精确余弦相似度 clamp 到 `[0.0, 1.0]`(两路径同一实现、同一参数顺序)。
    ///
    /// 路由:
    /// - `len < hnsw_threshold`(或近似层被关)→ 精确层线性扫描,代码路径逐位不变
    /// - `len ≥ hnsw_threshold` 且图已建且未 stale → 近似层(图取候选 + 精确重算)
    /// - `len ≥ hnsw_threshold` 但图缺失/已 stale → `try_write` 内惰性建图/重建后再走近似层;
    ///   拿不到写锁则本轮回退精确扫描(降级方向=变慢而非变错)
    ///
    /// WHY 精确层保留原实现与 `Vec<(usize, f32)>` 索引方案(SubTask 19.3):
    /// 评分阶段存 vectors 索引(usize 拷贝)而非 MemoryId clone,避免 4096 次
    /// String 堆分配;该路径在阈值以下仍是最优路径,不动。
    pub fn recall_by_clv(
        &self,
        query: &CLV,
        top_k: usize,
    ) -> Result<Vec<(MemoryId, f32)>, MlcError> {
        // WHY read() 而非 write():召回是只读操作,RwLock 允许多个召回并发
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;

        if inner.vectors.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }

        match self.route(&inner.hnsw, inner.entries.len()) {
            TierRoute::Exact => Ok(recall_exact(&inner, query, top_k)),
            TierRoute::Approx => match inner.hnsw.as_ref() {
                Some(tier) => Ok(self.recall_approx(tier, &inner, query, top_k)),
                // 不可达：route 仅在侧表在场且新鲜时判 Approx。写成回退而非
                // expect/panic：该路径本层无不变量可破坏，宁可少一次加速不担风险
                None => Ok(recall_exact(&inner, query, top_k)),
            },
            // 走到此处 = 需要首次建图或重建（读锁必须先释放才能取写锁）
            TierRoute::RebuildThenApprox => {
                drop(inner);
                self.rebuild_and_recall(query, top_k)
            }
        }
    }

    /// 建图/重建窗口:取写锁全量重建后本次召回走近似层
    ///
    /// WHY `try_write` 而非 `write`:稳态下召回是热路,若多个读者长期持读锁,
    /// 在此阻塞会拖慢所有读者;拿不到写锁就本轮精确回退(结果仍精确,只是本轮无加速)。
    /// 重建在写锁内完成且无 await,符合 §4.4 不持锁跨 await 纪律。
    fn rebuild_and_recall(
        &self,
        query: &CLV,
        top_k: usize,
    ) -> Result<Vec<(MemoryId, f32)>, MlcError> {
        let poisoned = |e: std::sync::PoisonError<_>| {
            MlcError::StorageError(format!("L2 rwlock poisoned: {e}"))
        };
        match self.inner.try_write() {
            Ok(mut inner) => {
                // 二次确认:释放读锁与取写锁的窗口内条目数可能已变(驱逐/删除)
                if !self.approx_requested(inner.entries.len()) {
                    return Ok(recall_exact(&inner, query, top_k));
                }
                let need_build = inner.hnsw.as_ref().is_none_or(|tier| tier.stale);
                if need_build {
                    let built = std::time::Instant::now();
                    let tier = HnswTier::build(&inner.vectors);
                    debug!(
                        points = tier.live_points(),
                        tombstones = tier.tombstones,
                        elapsed_us = built.elapsed().as_micros() as u64,
                        "L2 近似层建图/重建完成(ADR-192 D3)"
                    );
                    inner.hnsw = Some(tier);
                }
                let tier = inner
                    .hnsw
                    .as_ref()
                    .ok_or_else(|| MlcError::StorageError("L2 近似层建图后丢失侧表".to_string()))?;
                Ok(self.recall_approx(tier, &inner, query, top_k))
            }
            Err(_) => {
                // 重建窗口回退精确扫描:结果仍精确,仅失去本轮加速
                let inner = self.inner.read().map_err(poisoned)?;
                trace!("L2 近似层待重建,本轮召回回退精确扫描");
                Ok(recall_exact(&inner, query, top_k))
            }
        }
    }

    /// 近似层召回:HNSW 取候选 → 逐条**重算精确余弦** → `xts_top_k_by` 截断
    ///
    /// WHY 重算而非直接用 `1 - distance`(ADR-192 D1):
    /// ① 分数契约与精确层同源(`cosine_similarity_slices` 同一实现、同一参数顺序),
    ///    返回值逐位可比;② 消化 `DistCosine` 的 f32 漂移与 clamp 差异
    ///    (相似度 ≤ 0 的点在精确层是并列 0.0,不应被图距离抬进结果)。
    ///
    /// 近似性只体现在**候选集选取**,故补口需保证长度不变量:存活候选不足
    /// `min(top_k, 条目数)` 时(刚跨阈值或重度墓碑),对剩余向量精确扫描补足。
    fn recall_approx(
        &self,
        tier: &HnswTier,
        inner: &SemanticInner,
        query: &CLV,
        top_k: usize,
    ) -> Vec<(MemoryId, f32)> {
        // 图-主表一致性(不变量):不一致则本轮退回精确扫描,不返回残缺结果集
        if tier.live_points() != inner.entries.len() {
            debug!(
                graph_points = tier.live_points(),
                entries = inner.entries.len(),
                "L2 近似层与主表不一致,本轮回退精确扫描"
            );
            return recall_exact(inner, query, top_k);
        }

        // over-fetch:多取墓碑量,但至少 top_k × TOMBSTONE_OVERFETCH_FACTOR;
        // 墓碑很多时 over-fetch 被 2×top_k 钳住,不足部分交给下方精确补口
        // (否则重度墓碑会退化成「图上做一次全量扫描」,反而更慢)
        let overfetch = tier
            .tombstones
            .min(top_k.saturating_mul(TOMBSTONE_OVERFETCH_FACTOR));
        let fetch_k = top_k
            .saturating_add(overfetch)
            .min(tier.live_points() + tier.tombstones);
        // ef 必须 ≥ k:取「规模自适应基准」与 fetch_k 的较大者
        let ef = adaptive_ef_search(tier.live_points()).max(fetch_k);
        let neighbours = tier.index.search(query.as_slice(), fetch_k, ef);

        // 候选→(MemoryId, 精确分数);墓碑/陈旧 dataid 不在 by_dataid 中,自然被过滤
        let mut scored: Vec<(MemoryId, f32)> = Vec::with_capacity(neighbours.len());
        for nb in &neighbours {
            if let Some((id, clv)) = tier.by_dataid.get(&nb.d_id) {
                if scored.iter().any(|(picked, _)| picked == id) {
                    continue;
                }
                let sim = clv.cosine_similarity_clv(query).clamp(0.0, 1.0);
                scored.push((id.clone(), sim));
            }
        }

        let want = top_k.min(inner.entries.len());
        if scored.len() < want {
            // 补口:精确扫描未入选向量,取其中最优的 want - scored.len() 个
            let mut rest: Vec<(MemoryId, f32)> = inner
                .vectors
                .iter()
                .filter(|(_, id)| !scored.iter().any(|(picked, _)| picked == id))
                .map(|(clv, id)| {
                    let sim = clv.cosine_similarity_clv(query).clamp(0.0, 1.0);
                    (id.clone(), sim)
                })
                .collect();
            let picked = xts_top_k_by(&mut rest, want - scored.len(), sim_desc).to_vec();
            scored.extend(picked);
        }

        // 候选集截断:xts 范式(L0 select_nth O(n) + 前 k 段二次排序),结果已按降序
        let k = top_k.min(scored.len());
        let result = xts_top_k_by(&mut scored, k, sim_desc).to_vec();
        trace!(
            top_k,
            graph_points = tier.live_points(),
            tombstones = tier.tombstones,
            candidates = neighbours.len(),
            returned = result.len(),
            "L2 分层召回完成(近似层)"
        );
        result
    }

    /// 按 ID 获取条目克隆
    pub fn get(&self, id: &str) -> Result<MemoryEntry, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        // **v:&Arc<MemoryEntry> → Arc<MemoryEntry> → MemoryEntry,clone 返回 owned
        inner
            .entries
            .get(id)
            .map(|v| (**v).clone())
            .ok_or_else(|| MlcError::EntryNotFound(format!("L2 语义记忆条目: {id}")))
    }

    /// 驱逐最旧插入的条目(按 vectors 顺序,FIFO)
    ///
    /// 驱逐后清理池中无引用的 Arc(引用计数 ≤ 1 表示仅池引用),
    /// 避免池无限增长(SubTask 13.1)。同时对近似层旧 dataid 打墓碑(ADR-192 D3)。
    fn evict_oldest_locked(
        &self,
        inner: &mut SemanticInner,
    ) -> Result<Option<MemoryEntry>, MlcError> {
        if inner.vectors.is_empty() {
            return Ok(None);
        }

        // 取出最旧的条目 ID(VecDeque::pop_front O(1),原 Vec::remove(0) O(n))
        // WHY pop_front:VecDeque 双端队列,头部弹出 O(1);
        // Vec::remove(0) 需移动所有元素 O(n),高容量下驱逐开销显著
        let (shared_clv, victim_id) = match inner.vectors.pop_front() {
            Some(pair) => pair,
            None => return Ok(None),
        };
        // 近似层:被驱逐条目打墓碑(图不可删点)
        inner.tier_forget(&victim_id);

        // 清理池:被驱逐的 SharedCLV drop 后,检查池中对应 Arc 是否仅池引用
        // WHY 延迟清理:shared_clv 在此 drop,Arc 引用计数 -1;
        // 若池中该 Arc 引用计数为 1(仅池持有),移除以释放内存
        let dropped_arc_hash = shared_clv.content_hash();
        drop(shared_clv);
        if let Some(pool_arc) = inner.clv_pool.get(&dropped_arc_hash) {
            if Arc::strong_count(pool_arc) <= 1 {
                inner.clv_pool.remove(&dropped_arc_hash);
            }
        }

        let victim = inner
            .entries
            .remove(&victim_id)
            .map(|v| Arc::try_unwrap(v).unwrap_or_else(|arc| (*arc).clone()));

        if let Some(ref v) = victim {
            trace!(victim_id = %v.id, "L2 FIFO 驱逐完成");
        }
        Ok(victim)
    }

    /// 移除指定条目(不更新驱逐计数)
    ///
    /// 移除后清理池中无引用的 Arc(SubTask 13.1),并对近似层打墓碑(ADR-192 D3)。
    pub fn remove(&self, id: &str) -> Result<Option<MemoryEntry>, MlcError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;

        let entry = inner.entries.remove(id);
        if entry.is_some() {
            // 近似层:图侧同步打墓碑(`&str` → `MemoryId` 走 id_newtype 的 Into 实现)
            let removed_id: MemoryId = id.into();
            inner.tier_forget(&removed_id);
            // 收集被移除条目的 CLV 哈希,用于池清理
            let mut removed_hashes: Vec<u64> = Vec::new();
            let before_len = inner.vectors.len();
            inner.vectors.retain(|(shared_clv, mid)| {
                if mid.as_str() == id {
                    removed_hashes.push(shared_clv.content_hash());
                    false
                } else {
                    true
                }
            });
            debug_assert_eq!(inner.vectors.len(), before_len - 1);

            // 清理池中无引用的 Arc
            for h in removed_hashes {
                if let Some(pool_arc) = inner.clv_pool.get(&h) {
                    if Arc::strong_count(pool_arc) <= 1 {
                        inner.clv_pool.remove(&h);
                    }
                }
            }
        }
        // Arc<MemoryEntry> → MemoryEntry(try_unwrap 零拷贝,共享时 fallback clone)
        Ok(entry.map(|v| Arc::try_unwrap(v).unwrap_or_else(|arc| (*arc).clone())))
    }

    /// 列出所有条目(深拷贝,用于迁移或快照)
    ///
    /// WHY 保留 list_all:API 兼容,调用方需 owned `MemoryEntry`。
    /// 热路径(批量只读扫描)应优先使用 `list_all_arc()` 避免 4096 条目 ~8MB 深拷贝。
    pub fn list_all(&self) -> Result<Vec<MemoryEntry>, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(inner.entries.values().map(|v| (**v).clone()).collect())
    }

    /// 列出所有条目的 Arc 引用(零拷贝共享,避免全量深拷贝)
    ///
    /// WHY list_all_arc:`list_all()` 返回 `Vec<MemoryEntry>` 需深拷贝每个条目,
    /// 4096 条目时 ~8MB 堆分配。本方法返回 `Vec<Arc<MemoryEntry>>`,
    /// 通过 `Arc::clone` 仅增加引用计数(原子 +1),无堆分配,适用于迁移/快照等热路径。
    pub fn list_all_arc(&self) -> Result<Vec<Arc<MemoryEntry>>, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(inner.entries.values().map(Arc::clone).collect())
    }

    /// 清空所有条目、CLV 池与近似层侧表
    pub fn clear(&self) -> Result<(), MlcError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        inner.entries.clear();
        inner.vectors.clear();
        inner.clv_pool.clear();
        // 近似层随主表作废(下次达阈召回会重新惰性建图)
        inner.hnsw = None;
        Ok(())
    }

    /// 返回 CLV 池中不同 CLV 的数量(用于内存占用诊断与测试)
    ///
    /// WHY 暴露此方法:SubTask 13.1 验证要求"4096 条目后 CLV 总内存 < 2MB",
    /// 通过池大小可计算 CLV 内存 = pool_size × 2KB,验证共享效果。
    pub fn clv_pool_size(&self) -> Result<usize, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(inner.clv_pool.len())
    }

    /// 返回 CLV 池占用的近似内存(字节)
    ///
    /// 计算:pool_size × (512 × 4 + Arc 元数据 16) ≈ pool_size × 2064 字节
    pub fn clv_pool_memory_bytes(&self) -> Result<usize, MlcError> {
        let pool_size = self.clv_pool_size()?;
        // Arc<[f32]> 内存:512 × 4 字节数据 + Arc 元数据(约 16 字节)
        Ok(pool_size * (CLV::DIMENSION * 4 + 16))
    }

    /// 返回近似层状态快照(诊断与测试用,ADR-192)
    ///
    /// WHY 暴露:与 `clv_pool_size` 同纪律 —— 分层路由是否生效、墓碑是否临近重建、
    /// 图与主表是否一致,均需可观测才能写出不靠运气的断言(也供运维诊断)。
    pub fn hnsw_tier_snapshot(&self) -> Result<HnswTierSnapshot, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(HnswTierSnapshot {
            enabled: self.hnsw_enabled,
            threshold: self.hnsw_threshold,
            built: inner.hnsw.is_some(),
            live_points: inner.hnsw.as_ref().map_or(0, HnswTier::live_points),
            tombstones: inner.hnsw.as_ref().map_or(0, |tier| tier.tombstones),
            stale: inner.hnsw.as_ref().is_some_and(|tier| tier.stale),
        })
    }
}

/// L2 近似层状态快照(诊断与测试用,ADR-192)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HnswTierSnapshot {
    /// 运行时是否允许近似层(`CHIMERA_NO_HNSW_MLC` 未设置即为 true)
    pub enabled: bool,
    /// 生效的切换阈值(0 = 关闭近似层)
    pub threshold: usize,
    /// 图是否已建(惰性建图:首次达阈的召回才建)
    pub built: bool,
    /// 图侧存活点数(等于 `len()` 时为一致态)
    pub live_points: usize,
    /// 已打墓碑的图点数(待重建回收)
    pub tombstones: usize,
    /// 是否处于 stale(墓碑超限;下一次召回重建,期间回退精确扫描)
    pub stale: bool,
}

/// 精确层召回(线性扫描 KNN)—— ADR-005 降级后的原实现,逐位保持不动
///
/// WHY 独立成自函数:分层后两个调用点(精确路由 / 重建窗口回退 / 图不一致防御)
/// 共用同一份实现,避免“回退路径与主路径语义不同”这类隐性漂移。
///
/// WHY 索引替代 MemoryId clone(SubTask 19.3):评分阶段创建 `Vec<(usize, f32)>`
/// 而非 `Vec<(MemoryId, f32)>`,4096 条目可省 4096 次 String 堆分配/释放;
/// Top-K 选择后才从 `vectors` 取 MemoryId(仅对通常 ≤ 10 个结项 clone)。
fn recall_exact(inner: &SemanticInner, query: &CLV, top_k: usize) -> Vec<(MemoryId, f32)> {
    // 计算所有向量的相似度,存储 vectors 索引而非 MemoryId
    // WHY 索引:usize 是 Copy 类型,8 字节栈分配,无需堆分配;
    // MemoryId(String) clone 需堆分配,4096 条目 = 4096 次堆分配/释放
    let mut scored: Vec<(usize, f32)> = inner
        .vectors
        .iter()
        .enumerate()
        .map(|(idx, (shared_clv, _))| {
            let sim = shared_clv.cosine_similarity_clv(query);
            // clamp 到 [0.0, 1.0]:负值表示语义相反,对召回无意义
            let clamped = sim.clamp(0.0, 1.0);
            (idx, clamped)
        })
        .collect();

    // 按相似度降序部分排序(仅取 Top-K,O(n) 而非 O(n log n))
    // WHY select_nth_unstable_by:Top-K 召回只需前 K 个最相似元素,
    // 全排序浪费计算。部分排序将第 K 大元素放到正确位置,
    // 前 K 个元素为 Top-K(内部无序),再对前 K 个排序确保降序
    if top_k < scored.len() {
        scored.select_nth_unstable_by(top_k, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal)
        });
    }
    // 对前 top_k 个元素排序,确保降序输出(K log K << n log n)
    let k = top_k.min(scored.len());
    scored[..k].sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    scored.truncate(top_k);

    // Top-K 选择后,从 vectors 中取 MemoryId 构造返回值
    // WHY 延迟 clone:仅对 Top-K(通常 ≤ 10)条目 clone MemoryId,
    // 而非对全部 4096 条目 clone,消除 4086+ 次无用 String 分配
    let result: Vec<(MemoryId, f32)> = scored
        .into_iter()
        .filter_map(|(idx, score)| inner.vectors.get(idx).map(|(_, id)| (id.clone(), score)))
        .collect();

    trace!(top_k, returned = result.len(), "L2 KNN 召回完成(精确层)");
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_clv(seed: f32) -> CLV {
        // 构造非零 CLV:每个维度设为 seed,确保余弦相似度有意义
        let v = vec![seed; CLV::DIMENSION];
        CLV::from_vec(v).unwrap()
    }

    fn make_clv_with_value(dim_0: f32) -> CLV {
        // 构造仅在 dim_0 不同的 CLV,用于测试正交性
        let mut v = vec![0.0_f32; CLV::DIMENSION];
        v[0] = dim_0;
        CLV::from_vec(v).unwrap()
    }

    fn make_entry(id: &str, clv: CLV) -> MemoryEntry {
        MemoryEntry::new(id, format!("content-{id}"), MemoryTier::L2Semantic).with_clv(clv)
    }

    #[test]
    fn test_insert_requires_clv() {
        let mem = SemanticMemory::new(64);
        let entry = MemoryEntry::new("m-1", "content", MemoryTier::L2Semantic);
        let err = mem.insert(entry).unwrap_err();
        assert!(matches!(err, MlcError::InvalidConfig(_)));
    }

    #[test]
    fn test_insert_and_get() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        let entry = make_entry("m-1", clv);
        mem.insert(entry.clone()).unwrap();

        let fetched = mem.get("m-1").unwrap();
        assert_eq!(fetched.id.as_str(), "m-1");
        assert_eq!(fetched.tier, MemoryTier::L2Semantic);
        assert!(fetched.clv.is_some());
    }

    #[test]
    fn test_recall_by_clv_identical_returns_one() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        mem.insert(make_entry("m-1", clv.clone())).unwrap();

        let results = mem.recall_by_clv(&clv, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "m-1");
        // 相同向量余弦相似度 ≈ 1.0
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_recall_by_clv_top_k_ordering() {
        let mem = SemanticMemory::new(64);

        // 插入 3 个条目,与 query 的相似度递减
        // query = [1.0, 0, 0, ...]
        // m-1 = [1.0, 0, 0, ...] → 相似度 1.0
        // m-2 = [0.5, 0, 0, ...] → 相似度 1.0(方向相同,余弦=1.0)
        // m-3 = [0.0, 1.0, 0, ...] → 相似度 0.0(正交)
        let query = make_clv_with_value(1.0);
        mem.insert(make_entry("m-1", make_clv_with_value(1.0)))
            .unwrap();
        mem.insert(make_entry("m-2", make_clv_with_value(0.5)))
            .unwrap();
        mem.insert(make_entry("m-3", make_clv_with_value(0.0)))
            .unwrap();
        // m-3 的 dim_0=0,但其他维度也全 0,是零向量,相似度为 0.0
        // 用另一个向量替换 m-3
        mem.remove("m-3").unwrap();
        let mut v3 = vec![0.0_f32; CLV::DIMENSION];
        v3[1] = 1.0; // dim_1=1,与 query 正交
        mem.insert(make_entry("m-3", CLV::from_vec(v3).unwrap()))
            .unwrap();

        let results = mem.recall_by_clv(&query, 3).unwrap();
        assert_eq!(results.len(), 3);
        // m-1 和 m-2 相似度应为 1.0(方向相同),m-3 应为 0.0(正交)
        let m1_score = results
            .iter()
            .find(|(id, _)| id.as_str() == "m-1")
            .map(|(_, s)| *s);
        let m2_score = results
            .iter()
            .find(|(id, _)| id.as_str() == "m-2")
            .map(|(_, s)| *s);
        let m3_score = results
            .iter()
            .find(|(id, _)| id.as_str() == "m-3")
            .map(|(_, s)| *s);

        assert!(m1_score.is_some());
        assert!(m2_score.is_some());
        assert!(m3_score.is_some());
        assert!((m1_score.unwrap() - 1.0).abs() < 1e-5);
        assert!((m2_score.unwrap() - 1.0).abs() < 1e-5);
        assert!(m3_score.unwrap() < 1e-6);
    }

    #[test]
    fn test_recall_by_clv_top_k_limit() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        for i in 0..10 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }

        let results = mem.recall_by_clv(&clv, 3).unwrap();
        assert_eq!(results.len(), 3); // top_k=3
    }

    #[test]
    fn test_recall_by_clv_empty() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        let results = mem.recall_by_clv(&clv, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_recall_by_clv_zero_top_k() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        mem.insert(make_entry("m-1", clv.clone())).unwrap();
        let results = mem.recall_by_clv(&clv, 0).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_similarity_clamped_to_zero() {
        let mem = SemanticMemory::new(64);

        // 构造正交向量(相似度 0.0)
        let mut v1 = vec![0.0_f32; CLV::DIMENSION];
        v1[0] = 1.0;
        let mut v2 = vec![0.0_f32; CLV::DIMENSION];
        v2[1] = 1.0;

        mem.insert(make_entry("m-1", CLV::from_vec(v1).unwrap()))
            .unwrap();
        let query = CLV::from_vec(v2).unwrap();

        let results = mem.recall_by_clv(&query, 10).unwrap();
        assert_eq!(results.len(), 1);
        // 正交向量相似度应为 0.0(clamp 后)
        assert!(results[0].1 < 1e-6);
        assert!(results[0].1 >= 0.0); // 不为负
    }

    #[test]
    fn test_fifo_eviction_on_overflow() {
        let mem = SemanticMemory::new(2);
        let clv = make_clv(1.0);

        mem.insert(make_entry("m-1", clv.clone())).unwrap();
        mem.insert(make_entry("m-2", clv.clone())).unwrap();
        assert_eq!(mem.len().unwrap(), 2);

        // 插入第 3 个,应驱逐 m-1(最旧)
        let evicted = mem.insert(make_entry("m-3", clv.clone())).unwrap();
        assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-1"));
        assert_eq!(mem.evictions(), 1);
        assert_eq!(mem.len().unwrap(), 2);
        assert!(mem.get("m-1").is_err());
        assert!(mem.get("m-3").is_ok());
    }

    #[test]
    fn test_update_existing_removes_old_vector() {
        let mem = SemanticMemory::new(2);

        let clv1 = make_clv_with_value(1.0);
        let clv2 = make_clv_with_value(0.5);

        // 插入 m-1(clv1)
        mem.insert(make_entry("m-1", clv1.clone())).unwrap();
        assert_eq!(mem.len().unwrap(), 1);

        // 更新 m-1 为 clv2
        mem.insert(make_entry("m-1", clv2.clone())).unwrap();
        assert_eq!(mem.len().unwrap(), 1); // 不应增加

        // 用 clv2 查询,m-1 应匹配
        let results = mem.recall_by_clv(&clv2, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "m-1");
        assert!((results[0].1 - 1.0).abs() < 1e-5);

        // 用 clv1 查询,m-1 不应匹配(旧向量已移除)
        let results = mem.recall_by_clv(&clv1, 10).unwrap();
        assert_eq!(results.len(), 1);
        // m-1 的 clv2 与 clv1 方向相同(都是 dim_0 非零),相似度仍为 1.0
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_remove() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        mem.insert(make_entry("m-1", clv.clone())).unwrap();

        let removed = mem.remove("m-1").unwrap();
        assert!(removed.is_some());
        assert!(mem.get("m-1").is_err());

        // 移除后召回应返回空
        let results = mem.recall_by_clv(&clv, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_clear() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        for i in 0..5 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }
        assert_eq!(mem.len().unwrap(), 5);

        mem.clear().unwrap();
        assert_eq!(mem.len().unwrap(), 0);
    }

    #[test]
    fn test_list_all_arc_shares_entries() {
        // 验证 list_all_arc 返回 Arc 引用,内容与 list_all 一致且 Arc 共享
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        for i in 0..3 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }
        let arcs = mem.list_all_arc().unwrap();
        assert_eq!(arcs.len(), 3);
        // 验证 Arc 内数据正确
        assert!(arcs.iter().all(|a| a.id.as_str().starts_with("m-")));
        // 验证 Arc 共享:存储中的 Arc 与返回的 Arc 是同一份(refcount > 1)
        assert!(arcs.iter().all(|a| Arc::strong_count(a) >= 2));
    }

    // 注:test_recall_performance_100_entries 已删除(与 tests/semantic.rs 中的
    // test_l2_recall_performance_100_entries 重复,仅保留 tests/ 版本)

    // ============================================================
    // ADR-192 分层检索(近似层机制)单测
    //
    // WHY 用小阈值而非 4096:分层路由/墓碑/重建是与规模无关的机制,
    // 以 `with_hnsw_threshold(cap, 8)` 验证机制可在 debug 下秒级完成;
    // 默认阈值 4096 本身由 `test_default_threshold_and_constant_pinned` 固定,
    // 真实规模的召回质量判据在 tests/hnsw_tiered.rs 与 benches/hnsw_tiered.rs 把关。
    // ============================================================

    /// 构造方向互异的 CLV(第 `i` 维与相邻位非零,保证 512 维内方向可分)
    fn make_direction_clv(i: usize) -> CLV {
        let mut v = vec![0.0_f32; CLV::DIMENSION];
        v[i % CLV::DIMENSION] = 1.0;
        v[(i * 13 + 7) % CLV::DIMENSION] = 0.5;
        v[(i * 29 + 3) % CLV::DIMENSION] = 0.25;
        CLV::from_vec(v).unwrap()
    }

    /// 插入 n 个方向互异的条目
    fn fill_directions(mem: &SemanticMemory, n: usize) {
        for i in 0..n {
            mem.insert(make_entry(&format!("m-{i}"), make_direction_clv(i)))
                .unwrap();
        }
    }

    /// 关闭近似层(阈值 0)的实例,用作精确层基线
    fn exact_mem(capacity: usize) -> SemanticMemory {
        SemanticMemory::with_hnsw_threshold(capacity, 0)
    }

    #[test]
    fn test_default_threshold_and_constant_pinned() {
        // 默认切换阈值 = 4096(精确层红线守护规模),与 ADR-192 D2 一致
        assert_eq!(HNSW_MIN_ENTRIES, 4096);
        let mem = SemanticMemory::new(64);
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert_eq!(snap.threshold, HNSW_MIN_ENTRIES);
    }

    #[test]
    fn test_tier_not_built_below_threshold() {
        // 边界下沿:7 条目(< 阈值 8)不得建图,仍走精确扫描
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&mem, 7);
        let results = mem.recall_by_clv(&make_direction_clv(3), 5).unwrap();
        assert_eq!(results.len(), 5);
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert!(!snap.built, "条目数低于阈值时不应建图");
        assert_eq!(snap.live_points, 0);
    }

    #[test]
    fn test_tier_engages_at_threshold_and_stays_consistent() {
        // 边界上沿:恰达阈值的首次召回惰性建图,图侧存活点 == 主表条目数
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&mem, 8);
        assert!(!mem.hnsw_tier_snapshot().unwrap().built, "建图前不应已建");

        let results = mem.recall_by_clv(&make_direction_clv(3), 5).unwrap();
        assert_eq!(results.len(), 5);
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert!(snap.built, "达阈召回后应已建图");
        assert_eq!(snap.live_points, 8);
        assert_eq!(snap.live_points, mem.len().unwrap(), "图-主表一致性");
        assert_eq!(snap.tombstones, 0);
        assert!(!snap.stale);
    }

    #[test]
    fn test_approx_scores_bitwise_match_exact_for_same_id() {
        // 分数契约:近似层返回的分数与精确层对同一 id 逐位相等(重算同函数同参数序)
        let approx = SemanticMemory::with_hnsw_threshold(256, 8);
        let exact = exact_mem(256);
        fill_directions(&approx, 16);
        fill_directions(&exact, 16);
        let query = make_direction_clv(5);
        // 基线取 top_k = 全量 → 包含每个存活点的精确分数,供逐位比对
        let e = exact.recall_by_clv(&query, 16).unwrap();
        let a = approx.recall_by_clv(&query, 8).unwrap();
        assert!(!a.is_empty());
        assert_eq!(a.len(), 8, "近似层长度契约 top_k ≤ 条目数");
        for (id, score) in &a {
            let (_, exact_score) = e
                .iter()
                .find(|(eid, _)| eid == id)
                .unwrap_or_else(|| panic!("近似层返回了主表外的 id: {}", id.as_str()));
            assert_eq!(
                score.to_bits(),
                exact_score.to_bits(),
                "id {} 分数漂移",
                id.as_str()
            );
        }
        // 降序契约
        for w in a.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
    }

    #[test]
    fn test_update_and_remove_mark_tombstone() {
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&mem, 8);
        mem.recall_by_clv(&make_direction_clv(0), 4).unwrap(); // 建图

        // 同 ID 更新:旧 dataid 打墓碑,存活点仍 == 条目数
        mem.insert(make_entry("m-0", make_direction_clv(11)))
            .unwrap();
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert_eq!(snap.tombstones, 1);
        assert_eq!(snap.live_points, mem.len().unwrap());

        // remove:同样打墓碑(更新已新增一点,故存活点 = 8 - 1 = 7 仍 == 条目数)
        mem.remove("m-1").unwrap();
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert_eq!(snap.tombstones, 2);
        assert_eq!(snap.live_points, mem.len().unwrap());
        assert_eq!(snap.live_points, 7);
    }

    #[test]
    fn test_eviction_marks_tombstone() {
        // 容量满驱逐走 evict_oldest_locked 路径,同样必须打墓碑
        let mem = SemanticMemory::with_hnsw_threshold(8, 8);
        fill_directions(&mem, 8);
        mem.recall_by_clv(&make_direction_clv(1), 3).unwrap(); // 建图

        mem.insert(make_entry("m-100", make_direction_clv(100)))
            .unwrap();
        assert_eq!(mem.evictions(), 1);
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert_eq!(snap.tombstones, 1, "驱逐应在图侧留下墓碑");
        assert_eq!(snap.live_points, mem.len().unwrap());
        // 被驱逐条目不得出现在结果中(不变量:结果 id ∈ entries)
        let results = mem.recall_by_clv(&make_direction_clv(0), 8).unwrap();
        assert!(results.iter().all(|(id, _)| mem.get(id.as_str()).is_ok()));
        assert!(!results.iter().any(|(id, _)| id.as_str() == "m-0"));
    }

    #[test]
    fn test_stale_then_rebuild_clears_tombstones() {
        // 阈值 8 + 16 条目:死点达 25% 时仍保持 len ≥ 阈值,能走完「重建后继续近似」一整环
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&mem, 16);
        mem.recall_by_clv(&make_direction_clv(2), 4).unwrap();
        for i in 0..4 {
            mem.remove(&format!("m-{i}")).unwrap();
        }
        let stale_snap = mem.hnsw_tier_snapshot().unwrap();
        assert!(stale_snap.stale, "4/12 死点应触发 stale 标记");
        assert_eq!(stale_snap.tombstones, 4);

        // stale 期间的召回仍正确(重建后走近似层)
        let results = mem.recall_by_clv(&make_direction_clv(2), 4).unwrap();
        assert_eq!(results.len(), 4);
        assert!(!results.iter().any(|(id, _)| id.as_str() == "m-0"));
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert!(!snap.stale, "重建后应解除 stale");
        assert_eq!(snap.tombstones, 0, "重建应清空墓碑");
        assert_eq!(snap.live_points, mem.len().unwrap());
        assert_eq!(snap.live_points, 12);
    }

    #[test]
    fn test_recall_is_deterministic_within_instance() {
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&mem, 8);
        let query = make_direction_clv(4);
        let first = mem.recall_by_clv(&query, 5).unwrap();
        for _ in 0..3 {
            let again = mem.recall_by_clv(&query, 5).unwrap();
            assert_eq!(first, again, "同一实例同一 query 多次召回必须逐位一致");
        }
    }

    #[test]
    fn test_tier_boundaries_empty_and_zero_top_k() {
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        // 空输入
        assert!(mem
            .recall_by_clv(&make_direction_clv(0), 10)
            .unwrap()
            .is_empty());
        fill_directions(&mem, 8);
        // top_k = 0(近似层也应短路返回空)
        assert!(mem
            .recall_by_clv(&make_direction_clv(0), 0)
            .unwrap()
            .is_empty());
        // top_k 超过条目数 → 钉到条目数
        let results = mem.recall_by_clv(&make_direction_clv(0), 100).unwrap();
        assert_eq!(results.len(), 8, "top_k 超界应钳到存活条目数");
    }

    #[test]
    fn test_clear_drops_tier() {
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&mem, 8);
        mem.recall_by_clv(&make_direction_clv(0), 4).unwrap();
        assert!(mem.hnsw_tier_snapshot().unwrap().built);

        mem.clear().unwrap();
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert!(!snap.built, "clear 后侧表应作废");
        assert_eq!(snap.live_points, 0);
        // 重新填充后仍可再次惰性建图
        fill_directions(&mem, 8);
        assert_eq!(
            mem.recall_by_clv(&make_direction_clv(0), 3).unwrap().len(),
            3
        );
        assert!(mem.hnsw_tier_snapshot().unwrap().built);
    }

    /// 本线程已持读锁时 `try_write` 必失败 → 进入「重建窗口」分支回退精确扫描
    ///
    /// WHY 能确定性触发：crate 内单测可直接持有 `inner` 读守卫（外部测试无此能力），
    /// 而不需赌并发时序；该分支是 ADR-192 D3「降级方向=变慢而非变错」的兑现路径。
    #[test]
    fn test_rebuild_window_falls_back_to_exact_when_write_lock_busy() {
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&mem, 8);
        let query = make_direction_clv(2);

        // 持有读守卫 → recall 内的 try_write 拿不到写锁 → 走精确回退
        let results = {
            let _reader = mem.inner.read().unwrap();
            mem.recall_by_clv(&query, 5).unwrap()
        };
        assert_eq!(results.len(), 5, "回退路径仍须满足长度契约");
        assert!(
            !mem.hnsw_tier_snapshot().unwrap().built,
            "回退本轮不应留下侧表"
        );

        // 回退结果与关闭近似层的基线逐位一致（降级不改变语义）
        let off = exact_mem(64);
        fill_directions(&off, 8);
        assert_eq!(results, off.recall_by_clv(&query, 5).unwrap());
    }

    #[test]
    fn test_route_decision_axes() {
        // 穷举 `route()` 四个轴向：关闭/低于阈值/达阈未建图/已建图新鲜/已 stale
        let off = exact_mem(64);
        fill_directions(&off, 12);
        {
            let inner = off.inner.read().unwrap();
            assert_eq!(
                off.route(&inner.hnsw, inner.entries.len()),
                TierRoute::Exact
            );
        }

        let small = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&small, 7);
        {
            let inner = small.inner.read().unwrap();
            assert_eq!(
                small.route(&inner.hnsw, inner.entries.len()),
                TierRoute::Exact
            );
        }

        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&mem, 8);
        {
            let inner = mem.inner.read().unwrap();
            assert_eq!(
                mem.route(&inner.hnsw, inner.entries.len()),
                TierRoute::RebuildThenApprox,
                "达阈但未建图应走重建窗"
            );
        }
        mem.recall_by_clv(&make_direction_clv(0), 3).unwrap();
        {
            let inner = mem.inner.read().unwrap();
            assert_eq!(
                mem.route(&inner.hnsw, inner.entries.len()),
                TierRoute::Approx
            );
        }
        // 死点占比达 25%（16 存活中删 4）后应回到重建轴
        let big = SemanticMemory::with_hnsw_threshold(64, 8);
        fill_directions(&big, 16);
        big.recall_by_clv(&make_direction_clv(1), 3).unwrap();
        for i in 0..4 {
            big.remove(&format!("m-{i}")).unwrap();
        }
        {
            let inner = big.inner.read().unwrap();
            assert_eq!(
                big.route(&inner.hnsw, inner.entries.len()),
                TierRoute::RebuildThenApprox,
                "stale 后不得继续用旧图服务"
            );
        }
    }

    #[test]
    fn test_threshold_zero_disables_tier_entirely() {
        // 与 CHIMERA_NO_HNSW_MLC 等价的代码级开关:永不建图,结果与历史行为一致
        let mem = exact_mem(64);
        fill_directions(&mem, 12);
        let results = mem.recall_by_clv(&make_direction_clv(3), 5).unwrap();
        assert_eq!(results.len(), 5);
        let snap = mem.hnsw_tier_snapshot().unwrap();
        assert_eq!(snap.threshold, 0);
        assert!(!snap.built);
    }
}
