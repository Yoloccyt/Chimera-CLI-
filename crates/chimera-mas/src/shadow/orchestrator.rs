//! 影子模式编排器 — 熔断/范围/证据门/胜率门的组合裁决(ADR-053 备忘录 §五 B-4)
//!
//! 对应架构层: L9 Quest(chimera-mas shadow 子模块)
//! 对应 ADR: ADR-053-rev4(权威版)+ ADR-042(R2 冻结)+ ADR-052(解冻三阶)
//!
//! # WHY 本模块不违反 R2 冻结(合规声明,仿 decay-engine shadow_breaker 范式)
//!
//! 本编排器**不执行任何 RL 训练、无梯度更新、不含 5 个 R2 扫描关键词**。
//! 它只做一件事:把已就位的四道安全护栏(熔断器/范围守卫/外部证据门/
//! 批次胜率门)按 fail-closed 短路顺序**组合**成统一裁决点,终判仅产出
//! [`PromotionAdvice`] **建议**——真正解冻须阶段③ 实跑满门 + ADR-054
//! 三方复核 + 用户治理签署,编排器无解冻能力。构建它**收紧**而非放松约束:
//!
//! - 无治理签署配置 → 编排器不可实例化(见 config 模块 fail-closed 构造)
//! - 熔断器跳闸 → 一切批次摄入与终判短路拒绝
//! - 6 项须实跑前置([`Stage3Prerequisites`])未就绪 → 终判拒绝晋级建议
//!   (rev4 诚实二分:不把待标定项伪装成已闭合)
//!
//! # 裁决链(顺序固定,fail-closed 短路)
//!
//! ```text
//! 熔断器(L4 ShadowModeCircuitBreaker,Tripped 即全拒)
//!   → 范围(L5 UnfreezeScope,OutOfScope 即拒)
//!   → 证据门(资格层 s_min/AHIRT 硬门 → 排名层加权 ε_win)
//!   → 批次账本(检查点门控,防 optional stopping)
//!   → 胜率门(min(Wilson, bootstrap) 单侧 95% 下界 > 0.5)
//!   → 前置就绪门(6 项须实跑,缺一不出晋级建议)
//! ```
//!
//! # AHIRT 门禁语义接线(ADR-053 备忘录 §五 B-5)
//!
//! [`AhirtEvidenceCollector`] 订阅 event-bus 双通道聚合 D 维证据:
//! - `AhirtProbeCompleted`(broadcast):按攻击类别累计探测统计;
//!   **丢失天然 fail-closed**——本批未收到事件即证据缺失计非胜(rev3)
//! - `RedTeamAudit`(Critical mpsc 旁路,必达):观测到即标记批窗口
//!   安全告警 → 整批非胜
//!
//! 订阅遵循 §4.4 反模式 3(spawn 前同步 subscribe)与反模式 1
//! (锁不跨 await),模板取自 efficiency-monitor `start_event_subscriber`。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use decay_engine::{ResetAuthorization, RlGateVerdict, ShadowModeCircuitBreaker};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use gsoe_evolution::{RlUpdateTarget, ScopeVerdict, UnfreezeScope};
use nexus_contracts::formal_props::VerificationResult;

use crate::error::{MasError, Result};
use crate::shadow::batch::{
    BatchLedger, BatchRecord, Checkpoint, BASE_BATCHES, EXTENDED_BATCHES, EXTENSION_BAND,
};
use crate::shadow::config::ShadowModeConfig;
use crate::shadow::evidence_gate::{
    AhirtBatchEvidence, AhirtCategoryStats, BatchEvidence, BatchVerdict, EvidenceGate,
};
use crate::shadow::stats::{effective_lower_bound, EffectiveLowerBound};

// ============================================================
// 阶段③ 前置就绪清单(rev4 "6 项须实跑",不伪装闭合)
// ============================================================

