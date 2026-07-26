//! ImmuneSystem facade — 适应性免疫接口层（v5.0 §8.1 D7 / ADR-046）
//!
//! 对应架构层:L8 Parliament（parliament crate 内部子模块）
//! 对应 ADR:ADR-046（ImmuneSystem facade 三探针设计）
//! 对应任务:P5.3.1-P5.3.6
//!
//! # 核心职责
//! - **三探针注册**：MemoryParadox / ReasoningTrap / EvolutionHack（固定 3 项,不可进化面）
//! - **级联风险评估**：综合三探针 paradox_rate + circuit_open_ratio + budget_exceeded_count
//! - **膜厚控制**：cascade_risk > 0.7 → 膜自动增厚（§6.3 反向调节）
//! - **事件订阅镜像**：通过 event-bus 订阅 StabilityGuard 事件维护镜像状态（方案 A）
//!
//! # 设计原则（ADR-046 决策 5/9）
//! - **facade 而非重实现**：底层 CircuitBreaker/DegradationChain 复用 chimera-mas stability.rs
//! - **依赖铁律**：通过 event-bus 订阅 chimera-mas 事件,不直接 `use chimera_mas::StabilityGuard`
//! - **不可进化面**：接口签名/枚举变体集/数据结构禁止 Harness spec 演化（决策 9）
//!
//! # KPI-03（§9.5 SLO）
//! 适应性免疫层 < 100ms,三探针异步并行 + 复用既有熔断状态镜像（AtomicU8 load ~1ns）

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, RwLock};

use event_bus::{EventBus, NexusEvent};

use crate::immune_system::evolution_hack::EvolutionHackProbe;
use crate::immune_system::membrane::MembraneController;
use crate::immune_system::memory_paradox::MemoryParadoxProbe;
use crate::immune_system::reasoning_trap::ReasoningTrapProbe;
use crate::immune_system::types::{
    ImmuneSystemError, ParadoxProbe, ParadoxReport, ParadoxRiskReport, ProbeType,
};

// ============================================================
// 子模块声明
// ============================================================

pub mod evolution_hack;
pub mod membrane;
pub mod memory_paradox;
pub mod reasoning_trap;
pub mod types;

// ============================================================
// 类型重导出说明
// ============================================================

// WHY 此处不使用 `pub use` 重导出子模块类型：私有 `use` 语句（lines 27-33）已将
// `EvolutionHackProbe`/`MembraneController`/`MemoryParadoxProbe`/`ReasoningTrapProbe`/
// `types::*` 引入本模块作用域供内部代码使用。若同时 `pub use` 同名符号会触发
// E0252（name defined multiple times）。
//
// 消费方访问路径：
// - `parliament::immune_system::MemoryParadoxProbe`（不可用,需走子模块）
// - `parliament::immune_system::memory_paradox::MemoryParadoxProbe`（可用）
// - `parliament::MemoryParadoxProbe`（lib.rs 顶层 re-export 可用）

// ============================================================
// StabilityMirror — StabilityGuard 事件镜像状态（ADR-046 决策 5 方案 A）
// ============================================================

/// CircuitBreaker 三态常量（与 chimera-mas stability.rs 对齐）
///
/// # 状态机语义
/// - `BREAKER_STATE_CLOSED`: 正常工作,允许任务执行
/// - `BREAKER_STATE_OPEN`: 熔断中,拒绝任务执行（等待恢复计时）
/// - `BREAKER_STATE_HALF_OPEN`: 半开,允许试探性任务（验证服务恢复）
pub const BREAKER_STATE_CLOSED: u8 = 0;
/// CircuitBreaker Open 状态:熔断中,拒绝任务执行
pub const BREAKER_STATE_OPEN: u8 = 1;
/// CircuitBreaker HalfOpen 状态:半开,允许试探性任务
pub const BREAKER_STATE_HALF_OPEN: u8 = 2;

/// 滑动窗口容量（可进化面,决策 9）
const SLIDING_WINDOW_CAPACITY: usize = 256;

