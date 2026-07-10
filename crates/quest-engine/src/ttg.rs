//! TTG(Thinking Toggle Governance)思考切换治理 — 基于 Quest 复杂度与预算自动选择思考模式
//!
//! 对应架构层:L9 Quest
//! 对应创新点:TTG(Thinking Toggle Governance)
//!
//! # 核心职责
//! - 评估 Quest 复杂度(任务数 × 0.3 + 依赖深度 × 0.4 + 描述长度因子 × 0.3)
//! - 基于复杂度与预算档位自动选择 Fast/Standard/Deep 三级思考模式
//! - 订阅 DECB 的 `BudgetAdjusted` 事件,联动切换思考模式(带滞后机制)
//! - 支持手动覆盖,但受预算档位约束(Degraded 档位不允许覆盖为 Deep)
//!
//! # 依赖方向(§2.2 依赖铁律)
//! TTG 是 L9 层,可向下依赖 L3 DECB(订阅 BudgetAdjusted 事件)。
//! 不能向上依赖 L8 Parliament(通过事件解耦)。
//!
//! # 事件集成
//! TtgGovernor 可选持有 `EventBus`,在模式切换时自动发布 `ThinkingModeSwitched` 事件。
//! 异步入口 `select_mode_and_publish` / `on_budget_adjusted_and_publish` 封装
//! "选择 + 发布" 的完整流程;同步方法(`select_mode` 等)保留纯计算语义,
//! 供测试和不依赖事件总线的场景使用。
//!
//! # 快速示例
//! ```
//! use decb_governor::BudgetTier;
//! use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
//! use quest_engine::{TtgGovernor, TtgConfig};
//!
//! let governor = TtgGovernor::new(TtgConfig::default());
//! let quest = Quest {
//!     quest_id: "q-1".into(),
//!     title: "demo".into(),
//!     tasks: vec![Task {
//!         task_id: "t-0".into(),
//!         description: "do something".into(),
//!         status: TaskStatus::Pending,
//!         dependencies: vec![],
//!     }],
//!     thinking_mode: ThinkingMode::Standard,
//!     checkpoint_id: None,
//! };
//! let (mode, reason) = governor.select_mode(&quest, BudgetTier::LowTier);
//! assert_eq!(mode, ThinkingMode::Fast);
//! ```

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use decb_governor::BudgetTier;
use nexus_core::{Quest, ThinkingMode};
// WHY 降级:ThinkingModeSwitched 事件已通过 EventBus 发布(见 publish_mode_switch),
// 同步方法内的 info! 与已发布事件重复(违反 DRY)。降级为 debug! 保留诊断信息,
// 避免生产日志噪声;下游消费者订阅 EventBus 获取结构化切换通知,不依赖 tracing。
use tracing::debug;
use uuid::Uuid;

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::adaptive_weights::{AdaptiveWeightLearner, AdaptiveWeights, QuestOutcome};
use crate::arbitration::ArbitrationLayer;
use crate::error::QuestError;

// ============================================================
// 配置 — TTG 阈值与滞后参数
// ============================================================

/// TTG 配置 — 控制复杂度阈值与滞后机制参数
///
/// WHY 字段语义:
/// - `simple_task_threshold`:任务数 ≤ 此值视为简单任务,倾向 Fast
/// - `complex_task_threshold`:任务数 > 此值视为复杂任务,倾向 Deep
/// - `lag_interval_ms`:档位切换后在此时间内不再次切换,防止抖动
///   (与 DECB 滞后机制一致,§6 架构红线:竞态/抖动防护)
/// - `description_length_normalizer`:描述长度归一化基数,
///   长度 / 此值 clamp 到 `[0,1]` 得到 description_length_factor
/// - `adaptive_weights`:P1-13 自适应权重学习配置
///   若启用,复杂度评估权重会根据历史 Quest 成功率动态调整
#[derive(Debug, Clone)]
pub struct TtgConfig {
    /// 简单任务阈值(任务数 ≤ 此值视为简单),默认 3
    pub simple_task_threshold: usize,
    /// 复杂任务阈值(任务数 > 此值视为复杂),默认 10
    pub complex_task_threshold: usize,
    /// 档位切换滞后间隔(毫秒),默认 10000(10 秒)
    pub lag_interval_ms: u64,
    /// 描述长度归一化基数,默认 1000.0
    pub description_length_normalizer: f32,
    /// P1-13: 是否启用自适应权重学习
    pub enable_adaptive_weights: bool,
    /// P1-13: 自适应权重学习率
    pub adaptive_learning_rate: f32,
    /// P1-13: 自适应权重滑动窗口大小
    pub adaptive_window_size: usize,
}

impl Default for TtgConfig {
    fn default() -> Self {
        Self {
            simple_task_threshold: 3,
            complex_task_threshold: 10,
            lag_interval_ms: 10_000,
            description_length_normalizer: 1000.0,
            enable_adaptive_weights: true,
            adaptive_learning_rate: 0.05,
            adaptive_window_size: 20,
        }
    }
}

impl TtgConfig {
    /// 创建自定义 TTG 配置
    pub fn new(
        simple_task_threshold: usize,
        complex_task_threshold: usize,
        lag_interval_ms: u64,
        description_length_normalizer: f32,
    ) -> Self {
        Self {
            simple_task_threshold,
            complex_task_threshold,
            lag_interval_ms,
            description_length_normalizer,
            enable_adaptive_weights: true,
            adaptive_learning_rate: 0.05,
            adaptive_window_size: 20,
        }
    }

    /// P1-13: 创建带自适应权重配置的 TTG 配置
    pub fn with_adaptive(
        simple_task_threshold: usize,
        complex_task_threshold: usize,
        lag_interval_ms: u64,
        description_length_normalizer: f32,
        enable_adaptive: bool,
        learning_rate: f32,
        window_size: usize,
    ) -> Self {
        Self {
            simple_task_threshold,
            complex_task_threshold,
            lag_interval_ms,
            description_length_normalizer,
            enable_adaptive_weights: enable_adaptive,
            adaptive_learning_rate: learning_rate,
            adaptive_window_size: window_size,
        }
    }
}

// ============================================================
// ComplexityScore — newtype 模式,类型安全
// ============================================================

/// 复杂度评分 — 包装 f32,防止与其他 f32 混用(§4.3 newtype 模式)
///
/// WHY newtype:复杂度评分是 TTG 内部计算的中间值,与预算系数、紧急度等
/// 其他 f32 语义不同,newtype 防止调用方误传其他 f32 作为复杂度。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComplexityScore(f32);

impl ComplexityScore {
    /// 创建复杂度评分
    pub fn new(value: f32) -> Self {
        Self(value)
    }

    /// 返回内部 f32 值
    pub fn value(&self) -> f32 {
        self.0
    }
}

// ============================================================
// ModeSwitchReason — 模式切换原因追溯
// ============================================================

/// 模式切换原因 — 追溯每次切换的触发源,用于审计与调试
#[derive(Debug, Clone, PartialEq)]
pub enum ModeSwitchReason {
    /// 自动选择 — 基于复杂度评分或预算档位自动决策
    AutoSelect {
        /// 复杂度评分(用于审计追溯)
        complexity_score: ComplexityScore,
        /// 触发决策的依据:"complexity_score" 或 "budget_tier"
        basis: String,
    },
    /// 预算联动切换 — DECB 档位变化触发的重新选择
    BudgetLinkage {
        /// 旧档位
        old_tier: BudgetTier,
        /// 新档位
        new_tier: BudgetTier,
    },
    /// 手动覆盖 — 由上层(如 Parliament)显式指定
    ManualOverride {
        /// 覆盖来源标识(如 "parliament"、"user")
        override_by: String,
    },
}

