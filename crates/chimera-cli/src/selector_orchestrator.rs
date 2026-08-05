//! 选择器学习编排器 — S4 学习结果注入 HCW 窗口（PROBE P2.2）
//!
//! L10 Interface 组合根组件：构造 S4Learner + SelectorLearnerHolder，
//! 经回调闭包将演化后的 SelectorPolicy 写入 holder（零事件变体，128 基线保持）。
//!
//! # 热路径隔离
//! learn_step 为低频原语（≤1Hz 由调用方节流）；update 后自动 emit 策略
//! （holder 写锁每秒<1 次，读锁 ~10ns——热路径零影响）。
//!
//! # 错误处理
//! 学习错误（无效奖励/矩阵病态）经 anyhow 传播；holder panic/poison
//! 内建 fallback（hcw-window selector_learner.rs）——学习失败不影响主链路。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use hcw_window::SelectorLearnerHolder;
// PROBE F1: 召回哨兵（P2.3 生产接线——warn_only 只告警不升档）
use hcw_window::recall::eval::sentinel::RecallSentinel;
use nexus_contracts::SelectorPolicy;
// PROBE F2: 超窗兜底链（P3.2 生产接线——effective_fold 生产调用点）
use crate::overwindow_bridge::OverWindowBridge;
use hcw_window::types::WindowTier;
use omega_learner::s4_selector::{PolicySink, S4Context, S4Learner, S4Reward};
// PROBE P2.4: 影子模式评估（shadow_mode.rs 零改动复用——ADR-043 四条件）
use omega_learner::shadow_mode::{
    PromotionReadiness, RollbackSignal, ShadowComparisonReport, ShadowModeTracker, StrategyMetrics,
    DEFAULT_OBSERVATION_DAYS,
};
/// 选择器学习编排器（PROBE P2.2/P2.4）
/// 持有单一 S4Learner + 共享 SelectorLearnerHolder；
/// learn_step 执行 select→update→自动 emit（回调闭包→holder）。
///
/// # 影子模式（P2.4）
/// 日频评估链：shadow_record_day（报告 + 回滚检测）→ shadow_promote_if_ready
/// （ADR-043 四条件：EWMA≥0.7 / 胜率≥71.4% / 观察期≥14 天 / 无 ASA 干预）；
/// **Learned 仅在四条件全过后一次性注入**（R7 影子期误升档缓解）。
pub struct SelectorOrchestrator {
    learner: Arc<Mutex<S4Learner>>,
    holder: Arc<SelectorLearnerHolder>,
    // === PROBE P2.4: 影子模式评估状态 ===
    /// 影子跟踪器（日频报告累计 + 回滚检测；Mutex 包裹保持 &self API 一致性）
    shadow_tracker: Mutex<ShadowModeTracker>,
    /// 当前 EWMA（调用方每日聚合哨兵/收集器快照后更新）
    shadow_ewma: Mutex<f32>,
    /// 观察期内是否发生 ASA 干预（P2 无 ASA 接入，恒 false）
    asa_intervention: AtomicBool,
    /// 解冻注入防重（一次性注入护栏）
    promotion_injected: AtomicBool,
    // === PROBE F1: 召回哨兵（可选接线——None = 未启用，零行为变化）===
    /// 召回哨兵（每 N Quest 测量压缩产物召回；warn_only 只告警不升档）
    recall_sentinel: Option<RecallSentinel>,
    // === PROBE F2: 超窗兜底链（可选接线——None = 未启用，零行为变化）===
    /// 超窗兜底桥（语料 > 有效窗口时经 kvbsr→repo-wiki 两级检索装窗）
    overwindow_bridge: Option<Arc<OverWindowBridge>>,
}
impl SelectorOrchestrator {
    /// 创建编排器（默认 α=1.0；回调闭包捕获 holder）
    pub fn new() -> Result<Self> {
        let holder = Arc::new(SelectorLearnerHolder::new());
        let mut learner =
            S4Learner::with_default_alpha().map_err(|e| anyhow!(format!("S4 构造失败: {e}")))?;
        let holder_clone = Arc::clone(&holder);
        learner.set_policy_sink(Some(PolicySink::new(move |p: SelectorPolicy| {
            holder_clone.update_policy(p);
        })));
        Ok(Self {
            learner: Arc::new(Mutex::new(learner)),
            holder,
            shadow_tracker: Mutex::new(ShadowModeTracker::new(now_secs())),
            shadow_ewma: Mutex::new(0.0),
            asa_intervention: AtomicBool::new(false),
            promotion_injected: AtomicBool::new(false),
            recall_sentinel: None,
            overwindow_bridge: None,
        })
    }
    /// 返回共享 holder（注入 HcwWindow 用）
    pub fn holder(&self) -> Arc<SelectorLearnerHolder> {
        Arc::clone(&self.holder)
    }

