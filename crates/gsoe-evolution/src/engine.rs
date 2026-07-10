//! 进化引擎核心 — GsoeEvolutionEngine
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:GSOE(Guided Self-Organizing Evolution)
//!
//! # 核心流程
//! 1. 采样:基于当前策略生成一组 rollout
//! 2. 评估:计算 GRPO 组内相对优势 + 规则适应度
//! 3. 选择:按适应度排序,选取 top elite_ratio 作为精英
//! 4. 变异:基于精英参数生成新策略
//! 5. 发布:通过 EventBus 广播 GsoePolicyUpdated 事件
//!
//! # 事件订阅
//! - `ConsensusReached`:议会共识作为进化奖励,提升下次采样的 reward 基线
//! - `RedTeamAudit`:红队审计作为对抗信号,提升下次变异的 mutation_rate

use crate::config::GsoeConfig;
use crate::error::GsoeError;
use crate::model_client::ModelSampler;
use crate::policy::fitness::evaluate_population;
use crate::policy::grpo::{compute_advantage, sample_rollouts, sample_rollouts_with_model};
use crate::policy::mutation::{apply_mutation, mutate};
use crate::types::{EvolutionPolicy, EvolutionResult, FitnessReport, GrpoRollout};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::debug;

/// GSOE 进化引擎 — GRPO 风格的在线强化学习驱动器
///
/// 维护当前进化策略、世代计数器与可选的 EventBus 连接。
/// 每次调用 `evolve_once` 执行一轮完整的"采样-评估-选择-变异"循环。
pub struct GsoeEvolutionEngine {
    /// 当前进化策略
    current_policy: EvolutionPolicy,
    /// 引擎配置
    config: GsoeConfig,
    /// 当前世代数(从 0 开始,每次 evolve_once 后 +1)
    generation: u64,
    /// 可选的 EventBus 连接(用于发布 GsoePolicyUpdated 事件)
    event_bus: Option<EventBus>,
    /// 模型采样客户端(Mock 或真实)
    model_sampler: ModelSampler,
    /// 待处理的议会共识信号数(作为进化奖励加成)
    pending_consensus_count: u32,
    /// 待处理的红队审计信号数(提升 mutation_rate)
    pending_red_team_count: u32,
}

impl GsoeEvolutionEngine {
    /// 构造进化引擎(无 EventBus 连接,Mock 模型采样)
    pub fn new(config: GsoeConfig) -> Self {
        let policy = config.to_initial_policy().unwrap_or_else(|_| {
            // 配置非法时回退到 Default(防御性:配置应已在外部校验)
            EvolutionPolicy::default()
        });
        Self {
            current_policy: policy,
            config,
            generation: 0,
            event_bus: None,
            model_sampler: ModelSampler::mock(),
            pending_consensus_count: 0,
            pending_red_team_count: 0,
        }
    }

    /// 构造进化引擎(带 EventBus 连接,Mock 模型采样)
    pub fn with_event_bus(config: GsoeConfig, bus: EventBus) -> Self {
        let mut engine = Self::new(config);
        engine.event_bus = Some(bus);
        engine
    }

    /// 构造进化引擎(带真实模型采样)
    ///
    /// # 参数
    /// - `config`:进化引擎配置
    /// - `model_endpoint`:模型服务 HTTP 端点(如 "http://203.0.113.1:8080/v1/sample")
    /// - `model_timeout_ms`:模型采样请求超时(毫秒)
    pub fn with_model(config: GsoeConfig, model_endpoint: impl Into<String>, model_timeout_ms: u64) -> Self {
        let mut engine = Self::new(config);
        engine.model_sampler = ModelSampler::new(model_endpoint, model_timeout_ms);
        engine
    }

    /// 构造进化引擎(带 EventBus 与真实模型采样)
    pub fn with_event_bus_and_model(config: GsoeConfig, bus: EventBus, model_endpoint: impl Into<String>, model_timeout_ms: u64) -> Self {
        let mut engine = Self::with_model(config, model_endpoint, model_timeout_ms);
        engine.event_bus = Some(bus);
        engine
    }

    /// 获取当前进化策略引用
    pub fn current_policy(&self) -> &EvolutionPolicy {
        &self.current_policy
    }

    /// 获取当前世代数
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 处理议会共识信号 — 作为进化奖励加成
    ///
    /// 收到 ConsensusReached 事件时调用,记录信号数量,
    /// 下次 evolve_once 时将 consensus_count * 0.1 加到 reward 基线。
    pub fn handle_consensus_reached(&mut self) {
        self.pending_consensus_count += 1;
        debug!(
            consensus_count = self.pending_consensus_count,
            "收到议会共识信号,作为进化奖励加成"
        );
    }

