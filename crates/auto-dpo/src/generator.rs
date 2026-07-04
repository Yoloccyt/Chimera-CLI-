//! AutoDPO 生成器主逻辑 — 偏好对生成、质量门控与事件发布
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:无(知识层辅助模块,服务于 GSOE 进化闭环)
//!
//! # 设计决策(WHY)
//! - `pair_counter` 用 `AtomicU64`:无锁计数,生成 pair_id,避免引入 uuid 依赖
//! - `event_bus` 为 `EventBus`:发布 `DpoPairGenerated` 事件,供 GSOE/Parliament 消费
//! - generate 方法选择最高分为 chosen、最低分为 rejected,确保偏好信号最强

use std::sync::atomic::{AtomicU64, Ordering};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{info, warn};

use crate::config::AutoDpoConfig;
use crate::error::AutoDpoError;
use crate::types::{ModelOutput, PreferencePair};

/// 偏好对生成器 — 自动构造 DPO 训练样本
///
/// 维护配置与计数器,提供:
/// - 偏好对生成(从候选中选 chosen/rejected)
/// - 质量门控(过滤低质量候选)
/// - 偏好对验证(格式与逻辑校验)
/// - 事件发布(通过 EventBus 广播生成结果)
///
/// # 线程安全
/// - `pair_counter` 用 `AtomicU64` 保护,无锁计数
/// - `event_bus` 为 `EventBus`(内部 `Arc` 引用计数,Clone 廉价)
pub struct PreferencePairGenerator {
    /// 生成器配置(只读,构造后不变)
    config: AutoDpoConfig,
    /// 偏好对 ID 计数器(无锁递增)
    pair_counter: AtomicU64,
    /// 事件总线(发布 DpoPairGenerated 事件)
    ///
    /// WHY:Ω-Event 定律要求所有状态变更经 EventBus 广播。
    /// `EventBus` 内部为 `Arc<broadcast::Sender>`,Clone 廉价。
    event_bus: EventBus,
}

impl PreferencePairGenerator {
    /// 创建新的偏好对生成器(内部创建私有 EventBus,仅用于测试)
    ///
    /// WHY 保留 `new()`:测试代码用 `new()` 创建私有总线,`publish` 静默丢弃,
    /// 不影响测试逻辑。生产代码改用 [`with_event_bus`](Self::with_event_bus) 注入共享总线。
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败
    pub fn new(config: AutoDpoConfig) -> Result<Self, AutoDpoError> {
        Self::with_event_bus(config, EventBus::new())
    }

    /// 创建带共享 EventBus 的偏好对生成器(生产代码推荐)
    ///
    /// WHY:生产代码需注入共享总线,使 `DpoPairGenerated` 事件能被 GSOE/Parliament 订阅。
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败
    pub fn with_event_bus(config: AutoDpoConfig, bus: EventBus) -> Result<Self, AutoDpoError> {
        config.validate()?;
        Ok(Self {
            config,
            pair_counter: AtomicU64::new(0),
            event_bus: bus,
        })
    }

    /// EventBus 访问器(供测试与上层共享总线)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 返回配置引用(测试与监控用)
    pub fn config(&self) -> &AutoDpoConfig {
        &self.config
    }