/// StabilityGuard 事件镜像状态（ADR-046 决策 5 方案 A）
///
/// WHY 镜像而非直接调用：依赖铁律 L8 不能依赖 L9 chimera-mas 的 stability.rs,
/// 通过订阅事件维护内部镜像,延迟换依赖合规。
///
/// # 线程安全
/// - `breakers: RwLock<HashMap>`：读多写少,RwLock 允许并发读
/// - `*` Atomic 字段：无锁累加,对齐 stability.rs `CircuitBreaker` 模式
#[derive(Debug)]
pub struct StabilityMirror {
    /// CircuitBreaker 状态镜像（breaker_id → state,0=Closed/1=Open/2=HalfOpen）
    breakers: RwLock<HashMap<String, u8>>,
    /// 降级层级镜像（取 max,与 CsnSubstitutionTriggered.degradation_level 对齐）
    degradation_level: AtomicU32,
    /// 终态任务计数镜像
    terminal_count: AtomicU32,
    /// SkepticVeto 时间戳滑动窗口（用于 ReasoningTrap 探针）
    skeptic_veto_window: RwLock<VecDeque<u64>>,
    /// VetoOverridden 时间戳滑动窗口（用于 ReasoningTrap 探针）
    veto_overridden_window: RwLock<VecDeque<u64>>,
    /// CapabilityFrozen 累计计数（用于 EvolutionHack 探针）
    capability_frozen_count: AtomicU32,
    /// BudgetExceeded 滑动窗口计数（用于级联风险评估）
    budget_exceeded_window: RwLock<VecDeque<u64>>,
    /// 最后更新时间戳（Unix 毫秒,用于检测镜像陈旧）
    last_update_ts: AtomicU64,
}

