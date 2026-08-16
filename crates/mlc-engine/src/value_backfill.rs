//! MSCE 双信号价值回填 — Vt = αt·Rt + (1−αt)·γ·Vt+1（设计文档 §7.5）
//!
//! 对应架构层: **L2 Memory**（mlc-engine 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §7.5
//! 对应论文: MSCE（记忆-技能协同进化，双信号价值回填）
//! 对应 ADR: ADR-049 决策 1（msce-integration 落点 mlc-engine，内嵌模块）
//!
//! # 核心职责
//!
//! 记忆轨迹的价值逆向传播，融合两个信号：
//! - **环境反馈 Rt**: 外部可观测奖励（执行结果/评分）
//! - **反思信号 αt**: 自我反思的可靠度（决定 Rt 与后续价值 Vt+1 的权重）
//!
//! 公式: `Vt = αt · Rt + (1 − αt) · γ · Vt+1`
//! - αt ≈ 1: 反思可靠，主要依赖环境反馈
//! - αt ≈ 0: 反思不可靠，主要依赖后续价值传播
//! - γ: 折减因子（时间衰减）
//!
//! # 设计约束
//!
//! - **铁律1（零 Python）**: `ReflectionScorer` 用 Rust 规则/启发式评分
//!   （反思文本结构化程度 + 关键词 + 长度），模型评分留待后续（文档如实声明）；
//!   接口形态保持与规范一致（score(reflection) -> α）
//! - **f32 红线**: value/α/γ 为 f32，仅 PartialEq

// ============================================================
// L1 记忆轨迹
// ============================================================

/// L1 记忆轨迹 — MSCE L1 Trace（回填工作类型）
///
/// 对应 L0 `AtomicMemoryCard` 的 reflection/value 字段，作为价值回填的
/// 可变工作副本（回填结果可写回 `AtomicMemoryCard.value`）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct L1Trace {
    /// 反思文本（None = 无反思，α 取保守默认）
    pub reflection: Option<String>,
    /// 环境反馈 Rt（None = 无外部奖励，取 0.0）
    pub environmental_feedback: Option<f32>,
    /// 回填的价值 Vt（backfill_values 写入）
    pub value: Option<f32>,
}

// ============================================================
// 反思评分器
// ============================================================

/// 反思评分器 — α 评分（反思可靠度）
///
/// **铁律1 占位**: 规范原型用模型评分（ScoringModel.predict）；Rust 侧用
/// 规则/启发式评分，模型接线留待后续。评分维度：
/// - 反思长度（过短不可靠）
/// - 结构化关键词（"因为/所以/学到/改进/原因"等因果/总结词）
/// - 明确结论标记
#[derive(Clone, Debug, Default)]
pub struct ReflectionScorer {
    /// 最小可靠长度（字符数，低于此值 α 折减）
    pub min_reliable_length: usize,
}

impl ReflectionScorer {
    /// 创建反思评分器（默认最小可靠长度 20 字符）
    pub fn new() -> Self {
        Self {
            min_reliable_length: 20,
        }
    }

    /// α 评分 — 反思可靠度 [0.0, 1.0]
    ///
    /// 规则评分（Rust 启发式，铁律1）：
    /// - 空/过短反思 → 低 α（不可靠）
    /// - 含因果/总结关键词 → 高 α
    /// - 长度达标 → 基础 α 提升
    pub fn score(&self, reflection: &str) -> f32 {
        let trimmed = reflection.trim();
        if trimmed.is_empty() {
            return 0.0;
        }
        let mut alpha = 0.0f32;
        // 长度维度: 达标得基础分（sigmoid 近似，线性 clamp）
        let length_score =
            (trimmed.chars().count() as f32 / self.min_reliable_length as f32).min(1.0) * 0.5;
        alpha += length_score;
        // 结构化关键词维度: 因果/总结词命中加分
        let causal_keywords = [
            "因为",
            "所以",
            "学到",
            "改进",
            "原因",
            "导致",
            "应该",
            "下次",
            "反思",
            "总结",
            "because",
            "therefore",
            "learned",
            "improve",
            "cause",
            "should",
        ];
        let hits = causal_keywords
            .iter()
            .filter(|kw| trimmed.contains(**kw))
            .count();
        let keyword_score = (hits as f32 * 0.15).min(0.5);
        alpha += keyword_score;
        alpha.clamp(0.0, 1.0)
    }
}

