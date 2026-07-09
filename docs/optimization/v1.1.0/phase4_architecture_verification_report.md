# Phase IV P1 架构补债验证报告

> Spec: `d:\Chimera CLI\.trae\specs\v1-1-0-systematic-optimization-deep-analysis\tasks.md` Task IV-1 ~ IV-7
> 执行日期: 2026-07-09
> 验证环境: Windows 11 + PowerShell + stable-x86_64-pc-windows-gnu
> 关联 commits: 211e91c(F1) / 4f10603(C1) / 9267553(N9) / e23337f(N6) / 83e0358(N7) / 1770a9a(N8)

## 1. 验收摘要

| 检查项 | 命令 | 结果 |
|--------|------|------|
| 全量测试 | `cargo test --workspace --jobs 1` | 通过(3219 passed + 51 E2E 修复后通过 / 0 failed / 53 ignored;测试增量 +23:C1:5 + N7:11 + N9:3 + N8:3 + F1:1) |
| Clippy | `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` | 通过,零警告 |
| Format | `cargo fmt --all -- --check` | 通过,零 diff |
| 远程同步 | `git push origin master` | 通过,所有 commit 已推送(83e0358 N7 / f098ae2 docs / e41644c E2E fix,2026-07-09) |

Phase IV 6 项 P1 架构补债任务状态(D1 延后 Phase V):

| 任务 | 状态 | 关键交付物 | Commit |
|------|------|-----------|--------|
| IV-1 F1 配置类型迁移到 nexus-core | 已完成 | `crates/nexus-core/src/config.rs` 14 个 section 类型 + `chimera-cli/src/config.rs` re-export | `211e91c` |
| IV-2 C1 event-bus EventTopic + FilteredSubscriber | 已完成 | `crates/event-bus/src/topic.rs` 9 类 EventTopic + FilteredSubscriber + 5 测试 | `4f10603` |
| IV-3 N9 sesa-router 前置事件校验 | 已完成 | `crates/sesa-router/src/prerequisite.rs` PrerequisiteChecker + 3 测试 | `9267553` |
| IV-4 N6 acb-governor 滞后机制 | 已完成 | `crates/acb-governor/src/governor.rs` tier_switch_lag_ms(默认 1000ms) | `e23337f` |
| IV-5 N7 TTG ACB/DECB 仲裁层 | 已完成 | `crates/quest-engine/src/arbitration.rs` ArbitrationLayer + 11 测试 | `83e0358` |
| IV-6 N8 parliament Skeptic 否决覆议 | 已完成 | `crates/parliament/src/debate.rs` reopen_veto + override_consensus_threshold + 3 测试 | `1770a9a` |
| IV-7 D1 repo-wiki r2d2 连接池 | 延后 Phase V | 架构决策:Phase III-4 写线程分离已满足 WAL 并发读需求 | N/A |

## 2. 任务详情与代码 Before/After

### 2.1 F1 配置类型迁移到 nexus-core

**状态**: 已完成,向后兼容。

**优化前**: 14 个 section 配置类型定义在 `crates/chimera-cli/src/config.rs`,L2-L9 各 crate 依赖 `chimera-cli`(L10)违反 §2.2 依赖铁律(L(N)→L(N+1) 禁止)。

**优化后**: 配置类型下沉到 L1 `nexus-core/src/config.rs`,`chimera-cli` 改为 `pub use nexus_core::config::*;` re-export 保持向后兼容。

```rust
// crates/nexus-core/src/config.rs(新建)
pub struct CosmosConfig { /* ... */ }
pub struct SeccoreConfig { /* ... */ }
// ... 14 个 section 类型

// crates/chimera-cli/src/config.rs(re-export)
pub use nexus_core::config::*;
```

**WHY**: L1 配置类型共享消除"平行类型漂移风险"——多个 crate 各自维护配置类型副本会导致配置不一致。集中到 L1 后,所有下游通过 `use nexus_core::config::*` 引用,单一真相源。

**测试**: `crates/nexus-core/tests/config_test.rs::test_config_types_in_nexus_core` 验证 14 个类型可从 nexus_core 直接导入。

### 2.2 C1 event-bus EventTopic + FilteredSubscriber

**状态**: 已完成,9 类分类覆盖全部 66 个 NexusEvent 变体。

**决策**: 采用方案 B(9 类),架构纯净度优先。每个 topic 对应一个功能域:

