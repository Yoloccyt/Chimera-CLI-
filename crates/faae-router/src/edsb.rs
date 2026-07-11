//! EDSB 熵驱动自均衡 — Entropy-Driven Self-Balancing
//!
//! 对应架构层:L6 Router
//! 对应创新点:EDSB(Entropy-Driven Self-Balancing)
//!
//! # 核心职责
//! - **熵计算**:通过香农熵度量工具负载分布的均匀程度
//! - **概率均衡**:熵值低于阈值时,以 `p = 1 - entropy` 的概率重分配到次优工具
//! - **指数衰减**:定期对使用计数应用指数衰减,近期使用权重更高
//! - **后台衰减循环**:异步定期执行衰减,不阻塞路由路径
//!
//! # 设计决策(WHY)
//! - **香农熵**:标准信息熵公式,适用于负载分布度量
//! - **指数衰减 τ=1 小时**:平衡时近性与历史,近期使用权重更高
//! - **均衡概率 `p = 1 - entropy`**:熵低(负载集中)时均衡概率高,
//!   熵高(负载均匀)时均衡概率低
//! - **不强制均衡**:强制均衡会破坏语义路由准确性,概率性均衡在准确性与均衡性间折中
//! - **伪随机用 rand crate**:`rand::random::<f32>()` 生成 [0.0, 1.0) 均匀分布随机数,
//!   替代原 SystemTime 纳秒方案(Task 11 修复:纳秒伪随机在高频调用时易碰撞,
//!   且非密码学安全;rand crate 提供更稳定的随机性)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::sync::RwLock;
use tracing::warn;

use crate::config::FaaeConfig;
use crate::error::FaaeError;
use crate::types::{EntropyStats, ExpertProfile, ToolId};

/// EDSB 熵驱动自均衡器 — 度量负载分布并概率性重分配
///
/// # 工作原理
/// 1. `compute_entropy`:读取所有工具的 usage_count,计算归一化香农熵
/// 2. `balance`:熵 < 阈值时,以 `p = 1 - entropy` 概率重分配到次优工具
/// 3. `decay_usage_counts`:对 usage_count 应用指数衰减 `raw × exp(-Δt/τ)`
/// 4. `spawn_decay_loop`:后台异步定期执行衰减
///
/// # 线程安全
/// EdsbBalancer 本身无状态(config + event_bus 均 Clone 廉价),
/// 所有方法接收外部 profiles 引用,不持有可变状态。
#[derive(Clone)]
pub struct EdsbBalancer {
    /// FaaE 配置(熵阈值、衰减 τ 等)
    config: FaaeConfig,
    /// 事件总线(发布 EntropyBalanced 事件)
    event_bus: EventBus,
}

impl EdsbBalancer {
    /// 创建 EDSB 均衡器
    pub fn new(config: FaaeConfig, event_bus: EventBus) -> Self {
        Self { config, event_bus }
    }

    /// 计算当前负载的归一化香农熵
    ///
    /// 公式:`H = -Σ(p_i × ln(p_i)) / ln(n)`
    /// - `p_i = usage_count_i / total_usage`
    /// - `n` = 工具数量
    /// - 归一化后熵值 ∈ [0, 1]:0 表示完全集中,1 表示完全均匀
    ///
    /// # 边界处理
    /// - total_usage = 0:返回 1.0(完全均匀,无负载时视为均匀)
    /// - n ≤ 1:返回 1.0(单工具无法计算熵,视为均匀)
    ///
    /// # 参数
    /// - `profiles`:专家注册表(只读访问 usage_count)
    pub async fn compute_entropy(
        &self,
        profiles: &HashMap<ToolId, Arc<RwLock<ExpertProfile>>>,
    ) -> Result<f32, FaaeError> {
        let n = profiles.len();
        if n <= 1 {
            return Ok(1.0);
        }

        // 收集所有工具的 usage_count(原子读,无需持锁)
        let mut counts: Vec<u64> = Vec::with_capacity(n);
        let mut total_usage: u64 = 0;
        for profile_arc in profiles.values() {
            let profile = profile_arc.read().await;
            let count = profile.get_usage_count();
            counts.push(count);
            total_usage = total_usage.saturating_add(count);
        }

        if total_usage == 0 {
            return Ok(1.0);
        }

        // 计算香农熵:H = -Σ(p_i × ln(p_i))
        let total_f = total_usage as f64;
        let mut entropy: f64 = 0.0;
        for &count in &counts {
            if count == 0 {
                continue;
            }
            let p = count as f64 / total_f;
            entropy -= p * p.ln();
        }

        // 归一化:除以 ln(n),使熵值 ∈ [0, 1]
        let max_entropy = (n as f64).ln();
        if max_entropy == 0.0 {
            return Ok(1.0);
        }
        let normalized = (entropy / max_entropy) as f32;
        // 钳制浮点误差,确保 ∈ [0.0, 1.0]
        Ok(normalized.clamp(0.0, 1.0))
    }

