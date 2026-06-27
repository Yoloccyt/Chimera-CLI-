# Week 5 P0 修复实施计划

> **计划类型**:Week 5 深度复审 P0 修复实施计划
> **目标**:修复 4 项阻塞 Week 6 启动的 P0 问题(C1/C2/M4/M6),恢复 Ω-Event 定律完整性
> **执行模式**:5 个并行 Sub-Agent,按依赖顺序推进
> **预估工时**:6 小时

***

## 1. 背景与目标 (Context)

### 1.1 触发原因

Week 5 深度复审(7 Task × 45 SubTask)完成后,识别出 13 项问题(2 Critical + 8 Major + 3 Low)。其中 P0 项直接阻塞 Week 6 启动:

| 编号     | 问题                                        | 严重级别     | 影响                                  |
| ------ | ----------------------------------------- | -------- | ----------------------------------- |
| **C1** | 8 个新 Week 5 事件未集成 EventBus                | Critical | Ω-Event 定律断裂,对抗治理闭环未真正形成            |
| **C2** | BudgetExceeded 未标记 Critical + 无 mpsc 投递保证 | Critical | 预算超限告警可能被背压丢弃                       |
| **M4** | ttg.rs 7 处 `expect()` 违反锁中毒规范             | Major    | 锁中毒时进程崩溃,违反"graceful degradation"原则 |
| **M6** | event-bus/src/types.rs 注释错误标注"L3 Storage" | Major    | 文档误导,实际发布者 DECB 在 L8 Parliament     |

### 1.2 修复目标

1. **C1**:8 个新事件(DebateStarted/SkepticVeto/RedTeamAudit/AhirtProbeCompleted/BudgetAdjusted/BudgetStatsReported/AsaIntervention/CapabilityFrozen)全部接入 EventBus
2. **C2 第一阶段**:BudgetExceeded 加入 `severity()` Critical 分支
3. **M4**:7 处 `expect()` 替换为 `unwrap_or_else(|p| p.into_inner())`
4. **M6**:修正 2 处注释 "L3 Storage" → "L8 Parliament"

### 1.3 不在本次范围

* C2 第二阶段(mpsc+broadcast 双通道架构)→ P1,Week 6 第一周内

* M3(TTG 订阅 BudgetAdjusted)→ P1

* M5(AhirtConfig 配置化)→ P2

* M7(String 弱类型 newtype)→ P2

* M8(同步 5 份文档)→ P1

* L1/L2/L3 → P2

***

## 2. 当前状态分析 (Current State Analysis)

### 2.1 C2 — severity() 与 backpressure.rs 现状

**event-bus/src/types.rs:958-968**:

```rust
pub fn severity(&self) -> EventSeverity {
    match self {
        Self::CheckpointSaved { .. }
        | Self::ConsensusReached { .. }
        | Self::SlowConsumerDropped { .. }
        | Self::OrphanCallDetected { .. }
        | Self::SkepticVeto { .. }
        | Self::RedTeamAudit { .. } => EventSeverity::Critical,
        _ => EventSeverity::Normal,  // BudgetExceeded 落入此分支
    }
}
```

**问题**:BudgetExceeded 被通配符分支误判为 Normal,backpressure 可能在通道满时丢弃。

**event-bus/src/backpressure.rs:11-13** 已明确"当前简化为 broadcast + Critical 标注",C2 第二阶段才需要实现 mpsc 双通道。本次仅完成第一阶段(标记 Critical)。

### 2.2 M6 — 注释错误位置

**event-bus/src/types.rs:784**:

```
/// DECB 预算档位调整 — L3 Storage → L8 Parliament/L9 Quest
```

应改为:`L8 Parliament → L9 Quest`(DECB 在 L8,见项目规则 §2.1)

**event-bus/src/types.rs:864**:

```
/// 预算消耗统计上报 — L3 Storage → L8 Parliament
```

应改为:`L8 Parliament(内部统计,无跨层消费)`

### 2.3 M4 — ttg.rs 7 处 expect() 位置

