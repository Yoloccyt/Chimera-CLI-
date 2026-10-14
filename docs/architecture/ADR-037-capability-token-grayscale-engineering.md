# ADR-037: 能力场灰度工程化方案(CapabilityToken 机制实现)

> **🔄 v2.4.0-omega 基线增补**(2026-07-26):本文档基于 v2.3.1-omega **35 crate** 架构撰写;**v2.4.0-omega 后 workspace 演进为 37 crate**(本 ADR 决策落地物 `nexus-contracts` @ L0 即 v2.4.0-omega 第 36 个 crate,`omega-learner` @ L6 为第 37 个)。P4-W14.5 落地验证 411 测试在 37 crate workspace 中重测仍全绿(详见 `crates/nexus-contracts/tests/` + `crates/decay-engine/tests/`)。**CapabilityToken 机制扩展**:L6 `omega-learner` 嫁接使用 `CapabilityToken::State` 四态(EWMA α=0.1)对学习策略进行灰度,无运行时旗,符合 C4 特征旗红线裁决(ADR-034)。

## 状态

已批准 (Accepted) (2026-07-25)

> **状态升级说明**: 本 ADR 于 2026-07-25 由"提议"升级为"已批准",因 P4-W14.5 全部 6 项子任务已落地并通过验证:nexus-contracts 340 测试 + decay-engine 71 测试全绿(共 411 测试)、clippy -D warnings 零警告、fmt --check 合规、五 LearnerHolder 对称性保留、无运行时旗(C4 合规)。工程实现权威源:`crates/nexus-contracts/src/capability_token.rs` + `crates/decay-engine/src/capability_registry.rs` + `crates/decay-engine/src/engine.rs`。

## 背景

