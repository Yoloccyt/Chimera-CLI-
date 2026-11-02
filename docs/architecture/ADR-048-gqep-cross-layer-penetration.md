# ADR-048: gqep-executor L7→L4 跨层渗透例外记录

> **状态**: Accepted (2026-08-01)(P9-T6 合规审查通过,2026-08-04)
> **状态更迭(2026-09-24 补记,兑现 ADR-185 D4 末段对主检出侧的委派)**: 本 ADR 的**例外豁免**已于 wave 3c / ADR-185 D4 **收编**——`gqep-executor`(L7)→`qeep-protocol`(L4) 属**合法向下边**,治理脚本 `scripts/check_dependency_rules.sh:217` 不再为其保留 `is_adr_exception` 条目。本 ADR 的 INV-GQEP-1~4 不变量与"不以 Event Bus 解耦消除该边"的结论**仍然有效**(ADR-185 D4 明确否决"消除"选项:性能证据未变);被收编的只是"这是一处需要豁免的异常"这一名义。
> **决策日期**: 2026-07-27
> **关联 ADR**: ADR-033(L0 nexus-contracts 依赖铁律扩展) / ADR-003(Event Bus) / ADR-021(MCP Mesh 2PC)
> **关联文档**: `nuxus规则.md` §2.2 依赖铁律 / §3.4.5 4 类病理诊断
> **关联 spec**: `systematic-deep-analysis-optimization` Phase 8 P1-4
> **权威源**: 本 ADR 记录 gqep-executor → qeep-protocol 跨层依赖现状的处置决策

---

## 1. 背景

### 1.1 依赖铁律回顾

`nuxus规则.md` §2.2 定义 OMEGA 十层架构的依赖铁律:

```
L(N) → L(N)    ✓ 同层互引允许
L(N) → L(N-1)  ✓ 向下依赖允许
L(N) → L(N+1)  ✗ 向上依赖禁止(铁律)
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走 Event Bus
```

铁律的工程价值在于:
- **编译期阻断循环依赖**:L0/L1 基础层零生产依赖切断回边
- **降低测试耦合**:层间通信单向化,测试可按层独立编排
- **支持三环重组**:内环 9 crate 共享内存 <1ms / 外环 25 crate 事件驱动,依赖方向一致性是前提

### 1.2 §3.4.5 4 类病理诊断

`三环循环_十层接口_元架构重组深度分析.md` §3.4.5 诊断当前 35→37 crate 架构存在 4 类病理:

| 病理类型 | 具体表现 | 状态 |
|---------|---------|------|
| 星型耦合 | L6 Router 4 crate 都依赖 osa-coordinator | ⚠️ 待治理 |
| **跨层渗透** | **L7 gqep-executor 依赖 L4 qeep-protocol** | **本 ADR 处置** |
| 隐式循环依赖 | L5 GSOE ↔ L9 Quest 通过 Event Bus 逻辑循环 | ⚠️ 已记录 |
| L1 上帝 crate | nexus-core / event-bus 被所有 37 crate 依赖 | ⚠️ 待治理 |

### 1.3 实证核验(2026-07-27)

Phase 8 P1-4 调研完成的实证发现:

**依赖声明**(`crates/gqep-executor/Cargo.toml:12`):
```toml
# L4 Security:QEEP 协议,提供 OrphanDetector 与 entangle 包裹(零孤儿调用保证)
qeep-protocol = { path = "../qeep-protocol" }
```

**类型导入**(`crates/gqep-executor/src/gatherer.rs:21`):
```rust
use qeep_protocol::{QeepError as QeepErr, QeepProtocol};
```

**字段持有**(`crates/gqep-executor/src/gatherer.rs:43`):
```rust
pub struct GqepExecutor {
    pub(crate) config: GqepConfig,
    pub(crate) event_bus: EventBus,
    /// QEEP 协议实例,提供孤儿调用检测(entangle 包裹 + OrphanGuard)
    pub(crate) qeep: QeepProtocol,
}
```

**方法调用**(`crates/gqep-executor/src/gatherer.rs:58` 及 `gather` 方法内):
```rust
let qeep = QeepProtocol::new(default_timeout);
// ...
// 调用 qeep.entangle() 方法对每个 future 进行量子纠缠包裹
```