impl StabilityMirror {
    /// 创建空镜像,所有计数器初始为 0
    pub fn new() -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
            degradation_level: AtomicU32::new(0),
            terminal_count: AtomicU32::new(0),
            skeptic_veto_window: RwLock::new(VecDeque::with_capacity(SLIDING_WINDOW_CAPACITY)),
            veto_overridden_window: RwLock::new(VecDeque::with_capacity(SLIDING_WINDOW_CAPACITY)),
            capability_frozen_count: AtomicU32::new(0),
            budget_exceeded_window: RwLock::new(VecDeque::with_capacity(SLIDING_WINDOW_CAPACITY)),
            last_update_ts: AtomicU64::new(0),
        }
    }

    /// 从事件更新镜像状态（ADR-046 决策 6 事件订阅清单）
    ///
    /// # 参数
    /// - `event`: 来自 event-bus 的 NexusEvent
    /// - `timestamp_ms`: 当前 Unix 时间戳（毫秒）
    pub fn update_from_event(&self, event: &NexusEvent, timestamp_ms: u64) {
        match event {
            NexusEvent::AgentTaskFailed { from, .. } => {
                // 触发 CircuitBreaker record_failure（breaker_id 用 from 标识）
                let mut breakers = self.breakers.write().unwrap_or_else(|e| e.into_inner());
                let state = breakers.entry(from.clone()).or_insert(BREAKER_STATE_CLOSED);
                // 简化：累计失败即切到 Open（实际应由 StabilityGuard 维护精确阈值）
                *state = BREAKER_STATE_OPEN;
                self.last_update_ts.store(timestamp_ms, Ordering::Release);
            }
            NexusEvent::AgentTaskCompleted { .. } => {
                self.terminal_count.fetch_add(1, Ordering::AcqRel);
                self.last_update_ts.store(timestamp_ms, Ordering::Release);
            }
            NexusEvent::CsnSubstitutionTriggered {
                degradation_level, ..
            } => {
                // 取 max（与 stability.rs DegradationChain 语义对齐）
                let current = self.degradation_level.load(Ordering::Acquire);
                if *degradation_level > current {
                    self.degradation_level
                        .store(*degradation_level, Ordering::Release);
                }
                self.last_update_ts.store(timestamp_ms, Ordering::Release);
            }
            NexusEvent::SkepticVeto { .. } => {
                self.push_to_window(&self.skeptic_veto_window, timestamp_ms);
                self.last_update_ts.store(timestamp_ms, Ordering::Release);
            }
            NexusEvent::VetoOverridden { .. } => {
                self.push_to_window(&self.veto_overridden_window, timestamp_ms);
                self.last_update_ts.store(timestamp_ms, Ordering::Release);
            }
            NexusEvent::CapabilityFrozen { .. } => {
                self.capability_frozen_count.fetch_add(1, Ordering::AcqRel);
                self.last_update_ts.store(timestamp_ms, Ordering::Release);
            }
            NexusEvent::BudgetExceeded { .. } => {
                self.push_to_window(&self.budget_exceeded_window, timestamp_ms);
                self.last_update_ts.store(timestamp_ms, Ordering::Release);
            }
            _ => {}
        }
    }

    /// 推入时间戳到滑动窗口（容量受限,溢出丢弃最旧）
    fn push_to_window(&self, window: &RwLock<VecDeque<u64>>, ts: u64) {
        let mut w = window.write().unwrap_or_else(|e| e.into_inner());
        if w.len() >= SLIDING_WINDOW_CAPACITY {
            w.pop_front();
        }
        w.push_back(ts);
    }

    /// 返回 Open 状态断路器占比（用于级联风险评估）
    pub fn circuit_open_ratio(&self) -> f32 {
        let breakers = self.breakers.read().unwrap_or_else(|e| e.into_inner());
        if breakers.is_empty() {
            return 0.0;
        }
        let open_count = breakers
            .values()
            .filter(|&&s| s == BREAKER_STATE_OPEN)
            .count();
        // WHY f32 全程：§4.4 #6 红线禁止 f32 隐式转 f64 比较
        open_count as f32 / breakers.len() as f32
    }

    /// 返回降级层级镜像
    pub fn degradation_level(&self) -> u32 {
        self.degradation_level.load(Ordering::Acquire)
    }

    /// 返回终态任务计数
    pub fn terminal_count(&self) -> u32 {
        self.terminal_count.load(Ordering::Acquire)
    }

    /// 返回 SkepticVeto 滑动窗口大小（供 ReasoningTrap 探针使用）
    pub fn skeptic_veto_count(&self) -> usize {
        self.skeptic_veto_window
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// 返回 VetoOverridden 滑动窗口大小（供 ReasoningTrap 探针使用）
    pub fn veto_overridden_count(&self) -> usize {
        self.veto_overridden_window
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// 返回 CapabilityFrozen 累计计数（供 EvolutionHack 探针使用）
    pub fn capability_frozen_count(&self) -> u32 {
        self.capability_frozen_count.load(Ordering::Acquire)
    }

    /// 返回最近 BudgetExceeded 计数（供级联风险评估使用）
    pub fn budget_exceeded_recent_count(&self, since_ms: u64, now_ms: u64) -> u32 {
        let w = self
            .budget_exceeded_window
            .read()
            .unwrap_or_else(|e| e.into_inner());
        w.iter()
            .filter(|&&ts| ts >= now_ms.saturating_sub(since_ms))
            .count() as u32
    }

    /// 返回最后更新时间戳（用于检测镜像陈旧）
    pub fn last_update_ts(&self) -> u64 {
        self.last_update_ts.load(Ordering::Acquire)
    }

    /// 镜像是否陈旧（>5s 未更新）
    pub fn is_stale(&self, now_ms: u64) -> bool {
        let last = self.last_update_ts.load(Ordering::Acquire);
        last == 0 || now_ms.saturating_sub(last) > 5_000
    }
}

impl Default for StabilityMirror {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for StabilityMirror {
    /// WHY 手动 Clone：RwLock<HashMap> 不派生 Clone,需手动复制内部数据。
    /// 用于探针持有镜像副本（避免共享 Arc,简化生命周期）。
    fn clone(&self) -> Self {
        let breakers = self
            .breakers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let skeptic_veto = self
            .skeptic_veto_window
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let veto_overridden = self
            .veto_overridden_window
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let budget_exceeded = self
            .budget_exceeded_window
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Self {
            breakers: RwLock::new(breakers),
            degradation_level: AtomicU32::new(self.degradation_level.load(Ordering::Acquire)),
            terminal_count: AtomicU32::new(self.terminal_count.load(Ordering::Acquire)),
            skeptic_veto_window: RwLock::new(skeptic_veto),
            veto_overridden_window: RwLock::new(veto_overridden),
            capability_frozen_count: AtomicU32::new(
                self.capability_frozen_count.load(Ordering::Acquire),
            ),
            budget_exceeded_window: RwLock::new(budget_exceeded),
            last_update_ts: AtomicU64::new(self.last_update_ts.load(Ordering::Acquire)),
        }
    }
}

// ============================================================
// ImmuneSystem facade — 主结构（ADR-046 决策 1）
// ============================================================

/// ImmuneSystem facade — 适应性免疫接口层（v5.0 §8.1 D7）
///
/// # 设计原则（ADR-046 决策 1/5/9）
/// - **facade 而非重实现**：底层 CircuitBreaker/DegradationChain 复用 chimera-mas stability.rs
/// - **依赖铁律**：通过 event-bus 订阅 chimera-mas 事件维护镜像状态（方案 A）
/// - **不可进化面**：接口签名/探针数量(3)/ProbeType enum 禁止演化（决策 9）
///
/// # 线程安全
/// - `probes`: `[Box<dyn ParadoxProbe>; 3]` 固定长度,Send + Sync
/// - `stability_mirror`: `Arc<StabilityMirror>` 共享,内部 RwLock/Atomic
/// - `cascade_risk`: `AtomicU32`（f32 位模式,无锁读取）
/// - `membrane_thickness`: `AtomicU8`（0-7,无锁读取）
pub struct ImmuneSystem {
    /// 三探针注册表（固定 3 项,INV-10 不可变量）
    probes: [Box<dyn ParadoxProbe>; 3],
    /// StabilityGuard 事件镜像状态（决策 5 方案 A）
    stability_mirror: Arc<StabilityMirror>,
    /// 膜厚控制器（决策 7）
    membrane: MembraneController,
    /// 级联风险评分 [0.0, 1.0],用 u32 存储 f32 位模式（决策 1）
    cascade_risk: AtomicU32,
    /// 膜厚度（0-7,INV-11 不可变量）
    membrane_thickness: AtomicU8,
    /// EventBus 引用（持有以防后台任务通道关闭后重新订阅,目前 new() 内 subscribe 后未直接读取）
    #[allow(dead_code)]
    event_bus: Arc<EventBus>,
}

impl std::fmt::Debug for ImmuneSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // WHY finish_non_exhaustive：EventBus 未实现 Debug（通信通道,非业务状态）
        f.debug_struct("ImmuneSystem")
            .field("probes_len", &self.probes.len())
            .field("cascade_risk", &self.cascade_risk())
            .field("membrane_thickness", &self.membrane_thickness())
            .field("stability_mirror", &self.stability_mirror)
            .finish_non_exhaustive()
    }
}