| 行号  | 表达式                                                                          | 上下文                        |
| --- | ---------------------------------------------------------------------------- | -------------------------- |
| 345 | `self.modes.lock().expect("modes mutex poisoned")`                           | `record_mode()`            |
| 364 | `self.modes.lock().expect("modes mutex poisoned")`                           | `current_mode()`           |
| 415 | `self.last_budget_switch.lock().expect("last_budget_switch mutex poisoned")` | `on_budget_adjusted()`     |
| 436 | `self.last_budget_switch.lock().expect("last_budget_switch mutex poisoned")` | `is_within_lag_interval()` |
| 469 | `self.modes.lock().expect("modes mutex poisoned")`                           | `override_mode()`          |
| 498 | `self.modes.lock().expect("modes mutex poisoned")`                           | `reset_override()`         |
| 510 | `self.modes.lock().expect("modes mutex poisoned")`                           | `is_overridden()`          |

**参考模式**(seccore/src/asa.rs:178-181):

```rust
let history = self
    .history
    .read()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
```

### 2.4 C1 — 8 个新事件集成现状

**当前状态**:8 个事件已在 types.rs 中定义,但生产代码中仅用 `tracing::error!`/`info!` 记录,未通过 EventBus 发布。具体 TODO 位置:

| 事件                  | TODO 位置                    | 发布者          |
| ------------------- | -------------------------- | ------------ |
| DebateStarted       | debate.rs:263              | Parliament   |
| SkepticVeto         | debate.rs:227              | Parliament   |
| CapabilityFrozen    | debate.rs:238              | Parliament   |
| RedTeamAudit        | ahirt.rs:356, ahirt.rs:385 | AhirtRedTeam |
| AhirtProbeCompleted | ahirt.rs:413               | AhirtRedTeam |
| BudgetAdjusted      | governor.rs(待查找)           | DecbGovernor |
| BudgetStatsReported | governor.rs(待查找)           | DecbGovernor |
| AsaIntervention     | asa.rs(待查找)                | AsaAuditor   |

**Parliament 已有参考模式**(debate.rs:301, 374):

* 持有 EventBus by value(line 166)

* 构造函数 `new(config, event_bus)`(line 181)

* 委托 free function 发布:`publish_consensus_event(&self.event_bus, ...)` / `publish_vote_event(&self.event_bus, ...)`

* 访问器:`pub fn event_bus(&self) -> &EventBus`

**DecbGovernor 现状**(governor.rs:61-74):

* 结构体未持有 EventBus

* `new(config: DecbConfig) -> Result<Self, DecbError>`(line 81)

* 27 处调用点全为测试代码(无生产代码持有)

**AsaAuditor 现状**(asa.rs:146-151):

* 结构体未持有 EventBus

* `new(config: AsaConfig) -> Self`(line 155)

* `with_default_config() -> Self`(line 163)

* 63 处调用点全为测试代码

**AhirtRedTeam 现状**(ahirt.rs:261-275):

* 结构体未持有 EventBus,仅 `library` + `policy` 字段

* `new(library: ProbePayloadLibrary) -> Self`(line 270)

* `spawn_periodic_probe()` 用 `self.clone()` 移入 spawn(AhirtRedTeam 需 Clone)

***

## 3. 修复方案 (Proposed Changes)

### 3.1 Sub-Agent A:C2 + M6 修复(event-bus)

**文件**:`crates/event-bus/src/types.rs`

**修改 1 (C2)** — 在 `severity()` Critical 分支添加 BudgetExceeded:

```rust
pub fn severity(&self) -> EventSeverity {
    match self {
        Self::CheckpointSaved { .. }
        | Self::ConsensusReached { .. }
        | Self::SlowConsumerDropped { .. }
        | Self::OrphanCallDetected { .. }
        | Self::SkepticVeto { .. }
        | Self::RedTeamAudit { .. }
        | Self::BudgetExceeded { .. } => EventSeverity::Critical,  // 新增
        _ => EventSeverity::Normal,
    }
}
```