    // === PROBE F1: 召回哨兵接线 ===

    /// 注入召回哨兵（builder；None = 未启用，零行为变化）
    ///
    /// # 参数
    /// - `sentinel`: 召回哨兵（每 N Quest 测量压缩产物召回）
    ///
    /// WHY builder: 保持 new() 签名不变（向后兼容）；哨兵为可选观测面
    pub fn with_sentinel(mut self, sentinel: RecallSentinel) -> Self {
        self.recall_sentinel = Some(sentinel);
        self
    }

    /// Quest 完成钩子：转发到哨兵（未启用时 no-op）
    ///
    /// # 返回
    /// - `Ok(None)`: 未到触发间隔或哨兵未启用
    /// - `Ok(Some(decision))`: 哨兵完成测量与判定（调用方记录即可）
    /// - `Err`: 测量失败（哨兵失败不影响主链路——调用方仅日志）
    pub async fn on_quest(
        &mut self,
    ) -> Result<Option<hcw_window::recall::eval::sentinel::SentinelDecision>> {
        match &mut self.recall_sentinel {
            Some(sentinel) => Ok(sentinel
                .on_quest()
                .await
                .map_err(|e| anyhow!(format!("哨兵测量失败: {e}")))?),
            None => Ok(None),
        }
    }

    // === PROBE F2: 超窗兜底链接线 ===

    /// 注入超窗兜底桥（builder；None = 未启用，零行为变化）
    ///
    /// # 参数
    /// - `bridge`: 超窗兜底桥（kvbsr→repo-wiki 两级检索真实链路）
    pub fn with_bridge(mut self, bridge: Arc<OverWindowBridge>) -> Self {
        self.overwindow_bridge = Some(bridge);
        self
    }

    /// 超窗兜底执行（P3.1 effective_fold 折减判定 + P3.2 两级检索）
    ///
    /// # 参数
    /// - `query`: 检索查询
    /// - `corpus`: 语料全文（首次调用会预构建块表——后续复用）
    /// - `corpus_tokens`: 语料规模（token）
    /// - `model_claimed`: 模型宣称窗口（token，如 1M）
    ///
    /// # 返回
    /// - `Ok(Some(outcome))`: 超窗触发——候选集（上层复用 P1 三区装窗）
    /// - `Ok(None)`: 未超窗（有效窗口内，走常规装窗）或桥未启用
    /// - `Err`: 桥空语料表（调用方未先 set_corpus）
    pub async fn run_overwindow(
        &self,
        query: &str,
        corpus: &str,
        corpus_tokens: u64,
        model_claimed: u64,
    ) -> Result<Option<hcw_window::recall::overwindow::OverWindowOutcome>> {
        let Some(bridge) = &self.overwindow_bridge else {
            return Ok(None);
        };
        // P3.1: 有效窗口 = 模型宣称 × 60%（生产调用点）
        let effective_window = WindowTier::effective_fold(model_claimed as usize) as u64;
        if corpus_tokens <= effective_window {
            return Ok(None);
        }
        bridge.set_corpus(corpus);
        let outcome = bridge
            .run(query, corpus_tokens, effective_window)
            .await
            .map_err(|e| anyhow!(format!("超窗兜底失败: {e}")))?;
        Ok(Some(outcome))
    }

    /// 学习步（低频 ≤1Hz 由调用方节流）— select→update→自动 emit
    pub fn learn_step(&self, context: &S4Context, reward: &S4Reward) -> Result<()> {
        let mut learner = self
            .learner
            .lock()
            .map_err(|_| anyhow!("S4Learner 锁 poisoned"))?;
        let weights = learner
            .select(context)
            .map_err(|e| anyhow!("S4 select 失败: {e}"))?;
        learner
            .update(context, weights, reward)
            .map_err(|e| anyhow!("S4 update 失败: {e}"))?;
        Ok(())
    }

