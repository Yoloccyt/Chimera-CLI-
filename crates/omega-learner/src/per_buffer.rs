//! PER 优先经验回放缓冲区 — SumTree O(log n) 优先级采样(polish-v2.7 P4-1)
//!
//! 对应架构层:L6 Router(omega-learner)
//! 对应 ADR:ADR-049 决策 4(PER 落点 omega-learner,否决入 event-bus;
//! SumTree 替代方案文档的 Vec 线性实现)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §5.1(RUC 采样策略)+
//! Schaul et al. 2016 "Prioritized Experience Replay"(ICLR)
//!
//! # R2 冻结声明(ADR-042)
//!
//! **冻结状态**:R2(GSOE×AutoDPO 约束 RL)路径在 FormalVerifier 落地前无条件冻结。
//! 本缓冲区的经验数据**仅可用于 R1 路径**(召回配额 CQL/IQL + 影子模式 2 周),
//! 禁止用于 R2 路径。与 `replay_pool.rs`(均匀采样)同款约束。
//!
//! # WHY SumTree 而非方案文档的 Vec 实现(ADR-049 决策 4)
//!
//! | 操作 | 方案 Vec 实现 | SumTree |
//! |---|---|---|
//! | 添加(满时淘汰) | O(n) 全扫找最小 + O(n) remove 搬移 | O(log n) 环形覆写 |
//! | 采样单条 | O(n) 前缀和扫描 | O(log n) 树下降 |
//! | 更新优先级 | O(1) 但需外部索引 | O(log n) 沿路径修正 |
//!
//! 10 万规模下 Vec 实现的采样为毫秒级,SumTree 保持微秒级
//! (验收门禁:100K 规模 batch 采样 p99 < 100µs,基线见
//! `docs/performance/polish_v2.7_phase0_baseline.md`)。
//!
//! # 数据结构
//!
//! 满二叉树的数组表示:`tree[0]` 为根(总优先级和),
//! 叶子区间 `[capacity-1, 2*capacity-1)` 存各槽位优先级,
//! 内部节点存左右子树优先级之和。采样时按 `[0, total)` 均匀取前缀值
//! 沿树下降定位叶子(比例采样,Schaul et al. §B.2.1 proportional variant)。

use rand::Rng;
use std::sync::Mutex;

/// 优先级指数 α — 0 = 均匀采样,1 = 完全按优先级(Schaul et al. 推荐 0.6)
const PRIORITY_ALPHA: f32 = 0.6;

/// 重要性采样指数 β 初始值(训练中退火至 1.0)
const IS_BETA_INITIAL: f32 = 0.4;

/// β 每次采样的退火步长
const IS_BETA_INCREMENT: f32 = 0.001;

/// 优先级下界 ε — 防止零优先级样本永不被采样
const PRIORITY_EPSILON: f32 = 1e-6;

/// SumTree — 前缀和满二叉树(内部结构,不单独暴露)
///
/// WHY 数组而非指针树:容量固定,数组表示零分配、cache 友好,
/// 父子索引算术推导(parent = (i-1)/2)。
struct SumTree {
    /// 树节点(长度 2*capacity - 1;前 capacity-1 为内部节点,其后为叶子)
    nodes: Vec<f32>,
    /// 叶子容量
    capacity: usize,
}

impl SumTree {
    fn new(capacity: usize) -> Self {
        Self {
            nodes: vec![0.0; 2 * capacity - 1],
            capacity,
        }
    }

    /// 总优先级和(根节点)
    fn total(&self) -> f32 {
        self.nodes[0]
    }

    /// 读取槽位优先级
    fn priority_of(&self, slot: usize) -> f32 {
        self.nodes[self.capacity - 1 + slot]
    }

    /// 更新槽位优先级并沿路径修正祖先和 — O(log n)
    fn update(&mut self, slot: usize, priority: f32) {
        let mut idx = self.capacity - 1 + slot;
        let delta = priority - self.nodes[idx];
        self.nodes[idx] = priority;
        while idx > 0 {
            idx = (idx - 1) / 2;
            self.nodes[idx] += delta;
        }
    }

    /// 按前缀值下降定位叶子槽位 — O(log n)
    ///
    /// `prefix ∈ [0, total)`:左子树和 ≥ prefix 走左,否则减去左和走右。
    fn find_slot(&self, mut prefix: f32) -> usize {
        let mut idx = 0usize;
        while idx < self.capacity - 1 {
            let left = 2 * idx + 1;
            if self.nodes[left] >= prefix {
                idx = left;
            } else {
                prefix -= self.nodes[left];
                idx = left + 1; // 右子
            }
        }
        idx - (self.capacity - 1)
    }
}

/// 单次采样结果 — 样本 + 槽位(供优先级回写)+ IS 权重
#[derive(Debug, Clone)]
pub struct PerSample<T> {
    /// 采样出的经验副本
    pub item: T,
    /// 样本所在槽位(训练后 `update_priorities` 回写用)
    pub slot: usize,
    /// 重要性采样权重(修正优先级采样引入的分布偏差,Schaul et al. §3.4)
    pub is_weight: f32,
}