WHY:BudgetExceeded 丢失会导致 Parliament 无法触发降级/终止,系统持续消耗预算至崩溃。本次仅标记 Critical,C2 第二阶段(P1)实现 mpsc 双通道确保投递。

**修改 2 (M6)** — line 784 注释:

```rust
// 修改前:
/// DECB 预算档位调整 — L3 Storage → L8 Parliament/L9 Quest
// 修改后:
/// DECB 预算档位调整 — L8 Parliament → L9 Quest
```

**修改 3 (M6)** — line 864 注释:

```rust
// 修改前:
/// 预算消耗统计上报 — L3 Storage → L8 Parliament
// 修改后:
/// 预算消耗统计上报 — L8 Parliament(同层内部统计,无跨层消费)
```

**修改 4 (C2 文档同步)** — 更新 `severity()` 上方文档注释(line 950-957),在 Week 5 新增 Critical 事件列表中追加 BudgetExceeded。

**验证**:

```powershell
cargo check -p event-bus
cargo test -p event-bus --lib
cargo clippy -p event-bus -- -D warnings
```

***

### 3.2 Sub-Agent B:M4 修复(quest-engine)

**文件**:`crates/quest-engine/src/ttg.rs`

**修改**:将 7 处 `Mutex::lock().expect("... mutex poisoned")` 替换为 `Mutex::lock().unwrap_or_else(|poisoned| poisoned.into_inner())`。

具体行号:345, 364, 415, 436, 469, 498, 510。

**统一模式**(参考 seccore/src/asa.rs:178-181):

```rust
// 修改前:
let modes = self.modes.lock().expect("modes mutex poisoned");
// 修改后:
let modes = self.modes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
```

WHY:锁中毒表示另一个线程在持锁时 panic,数据可能处于不一致状态。`expect()` 会让当前线程也 panic,放大故障;`unwrap_or_else(|p| p.into_inner())` 仍可访问内部数据,允许当前线程 graceful 退出或记录诊断信息。这是项目锁中毒规范的统一模式(见 project\_memory.md)。

**验证**:

```powershell
cargo check -p quest-engine
cargo test -p quest-engine --lib
cargo clippy -p quest-engine -- -D warnings
```

***

### 3.3 Sub-Agent C:C1-Parliament 修复

**文件**:

* `crates/parliament/src/debate.rs`

* `crates/parliament/src/ahirt.rs`

* `crates/parliament/src/voting.rs`(新增 publish free functions)

* `crates/parliament/Cargo.toml`(可能需要确认 event-bus 依赖)

**修改 1**:在 `voting.rs` 新增 3 个 free function(参考现有 `publish_vote_event`/`publish_consensus_event` 模式):

```rust
/// 发布 DebateStarted 事件
pub async fn publish_debate_started_event(
    bus: &EventBus,
    quest_id: &str,
    proposal_id: &str,
) {
    let event = NexusEvent::DebateStarted {
        metadata: EventMetadata::new("parliament"),
        quest_id: quest_id.to_string(),
        proposal_id: proposal_id.to_string(),
    };
    if let Err(e) = bus.publish(event).await {
        warn!(error = %e, "发布 DebateStarted 事件失败");
    }
}

/// 发布 SkepticVeto 事件 [Critical]
pub async fn publish_skeptic_veto_event(
    bus: &EventBus,
    quest_id: &str,
    proposal_id: &str,
    intent_type: &str,
    matched_pattern: &str,
    severity: &str,
    detail: &str,
) {
    let event = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: quest_id.to_string(),
        proposal_id: proposal_id.to_string(),
        intent_type: intent_type.to_string(),
        matched_pattern: matched_pattern.to_string(),
        severity: severity.to_string(),
        detail: detail.to_string(),
    };
    if let Err(e) = bus.publish(event).await {
        warn!(error = %e, "发布 SkepticVeto 事件失败");
    }
}

/// 发布 CapabilityFrozen 事件
pub async fn publish_capability_frozen_event(
    bus: &EventBus,
    capability_id: &str,
    reason: &str,
) {
    let event = NexusEvent::CapabilityFrozen {
        metadata: EventMetadata::new("parliament"),
        capability_id: capability_id.to_string(),
        reason: reason.to_string(),
    };
    if let Err(e) = bus.publish(event).await {
        warn!(error = %e, "发布 CapabilityFrozen 事件失败");
    }
}
```

