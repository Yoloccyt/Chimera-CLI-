//! 分层经验回放池 — Hot/Warm/Cold/Ice 四层经验分层(polish-v2.7 P4-2)
//!
//! 对应架构层:L3 Storage(cmt-tiering 子模块)
//! 对应 ADR:ADR-049 决策 1(分层回放池落点 cmt-tiering)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §7.1(快手经验回放 + RUC 采样策略)
//!
//! # R2 冻结声明(ADR-042)
//!
//! 本池经验数据**仅可用于 R1 路径**(与 omega-learner `replay_pool.rs`/
//! `per_buffer.rs` 同款约束),禁止用于 R2 约束 RL。
//!
//! # 分层语义(方案 §7.1,复用 CMT 热/温/冷/冰隐喻)
//!
//! | 层 | 容量 | 内容 | 采样权重 |
//! |---|---|---|---|
//! | Hot | 100 | 近期经验(高频采样) | 0.25 |
//! | Warm | 1000 | 高频访问经验(中频) | 0.25 |
//! | Cold | 10000 | 关键失败案例(低频高权重) | **0.5** |
//! | Ice | 100000 | 归档经验(仅离线分析,不参与在线采样) | 0 |
//!
//! WHY 失败经验权重 0.5:失败样本携带的梯度信息量高于成功样本
//! (快手 Process-Score 洞察),分层采样保证训练 batch 中失败案例占半。
//!
//! # 分期声明
//!
//! 本期 Ice 层为内存归档(FIFO 淘汰);SQLite 持久化(复用 `pool.rs`
//! SqlitePool + spawn_blocking)作为后续增量,接口已按 async 预留演进空间。

use std::collections::VecDeque;
use std::sync::Mutex;

use rand::Rng;
use serde::{Deserialize, Serialize};

/// Hot 层容量(近期经验)
const HOT_CAPACITY: usize = 100;
/// Warm 层容量(高频访问经验)
const WARM_CAPACITY: usize = 1_000;
/// Cold 层容量(关键失败案例)
const COLD_CAPACITY: usize = 10_000;
/// Ice 层容量(归档,仅离线分析)
const ICE_CAPACITY: usize = 100_000;

/// 分层采样比例:Hot / Warm / Cold(Ice 不参与在线采样)
///
/// 公开导出供 `pyramid_storage::PYRAMID_SAMPLE_RATIOS` 一致性对齐测试引用
/// (Wave 4 防漂移:两处采样比例必须语义一致)。
pub const SAMPLE_RATIOS: (f32, f32, f32) = (0.25, 0.25, 0.5);

/// 高价值失败经验的奖励阈值(方案 §7.1:reward < -5.0 → Cold)
const FAILURE_REWARD_THRESHOLD: f32 = -5.0;

/// 回放经验条目 — 分层路由的最小信息集
///
/// WHY 本地定义:L3 不依赖上层轨迹类型(§2.2 依赖铁律),
/// 上层将完整轨迹降维为本条目后投喂(与 AEGIS TrajectoryOutcome 同款模式)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayExperience {
    /// 经验唯一标识
    pub experience_id: String,
    /// 标量奖励(负值大 = 高价值失败)
    pub reward: f32,
    /// 轨迹是否成功终止
    pub success: bool,
    /// 序列化的经验负载(MessagePack 建议,ADR-004)
    pub payload: Vec<u8>,
}

/// 分层统计快照
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierStats {
    /// Hot 层当前条数
    pub hot: usize,
    /// Warm 层当前条数
    pub warm: usize,
    /// Cold 层当前条数
    pub cold: usize,
    /// Ice 层当前条数
    pub ice: usize,
}

/// 完整性审计报告（Milestone B-3b，九层防御 L3 补齐）
///
/// 校验回放池不变量：分层统计一致 + 容量不超限 + 无空 payload 损坏条目。
/// 调用方（efficiency-monitor / 发布 FormalViolation 前）定期审计，
/// 发现 `consistent == false` 应触发降级检查（数据面可信度受损）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    /// Hot 层条数
    pub hot: usize,
    /// Warm 层条数
    pub warm: usize,
    /// Cold 层条数
    pub cold: usize,
    /// Ice 层条数
    pub ice: usize,
    /// 总条数（= hot+warm+cold+ice）
    pub total: usize,
    /// 空 payload 条目数（数据损坏信号）
    pub empty_payload: usize,
    /// 超容量层列表（内部淘汰 bug 信号，正常应恒空）
    pub over_capacity_tiers: Vec<&'static str>,
    /// 全部不变量成立
    pub consistent: bool,
}

