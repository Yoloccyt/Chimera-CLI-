# ADR-046: ImmuneSystem facade 三探针设计

## 状态

已批准 (Accepted) (2026-07-26)

> **状态说明**: 本 ADR 于 2026-07-26 由 E02 安全架构师(12+ 年经验) + E03 记忆系统专家(12+ 年经验)组成的 ADR 撰写小组创建并批准。本 ADR 是 v5.0 设计文档 §8 "稳定性与免疫系统" 的工程实施层落地,定义 ImmuneSystem facade 的接口边界、三悖论探针算法、依赖铁律裁决与级联联动机制。属 append-only 扩展(对齐 ADR-028 决策 1 哲学),不修改既有 ADR-030(unsafe 红线)、ADR-033(L0 nexus-contracts)、ADR-034(灰度=能力场)、ADR-026(chimera-mas)的裁决结论。P5.3 ImmuneSystem facade 尚未启动,本 ADR 为前置设计。

> **ADR 编号确认**: `docs/architecture/` 现有最大编号为 ADR-044(2026-07-26 核实)。本 ADR 编号确认为 **ADR-046**(ADR-045 已预留给另一独立决策),与既有规划不冲突。

## 背景

### 上游设计要求

- **v5.0 设计文档 §8.1** 明确指出:"ImmuneSystem(facade,新接口层;底层复用既有实现,零新建检测器)"。其结构为四层:先天免疫(<1ms 反射,屏障+吞噬,既有 seccore+QEEP)、适应性免疫(<100ms,**复用** stability.rs + **新增** 三探针 + 级联预测)、免疫记忆(威胁-应答对 → gsoe 负样本 + auto-dpo 偏好对)、免疫耐受(正常模式白名单,PdcaLoop 4 告警规则并轨)。
- **v5.0 设计文档 §8.2** 明确三悖论免疫映射表(本 ADR 必须落地):

| 悖论 | 免疫机制 | 锚点 |
|---|---|---|
| 记忆悖论 | TemporalFilter(§4.3)+ S2 Bandit + INV-8 单调归档 | mlc-engine、chimera-mas archive/ |
| 推理悖论 | Fast Path 80% 跳过 + 自白通道 + 复杂度预算 | parliament、ahirt.rs |
| 进化悖论 | RHI-CG 双通道 + 不可进化面 + R2 冻结线 | gsoe-evolution、auto-dpo |

- **v5.0 设计文档 §3.4 复杂度预算**(净增长 ≤ 0):新增 ImmuneSystem facade,抵消项为"v3.0-stable 文档侧 StabilityGuard 设计(不新建,合并 chimera-mas stability.rs)"。即 ImmuneSystem facade 不重建 stability.rs,只新增三探针。
- **v5.0 设计文档 §6.3 级联联动**:ImmuneSystem 级联风险 >0.7 → 膜自动增厚(替代 v3.0-extreme 独立的 AdaptiveMembrane 设计,统一在此)。
- **v5.0 设计文档 §9.5 SLO**:适应性免疫层 <100ms(KPI-03)。

### 上游 ADR 约束

- **ADR-030(unsafe 红线)**:ImmuneSystem facade 必须 `#![forbid(unsafe_code)]`,探针算法禁止使用裸指针/UnsafeCell。无锁并发只能用 `AtomicU8/U32`(对齐 stability.rs `CircuitBreaker` 已有模式)。
- **ADR-033(L0 nexus-contracts)**:跨层共享类型必须上提到 L0 `nexus-contracts`。本 ADR 决策的探针输出类型 `ParadoxReport` 需评估是否上提。
- **ADR-034(灰度=能力场)**:探针的启用/禁用禁止运行时 Feature Flag,必须用 CapabilityToken 能力场或 Cargo 编译期 feature。

### P5.3 待启动现状

当前代码基线(v2.3.1-omega → v3.2.0-omega 目标,2026-07-26 核实):