**修改 2**:debate.rs:227 — SkepticVeto TODO 替换为实际 publish 调用:

```rust
publish_skeptic_veto_event(
    &self.event_bus,
    &quest.quest_id,
    &proposal.proposal_id,
    &format!("{:?}", veto_reason.intent_type),
    &veto_reason.matched_pattern,
    &format!("{:?}", veto_reason.severity),
    &veto_reason.detail,
).await;
```

**修改 3**:debate.rs:238-246 — CapabilityFrozen TODO 替换为循环 publish:

```rust
for cap in &frozen_capabilities {
    publish_capability_frozen_event(
        &self.event_bus,
        cap,
        &veto_reason.detail,
    ).await;
}
```

**修改 4**:debate.rs:263 — DebateStarted TODO 替换为 publish 调用:

```rust
publish_debate_started_event(
    &self.event_bus,
    &quest.quest_id,
    &proposal.proposal_id,
).await;
```

**修改 5**:AhirtRedTeam 添加 EventBus 字段 + 新构造函数(ahirt.rs:261-275):

```rust
#[derive(Clone)]
pub struct AhirtRedTeam {
    library: ProbePayloadLibrary,
    policy: CommandPolicy,
    event_bus: EventBus,  // 新增
}

impl AhirtRedTeam {
    pub fn new(library: ProbePayloadLibrary) -> Self {
        Self::with_event_bus(library, EventBus::new())
    }

    /// 创建带 EventBus 的 AHIRT 红队(生产代码推荐)
    pub fn with_event_bus(library: ProbePayloadLibrary, bus: EventBus) -> Self {
        Self {
            library,
            policy: CommandPolicy::default_secure(),
            event_bus: bus,
        }
    }
}
```

WHY `new()` 内部创建私有总线:AhirtRedTeam 有 29 处测试调用点,保留 `new()` 零测试修改。生产代码通过 `with_event_bus()` 注入共享总线。EventBus 基于 `Arc<tokio::broadcast::Sender>`,Clone 廉价,持字段无性能损耗。

**修改 6**:ahirt.rs:356 — RedTeamAudit \[Critical] TODO 替换:

```rust
// 在 verify_security() 内 detection_rate < 0.95 分支
let event = NexusEvent::RedTeamAudit {
    metadata: EventMetadata::new("parliament"),
    vulnerability_type: format!("{:?}", vulnerable_types),
    failed_probes: stats.total - stats.passed,
    total_probes: stats.total,
    detection_rate: stats.detection_rate,
    remediation_suggestion: remediation_suggestions.join("; "),
};
if let Err(e) = self.event_bus.publish(event).await {
    warn!(error = %e, "发布 RedTeamAudit 事件失败");
}
```

**修改 7**:ahirt.rs:385 — report\_vulnerability 内的 RedTeamAudit(可选,因为 verify\_security 已发布关键事件,report\_vulnerability 是细粒度补充)。**决策**:保持 tracing 日志,不再重复发布 RedTeamAudit(避免事件风暴)。仅修改 TODO 注释为"已在 verify\_security 中统一发布"。

**修改 8**:ahirt.rs:413 — AhirtProbeCompleted TODO 替换:

```rust
// spawn_periodic_probe() 内每次 tick 后
let event = NexusEvent::AhirtProbeCompleted {
    metadata: EventMetadata::new("parliament"),
    detection_rate: report.stats.detection_rate,
    total_probes: report.stats.total,
    passed_probes: report.stats.passed,
    vulnerable_types: report.vulnerable_types.iter().map(|t| format!("{:?}", t)).collect(),
};
if let Err(e) = red_team.event_bus.publish(event).await {
    warn!(error = %e, "发布 AhirtProbeCompleted 事件失败");
}
```