    /// 计算熵统计(熵值 + 总使用量 + 工具数)
    ///
    /// 便于外部监控与日志记录
    pub async fn compute_entropy_stats(
        &self,
        profiles: &HashMap<ToolId, Arc<RwLock<ExpertProfile>>>,
    ) -> Result<EntropyStats, FaaeError> {
        let n = profiles.len();
        let mut total_usage: u64 = 0;
        for profile_arc in profiles.values() {
            let profile = profile_arc.read().await;
            total_usage = total_usage.saturating_add(profile.get_usage_count());
        }

        let entropy = self.compute_entropy(profiles).await?;
        Ok(EntropyStats {
            entropy,
            total_usage,
            tool_count: n,
        })
    }

    /// 概率性均衡 — 熵值低于阈值时,以 `p = 1 - entropy` 概率重分配到次优工具
    ///
    /// # 均衡逻辑
    /// 1. 计算当前熵值
    /// 2. 若熵 ≥ 阈值:无需均衡,返回原工具
    /// 3. 若熵 < 阈值:
    ///    a. 计算均衡概率 `p = 1 - entropy`
    ///    b. 生成伪随机数 r ∈ [0, 1)
    ///    c. 若 r < p:调用多步均衡选择最优目标工具
    ///    d. 否则:保持原工具
    /// 4. 发布 EntropyBalanced 事件(携带 old/new 熵值与重分配计数)
    ///
    /// # 多步均衡(P1-7)
    /// 当 `config.multi_step_balance_depth > 1` 时,不简单地选 candidates[1],
    /// 而是模拟多步重分配后的熵值,选择使最终熵最大的分配路径。
    ///
    /// # 参数
    /// - `profiles`:专家注册表
    /// - `routed_tool`:语义路由选中的工具(Top-1)
    /// - `candidates`:Top-K 候选列表(按相似度降序)
    ///
    /// # 返回
    /// - `Some(tool_id)`:均衡后的工具(原工具或次优工具)
    /// - `None`:无法均衡(如候选不足)
    pub async fn balance(
        &self,
        profiles: &HashMap<ToolId, Arc<RwLock<ExpertProfile>>>,
        routed_tool: &ToolId,
        candidates: &[(ToolId, f32)],
    ) -> Option<ToolId> {
        // 候选不足,无法均衡
        if candidates.len() < 2 {
            return Some(routed_tool.clone());
        }

        let old_entropy = match self.compute_entropy(profiles).await {
            Ok(e) => e,
            Err(_) => return Some(routed_tool.clone()),
        };

        // 熵值足够高,无需均衡
        if old_entropy >= self.config.entropy_threshold {
            return Some(routed_tool.clone());
        }

        // 均衡概率 p = 1 - entropy
        let balance_prob = 1.0 - old_entropy;
        let random_val = pseudo_random_probability();

        let (final_tool, redistributed) = if random_val < balance_prob {
            // 多步均衡:选择使模拟熵最大的目标工具
            let depth = self.config.multi_step_balance_depth.max(1);
            let target = self
                .multi_step_balance(profiles, routed_tool, candidates, depth)
                .await;
            (target, 1u32)
        } else {
            (routed_tool.clone(), 0u32)
        };

        // 估算均衡后熵值(模拟计数变化)
        let new_entropy = self
            .estimate_entropy_after_redistribution(profiles, routed_tool, &final_tool)
            .await
            .unwrap_or(old_entropy);

        // 发布 EntropyBalanced 事件
        let event = NexusEvent::EntropyBalanced {
            metadata: EventMetadata::new("faae-router"),
            old_entropy,
            new_entropy,
            redistributed_count: redistributed,
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "EntropyBalanced 事件发布失败(不影响均衡结果)");
        }