/// 阶段③ 启动前置就绪清单 — rev4 显式登记的 6 项须实跑/须工程项
///
/// 全部默认 `false`(fail-closed):每项须凭**真实工程交付物**由治理面
/// 显式置位;任一未就绪时 [`ShadowModeOrchestrator::checkpoint_advice`]
/// 不产出晋级建议(rev4 诚实二分:不把待标定项伪装成已闭合)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stage3Prerequisites {
    /// R3-E06-2:α 复合错误率写入 + 扩展带定义冻结(须实跑数据)
    pub alpha_composite_calibrated: bool,
    /// R3-E06-3:功效换算 + 批内相关实测(须实跑 σ̂_d)
    pub power_intra_batch_verified: bool,
    /// R3-E06-4:binomial_sf 注释订正(significance.rs + shadow_mode.rs)
    pub binomial_sf_comments_corrected: bool,
    /// R3-E02-2:载荷库轮换/随机化载荷生成(须新建代码)
    pub payload_rotation_ready: bool,
    /// R3-E02-3:cargo-llvm-cov 独立插桩在 seccore 沙箱工程就绪(启动前置硬门)
    pub coverage_instrumentation_ready: bool,
    /// s_min 具体档位 + A/B/C 权重终值经用户确认(治理拍板项)
    pub s_min_final_confirmed: bool,
}

impl Stage3Prerequisites {
    /// 全部就绪?
    #[must_use]
    pub fn all_ready(&self) -> bool {
        self.missing().is_empty()
    }

    /// 未就绪项清单(人类可读,供审计与终判拒绝原因)
    #[must_use]
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.alpha_composite_calibrated {
            missing.push("R3-E06-2 α 复合错误率标定");
        }
        if !self.power_intra_batch_verified {
            missing.push("R3-E06-3 功效换算 + 批内相关实测");
        }
        if !self.binomial_sf_comments_corrected {
            missing.push("R3-E06-4 binomial_sf 注释订正");
        }
        if !self.payload_rotation_ready {
            missing.push("R3-E02-2 载荷轮换/随机化");
        }
        if !self.coverage_instrumentation_ready {
            missing.push("R3-E02-3 独立插桩就绪");
        }
        if !self.s_min_final_confirmed {
            missing.push("s_min/权重终值用户确认");
        }
        missing
    }
}

// ============================================================
// 终判建议类型
// ============================================================

/// 晋级终判建议 — **仅建议,无解冻效力**(解冻属 ADR-054 + 用户治理)
#[derive(Debug, Clone, PartialEq)]
pub enum PromotionAdvice {
    /// 建议晋级:满门通过,建议另立 ADR-054 三方复核
    Promote {
        /// 有效下界(含 Wilson/bootstrap/哨兵审计分解)
        lower_bound: EffectiveLowerBound,
        /// 胜数 / 批数
        wins: usize,
        /// 批数
        batches: usize,
    },
    /// 下界落扩展带 [0.45, 0.5]:按预注册规则扩展至 25 批(唯一一次)
    ExtendTo25 {
        /// 14 批时点的有效下界
        lower_bound: EffectiveLowerBound,
    },
    /// 不建议晋级(携带原因:下界不足 / 前置未就绪)
    NotPromote {
        /// 检查点有效下界
        lower_bound: EffectiveLowerBound,
        /// 拒绝原因(人类可读,供审计)
        reason: String,
    },
}

// ============================================================
// 影子模式编排器
// ============================================================

/// 影子模式编排器 — 见模块级文档
///
/// 所有方法为同步方法(§4.4 反模式 8),不持有异步资源;
/// AHIRT 事件消费由独立的 [`AhirtEvidenceCollector`] 承担。
pub struct ShadowModeOrchestrator {
    /// 治理配置(构造期已过签署校验)
    config: ShadowModeConfig,
    /// L4 熔断器(fail-closed,任一形式化属性 Violated 即永久跳闸)
    breaker: ShadowModeCircuitBreaker,
    /// L5 解冻范围守卫(fail-closed 白名单)
    scope: UnfreezeScope,
    /// 影子评估目标(须在 scope 白名单内)
    target: RlUpdateTarget,
    /// 外部证据门(资格层 + 排名层)
    gate: EvidenceGate,
    /// 批次账本(检查点门控)
    ledger: BatchLedger,
    /// 阶段③ 前置就绪清单(默认全未就绪)
    prerequisites: Stage3Prerequisites,
    /// bootstrap 预注册种子(审计:同序列同种子 → 同下界,杜绝重跑挑选)
    bootstrap_seed: u64,
    /// WS-4B:可选事件总线 — 晋级条件满足时发布 `R1ShadowPromotionReady`
    event_bus: Option<EventBus>,
}