**修改 9**:Parliament 顶层 lib.rs 文档注释更新(line 11) — 添加 SkepticVeto/CapabilityFrozen/DebateStarted/RedTeamAudit/AhirtProbeCompleted 到事件列表。

**验证**:

```powershell
cargo check -p parliament
cargo test -p parliament
cargo clippy -p parliament -- -D warnings
```

***

### 3.4 Sub-Agent D:C1-DecbGovernor 修复

**文件**:

* `crates/decb-governor/Cargo.toml`(添加 event-bus 依赖)

* `crates/decb-governor/src/governor.rs`

* `crates/decb-governor/src/lib.rs`(文档注释更新)

**修改 1**:Cargo.toml 添加 event-bus workspace 依赖:

```toml
[dependencies]
event-bus = { workspace = true }
# ... 现有依赖
```

**修改 2**:DecbGovernor 结构体添加 event\_bus 字段(governor.rs:61-74):

```rust
pub struct DecbGovernor {
    config: DecbConfig,
    tier_state: Arc<Mutex<TierState>>,
    total_consumption: Arc<Mutex<f64>>,
    consumption_count: AtomicU64,
    current_coefficient: Mutex<f32>,
    overflow_detector: OverflowDetector,
    event_bus: EventBus,  // 新增
}
```

**修改 3**:保留 `new(config)` 兼容现有测试,新增 `with_event_bus(config, bus)`:

```rust
impl DecbGovernor {
    /// 创建新的 DECB 治理器(内部创建私有 EventBus,仅用于测试)
    pub fn new(config: DecbConfig) -> Result<Self, DecbError> {
        Self::with_event_bus(config, EventBus::new())
    }

    /// 创建带共享 EventBus 的 DECB 治理器(生产代码推荐)
    pub fn with_event_bus(config: DecbConfig, bus: EventBus) -> Result<Self, DecbError> {
        config.validate()?;
        let overflow_detector = OverflowDetector::new(&config);
        Ok(Self {
            config,
            tier_state: Arc::new(Mutex::new(TierState {
                current_tier: BudgetTier::HighTier,
                last_switch_time: None,
            })),
            total_consumption: Arc::new(Mutex::new(0.0)),
            consumption_count: AtomicU64::new(0),
            current_coefficient: Mutex::new(1.0),
            overflow_detector,
            event_bus: bus,
        })
    }

    /// EventBus 访问器(供测试与上层访问)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }
}
```

WHY 保留 `new()`:DecbGovernor 有 27 处测试调用点,保留 `new()` 零测试修改。生产代码(Week 6 集成时)改用 `with_event_bus()`。

**修改 4**:在档位切换点(需 grep `tier_state.lock` 定位)publish BudgetAdjusted 事件:

```rust
// 档位切换成功后
let event = NexusEvent::BudgetAdjusted {
    metadata: EventMetadata::new("decb-governor"),
    quest_id: quest_id.to_string(),
    old_tier: format!("{:?}", old_tier),
    new_tier: format!("{:?}", new_tier),
    coefficient: new_coefficient,
};
if let Err(e) = self.event_bus.publish(event).await {
    warn!(error = %e, "发布 BudgetAdjusted 事件失败");
}
```

**修改 5**:在溢出检测点 publish BudgetExceeded \[Critical] 事件:

```rust
// 检测到预算溢出时
let event = NexusEvent::BudgetExceeded {
    metadata: EventMetadata::new("decb-governor"),
    budget_type: "cognitive".to_string(),
    current: current_consumption as u64,
    limit: limit as u64,
};
if let Err(e) = self.event_bus.publish(event).await {
    warn!(error = %e, "发布 BudgetExceeded 事件失败");
}
```

**修改 6**:在每 100 次消耗统计点 publish BudgetStatsReported 事件:

