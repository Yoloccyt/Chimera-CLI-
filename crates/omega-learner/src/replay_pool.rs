//! P4-W16.2.1: 经验回放池 — off-policy RL 训练的轨迹存储与采样基础设施
//!
//! 对应架构层: L6 Router(omega-learner)
//! 对应 spec.md §Scenario "经验回放池"
//! 对应 tasks.md P4-W16.2.1: 实现回放池(≥10K 轨迹)
//!
//! # 设计原则
//!
//! ## 1. 泛型设计(`ReplayPool<T>`)
//! 回放池不绑定具体轨迹类型,允许上层按需实例化:
//! - `ReplayPool<model_router::TrajectoryEvent>` — 捕获点 1(路由轨迹)
//! - `ReplayPool<quest_engine::QuestTrajectory>` — 捕获点 2(Quest 状态轨迹)
//! - `ReplayPool<ReplaySample>` — 统一样本格式(推荐)
//!
//! WHY 泛型而非具体类型:
//! - 避免依赖方向违规(omega-learner L6 不能依赖 quest-engine L9)
//! - 上层(L9 efficiency-monitor / L10 chimera-cli)统一实例化与填充
//! - 符合 §4.1 "避免 Box<dyn Trait>,优先 impl Trait 或 enum dispatch"
//!
//! ## 2. 有界缓冲 + FIFO 淘汰
//! 默认容量 10_000(与 spec "≥10K 轨迹"目标对齐)。
//! 超出容量时淘汰最旧条目,与 P4-W16.1.2 `RecordingHook` 同模式。
//! WHY FIFO 而非 LRU:RL 训练需要时序多样性,旧轨迹应优先淘汰。
//!
//! ## 3. 随机采样(with replacement)
//! off-policy RL(CQL/IQL)需要从回放池均匀采样 mini-batch。
//! 默认使用有放回采样(with replacement),允许 batch 内重复,
//! 数学上保证梯度估计的无偏性(Sutton & Barto, 2018, §5.5)。
//!
//! ## 4. 线程安全(Send + Sync)
//! - `buffer` 用 `Mutex<VecDeque>` 保护,Push/Sample 互斥
//! - `stats` 用 `AtomicU64` 计数,热路径(`push`)无锁读取
//! - 整体 `Send + Sync`,满足跨 async 任务共享需求
//!
//! # R2 冻结声明(ADR-042)
//!
//! **冻结状态**:R2(GSOE×AutoDPO 约束 RL)路径在 FormalVerifier 落地前无条件冻结。
//!
//! - **冻结依据**:[ADR-042](docs/architecture/ADR-042-r2-freeze-before-formal-verifier.md)(2026-07-25 批准)
//! - **冻结范围**:本回放池的轨迹数据**仅可用于 R1 路径**(召回配额 CQL/IQL,需满足影子模式 2 周前置,P4-W16.2.4),**禁止用于 R2 路径**(GSOE×AutoDPO 约束 RL)
//! - **解冻条件**:FormalVerifier 落地 + 新 ADR 评审通过 + 影子模式 2 周验证(ADR-042 决策 3)
//! - **违反处置**:自动回滚 + `NexusEvent::R2FreezeViolation` Critical 告警 + 事故复盘(ADR-042 决策 4)
//! - **关联 ADR**:[ADR-032 决策 4](docs/architecture/ADR-032-dual-channel-evaluator.md)(验证器层级跃迁路径)
//!
//! 本回放池本身(P4-W16.2.1)已落地,不在冻结范围,但数据使用受 R2 冻结约束。
//! R2 路径(GSOE×AutoDPO 约束 RL)在 FormalVerifier 落地前完全不存在于运行时。
//!
//! # 数据流
//! ```text
//! 捕获点 1 (model-router RecordingHook)
//!     │ drain() → Vec<TrajectoryEvent>
//!     ▼
//! ┌─────────────────────────────────────────┐
//! │         ReplayPool<T>                   │
//! │  ┌─────────────────────────────────┐    │
//! │  │  buffer: VecDeque<T>            │    │
//! │  │  [t_0, t_1, ..., t_N]           │    │
//! │  │  FIFO 淘汰 ↑        ↓ push      │    │
//! │  └─────────────────────────────────┘    │
//! │  stats: AtomicU64 (total/sampled/evicted)│
//! └─────────────────────────────────────────┘
//!     │ sample(batch_size, rng) → Vec<T>
//!     ▼
//! 离线 RL 训练 (CQL/IQL — P4-W16.2.2)
//! ```
//!
//! # 容量规划
//! - 单条 TrajectoryEvent ≈ 200 bytes(序列化后)
//! - 10K 轨迹 ≈ 2MB 内存,完全在预算内
//! - 生产环境可通过 `with_capacity` 自定义

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