impl std::fmt::Debug for ShadowModeOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShadowModeOrchestrator")
            .field("config", &self.config)
            .field("breaker", &self.breaker)
            .field("scope", &self.scope)
            .field("target", &self.target)
            .field("gate", &self.gate)
            .field("ledger", &self.ledger)
            .field("prerequisites", &self.prerequisites)
            .field("bootstrap_seed", &self.bootstrap_seed)
            // event_bus:EventBus 未实现 Debug,不输出
            .finish()
    }
}

impl ShadowModeOrchestrator {
    /// 构造编排器
    ///
    /// `config` 已在构造期完成治理签署校验(无签署配置在上游即构造失败,
    /// 编排器天然不可能以未签署状态存在)。熔断器初始 Armed、前置清单
    /// 全未就绪、账本为空——全部 fail-closed 初始态。
    #[must_use]
    pub fn new(
        config: ShadowModeConfig,
        scope: UnfreezeScope,
        target: RlUpdateTarget,
        bootstrap_seed: u64,
    ) -> Self {
        let gate = EvidenceGate::new(config.clone());
        Self {
            config,
            breaker: ShadowModeCircuitBreaker::new(),
            scope,
            target,
            gate,
            ledger: BatchLedger::new(),
            prerequisites: Stage3Prerequisites::default(),
            bootstrap_seed,
            event_bus: None,
        }
    }

    /// 链式注入事件总线 — 晋级条件满足时发布 `R1ShadowPromotionReady`(WS-4B)。
    ///
    /// 同时将总线注入批次账本,使连续非胜回归检测发布 `R1ShadowRegressionDetected`。
    #[must_use]
    pub fn with_event_bus(
        config: ShadowModeConfig,
        scope: UnfreezeScope,
        target: RlUpdateTarget,
        bootstrap_seed: u64,
        bus: EventBus,
    ) -> Self {
        let mut orch = Self::new(config, scope, target, bootstrap_seed);
        orch.event_bus = Some(bus.clone());
        orch.ledger = BatchLedger::new().with_event_bus(bus);
        orch
    }

    /// 喂入形式化验证器观测(转发给 L4 熔断器)
    ///
    /// 任一 `Violated` → 熔断器永久跳闸,后续批次摄入与终判全部短路拒绝。
    pub fn observe_verifications(&mut self, results: &[VerificationResult]) -> RlGateVerdict {
        self.breaker.observe(results)
    }

    /// 摄入一批影子评估证据并裁决胜负
    ///
    /// 裁决链前两级(熔断/范围)短路拒绝返回 `Err`;证据门裁决(胜/非胜)
    /// 属正常业务结果返回 `Ok(BatchVerdict)`。
    ///
    /// # 错误
    /// - 熔断器已跳闸 → [`MasError::ShadowGateRejected`](携带跳闸原因)
    /// - 评估目标不在解冻范围白名单 → [`MasError::ShadowGateRejected`]
    /// - 批次 ID 重复 / 检查点外加批 → 由账本拒绝(防 optional stopping)
    pub fn ingest_batch(
        &mut self,
        batch_id: impl Into<String>,
        lineage_snapshot_hash: impl Into<String>,
        evidence: &BatchEvidence,
    ) -> Result<BatchVerdict> {
        self.ensure_not_tripped()?;
        if let ScopeVerdict::OutOfScope { reason } = self.scope.is_in_scope(&self.target) {
            return Err(MasError::ShadowGateRejected {
                reason: format!("评估目标不在解冻范围内:{reason}"),
            });
        }

        let verdict = self.gate.evaluate(evidence);
        let (win, reasons) = match &verdict {
            BatchVerdict::Win => (true, Vec::new()),
            BatchVerdict::NonWin { reasons } => (false, reasons.clone()),
        };
        self.ledger.record(BatchRecord {
            batch_id: batch_id.into(),
            lineage_snapshot_hash: lineage_snapshot_hash.into(),
            win,
            reasons,
        })?;

        tracing::info!(
            batches = self.ledger.len(),
            win,
            "影子批次已裁决入账(结果永不回流进化谱系)"
        );
        Ok(verdict)
    }