```rust
// consumption_count % 100 == 0 时
let event = NexusEvent::BudgetStatsReported {
    metadata: EventMetadata::new("decb-governor"),
    total_consumption: *total,
    remaining_budget: remaining,
    utilization_rate: utilization,
};
if let Err(e) = self.event_bus.publish(event).await {
    warn!(error = %e, "发布 BudgetStatsReported 事件失败");
}
```

**修改 7**:lib.rs 文档注释更新(line 17-20)— 将"当前用 tracing::info! 记录事件,Task 37 统一集成"改为"已集成 event-bus,发布 BudgetAdjusted/BudgetExceeded/BudgetStatsReported 事件"。

**验证**:

```powershell
cargo check -p decb-governor
cargo test -p decb-governor
cargo clippy -p decb-governor -- -D warnings
```

***

### 3.5 Sub-Agent E:C1-AsaAuditor 修复

**文件**:

* `crates/seccore/Cargo.toml`(确认 event-bus 依赖,可能已存在)

* `crates/seccore/src/asa.rs`

**修改 1**:确认/添加 Cargo.toml event-bus 依赖:

```toml
[dependencies]
event-bus = { workspace = true }
```

**修改 2**:AsaAuditor 结构体添加 event\_bus 字段(asa.rs:146-151):

```rust
pub struct AsaAuditor {
    config: AsaConfig,
    history: RwLock<OperationHistory>,
    event_bus: EventBus,  // 新增
}
```

**修改 3**:保留 `new(config)` 和 `with_default_config()`,新增 `with_event_bus(config, bus)`:

```rust
impl AsaAuditor {
    pub fn new(config: AsaConfig) -> Self {
        Self::with_event_bus(config, EventBus::new())
    }

    pub fn with_default_config() -> Self {
        Self::new(AsaConfig::default())
    }

    /// 创建带共享 EventBus 的 ASA 审计器(生产代码推荐)
    pub fn with_event_bus(config: AsaConfig, bus: EventBus) -> Self {
        Self {
            config,
            history: RwLock::new(OperationHistory::new()),
            event_bus: bus,
        }
    }

    /// EventBus 访问器
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }
}
```

WHY 保留 `new()` 和 `with_default_config()`:AsaAuditor 有 63 处测试调用点,保留兼容零测试修改。

**修改 4**:在 `audit()` 方法的干预分级点(asa.rs:206 `classify_intervention` 后)publish AsaIntervention 事件:

```rust
// 仅在 intervention != NoIntervention 时发布(避免事件风暴)
if intervention != InterventionAction::NoIntervention {
    let event = NexusEvent::AsaIntervention {
        metadata: EventMetadata::new("seccore"),
        intervention_type: format!("{:?}", intervention),
        audit_reason: audit_reason.clone(),
        safety_score,
    };
    if let Err(e) = self.event_bus.publish(event).await {
        warn!(error = %e, "发布 AsaIntervention 事件失败");
    }
}
```

**注意**:`audit()` 当前是同步方法(`pub fn audit`)。如果 publish 需要 await,需要将 `audit` 改为 `async fn`。**决策**:为避免破坏 63 处测试调用,改为在 `audit()` 内部使用 `tokio::spawn` fire-and-forget 发布(事件丢失可接受,因为 tracing 日志仍记录)。或者更好:让 `audit()` 仅返回需发布的事件,由调用者负责 await publish。

**最终决策**(待 Sub-Agent E 实施时根据调用链决定):

* 选项 A:audit() 保持同步,使用 `tokio::spawn` fire-and-forget(简单,但事件可能丢失)

* 选项 B:audit() 改为 async,调用者 await(正确,但需修改 63 处测试)

* **推荐选项 A**(P0 阶段优先保证不破坏测试,P1 阶段重构成 async)

**修改 5**:seccore/src/lib.rs 顶部文档注释更新 — 在"四层防御"描述后添加"ASA 审计器发布 AsaIntervention 事件"说明。

**验证**:

```powershell
cargo check -p seccore
cargo test -p seccore
cargo clippy -p seccore -- -D warnings
```

***