        Some(final_tool)
    }

    /// 多步均衡 — 模拟多步重分配后的熵值,选择最优目标工具
    ///
    /// # 算法
    /// 1. 对 candidates[1..] 中的每个候选工具,模拟"将 routed_tool 的计数 -1,
    ///    候选工具计数 +1"后的新熵值
    /// 2. 选择使新熵值最大的候选作为目标
    /// 3. 若 depth > 1,递归对目标工具继续寻找最优重分配
    ///
    /// # 参数
    /// - `profiles`:专家注册表
    /// - `routed_tool`:当前被路由的工具
    /// - `candidates`:候选列表(按相似度降序)
    /// - `depth`:剩余均衡步数
    async fn multi_step_balance(
        &self,
        profiles: &HashMap<ToolId, Arc<RwLock<ExpertProfile>>>,
        routed_tool: &ToolId,
        candidates: &[(ToolId, f32)],
        depth: usize,
    ) -> ToolId {
        if depth == 0 || candidates.len() < 2 {
            return routed_tool.clone();
        }

        // 收集所有候选(排除 routed_tool 自身)
        let candidate_tools: Vec<&ToolId> = candidates
            .iter()
            .map(|(id, _)| id)
            .filter(|id| *id != routed_tool)
            .collect();

        if candidate_tools.is_empty() {
            return routed_tool.clone();
        }

        // 评估每个候选的单步重分配后的熵值
        let mut best_entropy = -1.0f32;
        let mut best_tool = candidate_tools[0].clone();

        for candidate in &candidate_tools {
            // 估算单步重分配后的熵值
            let entropy = self
                .estimate_entropy_after_redistribution(profiles, routed_tool, candidate)
                .await
                .unwrap_or(0.0);

            // 若 depth > 1,递归评估多步效果
            let final_entropy = if depth > 1 {
                // 模拟重分配后的 profiles(不修改实际 profiles)
                // WHY Box::pin:递归 async fn 调用会产生无限大 future,必须用 Pin<Box<dyn Future>> 引入间接层
                let next_tool =
                    Box::pin(self.multi_step_balance(profiles, candidate, candidates, depth - 1))
                        .await;
                // 如果下一步还会重分配,用下一步的目标估算
                if next_tool != **candidate {
                    self.estimate_entropy_after_redistribution(profiles, candidate, &next_tool)
                        .await
                        .unwrap_or(entropy)
                } else {
                    entropy
                }
            } else {
                entropy
            };

            if final_entropy > best_entropy {
                best_entropy = final_entropy;
                best_tool = (*candidate).clone();
            }
        }

        best_tool
    }

    /// 估算重分配后的熵值(模拟计数变化,不修改实际 usage_count)
    ///
    /// WHY:balance 需要发布 new_entropy,但不能在 balance 中修改 usage_count
    /// (route 路径会在 balance 返回后才更新计数)。通过模拟计数变化估算新熵值。
    async fn estimate_entropy_after_redistribution(
        &self,
        profiles: &HashMap<ToolId, Arc<RwLock<ExpertProfile>>>,
        original: &ToolId,
        redistributed: &ToolId,
    ) -> Result<f32, FaaeError> {
        let n = profiles.len();
        if n <= 1 {
            return Ok(1.0);
        }

        // 收集计数快照
        let mut counts: Vec<u64> = Vec::with_capacity(n);
        let mut total_usage: u64 = 0;
        for (tid, profile_arc) in profiles.iter() {
            let profile = profile_arc.read().await;
            let mut count = profile.get_usage_count();
            // 模拟:原工具计数 -1,重分配工具计数 +1
            if tid == original {
                count = count.saturating_sub(1);
            }
            if tid == redistributed {
                count = count.saturating_add(1);
            }
            counts.push(count);
            total_usage = total_usage.saturating_add(count);
        }

        if total_usage == 0 {
            return Ok(1.0);
        }

        let total_f = total_usage as f64;
        let mut entropy: f64 = 0.0;
        for &count in &counts {
            if count == 0 {
                continue;
            }
            let p = count as f64 / total_f;
            entropy -= p * p.ln();
        }

        let max_entropy = (n as f64).ln();
        if max_entropy == 0.0 {
            return Ok(1.0);
        }
        Ok(((entropy / max_entropy) as f32).clamp(0.0, 1.0))
    }

    /// 对所有工具的使用计数应用指数衰减
    ///
    /// 公式:`decayed_count = raw_count × exp(-Δt / τ)`
    /// - `Δt` = 当前时间 - last_used_at(秒)
    /// - `τ` = config.decay_tau(默认 3600 秒 = 1 小时)
    ///
    /// # 衰减效果
    /// - Δt = 0(刚使用):factor = 1.0(不衰减)
    /// - Δt = 5 分钟:factor ≈ 0.92(衰减 8%)
    /// - Δt = 1 小时:factor ≈ 0.37(衰减 63%)
    /// - Δt = 2 小时:factor ≈ 0.14(衰减 86%)
    ///
    /// # 注意
    /// 衰减后更新 usage_count(原子 store),last_used_at 不变
    /// (仅路由路径更新 last_used_at)
    pub async fn decay_usage_counts(&self, profiles: &HashMap<ToolId, Arc<RwLock<ExpertProfile>>>) {
        let now = Instant::now();
        let tau = self.config.decay_tau;

        for profile_arc in profiles.values() {
            // WHY: 分阶段获取锁,避免嵌套锁(profile 读锁内获取 last_used_at 读锁)跨 await:
            //   阶段1:持 profile 读锁获取 raw_count + clone last_used_at Arc,立即释放
            //   阶段2:锁外读取 last_used_at(单独 await,不嵌套 profile 锁),计算衰减
            //   阶段3:持 profile 读锁原子更新 usage_count(同步操作),立即释放
            // last_used_at 是 Arc<RwLock<Instant>>,可 clone 后锁外访问(B-Crit-3 修复)。

            // 阶段1:获取快照(raw_count + last_used_at Arc)
            let (raw_count, last_used_arc) = {
                let profile = profile_arc.read().await;
                (profile.get_usage_count(), profile.last_used_at.clone())
            }; // profile 读锁释放
            if raw_count == 0 {
                continue;
            }

            // 阶段2:锁外读取 last_used_at 并计算衰减(不嵌套 profile 锁)
            let last_used = *last_used_arc.read().await;
            let delta_secs = now.duration_since(last_used).as_secs_f64();

            // 衰减因子:exp(-Δt / τ)
            let decay_factor = (-delta_secs / tau).exp();
            let decayed_count = (raw_count as f64 * decay_factor).round() as u64;

            // 阶段3:原子更新 usage_count(set_usage_count 是 &self 原子 store,同步操作)
            {
                let profile = profile_arc.read().await;
                profile.set_usage_count(decayed_count);
            } // profile 读锁释放
        }
    }

    /// 启动后台衰减循环
    ///
    /// 每 `config.decay_interval_secs`(默认 300 秒 = 5 分钟)执行一次 `decay_usage_counts`,
    /// 在独立 tokio 任务中异步运行,不阻塞路由路径。
    ///
    /// WHY 配置化:衰减周期从 `FaaeConfig.decay_interval_secs` 读取,
    /// 支持按部署场景调整(边缘设备拉长降低 CPU 开销,高频场景缩短提升时近性)。
    ///
    /// WHY 返回 JoinHandle:调用方可通过 `handle.abort()` 优雅停止后台衰减,
    /// 或 `handle.await` 等待任务结束(B-Min-3 修复,避免 fire-and-forget)。
    /// 忽略返回值也是安全的 — 任务会独立运行直至进程退出。
    ///
    /// # 参数
    /// - `self`:需要 `Arc<Self>` 持有自身引用
    /// - `profiles`:专家注册表(Arc 共享,跨任务访问)
    ///
    /// # 返回
    /// 后台衰减任务的 `JoinHandle`,调用方可选择 await/abort 或忽略
    pub fn spawn_decay_loop(
        self: Arc<Self>,
        profiles: Arc<RwLock<HashMap<ToolId, Arc<RwLock<ExpertProfile>>>>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // WHY 从 config 读取周期:支持按部署场景调整,默认 300s 保持向后兼容
            let interval = Duration::from_secs(self.config.decay_interval_secs);
            loop {
                tokio::time::sleep(interval).await;
                // WHY: 克隆 HashMap 获取快照后立即释放 registry 读锁,再调用 decay_usage_counts,
                // 避免 decay 内部的多次 await(profile 读锁、last_used_at 读锁)跨 registry 锁,
                // 导致 register/unregister 写锁被阻塞(B-Crit-4)。
                // HashMap 内是 Arc<RwLock<ExpertProfile>>,Clone 仅增加引用计数,代价低。
                let registry_snapshot: HashMap<ToolId, Arc<RwLock<ExpertProfile>>> = {
                    let registry = profiles.read().await;
                    registry.clone()
                };
                self.decay_usage_counts(&registry_snapshot).await;
            }
        })
    }

    /// 获取配置引用
    pub fn config(&self) -> &FaaeConfig {
        &self.config
    }
}