    /// 预注册检查点终判(仅 n=14 / n=25 可调用,防 optional stopping)
    ///
    /// 判定(rev4 决策 6 细化):
    /// - 有效下界 = min(Wilson, bootstrap)(哨兵拒绝时)单侧 95%,恒 ≤ Wilson
    /// - 下界 > 0.5 且 6 项前置全就绪 → [`PromotionAdvice::Promote`]
    ///   (仅建议,解冻须 ADR-054);前置缺项 → `NotPromote`(不伪装闭合)
    /// - 14 批下界落扩展带 [0.45, 0.5] → 激活预注册扩展 → `ExtendTo25`
    /// - 其余 → `NotPromote`
    ///
    /// # 错误
    /// - 熔断器已跳闸 → [`MasError::ShadowGateRejected`]
    /// - 当前批数不在预注册检查点 → [`MasError::ShadowGateRejected`]
    pub fn checkpoint_advice(&mut self) -> Result<PromotionAdvice> {
        self.ensure_not_tripped()?;
        let (checkpoint, outcomes) =
            self.ledger
                .outcomes_at_checkpoint()
                .ok_or_else(|| MasError::ShadowGateRejected {
                    reason: format!(
                    "当前 {} 批不在预注册检查点({BASE_BATCHES}/{EXTENDED_BATCHES}),禁止中途终判",
                    self.ledger.len()
                ),
                })?;

        let lb = effective_lower_bound(&outcomes, self.bootstrap_seed);
        let wins = self.ledger.wins();
        let batches = self.ledger.len();

        if lb.value > 0.5 {
            let missing = self.prerequisites.missing();
            if missing.is_empty() {
                // WS-4B:满门通过 → 发布 R1ShadowPromotionReady 晋级就绪通知
                self.publish_promotion_ready(wins, batches, lb.value);
                return Ok(PromotionAdvice::Promote {
                    lower_bound: lb,
                    wins,
                    batches,
                });
            }
            return Ok(PromotionAdvice::NotPromote {
                lower_bound: lb,
                reason: format!(
                    "下界达标但阶段③ 前置未就绪(不伪装闭合):{}",
                    missing.join(" / ")
                ),
            });
        }

        // 扩展带仅在基础检查点生效且只能触发一次(账本强制)
        if checkpoint == Checkpoint::Base && lb.value >= EXTENSION_BAND.0 {
            self.ledger.activate_extension()?;
            return Ok(PromotionAdvice::ExtendTo25 { lower_bound: lb });
        }

        Ok(PromotionAdvice::NotPromote {
            reason: format!("有效下界 {:.4} ≤ 0.5,不建议晋级", lb.value),
            lower_bound: lb,
        })
    }

    /// 置位阶段③ 前置就绪清单
    ///
    /// 调用方须凭真实工程交付物置位(如插桩就绪验收报告);置位记录
    /// 由上层治理面审计,本方法只做状态承载。
    pub fn set_prerequisites(&mut self, prerequisites: Stage3Prerequisites) {
        self.prerequisites = prerequisites;
    }

    /// 人工复位熔断器(须携带 S-2.1 授权凭证,强制问责)
    pub fn reset_breaker(&mut self, authorization: ResetAuthorization) {
        self.breaker.reset(authorization);
    }

