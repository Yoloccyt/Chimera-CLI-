//! Segment-aware PER — 轨迹分段优先级经验回放（设计文档 §6.2）
//!
//! 对应架构层: **L1 Core**（event-bus 内部扩展）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §6.2
//! 对应论文: 微软 OpenForge/Dressage（Segment-aware 训练）
//!
//! # 核心职责
//!
//! 轨迹分段优先级经验回放（PER）：
//! - **铁律9 分段身份**: 同一父轨迹的全部分段共享 `parent_traj_id`；
//!   **仅 anchor segment 承载终局 reward**（`anchor_rewards` 登记）
//! - **prompt-equal denominator**: `td_error / sqrt(segment_count)` 折减，
//!   避免分段数量影响梯度（Dressage 32K 失败/64K 成功的实证教训）
//! - **reward 广播**: 非 anchor 段的终局 reward 通过 `broadcast_reward` 注入，
//!   由 anchor 段登记后广播至同轨迹全部分段
//! - **权重采样**: 按 td_error 权重轮盘赌采样（有放回），高 td_error 经验
//!   优先回放（PER 核心）
//!
//! # 设计约束
//!
//! - **L0 类型复用**: 直接使用 `nexus_contracts::rl_types::RLExperience` 与
//!   `nexus_contracts::token_evidence::SegmentMetadata`（L1→L0 合规），
//!   不修改 L0 已冻结线格式（向后兼容红线）
//! - **无持锁跨 await**: DashMap 索引同步短临界区；采样为纯同步
//! - **零新依赖**: 伪随机用内部 xorshift64（可种子复现，测试确定性强）
//! - **淘汰策略**: 超容量淘汰最低 td_error 条目（`select_nth_unstable_by`，
//!   红线 R8，O(n)）

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use nexus_contracts::rl_types::RLExperience;
use nexus_contracts::token_evidence::SegmentMetadata;

// ============================================================
// 优先级回放缓冲（D-4: 自建，零新依赖）
// ============================================================

/// 回放条目 — 经验 + 分段 + TD 误差的复合载荷
#[derive(Debug, Clone, PartialEq)]
pub struct PerEntry {
    /// RL 经验四元组（L0 类型，原样承载）
    pub experience: RLExperience,
    /// 所属分段元数据（铁律9: parent_traj_id 共享身份）
    pub segment: SegmentMetadata,
    /// TD 误差（回放优先级权重；负值按 0 处理——只关心误差幅度）
    pub td_error: f32,
}

/// 优先级经验回放缓冲 — 按 TD 误差权重采样
///
/// 内部 xorshift64 伪随机（无 rand 依赖），种子可注入保证测试确定性。
/// `Clone` 派生（entries 深拷贝 + rng 状态复制，SegmentAwarePER 共享语义）。
#[derive(Debug, Clone)]
pub struct PerBuffer {
    /// 回放条目（无序存储，采样时轮盘赌）
    entries: Vec<PerEntry>,
    /// 容量上限（超限淘汰最低 TD 误差）
    capacity: usize,
    /// xorshift64 状态（Relaxed 即可：采样允许近似随机）
    /// Arc: AtomicU64 非 Clone，Arc 支持 Clone 派生共享
    rng_state: Arc<AtomicU64>,
}

impl PerBuffer {
    /// 创建缓冲
    ///
    /// - `capacity`: 条目上限，0 表示无上限
    /// - `seed`: 随机种子（测试传固定值保证可复现；生产传时间种子）
    pub fn new(capacity: usize, seed: u64) -> Self {
        Self {
            entries: Vec::new(),
            capacity,
            rng_state: Arc::new(AtomicU64::new(seed)),
        }
    }