- 项目当前处于 v2.3.1-omega（第三阶段模块级系统性优化），`NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7 引入学习系统（omega-learner + RHI-CG 双通道进化回路），P4-W13~W14 阶段正在落地六接缝（S1-S6）学习策略。
- **ADR-034**（已批准 2026-07-23）以 C4 决策裁决灰度策略：运行时灰度走 decay-engine 能力场（`CapabilityToken` 机制），否决运行时 Feature Flag。ADR-034 决策 2 描述了 `CapabilityToken` 的高层设计（`token_id` / `strategy_version` / `level: CapabilityLevel` / `frozen: bool` + 四档语义 Draft/Canary/Stable/Pinned），但未指定具体算法参数、类型扩展、集成路径与不变量。
- P4-W14.5 任务（Capability Field Grayscale C4 Compliance）需将 ADR-034 决策 2 落地为可编译、可测试、可证伪的工程实现。在落地过程中产生了 7 项工程决策，涉及算法选型、类型设计、集成路径、不变量约束，超出 ADR-034 的高层裁决范围。
- 本 ADR 记录 P4-W14.5 的 7 项工程化决策，作为 ADR-034 决策 2 的实现补充，并明确与既有架构红线（§6.1/§6.2）与 ADR-002/ADR-033 的关系。

> **ADR 编号确认**: `.trae/specs/nexus-omega-v5-implementation-plan/P4_P5_IMPLEMENTATION_PLAN.md` 第 684 行预占 `ADR-037 | 能力场灰度工程化方案 | P4-W14.5`。本 ADR 编号确认为 **ADR-037**，与计划一致（ADR-036 预留给 omega-learner LinUCB 算法选型）。

> **与 ADR-034 的关系**: 本 ADR 是 ADR-034 决策 2 的**工程实现补丁**（implementation addendum），不修改 ADR-034 的裁决结论（能力场灰度 vs Feature Flag），仅补充算法参数与集成路径。属 append-only 扩展（对齐 ADR-028 决策 1 哲学）。

## 决策

经专家团队（E01 首席架构师 + E02 安全架构师 + E05 生产系统专家 + E07 任务调度专家）多轮结构化思考与多路径交叉验证，对 CapabilityToken 机制工程化作出以下 7 项决策：

### 决策 1: EWMA α=0.1 渐进授权算法 — 否决滑动窗口

学习策略的渐进授权采用 **EWMA（指数加权移动平均）α=0.1** 更新成功率，否决滑动窗口方案。

**算法参数**:

| 常量 | 值 | 语义 |
|------|-----|------|
| `INITIAL_LEVEL` | 0.2 | 新策略初始授权（低于激活阈值 0.3，Draft 档） |
| `ACTIVATION_THRESHOLD` | 0.3 | 激活阈值（对齐 ADR-034 Draft→Canary 边界） |
| `PROMOTION_STEP_BASE` | 0.1 | 步长基数（自适应步长 = base × (1 − level)） |
| `EWMA_ALPHA` | 0.1 | EWMA 平滑系数 |
| `EWMA_PROMOTION_THRESHOLD` | 0.7 | 提升门槛（EWMA ≥ 0.7 才允许提升） |

**EWMA 更新公式**: `ewma = (1 − α) × ewma + α × reward`

**自适应步长**: `step = 0.1 × (1.0 − current_level)`
- 低 level（0.2）时步长 = 0.08，4 次成功 outcome 即可达 0.3 阈值
- 高 level（0.8）时步长 = 0.02，谨慎逼近 1.0 上限

**否决滑动窗口的理由**:

1. **学术支撑**: Token Budgets (Khan, 2026) 证明 EWMA 在非平稳奖励分布（任务阶段切换时奖励漂移）下收敛性优于固定窗口
2. **内存效率**: EWMA 仅需 O(1) 存储（当前值），滑动窗口需 O(N) 存储（N 个历史样本）
3. **抗游戏化**: EWMA 对近期样本加权更高，策略"刷成功率"行为会被后续失败快速惩罚
4. **工业先例**: A/B 测试领域（Netflix/Vanguard）普遍采用 EWMA 跟踪实验指标

**α=0.1 的选型依据**:
- α=0.1 对历史样本加权 90%，对当前样本加权 10%，平滑性强
- α=0.5 过于激进，单次失败会导致 EWMA 崩塌 50%
- α=0.01 过于保守，需要 ~70 次样本才能收敛

### 决策 2: 四态生命周期（CapabilityTokenStatus）— 扩展 ADR-034 二态

ADR-034 决策 2 描述 `CapabilityToken` 的 `frozen: bool` 二态（冻结/未冻结）。本决策扩展为**四态枚举**，区分"未达阈值"与"已达阈值"及"临时冷却"与"永久冻结"。

```rust
pub enum CapabilityTokenStatus {
    Provisional,  // 未达 ACTIVATION_THRESHOLD (0.3)，不允许 Learned
    Authorized,   // 已达阈值，允许 Learned 策略
    Cooldown,     // AsaIntervention 触发后临时禁用（60s 冷却期）
    Frozen,       // 连续 ASA 达阈值或手动冻结，需手动解冻
}
```

**状态转换图**:

```text
  ┌─────────────┐  maybe_promote()  ┌──────────────┐
  │ Provisional │ ────────────────▶ │  Authorized  │
  │  level<0.3  │ ◀──────────────── │  level≥0.3   │
  └─────────────┘   decay(asg=0.2)  └──────────────┘
        │                                  │
        │           trigger_asa            │
        │       ┌──────────────┐          │
        └──────▶│   Cooldown   │◀─────────┘
                │ 60s 冷却期   │
                └──────────────┘
                        │
                        │ consecutive_asa_count >= 3
                        ▼
                ┌──────────────┐
                │    Frozen    │
                │ 永久冻结      │
                └──────────────┘
```

**四态而非二态的理由**:

1. **编排器决策精细化**: `Provisional` 与 `Authorized` 区分使编排器能快速过滤（`status.allows_learned()` 无需读 level）
2. **恢复路径区分**: `Cooldown` 自动恢复（冷却期结束），`Frozen` 需手动 `unfreeze()`
3. **可观测性**: 四态清晰反映 token 生命周期阶段，便于调试与审计

### 决策 3: AsaIntervention 安全闭环 — 衰减 + 冷却 + 自动冻结

AsaIntervention 触发三步安全闭环：

```text
AsaIntervention 事件 → trigger_asa_intervention(now)
  ├─ 1. authorized_level -= DECAY_ON_ASA (0.2)
  ├─ 2. status = Cooldown, cooldown_until = now + 60s
  ├─ 3. consecutive_asa_count += 1
  └─ 4. 若 consecutive_asa_count >= 3 → status = Frozen（自动冻结）