// ============================================================
// QuestModeEntry — 单个 Quest 的模式状态
// ============================================================

/// 单个 Quest 的模式状态(内部使用)
///
/// WHY 合并到一个结构体:override_mode 与 current_mode 需要原子读取
/// "当前模式 + 是否被手动覆盖",必须在同一个锁中完成,保证 check-then-act 原子性。
#[derive(Debug, Clone)]
struct QuestModeEntry {
    /// 当前思考模式
    current_mode: ThinkingMode,
    /// 手动覆盖标记(Some 表示被手动覆盖,None 表示自动选择)
    manual_override: Option<ThinkingMode>,
    /// 上次模式切换时间(UTC),用于滞后机制
    last_switch_time: Option<DateTime<Utc>>,
}

// ============================================================
// TtgGovernor — TTG 治理器主结构
// ============================================================

/// TTG 治理器 — 思考切换治理核心
///
/// 维护每个 Quest 的思考模式状态,提供:
/// - 复杂度评估(基于任务数、依赖深度、描述长度)
/// - 自动模式选择(基于复杂度与预算档位)
/// - 预算联动切换(订阅 DECB 档位变化,带滞后机制)
/// - 手动覆盖(受预算档位约束)
/// - P1-13: 自适应权重学习(根据历史 Quest 成功率动态调整权重)
/// - 事件集成:可选持有 EventBus,模式切换时发布 ThinkingModeSwitched 事件
///
/// # 线程安全
/// - `modes` 用 `Mutex<HashMap>` 保护,check-then-act 原子化
///   (§6 架构红线:竞态防护)
/// - `last_budget_switch` 用 `Mutex<HashMap>` 保护,滞后机制时间戳原子读写
/// - `event_bus` 基于 Arc,Clone 廉价,跨任务共享安全
///
/// # 架构红线
/// - 单函数 ≤ 200 行
/// - 无 unwrap()/expect() 在非测试代码
/// - 所有 async fn 满足 Send + 'static
pub struct TtgGovernor {
    /// TTG 配置(只读,构造后不变)
    config: TtgConfig,
    /// Quest 模式注册表(quest_id → 模式状态)
    modes: Mutex<HashMap<String, QuestModeEntry>>,
    /// 预算联动切换的上次切换时间(quest_id → 时间戳)
    ///
    /// WHY 独立于 modes:滞后机制仅作用于预算联动切换,
    /// 手动覆盖不受滞后限制(上层显式指定应立即生效)
    last_budget_switch: Mutex<HashMap<String, DateTime<Utc>>>,
    /// 事件总线(可选)— 模式切换时发布 ThinkingModeSwitched 事件
    ///
    /// WHY Option:测试场景或纯计算用途无需事件总线,
    /// None 时异步发布方法静默跳过事件发布(仅记录 tracing)
    event_bus: Option<EventBus>,
    /// ACB/DECB 仲裁层(可选)— 综合两个治理器信号,保守取严
    ///
    /// WHY Option:与 event_bus 同生命周期,仅 with_event_bus 时创建。
    /// None 时 effective_tier() 直接返回 fallback,向后兼容。
    arbitration: Option<ArbitrationLayer>,
    /// P1-13: 自适应权重学习器(可选)
    ///
    /// WHY Option:向后兼容,不启用自适应时退化为固定权重。
    /// 启用时根据历史 Quest 结果动态调整复杂度评估权重。
    adaptive_learner: Option<Mutex<AdaptiveWeightLearner>>,
}

impl TtgGovernor {
    /// 创建新的 TTG 治理器(不持有事件总线)
    pub fn new(config: TtgConfig) -> Self {
        let adaptive_learner = if config.enable_adaptive_weights {
            Some(Mutex::new(AdaptiveWeightLearner::with_params(
                AdaptiveWeights::default(),
                config.adaptive_window_size,
                config.adaptive_learning_rate,
                5,
            )))
        } else {
            None
        };
        Self {
            config,
            modes: Mutex::new(HashMap::new()),
            last_budget_switch: Mutex::new(HashMap::new()),
            event_bus: None,
            arbitration: None,
            adaptive_learner,
        }
    }

    /// 创建带事件总线的 TTG 治理器
    ///
    /// 模式切换时自动发布 `ThinkingModeSwitched` 事件,
    /// 供 Parliament、Dashboard 等下游消费者订阅。
    ///
    /// 同时创建 ArbitrationLayer 订阅 Parliament topic 事件,
    /// 使 `effective_tier()` / `select_mode_with_arbitration()` 可用。
    pub fn with_event_bus(config: TtgConfig, event_bus: EventBus) -> Self {
        // WHY 先创建 ArbitrationLayer 再 move event_bus:
        // ArbitrationLayer::new 借用 &EventBus,不消费所有权。
        // 但 event_bus 后续需要 move 到 Self,所以 arbitration 必须先创建。
        let arbitration = ArbitrationLayer::new(&event_bus);
        let adaptive_learner = if config.enable_adaptive_weights {
            Some(Mutex::new(AdaptiveWeightLearner::with_params(
                AdaptiveWeights::default(),
                config.adaptive_window_size,
                config.adaptive_learning_rate,
                5,
            )))
        } else {
            None
        };
        Self {
            config,
            modes: Mutex::new(HashMap::new()),
            last_budget_switch: Mutex::new(HashMap::new()),
            event_bus: Some(event_bus),
            arbitration: Some(arbitration),
            adaptive_learner,
        }
    }

    /// 注入事件总线(延迟绑定场景)
    ///
    /// WHY:某些构造流程中 EventBus 在 TtgGovernor 之后才可用,
    /// 此方法允许延迟注入,避免强制要求构造顺序。
    /// 同时创建 ArbitrationLayer 订阅 Parliament topic 事件。
    pub fn set_event_bus(&mut self, event_bus: EventBus) {
        self.arbitration = Some(ArbitrationLayer::new(&event_bus));
        self.event_bus = Some(event_bus);
    }

    /// P1-13: 获取当前自适应权重(若启用)
    ///
    /// 返回 None 表示未启用自适应权重学习。
    pub fn adaptive_weights(&self) -> Option<AdaptiveWeights> {
        self.adaptive_learner.as_ref().map(|l| {
            l.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .weights()
        })
    }

    /// P1-13: 记录 Quest 结果并更新自适应权重
    ///
    /// 应在 Quest 完成后调用,传入任务成功率。
    /// 若未启用自适应权重,此方法无效果。
    pub fn record_quest_outcome(
        &self,
        success_rate: f32,
        complexity_score: f32,
        mode: ThinkingMode,
    ) {
        let Some(learner) = self.adaptive_learner.as_ref() else {
            return;
        };
        let mode_val = match mode {
            ThinkingMode::Fast => 1,
            ThinkingMode::Standard => 2,
            ThinkingMode::Deep => 3,
        };
        let mut locked = learner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locked.record_outcome(QuestOutcome {
            mode_selected: mode_val,
            success_rate: success_rate.clamp(0.0, 1.0),
            complexity_score,
        });
    }

    /// P1-13: 获取平均成功率
    pub fn average_success_rate(&self) -> Option<f32> {
        self.adaptive_learner.as_ref().map(|l| {
            l.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .average_success_rate()
        })
    }