- [crates/chimera-mas/src/stability.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/stability.rs) 已实现 `StabilityGuard`(零孤儿终态保证 + 故障隔离)、`CircuitBreaker`(三态机 Closed/Open/HalfOpen,基于 AtomicU8 CAS)、`DegradationChain`(三压力源 MemoryNearBudget/ExpertOverload/ArchiveIoContention)— 100% GREEN,SubTask 19.7-19.9 验收通过。
- [crates/parliament/src/ahirt.rs](file:///D:/Chimera CLI/crates/parliament/src/ahirt.rs) 已实现 `AhirtRedTeam`(4 类攻击载荷 × 25 = 100 探测,探测率 >95% 阈值,周期 5 分钟)— 复用既有,作为 ReasoningTrap 探针的"自白通道"载体。
- [crates/event-bus/src/types.rs](file:///D:/Chimera CLI/crates/event-bus/src/types.rs) 已定义 `SkepticVeto` / `VetoOverridden` / `RedTeamAudit` / `AgentTaskDelegated` / `AgentTaskCompleted` / `AgentTaskFailed`(Critical)/ `CsnSubstitutionTriggered`(含 `degradation_level`)/ `BudgetExceeded`(Critical)— 三探针的数据源已就绪。
- **membrane.rs 不存在**:经 `Get-ChildItem crates/chimera-mas/src/` 核实,当前只有 stability.rs / orchestrator.rs / pdca.rs 等 14 个文件,无 membrane.rs。本 ADR 必须裁决膜增厚接口的承载位置。
- **ImmuneSystem facade 未启动**:35 crate × 624 .rs 文件零命中 `ImmuneSystem` / `MemoryParadox` / `ReasoningTrap` / `EvolutionHack`,P5.3 待实现。

### 依赖铁律关键约束(本 ADR 核心裁决点)

**核心问题**:ImmuneSystem facade 在 L8 parliament,而 `stability.rs` 位于 L9 chimera-mas 内部子模块。根据 §2.2 依赖铁律,L(N) → L(N+1) **向上依赖禁止**,parliament 不能直接 `use chimera_mas::stability::StabilityGuard`。

**stability.rs 内部强依赖核实**(2026-07-26 grep):

| 引用位置 | 引用形式 | 强度 |
|---|---|---|
| `chimera-mas/src/orchestrator.rs:36` | `use crate::stability::{StabilityGuard, TerminalState};` | 强(字段 + 构造 + 访问器) |
| `chimera-mas/src/orchestrator.rs:173` | `stability_guard: Arc<StabilityGuard>` 字段 | 强(类型依赖) |
| `chimera-mas/src/orchestrator.rs:201` | `Arc::new(StabilityGuard::new())` 构造 | 强(实例化) |
| `chimera-mas/src/orchestrator.rs:227` | `pub fn stability_guard(&self) -> &StabilityGuard` 访问器 | 强(公开 API) |
| `chimera-mas/src/lib.rs` | 重导出 `pub use stability::{...}` | 中(re-export) |
| `chimera-mas/src/error.rs` | `MasError` 引用 | 中(类型引用) |
| `chimera-mas/tests/stability_test.rs` | 测试 | 测试 |
| `csn-substitutor/src/degradation_chain.rs` | **独立实现** `pub struct DegradationChain` | 无(同名不同体) |
| `nexus-contracts/src/capability_token.rs:355` | 仅注释引用 | 无(注释) |

**结论**:stability.rs 被 chimera-mas/orchestrator.rs 强依赖,上提至 L8 会破坏 chimera-mas 既有 4 处内部引用(orchestrator 字段+构造+访问器、lib.rs re-export、error.rs),且与 csn-substitutor 的同名 `DegradationChain` 无关(独立实现),不构成上提收益。本 ADR 决策 5 裁决方案 A(事件订阅镜像)。

## 决策

经 E02 安全架构师 + E03 记忆系统专家多轮结构化思考与多路径交叉验证,对 ImmuneSystem facade 三探针设计作出以下 9 项工程实施决策。

### 决策 1: ImmuneSystem facade 接口设计 — 探针注册 + 级联风险评估 + 熔断触发

ImmuneSystem 作为 facade 接口层,落地于 `crates/parliament/src/immune_system.rs`(L8,与 ahirt.rs 同 crate 同层),不新建 crate(满足 §3.4 复杂度预算)。

**接口定义**(拟落地于 `crates/parliament/src/immune_system.rs`):

```rust
/// ImmuneSystem facade — 适应性免疫接口层(v5.0 §8.1 D7)
///
/// 架构层归属: L8 Parliament(parliament crate 内部子模块)
/// 核心职责: 三探针注册 + 级联风险评估 + 熔断触发接口
///
/// # 设计原则
/// - **facade 而非重实现**: 底层 CircuitBreaker/DegradationChain 复用 chimera-mas stability.rs
/// - **依赖铁律**: 通过 event-bus 订阅 chimera-mas 事件,不直接 `use chimera_mas::StabilityGuard`
/// - **不可进化面**: 本接口为不可进化面(决策 9),禁止 Harness spec 演化
pub struct ImmuneSystem {
    /// 三探针注册表(固定 3 项,无动态扩展)
    probes: [Box<dyn ParadoxProbe>; 3],
    /// StabilityGuard 事件镜像状态(由 event-bus 订阅维护)
    stability_mirror: StabilityMirror,
    /// 级联风险评分 [0.0, 1.0],>0.7 触发膜增厚(§6.3)
    cascade_risk: AtomicU32, // 用 u32 存储 f32 的位模式,无锁读取
    /// 膜厚度(0-7,由 cascade_risk 反向调节)
    membrane_thickness: AtomicU8,
}

impl ImmuneSystem {
    /// 执行三探针扫描,返回级联风险评分
    /// WHY <100ms KPI-03:三探针异步并行 + 复用既有熔断状态镜像
    pub async fn scan(&self) -> ParadoxReport;
    /// 触发熔断(委托给 stability_mirror 的镜像 CircuitBreaker)
    pub fn trip_circuit(&self, breaker_id: &str);
    /// 获取当前膜厚度(供 membrane.rs 调用)
    pub fn membrane_thickness(&self) -> u8;
}
```

**新增 trait `ParadoxProbe`**(探针统一接口):

```rust
pub trait ParadoxProbe: Send + Sync {
    fn probe_type(&self) -> ProbeType;
    async fn detect(&self) -> ProbeResult;
}
```

### 决策 2: MemoryParadox 探针算法 — 过时事实与当前事实共召回检测

**免疫机制锚点**(§8.2):TemporalFilter(§4.3)+ S2 Bandit + INV-8 单调归档。

**算法**(落地于 `crates/parliament/src/immune_system/memory_paradox.rs`):

```text
输入: mlc-engine 上下文召回流(订阅 ContextRetrieved 事件,含 temporal_meta)
     chimera-mas archive/ 的归档时间戳流(订阅 AgentArchived 事件)

算法:
  1. 滑动窗口(最近 N=100 次召回)维护 temporal_meta 直方图
  2. 对每次召回,计算"时间一致性分数":
     score = (current_facts.timestamp - recalled_facts.timestamp).abs()
            / temporal_window_size
  3. 若 score > 0.5 且召回中存在两个时间戳差异 > 7 天的事实 → 标记"幽灵矛盾"
  4. 悖论率 = ghost_contradictions / total_recalls
  5. INV-8 单调归档校验:若 archive/ 中存在被覆写的归档(归档时间戳非单调)
     → 悖论率 += 0.2(归档违反单调性是强信号)

输出: ParadoxReport {
    probe_type: MemoryParadox,
    paradox_rate: f32,        // 悖论率 [0.0, 1.0]
    ghost_facts: Vec<String>, // 幽灵事实 ID 清单
    severity: Severity,       // >0.3 = Warning, >0.7 = Critical
}
```

**与 mlc-engine 的协作**:不直接调用 mlc-engine API(向上依赖禁止),而是订阅 `ContextRetrieved` 事件(L2 → L8 跨层事件允许,§2.2)。`temporal_meta` 字段需在 mlc-engine 的 `ContextRetrieved` 事件中扩展(若未扩展则探针返回 `ParadoxReport::insufficient_data()`)。

**与 chimera-mas archive/ 的协作**:订阅 `AgentArchived` 事件(L9 → L8 跨层事件允许),内部维护归档时间戳单调性镜像。

### 决策 3: ReasoningTrap 探针算法 — SkepticVeto 模式化绕过识别

**免疫机制锚点**(§8.2):Fast Path 80% 跳过 + 自白通道 + 复杂度预算。

**算法**(落地于 `crates/parliament/src/immune_system/reasoning_trap.rs`):

```text
输入: event-bus 的 SkepticVeto 事件流(含 veto_reason + frozen_capabilities)
     event-bus 的 VetoOverridden 事件流(含 override_reason + override_by)

算法:
  1. 滑动窗口(最近 N=50 次 SkepticVeto 事件)
  2. 提取 veto_reason 关键词,聚类为模式:
     - "unsafe shell injection detected" → 模式 P1
     - "prompt injection detected" → 模式 P2
     - "privilege escalation" → 模式 P3
     - 其他 → 模式 P_other
  3. 计算 SkepticVeto 模式化指标:
     - 若同一模式 P_i 在窗口内出现 > 5 次 → 模式化告警
     - 若 VetoOverridden 同期出现 > 2 次 → 高风险(Skeptic 否决被反复覆盖)
  4. 自白通道检测(对齐 ahirt.rs):
     - 调用 ahirt.rs 的 probe_single() 对近期 veto_reason 做主动重测
     - 若 ahirt 探测率 < 95% → 自白通道暴露(reasoning 强到能绕过红队)
  5. 复杂度预算校验:
     - 若 parliament 的辩论平均耗时 > 1000ms(超过 Fast Path 80% 跳过阈值)
       → 推理悖论阈值触发(协调成本 > 推理增益)

输出: ParadoxReport {
    probe_type: ReasoningTrap,
    paradox_rate: f32,                  // SkepticVeto 模式化率
    veto_override_rate: f32,            // 否决被覆盖率
    fast_path_skip_ratio: f32,          // Fast Path 跳过比
    ahirt_detection_rate: f32,          // 自白通道探测率
    severity: Severity,
}
```

**与 ahirt.rs 的协作**:本探针直接调用 `AhirtRedTeam::probe_single()`(同 crate 同层,L8 内调用),不通过 event-bus。这是 §2.2 允许的同层互引。

### 决策 4: EvolutionHack 探针算法 — 通道 B 否决率异常检测

**免疫机制锚点**(§8.2):RHI-CG 双通道 + 不可进化面 + R2 冻结线。

**算法**(落地于 `crates/parliament/src/immune_system/evolution_hack.rs`):

```text
输入: event-bus 的 CsnSubstitutionTriggered 事件流(含 degradation_level)
     event-bus 的 CapabilityFrozen 事件流
     event-bus 的 AgentTaskCompleted/Failed 事件流
     ADR-044 RHI-CG 通道 B(Critical Gate)否决率统计

算法:
  1. 滑动窗口(最近 N=200 次 GSOE/AutoDPO 进化事件)
  2. 计算通道 B 否决率:
     channel_b_veto_rate = channel_b_vetoed / channel_b_total
  3. 异常检测:
     - 若 channel_b_veto_rate > 0.3(30% 进化被通道 B 否决)
       → 进化方向疑似被游戏化(进化悖论红线 3)
     - 若 CapabilityFrozen 同期触发 > 3 次
       → 进化系统反复冻结,不可进化面被试探
  4. R2 冻结线校验(ADR-042):
     - 若 R2 路径有任何触发事件 → 严重告警(R2 必须无条件冻结)
  5. 奖励黑客检测:
     - 若 AgentTaskCompleted 但 task_reward 异常高(>3σ)
       → 疑似奖励黑客(执行反馈被游戏化,L3 验证器层级风险)

输出: ParadoxReport {
    probe_type: EvolutionHack,
    paradox_rate: f32,                // 通道 B 否决率
    reward_anomaly_score: f32,        // 奖励异常分数(z-score)
    r2_violation: bool,              // R2 冻结线违反
    frozen_capability_count: u32,
    severity: Severity,
}
```

**与 ADR-044 RHI-CG 的协作**:本探针订阅 ADR-044 定义的通道 B 事件(若 ADR-044 已落地),否则探针返回 `ParadoxReport::insufficient_data()`。本 ADR 不强制 ADR-044 落地顺序,但建议 ADR-044 先行(否则 EvolutionHack 探针无法获取通道 B 数据)。

### 决策 5: 依赖铁律裁决 — 方案 A(事件订阅镜像)

**裁决结论**:采纳**方案 A(事件订阅镜像)**,否决方案 B(stability.rs 上提)。

**方案 A 详细设计**:

```text
                  event-bus
                      │
       ┌──────────────┼──────────────┐
       │              │              │
       ▼              ▼              ▼
   SkepticVeto   AgentTaskFailed  CsnSubstitution
   (L8→L4)       (L9→L4)         (L10→L6)
       │              │              │
       └──────┬───────┴──────┬───────┘
              │              │
              ▼              ▼
    ┌─────────────────────────────────┐
    │  ImmuneSystem (L8 parliament)  │
    │  ┌─────────────────────────┐   │
    │  │  StabilityMirror       │   │  ← 内部维护镜像状态
    │  │  - circuit_breaker_state│   │
    │  │  - degradation_level   │   │
    │  │  - terminal_count      │   │
    │  │  - last_update_ts      │   │
    │  └─────────────────────────┘   │
    └─────────────────────────────────┘
```

**StabilityMirror 类型**(parliament 内部,不上提):

```rust
/// StabilityGuard 事件镜像状态(决策 5 方案 A)
///
/// WHY 镜像而非直接调用:依赖铁律 L8 不能依赖 L9 chimera-mas 的 stability.rs,
/// 通过订阅事件维护内部镜像,延迟换依赖合规。
struct StabilityMirror {
    /// CircuitBreaker 状态镜像(breaker_id → state)
    breakers: DashMap<String, u8>, // 0=Closed, 1=Open, 2=HalfOpen
    /// 降级层级镜像
    degradation_level: AtomicU32,
    /// 终态任务计数镜像
    terminal_count: AtomicU32,
    /// 最后更新时间戳(用于检测镜像陈旧)
    last_update_ts: AtomicU64,
}

impl StabilityMirror {
    /// 订阅 event-bus 的事件(在 ImmuneSystem::new() 中调用)
    fn subscribe(&self, bus: &EventBus) {
        // 订阅 AgentTaskFailed → 触发 CircuitBreaker record_failure
        // 订阅 CsnSubstitutionTriggered → 更新 degradation_level
        // 订阅 AgentTaskCompleted → 更新 terminal_count
    }
}
```

**否决方案 B 的理由**:

1. **stability.rs 被 orchestrator.rs 强依赖**:`use crate::stability::{StabilityGuard, TerminalState}` + 字段 + 构造 + 访问器(4 处),上提需重构 chimera-mas/orchestrator.rs,违反"不破坏既有依赖"原则。
2. **csn-substitutor 已有同名 `DegradationChain`**(独立实现,非引用),stability.rs 上提不会消解同名,反而扩大混淆。
3. **符合"跨层通信只走 event-bus"铁律**(§2.2):ImmuneSystem 作为 facade 本就应通过事件解耦。
4. **复杂度预算**:方案 A 仅新增 `StabilityMirror`(约 100 行),方案 B 需重构 4 处引用 + 移动文件,复杂度更高。

### 决策 6: 复用 stability.rs 事件订阅清单

ImmuneSystem facade 通过 event-bus 订阅以下事件,维护 `StabilityMirror` 内部状态:

| 事件 | 发布层 | ImmuneSystem 用途 | 镜像字段更新 |
|---|---|---|---|
| `AgentTaskFailed` (Critical) | L9 chimera-mas | 触发 CircuitBreaker record_failure | `breakers[task_id_breaker].failure_count += 1` |
| `AgentTaskCompleted` | L9 chimera-mas | 更新终态计数 | `terminal_count += 1` |
| `AgentTaskDelegated` | L9 chimera-mas | 任务派生跟踪 | 维护 `active_tasks` 集合 |
| `CsnSubstitutionTriggered` | L10 csn-substitutor | 降级层级更新 | `degradation_level = max(current, event.degradation_level)` |
| `SkepticVeto` (Critical) | L8 parliament | ReasoningTrap 探针数据源 | 推入 ReasoningTrap 滑动窗口 |
| `VetoOverridden` (Critical) | L8 parliament | ReasoningTrap 探针数据源 | 推入 ReasoningTrap 滑动窗口 |
| `RedTeamAudit` (Critical) | L8 parliament | ReasoningTrap 自白通道 | 更新 `ahirt_detection_rate` |
| `CapabilityFrozen` | L8 parliament | EvolutionHack 探针数据源 | `frozen_capability_count += 1` |
| `BudgetExceeded` (Critical) | L8 acb-governor | 级联风险评估 | `cascade_risk += 0.1` |

**订阅时机**(对齐 §4.4 反模式 3):所有 `bus.subscribe()` 必须在 `tokio::spawn()` 之前同步调用,否则事件静默丢失(Week 6 SSRA 教训)。

### 决策 7: 级联联动机制 — 膜自动增厚接口

**§6.3 要求**:ImmuneSystem 级联风险 >0.7 → 膜自动增厚。

**裁决**:膜增厚接口落地于 `crates/parliament/src/immune_system/membrane.rs`(同 crate 内子模块,不新建 crate),接口 `set_thickness(thickness: u8)`。

**级联风险评分算法**:

```text
cascade_risk = 0.5 * max(paradox_rate[Memory], paradox_rate[Reasoning], paradox_rate[Evolution])
             + 0.3 * stability_mirror.circuit_open_ratio  (Open 断路器占比)
             + 0.2 * budget_exceeded_recent_count / 10

if cascade_risk > 0.7:
    membrane_thickness = min(7, membrane_thickness + 1)  # 增厚
elif cascade_risk < 0.3:
    membrane_thickness = max(0, membrane_thickness - 1)  # 变薄
```

**膜厚度影响**:`membrane_thickness` 越大,事件过滤越严格(对齐 §6.3 四档 Low/Medium/High/Critical 反向调节):

| thickness | 渗透规则 |
|---|---|
| 0-1 (Low) | 全部事件允许穿膜 |
| 2-3 (Medium) | Normal 级事件本地消化 |
| 4-5 (High) | 仅 Critical 事件穿膜 |
| 6-7 (Critical) | 仅 SkepticVeto/RedTeamAudit/BudgetExceeded 三类 Critical 事件穿膜 |

**与其他模块的协作**:membrane.rs 暴露 `pub fn membrane_thickness(&self) -> u8` 给 event-bus 调用,event-bus 在 publish 前查询厚度决定是否本地消化。这是 §6.3 "渗透过滤器(膜新增,替代调用方各自过滤)"的落地。

### 决策 8: KPI-03 探针延迟优化 — <100ms 实现路径

**SLO 目标**(§9.5):适应性免疫层 <100ms。

**实现路径**:

1. **三探针异步并行**:使用 `FuturesUnordered`(§4.1 通用约定)并发执行三探针,总延迟 = max(单探针延迟),而非 sum。
2. **复用既有熔断**:不重新实现 CircuitBreaker,直接读取 `StabilityMirror` 的镜像状态(AtomicU8 load,~1ns)。
3. **滑动窗口无锁**:探针的滑动窗口使用 `crossbeam::queue::SegQueue` 或 `tokio::sync::RwLock<VecDeque>`(读多写少),避免 DashMap 持锁。
4. **事件订阅非阻塞**:`bus.subscribe()` 返回 `Receiver<NexusEvent>`,探针通过 `try_recv()` 非阻塞拉取,不阻塞 reactor。
5. **快速路径优化**:80% 的扫描调用走 Fast Path(只读镜像状态,不触发探针算法),仅当镜像状态变化(如 circuit_open_ratio > 0.3)才触发完整三探针扫描。
6. **基准验收**:新增 criterion bench `immune_system_scan`(目标 p95 < 100ms),落地于 `crates/parliament/benches/immune_system.rs`。

### 决策 9: 不可进化面 — ImmuneSystem facade 接口本身禁止 Harness spec 演化

**ADR-034 红线**:灰度=能力场,禁止运行时 Feature Flag。ImmuneSystem facade 接口进一步升级为**不可进化面**:

1. **接口签名硬编码**:`ImmuneSystem` 的 `pub` 方法签名(`scan` / `trip_circuit` / `membrane_thickness`)**禁止**通过 Harness-as-Spec 演化。任何签名变更需 major 版本 + ADR。
2. **探针数量固定为 3**:`probes: [Box<dyn ParadoxProbe>; 3]` 数组长度固定,**禁止**运行时动态注册新探针。新增探针需 ADR + major 版本。
3. **探针类型枚举固定**:`ProbeType` enum 固定为 `MemoryParadox | ReasoningTrap | EvolutionHack` 三变体,`#[non_exhaustive]` 标注禁止外部 match(强制走 `probe_type()` 访问器)。
4. **不可进化面清单**(对齐 §8.2 进化悖论):
   - ImmuneSystem trait 签名
   - ParadoxProbe trait 签名
   - ProbeType enum 变体集
   - ParadoxReport 数据结构
   - membrane.rs 的 `set_thickness()` 接口
5. **可进化面**(允许 GSOE/AutoDPO 演化):
   - 探针内部算法(如滑动窗口大小 N、模式聚类阈值)
   - 级联风险评分权重(0.5/0.3/0.2)
   - 膜厚度档位映射表

**理由**:ImmuneSystem 是免疫系统,若可被演化则免疫机制本身可被游戏化(进化悖论红线 3)。把"接口"设为不可进化面,"算法参数"设为可进化面,既保证免疫机制稳定性,又允许参数级优化。

## 理由

### 决策 1 理由(facade 接口设计)

- **不新建 crate**:符合 §3.4 复杂度预算(净增长 ≤ 0)。ImmuneSystem 落地于 parliament crate 内部子模块,与 ahirt.rs 同层同 crate,复用既有 parliament 的 EventBus 注入路径。
- **`Box<dyn ParadoxProbe>` 例外**:虽然 §4.1 通用约定建议避免 `Box<dyn Trait>`,但探针数量固定为 3 且需运行时多态(同一接口不同实现),`Box<dyn>` 是最简方案。替代方案是 `enum dispatch`,但 enum dispatch 会让 `ImmuneSystem::scan()` 内部出现 3 分支 match,降低可读性。本 ADR 选择 `Box<dyn>` 但**固定数组长度为 3**,规避动态注册风险。
- **`cascade_risk: AtomicU32`**:用 u32 存储 f32 的位模式,实现无锁读取(对齐 stability.rs `CircuitBreaker` 的 AtomicU8 模式)。写入时 `compare_exchange` 循环,读取时 `load + f32::from_bits`,无锁。

### 决策 2-4 理由(三探针算法)

- **滑动窗口大小 N**:MemoryParadox N=100(召回频率高)、ReasoningTrap N=50(Veto 事件相对少)、EvolutionHack N=200(进化事件稀疏需更长窗口)。N 值可调(可进化面),但算法骨架不可调(不可进化面)。
- **阈值 0.3/0.7**:0.3 = Warning(告警但不阻断),0.7 = Critical(触发膜增厚 + 熔断)。阈值对齐 §6.3 级联风险 >0.7 触发膜增厚。
- **不直接调用 mlc-engine / chimera-mas API**:全部通过 event-bus 订阅,遵守依赖铁律(§2.2)。这是本 ADR 决策 5 方案 A 的具体落地。
- **ReasoningTrap 直接调用 ahirt.rs**:同 crate 同层(L8 内),§2.2 允许同层互引。避免重复实现红队探测逻辑。

### 决策 5 理由(依赖铁律裁决方案 A)

- **不破坏既有依赖**:stability.rs 被 chimera-mas/orchestrator.rs 强依赖(4 处),方案 B 上提需重构,违反"不破坏既有依赖"原则。
- **符合"跨层通信只走 event-bus"铁律**:ImmuneSystem 作为 facade 本就应通过事件解耦,方案 A 是教科书级 facade 模式。
- **延迟代价可接受**:event-bus 的 broadcast 延迟 <1ms(§9.5 SLO 跨膜 p95 <10ms),StabilityMirror 的镜像状态更新延迟 <<100ms KPI-03 预算。
- **镜像状态一致性**:event-bus 的 broadcast 保证至少一次投递(Critical 级走 mpsc 100% 送达,§5.3),镜像最终一致。`last_update_ts` 字段用于检测镜像陈旧(若 >5s 未更新则标记 `mirror_stale=true`,触发探针返回 `insufficient_data`)。

### 决策 6 理由(事件订阅清单)

- **9 个事件覆盖完整**:3 个探针的数据源(L9 任务事件 + L8 安全事件 + L10 降级事件)+ 级联风险输入(BudgetExceeded)。无遗漏。
- **Critical 事件走 mpsc**:对齐 §6.2 红线"SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded 必须用 mpsc channel 确保送达"。
- **订阅时机约束**:对齐 §4.4 反模式 3 "broadcast 先 subscribe 再 spawn"。

### 决策 7 理由(级联联动膜增厚)

- **membrane.rs 落地于 parliament crate**:不新建 crate,符合复杂度预算。membrane.rs 是 ImmuneSystem 的子模块,与 immune_system.rs 同 crate。
- **0-7 共 8 档厚度**:对齐 §6.3 四档(Low/Medium/High/Critical),但细化为 8 档提供更平滑的过渡。档位映射表是可进化面(决策 9)。
- **set_thickness() 接口暴露给 event-bus**:event-bus 在 publish 前查询厚度决定是否本地消化,这是 §6.3 "渗透过滤器(膜新增,替代调用方各自过滤)"的落地。

### 决策 8 理由(KPI-03 <100ms)

- **FuturesUnordered 并行**:对齐 §4.1 通用约定"并发收集用 FuturesUnordered,优于 join_all,减少内存占用,支持流式结果"。
- **Fast Path 80% 跳过**:对齐 §8.2 推理悖论"Fast Path 80% 跳过"。80% 的扫描只读镜像状态,20% 触发完整探针算法。
- **criterion bench 验收**:对齐 §3.4.1 第 6 条"性能可证伪 — 任何性能优化必须有 criterion benchmark 证据"。

### 决策 9 理由(不可进化面)

- **进化悖论红线 3**:GSOE/AutoDPO 使用执行反馈作为验证信号,存在被"奖励黑客"游戏化风险。若 ImmuneSystem 接口本身可被演化,则免疫系统可被游戏化(攻击者通过演化绕过免疫)。
- **接口 vs 算法参数二分法**:接口(签名/枚举/数据结构)不可演化,保证免疫机制稳定性;算法参数(窗口大小/阈值/权重)可演化,允许参数级优化。这是"不可进化面"与"可进化面"的精确划分。
- **`#[non_exhaustive]` 标注**:Rust 标准库惯用模式,强制外部走 `probe_type()` 访问器,避免外部 match 因新变体而 break。

## 影响

### 实施影响

1. **新增文件**(P5.3 待实现,约 800-1000 行):
   - `crates/parliament/src/immune_system.rs`(facade 主接口,约 200 行)
   - `crates/parliament/src/immune_system/memory_paradox.rs`(约 150 行)
   - `crates/parliament/src/immune_system/reasoning_trap.rs`(约 150 行)
   - `crates/parliament/src/immune_system/evolution_hack.rs`(约 150 行)
   - `crates/parliament/src/immune_system/membrane.rs`(约 100 行)
   - `crates/parliament/src/immune_system/types.rs`(ParadoxReport / ProbeType / Severity,约 100 行)
   - `crates/parliament/benches/immune_system.rs`(criterion bench,约 100 行)

2. **修改文件**:
   - `crates/parliament/src/lib.rs`:新增 `pub mod immune_system;` + 重导出
   - `crates/parliament/Cargo.toml`:无新增依赖(复用既有 event-bus / dashmap / crossbeam)
   - `crates/event-bus/src/types.rs`:可能需扩展 `ContextRetrieved` 事件字段(若 mlc-engine 未含 temporal_meta)

3. **依赖铁律影响**:
   - parliament crate **不新增**对 chimera-mas 的依赖(方案 A 走 event-bus 订阅)
   - parliament crate **不新增**对 mlc-engine 的依赖(同上)
   - 新增 parliament 内部子模块 `immune_system/`,与既有 `ahirt.rs` 同层

4. **复杂度预算对账**(§3.4):
   - 新增:ImmuneSystem facade(约 800-1000 行)
   - 抵消:v3.0-stable 文档侧 StabilityGuard 设计(不新建,合并 chimera-mas stability.rs,设计文档已裁决)
   - 净增长:≈ 0(满足 §3.4 净增长 ≤ 0)

5. **测试策略**:
   - 单元测试:每探针 ≥ 5 个单元测试(覆盖正常/边界/异常/Critical/insufficient_data 五类)
   - 集成测试:`tests/e2e/immune_system_integration.rs`(三探针 + 膜增厚联动)
   - proptest:级联风险评分算法的 proptest(输入 paradox_rate ∈ [0,1],输出 cascade_risk ∈ [0,1])
   - criterion bench:`immune_system_scan` p95 < 100ms

6. **不变量**(对齐 INV-9 规格,§9.3):
   - INV-10(新增):ImmuneSystem 探针数量恒为 3(`probes.len() == 3`)
   - INV-11(新增):membrane_thickness ∈ [0, 7]
   - INV-12(新增):cascade_risk ∈ [0.0, 1.0]

7. **CI 影响**:
   - `cargo check -p parliament` 必须通过
   - `cargo test -p parliament` 必须通过(含新增探针测试)
   - `cargo bench -p parliament immune_system_scan` p95 < 100ms
   - 不影响 release.yml / fuzz.yml / audit.yml

### 与既有 ADR 的关系

- **ADR-026(chimera-mas)**:本 ADR 不修改 chimera-mas 的 stability.rs,仅通过 event-bus 订阅其事件。append-only 扩展。
- **ADR-030(unsafe 红线)**:本 ADR 全部使用 safe Rust(AtomicU8/U32 + DashMap + crossbeam),无 unsafe。
- **ADR-033(L0 nexus-contracts)**:`ParadoxReport` 类型若需跨层共享(如 efficiency-monitor 订阅),应上提至 L0 `nexus-contracts`。本 ADR 暂不上提,落地于 parliament 内部,后续视使用范围评估。
- **ADR-034(灰度=能力场)**:探针的启用/禁用通过 CapabilityToken 能力场(非 Feature Flag),探针参数(窗口大小/阈值)通过 CapabilityToken 演化(可进化面)。
- **ADR-042(R2 冻结)**:EvolutionHack 探针订阅 R2 路径事件,若 R2 触发则严重告警。本 ADR 不修改 R2 冻结范围。
- **ADR-043(R1 影子模式)**:无直接关系。ImmuneSystem 不参与 R1 影子模式。
- **ADR-044(RHI-CG)**:EvolutionHack 探针依赖 ADR-044 的通道 B 事件。建议 ADR-044 先行落地。

## 附录

### 附录 A: 三探针算法伪代码汇总

```text
// MemoryParadox 探针
fn detect_memory_paradox(
    context_events: Vec<ContextRetrieved>,
    archive_events: Vec<AgentArchived>,
) -> ParadoxReport {
    let window = context_events.last(100);
    let mut ghost_count = 0;
    for ev in window {
        if ev.temporal_meta.timestamp_diff_days > 7
           && ev.has_concurrent_facts_with_diff_timestamps {
            ghost_count += 1;
        }
    }
    let archive_violation = !archive_events.is_monotonic();
    let paradox_rate = ghost_count as f32 / window.len() as f32
                     + if archive_violation { 0.2 } else { 0.0 };
    ParadoxReport { probe_type: MemoryParadox, paradox_rate, ... }
}

// ReasoningTrap 探针
fn detect_reasoning_trap(
    veto_events: Vec<SkepticVeto>,
    override_events: Vec<VetoOverridden>,
    ahirt: &AhirtRedTeam,
) -> ParadoxReport {
    let window = veto_events.last(50);
    let pattern_count = count_pattern_clusters(window);
    let veto_override_rate = override_events.len() as f32 / window.len() as f32;
    let ahirt_rate = ahirt.verify_security().stats.detection_rate;
    let paradox_rate = if pattern_count > 5 { 0.6 } else { 0.2 }
                     + veto_override_rate * 0.3;
    ParadoxReport { probe_type: ReasoningTrap, paradox_rate, ... }
}

// EvolutionHack 探针
fn detect_evolution_hack(
    evolution_events: Vec<EvolutionEvent>,
    frozen_events: Vec<CapabilityFrozen>,
    r2_events: Vec<R2Violation>,
) -> ParadoxReport {
    let window = evolution_events.last(200);
    let channel_b_veto_rate = count_channel_b_vetoed(window) as f32
                            / window.len() as f32;
    let reward_anomaly = z_score(window.iter().map(|e| e.reward));
    let r2_violation = !r2_events.is_empty();
    let paradox_rate = channel_b_veto_rate;
    ParadoxReport { probe_type: EvolutionHack, paradox_rate, ... }
}

// 级联风险评分
fn compute_cascade_risk(
    paradox_reports: &[ParadoxReport; 3],
    stability_mirror: &StabilityMirror,
    budget_exceeded_count: u32,
) -> f32 {
    let max_paradox = paradox_reports.iter()
        .map(|r| r.paradox_rate)
        .fold(0.0f32, f32::max);
    let circuit_open_ratio = stability_mirror.circuit_open_ratio();
    0.5 * max_paradox
        + 0.3 * circuit_open_ratio
        + 0.2 * (budget_exceeded_count as f32 / 10.0).min(1.0)
}
```

### 附录 B: 事件订阅清单

```rust
// ImmuneSystem::new() 中的事件订阅(决策 6)
impl ImmuneSystem {
    pub fn new(bus: &EventBus) -> Self {
        let mirror = StabilityMirror::new();

        // Critical 事件走 mpsc(对齐 §6.2 红线)
        mirror.subscribe_critical(bus.subscribe_mpsc::<SkepticVeto>());
        mirror.subscribe_critical(bus.subscribe_mpsc::<RedTeamAudit>());
        mirror.subscribe_critical(bus.subscribe_mpsc::<AgentTaskFailed>());
        mirror.subscribe_critical(bus.subscribe_mpsc::<VetoOverridden>());
        mirror.subscribe_critical(bus.subscribe_mpsc::<BudgetExceeded>());

        // Normal 事件走 broadcast
        mirror.subscribe_normal(bus.subscribe::<AgentTaskCompleted>());
        mirror.subscribe_normal(bus.subscribe::<AgentTaskDelegated>());
        mirror.subscribe_normal(bus.subscribe::<CsnSubstitutionTriggered>());
        mirror.subscribe_normal(bus.subscribe::<CapabilityFrozen>());

        Self {
            probes: [
                Box::new(MemoryParadoxProbe::new(mirror.clone())),
                Box::new(ReasoningTrapProbe::new(mirror.clone())),
                Box::new(EvolutionHackProbe::new(mirror.clone())),
            ],
            stability_mirror: mirror,
            cascade_risk: AtomicU32::new(0),
            membrane_thickness: AtomicU8::new(0),
        }
    }
}
```

### 附录 C: 不可进化面清单(决策 9)

| 类型 | 路径 | 不可/可进化 |
|---|---|---|
| `ImmuneSystem` trait 签名 | `parliament/src/immune_system.rs` | 不可进化 |
| `ParadoxProbe` trait 签名 | `parliament/src/immune_system/types.rs` | 不可进化 |
| `ProbeType` enum 变体集 | `parliament/src/immune_system/types.rs` | 不可进化(`#[non_exhaustive]`) |
| `ParadoxReport` 数据结构 | `parliament/src/immune_system/types.rs` | 不可进化 |
| `membrane::set_thickness()` 接口 | `parliament/src/immune_system/membrane.rs` | 不可进化 |
| 滑动窗口大小 N | 各探针内部 | 可进化(通过 CapabilityToken) |
| 阈值 0.3/0.7 | 各探针内部 | 可进化 |
| 级联风险权重 0.5/0.3/0.2 | `compute_cascade_risk()` | 可进化 |
| 膜厚度档位映射表 | `membrane.rs` | 可进化 |

### 附录 D: 与 §8.1 ImmuneSystem 四层结构对齐表

| §8.1 层级 | 落地位置 | 本 ADR 决策 | 状态 |
|---|---|---|---|
| 先天免疫(屏障) | seccore(既有) | 复用,不修改 | 既有 |
| 先天免疫(吞噬) | qeep-protocol OrphanDetector(既有) | 复用,不修改 | 既有 |
| 适应性免疫(压力源应答) | chimera-mas stability.rs(既有) | 决策 5/6 复用 + 事件订阅 | 既有 |
| 适应性免疫(悖论检测) | parliament/immune_system/ | 决策 1-4 新增 | P5.3 待实现 |
| 适应性免疫(级联预测) | parliament/immune_system/membrane.rs | 决策 7 新增 | P5.3 待实现 |
| 免疫记忆 | gsoe-evolution + auto-dpo(既有) | 复用,不修改 | 既有 |
| 免疫耐受 | PdcaLoop 4 告警规则(既有) | 复用,不修改 | 既有 |

---

> **复核日期**: 2026-07-26(本 ADR 创建日)
> **下次复核**: P5.3 实施完成后 + 首次 KPI-03 基准发布后
> **撰写人**: E02 安全架构师 + E03 记忆系统专家
> **审核状态**: 待 E01 首席架构师 + E04 路由算法专家交叉评审