**关键结论**:这是**功能性依赖**(非纯类型导入)。gqep-executor 不仅使用 QeepProtocol 类型,还:
1. 持有 `QeepProtocol` 实例作为结构体字段
2. 调用 `QeepProtocol::new()` 构造方法
3. 调用 `qeep.entangle()` 实例方法
4. 完整的 `map_gqep_to_qeep` / `map_qeep_to_gqep` 双向错误映射

### 1.4 设计意图

QEEP 协议为 GQEP 提供的核心能力:

- **`OrphanDetector`**:跟踪 `pending_calls`,Future 正常完成标记 `completed=true`,异常 drop 触发 `OrphanCallDetected` Critical 事件
- **`entangle` 包裹**:对每个 future 进行量子纠缠包裹,利用 `OrphanGuard` 在 future drop 时检测孤儿调用
- **零孤儿调用保证**:§6.2 架构红线"所有异步操作必须有 GQEP 聚集/超时处理"的工程实现

QEEP 的 `entangle` 操作是**同步低延迟调用**,设计目标是 <10μs 完成包裹注册。

---

## 2. 决策

**接受现状,标记为已知技术债,推迟至三环重组根治**。

具体决策项:

### 决策 1: 接受 L7→L4 跨层依赖现状(方案 C)

gqep-executor 对 qeep-protocol 的功能性依赖作为 ADR 例外接受,理由:
- 方案 A(类型下移至 L0)不适用:QeepProtocol 含 `pending_calls` 状态 + `OrphanDetector` 协议逻辑,非纯类型契约,不适合 L0 纯契约层
- 方案 B(Event Bus 解耦)性能代价过大:`entangle` 是同步低延迟调用(<10μs),改 Event Bus 异步通信会引入显著延迟(>1ms),违反 QEEP 设计意图
- 方案 C(ADR 例外)是当前现状的诚实记录,无工程代价

### 决策 2: 不变量与测试覆盖加固

接受现状的同时,以下不变量必须持续满足,作为该例外的合规边界:

- **INV-GQEP-1**: `gqep-executor` 对 `qeep-protocol` 的依赖仅限于 `QeepProtocol` 类型 + `entangle` 方法 + `QeepError` 错误映射,不得扩展至其他 L4 crate
- **INV-GQEP-2**: `gatherer.rs` 的 `entangle` 调用必须保持同步低延迟路径,不得引入异步等待或网络调用
- **INV-GQEP-3**: 错误映射 `map_gqep_to_qeep` / `map_qeep_to_gqep` 必须保持双向完整,新增 QeepError 变体时必须同步新增 GqepError 映射
- **INV-GQEP-4**: `tests/orphan_detection.rs` 必须保持覆盖孤儿调用检测路径,CI 不通过则阻塞发布

### 决策 3: 复审触发条件

该例外应在以下任一条件触发时复审:

- **触发条件 1**: 三环重组方案启动(内环 9 crate + 外环 25 crate),届时 gqep-executor 与 qeep-protocol 可能同属内环,跨层依赖自然消解
- **触发条件 2**: QEEP 协议接口扩展引入新的 L4 依赖需求(超出 `QeepProtocol` + `entangle` 范围)
- **触发条件 3**: 性能基线显示 `entangle` 调用延迟超过 100μs(当前基线未实施测量,作为 P2 待办)
- **触发条件 4**: §6.2 红线"零孤儿调用保证"出现回归(`OrphanCallDetected` 事件误报或漏报)

### 决策 4: 长期根治路径(三环重组)

参考 `三环循环_十层接口_元架构重组深度分析.md`,长期根治方案:

- **内环 9 crate**:mlc-engine / hcw-window / nmc-encoder / quest-engine / parliament / gea-activator / gsoe-evolution / auto-dpo / repo-wiki,共享内存 + 直接调用,延迟 <1ms
- **外环 25 crate**:事件驱动,通过 `CoreLoopEvent` 与内环通信
- **gqep-executor 与 qeep-protocol 的归属**:三环重组时评估是否同属内环(若都属内环,则 L7→L4 跨层依赖自然消解为内环直接调用);若分属内外环,则需通过事件解耦

