//! 协调成本/推理增益比值度量 — 三重悖论推理悖论红线度量体系
//!
//! 对应架构层:L9 Quest
//! 对应分析:P2-1(三重悖论推理悖论红线 — 协调成本/推理增益比值未度量)
//!
//! # 核心设计
//! - `CoordinationCostSample`:记录跨层通信/TTG切换/议会审议/委托开销等协调成本
//! - `InferenceGainSample`:记录任务成功率/质量分数/共识质量等推理增益
//! - `CoordinationToGainRatio`:归一化后的成本/增益比值,ratio > threshold 表示推理悖论风险
//! - `CoordinationMetricsCollector`:线程安全收集器,累积样本并计算比值
//!
//! # 归一化方法
//! 协调成本(ms)和推理增益(分数 [0,1])单位不同,采用"成本指数"归一化:
//! - `cost_index = min(total_ms / baseline, 1.0)`,`baseline` 默认 1000ms(P1-5:支持自适应基线)
//! - `gain_index = inference_gain`(已经是 [0,1])
//! - `ratio = cost_index / gain_index`(`gain_index` 为 0 时 `ratio = INFINITY`)
//!
//! # P1-5: 自适应基线
//! 新增 `adaptive_baseline_enabled` / `min_baseline_ms` / `max_baseline_ms` 配置。
//! 当启用时,`cost_index` 使用 `adaptive_baseline(complexity_score)` 计算:
//! - `normalized = sigmoid((score - 3.0) / 1.5)` 将 ComplexityScore [0,10] 映射到 [0,1]
//! - `baseline = min_baseline + (max_baseline - min_baseline) * normalized`
//!
//! 当禁用时,回退到 `cost_baseline_ms` 静态基准。
//!
//! # 推理悖论阈值
//! `ratio > threshold`(默认 1.0)表示协调成本超过推理增益,触发推理悖论风险告警。
//! 这对应三重悖论推理悖论红线:"当协调成本超过推理增益时,多 Agent 反而不如单 Agent"。
//!
//! # 时间复杂度
//! - `record_cost` / `record_gain`:O(1) amortized(Vec push)
//! - `compute_ratio`:O(1)(使用 EWMA 增量计算,非全量遍历)

use std::sync::Mutex;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use serde::{Deserialize, Serialize};

use crate::ComplexityScore;

// ============================================================
// 配置类型
// ============================================================

/// 协调度量配置 — 控制归一化基准与推理悖论告警阈值
///
/// WHY 独立于 `QuestConfig`:协调度量是跨 Quest 的全局观测指标,
/// 而 `QuestConfig` 是单 Quest 的分解/检查点配置,语义层级不同。
///
/// # P1-5: 自适应基线
///
/// 新增 `adaptive_baseline_enabled` / `min_baseline_ms` / `max_baseline_ms` 配置。
/// 当启用时,`cost_index` 根据 `ComplexityScore` 自适应计算基线:
/// - 简单 Quest(score≈0):基线 ≈ `min_baseline_ms`(200ms)
/// - 中等 Quest(score≈3):基线 ≈ 中间值(~1100ms)
/// - 复杂 Quest(score≥10):基线 ≈ `max_baseline_ms`(2000ms)
///
/// 当禁用时,回退到 `cost_baseline_ms` 静态基准(1000ms)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationMetricsConfig {
    /// 推理悖论告警阈值(默认 1.0)
    ///
    /// `ratio > threshold` 时 `is_paradox_risk = true`。
    /// WHY 默认 1.0:成本指数 = 增益指数时为临界点,超过即协调成本超过推理增益。
    /// 可调低至 0.8(更敏感)或调高至 1.5(更宽松),取决于业务场景。
    pub paradox_threshold: f64,

    /// 成本归一化基准(毫秒,默认 1000ms = 1s)
    ///
    /// `cost_index = min(total_ms / baseline, 1.0)`。
    /// 当 `adaptive_baseline_enabled = false` 时使用此静态基准。
    /// WHY 默认 1000ms:典型 Quest 生命周期(分解+调度+执行)的协调开销基线,
    /// 超过 1s 的协调成本视为满载(指数 = 1.0)。
    pub cost_baseline_ms: f64,

    /// EWMA 衰减系数(默认 0.3,范围 [0.0, 1.0])
    ///
    /// 指数加权移动平均(Exponentially Weighted Moving Average)的衰减系数。
    /// `ewma = alpha * new_sample + (1 - alpha) * old_ewma`。
    /// WHY 默认 0.3:平衡历史趋势与新样本权重,避免单次异常抖动影响判断。
    /// alpha=1.0 仅看最新样本,alpha=0.0 完全忽略新样本。
    pub ewma_alpha: f64,

    /// 最大样本数上限(默认 u64::MAX,即无限制)
    ///
    /// P3-11: 防止 sample_count 无限增长。当 `sample_count >= max_samples` 时,
    /// 停止递增 `sample_count`(EWMA 仍正常更新,不影响比值计算)。
    ///
    /// WHY 默认无限制:保持向后兼容,现有代码不受影响。
    pub max_samples: u64,

    // ============================================================
    // P1-5: 自适应基线配置
    // ============================================================
    /// 是否启用自适应基线(默认 true)
    ///
    /// 启用时根据 `ComplexityScore` 动态计算 `cost_baseline_ms`,
    /// 禁用时使用 `cost_baseline_ms` 静态基准。
    pub adaptive_baseline_enabled: bool,

    /// 自适应基线最小值(毫秒,默认 200ms)
    ///
    /// 最低复杂度 Quest(ComplexityScore ≈ 0)的协调成本基线。
    /// WHY 200ms:简单 Quest(1 个任务、无依赖)的协调成本通常 <200ms,
    /// 设置过低会频繁触发推理悖论误报。
    pub min_baseline_ms: f64,

    /// 自适应基线最大值(毫秒,默认 2000ms)
    ///
    /// 最高复杂度 Quest(ComplexityScore ≥ 10)的协调成本基线。
    /// WHY 2000ms:复杂 Quest(10+任务、深层依赖图)的协调成本可达 2000ms+,
    /// 设置过高会掩盖真实的推理悖论风险。
    pub max_baseline_ms: f64,
}

impl Default for CoordinationMetricsConfig {
    fn default() -> Self {
        Self {
            paradox_threshold: 1.0,
            cost_baseline_ms: 1000.0,
            ewma_alpha: 0.3,
            // P3-11: 默认无上限(保持向后兼容)
            max_samples: u64::MAX,
            // P1-5: 默认启用自适应基线
            adaptive_baseline_enabled: true,
            min_baseline_ms: 200.0,
            max_baseline_ms: 2000.0,
        }
    }
}

impl CoordinationMetricsConfig {
    /// 创建自定义配置
    ///
    /// # 参数
    /// - `paradox_threshold`:推理悖论告警阈值
    /// - `cost_baseline_ms`:成本归一化基准(毫秒)
    /// - `ewma_alpha`:EWMA 衰减系数 [0.0, 1.0]
    pub fn new(paradox_threshold: f64, cost_baseline_ms: f64, ewma_alpha: f64) -> Self {
        Self {
            paradox_threshold,
            cost_baseline_ms,
            ewma_alpha: ewma_alpha.clamp(0.0, 1.0),
            ..Default::default()
        }
    }

    /// 创建带自定义阈值的配置(其他参数默认)
    pub fn with_threshold(paradox_threshold: f64) -> Self {
        Self {
            paradox_threshold,
            ..Default::default()
        }
    }