    /// 从模型输出候选中生成偏好对
    ///
    /// # 流程
    /// 1. 校验候选数 >= min_samples
    /// 2. 选最高分候选为 chosen,最低分候选为 rejected
    /// 3. 质量门控:仅检查 chosen 是否 >= quality_threshold
    ///    (WHY:rejected 本就是"不偏好"的输出,可以是低质量,无需门控)
    /// 4. 校验 chosen_score > rejected_score(确保有偏好信号)
    /// 5. 构造 PreferencePair,发布 DpoPairGenerated 事件
    ///
    /// # 错误
    /// - `InsufficientSamples`:候选数少于 min_samples
    /// - `QualityTooLow`:chosen(最高分)低于质量阈值
    /// - `GenerationFailed`:chosen 与 rejected 分数相同(无偏好信号)
    pub fn generate(&self, outputs: &[ModelOutput]) -> Result<PreferencePair, AutoDpoError> {
        // 步骤 1:校验候选数
        if outputs.len() < self.config.min_samples {
            return Err(AutoDpoError::InsufficientSamples {
                actual: outputs.len(),
            });
        }

        // 步骤 2:选最高分为 chosen,最低分为 rejected
        // WHY 手动遍历而非 sort:候选数通常较小(O(n) 比 O(n log n) 更优),
        // 且避免修改输入切片
        let threshold = self.config.quality_threshold;
        let mut chosen = &outputs[0];
        let mut rejected = &outputs[0];
        for output in &outputs[1..] {
            if output.score > chosen.score {
                chosen = output;
            }
            if output.score < rejected.score {
                rejected = output;
            }
        }

        // 步骤 3:质量门控 — 仅检查 chosen 是否达到阈值
        // WHY 仅检查 chosen:DPO 的 rejected 本就是"不偏好"的低质量输出,
        // 可以低于阈值;质量门控的目的是确保 chosen 作为训练正样本足够好
        if chosen.score < threshold {
            return Err(AutoDpoError::QualityTooLow {
                threshold,
                best_score: chosen.score,
            });
        }

        // 步骤 4:校验偏好信号(chosen 必须严格优于 rejected)
        if chosen.score <= rejected.score {
            return Err(AutoDpoError::GenerationFailed {
                reason: format!(
                    "chosen score ({}) must be > rejected score ({})",
                    chosen.score, rejected.score
                ),
            });
        }

        // 步骤 5:构造偏好对
        let pair_id = self.next_pair_id();
        let pair = PreferencePair::new(
            &pair_id,
            &chosen.text,
            &rejected.text,
            chosen.score,
            rejected.score,
        );

        info!(
            pair_id = %pair_id,
            quality = %pair.quality,
            score_gap = pair.score_gap(),
            "DPO preference pair generated"
        );

        // 发布 DpoPairGenerated 事件(Ω-Event 定律)
        // WHY publish_blocking:generate 为同步方法,且被同步 #[test] 调用;
        // publish_blocking 不依赖 tokio 运行时,同步测试不 panic。
        if self.config.enable_event_publish {
            let event = NexusEvent::DpoPairGenerated {
                metadata: EventMetadata::new("auto-dpo"),
                pair_id: pair_id.clone(),
                chosen: pair.chosen.clone(),
                rejected: pair.rejected.clone(),
            };
            if let Err(e) = self.event_bus.publish_blocking(event) {
                warn!(error = %e, "发布 DpoPairGenerated 事件失败");
            }
        }

        Ok(pair)
    }

    /// 验证偏好对的格式与逻辑合法性
    ///
    /// # 校验规则
    /// - chosen 与 rejected 非空
    /// - chosen_score > rejected_score(有偏好信号)
    /// - chosen_score ∈ [0.0, 1.0]
    /// - quality 为可接受质量(非 Low)
    /// - pair_id 非空
    pub fn validate(&self, pair: &PreferencePair) -> Result<(), AutoDpoError> {
        if pair.pair_id.is_empty() {
            return Err(AutoDpoError::GenerationFailed {
                reason: "pair_id is empty".into(),
            });
        }
        if pair.chosen.is_empty() {
            return Err(AutoDpoError::GenerationFailed {
                reason: "chosen is empty".into(),
            });
        }
        if pair.rejected.is_empty() {
            return Err(AutoDpoError::GenerationFailed {
                reason: "rejected is empty".into(),
            });
        }
        if pair.chosen_score <= pair.rejected_score {
            return Err(AutoDpoError::GenerationFailed {
                reason: format!(
                    "chosen_score ({}) must be > rejected_score ({})",
                    pair.chosen_score, pair.rejected_score
                ),
            });
        }
        if pair.chosen_score.is_nan() || !(0.0..=1.0).contains(&pair.chosen_score) {
            return Err(AutoDpoError::GenerationFailed {
                reason: format!(
                    "chosen_score must be in [0.0, 1.0], got {}",
                    pair.chosen_score
                ),
            });
        }
        if !pair.quality.is_acceptable() {
            return Err(AutoDpoError::QualityTooLow {
                threshold: self.config.quality_threshold,
                best_score: pair.chosen_score,
            });
        }
        Ok(())
    }