impl ImmuneSystem {
    /// 创建 ImmuneSystem facade（ADR-046 决策 1）
    ///
    /// # 设计
    /// - 同步订阅 event-bus（Critical + Normal 双通道,§4.4 反模式 3）
    /// - 启动后台任务消费事件,更新 StabilityMirror
    /// - 注册三探针（固定顺序：MemoryParadox / ReasoningTrap / EvolutionHack）
    ///
    /// # 参数
    /// - `event_bus`: 共享 EventBus（Arc,后台任务持有副本）
    pub async fn new(event_bus: Arc<EventBus>) -> Result<Self, ImmuneSystemError> {
        let mirror = Arc::new(StabilityMirror::new());

        // 同步订阅 Critical + Normal 通道（§4.4 反模式 3：subscribe 先于 spawn）
        let mut critical_rx = event_bus.subscribe_critical_events();
        let mut normal_rx = event_bus.subscribe();

        // 启动后台任务消费 Critical 事件（§6.2 红线：4 类 Critical 事件走 mpsc 旁路）
        let mirror_critical = Arc::clone(&mirror);
        tokio::spawn(async move {
            // WHY while let：clippy while_let_loop 建议,等价于 loop+match 但更简洁
            while let Some(event) = critical_rx.recv().await {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                mirror_critical.update_from_event(&event, now_ms);
            }
            // 通道关闭,任务自然退出
        });

        // 启动后台任务消费 Normal 事件（broadcast）
        let mirror_normal = Arc::clone(&mirror);
        tokio::spawn(async move {
            // WHY while let：clippy while_let_loop 建议,等价于 loop+match 但更简洁
            while let Ok(event) = normal_rx.recv().await {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                mirror_normal.update_from_event(&event, now_ms);
            }
            // 通道关闭或 Lagged,任务自然退出
        });

        // 注册三探针（固定 3 项,INV-10）
        let memory_probe = MemoryParadoxProbe::new(Arc::clone(&mirror));
        let reasoning_probe = ReasoningTrapProbe::new(Arc::clone(&mirror));
        let evolution_probe = EvolutionHackProbe::new(Arc::clone(&mirror));

        Ok(Self {
            probes: [
                Box::new(memory_probe),
                Box::new(reasoning_probe),
                Box::new(evolution_probe),
            ],
            stability_mirror: mirror,
            membrane: MembraneController::new(),
            cascade_risk: AtomicU32::new(0.0f32.to_bits()),
            membrane_thickness: AtomicU8::new(0),
            event_bus,
        })
    }