    /// 评估 Quest 复杂度 — 基于任务数、依赖深度与描述长度
    ///
    /// 公式:`complexity_score = task_count × w1 + dependency_depth × w2 + description_length_factor × w3`
    ///
    /// P1-13: 若启用自适应权重,权重会根据历史 Quest 成功率动态调整。
    /// 否则使用固定权重(0.3, 0.4, 0.3)。
    ///
    /// WHY 权重分配:
    /// - 依赖深度权重最高(0.4):深度反映任务编排复杂度,是执行难度的核心指标
    /// - 任务数次之(0.3):数量影响调度开销,但线性链比 DAG 简单
    /// - 描述长度因子最低(0.3):长度间接反映语义复杂度,但可能被冗余文本干扰
    pub fn evaluate_complexity(&self, quest: &Quest) -> ComplexityScore {
        let task_count = quest.tasks.len() as f32;
        let dependency_depth = compute_dependency_depth(&quest.tasks) as f32;
        let description_length_factor = self.compute_description_length_factor(quest);

        // P1-13: 使用自适应权重或固定权重
        let weights = self
            .adaptive_weights()
            .unwrap_or_else(AdaptiveWeights::default);
        let score = weights.compute_score(task_count, dependency_depth, description_length_factor);
        ComplexityScore::new(score)
    }

    /// 计算描述长度归一化因子 — 长度 / normalizer,clamp 到 [0, 1]
    fn compute_description_length_factor(&self, quest: &Quest) -> f32 {
        let total_length = quest.title.len()
            + quest
                .tasks
                .iter()
                .map(|t| t.description.len())
                .sum::<usize>();
        let factor = total_length as f32 / self.config.description_length_normalizer;
        factor.clamp(0.0, 1.0)
    }

    /// 自动选择思考模式 — 基于复杂度与预算档位
    ///
    /// 选择规则(按优先级从高到低):
    /// 1. `budget_tier == Degraded` → `Fast`(降级模式强制快速,§6 架构红线:预算优先)
    /// 2. `task_count <= simple_task_threshold && budget_tier != HighTier` → `Fast`
    ///    (简单任务 + 非高预算档位 → 快速)
    /// 3. `task_count <= complex_task_threshold || budget_tier == LowTier` → `Standard`
    ///    (中等任务或低预算档位 → 标准)
    /// 4. `task_count > complex_task_threshold || budget_tier == HighTier` → `Deep`
    ///    (复杂任务或高预算档位 → 深度)
    ///
    /// WHY 规则优先级:预算约束 > 任务复杂度。Degraded 档位下无论任务多复杂
    /// 都必须用 Fast,防止预算溢出(§6 架构红线:预算优先)。
    pub fn select_mode(
        &self,
        quest: &Quest,
        budget_tier: BudgetTier,
    ) -> (ThinkingMode, ModeSwitchReason) {
        let complexity_score = self.evaluate_complexity(quest);
        let task_count = quest.tasks.len();

        // 规则 1:Degraded 档位强制 Fast(预算优先,防止溢出)
        if budget_tier == BudgetTier::Degraded {
            let mode = ThinkingMode::Fast;
            self.record_mode(quest, mode);
            // WHY debug:事件由 select_mode_and_publish 发布,此处仅保留诊断
            debug!(
                quest_id = %quest.quest_id,
                ?mode,
                complexity_score = complexity_score.value(),
                budget_tier = %budget_tier,
                reason = "auto_select:degraded_forced_fast",
                "ThinkingModeChanged(异步发布见 select_mode_and_publish)"
            );
            return (
                mode,
                ModeSwitchReason::AutoSelect {
                    complexity_score,
                    basis: "budget_tier".into(),
                },
            );
        }

        // 规则 2:简单任务 + 非高预算档位 → Fast
        if task_count <= self.config.simple_task_threshold && budget_tier != BudgetTier::HighTier {
            let mode = ThinkingMode::Fast;
            self.record_mode(quest, mode);
            // WHY debug:事件由 select_mode_and_publish 发布,此处仅保留诊断
            debug!(
                quest_id = %quest.quest_id,
                ?mode,
                complexity_score = complexity_score.value(),
                budget_tier = %budget_tier,
                reason = "auto_select:simple_task",
                "ThinkingModeChanged(异步发布见 select_mode_and_publish)"
            );
            return (
                mode,
                ModeSwitchReason::AutoSelect {
                    complexity_score,
                    basis: "complexity_score".into(),
                },
            );
        }

        // 规则 3:中等任务或低预算档位 → Standard
        if task_count <= self.config.complex_task_threshold || budget_tier == BudgetTier::LowTier {
            let mode = ThinkingMode::Standard;
            self.record_mode(quest, mode);
            // WHY debug:事件由 select_mode_and_publish 发布,此处仅保留诊断
            debug!(
                quest_id = %quest.quest_id,
                ?mode,
                complexity_score = complexity_score.value(),
                budget_tier = %budget_tier,
                reason = "auto_select:medium_task_or_low_tier",
                "ThinkingModeChanged(异步发布见 select_mode_and_publish)"
            );
            return (
                mode,
                ModeSwitchReason::AutoSelect {
                    complexity_score,
                    basis: "complexity_score".into(),
                },
            );
        }

        // 规则 4:复杂任务或高预算档位 → Deep
        let mode = ThinkingMode::Deep;
        self.record_mode(quest, mode);
        // WHY debug:事件由 select_mode_and_publish 发布,此处仅保留诊断
        debug!(
            quest_id = %quest.quest_id,
            ?mode,
            complexity_score = complexity_score.value(),
            budget_tier = %budget_tier,
            reason = "auto_select:complex_task_or_high_tier",
            "ThinkingModeChanged(异步发布见 select_mode_and_publish)"
        );
        (
            mode,
            ModeSwitchReason::AutoSelect {
                complexity_score,
                basis: "complexity_score".into(),
            },
        )
    }

    /// 记录模式到注册表(内部辅助方法)
    ///
    /// WHY:若 quest_id 已存在且模式未变化,仅更新 last_switch_time;
    /// 若模式变化,更新 current_mode 并记录切换时间。
    fn record_mode(&self, quest: &Quest, mode: ThinkingMode) {
        let mut modes = self
            .modes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = modes
            .entry(quest.quest_id.clone())
            .or_insert_with(|| QuestModeEntry {
                current_mode: ThinkingMode::Standard,
                manual_override: None,
                last_switch_time: None,
            });
        if entry.current_mode != mode {
            entry.current_mode = mode;
            entry.last_switch_time = Some(Utc::now());
        }
    }

    /// 获取 Quest 当前思考模式
    ///
    /// 返回 None 表示该 Quest 尚未经过 TTG 决策(新创建的 Quest 默认 Standard,
    /// 但未经过 select_mode 或 override_mode 不会出现在注册表中)。
    pub fn current_mode(&self, quest_id: &str) -> Option<ThinkingMode> {
        let modes = self
            .modes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        modes.get(quest_id).map(|e| e.current_mode)
    }