    /// 计算自适应基线 — 基于 ComplexityScore 的 sigmoid 归一化映射
    ///
    /// # 公式
    /// ```text
    /// normalized = sigmoid((score - 3.0) / 1.5)
    ///            = 1.0 / (1.0 + exp(-(score - 3.0) / 1.5))
    /// baseline = min_baseline + (max_baseline - min_baseline) * normalized
    /// ```
    ///
    /// # 参数
    /// - `score`:Quest 复杂度评分(来自 `ComplexityScore` 的原始值)
    ///
    /// # 返回
    /// 自适应基线值(毫秒),在 [min_baseline_ms, max_baseline_ms] 范围内。
    ///
    /// # 设计决策(WHY)
    /// - **sigmoid 而非线性映射**:复杂度评分在 [0,10] 范围,但低复杂度 Quest 更常见。
    ///   sigmoid 在 score=3.0 附近提供平滑过渡,使中等复杂度 Quest 的基线更合理。
    /// - **中点 3.0**:基于 TTG 的 evaluate_complexity 公式,一个 3 任务+1 层依赖+中等描述
    ///   的 Quest 评分约为 3.0,是典型"中等复杂度"的参考点。
    /// - **斜率 1.5**:使 sigmoid 在 [0,10] 范围内覆盖 [0.12, 0.99] 的归一化值,
    ///   避免极端值过早饱和。
    ///
    /// # 时间复杂度
    /// O(1)(仅含一次 f64::exp 调用)
    pub fn adaptive_baseline(&self, score: f64) -> f64 {
        if !self.adaptive_baseline_enabled {
            return self.cost_baseline_ms;
        }
        // sigmoid 归一化:将 score 映射到 [0, 1]
        // 当 score=0 → normalized ≈ 0.12(接近 min)
        // 当 score=3 → normalized ≈ 0.50(中点)
        // 当 score=10 → normalized ≈ 0.99(接近 max)
        let normalized = 1.0 / (1.0 + f64::exp(-(score - 3.0) / 1.5));
        let baseline =
            self.min_baseline_ms + (self.max_baseline_ms - self.min_baseline_ms) * normalized;
        // 钳制在 [min_baseline_ms, max_baseline_ms] 范围内(防御性编程)
        baseline.clamp(self.min_baseline_ms, self.max_baseline_ms)
    }

    /// 设置最大样本数上限(builder 模式)
    ///
    /// P3-11: 防止 sample_count 无限增长。当达到上限后停止递增 sample_count,
    /// EWMA 仍正常更新,不影响比值计算。
    ///
    /// # 参数
    /// - `max`:最大样本数上限
    pub fn with_max_samples(mut self, max: u64) -> Self {
        self.max_samples = max;
        self
    }

    /// 获取当前有效基线 — 根据自适应配置返回基线值
    ///
    /// # 参数
    /// - `complexity`:可选的复杂度评分。`Some(score)` 且自适应启用时返回自适应基线;
    ///   `None` 或自适应禁用时返回 `cost_baseline_ms`。
    ///
    /// # 时间复杂度
    /// O(1)
    pub fn effective_baseline(&self, complexity: Option<ComplexityScore>) -> f64 {
        match complexity {
            Some(score) if self.adaptive_baseline_enabled => {
                self.adaptive_baseline(score.value() as f64)
            }
            _ => self.cost_baseline_ms,
        }
    }
}

// ============================================================
// 协调成本样本
// ============================================================

/// 单次协调成本样本 — 记录 Quest 生命周期中各阶段的协调开销
///
/// 单位:毫秒(ms)。各字段为该阶段的实际延迟,`total_ms()` 返回总和。
///
/// # 设计决策
/// - 各阶段延迟分开记录(而非只记总和):便于诊断"哪个阶段是协调成本瓶颈"
/// - `parliament_debate_latency_ms` / `delegation_overhead_ms` 为 `Option`:
///   不是所有 Quest 都触发议会审议或多 Agent 委托
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CoordinationCostSample {
    /// Event Bus 消息发布到订阅者接收的延迟(ms)
    ///
    /// 包括 QuestCreated / QuestProgressUpdated / ExecutionCompleted 等事件的发布开销。
    pub event_bus_latency_ms: f64,

    /// TTG 思考模式切换延迟(ms)
    ///
    /// `select_mode_and_publish` + `apply_thinking_mode` 的执行时间。
    pub ttg_switch_latency_ms: f64,

    /// 议会审议延迟(ms,可选)
    ///
    /// Parliament 辩论 + 投票 + 共识达成的总时间。
    /// `None` 表示该 Quest 未触发议会审议。
    pub parliament_debate_latency_ms: Option<f64>,

    /// 多 Agent 委托开销(ms,可选)
    ///
    /// chimera-mas 委托执行的总时间(含子 Agent 通信 + 结果汇聚)。
    /// `None` 表示该 Quest 未使用多 Agent 委托。
    pub delegation_overhead_ms: Option<f64>,
}

impl CoordinationCostSample {
    /// 创建新的协调成本样本
    pub fn new(event_bus_latency_ms: f64, ttg_switch_latency_ms: f64) -> Self {
        Self {
            event_bus_latency_ms,
            ttg_switch_latency_ms,
            parliament_debate_latency_ms: None,
            delegation_overhead_ms: None,
        }
    }

    /// 设置议会审议延迟(builder 模式)
    pub fn with_parliament_debate(mut self, latency_ms: f64) -> Self {
        self.parliament_debate_latency_ms = Some(latency_ms);
        self
    }

    /// 设置多 Agent 委托开销(builder 模式)
    pub fn with_delegation_overhead(mut self, overhead_ms: f64) -> Self {
        self.delegation_overhead_ms = Some(overhead_ms);
        self
    }

    /// 计算总协调成本(ms)
    ///
    /// 时间复杂度:O(1)
    pub fn total_ms(&self) -> f64 {
        self.event_bus_latency_ms
            + self.ttg_switch_latency_ms
            + self.parliament_debate_latency_ms.unwrap_or(0.0)
            + self.delegation_overhead_ms.unwrap_or(0.0)
    }

    /// 归一化为成本指数 [0.0, 1.0]
    ///
    /// `cost_index = min(total_ms / cost_baseline_ms, 1.0)`
    ///
    /// 时间复杂度:O(1)
    pub fn cost_index(&self, cost_baseline_ms: f64) -> f64 {
        if cost_baseline_ms <= 0.0 {
            return 1.0; // 基准为 0 视为满载
        }
        (self.total_ms() / cost_baseline_ms).min(1.0)
    }
}

// ============================================================
// 推理增益样本
// ============================================================

/// 单次推理增益样本 — 记录 Quest 执行的推理质量
///
/// 各字段为 [0.0, 1.0] 的归一化分数,`total_gain()` 返回加权平均。
///
/// # 设计决策
/// - `task_success_rate` 为必填(核心指标),其余为 `Option`(可选增强)
/// - `quality_score` / `consensus_quality` 为 `Option`:
///   不是所有 Quest 都有 PVL 验证器质量分数或议会共识质量
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct InferenceGainSample {
    /// 任务成功率 [0.0, 1.0](必填)
    ///
    /// `completed_tasks / total_tasks`。0.0 表示全部失败,1.0 表示全部成功。
    pub task_success_rate: f32,

    /// PVL 验证器质量分数 [0.0, 1.0](可选)
    ///
    /// Producer-Verifier Loop 的验证评分均值。
    /// `None` 表示该 Quest 未启用 PVL 验证。
    pub quality_score: Option<f32>,

    /// 议会共识质量 [0.0, 1.0](可选)
    ///
    /// 共识决策的正确率(由下游反馈或专家复盘得出)。
    /// `None` 表示该 Quest 未触发议会审议。
    pub consensus_quality: Option<f32>,
}