    /// 处理红队审计信号 — 作为对抗进化触发器
    ///
    /// 收到 RedTeamAudit 事件时调用,记录信号数量,
    /// 下次 evolve_once 时临时提升 mutation_rate(对抗进化)。
    pub fn handle_red_team_audit(&mut self) {
        self.pending_red_team_count += 1;
        debug!(
            red_team_count = self.pending_red_team_count,
            "收到红队审计信号,触发对抗进化(提升 mutation_rate)"
        );
    }

    /// 执行单轮进化
    ///
    /// 完整流程:采样 → 评估 → 选择 → 变异 → 发布事件
    pub async fn evolve_once(&mut self) -> Result<EvolutionResult, GsoeError> {
        // 检查世代上限(架构红线:避免无限进化消耗资源)
        if self.generation >= self.config.max_generation {
            return Err(GsoeError::MaxGenerationReached {
                max_generation: self.config.max_generation,
            });
        }

        // 步骤 1:采样
        let mut rollouts = self.sample_with_signals().await;

        // 步骤 2:计算优势 + 评估适应度
        compute_advantage(&mut rollouts);
        let fitness_reports = evaluate_population(&rollouts);

        // 步骤 3:选择精英
        let old_avg_fitness = Self::average_fitness(&fitness_reports);
        let elite_reports = Self::select_elite(&fitness_reports, self.current_policy.elite_ratio);
        let elite_avg_fitness = Self::average_fitness(&elite_reports);

        // 步骤 4:基于精英生成新策略
        let new_policy = self.generate_new_policy(&elite_reports)?;

        // 步骤 5:计算改进幅度
        // improvement = elite 平均适应度 - 种群平均适应度
        // (elite 代表新策略方向,种群代表旧策略水平)
        let improvement = elite_avg_fitness - old_avg_fitness;

        // 步骤 6:更新状态
        self.generation += 1;
        let old_policy = self.current_policy.clone();
        self.current_policy = new_policy.clone();

        // 清除待处理信号(已在本轮进化中消费)
        self.pending_consensus_count = 0;
        self.pending_red_team_count = 0;

        // 步骤 7:发布事件
        self.publish_evolution_event(&new_policy, improvement).await;

        debug!(
            generation = self.generation,
            improvement,
            old_mr = old_policy.mutation_rate,
            new_mr = new_policy.mutation_rate,
            "进化完成"
        );

        Ok(EvolutionResult {
            new_policy,
            improvement,
            generation: self.generation,
        })
    }

    /// 带信号加成的采样 — 支持真实模型采样(P0-6)
    ///
    /// - consensus 信号:每个加 0.1 到 reward 基线(进化奖励)
    /// - red_team 信号:临时提升 mutation_rate(对抗进化)
    async fn sample_with_signals(&self) -> Vec<GrpoRollout> {
        // 对抗进化:red_team 信号提升 mutation_rate
        let effective_rate = if self.pending_red_team_count > 0 {
            // 每个红队信号提升 50% mutation_rate,上限 1.0
            let boost = 1.0 + 0.5 * self.pending_red_team_count as f32;
            (self.current_policy.mutation_rate * boost).min(1.0)
        } else {
            self.current_policy.mutation_rate
        };

        let mut adjusted_policy = self.current_policy.clone();
        adjusted_policy.mutation_rate = effective_rate;

        // P0-6: 使用 ModelSampler 进行真实模型采样(Mock 模式自动回退)
        let mut rollouts = sample_rollouts_with_model(
            &adjusted_policy,
            self.config.default_rollout_count,
            &self.model_sampler,
        )
        .await;

        // consensus 信号:加到 reward 基线(进化奖励)
        if self.pending_consensus_count > 0 {
            let consensus_bonus = 0.1 * self.pending_consensus_count as f32;
            for rollout in rollouts.iter_mut() {
                rollout.reward += consensus_bonus;
            }
        }

        rollouts
    }