    /// 预算联动切换 — DECB 档位变化时自动重新选择思考模式
    ///
    /// 联动规则:
    /// - HighTier → 倾向 Deep(高预算支持深度思考)
    /// - LowTier → 倾向 Standard(低预算限制深度)
    /// - Degraded → 强制 Fast(降级模式强制快速)
    ///
    /// 滞后机制:档位变化后 `lag_interval_ms` 内不再次切换,
    /// 防止档位抖动导致模式频繁切换(与 DECB 滞后机制一致)。
    ///
    /// # 返回值
    /// - `Some((mode, reason))`:触发了模式切换
    /// - `None`:档位未变化、滞后期内或新档位下模式未变化
    ///
    /// # 架构红线
    /// 联动切换不阻塞主流程(事件订阅异步处理,在 Task 37 统一集成 event-bus 订阅)。
    pub fn on_budget_adjusted(
        &self,
        quest_id: &str,
        old_tier: BudgetTier,
        new_tier: BudgetTier,
        quest: &Quest,
    ) -> Option<(ThinkingMode, ModeSwitchReason)> {
        // 档位未变化,无需切换
        if old_tier == new_tier {
            return None;
        }

        // 滞后机制检查:上次切换时间 + lag_interval 内不再次切换
        if self.is_within_lag_interval(quest_id) {
            // WHY debug:抑制非事件,仅诊断;不发布事件故无需 EventBus
            debug!(
                quest_id = %quest_id,
                old_tier = %old_tier,
                new_tier = %new_tier,
                "预算联动切换被滞后机制抑制"
            );
            return None;
        }

        // 重新选择模式
        let (mode, _) = self.select_mode(quest, new_tier);
        let reason = ModeSwitchReason::BudgetLinkage { old_tier, new_tier };

        // 更新上次切换时间
        let mut last_switch = self
            .last_budget_switch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        last_switch.insert(quest_id.to_string(), Utc::now());

        // WHY debug:事件由 on_budget_adjusted_and_publish 发布,此处仅保留诊断
        debug!(
            quest_id = %quest_id,
            ?mode,
            old_tier = %old_tier,
            new_tier = %new_tier,
            reason = "budget_linkage",
            "ThinkingModeChanged(异步发布见 select_mode_and_publish)"
        );
        Some((mode, reason))
    }

    /// 检查是否在滞后期内
    ///
    /// WHY:滞后机制防止档位抖动导致模式频繁切换(§6 架构红线:竞态/抖动防护)
    fn is_within_lag_interval(&self, quest_id: &str) -> bool {
        let last_switch = self
            .last_budget_switch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(last_time) = last_switch.get(quest_id) {
            let lag_duration = Duration::milliseconds(self.config.lag_interval_ms as i64);
            let elapsed = Utc::now() - *last_time;
            return elapsed < lag_duration;
        }
        false
    }

    /// 手动覆盖思考模式 — 优先级高于自动选择,但受预算档位约束
    ///
    /// 约束规则:
    /// - `Degraded` 档位不允许覆盖为 `Deep`(预算接近耗尽,Deep 会溢出)
    /// - 其他档位允许覆盖为任意模式
    ///
    /// # 错误
    /// - `TtgOverrideRejected`:Degraded 档位下尝试覆盖为 Deep
    pub fn override_mode(
        &self,
        quest_id: &str,
        mode: ThinkingMode,
        current_tier: BudgetTier,
    ) -> Result<ThinkingMode, QuestError> {
        // 预算约束:Degraded 档位不允许覆盖为 Deep
        if current_tier == BudgetTier::Degraded && mode == ThinkingMode::Deep {
            return Err(QuestError::TtgOverrideRejected {
                quest_id: quest_id.to_string(),
                requested_mode: format!("{mode:?}"),
                current_tier: current_tier.to_string(),
                reason: "Degraded 档位下不允许覆盖为 Deep(预算接近耗尽)".into(),
            });
        }

        let mut modes = self
            .modes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = modes
            .entry(quest_id.to_string())
            .or_insert_with(|| QuestModeEntry {
                current_mode: ThinkingMode::Standard,
                manual_override: None,
                last_switch_time: None,
            });
        entry.current_mode = mode;
        entry.manual_override = Some(mode);
        entry.last_switch_time = Some(Utc::now());
        drop(modes);

        // WHY debug:事件由 override_mode_and_publish 发布,此处仅保留诊断
        debug!(
            quest_id = %quest_id,
            ?mode,
            current_tier = %current_tier,
            reason = "manual_override",
            override_by = "external",
            "ThinkingModeChanged(异步发布见 select_mode_and_publish)"
        );
        Ok(mode)
    }

    /// 清除手动覆盖,恢复自动选择
    ///
    /// WHY:手动覆盖是临时干预,任务完成后应清除以恢复自动决策。
    /// 清除后 current_mode 保留为覆盖值,下次 select_mode 调用会重新决策。
    pub fn reset_override(&self, quest_id: &str) {
        let mut modes = self
            .modes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = modes.get_mut(quest_id) {
            entry.manual_override = None;
        }
        // WHY debug:reset 清除覆盖标记、恢复自动决策,非模式切换,不发布事件;
        // 降级为 debug 避免与 ThinkingModeSwitched 事件混淆
        debug!(
            quest_id = %quest_id,
            "手动覆盖已清除,恢复自动选择"
        );
    }

    /// 检查 Quest 是否被手动覆盖(用于测试与调试)
    pub fn is_overridden(&self, quest_id: &str) -> bool {
        let modes = self
            .modes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        modes
            .get(quest_id)
            .map(|e| e.manual_override.is_some())
            .unwrap_or(false)
    }

    // ============================================================
    // N7: ACB/DECB 仲裁层集成
    // ============================================================

    /// 返回仲裁后的有效 DECB 档位
    ///
    /// 综合订阅到的 ACB 与 DECB 事件,应用保守取严策略:
    /// - ACB L0 → Degraded(无论 DECB 报告什么)
    /// - ACB L1 → LowTier
    /// - ACB L2/L3 或无 ACB 事件 → 使用 DECB 最新档位
    ///
    /// 无仲裁层(无 EventBus)或无事件时返回 `fallback_tier`,
    /// 保证向后兼容。
    ///
    /// # 使用场景
    /// 调用方在 `select_mode` 前先调用此方法获取有效档位:
    /// ```ignore
    /// let tier = governor.effective_tier(decb_tier);
    /// let (mode, reason) = governor.select_mode(&quest, tier);
    /// ```
    pub fn effective_tier(&self, fallback_tier: BudgetTier) -> BudgetTier {
        match &self.arbitration {
            Some(layer) => layer.arbitrated_tier().unwrap_or(fallback_tier),
            None => fallback_tier,
        }
    }

    /// 基于仲裁档位自动选择思考模式
    ///
    /// 封装 `effective_tier()` + `select_mode()` 的完整流程:
    /// 1. 调用 ArbitrationLayer 获取仲裁后的有效 DECB 档位
    /// 2. 基于有效档位调用 `select_mode` 选择思考模式
    ///
    /// 无仲裁层时使用 `fallback_tier`,等价于直接调用 `select_mode`。
    ///
    /// # 参数
    /// - `quest`:待评估的 Quest
    /// - `fallback_tier`:无仲裁事件时的降级档位(通常是 DECB 直接报告的档位)
    pub fn select_mode_with_arbitration(
        &self,
        quest: &Quest,
        fallback_tier: BudgetTier,
    ) -> (ThinkingMode, ModeSwitchReason) {
        let effective = self.effective_tier(fallback_tier);
        self.select_mode(quest, effective)
    }

    // ============================================================
    // 异步事件发布入口 — 封装 "选择 + 发布" 完整流程
    // ============================================================