    /// 熔断器只读访问(状态/跳闸原因/观测数审计)
    #[must_use]
    pub fn breaker(&self) -> &ShadowModeCircuitBreaker {
        &self.breaker
    }

    /// 治理配置只读访问
    #[must_use]
    pub fn config(&self) -> &ShadowModeConfig {
        &self.config
    }

    /// 已入账批数
    #[must_use]
    pub fn batches(&self) -> usize {
        self.ledger.len()
    }

    /// 熔断短路检查(fail-closed 第一级)
    fn ensure_not_tripped(&self) -> Result<()> {
        if self.breaker.is_tripped() {
            return Err(MasError::ShadowGateRejected {
                reason: format!(
                    "熔断器已跳闸(fail-closed 短路):{}",
                    self.breaker.trip_cause().unwrap_or("未知原因")
                ),
            });
        }
        Ok(())
    }

    /// 发布 `R1ShadowPromotionReady` 事件 — R1 影子模式解冻就绪(WS-4B)。
    ///
    /// `win_rate` 为批次计胜率(30 天观察期胜率代理),`ewma_level` 用
    /// 有效下界近似(解冻条件 1 的成败率信号)。仅建议,解冻仍须 ADR-054。
    /// 未注入 EventBus 时静默跳过;发布失败仅 warn,不阻断晋级建议产出。
    fn publish_promotion_ready(&self, wins: usize, batches: usize, lb_value: f64) {
        if let Some(bus) = &self.event_bus {
            let win_rate = if batches > 0 {
                wins as f64 / batches as f64
            } else {
                0.0
            };
            let event = NexusEvent::R1ShadowPromotionReady {
                metadata: EventMetadata::new("chimera-mas"),
                report_date: Utc::now(),
                win_rate,
                ewma_level: lb_value as f32,
            };
            if let Err(e) = bus.publish_blocking(event) {
                tracing::warn!(error = %e, "发布 R1ShadowPromotionReady 事件失败");
            }
        }
    }
}

// ============================================================
// AHIRT 证据采集器(事件订阅侧)
// ============================================================

/// AHIRT 批窗口聚合状态(采集器内部,Mutex 保护)
#[derive(Debug, Default)]
struct AhirtWindow {
    /// 类别 → (total, failed) 累计
    ///
    /// WHY BTreeMap:类别数固定 4,导出时按类别名有序,证据可复现比对。
    categories: BTreeMap<String, (u32, u32)>,
    /// 窗口内是否观测到 RedTeamAudit Critical 告警
    red_team_audit_seen: bool,
}

/// AHIRT 证据采集器 — 订阅双通道聚合 D 维证据(备忘录 §五 B-5 接线)
///
/// # 通道分工(仿 efficiency-monitor 双通道职责拆分)
/// - broadcast 主通道:`AhirtProbeCompleted` 累计探测统计
///   (Lagged 丢失 → 证据不足计非胜,天然 fail-closed,可容忍)
/// - Critical mpsc 旁路:`RedTeamAudit` 必达,观测到即标记安全告警
///
/// # 生命周期
/// 后台任务由 `JoinHandle` 管理,[`Drop`] 时 abort——采集器随批次
/// 评估会话存亡,不做 fire-and-forget(证据窗口有明确归属)。
#[derive(Debug)]
pub struct AhirtEvidenceCollector {
    window: Arc<Mutex<AhirtWindow>>,
    handle: tokio::task::JoinHandle<()>,
}