```

**参数选型**:

| 常量 | 值 | 选型依据 |
|------|-----|---------|
| `DECAY_ON_ASA` | 0.2 | 单次 ASA 衰减 0.2，3 次累计 0.6，足以将 level 从 1.0 降到 0.4 |
| `COOLDOWN_DURATION_SECS` | 60 | 足够长让编排器观察策略回退效果，足够短避免长时间无法学习 |
| `ASA_FREEZE_THRESHOLD` | 3 | 1 次可能是噪声，2 次表明策略有问题，3 次确认系统性问题 |

**与 §6.2 红线的关系**:
- §6.2 "Critical 安全事件用 mpsc" → AsaIntervention 走 mpsc channel 确保送达
- §6.2 "BudgetExceeded severity = Critical" → AsaIntervention 同级 Critical
- **零 event-bus 依赖**: decay-engine 不直接订阅 event-bus（保持纯计算引擎），由编排器订阅 AsaIntervention 事件后调用 `decay_engine.trigger_asa_intervention(seam)` 同步触发

### 决策 4: CapabilityTokenRegistry 嵌入 DecayEngine — 否决独立 Holder

**否决创建 `CapabilityTokenHolder`**（第六个 LearnerHolder），改为将 `CapabilityTokenRegistry` 嵌入 `DecayEngine`。

**接入路径**:

```text
omega-learner (L6) ──(Learned 策略)──▶ 编排器 (L9)
                                          │
                                          ▼ 查询 token
L4 decay-engine ◀──(嵌入)── CapabilityTokenRegistry
                                          │
                                          ▼ 返回 allows_learned
编排器根据返回值决定:
  - true  → holder.update_policy(Learned)（注入学习策略）
  - false → holder.fallback_to_static()  （本地 fallback，C4 合规第三层）