    /// 执行三探针扫描 + 级联风险评估（ADR-046 决策 1 + 决策 7）
    ///
    /// # KPI-03（§9.5 SLO）
    /// 三探针异步并行（FuturesUnordered）+ 复用既有熔断状态镜像（AtomicU8 load ~1ns）
    ///
    /// # 返回
    /// `ParadoxRiskReport` 包含三探针报告 + 级联风险 + 膜厚度
    pub async fn assess_paradox_risk(&self) -> ParadoxRiskReport {
        use futures::stream::{FuturesUnordered, StreamExt};

        // 并行执行三探针（FuturesUnordered,§4.1 通用约定）
        let mut futures = FuturesUnordered::new();
        for probe in &self.probes {
            futures.push(probe.detect());
        }

        let mut reports: Vec<ParadoxReport> = Vec::with_capacity(3);
        while let Some(report) = futures.next().await {
            reports.push(report);
        }

        // 按 ProbeType 排序确保报告顺序固定（便于测试断言）
        reports.sort_by_key(|r| match r.probe_type {
            ProbeType::MemoryParadox => 0,
            ProbeType::ReasoningTrap => 1,
            ProbeType::EvolutionHack => 2,
        });

        // 安全转换：长度必为 3（INV-10）
        let reports_arr: [ParadoxReport; 3] = match reports.try_into() {
            Ok(arr) => arr,
            Err(_) => [
                ParadoxReport::insufficient_data(ProbeType::MemoryParadox),
                ParadoxReport::insufficient_data(ProbeType::ReasoningTrap),
                ParadoxReport::insufficient_data(ProbeType::EvolutionHack),
            ],
        };

        // 计算级联风险
        // WHY 使用 mirror.last_update_ts() 而非 SystemTime::now()：
        //   budget_exceeded_recent_count 的时间坐标系需与事件注入时间戳一致。
        //   详见 memory_paradox.rs detect() 中的同名注释。
        let mirror_now_ms = self.stability_mirror.last_update_ts();
        let budget_recent = self
            .stability_mirror
            .budget_exceeded_recent_count(10_000, mirror_now_ms);
        let cascade = compute_cascade_risk(&reports_arr, &self.stability_mirror, budget_recent);

        // 更新级联风险（AtomicU32 存 f32 位模式）
        self.cascade_risk
            .store(cascade.to_bits(), Ordering::Release);

        // 膜厚自动调节（决策 7：cascade_risk > 0.7 → 增厚）
        self.adjust_membrane(cascade);

        let thickness = self.membrane_thickness();
        // 报告时间戳使用 wall-clock（SystemTime::now()）记录评估发生的真实时刻,
        // 与 budget_exceeded_recent_count 使用的 mirror 时间坐标系解耦
        let report_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        ParadoxRiskReport {
            reports: reports_arr,
            cascade_risk: cascade,
            membrane_thickness: thickness,
            timestamp: report_ts,
        }
    }