```rust
// crates/event-bus/src/topic.rs
pub enum EventTopic {
    Routing,    // L6 Router:OSA/KVBSR/FaaE/SESA/GEA (11 变体)
    Memory,     // L2 Memory:NMC/MLC/HCW/CMT (7 变体,含 NmcEncoded)
    Security,   // L4 Security:SecCore/Decay/ASA/AHIRT (8 变体)
    Execution,  // L7 Execution:PVL/MTPE/SSRA (11 变体)
    Parliament, // L8 Parliament:投票/共识/预算 (7 变体)
    Quest,      // L9 Quest:意图/任务/检查点 (7 变体)
    System,     // L10 Interface + 跨层告警 (6 变体)
    Knowledge,  // L5 Knowledge:Wiki/GSOE/AutoDPO (4 变体)
    Storage,    // L3 Storage:SCC/LSCT (5 变体)
}
```

**FilteredSubscriber 包装 EventReceiver**,仅接收匹配 topic 的事件,不匹配事件从缓冲区消费丢弃(与 `recv_matching` 语义一致):

```rust
// crates/event-bus/src/topic.rs
pub struct FilteredSubscriber {
    inner: crate::bus::EventReceiver,
    topics: HashSet<EventTopic>,
}

impl FilteredSubscriber {
    pub(crate) fn new(inner: crate::bus::EventReceiver, topics: HashSet<EventTopic>) -> Self {
        Self { inner, topics }
    }
    pub fn try_recv(&mut self) -> Result<Option<NexusEvent>, RecvError> { /* 跳过不匹配 */ }
}

// crates/event-bus/src/bus.rs
impl EventBus {
    pub fn subscribe_filtered(&self, topics: HashSet<EventTopic>) -> FilteredSubscriber {
        FilteredSubscriber::new(self.subscribe(), topics)
    }
}
```

**WHY 9 类而非 66 类**: 66 个变体一一映射 topic 失去过滤意义;9 类按架构层职责划分,FilteredSubscriber 订阅指定 topic 集合即可减少无关事件对消费者缓冲区的占用。既有 `subscribe()` 保持全量广播,向后兼容。

**测试**: 5 个测试用例覆盖 topic 覆盖完整性、FilteredSubscriber 过滤、向后兼容、9 类穷尽性。

### 2.3 N9 sesa-router 前置事件校验

**状态**: 已完成,PrerequisiteChecker 默认启用(安全优先)。

**优化前**: SESA 激活路径无前置校验,可能在 OSA/KVBSR/FaaE 未就绪时激活,违反五层路由顺序。

**优化后**: `PrerequisiteChecker` 在构造时同步 `bus.subscribe_filtered()`,监听 Routing topic;`activate()` 入口校验上游事件,未收到时返回 `SesaError::PrerequisiteNotMet`。

```rust
// crates/sesa-router/src/prerequisite.rs
pub struct PrerequisiteChecker {
    subscriber: Mutex<FilteredSubscriber>,
    osa_seen: Mutex<bool>,
    kvbsr_seen: Mutex<bool>,
    faae_seen: Mutex<bool>,
    enabled: bool,
}

impl PrerequisiteChecker {
    pub fn new(event_bus: &EventBus) -> Self {
        // WHY 同步 subscribe:遵守 broadcast 反模式,spawn 前订阅避免事件丢失
        let mut topics = HashSet::new();
        topics.insert(EventTopic::Routing);
        let subscriber = event_bus.subscribe_filtered(topics);
        Self { subscriber: Mutex::new(subscriber), /* ... */, enabled: true }
    }

    pub fn check(&self) -> Result<(), SesaError> {
        if !self.enabled { return Ok(()); }
        self.drain_events();
        if self.osa_seen.lock().unwrap().clone() && /* ... */ {
            Ok(())
        } else {
            Err(SesaError::PrerequisiteNotMet)
        }
    }
}
```

**WHY 默认启用**: 五层路由顺序(OSA → KVBSR → FaaE → SESA → GEA)是 OMEGA Ω-Sparse 核心约束,代码强制优于文档约定。测试场景可通过 `prerequisite_check_enabled: false` 显式禁用。

**测试**: 3 个测试覆盖无上游事件拒绝、有上游事件通过、默认启用行为。

### 2.4 N6 acb-governor 滞后机制

**状态**: 已完成,复用 DECB 的 `tier_switch_lag_ms` 模式。

**优化前**: 利用率在阈值附近波动时,ACB 立即切换 tier 导致振荡(tier 抖动)。

**优化后**: 引入 `tier_switch_lag_ms`(默认 1000ms),`Mutex<Option<DateTime<Utc>>>` 记录上次切换时间,check-then-act 原子化避免竞态。