/// PER 缓冲区内部状态 — 单锁保护(锁内无 await,§4.4 红线 1)
struct PerState<T> {
    tree: SumTree,
    /// 槽位数据(环形覆写)
    items: Vec<Option<T>>,
    /// 下一个写入槽位(环形指针)
    write_cursor: usize,
    /// 当前有效样本数(≤ capacity)
    len: usize,
    /// 当前 β(随采样退火至 1.0)
    beta: f32,
    /// 历史最大优先级(新样本默认取此值,保证至少被采样一次)
    max_priority: f32,
}

/// PER 优先经验回放缓冲区 — SumTree 比例采样 + IS 权重
///
/// # 线程安全
/// 单 `Mutex` 保护全部状态(树 + 数据 + 游标必须原子更新,分锁会撕裂)。
/// 锁内均为纯内存操作(无 I/O 无 await),临界区微秒级。
///
/// # 使用示例
/// ```
/// use omega_learner::per_buffer::PerBuffer;
/// use rand::rngs::StdRng;
/// use rand::SeedableRng;
///
/// let buffer: PerBuffer<u32> = PerBuffer::with_capacity(1024);
/// // TD 误差作为初始优先级
/// buffer.push(42, 1.5);
/// buffer.push(43, 0.2);
///
/// let mut rng = StdRng::seed_from_u64(7);
/// let batch = buffer.sample(2, &mut rng);
/// assert_eq!(batch.len(), 2);
/// // 训练后按新 TD 误差回写优先级
/// let updates: Vec<(usize, f32)> = batch.iter().map(|s| (s.slot, 0.5)).collect();
/// buffer.update_priorities(&updates);
/// ```
pub struct PerBuffer<T> {
    state: Mutex<PerState<T>>,
    capacity: usize,
}

impl<T: Clone> PerBuffer<T> {
    /// 创建指定容量的 PER 缓冲区(容量 0 视为 1,防边界)
    pub fn with_capacity(capacity: usize) -> Self {
        let normalized = capacity.max(1);
        Self {
            state: Mutex::new(PerState {
                tree: SumTree::new(normalized),
                items: (0..normalized).map(|_| None).collect(),
                write_cursor: 0,
                len: 0,
                beta: IS_BETA_INITIAL,
                max_priority: 1.0,
            }),
            capacity: normalized,
        }
    }

    /// 缓冲区容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 当前有效样本数
    pub fn len(&self) -> usize {
        self.state.lock().map(|s| s.len).unwrap_or(0)
    }

    /// 缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 推入经验(TD 误差作初始优先级)— O(log n)
    ///
    /// # 淘汰语义
    /// 环形覆写:满时覆盖最旧槽位(FIFO 时序淘汰)。
    /// WHY 不是方案文档的"淘汰最低优先级":最低优先级淘汰会让旧的高优先级
    /// 样本永久驻留(时序偏置),FIFO 保证时序多样性(与 replay_pool.rs 一致)。
    pub fn push(&self, item: T, td_error: f32) {
        let priority = priority_from_td(td_error);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // 新样本用 max(历史最大, 本次) 优先级,保证至少被采样一次后再由训练回写
        let effective = priority.max(state.max_priority);
        state.max_priority = state.max_priority.max(priority);

        let slot = state.write_cursor;
        state.items[slot] = Some(item);
        state.tree.update(slot, effective);
        state.write_cursor = (slot + 1) % self.capacity;
        state.len = (state.len + 1).min(self.capacity);
    }

    /// 比例优先级采样 batch(有放回)— O(batch · log n)
    ///
    /// 返回样本 + 槽位 + IS 权重;空缓冲区返回空 Vec。
    /// 每次调用后 β 退火 +0.001(封顶 1.0)。
    pub fn sample<R: Rng>(&self, batch_size: usize, rng: &mut R) -> Vec<PerSample<T>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.len == 0 || batch_size == 0 {
            return Vec::new();
        }

        let total = state.tree.total();
        if total <= 0.0 {
            return Vec::new();
        }

        let n = state.len as f32;
        let beta = state.beta;
        let mut samples = Vec::with_capacity(batch_size);
        // IS 权重按 batch 内最大值归一化(Schaul et al. §3.4,稳定梯度尺度)
        let mut max_weight = f32::MIN_POSITIVE;

        for _ in 0..batch_size {
            let prefix = rng.gen_range(0.0..total);
            let slot = state.tree.find_slot(prefix);
            // 环形覆写下槽位必有数据(find_slot 只会命中优先级 >0 的叶子)
            let Some(item) = state.items[slot].clone() else {
                continue;
            };
            let prob = state.tree.priority_of(slot) / total;
            // w_i = (N · P(i))^(-β)
            let weight = (n * prob).powf(-beta);
            max_weight = max_weight.max(weight);
            samples.push(PerSample {
                item,
                slot,
                is_weight: weight,
            });
        }