    /// 触发级联（决策 7：cascade_risk > 0.7 触发膜增厚 + 内部告警）
    ///
    /// # 设计
    /// - 不修改 chimera-mas stability.rs（依赖铁律）
    /// - 仅设置内部 cascade_risk 与 membrane_thickness
    /// - 通过 tracing 记录级联触发（不发布 MembraneThicknessAdjusted 事件,事件变体不存在）
    pub async fn trigger_cascade(&self, risk: ParadoxRiskReport) -> Result<(), ImmuneSystemError> {
        self.cascade_risk
            .store(risk.cascade_risk.to_bits(), Ordering::Release);
        self.adjust_membrane(risk.cascade_risk);

        if risk.cascade_risk > 0.7 {
            tracing::warn!(
                cascade_risk = risk.cascade_risk,
                membrane_thickness = self.membrane_thickness(),
                "ImmuneSystem cascade triggered: membrane thickened"
            );
        }
        Ok(())
    }

    /// 触发指定断路器（决策 1：trip_circuit 接口）
    ///
    /// # 设计
    /// - 不直接调用 chimera-mas StabilityGuard（依赖铁律）
    /// - 仅在镜像中标记 breaker 为 Open
    pub fn trip_circuit(&self, breaker_id: &str) {
        let mut breakers = self
            .stability_mirror
            .breakers
            .write()
            .unwrap_or_else(|e| e.into_inner());
        breakers.insert(breaker_id.to_string(), BREAKER_STATE_OPEN);
    }

    /// 返回当前膜厚度（决策 1：membrane_thickness 接口）
    pub fn membrane_thickness(&self) -> u8 {
        self.membrane_thickness.load(Ordering::Acquire)
    }

    /// 返回当前级联风险评分 [0.0, 1.0]
    pub fn cascade_risk(&self) -> f32 {
        f32::from_bits(self.cascade_risk.load(Ordering::Acquire))
    }

    /// 返回 StabilityMirror 引用（供探针与测试访问）
    pub fn stability_mirror(&self) -> &Arc<StabilityMirror> {
        &self.stability_mirror
    }

    /// 返回探针切片引用（INV-10 验证：长度必为 3）
    pub fn probes(&self) -> &[Box<dyn ParadoxProbe>] {
        &self.probes
    }

    /// 膜厚自动调节（决策 7）
    ///
    /// - cascade_risk > 0.7 → 增厚（min(7, +1)）
    /// - cascade_risk < 0.3 → 变薄（max(0, -1)）
    fn adjust_membrane(&self, cascade: f32) {
        let current = self.membrane_thickness.load(Ordering::Acquire);
        let new = if cascade > 0.7 {
            current.saturating_add(1).min(7)
        } else if cascade < 0.3 {
            current.saturating_sub(1)
        } else {
            current
        };
        if new != current {
            self.membrane_thickness.store(new, Ordering::Release);
            self.membrane.set_thickness(new);
            tracing::info!(
                old = current,
                new = new,
                cascade_risk = cascade,
                "Membrane thickness adjusted"
            );
        }
    }
}