// ============================================================
// 双信号价值回填
// ============================================================

/// 双信号价值回填 — MSCE 核心价值传播
#[derive(Clone, Debug)]
pub struct DualSignalBackfill {
    /// 折减因子 γ（时间衰减，常见 0.9-0.99）
    pub gamma: f32,
    /// 反思评分器（α 计算）
    pub reflection_scorer: ReflectionScorer,
    /// 无反思时的保守默认 α（低值 → 更依赖后续价值传播）
    pub default_alpha: f32,
}

impl DualSignalBackfill {
    /// 创建回填器
    ///
    /// - `gamma`: 折减因子 [0,1]
    pub fn new(gamma: f32) -> Self {
        Self {
            gamma: gamma.clamp(0.0, 1.0),
            reflection_scorer: ReflectionScorer::new(),
            default_alpha: 0.3,
        }
    }

    /// 自定义反思评分器
    pub fn with_scorer(mut self, scorer: ReflectionScorer) -> Self {
        self.reflection_scorer = scorer;
        self
    }

    /// 价值逆向回填 — Vt = αt·Rt + (1−αt)·γ·Vt+1（从尾到头传播）
    ///
    /// 遍历 traces 逆序，逐条计算 Vt 并写入 `trace.value`。
    /// 末尾轨迹 Vt+1 = 0（无后续价值）。
    pub fn backfill_values(&self, traces: &mut [L1Trace]) {
        let mut next_value = 0.0f32;
        for trace in traces.iter_mut().rev() {
            // α: 反思可靠度（无反思取保守默认）
            let alpha = match trace.reflection.as_ref() {
                Some(reflection) => self.reflection_scorer.score(reflection),
                None => self.default_alpha,
            };
            // Rt: 环境反馈（无则 0.0）
            let rt = trace.environmental_feedback.unwrap_or(0.0);
            // Vt = α·Rt + (1−α)·γ·Vt+1
            let vt = alpha * rt + (1.0 - alpha) * self.gamma * next_value;
            trace.value = Some(vt);
            next_value = vt;
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(reflection: Option<&str>, feedback: Option<f32>) -> L1Trace {
        L1Trace {
            reflection: reflection.map(String::from),
            environmental_feedback: feedback,
            value: None,
        }
    }

    #[test]
    fn backfill_single_trace_uses_environment_feedback() {
        let backfill = DualSignalBackfill::new(0.9);
        // 单轨迹: Vt+1 = 0，故 Vt = α·Rt
        let mut traces = vec![trace(
            Some("因为类型不匹配所以报错，学到了类型标注的重要性"),
            Some(1.0),
        )];
        backfill.backfill_values(&mut traces);
        let v = traces[0].value.expect("已回填");
        // α > 0（有反思），Rt=1.0，Vt+1=0 → Vt = α·1.0 ∈ (0, 1]
        assert!(v > 0.0 && v <= 1.0, "Vt 应在 (0,1]（实际 {})", v);
    }

    #[test]
    fn backfill_gamma_discounts_future_value() {
        // γ 折减: 后续价值向早期传播时衰减
        let backfill = DualSignalBackfill::new(0.5);
        let mut traces = vec![
            trace(None, Some(0.0)), // t=0（最早）
            trace(None, Some(1.0)), // t=1（最晚）
        ];
        backfill.backfill_values(&mut traces);
        // 末尾 t=1: Vt = α·1.0 + (1-α)·γ·0 = α·1.0（α=default_alpha=0.3）= 0.3
        let v1 = traces[1].value.expect("已回填");
        assert!((v1 - 0.3).abs() < 1e-6, "末尾 Vt=α·Rt=0.3（实际 {})", v1);
        // t=0: Vt = α·0 + (1-α)·γ·V1 = 0.7·0.5·0.3 = 0.105
        let v0 = traces[0].value.expect("已回填");
        assert!((v0 - 0.105).abs() < 1e-6, "γ 折减后 V0=0.105（实际 {})", v0);
    }

    #[test]
    fn alpha_one_pure_environment_feedback() {
        // α=1: 纯环境反馈（Vt = Rt，不依赖后续价值）
        let scorer = ReflectionScorer::new();
        // 构造必然 α=1 的反思（长度达标 + 足够关键词）
        let strong_reflection = "因为A所以B，学到C，改进D，原因E，应该F，总结G，反思H，下次I";
        let alpha = scorer.score(strong_reflection);
        assert!((alpha - 1.0).abs() < 1e-6, "强反思应 α=1（实际 {})", alpha);
    }

    #[test]
    fn alpha_zero_empty_reflection() {
        let scorer = ReflectionScorer::new();
        assert_eq!(scorer.score(""), 0.0, "空反思 α=0");
        assert_eq!(scorer.score("   "), 0.0, "空白反思 α=0");
    }

    #[test]
    fn reflection_scorer_keyword_boost() {
        let scorer = ReflectionScorer::new();
        let plain = "这是一段足够长的反思文本但没有关键词";
        let causal = "这是一段足够长的反思文本，因为是类型错误所以修复了";
        let alpha_plain = scorer.score(plain);
        let alpha_causal = scorer.score(causal);
        assert!(
            alpha_causal > alpha_plain,
            "含因果关键词应 α 更高（{} vs {})",
            alpha_causal,
            alpha_plain
        );
    }

    #[test]
    fn backfill_empty_traces_no_panic() {
        let backfill = DualSignalBackfill::new(0.9);
        let mut traces: Vec<L1Trace> = vec![];
        backfill.backfill_values(&mut traces); // 不应 panic
        assert!(traces.is_empty());
    }

    #[test]
    fn backfill_no_reflection_uses_default_alpha() {
        let backfill = DualSignalBackfill::new(1.0); // γ=1 无折减
        let mut traces = vec![trace(None, Some(1.0))];
        backfill.backfill_values(&mut traces);
        // α=default_alpha=0.3, γ=1, Vt+1=0 → Vt = 0.3·1.0 = 0.3
        let v = traces[0].value.expect("已回填");
        assert!((v - 0.3).abs() < 1e-6, "无反思用默认 α=0.3（实际 {})", v);
    }

    #[test]
    fn backfill_chain_propagates_value_backward() {
        // 价值从尾向头传播链
        let backfill = DualSignalBackfill::new(0.9);
        let mut traces = vec![
            trace(None, Some(0.0)),
            trace(None, Some(0.0)),
            trace(None, Some(1.0)), // 尾
        ];
        backfill.backfill_values(&mut traces);
        // 所有 value 应已填充
        for t in &traces {
            assert!(t.value.is_some(), "全部轨迹应回填价值");
        }
        // 越早的轨迹价值越低（折减累积）
        let v0 = traces[0].value.unwrap();
        let v1 = traces[1].value.unwrap();
        let v2 = traces[2].value.unwrap();
        assert!(v2 >= v1, "尾部价值应不低于中部");
        assert!(v1 >= v0, "中部价值应不低于头部");
    }

    #[test]
    fn gamma_clamped_to_valid_range() {
        let backfill = DualSignalBackfill::new(1.5); // 超范围
        assert_eq!(backfill.gamma, 1.0, "γ 应 clamp 到 1.0");
        let backfill2 = DualSignalBackfill::new(-0.5);
        assert_eq!(backfill2.gamma, 0.0, "γ 应 clamp 到 0.0");
    }
}