// ============================================================
// ReplayPool 核心实现
// ============================================================

/// 经验回放池 — off-policy RL 训练的轨迹存储与采样基础设施
///
/// 对应 spec.md §P4-W16.2.1 "实现回放池(≥10K 轨迹)"
///
/// # 泛型参数
/// - `T`:轨迹样本类型,需满足 `Clone + Send + Sync`
///   (可选 `Serialize + Deserialize` 用于持久化,本期不实现持久化)
///
/// # 容量管理
/// 默认容量 10_000(与 spec "≥10K 轨迹"对齐)。
/// 超出时 FIFO 淘汰最旧条目并 `tracing::debug!` 记录。
///
/// # 线程安全
/// - `buffer` 用 `Mutex<VecDeque>` 保护
/// - `stats` 用 `AtomicU64` 计数,热路径无锁
/// - 整体 `Send + Sync`
///
/// # 使用示例
/// ```
/// use omega_learner::replay_pool::ReplayPool;
/// use rand::thread_rng;
///
/// // 创建默认容量(10K)的回放池
/// let pool: ReplayPool<i32> = ReplayPool::new();
///
/// // 推入轨迹
/// for i in 0..100 {
///     pool.push(i);
/// }
/// assert_eq!(pool.len(), 100);
///
/// // 随机采样 mini-batch
/// let mut rng = thread_rng();
/// let batch = pool.sample(10, &mut rng);
/// assert_eq!(batch.len(), 10);
/// ```
pub struct ReplayPool<T> {
    /// 有界缓冲区 — FIFO 顺序保存轨迹样本
    buffer: Mutex<VecDeque<T>>,
    /// 缓冲区容量 — 超出则 FIFO 淘汰最旧条目
    capacity: usize,
    /// 累计推入总数(含已淘汰)— AtomicU64 无锁热路径
    total_pushed: AtomicU64,
    /// 累计采样总数 — 用于观测训练活跃度
    total_sampled: AtomicU64,
    /// 因容量超限被淘汰的条目数 — 用于运维观测
    evicted_count: AtomicU64,
}