impl InferenceGainSample {
    /// 创建新的推理增益样本
    pub fn new(task_success_rate: f32) -> Self {
        Self {
            task_success_rate: task_success_rate.clamp(0.0, 1.0),
            quality_score: None,
            consensus_quality: None,
        }
    }

    /// 设置 PVL 验证器质量分数(builder 模式)
    pub fn with_quality_score(mut self, score: f32) -> Self {
        self.quality_score = Some(score.clamp(0.0, 1.0));
        self
    }

    /// 设置议会共识质量(builder 模式)
    pub fn with_consensus_quality(mut self, quality: f32) -> Self {
        self.consensus_quality = Some(quality.clamp(0.0, 1.0));
        self
    }

    /// 计算总推理增益 [0.0, 1.0]
    ///
    /// 加权平均:`task_success_rate` 权重 0.5,`quality_score` 权重 0.3,
    /// `consensus_quality` 权重 0.2。缺失字段权重重新归一化。
    ///
    /// 时间复杂度:O(1)
    pub fn total_gain(&self) -> f32 {
        // 权重:task_success_rate=0.5, quality_score=0.3, consensus_quality=0.2
        // 缺失字段时,将其权重分配给 task_success_rate(核心指标)
        let (w_success, w_quality, w_consensus) = match (self.quality_score, self.consensus_quality)
        {
            (Some(_), Some(_)) => (0.5_f32, 0.3_f32, 0.2_f32),
            (Some(_), None) => (0.7_f32, 0.3_f32, 0.0_f32),
            (None, Some(_)) => (0.7_f32, 0.0_f32, 0.3_f32),
            (None, None) => (1.0_f32, 0.0_f32, 0.0_f32),
        };

        let total = w_success * self.task_success_rate
            + w_quality * self.quality_score.unwrap_or(0.0)
            + w_consensus * self.consensus_quality.unwrap_or(0.0);

        total.clamp(0.0, 1.0)
    }

    /// 归一化为增益指数 [0.0, 1.0](即 `total_gain()`)
    pub fn gain_index(&self) -> f64 {
        self.total_gain() as f64
    }
}

// ============================================================
// 协调成本/推理增益比值
// ============================================================

/// 协调成本/推理增益比值 — 推理悖论风险度量结果
///
/// 归一化后的成本/增益比值,`ratio > threshold` 表示推理悖论风险。
///
/// # 字段说明
/// - `coordination_cost_ms`:原始协调成本(毫秒)
/// - `inference_gain`:原始推理增益 [0.0, 1.0]
/// - `cost_index`:归一化成本指数 [0.0, 1.0]
/// - `gain_index`:归一化增益指数 [0.0, 1.0]
/// - `ratio`:`cost_index / gain_index`(增益为 0 时为 `f64::INFINITY`)
/// - `is_paradox_risk`:`ratio > threshold` 时为 `true`
/// - `threshold`:告警阈值
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoordinationToGainRatio {
    /// 原始协调成本(毫秒)
    pub coordination_cost_ms: f64,

    /// 原始推理增益 [0.0, 1.0]
    pub inference_gain: f32,

    /// 归一化成本指数 [0.0, 1.0]
    pub cost_index: f64,

    /// 归一化增益指数 [0.0, 1.0]
    pub gain_index: f64,

    /// 协调成本/推理增益比值
    ///
    /// `cost_index / gain_index`。`gain_index = 0` 时为 `f64::INFINITY`。
    pub ratio: f64,

    /// 是否触发推理悖论风险(`ratio > threshold`)
    pub is_paradox_risk: bool,

    /// 推理悖论告警阈值
    pub threshold: f64,
}

impl CoordinationToGainRatio {
    /// 从协调成本样本和推理增益样本计算比值
    ///
    /// 时间复杂度:O(1)
    ///
    /// # 参数
    /// - `cost`:协调成本样本
    /// - `gain`:推理增益样本
    /// - `config`:度量配置(提供归一化基准与阈值)
    /// - `complexity`:可选的复杂度评分(P1-5:自适应基线用)
    pub fn compute(
        cost: &CoordinationCostSample,
        gain: &InferenceGainSample,
        config: &CoordinationMetricsConfig,
        complexity: Option<ComplexityScore>,
    ) -> Self {
        let coordination_cost_ms = cost.total_ms();
        let inference_gain = gain.total_gain();
        let baseline = config.effective_baseline(complexity);
        let cost_index = cost.cost_index(baseline);
        let gain_index = gain.gain_index();

        let ratio = if gain_index > 0.0 {
            cost_index / gain_index
        } else {
            f64::INFINITY // 增益为 0 时比值无穷大(推理悖论必然触发)
        };

        let is_paradox_risk = ratio > config.paradox_threshold;

        Self {
            coordination_cost_ms,
            inference_gain,
            cost_index,
            gain_index,
            ratio,
            is_paradox_risk,
            threshold: config.paradox_threshold,
        }
    }

    /// 获取比值的人类可读描述(用于日志与 TUI 展示)
    pub fn description(&self) -> String {
        let risk_label = if self.is_paradox_risk {
            "⚠️ 推理悖论风险"
        } else {
            "✅ 正常"
        };
        format!(
            "{}: ratio={:.3} (cost_index={:.3}, gain_index={:.3}, cost={:.1}ms, gain={:.3}, threshold={:.3})",
            risk_label, self.ratio, self.cost_index, self.gain_index,
            self.coordination_cost_ms, self.inference_gain, self.threshold
        )
    }
}

// ============================================================
// 协调度量收集器
// ============================================================

/// 协调度量收集器内部状态(EWMA 增量计算)
#[derive(Debug, Clone)]
struct MetricsState {
    /// 协调成本 EWMA(毫秒)
    cost_ewma_ms: f64,

    /// 推理增益 EWMA [0.0, 1.0]
    gain_ewma: f64,

    /// 已采集样本数
    sample_count: u64,

    /// 最近一次计算的比值(供查询)
    last_ratio: Option<CoordinationToGainRatio>,
}

impl Default for MetricsState {
    fn default() -> Self {
        Self {
            cost_ewma_ms: 0.0,
            gain_ewma: 0.0,
            sample_count: 0,
            last_ratio: None,
        }
    }
}

/// 协调度量收集器 — 线程安全累积样本并计算协调成本/推理增益比值
///
/// 使用 `std::sync::Mutex` 保证线程安全(操作不跨 `.await` 点)。
/// EWMA 增量计算避免全量遍历,每次 `record` 为 O(1)。
///
/// # 使用方式
/// 1. `record_cost` / `record_gain`:记录单次协调成本/推理增益样本
/// 2. `compute_and_store_ratio`:计算当前 EWMA 比值并存储
/// 3. `last_ratio`:查询最近一次计算的比值
/// 4. `snapshot`:获取当前 EWMA 状态快照
///
/// # 事件发布(P2-1 后续增强)
/// 若通过 `with_event_bus` 绑定了 EventBus,`record_and_compute` 会在计算比值后
/// 自动发布 `NexusEvent::CoordinationRatioReported` 事件,供 efficiency-monitor
/// 等订阅者消费。使用 `publish_blocking`(同步发布)而非 `publish().await`,
/// 因为本收集器所有方法为同步方法(§4.4 反模式 8)。
///
/// # 并发安全
/// 遵循 §4.4 反模式 #1(禁止持锁 .await):所有方法为同步方法,
/// `Mutex` 锁在方法返回时释放,不跨 `.await` 点。
pub struct CoordinationMetricsCollector {
    /// 内部状态(Mutex 保护)
    state: Mutex<MetricsState>,
    /// 度量配置(只读,Clone 廉价)
    config: CoordinationMetricsConfig,
    /// 可选事件总线(P2-1 后续增强:发布 CoordinationRatioReported 事件)
    ///
    /// WHY Option:收集器在单元测试中可独立使用(无 EventBus),生产环境
    /// 通过 `with_event_bus` 注入总线。EventBus 内部为 Arc,Clone 廉价。
    event_bus: Option<EventBus>,
}