## 4. 执行顺序与并行策略

### 4.1 依赖关系图

```
Sub-Agent A (C2+M6) ──────┐
                          │
Sub-Agent B (M4) ─────────┼──→ 无依赖,完全并行
                          │
Sub-Agent C (C1-Parliament)┤
                          │
Sub-Agent D (C1-DecbGovernor)┤
                          │
Sub-Agent E (C1-AsaAuditor)┘
```

### 4.2 5 个 Sub-Agent 并行执行

所有 5 个 Sub-Agent **完全并行**,无文件冲突:

* A:event-bus/src/types.rs

* B:quest-engine/src/ttg.rs

* C:parliament/src/{debate,ahirt,voting}.rs

* D:decb-governor/src/governor.rs + Cargo.toml

* E:seccore/src/asa.rs + Cargo.toml

### 4.3 收尾阶段(并行完成后)

由主代理统一执行:

```powershell
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

***

## 5. 假设与决策 (Assumptions & Decisions)

### 5.1 关键决策

| ID | 决策                                                                      | 理由                                                       |
| -- | ----------------------------------------------------------------------- | -------------------------------------------------------- |
| D1 | DecbGovernor/AsaAuditor/AhirtRedTeam 保留 `new()` + 新增 `with_event_bus()` | 兼容 27+63+29 处测试调用点,零测试修改                                 |
| D2 | EventBus 持有方式:by value(`event_bus: EventBus`)                           | 参考 Parliament 模式;EventBus 基于 Arc,Clone 廉价                |
| D3 | C2 仅完成第一阶段(标记 Critical),不实现 mpsc 双通道                                    | backpressure.rs 已预留扩展点;P1 阶段实现双通道                        |
| D4 | AsaAuditor `audit()` 保持同步,使用 `tokio::spawn` fire-and-forget 发布          | 避免 63 处测试改签名;P1 重构为 async                                |
| D5 | report\_vulnerability() 不再发布 RedTeamAudit 事件                            | verify\_security() 已统一发布,避免事件风暴                          |
| D6 | M6 修改 types.rs 注释,不修改 CODE\_WIKI.md / lib.rs 注释                         | 错误源头在 types.rs,其他文件已正确标注 L8                              |
| D7 | 发布失败仅 `warn!` 日志,不阻塞主流程                                                 | 参考现有 publish\_vote\_event 模式;EventBus publish 是"假 async" |

### 5.2 假设

* AhirtRedTeam 已实现 `Clone`(spawn\_periodic\_probe 使用 `self.clone()`)— 需 Sub-Agent C 验证

* DecbGovernor 档位切换点可在 governor.rs 内 grep 定位 — Sub-Agent D 探索

* AsaIntervention 事件已在 types.rs 定义(包含 intervention\_type/audit\_reason/safety\_score 字段)— Sub-Agent E 验证

* BudgetAdjusted/BudgetStatsReported 事件字段定义与 governor.rs 数据匹配 — Sub-Agent D 验证

### 5.3 风险与缓解

| 风险                                           | 概率 | 缓解                                            |
| -------------------------------------------- | -- | --------------------------------------------- |
| 添加 event\_bus 字段后,Clone 派生失败                 | 中  | EventBus 已实现 Clone(Arc 内部)                    |
| 异步 publish 调用破坏同步方法签名                        | 高  | 使用 `tokio::spawn` fire-and-forget 或返回事件由调用者发布 |
| 测试中 EventBus::new() 创建的私有总线无订阅者,publish 静默丢弃 | 低  | 这是预期行为,测试不验证事件发布                              |
| Cargo.toml event-bus 依赖缺失导致编译失败              | 低  | Sub-Agent D/E 第一步检查并添加                        |

***

## 6. 验证步骤 (Verification)

### 6.1 单 crate 验证(各 Sub-Agent 自行执行)

每个 Sub-Agent 完成修改后,立即执行:

```powershell
cargo check -p <crate-name>
cargo test -p <crate-name>
cargo clippy -p <crate-name> -- -D warnings
```

### 6.2 全 workspace 验证(主代理执行)

5 个 Sub-Agent 全部完成后:

```powershell
# 1. 类型检查
cargo check --workspace