    /// 当前策略（影子期对比用）
    pub fn current_policy(&self) -> SelectorPolicy {
        self.holder.current_policy()
    }

    // === PROBE P2.4: 影子模式评估链（shadow_mode.rs 零改动复用）===

    /// 记录每日影子对比报告（R1=Learned 影子策略 vs L3=Static 基线）
    ///
    /// # 参数
    /// - `r1`: R1 策略当日指标（needle_recall → recall_rate；composite 自动计算）
    /// - `l3`: L3 基线当日指标
    /// - `now_secs`: 当日 UTC 秒
    ///
    /// # 返回
    /// - `Some(RollbackSignal)`: 触发回滚（连续 3 天显著退化 / 召回下降≥5%）——
    ///   已自动 `fallback_to_static()`（影子期主路径零扰动）
    /// - `None`: 正常累积
    ///
    /// # 红线
    /// - 日频调用（调用方每日一次）；报告 O(14) 零压力
    pub fn shadow_record_day(
        &self,
        r1: StrategyMetrics,
        l3: StrategyMetrics,
        now_secs: i64,
    ) -> Option<RollbackSignal> {
        let mut tracker = self
            .shadow_tracker
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let ewma = *self.shadow_ewma.lock().unwrap_or_else(|p| p.into_inner());
        // 观察期剩余天数：以 tracker 已观察天数为准（展示字段）
        let elapsed = tracker
            .evaluate_promotion_readiness(now_secs, ewma)
            .elapsed_days;
        let remaining = DEFAULT_OBSERVATION_DAYS.saturating_sub(elapsed);
        let report = ShadowComparisonReport::new(
            chrono::DateTime::from_timestamp(now_secs, 0).unwrap_or_else(chrono::Utc::now),
            r1,
            l3,
            remaining,
        );
        let signal = tracker.record_daily_report(report);
        if signal.is_some() {
            // 回滚触发：影子策略退回 Static（主路径零扰动）
            self.holder.fallback_to_static();
        }
        signal
    }

    /// 更新当日 EWMA（调用方聚合哨兵/收集器快照后传入）
    ///
    /// # 返回
    /// - `Some(RollbackSignal::EwmaCollapse)`: 24h 内下降≥0.3（已自动 fallback）
    pub fn shadow_update_ewma(&self, ewma: f32, now_secs: i64) -> Option<RollbackSignal> {
        let mut tracker = self
            .shadow_tracker
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let signal = tracker.update_ewma(ewma, now_secs);
        if signal.is_some() {
            self.holder.fallback_to_static();
        }
        *self.shadow_ewma.lock().unwrap_or_else(|p| p.into_inner()) = ewma;
        signal
    }

    /// 评估解冻就绪状态（ADR-043 四条件）
    pub fn shadow_readiness(&self, now_secs: i64) -> PromotionReadiness {
        let tracker = self
            .shadow_tracker
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let ewma = *self.shadow_ewma.lock().unwrap_or_else(|p| p.into_inner());
        let mut readiness = tracker.evaluate_promotion_readiness(now_secs, ewma);
        // P2 无 ASA 接入：no_asa_intervention 恒真（保留字段语义，不触碰 ADR-043 逻辑）
        readiness.no_asa_intervention = !self.asa_intervention.load(Ordering::Relaxed);
        readiness
    }

    /// 四条件全过后一次性注入 Learned（R7：影子期注入链只读不写主策略）
    ///
    /// # 返回
    /// - `true`: 本次完成注入（幂等——已注入过则不再重复）
    /// - `false`: 未就绪或已注入
    pub fn shadow_promote_if_ready(&self, now_secs: i64) -> bool {
        if self.promotion_injected.load(Ordering::Relaxed) {
            return false;
        }
        let readiness = self.shadow_readiness(now_secs);
        if !readiness.is_ready() {
            return false;
        }
        // 一次性注入：Learned 策略（版本 = 当前学习步数，单调递增）
        let learner = self.learner.lock().unwrap_or_else(|p| p.into_inner());
        let policy = learner.current_policy(learner.total_steps());
        drop(learner);
        self.holder.update_policy(policy);
        self.promotion_injected.store(true, Ordering::Relaxed);
        true
    }
}