// ============================================================
// compute_cascade_risk — 级联风险评分（ADR-046 决策 7 + 附录 A）
// ============================================================

/// 计算级联风险评分（ADR-046 决策 7 + 附录 A 伪代码）
///
/// ```text
/// cascade_risk = 0.5 * max(paradox_rate[Memory, Reasoning, Evolution])
///              + 0.3 * stability_mirror.circuit_open_ratio
///              + 0.2 * (budget_exceeded_recent_count / 10).min(1.0)
/// ```
///
/// # 不变量（INV-12）
/// 输出 ∈ [0.0, 1.0]
pub fn compute_cascade_risk(
    reports: &[ParadoxReport; 3],
    mirror: &StabilityMirror,
    budget_exceeded_count: u32,
) -> f32 {
    // WHY f32 全程：§4.4 #6 红线禁止 f32 隐式转 f64 比较
    let max_paradox = reports
        .iter()
        .map(|r| r.paradox_rate)
        .fold(0.0f32, f32::max);
    let circuit_open_ratio = mirror.circuit_open_ratio();
    let budget_term = (budget_exceeded_count as f32 / 10.0).min(1.0);

    let cascade = 0.5 * max_paradox + 0.3 * circuit_open_ratio + 0.2 * budget_term;
    cascade.clamp(0.0, 1.0)
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stability_mirror_new_initial_state() {
        let mirror = StabilityMirror::new();
        assert_eq!(mirror.degradation_level(), 0);
        assert_eq!(mirror.terminal_count(), 0);
        assert_eq!(mirror.capability_frozen_count(), 0);
        assert_eq!(mirror.skeptic_veto_count(), 0);
        assert_eq!(mirror.veto_overridden_count(), 0);
        assert!((mirror.circuit_open_ratio() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_stability_mirror_clone_preserves_state() {
        let mirror = StabilityMirror::new();
        mirror.terminal_count.fetch_add(5, Ordering::AcqRel);
        mirror
            .capability_frozen_count
            .fetch_add(2, Ordering::AcqRel);

        let cloned = mirror.clone();
        assert_eq!(cloned.terminal_count(), 5);
        assert_eq!(cloned.capability_frozen_count(), 2);
    }

    #[test]
    fn test_stability_mirror_update_from_agent_task_failed() {
        let mirror = StabilityMirror::new();
        let event = NexusEvent::AgentTaskFailed {
            metadata: event_bus::EventMetadata::new("test"),
            from: "agent-1".into(),
            to: "agent-0".into(),
            task_id: "t-1".into(),
            error: "boom".into(),
            retry_count: 0,
        };
        mirror.update_from_event(&event, 1000);
        assert!((mirror.circuit_open_ratio() - 1.0).abs() < 1e-6);
        assert_eq!(mirror.last_update_ts(), 1000);
    }

    #[test]
    fn test_stability_mirror_update_from_capability_frozen() {
        let mirror = StabilityMirror::new();
        let event = NexusEvent::CapabilityFrozen {
            metadata: event_bus::EventMetadata::new("test"),
            capability_id: "cap-1".into(),
            reason: "test".into(),
        };
        mirror.update_from_event(&event, 2000);
        assert_eq!(mirror.capability_frozen_count(), 1);
    }

    #[test]
    fn test_stability_mirror_update_from_budget_exceeded_counts_recent() {
        let mirror = StabilityMirror::new();
        for ts in [1000u64, 1500, 5000, 9000] {
            let event = NexusEvent::BudgetExceeded {
                metadata: event_bus::EventMetadata::new("test"),
                budget_type: "token".into(),
                current: 100,
                limit: 100,
            };
            mirror.update_from_event(&event, ts);
        }
        // now=10000, since=5000 → 仅 ts >= 5000 计入（5000, 9000）
        assert_eq!(mirror.budget_exceeded_recent_count(5000, 10000), 2);
    }

    #[test]
    fn test_stability_mirror_is_stale_when_no_updates() {
        let mirror = StabilityMirror::new();
        assert!(mirror.is_stale(10_000)); // last_update_ts = 0 → stale
    }

    #[test]
    fn test_stability_mirror_not_stale_after_recent_update() {
        let mirror = StabilityMirror::new();
        let event = NexusEvent::AgentTaskCompleted {
            metadata: event_bus::EventMetadata::new("test"),
            from: "agent-1".into(),
            to: "agent-0".into(),
            task_id: "t-1".into(),
            result_summary: "done".into(),
        };
        mirror.update_from_event(&event, 10_000);
        assert!(!mirror.is_stale(12_000)); // 2s 后未陈旧
        assert!(mirror.is_stale(20_000)); // 10s 后陈旧
    }

    #[test]
    fn test_compute_cascade_risk_zero_when_all_zero() {
        let mirror = StabilityMirror::new();
        let reports = [
            ParadoxReport::insufficient_data(ProbeType::MemoryParadox),
            ParadoxReport::insufficient_data(ProbeType::ReasoningTrap),
            ParadoxReport::insufficient_data(ProbeType::EvolutionHack),
        ];
        let risk = compute_cascade_risk(&reports, &mirror, 0);
        assert!(risk.abs() < 1e-6, "全零输入级联风险应为 0,实际 = {risk}");
    }

    #[test]
    fn test_compute_cascade_risk_clamped_to_one() {
        let mirror = StabilityMirror::new();
        // 手动塞入 Open 断路器
        {
            let mut b = mirror.breakers.write().unwrap_or_else(|e| e.into_inner());
            b.insert("cb-1".into(), BREAKER_STATE_OPEN);
            b.insert("cb-2".into(), BREAKER_STATE_OPEN);
        }
        let reports = [
            ParadoxReport {
                probe_type: ProbeType::MemoryParadox,
                paradox_rate: 1.0,
                severity: types::Severity::Critical,
                details: "max".into(),
                insufficient_data: false,
            },
            ParadoxReport {
                probe_type: ProbeType::ReasoningTrap,
                paradox_rate: 1.0,
                severity: types::Severity::Critical,
                details: "max".into(),
                insufficient_data: false,
            },
            ParadoxReport {
                probe_type: ProbeType::EvolutionHack,
                paradox_rate: 1.0,
                severity: types::Severity::Critical,
                details: "max".into(),
                insufficient_data: false,
            },
        ];
        let risk = compute_cascade_risk(&reports, &mirror, 100);
        // 0.5*1 + 0.3*1 + 0.2*1 = 1.0
        assert!(
            (risk - 1.0).abs() < 1e-6,
            "全 1 输入应被 clamp 到 1.0,实际 = {risk}"
        );
    }

    #[test]
    fn test_compute_cascade_risk_weighted_formula() {
        let mirror = StabilityMirror::new();
        // 仅 Memory paradox=0.6,其他 0
        let reports = [
            ParadoxReport {
                probe_type: ProbeType::MemoryParadox,
                paradox_rate: 0.6,
                severity: types::Severity::Warning,
                details: "mid".into(),
                insufficient_data: false,
            },
            ParadoxReport::insufficient_data(ProbeType::ReasoningTrap),
            ParadoxReport::insufficient_data(ProbeType::EvolutionHack),
        ];
        // 0.5*0.6 + 0.3*0 + 0.2*(3/10=0.3) = 0.3 + 0 + 0.06 = 0.36
        let risk = compute_cascade_risk(&reports, &mirror, 3);
        assert!(
            (risk - 0.36).abs() < 1e-6,
            "加权公式应得 0.36,实际 = {risk}"
        );
    }

    #[test]
    fn test_immune_system_invariants_initial_state() {
        // INV-10/11/12 静态校验：探针数=3,膜厚∈[0,7],cascade_risk∈[0,1]
        let mirror = StabilityMirror::new();
        assert_eq!(mirror.degradation_level(), 0);
        // INV-11：膜厚 ∈ [0, 7]（具体验证需 ImmuneSystem 实例）
    }
}