    /// 追加条目（超容量时淘汰最低 TD 误差）
    pub fn add(&mut self, entry: PerEntry) {
        self.entries.push(entry);
        if self.capacity > 0 && self.entries.len() > self.capacity {
            // O(n) 找到最低 TD 误差条目并移除（红线 R8: 禁 sort_by O(n log n)）
            let min_idx = self
                .entries
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    a.td_error
                        .partial_cmp(&b.td_error)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(idx, _)| idx)
                .expect("entries 非空（len > capacity > 0）");
            self.entries.swap_remove(min_idx);
        }
    }

    /// 按 TD 误差权重轮盘赌采样（有放回）
    ///
    /// 返回最多 `batch_size` 条（不足时全量返回）；权重为 `td_error.max(0.0)`，
    /// 全零权重时返回空（无可采样经验）。
    pub fn sample_batch(&self, batch_size: usize) -> Vec<PerEntry> {
        if self.entries.is_empty() || batch_size == 0 {
            return Vec::new();
        }
        let total_weight: f64 = self
            .entries
            .iter()
            .map(|e| e.td_error.max(0.0) as f64)
            .sum();
        if total_weight <= 0.0 {
            return Vec::new();
        }
        let mut state = self.rng_state.load(Ordering::Relaxed);
        let mut samples = Vec::with_capacity(batch_size.min(self.entries.len()));
        for _ in 0..batch_size {
            // 轮盘赌: 随机落点 ∈ [0, total_weight)，按累积权重选择
            let mut r = (next_xorshift(&mut state) % 1_000_000) as f64 / 1_000_000.0 * total_weight;
            for e in &self.entries {
                r -= e.td_error.max(0.0) as f64;
                if r <= 0.0 {
                    samples.push(e.clone());
                    break;
                }
            }
        }
        self.rng_state.store(state, Ordering::Relaxed);
        samples
    }

    /// 条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 容量上限（0 = 无上限）
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// xorshift64 伪随机 — 零依赖确定性强随机源
///
/// WHY 自实现: 避免引入 rand 依赖（本模块仅需采样随机）；
/// 种子可注入使测试分布断言确定可复现。
fn next_xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

// ============================================================
// Segment-aware PER
// ============================================================

/// Segment-aware PER 统计
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerStats {
    /// 缓冲条目数
    pub buffer_len: usize,
    /// 轨迹数（唯一 parent_traj_id）
    pub trajectory_count: usize,
    /// anchor 段登记数
    pub anchor_count: usize,
}

/// Segment-aware PER — 轨迹分段经验回放主控（铁律9）
///
/// `Clone` 派生（DashMap Arc 语义），所有副本共享注册表与奖励表。
#[derive(Debug, Clone)]
pub struct SegmentAwarePER {
    /// 优先级回放缓冲（含 prompt-equal 折减后的 TD 误差）
    per_buffer: PerBuffer,
    /// 轨迹分段注册表: parent_traj_id → segments（铁律9 共享身份）
    /// Arc: DashMap 深拷贝语义下 Clone 副本必须共享注册表
    segment_registry: Arc<DashMap<String, Vec<SegmentMetadata>>>,
    /// anchor 终局奖励: parent_traj_id → reward（仅 anchor 段承载）
    anchor_rewards: Arc<DashMap<String, f32>>,
}

impl SegmentAwarePER {
    /// 创建 Segment-aware PER
    ///
    /// - `capacity`: 回放缓冲容量（0 = 无上限）
    /// - `seed`: 采样随机种子
    pub fn new(capacity: usize, seed: u64) -> Self {
        Self {
            per_buffer: PerBuffer::new(capacity, seed),
            segment_registry: Arc::new(DashMap::new()),
            anchor_rewards: Arc::new(DashMap::new()),
        }
    }

    /// 登记分段经验（铁律9 + prompt-equal denominator）
    ///
    /// 1. anchor 段：登记终局 reward（`anchor_rewards[parent_traj_id] = reward`）
    /// 2. 分段注册：按 parent_traj_id 分组登记（共享身份）
    /// 3. 折减：`td_error / sqrt(segment_count)` 后入缓冲
    ///    （Dressage: 避免 segment 数量影响梯度）
    pub fn add_segment(
        &mut self,
        experience: RLExperience,
        segment: SegmentMetadata,
        td_error: f32,
    ) {
        if segment.is_anchor {
            // 铁律9: anchor segment 承载终局 reward
            self.anchor_rewards
                .insert(segment.parent_traj_id.to_string(), experience.reward);
        }
        // 分段登记（先登记后计数，count 含当前段）
        self.segment_registry
            .entry(segment.parent_traj_id.to_string())
            .or_default()
            .push(segment.clone());
        // prompt-equal denominator: 段数越多，单段梯度权重越低
        let segment_count = self
            .segment_registry
            .get(segment.parent_traj_id.as_ref())
            .map(|v| v.len() as f32)
            .unwrap_or(1.0)
            .max(1.0);
        let adjusted_td_error = td_error / segment_count.sqrt();
        self.per_buffer.add(PerEntry {
            experience,
            segment,
            td_error: adjusted_td_error,
        });
    }