三环重组属于 v3.x 演进方向,本 ADR 不强制时间表。

### 决策 5: 文档同步

- 本 ADR 落档后,`nuxus规则.md` §2.2 依赖铁律章节追加"ADR-048 例外"说明
- `docs/architecture/CODE_WIKI.md` gqep-executor 条目新增"跨层渗透例外(ADR-048)"标注
- `docs/architecture/adr_index.md` ADR 总数 42 → 43,新增 ADR-048 表行

---

## 3. 理由

### 3.1 方案 A(类型下移至 L0)否决理由

| 维度 | 评估 |
|------|------|
| L0 定位 | ADR-033 定义 L0 nexus-contracts 为"纯类型 + 零逻辑 + 零依赖"契约层 |
| QeepProtocol 性质 | 含 `pending_calls: DashMap<CallId, PendingCall>` 状态 + `OrphanDetector` 协议逻辑 + `entangle` 方法 |
| 下移可行性 | ❌ 否决:QeepProtocol 非纯类型,下移会破坏 L0 零逻辑约束 |
| 替代尝试 | 考虑将 `QeepProtocol` 拆分为"类型契约(下移 L0)+ 协议实现(留 L4)",但拆分后 gqep-executor 仍需依赖 L4 协议实现,未解决跨层渗透 |

### 3.2 方案 B(Event Bus 解耦)否决理由

| 维度 | 评估 |
|------|------|
| 设计意图 | QEEP `entangle` 是同步低延迟调用,设计目标 <10μs 完成包裹注册 |
| Event Bus 延迟 | broadcast + mpsc 双通道典型延迟 100μs~1ms(P1-W2.1 有界化后),比同步调用慢 10~100× |
| 性能影响 | 🔴 高:`gather` 方法可能聚集 100+ future,每个 future 经 entangle 包裹,Event Bus 解耦会引入 10ms+ 累积延迟 |
| 架构红线冲突 | 违反 §6.2"所有异步操作必须有 GQEP 聚集/超时处理"的性能预期(GQEP 应低延迟,不该自己成为性能瓶颈) |
| 替代尝试 | 考虑仅 `OrphanCallDetected` 事件走 Event Bus(已实现),`entangle` 同步调用保留,但这并未消除 gqep-executor 对 qeep-protocol 的依赖 |

### 3.3 方案 C(ADR 例外)接受理由

| 维度 | 评估 |
|------|------|
| 工程代价 | 零:无代码变更,仅 ADR 记录 |
| 风险可控 | INV-GQEP-1~4 不变量 + 测试覆盖 + 复审触发条件构成合规边界 |
| 诚实记录 | 当前现状的诚实记录,符合"性能可证伪 + 学术支撑落地"原则 |
| 长期路径明确 | 三环重组方案根治,本 ADR 不阻塞 v3.x 演进 |
| 业界先例 | 微服务架构中"同步 RPC 优先 + 异步事件解耦"是常见模式,跨层同步调用在性能敏感路径可接受 |

### 3.4 决策一致性核验

| 原则 | 一致性 | 说明 |
|------|--------|------|
| OMEGA 四定律 | ✅ | Ω-Event 要求跨层通信走 Event Bus,但 QEEP 的同步调用是"零孤儿调用保证"的工程实现,非状态变更广播 |
| 依赖铁律(§2.2) | ⚠️ 例外 | 本 ADR 正式记录例外,不破坏铁律的普遍性 |
| 性能可证伪(§3.4.1) | ✅ | 决策基于 `entangle` 同步低延迟设计意图,触发条件 3 要求性能基线测量 |
| 长期主义(§3.4.1) | ✅ | 推迟至三环重组根治,不短期硬解 |
| 学术支撑落地(§3.4.1) | ✅ | 参考 PiKV (arXiv:2508.06526) 分层 KV 缓存策略 + 工业尸检(Claude Code 孤儿 Promise 教训) |

---

## 4. 影响

### 4.1 短期影响(v2.4.0-omega)

- **依赖铁律**:L7→L4 跨层依赖作为 ADR-048 例外接受,不阻塞 v2.4.0-omega 发布
- **测试回归**:`tests/orphan_detection.rs` 必须保持覆盖,CI 不通过则阻塞发布
- **文档同步**:nuxus规则.md / CODE_WIKI.md / adr_index.md 同步本 ADR