impl CoordinationMetricsCollector {
    /// 创建新的度量收集器(使用默认配置,无 EventBus)
    pub fn new() -> Self {
        Self::with_config(CoordinationMetricsConfig::default())
    }

    /// 创建带自定义配置的度量收集器(无 EventBus)
    pub fn with_config(config: CoordinationMetricsConfig) -> Self {
        Self {
            state: Mutex::new(MetricsState::default()),
            config,
            event_bus: None,
        }
    }

    /// 创建带 EventBus 的度量收集器(P2-1 后续增强)
    ///
    /// 绑定后,`record_and_compute` 会在每次计算比值后自动发布
    /// `NexusEvent::CoordinationRatioReported` 事件。
    ///
    /// # 参数
    /// - `config`:度量配置(归一化基准与告警阈值)
    /// - `bus`:事件总线(EventBus 内部为 Arc,Clone 廉价)
    pub fn with_event_bus(config: CoordinationMetricsConfig, bus: EventBus) -> Self {
        Self {
            state: Mutex::new(MetricsState::default()),
            config,
            event_bus: Some(bus),
        }
    }

    /// 获取内部状态锁(毒锁降级恢复)
    ///
    /// WHY unwrap_or_else 而非 expect:指标采集非关键路径,前任持有者 panic
    /// 导致 poison 后,继续抛 panic 会把崩溃传染给 Quest 主流程(§4.1 红线:
    /// 避免 unwrap/expect)。降级访问中毒数据更稳健:MetricsState 仅含 EWMA
    /// 标量与计数器,即使前任持有者在写入中途 panic,残留值也只造成单次采样
    /// 偏差,后续 EWMA 会指数衰减掉影响。与 event-bus bus.rs 的中毒锁降级
    /// 处理方式保持一致。
    fn lock_state(&self) -> std::sync::MutexGuard<'_, MetricsState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 记录协调成本样本(更新 EWMA)
    ///
    /// 时间复杂度:O(1)
    pub fn record_cost(&self, sample: &CoordinationCostSample) {
        let mut state = self.lock_state();
        let total_ms = sample.total_ms();
        let alpha = self.config.ewma_alpha;

        if state.sample_count == 0 {
            // 首个样本:EWMA = 样本值
            state.cost_ewma_ms = total_ms;
        } else {
            // EWMA 增量更新:ewma = alpha * new + (1 - alpha) * old
            state.cost_ewma_ms = alpha * total_ms + (1.0 - alpha) * state.cost_ewma_ms;
        }
    }

    /// 记录推理增益样本(更新 EWMA)
    ///
    /// 时间复杂度:O(1)
    pub fn record_gain(&self, sample: &InferenceGainSample) {
        let mut state = self.lock_state();
        let gain = sample.total_gain() as f64;
        let alpha = self.config.ewma_alpha;

        if state.sample_count == 0 {
            // 首个样本:EWMA = 样本值
            state.gain_ewma = gain;
        } else {
            // EWMA 增量更新
            state.gain_ewma = alpha * gain + (1.0 - alpha) * state.gain_ewma;
        }
    }

    /// 记录协调成本和推理增益样本对,并计算当前比值
    ///
    /// 这是最常用的方法:在 Quest 完成时同时记录成本和增益,并计算比值。
    ///
    /// 时间复杂度:O(1)
    ///
    /// # 参数
    /// - `cost`:协调成本样本
    /// - `gain`:推理增益样本
    /// - `complexity`:可选的复杂度评分(P1-5:自适应基线用,`None` 时使用静态基准)
    ///
    /// # 事件发布(P2-1 后续增强)
    /// 若绑定了 EventBus,本方法会在计算比值后自动发布
    /// `NexusEvent::CoordinationRatioReported` 事件。事件发布使用
    /// `publish_blocking`(同步),失败时仅记录 warn 日志,不影响比值计算。
    ///
    /// # 返回
    /// 当前 EWMA 比值快照
    pub fn record_and_compute(
        &self,
        cost: &CoordinationCostSample,
        gain: &InferenceGainSample,
        complexity: Option<ComplexityScore>,
    ) -> CoordinationToGainRatio {
        // WHY 分两步记录而非合并:record_cost / record_gain 可独立调用,
        // 支持协调成本和推理增益在不同时机采集的异步场景。
        // 此处合并调用只是便捷方法,语义等价于先 record_cost 再 record_gain 再 compute。
        //
        // WHY 块作用域:将锁持有范围限定在计算块内,确保 publish_blocking
        // 在锁释放后执行(§4.4 反模式 1:禁止持锁跨 await / 阻塞调用)。
        let (result, sample_count) = {
            let mut state = self.lock_state();
            let alpha = self.config.ewma_alpha;
            let total_ms = cost.total_ms();
            let gain = gain.total_gain() as f64;

            if state.sample_count == 0 {
                state.cost_ewma_ms = total_ms;
                state.gain_ewma = gain;
            } else {
                state.cost_ewma_ms = alpha * total_ms + (1.0 - alpha) * state.cost_ewma_ms;
                state.gain_ewma = alpha * gain + (1.0 - alpha) * state.gain_ewma;
            }
            // P3-11: 当达到 max_samples 上限时停止递增 sample_count
            // (EWMA 仍正常更新,不影响比值计算)
            if state.sample_count < self.config.max_samples {
                state.sample_count += 1;
            }

            // 基于当前 EWMA 计算比值
            // P1-5:使用 effective_baseline 支持自适应基线
            let baseline = self.config.effective_baseline(complexity);
            let cost_index = if baseline > 0.0 {
                (state.cost_ewma_ms / baseline).min(1.0)
            } else {
                1.0
            };
            let gain_index = state.gain_ewma;
            let ratio = if gain_index > 0.0 {
                cost_index / gain_index
            } else {
                f64::INFINITY
            };
            let is_paradox_risk = ratio > self.config.paradox_threshold;

            let result = CoordinationToGainRatio {
                coordination_cost_ms: state.cost_ewma_ms,
                inference_gain: state.gain_ewma as f32,
                cost_index,
                gain_index,
                ratio,
                is_paradox_risk,
                threshold: self.config.paradox_threshold,
            };

            state.last_ratio = Some(result.clone());
            (result, state.sample_count)
        };
        // 锁已释放,可安全执行可能阻塞的 publish_blocking 调用

        // P2-1 后续增强:若绑定了 EventBus,发布 CoordinationRatioReported 事件
        // WHY publish_blocking:record_and_compute 是同步方法,不能用 .await
        // (§4.4 反模式 8:sync 方法用 publish_blocking,async 方法用 publish().await)
        // WHY 失败不传播:指标采集优先于事件发布。发布失败仅记 warn 日志,
        // 不影响比值计算的主流程(调用方仍能得到正确的 ratio 结果)。
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::CoordinationRatioReported {
                metadata: EventMetadata::new("quest-engine"),
                coordination_cost_ms: result.coordination_cost_ms,
                inference_gain: result.inference_gain,
                cost_index: result.cost_index,
                gain_index: result.gain_index,
                ratio: result.ratio,
                is_paradox_risk: result.is_paradox_risk,
                threshold: result.threshold,
                sample_count,
            };
            if let Err(e) = bus.publish_blocking(event) {
                tracing::warn!(
                    error = %e,
                    ratio = result.ratio,
                    "CoordinationRatioReported 事件发布失败,指标采集不受影响"
                );
            }
        }