    /// 广播终局 reward 至轨迹（铁律9: 由 anchor 承载后广播）
    ///
    /// 供非 anchor 段回放时获取同轨迹终局信号；覆盖写入（后到者胜）。
    pub fn broadcast_reward(&self, parent_traj_id: &str, reward: f32) {
        self.anchor_rewards
            .insert(parent_traj_id.to_string(), reward);
    }

    /// 查询轨迹分段数
    pub fn segment_count(&self, parent_traj_id: &str) -> usize {
        self.segment_registry
            .get(parent_traj_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// 查询轨迹 anchor 终局 reward
    pub fn anchor_reward(&self, parent_traj_id: &str) -> Option<f32> {
        self.anchor_rewards.get(parent_traj_id).map(|v| *v)
    }

    /// 权重采样回放批次（TD 误差轮盘赌）
    pub fn sample_batch(&self, batch_size: usize) -> Vec<PerEntry> {
        self.per_buffer.sample_batch(batch_size)
    }

    /// 缓冲与注册表统计
    pub fn stats(&self) -> PerStats {
        PerStats {
            buffer_len: self.per_buffer.len(),
            trajectory_count: self.segment_registry.len(),
            anchor_count: self.anchor_rewards.len(),
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::rl_types::{MemPiAction, RLAction, RLState};
    use nexus_contracts::token_evidence::SegmentCreationReason;
    use nexus_contracts::SeamId;

    fn exp(reward: f32) -> RLExperience {
        RLExperience {
            state: RLState::new(vec![0.1], 1),
            action: RLAction::MemPi(MemPiAction::Retrieve),
            reward,
            next_state: RLState::new(vec![0.2], 2),
            done: false,
            seam: SeamId::S8MemPi,
        }
    }

    fn segment(id: &str, traj: &str, idx: u32, is_anchor: bool) -> SegmentMetadata {
        SegmentMetadata::new(
            id,
            traj,
            idx,
            is_anchor,
            vec![],
            vec![],
            idx,
            idx,
            SegmentCreationReason::NaturalBoundary,
        )
    }

    // ---------- 铁律9: anchor reward 登记与广播 ----------

    #[test]
    fn anchor_segment_registers_terminal_reward() {
        let mut per = SegmentAwarePER::new(100, 42);
        let anchor = segment("seg-a", "traj-1", 0, true);
        per.add_segment(exp(1.0), anchor, 0.5);
        assert_eq!(per.anchor_reward("traj-1"), Some(1.0));
        assert_eq!(per.segment_count("traj-1"), 1);
    }

    #[test]
    fn broadcast_reward_overrides_anchor() {
        let mut per = SegmentAwarePER::new(100, 42);
        let anchor = segment("seg-a", "traj-1", 0, true);
        per.add_segment(exp(0.5), anchor, 0.5);
        // 广播覆盖（后到者胜，终局 reward 以最终值为准）
        per.broadcast_reward("traj-1", 0.9);
        assert_eq!(per.anchor_reward("traj-1"), Some(0.9));
    }

    #[test]
    fn non_anchor_does_not_register_reward() {
        let mut per = SegmentAwarePER::new(100, 42);
        let non_anchor = segment("seg-b", "traj-2", 1, false);
        per.add_segment(exp(0.3), non_anchor, 0.4);
        assert!(
            per.anchor_reward("traj-2").is_none(),
            "非 anchor 段不登记终局 reward"
        );
    }

    // ---------- prompt-equal denominator ----------

    #[test]
    fn prompt_equal_denominator_reduces_td_error() {
        // 单段轨迹: td_error 不折减（sqrt(1)=1）
        let mut per1 = SegmentAwarePER::new(100, 42);
        per1.add_segment(exp(1.0), segment("s1", "t1", 0, false), 0.8);
        // 4 段轨迹: 末段折减 td_error / sqrt(4) = 0.4
        let mut per4 = SegmentAwarePER::new(100, 42);
        for i in 0..4 {
            per4.add_segment(exp(1.0), segment(&format!("s{i}"), "t4", i, false), 0.8);
        }
        // 单段轨迹: 采样值恒为 0.8（无折减）
        let single = per1.sample_batch(1);
        assert_eq!(single[0].td_error, 0.8);
        // 4 段轨迹: 采样多份，末段（count=4）折减后应为 0.4
        let quad = per4.sample_batch(8);
        let min_adjusted = quad
            .iter()
            .map(|e| e.td_error)
            .fold(f32::INFINITY, f32::min);
        assert!(
            (min_adjusted - 0.4).abs() < 1e-6,
            "末段折减后应为 0.4（实际最小折减 {min_adjusted})"
        );
        // 折减单调: 4 段轨迹的所有折减值 ≤ 单段轨迹值
        assert!(quad.iter().all(|e| e.td_error <= 0.8 + 1e-6));
    }

    // ---------- 采样分布（高 TD 优先） ----------

    #[test]
    fn sampling_prefers_high_td_error() {
        let mut per = SegmentAwarePER::new(0, 7);
        // 100 条低权重 + 1 条高权重
        for i in 0..100 {
            per.add_segment(
                exp(0.1),
                segment(&format!("low-{i}"), "traj-low", i, false),
                0.01,
            );
        }
        per.add_segment(exp(1.0), segment("high", "traj-high", 0, false), 100.0);
        let samples = per.sample_batch(1000);
        // 高权重条目采样占比应远超其条数占比（权重比 10000:1）
        let high_count = samples
            .iter()
            .filter(|e| e.segment.segment_id.as_ref() == "high")
            .count();
        assert!(
            high_count > 900,
            "高 TD 条目应主导采样（实际 {high_count}/1000）"
        );
    }

    #[test]
    fn zero_weight_entries_not_sampled() {
        let mut per = SegmentAwarePER::new(0, 42);
        per.add_segment(exp(0.0), segment("z1", "t1", 0, false), 0.0);
        per.add_segment(exp(0.0), segment("z2", "t1", 1, false), 0.0);
        assert!(per.sample_batch(5).is_empty(), "全零权重无可采样");
    }

    // ---------- 容量淘汰 ----------

    #[test]
    fn capacity_evicts_lowest_td_error() {
        let mut buffer = PerBuffer::new(3, 42);
        buffer.add(PerEntry {
            experience: exp(0.0),
            segment: segment("a", "t", 0, false),
            td_error: 0.1,
        });
        buffer.add(PerEntry {
            experience: exp(0.0),
            segment: segment("b", "t", 1, false),
            td_error: 0.5,
        });
        buffer.add(PerEntry {
            experience: exp(0.0),
            segment: segment("c", "t", 2, false),
            td_error: 0.3,
        });
        buffer.add(PerEntry {
            experience: exp(0.0),
            segment: segment("d", "t", 3, false),
            td_error: 0.9,
        });
        assert_eq!(buffer.len(), 3);
        // 最低 TD（a=0.1）被淘汰（先绑定采样结果，避免临时值借用）
        let samples = buffer.sample_batch(3);
        let ids: Vec<&str> = samples
            .iter()
            .map(|e| e.segment.segment_id.as_ref())
            .collect();
        assert!(!ids.contains(&"a"), "最低 TD 误差条目应被淘汰");
    }

    // ---------- 统计 ----------

    #[test]
    fn stats_tracking() {
        let mut per = SegmentAwarePER::new(100, 42);
        per.add_segment(exp(1.0), segment("a1", "t1", 0, true), 0.5);
        per.add_segment(exp(0.5), segment("a2", "t1", 1, false), 0.3);
        per.add_segment(exp(0.2), segment("b1", "t2", 0, true), 0.4);
        let stats = per.stats();
        assert_eq!(stats.buffer_len, 3);
        assert_eq!(stats.trajectory_count, 2);
        assert_eq!(stats.anchor_count, 2);
    }

    // ---------- 种子确定性 ----------

    #[test]
    fn seeded_sampling_is_deterministic() {
        let mut per_a = SegmentAwarePER::new(0, 99);
        let mut per_b = SegmentAwarePER::new(0, 99);
        for i in 0..10 {
            per_a.add_segment(
                exp(0.1),
                segment(&format!("s{i}"), "t", i as u32, false),
                0.1 + i as f32,
            );
            per_b.add_segment(
                exp(0.1),
                segment(&format!("s{i}"), "t", i as u32, false),
                0.1 + i as f32,
            );
        }
        let a = per_a.sample_batch(20);
        let b = per_b.sample_batch(20);
        let ids_a: Vec<&str> = a.iter().map(|e| e.segment.segment_id.as_ref()).collect();
        let ids_b: Vec<&str> = b.iter().map(|e| e.segment.segment_id.as_ref()).collect();
        assert_eq!(ids_a, ids_b, "相同种子必须产生相同采样序列");
    }
}