### 4.2 中期影响(v2.5.0~v2.x)

- **性能基线**:触发条件 3 要求测量 `entangle` 调用延迟,作为 P2 待办
- **接口稳定性**:gqep-executor 对 qeep-protocol 的依赖接口(`QeepProtocol` + `entangle` + `QeepError`)冻结,扩展需 ADR 评审
- **不变量守护**:INV-GQEP-1~4 纳入 Phase 9 验证清单,每次发布前核验

### 4.3 长期影响(v3.x 三环重组)

- **根治路径**:三环重组时评估 gqep-executor 与 qeep-protocol 归属,自然消解跨层依赖
- **本 ADR 状态变更**:三环重组完成后,本 ADR 状态从 Proposed → Superseded,由新 ADR 取代
- **依赖铁律恢复**:三环重组后,L7→L4 跨层依赖消除,依赖铁律恢复普遍适用

---

## 5. 替代方案(已否决)

### 5.1 方案 A: 类型下移至 L0 nexus-contracts

**否决原因**:QeepProtocol 含协议逻辑(非纯类型),下移会破坏 L0 零逻辑约束(ADR-033)。

### 5.2 方案 B: Event Bus 解耦

**否决原因**:`entangle` 同步低延迟调用改 Event Bus 会引入 10~100× 延迟,违反 QEEP 设计意图与 §6.2 性能预期。

### 5.3 方案 D: 重新分层(将 qeep-protocol 上移至 L7 同层)

**否决原因**:
- qeep-protocol 当前属于 L4 安全层,与 seccore / decay-engine 共同构成安全三件套
- 上移至 L7 会破坏 L4 安全层的完整性(seccore + qeep + decay 是 ADR-002 能力衰减模型的协同三件套)
- 重新分层影响面过大,需评估所有 L4 依赖关系,不适合作为本 ADR 的处置方案

---

## 6. 工程约束

### 6.1 不变量清单

| 不变量 | 验证方式 | CI 集成 |
|--------|---------|---------|
| INV-GQEP-1 | 依赖范围核验(Cargo.toml + use 语句) | 手动核验,新增依赖需 ADR 评审 |
| INV-GQEP-2 | `entangle` 调用路径审计(同步 + 无网络) | 代码审查 + clippy 静态检查 |
| INV-GQEP-3 | 错误映射双向完整性 | `tests/error_mapping.rs` 单元测试 |
| INV-GQEP-4 | 孤儿调用检测路径 | `tests/orphan_detection.rs` 集成测试 |

### 6.2 测试覆盖要求

- `crates/gqep-executor/tests/orphan_detection.rs`:覆盖 `entangle` 包裹 + `OrphanGuard` drop 检测 + `OrphanCallDetected` 事件发布
- `crates/gqep-executor/tests/error_mapping.rs`:覆盖 `map_gqep_to_qeep` / `map_qeep_to_gqep` 双向映射完整性
- `crates/gqep-executor/benches/gather.rs`:criterion benchmark 测量 `gather` 方法延迟(含 `entangle` 调用),作为触发条件 3 的性能基线

### 6.3 文档同步要求

- `nuxus规则.md` §2.2:追加"ADR-048 例外:gqep-executor → qeep-protocol L7→L4 跨层依赖,接受现状,推迟至三环重组根治"
- `docs/architecture/CODE_WIKI.md` gqep-executor 条目:新增"跨层渗透例外(ADR-048)"标注
- `docs/architecture/adr_index.md`:ADR 总数 42 → 43,新增 ADR-048 表行
- `docs/architecture/audit/deep-analysis-2026-07-27.md` §8.3:更新 P1-4 状态为"✅ ADR-048 落档"

---

## 7. 时间表

| 阶段 | 时间 | 交付物 |
|------|------|--------|
| Proposed | 2026-07-27 | 本 ADR 文档 |
| Accepted | 2026-07-28~2026-08-15 | 文档同步(nuxus规则.md / CODE_WIKI.md / adr_index.md) |
| 性能基线 | 2026-08-15~2026-09-15 | `entangle` 调用延迟 benchmark(触发条件 3) |
| 三环重组评估 | v3.x 演进 | gqep-executor 与 qeep-protocol 归属评估 |
| Superseded | 三环重组完成后 | 新 ADR 取代本 ADR |