# 2. Lint(零警告)
cargo clippy --workspace -- -D warnings

# 3. 全量测试(预期 2023 个测试全部通过)
cargo test --workspace

# 4. Release 构建
cargo build --workspace --release
```

### 6.3 验收检查清单

* [ ] `severity()` Critical 分支包含 BudgetExceeded

* [ ] types.rs:784 注释为 "L8 Parliament → L9 Quest"

* [ ] types.rs:864 注释为 "L8 Parliament"

* [ ] ttg.rs 7 处 expect() 全部替换为 unwrap\_or\_else

* [ ] debate.rs:227 SkepticVeto 实际 publish

* [ ] debate.rs:238 CapabilityFrozen 实际 publish

* [ ] debate.rs:263 DebateStarted 实际 publish

* [ ] ahirt.rs:356 RedTeamAudit \[Critical] 实际 publish

* [ ] ahirt.rs:413 AhirtProbeCompleted 实际 publish

* [ ] governor.rs 档位切换 publish BudgetAdjusted

* [ ] governor.rs 溢出检测 publish BudgetExceeded

* [ ] governor.rs 统计周期 publish BudgetStatsReported

* [ ] asa.rs 干预分级 publish AsaIntervention

* [ ] DecbGovernor/AsaAuditor/AhirtRedTeam 新增 with\_event\_bus() 构造函数

* [ ] cargo test --workspace 全部通过

* [ ] cargo clippy --workspace -- -D warnings 零警告

***

## 7. 文件清单

### 7.1 需修改文件(11 个)

| 文件                                   | Sub-Agent | 修改内容                     |
| ------------------------------------ | --------- | ------------------------ |
| crates/event-bus/src/types.rs        | A         | severity() + 2 处注释       |
| crates/quest-engine/src/ttg.rs       | B         | 7 处 expect 替换            |
| crates/parliament/src/debate.rs      | C         | 3 处 TODO 替换              |
| crates/parliament/src/ahirt.rs       | C         | 结构体 + 3 处 TODO 替换        |
| crates/parliament/src/voting.rs      | C         | 新增 3 个 publish 函数        |
| crates/parliament/src/lib.rs         | C         | 文档注释更新                   |
| crates/decb-governor/Cargo.toml      | D         | 添加 event-bus 依赖          |
| crates/decb-governor/src/governor.rs | D         | 结构体 + 构造函数 + 3 处 publish |
| crates/decb-governor/src/lib.rs      | D         | 文档注释更新                   |
| crates/seccore/Cargo.toml            | E         | 确认/添加 event-bus 依赖       |
| crates/seccore/src/asa.rs            | E         | 结构体 + 构造函数 + 1 处 publish |
| crates/seccore/src/lib.rs            | E         | 文档注释更新                   |

### 7.2 不修改文件

* CODE\_WIKI.md → P1 阶段同步(M8)

* AETHER\_NEXUS\_OMEGA\_ULTIMATE.md → P1 阶段同步(M8)

* CHANGELOG.md → P1 阶段同步(M8)

***

## 8. 后续工作(P1/P2,本次不实施)

* **P1(Week 6 第一周)**:

  * C2 第二阶段:实现 mpsc+broadcast 双通道架构(backpressure.rs)

  * M3:TTG 改为订阅 BudgetAdjusted 事件,移除直接依赖 decb\_governor

  * M8:同步 5 份文档(CODE\_WIKI/CHANGELOG/lib.rs/spec/project\_memory)

* **P2(Week 6 第二周)**:

  * M5:引入 AhirtConfig 配置化(5 分钟周期、0.95 检测率阈值)

  * M7:为 9 个事件引入 newtype(避免 String 弱类型)

  * L1:TTG 跨锁原子性审查

  * L2:qeep-protocol 添加 proptest

  * L3:AHIRT WHY 注释补充