    /// 自动选择思考模式并发布 ThinkingModeSwitched 事件
    ///
    /// 封装 `select_mode`(同步) + 事件发布(异步)的完整流程。
    /// 仅当新模式与当前模式不同时才发布事件(避免冗余事件)。
    ///
    /// # 返回值
    /// - `Some((mode, reason))`:模式发生变化,已发布事件
    /// - `None`:模式未变化或 Quest 尚未经过 TTG 注册
    ///
    /// # 并发注意
    /// `current_mode` 读取与 `select_mode` 写入之间是两次独立锁获取,
    /// 存在微小 TOCTOU 窗口,极端并发下可能产生冗余事件。
    /// 消费者应幂等处理 ThinkingModeSwitched 事件。
    ///
    /// # 错误
    /// 事件总线发布失败时返回 `QuestError::EventBusError`。
    pub async fn select_mode_and_publish(
        &self,
        quest_id: &str,
        quest: &Quest,
        budget_tier: BudgetTier,
    ) -> Result<Option<(ThinkingMode, ModeSwitchReason)>, QuestError> {
        let previous_mode = self.current_mode(quest_id);
        let (new_mode, reason) = self.select_mode(quest, budget_tier);

        // 仅在模式实际变化时发布事件(避免冗余事件噪声)
        if previous_mode == Some(new_mode) {
            return Ok(None);
        }

        let from_mode_str = previous_mode
            .map(|m| format!("{m:?}"))
            .unwrap_or_else(|| "None".into());

        self.publish_mode_switch(quest_id, from_mode_str, new_mode, &reason)
            .await?;
        Ok(Some((new_mode, reason)))
    }

    /// 预算联动切换并发布事件
    ///
    /// 封装 `on_budget_adjusted`(同步) + 事件发布(异步)。
    /// 滞后机制和档位未变化时自动跳过。
    pub async fn on_budget_adjusted_and_publish(
        &self,
        quest_id: &str,
        old_tier: BudgetTier,
        new_tier: BudgetTier,
        quest: &Quest,
    ) -> Result<Option<(ThinkingMode, ModeSwitchReason)>, QuestError> {
        let previous_mode = self.current_mode(quest_id);
        let result = self.on_budget_adjusted(quest_id, old_tier, new_tier, quest);

        let (new_mode, reason) = match result {
            Some(pair) => pair,
            None => return Ok(None),
        };

        let from_mode_str = previous_mode
            .map(|m| format!("{m:?}"))
            .unwrap_or_else(|| "None".into());

        self.publish_mode_switch(quest_id, from_mode_str, new_mode, &reason)
            .await?;
        Ok(Some((new_mode, reason)))
    }

    /// 手动覆盖并发布事件
    ///
    /// 封装 `override_mode`(同步) + 事件发布(异步)。
    /// Degraded 档位下覆盖为 Deep 仍返回 `TtgOverrideRejected` 错误。
    pub async fn override_mode_and_publish(
        &self,
        quest_id: &str,
        mode: ThinkingMode,
        current_tier: BudgetTier,
    ) -> Result<ThinkingMode, QuestError> {
        let previous_mode = self.current_mode(quest_id);
        let result_mode = self.override_mode(quest_id, mode, current_tier)?;

        let from_mode_str = previous_mode
            .map(|m| format!("{m:?}"))
            .unwrap_or_else(|| "None".into());
        let reason = ModeSwitchReason::ManualOverride {
            override_by: "external".into(),
        };

        self.publish_mode_switch(quest_id, from_mode_str, result_mode, &reason)
            .await?;
        Ok(result_mode)
    }

    // ============================================================
    // 内部辅助 — 事件构建与发布
    // ============================================================

    /// 构建并发布 ThinkingModeSwitched 事件
    ///
    /// 若 `event_bus` 为 None,仅记录 tracing(与集成前行为一致)。
    /// 发布失败时向上传播错误,由调用方决定降级策略。
    async fn publish_mode_switch(
        &self,
        quest_id: &str,
        from_mode: String,
        to_mode: ThinkingMode,
        reason: &ModeSwitchReason,
    ) -> Result<(), QuestError> {
        let reason_str = mode_switch_reason_to_str(reason);

        // WHY 删除 info!:本函数即发布 ThinkingModeSwitched 事件,事件本身已携带
        // from_mode/to_mode/reason 结构化字段,重复 tracing::info! 违反 DRY。
        // 有 EventBus 时消费者订阅事件;无 EventBus 时静默返回(集成前行为)。
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::ThinkingModeSwitched {
                metadata: EventMetadata::new("ttg-governor"),
                quest_id: quest_id.to_string(),
                from_mode,
                to_mode: format!("{to_mode:?}"),
                reason: reason_str,
            };
            bus.publish(event).await?;
        }
        Ok(())
    }

    /// 生成唯一 ID(用于测试辅助,复用 UUIDv7)
    #[allow(dead_code)]
    fn generate_id() -> String {
        format!("ttg-{}", Uuid::now_v7())
    }
}

// ============================================================
// 辅助函数 — 模式切换原因序列化
// ============================================================

/// 将 ModeSwitchReason 转为人类可读字符串,用于事件 `reason` 字段
///
/// WHY 独立函数:避免在多个发布入口重复 match 逻辑,
/// 且测试可独立验证字符串格式。
fn mode_switch_reason_to_str(reason: &ModeSwitchReason) -> String {
    match reason {
        ModeSwitchReason::AutoSelect {
            complexity_score,
            basis,
        } => format!("auto_select({basis},score={:.2})", complexity_score.value()),
        ModeSwitchReason::BudgetLinkage { old_tier, new_tier } => {
            format!("budget_linkage({old_tier}->{new_tier})")
        }
        ModeSwitchReason::ManualOverride { override_by } => {
            format!("manual_override(by={override_by})")
        }
    }
}

// ============================================================
// 辅助函数 — DAG 依赖深度计算
// ============================================================