    /// 生成下一个偏好对 ID
    fn next_pair_id(&self) -> String {
        let counter = self.pair_counter.fetch_add(1, Ordering::Relaxed);
        format!("dpo-pair-{counter}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_generator() -> PreferencePairGenerator {
        PreferencePairGenerator::new(AutoDpoConfig::default()).unwrap()
    }

    // ============================================================
    // 偏好对生成测试
    // ============================================================

    #[test]
    fn test_generate_valid_pair() {
        let generator = make_generator();
        let outputs = vec![
            ModelOutput::new("good-output", 0.9),
            ModelOutput::new("bad-output", 0.3),
        ];
        let pair = generator.generate(&outputs).unwrap();
        assert_eq!(pair.chosen, "good-output");
        assert_eq!(pair.rejected, "bad-output");
        assert!((pair.chosen_score - 0.9).abs() < 1e-6);
        assert!((pair.rejected_score - 0.3).abs() < 1e-6);
        assert!(pair.score_gap() > 0.0);
    }

    #[test]
    fn test_generate_insufficient_samples() {
        let generator = make_generator();
        let outputs = vec![ModelOutput::new("only-one", 0.9)];
        let result = generator.generate(&outputs);
        assert!(
            matches!(result, Err(AutoDpoError::InsufficientSamples { actual: 1 })),
            "should reject with InsufficientSamples"
        );
    }

    #[test]
    fn test_generate_quality_too_low() {
        let generator = make_generator();
        // 两个候选都低于阈值 0.5
        let outputs = vec![
            ModelOutput::new("low-a", 0.2),
            ModelOutput::new("low-b", 0.3),
        ];
        let result = generator.generate(&outputs);
        assert!(
            matches!(result, Err(AutoDpoError::QualityTooLow { .. })),
            "should reject with QualityTooLow"
        );
    }

    #[test]
    fn test_generate_picks_extreme_scores_without_filtering() {
        // WHY 不过滤:DPO 语义要求 rejected 本就是"不偏好"的低质量输出,
        // 质量门控只确保 chosen 达到阈值,rejected 可以低于阈值。
        // 此测试验证:3 个候选中,chosen=最高分,rejected=最低分(无过滤)。
        let generator = make_generator();
        let outputs = vec![
            ModelOutput::new("high", 0.9),
            ModelOutput::new("medium", 0.6),
            ModelOutput::new("low", 0.2), // 成为 rejected(不被过滤)
        ];
        let pair = generator.generate(&outputs).unwrap();
        // chosen 应为 high(0.9),rejected 应为 low(0.2),medium 居中不入选
        assert_eq!(pair.chosen, "high");
        assert_eq!(pair.rejected, "low");
        assert!((pair.chosen_score - 0.9).abs() < 1e-6);
        assert!((pair.rejected_score - 0.2).abs() < 1e-6);
    }

    #[test]
    fn test_generate_pair_id_increments() {
        let generator = make_generator();
        let outputs = vec![ModelOutput::new("a", 0.9), ModelOutput::new("b", 0.3)];
        let pair1 = generator.generate(&outputs).unwrap();
        let pair2 = generator.generate(&outputs).unwrap();
        assert_ne!(pair1.pair_id, pair2.pair_id);
        assert!(pair1.pair_id.starts_with("dpo-pair-"));
        assert!(pair2.pair_id.starts_with("dpo-pair-"));
    }

    // ============================================================
    // 偏好对验证测试
    // ============================================================

    #[test]
    fn test_validate_valid_pair() {
        let generator = make_generator();
        let pair = PreferencePair::new("pair-1", "good", "bad", 0.9, 0.2);
        assert!(generator.validate(&pair).is_ok());
    }

    #[test]
    fn test_validate_empty_pair_id() {
        let generator = make_generator();
        let pair = PreferencePair::new("", "good", "bad", 0.9, 0.2);
        assert!(generator.validate(&pair).is_err());
    }

    #[test]
    fn test_validate_empty_chosen() {
        let generator = make_generator();
        let pair = PreferencePair::new("pair-1", "", "bad", 0.9, 0.2);
        assert!(generator.validate(&pair).is_err());
    }

    #[test]
    fn test_validate_chosen_score_not_greater() {
        let generator = make_generator();
        // chosen_score <= rejected_score,无偏好信号
        let pair = PreferencePair::new("pair-1", "a", "b", 0.5, 0.5);
        assert!(generator.validate(&pair).is_err());
    }

    #[test]
    fn test_validate_chosen_score_out_of_range() {
        let generator = make_generator();
        let pair = PreferencePair::new("pair-1", "a", "b", 1.5, 0.2);
        assert!(generator.validate(&pair).is_err());
    }

    #[test]
    fn test_validate_low_quality_pair() {
        let generator = make_generator();
        // chosen_score 0.4 < 0.5,质量为 Low
        let pair = PreferencePair::new("pair-1", "a", "b", 0.4, 0.2);
        assert!(generator.validate(&pair).is_err());
    }

    // ============================================================
    // 配置错误测试
    // ============================================================

    #[test]
    fn test_invalid_config_rejected() {
        let config = AutoDpoConfig {
            min_samples: 1,
            ..Default::default()
        };
        assert!(PreferencePairGenerator::new(config).is_err());
    }
}