        result
    }

    /// 查询最近一次计算的比值
    ///
    /// 时间复杂度:O(1)
    pub fn last_ratio(&self) -> Option<CoordinationToGainRatio> {
        let state = self.lock_state();
        state.last_ratio.clone()
    }

    /// 获取当前 EWMA 状态快照
    ///
    /// 时间复杂度:O(1)
    pub fn snapshot(&self) -> (f64, f64, u64) {
        let state = self.lock_state();
        (state.cost_ewma_ms, state.gain_ewma, state.sample_count)
    }

    /// 获取已采集样本数
    ///
    /// 时间复杂度:O(1)
    pub fn sample_count(&self) -> u64 {
        let state = self.lock_state();
        state.sample_count
    }

    /// 获取度量配置引用
    pub fn config(&self) -> &CoordinationMetricsConfig {
        &self.config
    }

    /// 获取事件总线引用(若已绑定)
    ///
    /// 用于 `QuestEngine::with_metrics_config` 在更换配置时保留 EventBus 绑定。
    pub fn event_bus(&self) -> Option<&EventBus> {
        self.event_bus.as_ref()
    }

    /// 重置收集器(清空所有样本与 EWMA 状态)
    ///
    /// WHY:测试场景或系统重启后需要清空历史数据重新采集。
    pub fn reset(&self) {
        let mut state = self.lock_state();
        *state = MetricsState::default();
    }
}