impl<T> ReplayPool<T>
where
    T: Clone + Send + Sync,
{
    /// 创建默认容量(10_000)的回放池
    ///
    /// # 容量选择
    /// 10_000 与 spec "≥10K 轨迹"目标对齐。
    /// 生产环境可通过 [`with_capacity`] 自定义。
    pub fn new() -> Self {
        Self::with_capacity(10_000)
    }

    /// 创建指定容量的回放池
    ///
    /// # 参数
    /// - `capacity`:缓冲区最大条目数,超出时 FIFO 淘汰最旧条目
    ///
    /// # 约束
    /// - `capacity = 0` 视为 1(避免 push 时边界条件)
    /// - 容量过小会增加淘汰频率,建议 ≥1_000
    pub fn with_capacity(capacity: usize) -> Self {
        let normalized = if capacity == 0 { 1 } else { capacity };
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(normalized)),
            capacity: normalized,
            total_pushed: AtomicU64::new(0),
            total_sampled: AtomicU64::new(0),
            evicted_count: AtomicU64::new(0),
        }
    }

    /// 获取当前缓冲区中的样本数(不含已淘汰)
    ///
    /// # 注意
    /// 此方法获取 Mutex,不应在热路径调用。
    /// 仅供运维查询或测试断言使用。
    pub fn len(&self) -> usize {
        self.buffer.lock().map(|buf| buf.len()).unwrap_or(0)
    }

    /// 判断缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 获取缓冲区容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 获取累计推入总数(含已淘汰)— 原子读,无锁
    pub fn total_pushed(&self) -> u64 {
        self.total_pushed.load(Ordering::Relaxed)
    }

    /// 获取累计采样总数 — 原子读,无锁
    pub fn total_sampled(&self) -> u64 {
        self.total_sampled.load(Ordering::Relaxed)
    }

    /// 获取因容量超限被淘汰的条目数 — 原子读,无锁
    pub fn evicted_count(&self) -> u64 {
        self.evicted_count.load(Ordering::Relaxed)
    }

    /// 获取回放池统计快照 — 一次性返回所有计数器
    ///
    /// # 返回
    /// `ReplayPoolStats` 包含 total_pushed/total_sampled/evicted/buffer_len 字段,
    /// 便于上层(L9 efficiency-monitor / L10 TUI)统一观测。
    ///
    /// # 性能
    /// 3 次 AtomicU64::load + 1 次 Mutex lock,~100ns,适合定期采样。
    pub fn stats(&self) -> ReplayPoolStats {
        ReplayPoolStats {
            total_pushed: self.total_pushed.load(Ordering::Relaxed),
            total_sampled: self.total_sampled.load(Ordering::Relaxed),
            evicted_count: self.evicted_count.load(Ordering::Relaxed),
            buffer_len: self.len(),
            capacity: self.capacity,
        }
    }

    /// 推入一条轨迹样本 — 容量超限时 FIFO 淘汰最旧条目
    ///
    /// # 性能预算
    /// - 原子计数器更新:~5ns
    /// - Mutex lock + push_back:~50-100ns
    /// - 总开销:<200ns,适合在轨迹捕获后立即调用
    ///
    /// # 错误处理
    /// Mutex poison 时静默丢弃(避免 panic 拖垮调用方)
    pub fn push(&self, item: T) {
        self.total_pushed.fetch_add(1, Ordering::Relaxed);

        if let Ok(mut buf) = self.buffer.lock() {
            if buf.len() >= self.capacity {
                // FIFO 淘汰最旧条目
                buf.pop_front();
                self.evicted_count.fetch_add(1, Ordering::Relaxed);
                // WHY tracing::debug 而非 warn:容量淘汰是设计预期行为,
                // 高频 warn 会污染日志;生产环境可通过 stats().evicted_count 观测
                tracing::debug!(
                    capacity = self.capacity,
                    evicted_total = self.evicted_count.load(Ordering::Relaxed),
                    "ReplayPool 容量超限,FIFO 淘汰最旧条目"
                );
            }
            buf.push_back(item);
        }
        // Mutex poison 时静默丢弃(避免 panic)
    }

    /// 批量推入轨迹样本 — 便于 RecordingHook.drain() 结果直接灌入
    ///
    /// # 性能
    /// 单次 Mutex lock 批量 push,优于多次 push。
    pub fn extend<I>(&self, items: I)
    where
        I: IntoIterator<Item = T>,
    {
        if let Ok(mut buf) = self.buffer.lock() {
            for item in items {
                self.total_pushed.fetch_add(1, Ordering::Relaxed);
                if buf.len() >= self.capacity {
                    buf.pop_front();
                    self.evicted_count.fetch_add(1, Ordering::Relaxed);
                }
                buf.push_back(item);
            }
        }
    }

    /// 随机采样(有放回)— off-policy RL 训练标准采样方式
    ///
    /// # 算法
    /// 从缓冲区均匀随机选取 `batch_size` 个样本(允许重复)。
    /// 数学上保证梯度估计的无偏性(Sutton & Barto, 2018, §5.5)。
    ///
    /// # 参数
    /// - `batch_size`:期望的样本数
    /// - `rng`:随机数生成器(允许调用方注入确定性 RNG 用于测试)
    ///
    /// # 返回
    /// `Vec<T>` 长度 = min(batch_size, buffer.len())
    /// - 缓冲区为空时返回空 Vec
    /// - 缓冲区条目少于 batch_size 时返回所有条目(不重复)
    ///
    /// # 性能
    /// - Mutex lock + N 次 random index + clone:O(batch_size)
    /// - 10K 池中采样 32 条:~5-10μs
    pub fn sample<R: Rng>(&self, batch_size: usize, rng: &mut R) -> Vec<T> {
        let buf = match self.buffer.lock() {
            Ok(buf) => buf,
            Err(_) => return Vec::new(),
        };
        let len = buf.len();
        if len == 0 || batch_size == 0 {
            return Vec::new();
        }

        // 缓冲区条目少于 batch_size 时,返回所有条目(避免过度采样)
        let actual_size = batch_size.min(len);
        self.total_sampled
            .fetch_add(actual_size as u64, Ordering::Relaxed);

        // 有放回采样:每次随机选一个 index 并 clone
        // WHY 不用 iter::choose_multiple:该方法是 without replacement,
        // 而 off-policy RL 标准做法是 with replacement(保证无偏性)
        let mut result = Vec::with_capacity(actual_size);
        for _ in 0..actual_size {
            let idx = rng.gen_range(0..len);
            if let Some(item) = buf.get(idx) {
                result.push(item.clone());
            }
        }
        result
    }

    /// 随机采样(无放回)— 用于评估集或多样性优先场景
    ///
    /// # 算法
    /// 使用 `rand::seq::index::sample` 选取不重复索引,
    /// 适合需要样本多样性的场景(如评估集构造)。
    ///
    /// # 参数
    /// - `batch_size`:期望的样本数
    /// - `rng`:随机数生成器
    ///
    /// # 返回
    /// `Vec<T>` 长度 = min(batch_size, buffer.len())
    pub fn sample_without_replacement<R: Rng>(&self, batch_size: usize, rng: &mut R) -> Vec<T> {
        use rand::seq::index::sample;

        let buf = match self.buffer.lock() {
            Ok(buf) => buf,
            Err(_) => return Vec::new(),
        };
        let len = buf.len();
        if len == 0 || batch_size == 0 {
            return Vec::new();
        }

        let actual_size = batch_size.min(len);
        self.total_sampled
            .fetch_add(actual_size as u64, Ordering::Relaxed);

        // 使用 rand 的 index::sample 生成不重复索引
        let indices = sample(rng, len, actual_size);
        let mut result = Vec::with_capacity(actual_size);
        for idx in indices {
            if let Some(item) = buf.get(idx) {
                result.push(item.clone());
            }
        }
        result
    }

    /// 取出所有缓冲的轨迹样本并清空缓冲区
    ///
    /// # 用途
    /// 测试场景或批量迁移(如持久化前 drain 所有样本)。
    /// 生产训练一般用 `sample` 而非 `drain`(保留历史样本供后续训练)。
    ///
    /// # 错误处理
    /// Mutex poison 时返回空 Vec(不 panic)
    pub fn drain(&self) -> Vec<T> {
        self.buffer
            .lock()
            .map(|mut buf| buf.drain(..).collect())
            .unwrap_or_default()
    }

    /// 获取当前缓冲区的快照(不影响缓冲区)
    ///
    /// # 用途
    /// 测试断言与调试观测。生产消费请用 `sample` 或 `drain`。
    pub fn snapshot(&self) -> Vec<T> {
        self.buffer
            .lock()
            .map(|buf| buf.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 清空缓冲区(不重置计数器)
    ///
    /// # 用途
    /// 测试场景重置缓冲区但保留累计统计。
    pub fn clear(&self) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }
    }
}