/// 内部四层缓冲(单锁保护,锁内无 await,§4.4 红线 1)
struct Tiers {
    hot: VecDeque<ReplayExperience>,
    warm: VecDeque<ReplayExperience>,
    cold: VecDeque<ReplayExperience>,
    ice: VecDeque<ReplayExperience>,
}

/// 分层经验回放池
///
/// # 线程安全
/// 单 `Mutex` 保护四层(层间迁移需原子性,分锁会撕裂)。
pub struct TieredReplayPool {
    tiers: Mutex<Tiers>,
}

impl TieredReplayPool {
    /// 创建空分层池
    pub fn new() -> Self {
        Self {
            tiers: Mutex::new(Tiers {
                hot: VecDeque::with_capacity(HOT_CAPACITY),
                warm: VecDeque::with_capacity(WARM_CAPACITY),
                cold: VecDeque::with_capacity(COLD_CAPACITY),
                ice: VecDeque::new(),
            }),
        }
    }

    /// 存储经验 — 按价值自动分层路由(方案 §7.1)
    ///
    /// # 路由规则
    /// 1. 高价值失败(reward < -5.0 或失败终止)→ Cold(失败案例库)
    /// 2. 其他 → Hot(近期经验);Hot 满时最旧经验降级 Warm;
    ///    Warm 满时最旧降级 Ice(归档);Ice 满时 FIFO 淘汰
    ///
    /// WHY 级联降级而非直接丢弃:经验价值随时间衰减但非归零,
    /// 逐层沉降给离线分析(Ice)保留完整历史窗口。
    pub fn store(&self, exp: ReplayExperience) {
        let mut tiers = self.tiers.lock().unwrap_or_else(|e| e.into_inner());

        if exp.reward < FAILURE_REWARD_THRESHOLD || !exp.success {
            // 高价值失败 → Cold;Cold 满时最旧失败案例归档 Ice
            if tiers.cold.len() >= COLD_CAPACITY {
                if let Some(evicted) = tiers.cold.pop_front() {
                    push_ice(&mut tiers.ice, evicted);
                }
            }
            tiers.cold.push_back(exp);
            return;
        }

        // 成功经验 → Hot,级联沉降 Hot → Warm → Ice
        if tiers.hot.len() >= HOT_CAPACITY {
            if let Some(demoted) = tiers.hot.pop_front() {
                if tiers.warm.len() >= WARM_CAPACITY {
                    if let Some(archived) = tiers.warm.pop_front() {
                        push_ice(&mut tiers.ice, archived);
                    }
                }
                tiers.warm.push_back(demoted);
            }
        }
        tiers.hot.push_back(exp);
    }

    /// 分层采样 — 按 0.25/0.25/0.5 比例从 Hot/Warm/Cold 抽取(Ice 不参与)
    ///
    /// 某层样本不足时按该层实际量抽取(不跨层补齐,保持比例语义单纯;
    /// 调用方可按返回量判断池的成熟度)。
    pub fn sample<R: Rng>(&self, batch_size: usize, rng: &mut R) -> Vec<ReplayExperience> {
        let tiers = self.tiers.lock().unwrap_or_else(|e| e.into_inner());
        let mut batch = Vec::with_capacity(batch_size);

        let quota = |ratio: f32| ((batch_size as f32) * ratio).round() as usize;
        sample_tier(&tiers.hot, quota(SAMPLE_RATIOS.0), rng, &mut batch);
        sample_tier(&tiers.warm, quota(SAMPLE_RATIOS.1), rng, &mut batch);
        sample_tier(&tiers.cold, quota(SAMPLE_RATIOS.2), rng, &mut batch);
        batch
    }

    /// 各层条数快照(运维观测)
    pub fn stats(&self) -> TierStats {
        let tiers = self.tiers.lock().unwrap_or_else(|e| e.into_inner());
        TierStats {
            hot: tiers.hot.len(),
            warm: tiers.warm.len(),
            cold: tiers.cold.len(),
            ice: tiers.ice.len(),
        }
    }