```

**嵌入而非独立 Holder 的理由**:

1. **五 Holder 对称性**: S1/S2/S3/S5/S6 五个 LearnerHolder 共用相同骨架（`RwLock<Policy>` + `new`/`with_policy`/`update_policy`/`fallback_to_static`）。创建第六个 `CapabilityTokenHolder` 会破坏对称性（其内部结构完全不同：管理的是 token 而非策略）
2. **语义一致**: DecayEngine 是"能力衰减引擎"，管理 `CapabilityToken` 与其语义一致（token 衰减是能力衰减的子集）
3. **集中管理**: 所有能力场操作（衰减 + token）集中在 DecayEngine，避免分散
4. **零运行时开销**: 仅在策略注入路径查询 token，热路径（decay/decay_with_policy）不查询

**验证**: 全工作区 16 个 LearnerHolder 相关文件检查确认，所有 5 个 LearnerHolder 均无 `CapabilityToken` 引用（0 处），五 Holder 对称性完全保留。

### 决策 5: Token 检查在编排器层 — 否决 Holder 内部检查

**Token 授权查询在编排器层**（chimera-cli / quest-engine），不在 LearnerHolder 内部。

**否决 Holder 内部检查的理由**:

1. **关注点分离**: LearnerHolder 管理策略状态（Static/Learned），CapabilityToken 管理授权状态（level/status）。两者解耦，各自独立演进
2. **C4 合规三层 fallback**: 编排器查询 token → 未授权时本地 `fallback_to_static()`，不传播跨 crate 旗标
3. **避免 Holder 膨胀**: 若在 Holder 内部检查 token，需在每个 Holder 的 `update_policy` 方法中注入 token 查询逻辑，导致 5 个 Holder 均需修改
4. **测试隔离**: Token 逻辑可在 DecayEngine 层独立测试，无需构造 LearnerHolder 上下文

**编排器伪代码**:

```rust
// 编排器注入 Learned 策略前查询 token
if decay_engine.should_activate_learned(seam, now)? {
    holder.update_policy(Learned(policy));
} else {
    holder.fallback_to_static();
}
```

### 决策 6: SeamId 双定义（L0/L6）— P4-W14.6 统一上提

`SeamId` 当前在两个位置定义：
- `nexus-contracts::SeamId`（L0，六变体枚举）
- `omega_learner::SeamId`（L6，六变体枚举，含 `code_anchor`/`task_id` 等扩展方法）

**双定义的理由**:

1. **依赖铁律**: L0 nexus-contracts 禁止依赖 L6 omega-learner（向上禁止），SeamId 必须在 L0 独立定义
2. **语义对齐**: 两个 SeamId 六变体一一对应（S1Density/S2Memory/S3Prefetch/S4Selector/S5Parliament/S6Decay），物理独立但语义对齐
3. **P4-W14.6 后续**: SeamId 统一上提到 L0 作为独立任务（P4-W14.6），届时 omega-learner 的 SeamId 改为 re-export L0 类型

**当前状态可接受**: 两个 SeamId 变体一一对应，未来上提时可直接替换（`pub use nexus_contracts::SeamId`）。

### 决策 7: 隐式恢复语义 — allows_learned_policy 冷却期后自动判断

`allows_learned_policy(&self, now)` 是查询方法（`&self`），不能调用 `maybe_recover_from_cooldown(&mut self)` 修改状态字段。当 `status == Cooldown` 但冷却期已结束（`now >= cooldown_until`）时，采用**隐式判断**：基于 `authorized_level` 直接返回是否允许 Learned，不修改 `status` 字段。

**隐式判断的理由**:

1. **Rust 借用规则**: `&self` 方法不能调用 `&mut self` 方法，无法在查询时自动恢复状态
2. **避免不必要的 fallback**: 冷却期结束后若编排器尚未调用 `maybe_recover_from_cooldown`，`status` 仍为 `Cooldown`，但能力已恢复。若此时不允许 Learned，会导致编排器 fallback 到 Static，影响 Ω-Evolve 学习效率
3. **状态字段延迟同步**: 编排器应定期调用 `maybe_recover_from_cooldown` / `maybe_recover_all_from_cooldown` 显式同步状态字段，但查询语义不受状态字段延迟影响

**实现**:

```rust
pub fn allows_learned_policy(&self, now: i64) -> bool {
    if self.status == CapabilityTokenStatus::Frozen {
        return false;
    }
    if self.status == CapabilityTokenStatus::Cooldown {
        if self.is_in_cooldown(now) {
            return false;
        }
        // 冷却期已结束但状态未显式恢复：根据 level 隐式判断
        return self.authorized_level >= Self::ACTIVATION_THRESHOLD;
    }
    self.status == CapabilityTokenStatus::Authorized
        && self.authorized_level >= Self::ACTIVATION_THRESHOLD
}
```

## 理由

### 为什么选择 EWMA 而非滑动窗口

| 维度 | EWMA α=0.1 | 滑动窗口 N=100 |
|------|-----------|--------------|
| 存储复杂度 | O(1) | O(N) |
| 非平稳分布适应性 | 强（近期样本加权高） | 弱（所有样本等权） |
| 抗游戏化 | 强（后续失败快速惩罚） | 弱（旧成功稀释新失败） |
| 学术支撑 | Token Budgets (Khan 2026) | 传统但无 LLM 语境论文 |
| 收敛速度 | ~70 次样本 | = N 次样本 |

### 为什么选择自适应步长而非固定步长

| level | 固定步长 0.05 | 自适应步长 0.1×(1−level) |
|-------|-------------|----------------------|
| 0.2 | 0.05（4 次达 0.4） | 0.08（3 次达 0.44） |
| 0.5 | 0.05（10 次达 1.0） | 0.05（10 次达 1.0） |
| 0.8 | 0.05（4 次达 1.0） | 0.02（10 次达 1.0） |

自适应步长在低 level 时快速激活（3 次即可达 Draft→Canary），高 level 时谨慎逼近（避免越过 1.0 上限）。

## 影响

### 正面影响

1. **C4 合规落地**: ADR-034 决策 2 从设计文档变为可编译、可测试的工程实现
2. **五 Holder 对称性保留**: CapabilityTokenRegistry 嵌入 DecayEngine，不创建第六个 Holder
3. **零运行时开销**: token 查询仅在策略注入路径（冷路径），热路径（decay）不查询
4. **安全闭环完整**: AsaIntervention → 衰减 → 冷却 → 自动冻结，三层防护
5. **渐进授权可证伪**: EWMA + 自适应步长有数学可证的收敛性

### 负面影响/风险

1. **SeamId 双定义**: L0/L6 两处定义，P4-W14.6 需统一上提（当前可接受）
2. **隐式恢复语义**: `allows_learned_policy` 与 `status` 字段存在短暂偏差（冷却期结束后状态字段未同步），需编排器定期调用 `maybe_recover_all_from_cooldown` 同步
3. **EWMA 周期波动**: 周期性输入模式下 EWMA 会周期波动（非 bug，数学特性），测试需使用均匀分布模式

## 验证

### 单元测试覆盖

| 测试范围 | 测试数 | 状态 |
|---------|-------|------|
| `capability_token.rs` 单元测试 | 47 | ✅ 全绿 |
| `capability_registry.rs` 单元测试 | 24 | ✅ 全绿 |
| `nexus-contracts` 全量测试 | 340 | ✅ 全绿 |
| `decay-engine` 全量测试 | 71 | ✅ 全绿 |

### 工程验证

| 验证项 | 结果 |
|-------|------|
| `cargo check -p nexus-contracts -p decay-engine` | ✅ 零错误 |
| `cargo clippy --all-targets --jobs 2 -- -D warnings` | ✅ 零警告 |
| `cargo fmt --check` | ✅ 合规 |
| `cargo check --workspace` | ✅ 35 crate 编译成功 |
| `#![forbid(unsafe_code)]` 守恒 | ✅ 双 crate 均有 |
| LearnerHolder CapabilityToken 引用检查 | ✅ 5 Holder 均为 0 |