impl<T> Default for ReplayPool<T>
where
    T: Clone + Send + Sync,
{
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 统计快照类型
// ============================================================

/// 回放池统计快照 — `ReplayPool` 的可观测视图
///
/// # 设计原则
/// - 值类型(Snapshot),可跨线程传递
/// - 一次性快照(非实时视图),避免锁持有
/// - 包含 buffer_len/capacity 便于容量监控
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayPoolStats {
    /// 累计推入总数(含已淘汰)
    pub total_pushed: u64,
    /// 累计采样总数
    pub total_sampled: u64,
    /// 因容量超限被淘汰的条目数
    pub evicted_count: u64,
    /// 当前缓冲区中的样本数(不含已淘汰)
    pub buffer_len: usize,
    /// 缓冲区容量上限
    pub capacity: usize,
}

impl ReplayPoolStats {
    /// 计算淘汰率(0.0-1.0)
    ///
    /// # 返回
    /// - `total_pushed = 0` 时返回 0.0(避免除零)
    /// - 否则返回 `evicted_count / total_pushed` 的浮点比值
    pub fn eviction_rate(&self) -> f64 {
        if self.total_pushed == 0 {
            0.0
        } else {
            self.evicted_count as f64 / self.total_pushed as f64
        }
    }

    /// 计算缓冲区使用率(0.0-1.0)
    ///
    /// # 返回
    /// - `capacity = 0` 时返回 0.0(避免除零,虽然构造时已规范化)
    /// - 否则返回 `buffer_len / capacity` 的浮点比值
    pub fn buffer_usage(&self) -> f64 {
        if self.capacity == 0 {
            0.0
        } else {
            self.buffer_len as f64 / self.capacity as f64
        }
    }

    /// 判断回放池是否已达到 spec 目标(≥10K 轨迹)
    ///
    /// # 返回
    /// - `true`:`total_pushed >= 10_000`(spec P4-W16.4.3 验收条件)
    /// - `false`:尚未达到目标
    pub fn meets_spec_target(&self) -> bool {
        self.total_pushed >= 10_000
    }
}

// ============================================================
// ReplaySample 统一样本类型(可选,供上层统一使用)
// ============================================================

/// 统一轨迹样本 — 供上层(L9/L10)将两种捕获点转换为统一格式后存入回放池
///
/// # 设计决策
/// - **自包含类型**:仅含原始字段(String/f32/u64 等),不依赖 quest-engine 或
///   model-router 的具体类型,避免 L6→L9 依赖违规(§2.2 依赖铁律)
/// - **可选使用**:上层可直接使用 `ReplayPool<ReplaySample>`,
///   也可使用 `ReplayPool<model_router::TrajectoryEvent>` 等具体类型
/// - **序列化支持**:派生 `Serialize + Deserialize`,便于持久化与跨进程传输
///
/// # 字段映射
/// | 字段 | 捕获点 1 (model-router) | 捕获点 2 (quest-engine) |
/// |------|------------------------|------------------------|
/// | source | Router | QuestCheckpoint |
/// | quest_id | TrajectoryEvent.quest_id | QuestTrajectory.state.quest_id |
/// | arm | strategy 名称 | thinking_mode 名称 |
/// | reward | outcome 成功=1.0/失败=0.0 | net_reward |
/// | context_hash | (可选)request hash | memory_snapshot_hash |
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplaySample {
    /// 轨迹来源(捕获点标识)
    pub source: TrajectorySource,
    /// 所属 Quest ID(跨模块追踪键)
    pub quest_id: String,
    /// 动作标识(arm 名称,如 "Lite"/"Standard"/"Deep")
    pub arm: String,
    /// 奖励信号 [-0.5, 1.0](来自 L3 执行反馈,spec §P4-W16.3.3 奖励护栏)
    pub reward: f32,
    /// 上下文哈希(可选,用于去重)
    ///
    /// - 捕获点 1:可设为 None 或 request hash
    /// - 捕获点 2:Checkpoint.memory_snapshot_hash
    pub context_hash: Option<String>,
    /// 时间戳(UTC,轨迹产生时间)
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 轨迹来源标识 — 区分捕获点 1 与捕获点 2
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrajectorySource {
    /// 捕获点 1:model-router 路由调用轨迹
    Router,
    /// 捕获点 2:quest-engine Checkpoint 状态轨迹
    QuestCheckpoint,
}

impl TrajectorySource {
    /// 获取来源简称(用于日志与 metrics 标签)
    pub fn short_name(&self) -> &'static str {
        match self {
            Self::Router => "router",
            Self::QuestCheckpoint => "quest-cp",
        }
    }
}

impl ReplaySample {
    /// 创建新的统一轨迹样本
    pub fn new(
        source: TrajectorySource,
        quest_id: impl Into<String>,
        arm: impl Into<String>,
        reward: f32,
    ) -> Self {
        Self {
            source,
            quest_id: quest_id.into(),
            arm: arm.into(),
            reward,
            context_hash: None,
            timestamp: chrono::Utc::now(),
        }
    }

    /// 设置上下文哈希(链式调用)
    pub fn with_context_hash(mut self, hash: impl Into<String>) -> Self {
        self.context_hash = Some(hash.into());
        self
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;

    // ============================================================
    // 基础功能测试
    // ============================================================

    #[test]
    fn test_replay_pool_default_capacity() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        assert_eq!(pool.capacity(), 10_000, "默认容量必须为 10_000");
        assert_eq!(pool.total_pushed(), 0);
        assert_eq!(pool.total_sampled(), 0);
        assert_eq!(pool.evicted_count(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn test_replay_pool_with_custom_capacity() {
        let pool: ReplayPool<i32> = ReplayPool::with_capacity(500);
        assert_eq!(pool.capacity(), 500);
    }

    #[test]
    fn test_replay_pool_zero_capacity_normalizes_to_one() {
        let pool: ReplayPool<i32> = ReplayPool::with_capacity(0);
        assert_eq!(pool.capacity(), 1, "容量 0 必须规范化为 1");
    }

    #[test]
    fn test_push_single_item() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        pool.push(42);
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.total_pushed(), 1);
        assert_eq!(pool.evicted_count(), 0);
    }

    #[test]
    fn test_push_multiple_items() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        for i in 0..100 {
            pool.push(i);
        }
        assert_eq!(pool.len(), 100);
        assert_eq!(pool.total_pushed(), 100);
    }

    #[test]
    fn test_extend_batch_push() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        pool.extend(vec![1, 2, 3, 4, 5]);
        assert_eq!(pool.len(), 5);
        assert_eq!(pool.total_pushed(), 5);
    }

    // ============================================================
    // FIFO 淘汰测试
    // ============================================================

    #[test]
    fn test_fifo_eviction_when_full() {
        let pool: ReplayPool<i32> = ReplayPool::with_capacity(3);

        // 写入 3 个事件(满)
        pool.push(1);
        pool.push(2);
        pool.push(3);
        assert_eq!(pool.len(), 3);
        assert_eq!(pool.evicted_count(), 0);

        // 写入第 4 个 — 淘汰 1
        pool.push(4);
        assert_eq!(pool.len(), 3, "容量满后 push 仍保持 3 条");
        assert_eq!(pool.total_pushed(), 4, "total_pushed 累计 4");
        assert_eq!(pool.evicted_count(), 1, "应淘汰 1 条");

        // 验证 FIFO — 1 已被淘汰,buffer 中是 2/3/4
        let snapshot = pool.snapshot();
        assert_eq!(snapshot, vec![2, 3, 4]);
    }

    #[test]
    fn test_extend_with_eviction() {
        let pool: ReplayPool<i32> = ReplayPool::with_capacity(5);
        pool.extend(vec![1, 2, 3, 4, 5, 6, 7]); // 7 条,容量 5
        assert_eq!(pool.len(), 5);
        assert_eq!(pool.total_pushed(), 7);
        assert_eq!(pool.evicted_count(), 2, "应淘汰 2 条");

        // FIFO:1/2 被淘汰,buffer 中是 3/4/5/6/7
        let snapshot = pool.snapshot();
        assert_eq!(snapshot, vec![3, 4, 5, 6, 7]);
    }

    // ============================================================
    // 采样测试
    // ============================================================

    #[test]
    fn test_sample_empty_pool_returns_empty() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        let mut rng = thread_rng();
        let result = pool.sample(10, &mut rng);
        assert!(result.is_empty(), "空池采样应返回空 Vec");
    }

    #[test]
    fn test_sample_zero_batch_size_returns_empty() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        pool.push(1);
        let mut rng = thread_rng();
        let result = pool.sample(0, &mut rng);
        assert!(result.is_empty(), "batch_size=0 应返回空 Vec");
    }

    #[test]
    fn test_sample_with_replacement() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        for i in 0..100 {
            pool.push(i);
        }

        let mut rng = thread_rng();
        let batch = pool.sample(32, &mut rng);
        assert_eq!(batch.len(), 32, "应采样 32 条");
        assert_eq!(pool.total_sampled(), 32);

        // 验证所有样本都在 [0, 100) 范围内
        for &item in &batch {
            assert!(item < 100, "样本应在 [0, 100) 范围内,实际: {}", item);
        }
    }

    #[test]
    fn test_sample_more_than_available() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        pool.push(1);
        pool.push(2);
        pool.push(3);

        let mut rng = thread_rng();
        // 请求 10 条,但池中只有 3 条 → 返回 3 条
        let batch = pool.sample(10, &mut rng);
        assert_eq!(batch.len(), 3, "应返回池中所有 3 条");
    }

    #[test]
    fn test_sample_without_replacement() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        for i in 0..100 {
            pool.push(i);
        }

        let mut rng = thread_rng();
        let batch = pool.sample_without_replacement(32, &mut rng);
        assert_eq!(batch.len(), 32);

        // 验证无重复(无放回采样)
        let mut sorted = batch.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 32, "无放回采样不应有重复");
    }

    #[test]
    fn test_sample_does_not_drain_pool() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        for i in 0..50 {
            pool.push(i);
        }

        let mut rng = thread_rng();
        let _batch = pool.sample(10, &mut rng);

        // 采样不应影响缓冲区
        assert_eq!(pool.len(), 50, "采样不应清空缓冲区");
    }

    // ============================================================
    // drain / snapshot / clear 测试
    // ============================================================

    #[test]
    fn test_drain() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        pool.push(1);
        pool.push(2);
        pool.push(3);

        let items = pool.drain();
        assert_eq!(items, vec![1, 2, 3]);

        // drain 后缓冲区清空,但计数器保留
        assert!(pool.is_empty());
        assert_eq!(pool.total_pushed(), 3, "计数器不重置");
    }

    #[test]
    fn test_drain_empty_pool() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        let items = pool.drain();
        assert!(items.is_empty());
    }

    #[test]
    fn test_snapshot_does_not_mutate() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        pool.push(1);
        pool.push(2);

        let snap1 = pool.snapshot();
        assert_eq!(snap1.len(), 2);

        // snapshot 不影响缓冲区
        assert_eq!(pool.len(), 2);

        let snap2 = pool.snapshot();
        assert_eq!(snap2.len(), 2);
    }

    #[test]
    fn test_clear() {
        let pool: ReplayPool<i32> = ReplayPool::new();
        pool.push(1);
        pool.push(2);

        pool.clear();
        assert!(pool.is_empty());
        // 计数器不重置
        assert_eq!(pool.total_pushed(), 2, "计数器不应被 clear 影响");
    }

    // ============================================================
    // 统计测试
    // ============================================================

    #[test]
    fn test_stats_snapshot() {
        let pool: ReplayPool<i32> = ReplayPool::with_capacity(100);
        for i in 0..50 {
            pool.push(i);
        }

        let mut rng = thread_rng();
        let _ = pool.sample(10, &mut rng);

        let stats = pool.stats();
        assert_eq!(stats.total_pushed, 50);
        assert_eq!(stats.total_sampled, 10);
        assert_eq!(stats.evicted_count, 0);
        assert_eq!(stats.buffer_len, 50);
        assert_eq!(stats.capacity, 100);
    }

    #[test]
    fn test_stats_eviction_rate() {
        let stats = ReplayPoolStats {
            total_pushed: 100,
            total_sampled: 0,
            evicted_count: 10,
            buffer_len: 90,
            capacity: 100,
        };
        // 淘汰率 = 10/100 = 0.1
        assert!((stats.eviction_rate() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn test_stats_eviction_rate_zero_total() {
        let stats = ReplayPoolStats {
            total_pushed: 0,
            total_sampled: 0,
            evicted_count: 0,
            buffer_len: 0,
            capacity: 100,
        };
        assert_eq!(stats.eviction_rate(), 0.0);
    }

    #[test]
    fn test_stats_buffer_usage() {
        let stats = ReplayPoolStats {
            total_pushed: 50,
            total_sampled: 0,
            evicted_count: 0,
            buffer_len: 50,
            capacity: 100,
        };
        // 使用率 = 50/100 = 0.5
        assert!((stats.buffer_usage() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_stats_meets_spec_target() {
        // 未达 10K
        let stats = ReplayPoolStats {
            total_pushed: 9_999,
            total_sampled: 0,
            evicted_count: 0,
            buffer_len: 9_999,
            capacity: 10_000,
        };
        assert!(!stats.meets_spec_target(), "9999 不应满足 ≥10K 目标");

        // 达到 10K
        let stats = ReplayPoolStats {
            total_pushed: 10_000,
            total_sampled: 0,
            evicted_count: 0,
            buffer_len: 10_000,
            capacity: 10_000,
        };
        assert!(stats.meets_spec_target(), "10000 应满足 ≥10K 目标");
    }

    // ============================================================
    // 并发测试
    // ============================================================

    #[test]
    fn test_concurrent_push() {
        use std::sync::Arc;
        use std::thread;

        let pool = Arc::new(ReplayPool::<i32>::with_capacity(1000));
        let mut handles = vec![];

        // 10 个线程,每个写入 100 条
        for t in 0..10 {
            let pool_clone = Arc::clone(&pool);
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    pool_clone.push(t * 100 + i);
                }
            }));
        }

        for handle in handles {
            handle.join().expect("线程不应 panic");
        }

        // 总推入 = 10 * 100 = 1000
        assert_eq!(pool.total_pushed(), 1000);
        assert_eq!(pool.len(), 1000, "缓冲区应容纳全部 1000 条");
        assert_eq!(pool.evicted_count(), 0);
    }

    #[test]
    fn test_concurrent_push_with_eviction() {
        use std::sync::Arc;
        use std::thread;

        // 容量 100,写入 1000 条,应淘汰 900
        let pool = Arc::new(ReplayPool::<i32>::with_capacity(100));
        let mut handles = vec![];

        for t in 0..10 {
            let pool_clone = Arc::clone(&pool);
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    pool_clone.push(t * 100 + i);
                }
            }));
        }

        for handle in handles {
            handle.join().expect("线程不应 panic");
        }

        assert_eq!(pool.total_pushed(), 1000);
        assert_eq!(pool.len(), 100, "缓冲区保持容量上限 100");
        assert_eq!(pool.evicted_count(), 900, "应淘汰 900 条");
    }

    #[test]
    fn test_concurrent_push_and_sample() {
        use std::sync::Arc;
        use std::thread;

        let pool = Arc::new(ReplayPool::<i32>::with_capacity(500));

        // 预填充 50 条,确保消费者首次采样能拿到样本(避免竞态导致 sampled=0)
        for i in 0..50 {
            pool.push(i);
        }

        // 生产者线程:写入剩余 450 条
        let producer_pool = Arc::clone(&pool);
        let producer = thread::spawn(move || {
            for i in 50..500 {
                producer_pool.push(i);
            }
        });

        // 消费者线程:采样
        let consumer_pool = Arc::clone(&pool);
        let consumer = thread::spawn(move || {
            let mut rng = thread_rng();
            let mut total_sampled = 0;
            for _ in 0..10 {
                let batch = consumer_pool.sample(10, &mut rng);
                total_sampled += batch.len();
                // 短暂等待,让生产者有机会写入
                std::thread::yield_now();
            }
            total_sampled
        });

        producer.join().expect("生产者不应 panic");
        let sampled = consumer.join().expect("消费者不应 panic");

        // 验证最终状态
        assert_eq!(pool.total_pushed(), 500);
        assert!(sampled > 0, "应至少采样到一些样本");
    }

    // ============================================================
    // Send + Sync 静态断言
    // ============================================================

    #[test]
    fn test_replay_pool_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ReplayPool<i32>>();
        assert_send_sync::<ReplayPool<ReplaySample>>();
        assert_send_sync::<ReplayPoolStats>();
        assert_send_sync::<std::sync::Arc<ReplayPool<i32>>>();
    }

    // ============================================================
    // ReplaySample 测试
    // ============================================================

    #[test]
    fn test_replay_sample_new() {
        let sample = ReplaySample::new(TrajectorySource::Router, "q-1", "Lite", 1.0);
        assert_eq!(sample.source, TrajectorySource::Router);
        assert_eq!(sample.quest_id, "q-1");
        assert_eq!(sample.arm, "Lite");
        assert!((sample.reward - 1.0).abs() < f32::EPSILON);
        assert!(sample.context_hash.is_none());
        assert!(sample.timestamp <= chrono::Utc::now());
    }

    #[test]
    fn test_replay_sample_with_context_hash() {
        let sample = ReplaySample::new(TrajectorySource::QuestCheckpoint, "q-1", "Deep", 0.375)
            .with_context_hash("abc123");
        assert_eq!(sample.context_hash, Some("abc123".into()));
    }

    #[test]
    fn test_trajectory_source_short_name() {
        assert_eq!(TrajectorySource::Router.short_name(), "router");
        assert_eq!(TrajectorySource::QuestCheckpoint.short_name(), "quest-cp");
    }

    #[test]
    fn test_replay_sample_serde_roundtrip() {
        let sample = ReplaySample::new(
            TrajectorySource::QuestCheckpoint,
            "q-serde",
            "Standard",
            0.5,
        )
        .with_context_hash("hash123");

        let json = serde_json::to_string(&sample).expect("序列化必须成功");
        let de: ReplaySample = serde_json::from_str(&json).expect("反序列化必须成功");
        assert_eq!(de, sample);
    }

    // ============================================================
    // 10K 容量压力测试(spec 验收条件 P4-W16.4.3)
    // ============================================================

    #[test]
    fn test_pool_accumulates_10k_trajectories() {
        // spec P4-W16.4.3: ≥10K 轨迹累计
        let pool: ReplayPool<i32> = ReplayPool::new();

        // 推入 10_000 条
        for i in 0..10_000 {
            pool.push(i);
        }

        let stats = pool.stats();
        assert_eq!(stats.total_pushed, 10_000);
        assert_eq!(stats.buffer_len, 10_000);
        assert_eq!(stats.evicted_count, 0);
        assert!(stats.meets_spec_target(), "应满足 spec ≥10K 轨迹目标");

        // 验证可从 10K 池中采样
        let mut rng = thread_rng();
        let batch = pool.sample(64, &mut rng);
        assert_eq!(batch.len(), 64, "应能从 10K 池中采样 64 条");
    }

    #[test]
    fn test_pool_exceeds_10k_with_eviction() {
        // 推入 15K 条,容量 10K,应淘汰 5K
        let pool: ReplayPool<i32> = ReplayPool::new();

        for i in 0..15_000 {
            pool.push(i);
        }

        let stats = pool.stats();
        assert_eq!(stats.total_pushed, 15_000);
        assert_eq!(stats.buffer_len, 10_000, "缓冲区保持容量上限");
        assert_eq!(stats.evicted_count, 5_000, "应淘汰 5K 条");
        assert!(stats.meets_spec_target(), "累计推入仍满足 ≥10K");

        // 淘汰率 = 5000/15000 ≈ 0.333
        let eviction_rate = stats.eviction_rate();
        assert!(
            (eviction_rate - 0.3333).abs() < 0.01,
            "淘汰率应约为 0.333,实际: {}",
            eviction_rate
        );
    }
}