impl Default for CoordinationMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // === CoordinationMetricsConfig 测试 ===

    #[test]
    fn test_config_default() {
        let cfg = CoordinationMetricsConfig::default();
        assert_eq!(cfg.paradox_threshold, 1.0);
        assert_eq!(cfg.cost_baseline_ms, 1000.0);
        assert_eq!(cfg.ewma_alpha, 0.3);
        // P1-5: 验证自适应基线默认值
        assert!(cfg.adaptive_baseline_enabled, "自适应基线默认启用");
        assert_eq!(cfg.min_baseline_ms, 200.0);
        assert_eq!(cfg.max_baseline_ms, 2000.0);
    }

    #[test]
    fn test_config_new_clamps_alpha() {
        let cfg = CoordinationMetricsConfig::new(1.5, 500.0, 1.5);
        assert_eq!(cfg.ewma_alpha, 1.0); // clamp to [0.0, 1.0]
        assert_eq!(cfg.paradox_threshold, 1.5);
        assert_eq!(cfg.cost_baseline_ms, 500.0);
        // P1-5: new 使用 ..Default::default()，自适应基线应保留默认值
        assert!(cfg.adaptive_baseline_enabled);
        assert_eq!(cfg.min_baseline_ms, 200.0);
        assert_eq!(cfg.max_baseline_ms, 2000.0);
    }

    #[test]
    fn test_config_with_threshold() {
        let cfg = CoordinationMetricsConfig::with_threshold(0.8);
        assert_eq!(cfg.paradox_threshold, 0.8);
        assert_eq!(cfg.cost_baseline_ms, 1000.0); // 默认值
                                                  // P1-5: with_threshold 使用 ..Default::default()，自适应基线应保留默认值
        assert!(cfg.adaptive_baseline_enabled);
    }

    // ============================================================
    // P3-11: max_samples 测试
    // ============================================================

    #[test]
    fn test_config_with_max_samples() {
        let cfg = CoordinationMetricsConfig::default().with_max_samples(100);
        assert_eq!(cfg.max_samples, 100);
        // 其他字段应保持默认值
        assert_eq!(cfg.paradox_threshold, 1.0);
        assert_eq!(cfg.cost_baseline_ms, 1000.0);
    }

    #[test]
    fn test_config_max_samples_default_is_max() {
        let cfg = CoordinationMetricsConfig::default();
        assert_eq!(cfg.max_samples, u64::MAX);
    }

    #[test]
    fn test_collector_max_samples_limits_sample_count() {
        let config = CoordinationMetricsConfig::default().with_max_samples(3);
        let collector = CoordinationMetricsCollector::with_config(config);

        // 记录 5 个样本,但 sample_count 应止于 3
        let cost = CoordinationCostSample::new(100.0, 50.0);
        let gain = InferenceGainSample::new(0.8);

        for _ in 0..5 {
            collector.record_and_compute(&cost, &gain, None);
        }

        // sample_count 应被限制在 3
        assert_eq!(collector.sample_count(), 3);

        // EWMA 仍应正常更新(不受 max_samples 影响)
        let (cost_ewma, gain_ewma, _) = collector.snapshot();
        assert!(
            (cost_ewma - 150.0).abs() < 1e-3,
            "EWMA cost 应更新到 150ms,实际: {cost_ewma}"
        );
        assert!(
            (gain_ewma - 0.8).abs() < 1e-3,
            "EWMA gain 应更新到 0.8,实际: {gain_ewma}"
        );

        // last_ratio 应存在
        let ratio = collector.last_ratio();
        assert!(ratio.is_some(), "last_ratio 应存在");
    }

    #[test]
    fn test_collector_max_samples_zero_disables_recording() {
        // max_samples = 0 时,永不递增 sample_count(永不记录样本数)
        // 但 EWMA 仍正常更新(首个样本初始化,后续样本增量更新)
        let config = CoordinationMetricsConfig::default().with_max_samples(0);
        let collector = CoordinationMetricsCollector::with_config(config);

        let cost = CoordinationCostSample::new(100.0, 50.0);
        let gain = InferenceGainSample::new(0.8);

        // 记录多个样本
        for _ in 0..3 {
            collector.record_and_compute(&cost, &gain, None);
        }

        // sample_count 应始终为 0
        assert_eq!(collector.sample_count(), 0);

        // EWMA 仍应正常更新
        let (cost_ewma, gain_ewma, _) = collector.snapshot();
        assert!(
            (cost_ewma - 150.0).abs() < 1e-3,
            "EWMA cost 应更新到 150ms,实际: {cost_ewma}"
        );
        assert!(
            (gain_ewma - 0.8).abs() < 1e-3,
            "EWMA gain 应更新到 0.8,实际: {gain_ewma}"
        );
    }

    #[test]
    fn test_collector_max_samples_ewma_still_updates_after_cap() {
        // 验证达到上限后 EWMA 仍继续更新
        let config = CoordinationMetricsConfig::default().with_max_samples(2);
        let collector = CoordinationMetricsCollector::with_config(config);

        // 前 3 个样本用 cost=100ms,后 3 个用 cost=900ms
        let cost_low = CoordinationCostSample::new(50.0, 50.0); // total=100ms
        let cost_high = CoordinationCostSample::new(300.0, 600.0); // total=900ms
        let gain = InferenceGainSample::new(0.8);

        for _ in 0..3 {
            collector.record_and_compute(&cost_low, &gain, None);
        }
        // sample_count 应止于 2
        assert_eq!(collector.sample_count(), 2);
        // EWMA 应收敛到 100ms(α=0.3, 3 个相同样本)
        let (cost_ewma, _, _) = collector.snapshot();
        assert!(
            (cost_ewma - 100.0).abs() < 1.0,
            "EWMA cost 应≈100ms,实际: {cost_ewma}"
        );

        // 再记录 3 个高成本样本,EWMA 应更新
        for _ in 0..3 {
            collector.record_and_compute(&cost_high, &gain, None);
        }
        // sample_count 仍为 2(已达上限)
        assert_eq!(collector.sample_count(), 2);
        // EWMA 应向 900ms 移动
        let (cost_ewma, _, _) = collector.snapshot();
        assert!(
            cost_ewma > 100.0,
            "EWMA cost 应向 900ms 移动,实际: {cost_ewma}"
        );
    }

    // ============================================================
    // P1-5: 自适应基线测试
    // ============================================================

    #[test]
    fn test_adaptive_baseline_min_complexity() {
        // 最低复杂度(score=0):基线应接近 min_baseline_ms(200ms)
        // sigmoid(0) = 1/(1+exp(3/1.5)) ≈ 0.119, baseline ≈ 200 + 1800*0.119 ≈ 414
        let cfg = CoordinationMetricsConfig::default();
        let baseline = cfg.adaptive_baseline(0.0);
        assert!(
            (baseline - 414.0).abs() < 20.0,
            "score=0 时基线应 ≈ 414ms,实际: {baseline}"
        );
    }

    #[test]
    fn test_adaptive_baseline_max_complexity() {
        // 最高复杂度(score=10):基线应接近 max_baseline_ms(2000ms)
        let cfg = CoordinationMetricsConfig::default();
        let baseline = cfg.adaptive_baseline(10.0);
        assert!(
            (baseline - 2000.0).abs() < 20.0,
            "score=10 时基线应 ≈ 2000ms,实际: {baseline}"
        );
    }

    #[test]
    fn test_adaptive_baseline_mid_complexity() {
        // 中等复杂度(score=3):基线应为中间值
        // sigmoid(3) = 1/(1+exp(0)) = 0.5, baseline = 200 + 1800*0.5 = 1100
        let cfg = CoordinationMetricsConfig::default();
        let baseline = cfg.adaptive_baseline(3.0);
        assert!(
            (baseline - 1100.0).abs() < 20.0,
            "score=3 时基线应 ≈ 1100ms,实际: {baseline}"
        );
    }

    #[test]
    fn test_adaptive_baseline_disabled_uses_static() {
        // 自适应禁用时，无论传入何复杂度，均使用 cost_baseline_ms
        let cfg = CoordinationMetricsConfig {
            adaptive_baseline_enabled: false,
            cost_baseline_ms: 500.0,
            ..Default::default()
        };
        // effective_baseline 应返回 cost_baseline_ms
        assert_eq!(
            cfg.effective_baseline(Some(ComplexityScore::new(10.0))),
            500.0
        );
        assert_eq!(cfg.effective_baseline(None), 500.0);
    }

    #[test]
    fn test_effective_baseline_none_uses_static() {
        // 未传入复杂度时，使用静态 cost_baseline_ms
        let cfg = CoordinationMetricsConfig::default();
        assert_eq!(cfg.effective_baseline(None), cfg.cost_baseline_ms);
    }

    #[test]
    fn test_adaptive_baseline_clamps_to_range() {
        // 防御性编程:即使用极端值也应钳制在 [min, max] 范围内
        let cfg = CoordinationMetricsConfig::default();
        let baseline = cfg.adaptive_baseline(-100.0); // 极低
        assert!(baseline >= cfg.min_baseline_ms);
        assert!(baseline <= cfg.max_baseline_ms);

        let baseline = cfg.adaptive_baseline(100.0); // 极高
        assert!(baseline >= cfg.min_baseline_ms);
        assert!(baseline <= cfg.max_baseline_ms);
    }

    // === CoordinationCostSample 测试 ===

    #[test]
    fn test_cost_sample_total_ms_basic() {
        let sample = CoordinationCostSample::new(100.0, 50.0);
        assert_eq!(sample.total_ms(), 150.0);
    }

    #[test]
    fn test_cost_sample_total_ms_with_optional() {
        let sample = CoordinationCostSample::new(100.0, 50.0)
            .with_parliament_debate(200.0)
            .with_delegation_overhead(300.0);
        assert_eq!(sample.total_ms(), 650.0);
    }

    #[test]
    fn test_cost_sample_cost_index() {
        let sample = CoordinationCostSample::new(300.0, 200.0); // total = 500ms
                                                                // baseline = 1000ms, cost_index = 500/1000 = 0.5
        assert_eq!(sample.cost_index(1000.0), 0.5);
    }

    #[test]
    fn test_cost_sample_cost_index_clamped_to_1() {
        let sample = CoordinationCostSample::new(800.0, 400.0); // total = 1200ms
                                                                // baseline = 1000ms, cost_index = min(1200/1000, 1.0) = 1.0
        assert_eq!(sample.cost_index(1000.0), 1.0);
    }

    #[test]
    fn test_cost_sample_cost_index_zero_baseline() {
        let sample = CoordinationCostSample::new(100.0, 50.0);
        // baseline = 0, 视为满载
        assert_eq!(sample.cost_index(0.0), 1.0);
    }

    // === InferenceGainSample 测试 ===

    #[test]
    fn test_gain_sample_success_rate_only() {
        let sample = InferenceGainSample::new(0.8);
        // 只有 task_success_rate,权重 1.0
        assert!((sample.total_gain() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_gain_sample_with_quality_score() {
        let sample = InferenceGainSample::new(0.8).with_quality_score(0.6);
        // w_success=0.7, w_quality=0.3
        // total = 0.7*0.8 + 0.3*0.6 = 0.56 + 0.18 = 0.74
        assert!((sample.total_gain() - 0.74).abs() < 1e-6);
    }

    #[test]
    fn test_gain_sample_with_all_fields() {
        let sample = InferenceGainSample::new(0.8)
            .with_quality_score(0.6)
            .with_consensus_quality(0.9);
        // w_success=0.5, w_quality=0.3, w_consensus=0.2
        // total = 0.5*0.8 + 0.3*0.6 + 0.2*0.9 = 0.4 + 0.18 + 0.18 = 0.76
        assert!((sample.total_gain() - 0.76).abs() < 1e-6);
    }

    #[test]
    fn test_gain_sample_clamps_values() {
        let sample = InferenceGainSample::new(1.5); // 超出 [0,1]
        assert_eq!(sample.task_success_rate, 1.0); // clamp to 1.0
    }

    #[test]
    fn test_gain_sample_gain_index() {
        let sample = InferenceGainSample::new(0.6);
        assert!((sample.gain_index() - 0.6).abs() < 1e-6);
    }

    // === CoordinationToGainRatio 测试 ===

    #[test]
    fn test_ratio_normal_case() {
        let cost = CoordinationCostSample::new(300.0, 200.0); // total=500ms, index=0.5
        let gain = InferenceGainSample::new(1.0); // gain=1.0, index=1.0
        let config = CoordinationMetricsConfig::default(); // baseline=1000, threshold=1.0

        let ratio = CoordinationToGainRatio::compute(&cost, &gain, &config, None);
        assert!((ratio.cost_index - 0.5).abs() < 1e-6);
        assert!((ratio.gain_index - 1.0).abs() < 1e-6);
        assert!((ratio.ratio - 0.5).abs() < 1e-6);
        assert!(!ratio.is_paradox_risk); // 0.5 < 1.0
    }

    #[test]
    fn test_ratio_paradox_risk_triggered() {
        let cost = CoordinationCostSample::new(600.0, 400.0); // total=1000ms, index=1.0
        let gain = InferenceGainSample::new(0.5); // gain=0.5, index=0.5
        let config = CoordinationMetricsConfig::default(); // threshold=1.0

        let ratio = CoordinationToGainRatio::compute(&cost, &gain, &config, None);
        // ratio = 1.0 / 0.5 = 2.0 > 1.0 → paradox risk
        assert!((ratio.ratio - 2.0).abs() < 1e-6);
        assert!(ratio.is_paradox_risk);
    }

    #[test]
    fn test_ratio_zero_gain_is_infinity() {
        let cost = CoordinationCostSample::new(100.0, 50.0);
        let gain = InferenceGainSample::new(0.0); // gain=0
        let config = CoordinationMetricsConfig::default();

        let ratio = CoordinationToGainRatio::compute(&cost, &gain, &config, None);
        assert!(ratio.ratio.is_infinite());
        assert!(ratio.is_paradox_risk); // infinity > 1.0
    }

    #[test]
    fn test_ratio_custom_threshold() {
        let cost = CoordinationCostSample::new(300.0, 200.0); // index=0.5
        let gain = InferenceGainSample::new(1.0); // index=1.0
        let config = CoordinationMetricsConfig::with_threshold(0.4); // 更敏感的阈值

        let ratio = CoordinationToGainRatio::compute(&cost, &gain, &config, None);
        // ratio = 0.5 > 0.4 → paradox risk
        assert!((ratio.ratio - 0.5).abs() < 1e-6);
        assert!(ratio.is_paradox_risk);
    }

    #[test]
    fn test_ratio_description_contains_risk_label() {
        let cost = CoordinationCostSample::new(600.0, 400.0); // index=1.0
        let gain = InferenceGainSample::new(0.5); // index=0.5
        let config = CoordinationMetricsConfig::default();

        let ratio = CoordinationToGainRatio::compute(&cost, &gain, &config, None);
        let desc = ratio.description();
        assert!(desc.contains("推理悖论风险"));
        assert!(desc.contains("ratio="));
    }

    #[test]
    fn test_ratio_description_normal_case() {
        let cost = CoordinationCostSample::new(100.0, 50.0); // index=0.15
        let gain = InferenceGainSample::new(1.0); // index=1.0
        let config = CoordinationMetricsConfig::default();

        let ratio = CoordinationToGainRatio::compute(&cost, &gain, &config, None);
        let desc = ratio.description();
        assert!(desc.contains("正常"));
        assert!(!desc.contains("推理悖论风险"));
    }

    // === CoordinationMetricsCollector 测试 ===

    #[test]
    fn test_collector_record_and_compute_first_sample() {
        let collector = CoordinationMetricsCollector::new();
        let cost = CoordinationCostSample::new(300.0, 200.0); // total=500ms
        let gain = InferenceGainSample::new(0.8);

        let ratio = collector.record_and_compute(&cost, &gain, None);
        // 首个样本:EWMA = 样本值
        assert!((ratio.coordination_cost_ms - 500.0).abs() < 1e-6);
        assert!((ratio.inference_gain - 0.8).abs() < 1e-6);
        assert_eq!(collector.sample_count(), 1);
    }

    #[test]
    fn test_collector_ewma_convergence() {
        let collector = CoordinationMetricsCollector::new();
        let config = collector.config().clone();

        // 记录多个样本,验证 EWMA 收敛
        for _ in 0..10 {
            let cost = CoordinationCostSample::new(300.0, 200.0); // total=500ms
            let gain = InferenceGainSample::new(0.8);
            collector.record_and_compute(&cost, &gain, None);
        }

        // 10 个相同样本后,EWMA 应收敛到样本值
        let (cost_ewma, gain_ewma, count) = collector.snapshot();
        assert!((cost_ewma - 500.0).abs() < 1e-3);
        assert!((gain_ewma - 0.8).abs() < 1e-3);
        assert_eq!(count, 10);

        // 比值应稳定
        let ratio = collector.last_ratio().expect("last_ratio should exist");
        let expected_cost_index = (500.0 / config.cost_baseline_ms).min(1.0);
        let expected_ratio = expected_cost_index / 0.8;
        assert!((ratio.ratio - expected_ratio).abs() < 1e-3);
    }

    #[test]
    fn test_collector_reset() {
        let collector = CoordinationMetricsCollector::new();
        let cost = CoordinationCostSample::new(300.0, 200.0);
        let gain = InferenceGainSample::new(0.8);
        collector.record_and_compute(&cost, &gain, None);
        assert_eq!(collector.sample_count(), 1);

        collector.reset();
        assert_eq!(collector.sample_count(), 0);
        assert!(collector.last_ratio().is_none());
    }

    #[test]
    fn test_collector_independent_record_cost_and_gain() {
        let collector = CoordinationMetricsCollector::new();

        // 独立记录协调成本(不计算比值)
        let cost = CoordinationCostSample::new(200.0, 100.0);
        collector.record_cost(&cost);
        assert_eq!(collector.sample_count(), 0); // record_cost 不增加 sample_count

        // 独立记录推理增益
        let gain = InferenceGainSample::new(0.6);
        collector.record_gain(&gain);
        assert_eq!(collector.sample_count(), 0); // record_gain 也不增加 sample_count

        // snapshot 显示 EWMA 已更新
        let (cost_ewma, gain_ewma, count) = collector.snapshot();
        assert!((cost_ewma - 300.0).abs() < 1e-6); // 首个样本:EWMA = 样本值
        assert!((gain_ewma - 0.6).abs() < 1e-6);
        assert_eq!(count, 0); // sample_count 仍为 0
    }

    #[test]
    fn test_collector_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let collector = Arc::new(CoordinationMetricsCollector::new());
        let mut handles = Vec::new();

        for _ in 0..4 {
            let c = Arc::clone(&collector);
            handles.push(thread::spawn(move || {
                for _ in 0..10 {
                    let cost = CoordinationCostSample::new(100.0, 50.0);
                    let gain = InferenceGainSample::new(0.9);
                    c.record_and_compute(&cost, &gain, None);
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should not panic");
        }

        // 4 线程 × 10 次 = 40 个样本
        assert_eq!(collector.sample_count(), 40);
    }

    #[test]
    fn test_collector_paradox_risk_detection() {
        let collector = CoordinationMetricsCollector::new();

        // 高协调成本 + 低推理增益 → 推理悖论风险
        let cost = CoordinationCostSample::new(600.0, 400.0); // total=1000ms, index=1.0
        let gain = InferenceGainSample::new(0.2); // gain=0.2, index=0.2

        let ratio = collector.record_and_compute(&cost, &gain, None);
        assert!(ratio.is_paradox_risk); // ratio = 1.0/0.2 = 5.0 > 1.0
    }

    #[test]
    fn test_collector_config_access() {
        let config = CoordinationMetricsConfig::with_threshold(0.7);
        let collector = CoordinationMetricsCollector::with_config(config);
        assert_eq!(collector.config().paradox_threshold, 0.7);
    }

    #[test]
    fn test_collector_serde_roundtrip() {
        let cost = CoordinationCostSample::new(100.0, 50.0)
            .with_parliament_debate(200.0)
            .with_delegation_overhead(300.0);
        let gain = InferenceGainSample::new(0.8).with_quality_score(0.7);

        // 序列化/反序列化 round-trip
        let cost_json = serde_json::to_string(&cost).unwrap();
        let cost_de: CoordinationCostSample = serde_json::from_str(&cost_json).unwrap();
        assert_eq!(cost, cost_de);

        let gain_json = serde_json::to_string(&gain).unwrap();
        let gain_de: InferenceGainSample = serde_json::from_str(&gain_json).unwrap();
        assert_eq!(gain, gain_de);

        let config = CoordinationMetricsConfig::default();
        let ratio = CoordinationToGainRatio::compute(&cost, &gain, &config, None);
        let ratio_json = serde_json::to_string(&ratio).unwrap();
        let ratio_de: CoordinationToGainRatio = serde_json::from_str(&ratio_json).unwrap();
        assert_eq!(ratio, ratio_de);
    }

    // ============================================================
    // P2-1 后续增强:EventBus 集成测试
    // ============================================================

    /// 验证绑定了 EventBus 的收集器在 record_and_compute 时发布事件
    ///
    /// 关键验证点:
    /// 1. 事件被发布到 EventBus(可通过 try_recv 接收)
    /// 2. 事件类型为 CoordinationRatioReported
    /// 3. 事件字段与计算结果一致
    #[test]
    fn test_collector_with_event_bus_publishes_event() {
        let bus = event_bus::EventBus::new();
        // §4.4 反模式 3:必须在发布前 subscribe,否则事件静默丢失
        let mut rx = bus.subscribe();
        let collector =
            CoordinationMetricsCollector::with_event_bus(CoordinationMetricsConfig::default(), bus);

        let cost = CoordinationCostSample::new(300.0, 200.0); // total=500ms, index=0.5
        let gain = InferenceGainSample::new(1.0); // gain=1.0, index=1.0
        let ratio = collector.record_and_compute(&cost, &gain, None);

        // 接收并验证事件
        let event = rx
            .try_recv()
            .expect("try_recv 不应出错")
            .expect("应收到事件");
        match event {
            NexusEvent::CoordinationRatioReported {
                coordination_cost_ms,
                inference_gain,
                cost_index,
                gain_index,
                ratio: event_ratio,
                is_paradox_risk,
                threshold,
                sample_count,
                ..
            } => {
                assert!((coordination_cost_ms - ratio.coordination_cost_ms).abs() < 1e-6);
                assert!((inference_gain - ratio.inference_gain).abs() < 1e-6);
                assert!((cost_index - ratio.cost_index).abs() < 1e-6);
                assert!((gain_index - ratio.gain_index).abs() < 1e-6);
                assert!((event_ratio - ratio.ratio).abs() < 1e-6);
                assert_eq!(is_paradox_risk, ratio.is_paradox_risk);
                assert!((threshold - ratio.threshold).abs() < 1e-6);
                assert_eq!(sample_count, 1);
            }
            other => panic!(
                "Expected CoordinationRatioReported, got {:?}",
                other.type_name()
            ),
        }
    }

    /// 验证未绑定 EventBus 的收集器不发布事件(且不 panic)
    ///
    /// 这是 `event_bus: None` 的默认路径,确保单元测试场景下
    /// record_and_compute 正常返回比值,不尝试发布事件。
    #[test]
    fn test_collector_without_event_bus_does_not_publish() {
        let collector = CoordinationMetricsCollector::new();
        let cost = CoordinationCostSample::new(100.0, 50.0);
        let gain = InferenceGainSample::new(0.9);

        // 不应 panic,应正常返回比值
        // cost = 100+50 = 150ms, cost_index = 150/1000 = 0.15
        // gain = 0.9, gain_index = 0.9
        // ratio = cost_index / gain_index = 0.15 / 0.9 = 0.16666...
        let ratio = collector.record_and_compute(&cost, &gain, None);
        let expected_ratio = 0.15_f64 / 0.9_f64;
        assert!(
            (ratio.ratio - expected_ratio).abs() < 1e-6,
            "ratio 应为 {expected_ratio},实际为 {}",
            ratio.ratio
        );
        assert!(!ratio.is_paradox_risk);
    }

    /// 验证推理悖论风险场景下事件携带 is_paradox_risk = true
    ///
    /// 当 ratio > threshold 时,事件应携带 is_paradox_risk = true,
    /// 供 efficiency-monitor 订阅后触发 EfficiencyAlertTriggered 告警。
    #[test]
    fn test_collector_publishes_paradox_risk_event() {
        let bus = event_bus::EventBus::new();
        let mut rx = bus.subscribe();
        let collector =
            CoordinationMetricsCollector::with_event_bus(CoordinationMetricsConfig::default(), bus);

        // 构造推理悖论场景:高协调成本 + 低推理增益
        let cost = CoordinationCostSample::new(600.0, 400.0); // total=1000ms, index=1.0
        let gain = InferenceGainSample::new(0.5); // gain=0.5, index=0.5
                                                  // ratio = 1.0 / 0.5 = 2.0 > 1.0 → paradox risk

        let ratio = collector.record_and_compute(&cost, &gain, None);
        assert!(ratio.is_paradox_risk, "应触发推理悖论风险");

        let event = rx
            .try_recv()
            .expect("try_recv 不应出错")
            .expect("应收到事件");
        match event {
            NexusEvent::CoordinationRatioReported {
                is_paradox_risk,
                ratio: event_ratio,
                ..
            } => {
                assert!(is_paradox_risk, "事件应携带 is_paradox_risk = true");
                assert!((event_ratio - 2.0).abs() < 1e-6, "ratio 应为 2.0");
            }
            _ => panic!("Expected CoordinationRatioReported"),
        }
    }

    /// 验证多次 record_and_compute 发布多个事件
    ///
    /// 每次 record_and_compute 都应发布一个事件,
    /// EWMA 增量更新不影响事件发布的可靠性。
    #[test]
    fn test_collector_multiple_records_publish_multiple_events() {
        let bus = event_bus::EventBus::new();
        let mut rx = bus.subscribe();
        let collector =
            CoordinationMetricsCollector::with_event_bus(CoordinationMetricsConfig::default(), bus);

        let cost = CoordinationCostSample::new(200.0, 100.0);
        let gain = InferenceGainSample::new(0.8);

        // 连续记录 3 次
        for _ in 0..3 {
            collector.record_and_compute(&cost, &gain, None);
        }

        // 应收到 3 个事件
        for i in 0..3 {
            let event = rx
                .try_recv()
                .expect("try_recv 不应出错")
                .unwrap_or_else(|| panic!("应收到第 {} 个事件", i + 1));
            match event {
                NexusEvent::CoordinationRatioReported { sample_count, .. } => {
                    assert_eq!(sample_count, (i + 1) as u64);
                }
                _ => panic!("Expected CoordinationRatioReported"),
            }
        }

        // 确认没有更多事件
        let extra = rx.try_recv().expect("try_recv 不应出错");
        assert!(extra.is_none(), "不应有额外事件");
    }

    /// 验证毒锁降级:前任持有者 panic 后采集器仍可用而非传染 panic
    ///
    /// 对应 lock_state 的 unwrap_or_else 降级语义(§4.1 红线:避免
    /// unwrap/expect):另一线程持锁 panic 使 Mutex 中毒后,record_and_compute/
    /// snapshot/reset 应继续工作(降级访问中毒数据),而不是把崩溃传染给调用方。
    ///
    /// WHY 用 record_and_compute 而非 record_cost:sample_count 语义为"比值计算次数",
    /// 仅 record_and_compute 递增它;record_cost/record_gain 仅更新 EWMA 累加器不计数。
    #[test]
    fn test_poisoned_lock_degrades_instead_of_panic() {
        use std::sync::Arc;
        let collector = Arc::new(CoordinationMetricsCollector::new());
        collector.record_and_compute(
            &CoordinationCostSample::new(100.0, 50.0),
            &InferenceGainSample::new(0.8),
            None,
        );

        // 在另一线程持锁 panic,使 Mutex 中毒
        let poisoner = Arc::clone(&collector);
        let handle = std::thread::spawn(move || {
            let _guard = poisoner.state.lock().expect("测试线程首次加锁不应失败");
            panic!("故意 panic 使锁中毒");
        });
        assert!(handle.join().is_err(), "毒化线程应以 panic 退出");

        // 毒锁后各方法应降级继续工作而非 panic
        collector.record_and_compute(
            &CoordinationCostSample::new(200.0, 100.0),
            &InferenceGainSample::new(0.8),
            None,
        );
        let (_cost_ewma, _gain_ewma, count) = collector.snapshot();
        assert_eq!(count, 2, "毒锁降级后应继续采集样本");
        assert!(
            collector.last_ratio().is_some(),
            "record_and_compute 后应有比值"
        );
        collector.reset();
        assert_eq!(collector.sample_count(), 0, "毒锁降级后 reset 仍生效");
    }
}