    /// 完整性审计（Milestone B-3b）— 回放池不变量校验
    ///
    /// # 检查项
    /// 1. 分层统计一致：total = hot + warm + cold + ice
    /// 2. 各层不超容量（超限 = 内部 FIFO 淘汰 bug）
    /// 3. 无空 payload 条目（空负载 = 序列化损坏信号）
    ///
    /// # 复杂度
    /// O(total)：全量扫描各层 payload（审计低频，完整覆盖优于采样）。
    pub fn integrity_audit(&self) -> IntegrityReport {
        let tiers = self.tiers.lock().unwrap_or_else(|e| e.into_inner());

        let (hot, warm, cold, ice) = (
            tiers.hot.len(),
            tiers.warm.len(),
            tiers.cold.len(),
            tiers.ice.len(),
        );
        let total = hot + warm + cold + ice;

        // 空 payload 扫描（所有层；审计低频可接受全量）
        let empty_payload = tiers
            .hot
            .iter()
            .chain(tiers.warm.iter())
            .chain(tiers.cold.iter())
            .chain(tiers.ice.iter())
            .filter(|e| e.payload.is_empty())
            .count();

        // 超容量层检测
        let mut over_capacity_tiers = Vec::new();
        if hot > HOT_CAPACITY {
            over_capacity_tiers.push("hot");
        }
        if warm > WARM_CAPACITY {
            over_capacity_tiers.push("warm");
        }
        if cold > COLD_CAPACITY {
            over_capacity_tiers.push("cold");
        }
        if ice > ICE_CAPACITY {
            over_capacity_tiers.push("ice");
        }

        let consistent = empty_payload == 0
            && over_capacity_tiers.is_empty()
            && total == hot + warm + cold + ice;

        IntegrityReport {
            hot,
            warm,
            cold,
            ice,
            total,
            empty_payload,
            over_capacity_tiers,
            consistent,
        }
    }
}

impl Default for TieredReplayPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Ice 层归档写入(满时 FIFO 淘汰,容量硬上限防 OOM)
fn push_ice(ice: &mut VecDeque<ReplayExperience>, exp: ReplayExperience) {
    if ice.len() >= ICE_CAPACITY {
        ice.pop_front();
    }
    ice.push_back(exp);
}

/// 从单层有放回均匀抽取 quota 条(层空时跳过)
fn sample_tier<R: Rng>(
    tier: &VecDeque<ReplayExperience>,
    quota: usize,
    rng: &mut R,
    out: &mut Vec<ReplayExperience>,
) {
    if tier.is_empty() {
        return;
    }
    for _ in 0..quota {
        let idx = rng.gen_range(0..tier.len());
        out.push(tier[idx].clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    fn success_exp(id: &str) -> ReplayExperience {
        ReplayExperience {
            experience_id: id.into(),
            reward: 1.0,
            success: true,
            payload: vec![1, 2, 3],
        }
    }

    fn failure_exp(id: &str, reward: f32) -> ReplayExperience {
        ReplayExperience {
            experience_id: id.into(),
            reward,
            success: false,
            payload: vec![9],
        }
    }

    #[test]
    fn test_failure_routes_to_cold() {
        let pool = TieredReplayPool::new();
        pool.store(failure_exp("f1", -10.0));
        pool.store(failure_exp("f2", 0.0)); // 失败终止即使 reward=0 也入 Cold
        pool.store(success_exp("s1"));
        let stats = pool.stats();
        assert_eq!(stats.cold, 2);
        assert_eq!(stats.hot, 1);
    }

    #[test]
    fn test_hot_cascades_to_warm_on_overflow() {
        let pool = TieredReplayPool::new();
        // 填满 Hot(100)+ 1 条溢出 → 最旧降级 Warm
        for i in 0..(HOT_CAPACITY + 1) {
            pool.store(success_exp(&format!("s{i}")));
        }
        let stats = pool.stats();
        assert_eq!(stats.hot, HOT_CAPACITY);
        assert_eq!(stats.warm, 1);
        assert_eq!(stats.ice, 0);
    }

    #[test]
    fn test_tiered_sample_ratio() {
        let pool = TieredReplayPool::new();
        for i in 0..50 {
            pool.store(success_exp(&format!("s{i}")));
        }
        for i in 0..50 {
            pool.store(failure_exp(&format!("f{i}"), -10.0));
        }
        let mut rng = StdRng::seed_from_u64(11);
        let batch = pool.sample(32, &mut rng);
        // Hot 配额 8 + Warm 配额 8(空层跳过)+ Cold 配额 16
        let failures = batch.iter().filter(|e| !e.success).count();
        assert_eq!(failures, 16, "失败经验应占 batch 的 0.5 配额");
        assert_eq!(batch.len(), 8 + 16, "Warm 为空,总量 = Hot 8 + Cold 16");
    }

    #[test]
    fn test_empty_pool_sample_is_empty() {
        let pool = TieredReplayPool::new();
        let mut rng = StdRng::seed_from_u64(1);
        assert!(pool.sample(32, &mut rng).is_empty());
    }
}