/// 计算 DAG 最长路径深度 — 从入度为 0 的节点出发的最长路径节点数
///
/// WHY 复用 DAG 逻辑:dependency_depth 反映任务编排复杂度,
/// 线性链深度 = 节点数,扁平并行深度 = 1。
///
/// 算法:动态规划 + 拓扑序,`depth[v] = 1 + max(depth[u] for u in deps[v])`
/// 无依赖的节点 depth = 1,空图返回 0。
fn compute_dependency_depth(tasks: &[nexus_core::Task]) -> usize {
    if tasks.is_empty() {
        return 0;
    }

    // 构建 task_id → Task 索引
    let mut id_to_idx: HashMap<&str, usize> = HashMap::new();
    for (idx, task) in tasks.iter().enumerate() {
        id_to_idx.insert(task.task_id.as_str(), idx);
    }

    // 动态规划计算每个节点的深度(记忆化递归)
    // WHY 用 memo:避免重复计算,DAG 保证无环,递归必然终止
    let mut memo: Vec<Option<usize>> = vec![None; tasks.len()];

    fn depth_of(
        idx: usize,
        tasks: &[nexus_core::Task],
        id_to_idx: &HashMap<&str, usize>,
        memo: &mut [Option<usize>],
    ) -> usize {
        if let Some(d) = memo[idx] {
            return d;
        }
        let task = &tasks[idx];
        let max_dep_depth = if task.dependencies.is_empty() {
            0
        } else {
            task.dependencies
                .iter()
                .filter_map(|dep_id| id_to_idx.get(dep_id.as_str()).copied())
                .map(|dep_idx| depth_of(dep_idx, tasks, id_to_idx, memo))
                .max()
                .unwrap_or(0)
        };
        let depth = max_dep_depth + 1;
        memo[idx] = Some(depth);
        depth
    }

    let mut max_depth = 0usize;
    for idx in 0..tasks.len() {
        let d = depth_of(idx, tasks, &id_to_idx, &mut memo);
        if d > max_depth {
            max_depth = d;
        }
    }
    max_depth
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{Task, TaskStatus};

    /// 构造测试用 Quest
    fn make_quest(quest_id: &str, tasks: Vec<Task>) -> Quest {
        Quest {
            quest_id: quest_id.to_string(),
            title: format!("quest {quest_id}"),
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    /// 构造测试用 Task(线性依赖链)
    fn make_linear_tasks(count: usize) -> Vec<Task> {
        (0..count)
            .map(|idx| Task {
                task_id: format!("task-{idx}"),
                description: format!("do task {idx}"),
                status: TaskStatus::Pending,
                dependencies: if idx == 0 {
                    vec![]
                } else {
                    vec![format!("task-{}", idx - 1)]
                },
            })
            .collect()
    }

    /// 构造测试用 Task(扁平并行,无依赖)
    fn make_parallel_tasks(count: usize) -> Vec<Task> {
        (0..count)
            .map(|idx| Task {
                task_id: format!("task-{idx}"),
                description: format!("do task {idx}"),
                status: TaskStatus::Pending,
                dependencies: vec![],
            })
            .collect()
    }

    // ============================================================
    // SubTask 35.1: 类型与配置测试
    // ============================================================

    #[test]
    fn test_ttg_config_default() {
        let cfg = TtgConfig::default();
        assert_eq!(cfg.simple_task_threshold, 3);
        assert_eq!(cfg.complex_task_threshold, 10);
        assert_eq!(cfg.lag_interval_ms, 10_000);
        assert!((cfg.description_length_normalizer - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn test_ttg_config_custom() {
        let cfg = TtgConfig::new(5, 20, 5_000, 500.0);
        assert_eq!(cfg.simple_task_threshold, 5);
        assert_eq!(cfg.complex_task_threshold, 20);
        assert_eq!(cfg.lag_interval_ms, 5_000);
        assert!((cfg.description_length_normalizer - 500.0).abs() < 1e-6);
    }

    #[test]
    fn test_complexity_score_newtype() {
        let score = ComplexityScore::new(2.5);
        assert!((score.value() - 2.5).abs() < 1e-6);
    }

    #[test]
    fn test_mode_switch_reason_variants() {
        let auto = ModeSwitchReason::AutoSelect {
            complexity_score: ComplexityScore::new(5.0),
            basis: "complexity_score".into(),
        };
        assert!(matches!(auto, ModeSwitchReason::AutoSelect { .. }));

        let linkage = ModeSwitchReason::BudgetLinkage {
            old_tier: BudgetTier::HighTier,
            new_tier: BudgetTier::LowTier,
        };
        assert!(matches!(linkage, ModeSwitchReason::BudgetLinkage { .. }));

        let manual = ModeSwitchReason::ManualOverride {
            override_by: "parliament".into(),
        };
        assert!(matches!(manual, ModeSwitchReason::ManualOverride { .. }));
    }

    // ============================================================
    // SubTask 35.2: 复杂度评估测试
    // ============================================================

    #[test]
    fn test_evaluate_complexity_simple_quest() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-1", make_parallel_tasks(1));
        let score = governor.evaluate_complexity(&quest);
        // 1 task × 0.3 + 1 depth × 0.4 + factor × 0.3
        // factor = (title.len + sum(desc.len)) / 1000,clamp [0,1]
        // title = "quest q-1" (9 bytes), desc = "do task 0" (9 bytes), total = 18
        // factor = 18 / 1000 = 0.018
        // score = 0.3 + 0.4 + 0.018 × 0.3 = 0.7054
        assert!(
            (score.value() - 0.7054).abs() < 1e-3,
            "simple quest score = {}",
            score.value()
        );
    }

    #[test]
    fn test_evaluate_complexity_medium_quest() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-2", make_parallel_tasks(5));
        let score = governor.evaluate_complexity(&quest);
        // 5 × 0.3 + 1 × 0.4 + factor × 0.3
        // factor = (9 + 5×9) / 1000 = 54/1000 = 0.054
        // score = 1.5 + 0.4 + 0.054×0.3 = 1.9162
        assert!(
            (score.value() - 1.9162).abs() < 1e-3,
            "medium quest score = {}",
            score.value()
        );
    }

    #[test]
    fn test_evaluate_complexity_complex_quest() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-3", make_parallel_tasks(20));
        let score = governor.evaluate_complexity(&quest);
        // 20 × 0.3 + 1 × 0.4 + factor × 0.3
        // title = "quest q-3" (9 bytes)
        // desc: task 0-9 = "do task {i}" (9 bytes each, 10 tasks)
        //       task 10-19 = "do task {i}" (10 bytes each, 10 tasks)
        // total = 9 + 10×9 + 10×10 = 199, factor = 0.199
        // score = 6.0 + 0.4 + 0.199×0.3 = 6.4597
        assert!(
            (score.value() - 6.4597).abs() < 1e-3,
            "complex quest score = {}",
            score.value()
        );
    }

    #[test]
    fn test_evaluate_complexity_dependency_depth_impact() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 线性链 5 个任务,深度 = 5
        let linear = make_quest("q-linear", make_linear_tasks(5));
        // 扁平并行 5 个任务,深度 = 1
        let parallel = make_quest("q-parallel", make_parallel_tasks(5));

        let linear_score = governor.evaluate_complexity(&linear).value();
        let parallel_score = governor.evaluate_complexity(&parallel).value();
        // 线性链深度更高,复杂度评分应更高
        assert!(
            linear_score > parallel_score,
            "linear score {} should > parallel score {}",
            linear_score,
            parallel_score
        );
    }

    #[test]
    fn test_evaluate_complexity_description_length_impact() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 短描述
        let short_desc_quest = make_quest(
            "q-short",
            vec![Task {
                task_id: "t-0".into(),
                description: "x".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
        );
        // 长描述(超过 normalizer)
        let long_desc_quest = make_quest(
            "q-long",
            vec![Task {
                task_id: "t-0".into(),
                description: "x".repeat(2000),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
        );

        let short_score = governor.evaluate_complexity(&short_desc_quest).value();
        let long_score = governor.evaluate_complexity(&long_desc_quest).value();
        // 长描述的复杂度评分应更高
        assert!(
            long_score > short_score,
            "long desc score {} should > short desc score {}",
            long_score,
            short_score
        );
    }

    #[test]
    fn test_evaluate_complexity_empty_quest() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-empty", vec![]);
        let score = governor.evaluate_complexity(&quest);
        // 0 tasks, 0 depth, factor = title.len / 1000
        // title = "quest q-empty" (13 bytes), factor = 0.013
        // score = 0 + 0 + 0.013 × 0.3 = 0.0039
        assert!(
            (score.value() - 0.0039).abs() < 1e-3,
            "empty quest score = {}",
            score.value()
        );
    }

    // ============================================================
    // SubTask 35.3: 自动模式选择测试
    // ============================================================

    #[test]
    fn test_select_mode_degraded_forces_fast() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 20 个任务的复杂 Quest,Degraded 档位应强制 Fast
        let quest = make_quest("q-1", make_parallel_tasks(20));
        let (mode, reason) = governor.select_mode(&quest, BudgetTier::Degraded);
        assert_eq!(mode, ThinkingMode::Fast);
        assert!(matches!(reason, ModeSwitchReason::AutoSelect { .. }));
    }

    #[test]
    fn test_select_mode_simple_task_low_tier() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 1 个任务(≤ simple_task_threshold=3),LowTier → Fast
        let quest = make_quest("q-2", make_parallel_tasks(1));
        let (mode, _) = governor.select_mode(&quest, BudgetTier::LowTier);
        assert_eq!(mode, ThinkingMode::Fast);
    }

    #[test]
    fn test_select_mode_simple_task_high_tier() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 1 个任务,HighTier → 不走规则 2(因为 HighTier),走规则 3(任务数 ≤ complex_task_threshold)
        let quest = make_quest("q-3", make_parallel_tasks(1));
        let (mode, _) = governor.select_mode(&quest, BudgetTier::HighTier);
        assert_eq!(mode, ThinkingMode::Standard);
    }

    #[test]
    fn test_select_mode_medium_task_low_tier() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 5 个任务(> simple, ≤ complex),LowTier → Standard(规则 3)
        let quest = make_quest("q-4", make_parallel_tasks(5));
        let (mode, _) = governor.select_mode(&quest, BudgetTier::LowTier);
        assert_eq!(mode, ThinkingMode::Standard);
    }

    #[test]
    fn test_select_mode_complex_task_high_tier() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 20 个任务(> complex),HighTier → Deep(规则 4)
        let quest = make_quest("q-5", make_parallel_tasks(20));
        let (mode, _) = governor.select_mode(&quest, BudgetTier::HighTier);
        assert_eq!(mode, ThinkingMode::Deep);
    }

    #[test]
    fn test_select_mode_boundary_simple_threshold() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 任务数 = simple_task_threshold(3),LowTier → Fast(规则 2,≤)
        let quest = make_quest("q-6", make_parallel_tasks(3));
        let (mode, _) = governor.select_mode(&quest, BudgetTier::LowTier);
        assert_eq!(mode, ThinkingMode::Fast);
    }

    #[test]
    fn test_select_mode_boundary_complex_threshold() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 任务数 = complex_task_threshold(10),LowTier → Standard(规则 3,≤)
        let quest = make_quest("q-7", make_parallel_tasks(10));
        let (mode, _) = governor.select_mode(&quest, BudgetTier::LowTier);
        assert_eq!(mode, ThinkingMode::Standard);
    }

    #[test]
    fn test_select_mode_records_current_mode() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-8", make_parallel_tasks(20));
        let (mode, _) = governor.select_mode(&quest, BudgetTier::HighTier);
        assert_eq!(mode, ThinkingMode::Deep);
        // current_mode 应返回 Deep
        assert_eq!(
            governor.current_mode(&quest.quest_id),
            Some(ThinkingMode::Deep)
        );
    }

    #[test]
    fn test_current_mode_returns_none_for_unknown_quest() {
        let governor = TtgGovernor::new(TtgConfig::default());
        assert_eq!(governor.current_mode("unknown"), None);
    }

    // ============================================================
    // SubTask 35.4: 预算联动切换测试
    // ============================================================

    #[test]
    fn test_on_budget_adjusted_high_tier_to_deep() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 复杂 Quest,从 LowTier 切换到 HighTier → Deep
        let quest = make_quest("q-link-1", make_parallel_tasks(20));
        let result = governor.on_budget_adjusted(
            "q-link-1",
            BudgetTier::LowTier,
            BudgetTier::HighTier,
            &quest,
        );
        let (mode, reason) = result.expect("应触发切换");
        assert_eq!(mode, ThinkingMode::Deep);
        assert!(matches!(reason, ModeSwitchReason::BudgetLinkage { .. }));
    }

    #[test]
    fn test_on_budget_adjusted_to_low_tier_standard() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 中等 Quest,从 HighTier 切换到 LowTier → Standard
        let quest = make_quest("q-link-2", make_parallel_tasks(5));
        let result = governor.on_budget_adjusted(
            "q-link-2",
            BudgetTier::HighTier,
            BudgetTier::LowTier,
            &quest,
        );
        let (mode, _) = result.expect("应触发切换");
        assert_eq!(mode, ThinkingMode::Standard);
    }

    #[test]
    fn test_on_budget_adjusted_to_degraded_fast() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 复杂 Quest,从 HighTier 切换到 Degraded → Fast
        let quest = make_quest("q-link-3", make_parallel_tasks(20));
        let result = governor.on_budget_adjusted(
            "q-link-3",
            BudgetTier::HighTier,
            BudgetTier::Degraded,
            &quest,
        );
        let (mode, _) = result.expect("应触发切换");
        assert_eq!(mode, ThinkingMode::Fast);
    }

    #[test]
    fn test_on_budget_adjusted_same_tier_no_switch() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-link-4", make_parallel_tasks(5));
        let result = governor.on_budget_adjusted(
            "q-link-4",
            BudgetTier::LowTier,
            BudgetTier::LowTier,
            &quest,
        );
        assert!(result.is_none(), "档位未变化不应触发切换");
    }

    #[test]
    fn test_on_budget_adjusted_lag_interval_suppresses() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-link-5", make_parallel_tasks(20));
        // 第一次切换:HighTier → LowTier
        let first = governor.on_budget_adjusted(
            "q-link-5",
            BudgetTier::HighTier,
            BudgetTier::LowTier,
            &quest,
        );
        assert!(first.is_some(), "首次切换应成功");
        // 第二次切换:LowTier → HighTier,在滞后期内应被抑制
        let second = governor.on_budget_adjusted(
            "q-link-5",
            BudgetTier::LowTier,
            BudgetTier::HighTier,
            &quest,
        );
        assert!(second.is_none(), "滞后期内不应再次切换");
    }

    // ============================================================
    // SubTask 35.5: 手动覆盖与回退测试
    // ============================================================

    #[test]
    fn test_override_mode_to_deep() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let result =
            governor.override_mode("q-override-1", ThinkingMode::Deep, BudgetTier::HighTier);
        assert_eq!(result.unwrap(), ThinkingMode::Deep);
        assert_eq!(
            governor.current_mode("q-override-1"),
            Some(ThinkingMode::Deep)
        );
        assert!(governor.is_overridden("q-override-1"));
    }

    #[test]
    fn test_override_mode_degraded_rejects_deep() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let result =
            governor.override_mode("q-override-2", ThinkingMode::Deep, BudgetTier::Degraded);
        assert!(matches!(
            result,
            Err(QuestError::TtgOverrideRejected { .. })
        ));
    }

    #[test]
    fn test_override_mode_degraded_allows_standard() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let result =
            governor.override_mode("q-override-3", ThinkingMode::Standard, BudgetTier::Degraded);
        assert_eq!(result.unwrap(), ThinkingMode::Standard);
    }

    #[test]
    fn test_override_mode_degraded_allows_fast() {
        let governor = TtgGovernor::new(TtgConfig::default());
        let result =
            governor.override_mode("q-override-4", ThinkingMode::Fast, BudgetTier::Degraded);
        assert_eq!(result.unwrap(), ThinkingMode::Fast);
    }

    #[test]
    fn test_reset_override() {
        let governor = TtgGovernor::new(TtgConfig::default());
        governor
            .override_mode("q-override-5", ThinkingMode::Deep, BudgetTier::HighTier)
            .unwrap();
        assert!(governor.is_overridden("q-override-5"));
        governor.reset_override("q-override-5");
        assert!(!governor.is_overridden("q-override-5"));
        // current_mode 保留为覆盖值,下次 select_mode 会重新决策
        assert_eq!(
            governor.current_mode("q-override-5"),
            Some(ThinkingMode::Deep)
        );
    }

    #[test]
    fn test_reset_override_unknown_quest() {
        let governor = TtgGovernor::new(TtgConfig::default());
        // 不存在的 quest,reset_override 不应 panic
        governor.reset_override("unknown");
    }

    // ============================================================
    // 辅助函数测试:compute_dependency_depth
    // ============================================================

    #[test]
    fn test_compute_dependency_depth_empty() {
        assert_eq!(compute_dependency_depth(&[]), 0);
    }

    #[test]
    fn test_compute_dependency_depth_single() {
        let tasks = make_parallel_tasks(1);
        assert_eq!(compute_dependency_depth(&tasks), 1);
    }

    #[test]
    fn test_compute_dependency_depth_linear() {
        // 线性链 5 个任务,深度 = 5
        let tasks = make_linear_tasks(5);
        assert_eq!(compute_dependency_depth(&tasks), 5);
    }

    #[test]
    fn test_compute_dependency_depth_parallel() {
        // 扁平并行 5 个任务,深度 = 1
        let tasks = make_parallel_tasks(5);
        assert_eq!(compute_dependency_depth(&tasks), 1);
    }

    #[test]
    fn test_compute_dependency_depth_diamond() {
        // 菱形:a → b, a → c, b → d, c → d
        // 深度 = 3 (a → b → d 或 a → c → d)
        let tasks = vec![
            Task {
                task_id: "a".into(),
                description: "a".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            },
            Task {
                task_id: "b".into(),
                description: "b".into(),
                status: TaskStatus::Pending,
                dependencies: vec!["a".into()],
            },
            Task {
                task_id: "c".into(),
                description: "c".into(),
                status: TaskStatus::Pending,
                dependencies: vec!["a".into()],
            },
            Task {
                task_id: "d".into(),
                description: "d".into(),
                status: TaskStatus::Pending,
                dependencies: vec!["b".into(), "c".into()],
            },
        ];
        assert_eq!(compute_dependency_depth(&tasks), 3);
    }

    // ============================================================
    // P1-6: 事件总线集成测试
    // ============================================================

    #[test]
    fn test_mode_switch_reason_to_str_auto_select() {
        let reason = ModeSwitchReason::AutoSelect {
            complexity_score: ComplexityScore::new(3.5),
            basis: "complexity_score".into(),
        };
        let s = mode_switch_reason_to_str(&reason);
        assert!(s.contains("auto_select"), "got: {s}");
        assert!(s.contains("complexity_score"), "got: {s}");
        assert!(s.contains("3.5"), "got: {s}");
    }

    #[test]
    fn test_mode_switch_reason_to_str_budget_linkage() {
        let reason = ModeSwitchReason::BudgetLinkage {
            old_tier: BudgetTier::HighTier,
            new_tier: BudgetTier::Degraded,
        };
        let s = mode_switch_reason_to_str(&reason);
        assert!(s.contains("budget_linkage"), "got: {s}");
        assert!(s.contains("high_tier"), "got: {s}");
        assert!(s.contains("degraded"), "got: {s}");
    }

    #[test]
    fn test_mode_switch_reason_to_str_manual_override() {
        let reason = ModeSwitchReason::ManualOverride {
            override_by: "parliament".into(),
        };
        let s = mode_switch_reason_to_str(&reason);
        assert!(s.contains("manual_override"), "got: {s}");
        assert!(s.contains("parliament"), "got: {s}");
    }

    #[test]
    fn test_with_event_bus_constructor() {
        let bus = event_bus::EventBus::new();
        let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);
        assert!(governor.event_bus.is_some());
    }

    #[test]
    fn test_set_event_bus() {
        let mut governor = TtgGovernor::new(TtgConfig::default());
        assert!(governor.event_bus.is_none());
        governor.set_event_bus(event_bus::EventBus::new());
        assert!(governor.event_bus.is_some());
    }

    #[tokio::test]
    async fn test_select_mode_and_publish_publishes_event() {
        let bus = event_bus::EventBus::new();
        let mut rx = bus.subscribe();
        let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);

        // 复杂 Quest + HighTier → Deep
        let quest = make_quest("q-evt-1", make_parallel_tasks(20));
        let result = governor
            .select_mode_and_publish("q-evt-1", &quest, BudgetTier::HighTier)
            .await
            .expect("发布不应失败");

        assert!(result.is_some(), "首次选择应触发事件");
        let (mode, _) = result.unwrap();
        assert_eq!(mode, ThinkingMode::Deep);

        // 验证事件已发布
        let event = rx.recv().await.expect("应收到事件");
        match event {
            NexusEvent::ThinkingModeSwitched {
                quest_id,
                to_mode,
                reason,
                ..
            } => {
                assert_eq!(quest_id, "q-evt-1");
                assert_eq!(to_mode, "Deep");
                assert!(reason.contains("auto_select"), "reason: {reason}");
            }
            other => panic!("expected ThinkingModeSwitched, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_select_mode_and_publish_skips_same_mode() {
        let bus = event_bus::EventBus::new();
        let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus.clone());

        let quest = make_quest("q-evt-2", make_parallel_tasks(20));
        // 第一次选择:None → Deep,应发布事件
        let first = governor
            .select_mode_and_publish("q-evt-2", &quest, BudgetTier::HighTier)
            .await
            .unwrap();
        assert!(first.is_some());

        // 第二次选择:Deep → Deep(不变),应跳过
        let second = governor
            .select_mode_and_publish("q-evt-2", &quest, BudgetTier::HighTier)
            .await
            .unwrap();
        assert!(second.is_none(), "模式未变化不应重复发布");
    }

    #[tokio::test]
    async fn test_select_mode_and_publish_without_bus() {
        // 无 EventBus 时仍应正常工作(仅 tracing 记录)
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-evt-3", make_parallel_tasks(1));
        let result = governor
            .select_mode_and_publish("q-evt-3", &quest, BudgetTier::LowTier)
            .await;
        assert!(result.is_ok(), "无 EventBus 不应报错");
        assert!(result.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_on_budget_adjusted_and_publish() {
        let bus = event_bus::EventBus::new();
        let mut rx = bus.subscribe();
        let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);

        let quest = make_quest("q-evt-4", make_parallel_tasks(20));
        let result = governor
            .on_budget_adjusted_and_publish(
                "q-evt-4",
                BudgetTier::HighTier,
                BudgetTier::Degraded,
                &quest,
            )
            .await
            .unwrap();

        assert!(result.is_some());
        let (mode, _) = result.unwrap();
        assert_eq!(mode, ThinkingMode::Fast);

        let event = rx.recv().await.expect("应收到事件");
        match event {
            NexusEvent::ThinkingModeSwitched { to_mode, .. } => {
                assert_eq!(to_mode, "Fast");
            }
            other => panic!("expected ThinkingModeSwitched, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_override_mode_and_publish() {
        let bus = event_bus::EventBus::new();
        let mut rx = bus.subscribe();
        let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);

        let mode = governor
            .override_mode_and_publish("q-evt-5", ThinkingMode::Deep, BudgetTier::HighTier)
            .await
            .unwrap();
        assert_eq!(mode, ThinkingMode::Deep);

        let event = rx.recv().await.expect("应收到事件");
        match event {
            NexusEvent::ThinkingModeSwitched {
                quest_id, reason, ..
            } => {
                assert_eq!(quest_id, "q-evt-5");
                assert!(reason.contains("manual_override"), "reason: {reason}");
            }
            other => panic!("expected ThinkingModeSwitched, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_override_mode_and_publish_degraded_rejects_deep() {
        let bus = event_bus::EventBus::new();
        let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);

        let result = governor
            .override_mode_and_publish("q-evt-6", ThinkingMode::Deep, BudgetTier::Degraded)
            .await;
        assert!(matches!(
            result,
            Err(QuestError::TtgOverrideRejected { .. })
        ));
    }
}