/// 生成伪随机概率 [0.0, 1.0)
///
/// WHY:使用 `rand::random::<f32>()` 生成 [0.0, 1.0) 均匀分布随机数。
/// Task 11 修复:原 SystemTime 纳秒方案在高频调用时(纳秒精度内多次调用)
/// 易产生相同返回值,且非密码学安全;rand crate 使用 thread-local CSPRNG
/// ( ChaCha12-based),提供更稳定的随机性,满足 EDSB 概率均衡严格性。
fn pseudo_random_probability() -> f32 {
    rand::random::<f32>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolId;

    /// 构造测试用专家注册表
    async fn make_profiles(counts: &[(&str, u64)]) -> HashMap<ToolId, Arc<RwLock<ExpertProfile>>> {
        let mut map = HashMap::new();
        for &(name, count) in counts {
            let profile = ExpertProfile::with_usage_count(name, vec![0.5; 64], vec![], 0.5, count);
            map.insert(ToolId::new(name), Arc::new(RwLock::new(profile)));
        }
        map
    }

    #[tokio::test]
    async fn test_entropy_uniform_distribution() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        // 4 个工具,各 25 次使用 → 完全均匀 → 熵 = 1.0
        let profiles = make_profiles(&[("t1", 25), ("t2", 25), ("t3", 25), ("t4", 25)]).await;
        let entropy = balancer.compute_entropy(&profiles).await.unwrap();
        assert!(
            (entropy - 1.0).abs() < 1e-5,
            "完全均匀分布熵应为 1.0,实际 {entropy}"
        );
    }

    #[tokio::test]
    async fn test_entropy_concentrated_distribution() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        // 4 个工具,全部负载集中在一个工具 → 熵 ≈ 0.0
        let profiles = make_profiles(&[("t1", 100), ("t2", 0), ("t3", 0), ("t4", 0)]).await;
        let entropy = balancer.compute_entropy(&profiles).await.unwrap();
        assert!(entropy < 0.01, "完全集中分布熵应接近 0.0,实际 {entropy}");
    }

    #[tokio::test]
    async fn test_entropy_zero_total_usage() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        // 所有工具 usage_count = 0 → 返回 1.0
        let profiles = make_profiles(&[("t1", 0), ("t2", 0), ("t3", 0)]).await;
        let entropy = balancer.compute_entropy(&profiles).await.unwrap();
        assert_eq!(entropy, 1.0);
    }

    #[tokio::test]
    async fn test_entropy_single_tool() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        // 仅 1 个工具 → 返回 1.0
        let profiles = make_profiles(&[("t1", 100)]).await;
        let entropy = balancer.compute_entropy(&profiles).await.unwrap();
        assert_eq!(entropy, 1.0);
    }

    #[tokio::test]
    async fn test_entropy_partial_concentration() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        // 4 个工具,负载 85/10/3/2 → 熵 < 0.6(触发均衡)
        // WHY:70/10/10/10 的归一化熵 ≈ 0.678(> 0.6,不触发均衡),
        // 需更集中的分布才能低于阈值。85/10/3/2 的归一化熵 ≈ 0.398
        let profiles = make_profiles(&[("t1", 85), ("t2", 10), ("t3", 3), ("t4", 2)]).await;
        let entropy = balancer.compute_entropy(&profiles).await.unwrap();
        assert!(entropy < 0.6, "85/10/3/2 分布熵应 < 0.6,实际 {entropy}");
        assert!(entropy > 0.0, "熵应 > 0.0,实际 {entropy}");
    }

    #[tokio::test]
    async fn test_balance_high_entropy_no_redistribution() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        // 均匀分布 → 熵 = 1.0 > 0.6 → 不均衡
        let profiles = make_profiles(&[("t1", 25), ("t2", 25), ("t3", 25), ("t4", 25)]).await;
        let candidates: Vec<(ToolId, f32)> =
            vec![(ToolId::new("t1"), 0.9), (ToolId::new("t2"), 0.8)];
        let result = balancer
            .balance(&profiles, &ToolId::new("t1"), &candidates)
            .await;
        assert_eq!(result, Some(ToolId::new("t1"))); // 保持原工具
    }

    #[tokio::test]
    async fn test_balance_low_entropy_triggers_balance() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        // 集中分布 → 熵 < 0.6 → 触发均衡
        let profiles = make_profiles(&[("t1", 100), ("t2", 0), ("t3", 0), ("t4", 0)]).await;
        let candidates: Vec<(ToolId, f32)> =
            vec![(ToolId::new("t1"), 0.9), (ToolId::new("t2"), 0.8)];

        // 多次调用,验证有时会重分配(概率性)
        let mut redistributed_count = 0;
        for _ in 0..100 {
            let result = balancer
                .balance(&profiles, &ToolId::new("t1"), &candidates)
                .await;
            if result == Some(ToolId::new("t2")) {
                redistributed_count += 1;
            }
        }
        // 熵 ≈ 0 → p ≈ 1.0,应大部分时候重分配
        assert!(
            redistributed_count > 50,
            "低熵时应大部分重分配,实际重分配 {redistributed_count}/100 次"
        );
    }

    #[tokio::test]
    async fn test_balance_single_candidate_no_redistribution() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        let profiles = make_profiles(&[("t1", 100), ("t2", 0)]).await;
        // 仅 1 个候选 → 无法均衡
        let candidates: Vec<(ToolId, f32)> = vec![(ToolId::new("t1"), 0.9)];
        let result = balancer
            .balance(&profiles, &ToolId::new("t1"), &candidates)
            .await;
        assert_eq!(result, Some(ToolId::new("t1")));
    }

    #[tokio::test]
    async fn test_decay_usage_counts() {
        let bus = EventBus::new();
        let config = FaaeConfig::default();
        let balancer = EdsbBalancer::new(config.clone(), bus);

        // 创建一个 profile,设置 last_used_at 为 1 小时前
        let profile = ExpertProfile::with_usage_count("t1", vec![0.5; 64], vec![], 0.5, 100);
        // 手动设置 last_used_at 为 1 小时前
        // WHY: Windows 上 Instant 基于 QPC(从系统启动计数),系统启动不足 3600 秒时
        // `Instant::now() - Duration::from_secs(3600)` 会溢出 panic。
        // 用 checked_sub 安全降级到 60 秒,并动态计算实际 Δt 用于断言。
        let now_before = Instant::now();
        let last_used_instant = now_before
            .checked_sub(Duration::from_secs(3600))
            .unwrap_or_else(|| now_before - Duration::from_secs(60));
        {
            let mut last_used = profile.last_used_at.write().await;
            *last_used = last_used_instant;
        }

        let mut profiles = HashMap::new();
        profiles.insert(ToolId::new("t1"), Arc::new(RwLock::new(profile)));

        // 衰减:Δt = actual_delta_secs,τ = 3600 秒 → factor = exp(-Δt/τ)
        balancer.decay_usage_counts(&profiles).await;

        // 计算实际 Δt(decay_usage_counts 内部用 decay 调用前的 Instant::now())
        let actual_delta_secs = Instant::now()
            .duration_since(last_used_instant)
            .as_secs_f64();
        let tau = config.decay_tau;
        let expected = 100.0 * (-actual_delta_secs / tau).exp();

        let profile = profiles.get(&ToolId::new("t1")).unwrap().read().await;
        let decayed = profile.get_usage_count();
        // 容差 ±2(四舍五入误差)
        assert!(
            (decayed as f64 - expected).abs() < 2.0,
            "衰减后计数应 ≈ {expected:.2}(Δt={actual_delta_secs:.0}s),实际 {decayed}"
        );
    }

    #[tokio::test]
    async fn test_decay_zero_count_no_change() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);

        let profile = ExpertProfile::with_usage_count("t1", vec![0.5; 64], vec![], 0.5, 0);
        let mut profiles = HashMap::new();
        profiles.insert(ToolId::new("t1"), Arc::new(RwLock::new(profile)));

        balancer.decay_usage_counts(&profiles).await;

        let profile = profiles.get(&ToolId::new("t1")).unwrap().read().await;
        assert_eq!(profile.get_usage_count(), 0);
    }

    #[tokio::test]
    async fn test_decay_recent_usage_minimal_decay() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);

        // last_used_at = now → Δt ≈ 0 → 几乎不衰减
        let profile = ExpertProfile::with_usage_count("t1", vec![0.5; 64], vec![], 0.5, 100);
        let mut profiles = HashMap::new();
        profiles.insert(ToolId::new("t1"), Arc::new(RwLock::new(profile)));

        balancer.decay_usage_counts(&profiles).await;

        let profile = profiles.get(&ToolId::new("t1")).unwrap().read().await;
        let decayed = profile.get_usage_count();
        // Δt ≈ 0 → factor ≈ 1.0 → 衰减后 ≈ 100
        assert!(decayed >= 99, "刚使用的工具应几乎不衰减,实际 {decayed}");
    }

    #[tokio::test]
    async fn test_compute_entropy_stats() {
        let bus = EventBus::new();
        let balancer = EdsbBalancer::new(FaaeConfig::default(), bus);
        let profiles = make_profiles(&[("t1", 50), ("t2", 30), ("t3", 20)]).await;
        let stats = balancer.compute_entropy_stats(&profiles).await.unwrap();
        assert_eq!(stats.tool_count, 3);
        assert_eq!(stats.total_usage, 100);
        assert!(stats.entropy > 0.0 && stats.entropy <= 1.0);
    }

    #[test]
    fn test_pseudo_random_probability_range() {
        for _ in 0..1000 {
            let p = pseudo_random_probability();
            assert!(
                (0.0..1.0).contains(&p),
                "概率应在 [0.0, 1.0) 范围内,实际 {p}"
            );
        }
    }
}