```rust
// crates/acb-governor/src/governor.rs
pub struct AcbGovernor {
    // ...
    last_tier_switch: Mutex<Option<DateTime<Utc>>>,
    tier_switch_lag_ms: u64,  // 默认 1000ms
}

impl AcbGovernor {
    fn should_switch_tier(&self) -> bool {
        let last = self.last_tier_switch.lock().unwrap();
        match *last {
            Some(t) => {
                let elapsed = Utc::now().signed_duration_since(t).num_milliseconds();
                elapsed >= self.tier_switch_lag_ms as i64
            }
            None => true,  // 首次切换无限制
        }
    }
}
```

**WHY 复用 DECB 模式**: DECB 已在 Phase II 实现 `tier_switch_lag_ms`,ACB 复用相同模式保持架构一致性,避免引入新概念。`Mutex<Option<DateTime<Utc>>>` + check-then-act 原子化避免 TOCTOU 竞态(§4.4 反模式 #1)。

### 2.5 N7 TTG ACB/DECB 仲裁层

**状态**: 已完成,保守取严策略。

**优化前**: TTG 仅订阅 DECB 事件,忽略 ACB 档位,可能导致预算已降级但 TTG 仍选择 Deep 模式。

**优化后**: `ArbitrationLayer` 同时订阅 ACB 与 DECB 的 `BudgetAdjusted` 事件,通过 `metadata.source` 区分发布者,保守取严:

```rust
// crates/quest-engine/src/arbitration.rs
pub struct ArbitrationLayer {
    acb_tier: Mutex<Option<AcbTier>>,
    decb_tier: Mutex<Option<BudgetTier>>,
    subscriber: Mutex<Option<FilteredSubscriber>>,
    enabled: bool,
}

impl ArbitrationLayer {
    pub fn arbitrated_tier(&self) -> Option<BudgetTier> {
        self.drain_events();
        let acb = self.acb_tier.lock().unwrap();
        let decb = self.decb_tier.lock().unwrap();
        match *acb {
            // 保守取严:ACB 低档位优先,DECB 跟随
            Some(AcbTier::L0) => Some(BudgetTier::Degraded),  // L0_degraded → Degraded
            Some(AcbTier::L1) => Some(BudgetTier::LowTier),   // L1_basic → LowTier
            Some(AcbTier::L2) | Some(AcbTier::L3) | None => *decb,  // 跟随 DECB
        }
    }
}
```

**WHY 字符串解析而非依赖 acb-governor crate**: `quest-engine`(L9)依赖 `acb-governor`(L8)违反 §2.2 依赖铁律。通过字符串解析 ACB tier("L0_degraded" 等)避免向上依赖,保持最小依赖原则。`FilteredSubscriber` 订阅 Parliament topic 即可接收两个治理器的 `BudgetAdjusted` 事件。

**WHY Mutex<Option<FilteredSubscriber>>**: `FilteredSubscriber::try_recv()` 需要 `&mut self`,但 `drain_events()` 是 `&self` 方法。用 `Mutex` 包装获得内部可变性(与 N9 PrerequisiteChecker 设计模式一致)。

**测试**: 11 个集成测试覆盖保守取严全部分支 + TtgGovernor 集成(`effective_tier()` / `select_mode_with_arbitration()`)。

### 2.6 N8 parliament Skeptic 否决覆议

**状态**: 已完成,2/3 超级多数覆议机制。

**决策**: 采用方案 C(配置阈值),新增 `override_consensus_threshold`(默认 0.667 = 2/3)。

**优化前**: Skeptic 否决后无覆议路径,可能导致合理方案被一票否决。

**优化后**: `reopen_veto()` 公开方法重新开启辩论,4 角色(Explorer/Architect/Skeptic/Validator)中 3 个或以上赞成可推翻 Skeptic 否决:

```rust
// crates/parliament/src/debate.rs
pub fn reopen_veto(&self, ballot_id: &str) -> Result<DebateResult, ParliamentError> {
    // 票据校验(防止伪造)
    if !self.validate_ballot(ballot_id)? {
        return Err(ParliamentError::InvalidBallot);
    }
    // 使用 override_consensus_threshold(0.667)而非默认 consensus_threshold(0.5)
    self.deliberate_with_override()
}

fn deliberate_with_override(&self) -> DebateResult {
    let votes = self.collect_votes();
    let threshold = self.config.override_consensus_threshold;  // 0.667
    let approval_rate = votes.approvals as f32 / votes.total as f32;
    if approval_rate >= threshold {
        DebateResult::Overridden  // 2/3 超级多数推翻否决
    } else {
        DebateResult::VetoSustained
    }
}
```

**WHY 2/3 超级多数而非简单多数(0.5)**: 防止轻率绕过红队安全防线。Skeptic 代表安全审计,推翻其否决需要更高门槛。0.667 对应 3/4 角色赞成,即除 Skeptic 外其他 3 角色一致同意。

**测试**: 3 个测试覆盖有效票据覆议成功、不匹配票据拒绝、超级多数未达维持否决。

### 2.7 D1 repo-wiki r2d2 连接池(延后 Phase V)

**状态**: 延后决策,已有架构决策记录。

**决策**: 延后到 Phase V,r2d2 与 Phase III-4 写线程分离(mpsc + spawn_blocking + read_conns)冲突,现有架构已满足 WAL 并发读需求。Phase III-4 验证显示写入时读取不阻塞,10 并发读 ~1280 万 ops/s。

**WHY 延后**: 引入 r2d2 需要重构 `store.rs` 的写线程架构,与 Phase III-4 刚稳定的写线程分离冲突。现有 `Arc<Vec<Mutex<Connection>>>` 只读连接池已满足需求,r2d2 收益有限。Phase V 可重新评估是否引入连接池抽象层。

## 3. 关键设计教训

1. **EventTopic 9 类 vs 66 类权衡**: 细粒度(66 类)失去过滤意义,粗粒度(2 类 severity)无法支撑 N9 PrerequisiteChecker 等只需 Routing 事件的场景。9 类按架构层职责划分是架构纯净度与实用性的平衡点。

2. **FilteredSubscriber 内部可变性**: `try_recv()` 需要 `&mut self`,但订阅者通常作为共享状态(`Arc<T>` 或字段)。用 `Mutex<Option<FilteredSubscriber>>` 包装获得内部可变性,与 N9 PrerequisiteChecker 设计模式一致。直接 `&self` + `RefCell` 在多线程场景不适用。

3. **跨层依赖规避字符串解析**: `quest-engine`(L9)需要 ACB tier 信息,但 L9→L8 依赖违反铁律。通过 `metadata.source` 字符串解析避免依赖 `acb-governor` crate,保持最小依赖原则。代价:字符串解析脆弱,需测试覆盖所有合法值。

4. **保守取严仲裁策略**: ACB 与 DECB 档位不一致时,取更严格的一方。ACB L0(降级)→ DECB Degraded,ACB L1(基础)→ DECB LowTier,ACB L2/L3 → 跟随 DECB。确保预算紧张时 TTG 选择更保守的思考模式(Fast 而非 Deep)。

5. **broadcast subscribe 时序**: `FilteredSubscriber` 必须在 `tokio::spawn` 之前同步创建(构造时调用 `subscribe_filtered()`),否则事件静默丢失。这是 §4.4 反模式 #3 的强制约束。`ArbitrationLayer::new()` 与 `PrerequisiteChecker::new()` 均在构造时同步订阅。

6. **2/3 超级多数覆议门槛**: 安全审计否决(Skeptic veto)的覆议需要更高门槛(0.667)而非简单多数(0.5)。这防止"3 人小团体绕过 1 人安全否决"的轻率决策,要求除 Skeptic 外其他 3 角色一致同意才能推翻。

## 4. 远程同步验证

```
$ git log origin/master..HEAD --oneline
(空,无未推送 commit)

$ git status -sb
## master...origin/master
```

commit `83e0358`(N7 TTG ACB/DECB 仲裁层)于 2026-07-09 成功推送到 `origin/master`。所有 Phase IV P1 架构补债代码已同步至远程仓库。

## 5. 关联文档

- Spec: `.trae/specs/v1-1-0-systematic-optimization-deep-analysis/spec.md`
- Tasks: `.trae/specs/v1-1-0-systematic-optimization-deep-analysis/tasks.md`
- Checklist: `.trae/specs/v1-1-0-systematic-optimization-deep-analysis/checklist.md`
- Phase I 报告: `docs/optimization/v1.1.0/phase1_security_verification_report.md`
- Phase II 报告: `docs/optimization/v1.1.0/phase2_correctness_verification_report.md`
- Phase III 报告: `docs/optimization/v1.1.0/phase3_performance_verification_report.md`
- 性能基线对比: `docs/optimization/v1.1.0/performance_baseline_comparison.md`
- ADR 索引: `CODE_WIKI.md §2.3`(ADR-007 ~ ADR-010)