    /// 基于精英适应度报告生成新策略
    ///
    /// 策略:对当前策略应用变异,变异幅度受精英平均置信度调制
    fn generate_new_policy(
        &self,
        elite_reports: &[FitnessReport],
    ) -> Result<EvolutionPolicy, GsoeError> {
        let mut new_policy = self.current_policy.clone();

        // 精英平均置信度作为变异率调制因子
        let elite_confidence = Self::average_confidence(elite_reports);
        let mutation_rate = self.current_policy.mutation_rate * elite_confidence;

        // 生成并应用变异候选
        let candidate = mutate(&self.current_policy, mutation_rate)?;
        apply_mutation(&mut new_policy, &candidate);

        // 确保 rollout_count 不变(变异不应改变采样规模)
        new_policy.rollout_count = self.current_policy.rollout_count;

        Ok(new_policy)
    }

    /// 发布 GsoePolicyUpdated 事件(若已连接 EventBus)
    async fn publish_evolution_event(&self, new_policy: &EvolutionPolicy, improvement: f32) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::GsoePolicyUpdated {
                metadata: EventMetadata::new("gsoe-evolution"),
                generation: self.generation,
                improvement,
                new_mutation_rate: new_policy.mutation_rate,
                new_selection_pressure: new_policy.selection_pressure,
            };
            // WHY:publish 失败不应阻断进化流程,仅记录日志
            if let Err(e) = bus.publish(event).await {
                tracing::warn!(error = %e, "发布 GsoePolicyUpdated 事件失败");
            }
        }
    }

    /// 计算适应度报告列表的平均 fitness_score
    fn average_fitness(reports: &[FitnessReport]) -> f32 {
        if reports.is_empty() {
            return 0.0;
        }
        reports.iter().map(|r| r.fitness_score).sum::<f32>() / reports.len() as f32
    }

    /// 计算适应度报告列表的平均 confidence
    fn average_confidence(reports: &[FitnessReport]) -> f32 {
        if reports.is_empty() {
            return 0.5; // 默认中等置信度
        }
        reports.iter().map(|r| r.confidence).sum::<f32>() / reports.len() as f32
    }

    /// 按 fitness_score 降序排序,选取 top elite_ratio 作为精英
    fn select_elite(reports: &[FitnessReport], elite_ratio: f32) -> Vec<FitnessReport> {
        if reports.is_empty() {
            return Vec::new();
        }

        // 至少保留 1 个精英,最多保留全部
        let elite_count = ((reports.len() as f32) * elite_ratio).ceil() as usize;
        let elite_count = elite_count.max(1).min(reports.len());

        let mut sorted: Vec<FitnessReport> = reports.to_vec();
        // WHY: Top-K 必须用 select_nth_unstable (O(n)) 而非 sort_by + truncate (O(n log n))
        //      (§6.2 红线 + §4.4 工程约定)。原实现全排序 O(n log n) 后 take,
        //      改用 select_nth_unstable O(n) 划分,再对前 elite_count 做 K-log-K 排序,
        //      总复杂度 O(n + k log k)。
        // 降序:b.fitness_score vs a.fitness_score,让前 elite_count 是适应度最高的
        if elite_count < sorted.len() {
            sorted.select_nth_unstable_by(elite_count, |a, b| {
                b.fitness_score
                    .partial_cmp(&a.fitness_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        sorted.truncate(elite_count);
        // 仅对前 elite_count 做 K-log-K 排序(降序,适应度高的在前),保证精英有序
        sorted.sort_by(|a, b| {
            b.fitness_score
                .partial_cmp(&a.fitness_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        sorted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_new_initializes_correctly() {
        let engine = GsoeEvolutionEngine::new(GsoeConfig::default());
        assert_eq!(engine.generation(), 0);
        assert_eq!(engine.current_policy().mutation_rate, 0.1);
        assert_eq!(engine.current_policy().rollout_count, 8);
    }

    #[test]
    fn test_engine_with_event_bus() {
        let bus = EventBus::new();
        let engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);
        assert!(engine.event_bus.is_some());
    }

    #[test]
    fn test_handle_consensus_reached_increments() {
        let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
        assert_eq!(engine.pending_consensus_count, 0);
        engine.handle_consensus_reached();
        engine.handle_consensus_reached();
        assert_eq!(engine.pending_consensus_count, 2);
    }

    #[test]
    fn test_handle_red_team_audit_increments() {
        let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
        assert_eq!(engine.pending_red_team_count, 0);
        engine.handle_red_team_audit();
        assert_eq!(engine.pending_red_team_count, 1);
    }

    #[test]
    fn test_select_elite_returns_top_n() {
        let reports: Vec<FitnessReport> = (0..10)
            .map(|i| FitnessReport {
                fitness_score: i as f32 * 0.1,
                confidence: 0.5,
                evidence: vec![],
            })
            .collect();
        let elite = GsoeEvolutionEngine::select_elite(&reports, 0.2);
        // 10 * 0.2 = 2,ceil = 2
        assert_eq!(elite.len(), 2);
        // 应是分数最高的两个(0.9 和 0.8)
        assert!((elite[0].fitness_score - 0.9).abs() < 1e-6);
        assert!((elite[1].fitness_score - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_select_elite_empty() {
        let elite = GsoeEvolutionEngine::select_elite(&[], 0.2);
        assert!(elite.is_empty());
    }

    #[test]
    fn test_select_elite_minimum_one() {
        let reports = vec![FitnessReport {
            fitness_score: 0.5,
            confidence: 0.5,
            evidence: vec![],
        }];
        let elite = GsoeEvolutionEngine::select_elite(&reports, 0.01);
        // 即使 elite_ratio 很小,至少保留 1 个
        assert_eq!(elite.len(), 1);
    }

    #[test]
    fn test_average_fitness_empty() {
        assert_eq!(GsoeEvolutionEngine::average_fitness(&[]), 0.0);
    }

    #[test]
    fn test_average_fitness_calculation() {
        let reports = vec![
            FitnessReport {
                fitness_score: 0.4,
                confidence: 0.5,
                evidence: vec![],
            },
            FitnessReport {
                fitness_score: 0.6,
                confidence: 0.5,
                evidence: vec![],
            },
        ];
        let avg = GsoeEvolutionEngine::average_fitness(&reports);
        assert!((avg - 0.5).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_evolve_once_basic() {
        let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
        let result = engine.evolve_once().await.unwrap();
        assert_eq!(result.generation, 1);
        assert_eq!(engine.generation(), 1);
    }

    #[tokio::test]
    async fn test_evolve_once_multiple_generations() {
        let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
        for i in 1..=5 {
            let result = engine.evolve_once().await.unwrap();
            assert_eq!(result.generation, i);
        }
        assert_eq!(engine.generation(), 5);
    }

    #[tokio::test]
    async fn test_evolve_once_max_generation_reached() {
        let config = GsoeConfig {
            max_generation: 2,
            ..Default::default()
        };
        let mut engine = GsoeEvolutionEngine::new(config);

        // 前两轮应成功
        engine.evolve_once().await.unwrap();
        engine.evolve_once().await.unwrap();

        // 第三轮应返回 MaxGenerationReached
        let result = engine.evolve_once().await;
        assert!(matches!(
            result,
            Err(GsoeError::MaxGenerationReached { .. })
        ));
    }

    #[tokio::test]
    async fn test_evolve_once_publishes_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

        engine.evolve_once().await.unwrap();

        // 应收到 GsoePolicyUpdated 事件
        let event = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .await
            .expect("接收事件超时");
        match event {
            NexusEvent::GsoePolicyUpdated {
                generation,
                improvement,
                new_mutation_rate,
                new_selection_pressure,
                ..
            } => {
                assert_eq!(generation, 1);
                assert!(improvement.is_finite());
                assert!((0.0..=1.0).contains(&new_mutation_rate));
                assert!(new_selection_pressure >= 0.0);
            }
            other => panic!("期望 GsoePolicyUpdated 事件,收到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_evolve_once_no_event_bus_succeeds() {
        // 无 EventBus 时进化仍应正常工作
        let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
        let result = engine.evolve_once().await.unwrap();
        assert_eq!(result.generation, 1);
    }

    #[tokio::test]
    async fn test_evolve_once_with_consensus_signal() {
        let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
        engine.handle_consensus_reached();
        engine.handle_consensus_reached();

        let result = engine.evolve_once().await.unwrap();
        assert_eq!(result.generation, 1);
        // 信号应在进化后被清除
        assert_eq!(engine.pending_consensus_count, 0);
    }

    #[tokio::test]
    async fn test_evolve_once_with_red_team_signal() {
        let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
        engine.handle_red_team_audit();

        let result = engine.evolve_once().await.unwrap();
        assert_eq!(result.generation, 1);
        // 信号应在进化后被清除
        assert_eq!(engine.pending_red_team_count, 0);
    }

    #[tokio::test]
    async fn test_evolve_once_event_source_is_gsoe() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

        engine.evolve_once().await.unwrap();

        let event = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(
            event.metadata().source,
            "gsoe-evolution",
            "事件 source 应为 gsoe-evolution"
        );
    }
}