impl AhirtEvidenceCollector {
    /// 启动采集器(须在 tokio runtime 上下文中调用)
    ///
    /// WHY spawn 前同步 subscribe(§4.4 反模式 3):tokio broadcast 仅投递
    /// 给发布时已存在的 receiver,在 spawn 的 async block 内 subscribe
    /// 会因调度时机不确定而静默丢失事件。
    #[must_use]
    pub fn start(bus: &EventBus) -> Self {
        let mut rx = bus.subscribe();
        let mut critical_rx = bus.subscribe_critical_events();
        let window = Arc::new(Mutex::new(AhirtWindow::default()));
        let window_task = Arc::clone(&window);

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Critical mpsc 旁路:RedTeamAudit 必达(通道关闭即退出)
                    critical = critical_rx.recv() => {
                        match critical {
                            Some(event) => Self::on_critical_event(&window_task, &event),
                            None => break,
                        }
                    }
                    // broadcast 主通道:探测统计(慢消费丢弃容忍,其余错误退出)
                    received = rx.recv() => {
                        match received {
                            Ok(event) => Self::on_broadcast_event(&window_task, &event),
                            // Lagged 丢失 → 本批证据不足计非胜(fail-closed),继续消费
                            Err(event_bus::EventBusError::SlowConsumerDropped { .. }) => continue,
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        Self { window, handle }
    }

    /// 处理 Critical 旁路事件(锁在同步函数内,不跨 await)
    fn on_critical_event(window: &Arc<Mutex<AhirtWindow>>, event: &NexusEvent) {
        if matches!(event, NexusEvent::RedTeamAudit { .. }) {
            // WHY unwrap_or_else:毒锁降级访问(与 event-bus 处理一致);
            // 窗口只含计数器,降级读写不破坏安全语义(标记只会置 true)
            let mut guard = window.lock().unwrap_or_else(|e| e.into_inner());
            guard.red_team_audit_seen = true;
        }
    }

    /// 处理 broadcast 主通道事件(锁在同步函数内,不跨 await)
    fn on_broadcast_event(window: &Arc<Mutex<AhirtWindow>>, event: &NexusEvent) {
        if let NexusEvent::AhirtProbeCompleted {
            probe_type,
            total,
            failed,
            ..
        } = event
        {
            let mut guard = window.lock().unwrap_or_else(|e| e.into_inner());
            let entry = guard.categories.entry(probe_type.clone()).or_insert((0, 0));
            entry.0 = entry.0.saturating_add(*total);
            entry.1 = entry.1.saturating_add(*failed);
        }
    }

    /// 导出并清空当前批窗口证据(每批评估收口时调用)
    ///
    /// 返回 `None` 表示窗口内未收到任何 AHIRT 完成事件 —— 调用方应将
    /// [`BatchEvidence::ahirt`] 置 `None`(证据缺失计非胜,rev3 语义)。
    #[must_use]
    pub fn take_window(&self) -> Option<AhirtBatchEvidence> {
        let mut guard = self.window.lock().unwrap_or_else(|e| e.into_inner());
        let taken = std::mem::take(&mut *guard);
        if taken.categories.is_empty() && !taken.red_team_audit_seen {
            return None;
        }
        Some(AhirtBatchEvidence {
            categories: taken
                .categories
                .into_iter()
                .map(|(category, (total, failed))| AhirtCategoryStats {
                    category,
                    total,
                    failed,
                })
                .collect(),
            red_team_audit_seen: taken.red_team_audit_seen,
        })
    }
}

impl Drop for AhirtEvidenceCollector {
    /// 采集器销毁时终止后台订阅任务(生命周期与评估会话绑定,
    /// 避免孤儿任务持有 receiver 拖慢 broadcast)
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// ============================================================
// 单元测试(编排器组合逻辑;E2E 三路径见 tests/e2e/shadow_orchestrator_e2e.rs)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow::config::GovernanceSignoff;
    use crate::shadow::evidence_gate::DimensionScores;
    use pvl_layer::Verification;

    fn orchestrator() -> ShadowModeOrchestrator {
        let signoff =
            GovernanceSignoff::new("user", "ADR-053-rev4", "2026-07-29").expect("合法签署");
        let config = ShadowModeConfig::anchor_profile(signoff).expect("锚点档合法");
        let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
        ShadowModeOrchestrator::new(config, scope, RlUpdateTarget::GsoeVariantSelection, 42)
    }

    fn winning_evidence() -> BatchEvidence {
        BatchEvidence {
            candidate: DimensionScores {
                execution: 0.95,
                mutation: 0.7,
                held_out: 0.7,
            },
            baseline: DimensionScores {
                execution: 0.9,
                mutation: 0.6,
                held_out: 0.6,
            },
            candidate_verification: Verification {
                passed: true,
                pass_rate: 0.95,
                real_execution: true,
                errors: Vec::new(),
            },
            ahirt: Some(AhirtBatchEvidence {
                categories: ["a", "b", "c", "d"]
                    .iter()
                    .map(|&c| AhirtCategoryStats {
                        category: c.into(),
                        total: 25,
                        failed: 0,
                    })
                    .collect(),
                red_team_audit_seen: false,
            }),
        }
    }

    /// 范围外目标短路拒绝(fail-closed 第二级)
    #[test]
    fn test_out_of_scope_rejected() {
        let signoff =
            GovernanceSignoff::new("user", "ADR-053-rev4", "2026-07-29").expect("合法签署");
        let config = ShadowModeConfig::anchor_profile(signoff).expect("锚点档合法");
        // 全冻结范围:任何目标 OutOfScope
        let mut orch = ShadowModeOrchestrator::new(
            config,
            UnfreezeScope::frozen(),
            RlUpdateTarget::GsoeVariantSelection,
            42,
        );
        let result = orch.ingest_batch("b1", "hash", &winning_evidence());
        assert!(matches!(result, Err(MasError::ShadowGateRejected { .. })));
    }

    /// 熔断跳闸后批次摄入与终判全部短路(fail-closed 第一级)
    #[test]
    fn test_tripped_breaker_short_circuits() {
        let mut orch = orchestrator();
        // 喂入 Violated 观测使熔断器跳闸
        let violated = VerificationResult::Violated {
            counterexample: "测试反例".into(),
            samples_tested: 1,
        };
        let verdict = orch.observe_verifications(&[violated]);
        assert!(!verdict.is_permitted());
        assert!(orch.breaker().is_tripped());

        assert!(orch
            .ingest_batch("b1", "hash", &winning_evidence())
            .is_err());
        assert!(orch.checkpoint_advice().is_err());
    }

    /// 检查点外终判拒绝(防 optional stopping)
    #[test]
    fn test_advice_outside_checkpoint_rejected() {
        let mut orch = orchestrator();
        for i in 0..5 {
            orch.ingest_batch(format!("b{i}"), "hash", &winning_evidence())
                .expect("批次应入账");
        }
        assert!(orch.checkpoint_advice().is_err(), "5 批不是检查点");
    }

    /// 14 批全胜 + 前置未就绪 → NotPromote(不伪装闭合)
    #[test]
    fn test_prerequisites_gate_blocks_promotion() {
        let mut orch = orchestrator();
        for i in 0..BASE_BATCHES {
            orch.ingest_batch(format!("b{i}"), "hash", &winning_evidence())
                .expect("批次应入账");
        }
        match orch.checkpoint_advice().expect("14 批检查点终判应可执行") {
            PromotionAdvice::NotPromote {
                lower_bound,
                reason,
            } => {
                assert!(lower_bound.value > 0.5, "全胜下界应 >0.5");
                assert!(reason.contains("前置未就绪"), "拒绝原因应指向前置:{reason}");
            }
            other => panic!("前置未就绪不应产出其他建议:{other:?}"),
        }
    }

    /// 14 批全胜 + 前置全就绪 → Promote(仅建议)
    #[test]
    fn test_full_promotion_path() {
        let mut orch = orchestrator();
        orch.set_prerequisites(Stage3Prerequisites {
            alpha_composite_calibrated: true,
            power_intra_batch_verified: true,
            binomial_sf_comments_corrected: true,
            payload_rotation_ready: true,
            coverage_instrumentation_ready: true,
            s_min_final_confirmed: true,
        });
        for i in 0..BASE_BATCHES {
            orch.ingest_batch(format!("b{i}"), "hash", &winning_evidence())
                .expect("批次应入账");
        }
        match orch.checkpoint_advice().expect("终判应可执行") {
            PromotionAdvice::Promote {
                wins,
                batches,
                lower_bound,
            } => {
                assert_eq!(wins, BASE_BATCHES);
                assert_eq!(batches, BASE_BATCHES);
                assert!(lower_bound.value > 0.5);
            }
            other => panic!("满门通过应产出 Promote:{other:?}"),
        }
    }

    /// WS-4B:R1ShadowPromotionReady 幽灵事件生产者验证 —
    /// 满门通过晋级时发布晋级就绪事件。
    #[test]
    fn test_full_promotion_path_publishes_ready_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let signoff =
            GovernanceSignoff::new("user", "ADR-053-rev4", "2026-07-29").expect("合法签署");
        let config = ShadowModeConfig::anchor_profile(signoff).expect("锚点档合法");
        let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
        let mut orch = ShadowModeOrchestrator::with_event_bus(
            config,
            scope,
            RlUpdateTarget::GsoeVariantSelection,
            42,
            bus,
        );
        orch.set_prerequisites(Stage3Prerequisites {
            alpha_composite_calibrated: true,
            power_intra_batch_verified: true,
            binomial_sf_comments_corrected: true,
            payload_rotation_ready: true,
            coverage_instrumentation_ready: true,
            s_min_final_confirmed: true,
        });
        for i in 0..BASE_BATCHES {
            orch.ingest_batch(format!("b{i}"), "hash", &winning_evidence())
                .expect("批次应入账");
        }
        match orch.checkpoint_advice().expect("终判应可执行") {
            PromotionAdvice::Promote { .. } => {}
            other => panic!("满门通过应产出 Promote:{other:?}"),
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let event = loop {
            if let Ok(Some(e)) = rx.try_recv() {
                break e;
            }
            assert!(std::time::Instant::now() < deadline, "接收晋级事件超时");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(
            event.metadata().source,
            "chimera-mas",
            "R1ShadowPromotionReady 事件 source 应为 chimera-mas"
        );
        match event {
            NexusEvent::R1ShadowPromotionReady {
                win_rate,
                ewma_level,
                ..
            } => {
                assert!((win_rate - 1.0).abs() < 1e-6, "全胜批次 win_rate 应为 1.0");
                assert!(ewma_level > 0.5, "满门下界应 > 0.5");
            }
            other => panic!("期望 R1ShadowPromotionReady 事件,收到 {other:?}"),
        }
    }

    /// 下界落扩展带 → ExtendTo25 且账本接受第 15 批
    #[test]
    fn test_extension_band_activates_extension() {
        let mut orch = orchestrator();
        // 10/14 胜:Wilson 下界 ≈0.494 ∈ [0.45, 0.5) → 扩展
        // 交替排列避免游程哨兵拒绝干扰(独立路径下界=Wilson)
        let evidence_win = winning_evidence();
        let mut evidence_lose = winning_evidence();
        evidence_lose.candidate.mutation = 0.3; // B 维踩硬门 → 非胜
        let mut wins = 0;
        for i in 0..BASE_BATCHES {
            // 前 4 批中安插 4 个非胜(10 胜 4 负,近似交替分布)
            let is_win = !(matches!(i, 1 | 4 | 7 | 10));
            let ev = if is_win {
                wins += 1;
                &evidence_win
            } else {
                &evidence_lose
            };
            orch.ingest_batch(format!("b{i}"), "hash", ev)
                .expect("批次应入账");
        }
        assert_eq!(wins, 10);
        match orch.checkpoint_advice().expect("14 批终判应可执行") {
            PromotionAdvice::ExtendTo25 { lower_bound } => {
                assert!(
                    lower_bound.value >= EXTENSION_BAND.0 && lower_bound.value <= EXTENSION_BAND.1
                );
            }
            other => panic!("扩展带应产出 ExtendTo25:{other:?}"),
        }
        // 扩展激活后可继续入账
        orch.ingest_batch("b14", "hash", &winning_evidence())
            .expect("扩展激活后第 15 批应入账");
    }
}