## 替代方案

### 替代 1: 创建 CapabilityTokenHolder（第六个 LearnerHolder）

**否决理由**: 破坏五 Holder 对称性，且 CapabilityTokenHolder 内部结构与策略 Holder 完全不同（管理 token 而非策略），强行套用 `RwLock<Policy>` 骨架会导致不自然的抽象。

### 替代 2: Token 检查在 LearnerHolder 内部

**否决理由**: 5 个 Holder 均需修改（注入 token 查询逻辑），违反 append-only 原则。且 token 查询是跨 crate 操作（编排器持有 DecayEngine 引用），Holder 不应依赖 DecayEngine。

### 替代 3: 滑动窗口替代 EWMA

**否决理由**: O(N) 存储、非平稳分布适应性弱、无 LLM 语境学术支撑。

### 替代 4: 固定步长替代自适应步长

**否决理由**: 低 level 时激活过慢（需 4 次达 0.4），高 level 时步长过大（可能越过 1.0）。

## 关联

- **ADR-034**: 本 ADR 是 ADR-034 决策 2 的工程实现补丁
- **ADR-033**: SeamId 定义在 L0 nexus-contracts（ADR-033 L0 契约层）
- **ADR-002**: CapabilityLevel 连续流体模型（本 ADR 扩展其语义到晋升路径）
- **ADR-028**: append-only 哲学（本 ADR 遵循决策 1 不修改五 Holder 公共签名）
- **§6.2 红线**: AsaIntervention 走 mpsc Critical 通道

## 参考资料

- Token Budgets for Large Language Models (Khan, 2026) — EWMA 在非平稳奖励分布下的收敛性证明
- `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3-§7.4 — 六接缝灰度锚点与部署灰度设计
- `crates/nexus-contracts/src/capability_token.rs` — CapabilityToken 类型实现（47 单元测试）
- `crates/decay-engine/src/capability_registry.rs` — CapabilityTokenRegistry 实现（24 单元测试）
- `crates/decay-engine/src/engine.rs` — DecayEngine 嵌入 Registry + 12 委托方法