        // 归一化 IS 权重至 (0, 1]
        for s in &mut samples {
            s.is_weight /= max_weight;
        }

        state.beta = (beta + IS_BETA_INCREMENT).min(1.0);
        samples
    }

    /// 训练后批量回写优先级(新 TD 误差)— O(k · log n)
    pub fn update_priorities(&self, updates: &[(usize, f32)]) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for &(slot, td_error) in updates {
            if slot < self.capacity && state.items[slot].is_some() {
                let priority = priority_from_td(td_error);
                state.tree.update(slot, priority);
                state.max_priority = state.max_priority.max(priority);
            }
        }
    }
}

/// TD 误差 → 优先级:|δ|^α + ε(Schaul et al. proportional variant)
fn priority_from_td(td_error: f32) -> f32 {
    td_error.abs().powf(PRIORITY_ALPHA) + PRIORITY_EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn test_sumtree_prefix_descent() {
        let mut tree = SumTree::new(4);
        tree.update(0, 1.0);
        tree.update(1, 2.0);
        tree.update(2, 3.0);
        tree.update(3, 4.0);
        assert!((tree.total() - 10.0).abs() < 1e-6);
        // 前缀区间:slot0=[0,1) slot1=[1,3) slot2=[3,6) slot3=[6,10)
        assert_eq!(tree.find_slot(0.5), 0);
        assert_eq!(tree.find_slot(2.9), 1);
        assert_eq!(tree.find_slot(5.9), 2);
        assert_eq!(tree.find_slot(9.9), 3);
    }

    #[test]
    fn test_push_and_ring_eviction() {
        let buffer: PerBuffer<u32> = PerBuffer::with_capacity(2);
        buffer.push(1, 1.0);
        buffer.push(2, 1.0);
        assert_eq!(buffer.len(), 2);
        // 环形覆写:第 3 条覆盖最旧槽位,len 不超容量
        buffer.push(3, 1.0);
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn test_sample_returns_weights_normalized() {
        let buffer: PerBuffer<u32> = PerBuffer::with_capacity(16);
        for i in 0..16 {
            buffer.push(i, (i as f32) * 0.5 + 0.1);
        }
        let mut rng = StdRng::seed_from_u64(42);
        let batch = buffer.sample(8, &mut rng);
        assert_eq!(batch.len(), 8);
        // IS 权重归一化至 (0, 1]
        for s in &batch {
            assert!(s.is_weight > 0.0 && s.is_weight <= 1.0 + 1e-6);
        }
        assert!(batch.iter().any(|s| (s.is_weight - 1.0).abs() < 1e-6));
    }

    #[test]
    fn test_high_priority_sampled_more_frequently() {
        let buffer: PerBuffer<&'static str> = PerBuffer::with_capacity(2);
        buffer.push("low", 0.01);
        buffer.push("high", 10.0);
        // 拉高 high 的优先级差距后统计频率
        buffer.update_priorities(&[(0, 0.01), (1, 10.0)]);

        let mut rng = StdRng::seed_from_u64(7);
        let mut high_count = 0;
        for _ in 0..100 {
            for s in buffer.sample(1, &mut rng) {
                if s.item == "high" {
                    high_count += 1;
                }
            }
        }
        // |10|^0.6 ≈ 3.98 vs |0.01|^0.6 ≈ 0.063:high 应占绝对多数
        assert!(high_count > 80, "高优先级采样占比 {high_count}/100 过低");
    }

    #[test]
    fn test_update_priorities_shifts_distribution() {
        let buffer: PerBuffer<u32> = PerBuffer::with_capacity(2);
        buffer.push(0, 5.0);
        buffer.push(1, 5.0);
        // 训练后把 slot0 优先级压到近零 → 采样应几乎全命中 slot1
        buffer.update_priorities(&[(0, 0.0001)]);

        let mut rng = StdRng::seed_from_u64(9);
        let hits_slot1 = (0..50)
            .flat_map(|_| buffer.sample(1, &mut rng))
            .filter(|s| s.slot == 1)
            .count();
        assert!(hits_slot1 > 45, "优先级回写后分布未偏移: {hits_slot1}/50");
    }

    #[test]
    fn test_empty_buffer_sample_is_empty() {
        let buffer: PerBuffer<u32> = PerBuffer::with_capacity(8);
        let mut rng = StdRng::seed_from_u64(1);
        assert!(buffer.sample(4, &mut rng).is_empty());
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_beta_anneals_toward_one() {
        let buffer: PerBuffer<u32> = PerBuffer::with_capacity(4);
        buffer.push(1, 1.0);
        let mut rng = StdRng::seed_from_u64(3);
        for _ in 0..10 {
            let _ = buffer.sample(1, &mut rng);
        }
        let beta = buffer.state.lock().unwrap().beta;
        assert!((beta - (IS_BETA_INITIAL + 10.0 * IS_BETA_INCREMENT)).abs() < 1e-6);
    }
}