/// 当前 UTC 秒（tracker 构造用；测试可注入固定时间）
fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}
#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::SelectorWeights;
    use omega_learner::s4_selector::BlockType;

    #[test]
    fn test_learn_step_injects_learned_policy() {
        // 注入链闭环: learn_step → update → 自动 emit → holder 为 Learned
        let orch = SelectorOrchestrator::new().unwrap();
        assert!(
            orch.current_policy().is_static(),
            "初始应为 Static fallback"
        );
        let ctx = S4Context::new(BlockType::Code, 0.8, 0.5, 0.1).unwrap();
        let reward = S4Reward::new(0.2).unwrap(); // 低后悔 → 高奖励
        orch.learn_step(&ctx, &reward).unwrap();
        assert!(
            orch.current_policy().is_learned(),
            "学习后应为 Learned（回调注入）"
        );
        assert_eq!(orch.current_policy().version(), Some(1));
    }

    #[test]
    fn test_holder_shared_across_windows() {
        // 同一 holder 可注入多个窗口（with_learner 共享）
        let orch = SelectorOrchestrator::new().unwrap();
        let h1 = orch.holder();
        let h2 = orch.holder();
        h1.update_policy(nexus_contracts::SelectorPolicy::learned(
            9,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        assert!(h2.current_policy().is_learned(), "共享 holder 应同步");
    }

    // === PROBE P2.4: 影子模式测试（shadow_mode.rs 零改动复用验证）===

    const DAY: i64 = 86_400;

    /// 构造全胜日指标（R1 显著优于 L3 基线）
    fn winning_day() -> (StrategyMetrics, StrategyMetrics) {
        (
            StrategyMetrics::new(0.9, 0.05, 0.05, 100).unwrap(),
            StrategyMetrics::new(0.7, 0.1, 0.1, 100).unwrap(),
        )
    }

    /// 构造显著退化日指标（R1 远低于 L3）
    fn losing_day() -> (StrategyMetrics, StrategyMetrics) {
        (
            StrategyMetrics::new(0.2, 0.4, 0.4, 100).unwrap(),
            StrategyMetrics::new(0.7, 0.1, 0.1, 100).unwrap(),
        )
    }

    #[test]
    fn test_shadow_promote_after_14_winning_days() {
        // 14 天全胜 + EWMA 0.8 → 四条件全过 → 一次性注入 Learned（幂等）
        let orch = SelectorOrchestrator::new().unwrap();
        assert!(orch.current_policy().is_static(), "影子期主路径应为 Static");
        // 起点用真实当前时间（tracker 构造同为真实时间——避免未来时间戳导致 elapsed 失真）
        let start = now_secs();
        for day in 0..14 {
            let (r1, l3) = winning_day();
            assert!(
                orch.shadow_record_day(r1, l3, start + day * DAY).is_none(),
                "全胜日不应触发回滚"
            );
        }
        orch.shadow_update_ewma(0.8, start + 14 * DAY);
        let readiness = orch.shadow_readiness(start + 14 * DAY);
        assert!(
            readiness.is_ready(),
            "四条件应全过: {:?}",
            readiness.unmet_conditions()
        );
        // 一次性注入：Learned 生效 + 幂等
        assert!(orch.shadow_promote_if_ready(start + 14 * DAY));
        assert!(orch.current_policy().is_learned(), "解冻后应为 Learned");
        assert!(
            !orch.shadow_promote_if_ready(start + 14 * DAY),
            "已注入后不再重复"
        );
    }

    #[test]
    fn test_shadow_not_ready_before_observation_complete() {
        // 观察期未满（7 天）→ 不 ready
        let orch = SelectorOrchestrator::new().unwrap();
        let start = now_secs();
        for day in 0..7 {
            let (r1, l3) = winning_day();
            orch.shadow_record_day(r1, l3, start + day * DAY);
        }
        orch.shadow_update_ewma(0.8, start + 7 * DAY);
        assert!(!orch.shadow_readiness(start + 7 * DAY).is_ready());
        assert!(!orch.shadow_promote_if_ready(start + 7 * DAY));
        assert!(orch.current_policy().is_static(), "未解冻保持 Static");
    }

    #[test]
    fn test_shadow_rollback_on_consecutive_regression() {
        // 连续 3 天显著退化 → RollbackSignal::ConsecutiveRegression + 主路径保持 Static
        let orch = SelectorOrchestrator::new().unwrap();
        let start = now_secs();
        let mut signal = None;
        for day in 0..3 {
            let (r1, l3) = losing_day();
            signal = orch.shadow_record_day(r1, l3, start + day * DAY);
        }
        assert!(
            matches!(signal, Some(RollbackSignal::ConsecutiveRegression { .. })),
            "第 3 天应触发连续退化回滚"
        );
        assert!(
            orch.current_policy().is_static(),
            "回滚后保持 Static（主路径零扰动）"
        );
    }

    #[test]
    fn test_shadow_ewma_collapse_rollback() {
        // EWMA 崩塌（24h 内下降 ≥0.3）→ EwmaCollapse + fallback
        let orch = SelectorOrchestrator::new().unwrap();
        let start = now_secs();
        orch.shadow_update_ewma(0.9, start);
        let signal = orch.shadow_update_ewma(0.5, start + DAY);
        assert!(matches!(signal, Some(RollbackSignal::EwmaCollapse { .. })));
        assert!(orch.current_policy().is_static());
    }

    // === PROBE F1: 哨兵接线测试 ===

    #[tokio::test]
    async fn test_sentinel_forwarding_triggered() {
        // 哨兵接线闭环：with_sentinel → on_quest 转发 → 哨兵触发返回决策
        let bus = event_bus::EventBus::new();
        let sentinel =
            hcw_window::recall::eval::sentinel::RecallSentinel::new(bus).with_quest_interval(1);
        let mut orch = SelectorOrchestrator::new().unwrap().with_sentinel(sentinel);
        // 首次 on_quest（interval=1 → 触发测量）→ 返回 Some(决策)
        let decision = orch.on_quest().await.unwrap();
        assert!(
            decision.is_some(),
            "哨兵接线后 on_quest 应返回决策（触发测量）"
        );
    }

    #[tokio::test]
    async fn test_sentinel_none_is_noop() {
        // 未启用哨兵：on_quest 返回 None（零行为变化）
        let mut orch = SelectorOrchestrator::new().unwrap();
        assert!(orch.on_quest().await.unwrap().is_none());
    }

    // === PROBE F2: 超窗兜底链接线测试 ===

    /// 构造超窗语料（>400K token 估算）
    fn big_corpus() -> String {
        let base = "模块A 处理请求路由与鉴权，模块B 负责缓存失效与回写，模块C 执行语义检索。";
        let mut corpus = String::with_capacity(2_400_000);
        for i in 0..80_000 {
            corpus.push_str(&format!("{base} 段{i} "));
        }
        corpus
    }

    #[tokio::test]
    async fn test_run_overwindow_triggered() {
        // F2 闭环：with_bridge → 超窗 → Some(outcome)（effective_fold 生产调用）
        let bus = event_bus::EventBus::new();
        let bridge = crate::overwindow_bridge::OverWindowBridge::new(bus).unwrap();
        let orch = SelectorOrchestrator::new()
            .unwrap()
            .with_bridge(std::sync::Arc::new(bridge));
        let corpus = big_corpus();
        let corpus_tokens = (corpus.chars().count() / 4) as u64;
        let outcome = orch
            .run_overwindow("语义检索", &corpus, corpus_tokens, 1_048_576)
            .await
            .unwrap();
        // 1M 宣称 → 折减 600K < 语料 → 触发
        assert!(
            outcome.is_some(),
            "超窗应触发兜底（effective_fold 生产调用）"
        );
        let outcome = outcome.unwrap();
        assert!(outcome.triggered);
        assert!(outcome.candidate_count > 0, "候选集不应为空");
    }

    #[tokio::test]
    async fn test_run_overwindow_within_window() {
        // 未超窗（语料 < 折减窗口）→ None（零开销）
        let bus = event_bus::EventBus::new();
        let bridge = crate::overwindow_bridge::OverWindowBridge::new(bus).unwrap();
        let orch = SelectorOrchestrator::new()
            .unwrap()
            .with_bridge(std::sync::Arc::new(bridge));
        let outcome = orch
            .run_overwindow("查询", "小语料", 1_000, 1_048_576)
            .await
            .unwrap();
        assert!(outcome.is_none(), "有效窗口内不触发兜底");
    }

    #[tokio::test]
    async fn test_run_overwindow_no_bridge() {
        // 未接线桥：返回 None（零行为变化）
        let orch = SelectorOrchestrator::new().unwrap();
        let outcome = orch
            .run_overwindow("查询", "语料", 10_000_000, 1_048_576)
            .await
            .unwrap();
        assert!(outcome.is_none(), "无桥接线不触发");
    }
}