---

## 8. 验收标准

- [ ] 本 ADR 文档完整(8 章节)
- [ ] 5 项决策明确(接受现状 + 不变量 + 复审触发 + 长期路径 + 文档同步)
- [ ] 3 项否决方案理由充分(A 类型下移 / B Event Bus / D 重新分层)
- [ ] 4 项不变量(INV-GQEP-1~4)可验证
- [ ] 4 项复审触发条件明确
- [ ] 文档同步清单完整(nuxus规则.md / CODE_WIKI.md / adr_index.md / deep-analysis)
- [ ] 时间表对齐三环重组 v3.x 演进方向

---

## 9. 参考

- `nuxus规则.md` §2.2 依赖铁律
- `nuxus规则.md` §3.4.5 4 类病理诊断
- `三环循环_十层接口_元架构重组深度分析.md` §3.4.5
- `AI_Agent_三重悖论_x_Chimera_深度映射分析.md` 推理悖论红线
- `docs/architecture/audit/deep-analysis-2026-07-27.md` §8.3 P1-4 调研结论
- ADR-033(L0 nexus-contracts 依赖铁律扩展)
- ADR-003(Event Bus 实现选型)
- ADR-021(MCP Mesh 2PC 占位实现)
- ADR-042(R2 GSOE×AutoDPO 约束 RL FormalVerifier 落地前无条件冻结)
- PiKV (arXiv:2508.06526) 分层 KV 缓存策略
- Claude Code 尸检:5.4% 孤儿调用教训

---

> **维护者**: NEXUS-OMEGA 团队
> **状态变更记录**:
> - 2026-07-27: Proposed(初始版本,Phase 8 P1-4 调研完成后落档)
> - 2026-08-01: Accepted(P1-1 状态推进,文档同步完成)

---

## 附录: P9-T6 合规审查记录(2026-08-04)

Phase 9 三环重组启动(ADR-054 落档)触发复审条件 1,按 ADR-048 决策 3 执行合规审查(详见 `_blueprints/three-ring-reorg/ADR-048_合规审查报告_v2.20.md`):

- **INV-GQEP-1~4 核验**: 全部满足(仅 qeep-protocol 一个 L4 依赖 / entangle 注册同步无 IO / 错误映射 5↔7 变体穷尽 / 孤儿检测覆盖含 OrphanCallDetected Critical 断言)
- **触发条件 1(三环重组启动)**: 已触发。归属评估: gqep-executor(L7)与 qeep-protocol(L4)均为外环,外环内部 L7→L4 向下依赖合法,不构成内环独立发布阻断;本例外**继续有效**,根治路径延续至三环重组完成后评估归属
- **触发条件 2(QEEP 接口扩展)**: 未触发
- **触发条件 3(entangle 延迟 >100μs)**: 未触发但待测量(benches/gather.rs 无 entangle 单独延迟数值,待 P9-T9 内环延迟基准 2026-08-15~09-15 补录)
- **触发条件 3 闭环(2026-08-04,P9-T9)**: entangle_alone 实测 **0.41 μs**(criterion,gather.rs bench),≤100μs 阈值且低于 ADR-048 <10μs 估计一个数量级 → **未触发,正式关闭**(详见 _blueprints/three-ring-reorg/内环延迟基线报告_v2.20.md)
- **触发条件 4(孤儿检测回归)**: 无回归(cargo test -p gqep-executor 64 passed / 0 failed / 1 ignored)
- **文档漂移更正**: ① §6.2 方法声明"限于 entangle"字面过窄,实际使用 QeepProtocol 7 个方法(均限于 qeep-protocol crate);② §6.2 声明测试文件 `tests/orphan_detection.rs`/`tests/error_mapping.rs` 实际不存在,实为 gatherer.rs 内嵌测试 + tests/{error_paths,concurrent,gatherer_test,proptest}.rs(测试覆盖实质存在,属文档漂移)
- **结论**: ADR-048 保持 Accepted,例外继续有效,根治路径延续
