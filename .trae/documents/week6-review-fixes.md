# Week 6 复审问题修复计划

> **计划范围**:修复 Week 6 三维度深度复审发现的 8 个问题(2 个 Week 6 内部 + 6 个 Week 7 结转)
> **执行原则**:长期主义、TDD-first、高质量代码(清晰逻辑/高可读性/完善注释/行业最佳实践)
> **团队授权**:可调用所有系统允许的工具资源(MCP/skills/cargo 等)

---

## 1. 现状分析

### 1.1 问题清单(8 项)

| 编号 | 级别 | 问题 | 位置 | 修复工作量 |
|------|------|------|------|-----------|
| P3 | Minor | gsoe-evolution engine.rs:50 业务代码 unwrap | `crates/gsoe-evolution/src/engine.rs:50` | 1 行(+Default impl) |
| P4 | Cosmetic | spec §6.2 提到 `#[ignore]` 标记但实际无 | `.trae/specs/week6-adaptation-evolution-multimodal/spec.md:267` | 1 行 |
| W7-1 | Major | RoleRegistered 事件未实际发布(仅 TODO + tracing) | `crates/parliament/src/roles.rs:124` | ~30 行 |
| W7-2 | Should | Week 5 E2E 事件流测试缺失 | `tests/e2e/`(新建) | ~80 行 |
| W7-3 | Should | qeep-protocol proptest 缺失 | `crates/qeep-protocol/tests/proptest.rs`(新建) | ~60 行 |
| W7-4 | Should | DegradedModeRejected E2E 覆盖缺失 | `tests/e2e/`(可合并 W7-2) | ~30 行 |
| W7-5 | Minor | CHANGELOG Week 5 "9 个事件"描述不准确(实际 8/9) | `CHANGELOG.md` Week 5 章节 | 1-2 行 |
| W7-6 | Minor | week5 checklist 30.2 RoleRegistered 勾选但未实现 | `.trae/specs/week5-parliament-security-budget/checklist.md` | 1 行 |

### 1.2 关键代码现状(Phase 1 探索结论)

- **parliament 已依赖 event-bus**(`crates/parliament/Cargo.toml:10`),修复 RoleRegistered 无需新增依赖
- **RoleRegistry 结构简单**:仅 `registry: RwLock<HashMap<RoleId, RoleProfile>>` 字段,需添加 `event_bus: Option<EventBus>`
- **EvolutionPolicy 无 Default 实现**(`crates/gsoe-evolution/src/types.rs:34-68`),需新增
- **qeep-protocol/tests/qeep.rs** 已有单元测试,proptest 为新增独立文件
- **RoleRegistered 事件类型已定义**(`event-bus/types.rs:853`),三处 match 分支已覆盖

---

## 2. 执行优先级与依赖关系

### 2.1 优先级排序(MoSCoW + 依赖)

```
Must(阻塞):
  1. P3  gsoe unwrap 修复(独立,最简单,先热身)
  2. W7-1 RoleRegistered EventBus 集成(核心债务)
  3. W7-5 CHANGELOG 修正(依赖 W7-1 完成)
  4. W7-6 week5 checklist 状态修正(依赖 W7-1 完成)

Should(本周完成):
  5. P4  spec #[ignore] 描述修正(独立)
  6. W7-3 qeep-protocol proptest(独立)
  7. W7-2 + W7-4 E2E 事件流测试(合并为一个文件,独立)
```

### 2.2 任务依赖图

```
P3 ────────────────────────────────────────┐
P4 ────────────────────────────────────────┤
W7-3 ──────────────────────────────────────┤──→ 最终验证
W7-2+W7-4 ────────────────────────────────┤
W7-1 ──→ W7-5 ──→ W7-6 ───────────────────┘
```

**并行策略**:P3 / P4 / W7-1 / W7-3 / W7-2+W7-4 可并行执行(5 个独立工作流);W7-5 / W7-6 串行依赖 W7-1。

---

## 3. 具体修复方案

### 3.1 P3: gsoe-evolution engine.rs:50 unwrap 修复

**目标**:消除业务代码中的 `unwrap()`,符合 spec §6.3 "非测试代码 unwrap/expect = 0"

**修改文件**:
1. `crates/gsoe-evolution/src/types.rs` — 为 EvolutionPolicy 实现 Default trait
2. `crates/gsoe-evolution/src/engine.rs:48-51` — 改用 `EvolutionPolicy::default()`

**修改内容**:

文件 1: `crates/gsoe-evolution/src/types.rs`(在 impl EvolutionPolicy 块后新增)
```rust
impl Default for EvolutionPolicy {
    /// 默认进化策略:保守变异 + 适度选择压力 + 20% 精英 + 8 轮采样
    ///
    /// WHY 这些参数:0.1 变异率避免过度震荡,1.5 选择压力放大优势差异,
    /// 0.2 精英比例保留最优个体,8 轮采样满足 GRPO 组内优势统计需求。
    fn default() -> Self {
        Self {
            mutation_rate: 0.1,
            selection_pressure: 1.5,
            elite_ratio: 0.2,
            rollout_count: 8,
        }
    }
}
```

文件 2: `crates/gsoe-evolution/src/engine.rs:48-51`
```rust
// 修改前:
let policy = config.to_initial_policy().unwrap_or_else(|_| {
    // 配置非法时回退到 Default(防御性:配置应已在外部校验)
    EvolutionPolicy::new(0.1, 1.5, 0.2, 8).unwrap()
});

// 修改后:
let policy = config.to_initial_policy().unwrap_or_else(|_| {
    // 配置非法时回退到 Default(防御性:配置应已在外部校验)
    EvolutionPolicy::default()
});
```

**验证**:`cargo test -p gsoe-evolution` + `cargo clippy -p gsoe-evolution -- -D warnings`

---

### 3.2 W7-1: RoleRegistered 事件 EventBus 集成(核心)

**目标**:完成 Week 5 遗留的 RoleRegistered 事件发布,遵循 SSRA/GSOE 的 `with_event_bus` 模式

**修改文件**:
1. `crates/parliament/src/roles.rs` — RoleRegistry 结构扩展 + register() 发布事件

**修改内容**:

文件: `crates/parliament/src/roles.rs`

**(a) 结构体添加 event_bus 字段**(line 30-33):
```rust
pub struct RoleRegistry {
    /// 角色注册表(读多写少,用 RwLock)
    registry: RwLock<HashMap<RoleId, RoleProfile>>,
    /// 可选的 EventBus 连接(用于发布 RoleRegistered 事件)
    /// WHY Option:保留 new() 向后兼容(测试场景无 bus),生产场景用 with_event_bus 注入
    event_bus: Option<EventBus>,
}
```

**(b) 新增 with_event_bus 构造器**(在 new() 后):
```rust
/// 构造角色注册表并注入 EventBus(生产场景使用)
///
/// 与 new() 的区别:持有 EventBus 引用,register() 成功后发布 RoleRegistered 事件。
/// WHY 与 SSRA/GSOE 的 with_event_bus 模式一致:保持构造器 API 一致性。
pub fn with_event_bus(config: &ParliamentConfig, bus: EventBus) -> Self {
    let mut registry = Self::new(config);
    registry.event_bus = Some(bus);
    registry
}
```

**(c) new() 初始化 event_bus 为 None**(line 91-93):
```rust
Self {
    registry: RwLock::new(registry),
    event_bus: None,
}
```

**(d) register() 发布事件**(line 122-128,替换 TODO):
```rust
registry.insert(role_id.clone(), profile);

// 发布 RoleRegistered 事件(若有 EventBus 连接)
// WHY publish_blocking:register() 是同步方法,不持有 async runtime;
// publish_blocking 是 event-bus 官方同步 API(bus.rs:128),与 CMT/DECB 同步发布模式一致。
if let Some(ref bus) = self.event_bus {
    let event = NexusEvent::RoleRegistered {
        metadata: EventMetadata::new("parliament"),
        role_id: role_id.clone(),
        role_name,
    };
    if let Err(e) = bus.publish_blocking(event) {
        // 事件发布失败不阻塞注册流程,仅记录警告
        tracing::warn!(error = %e, role_id = %role_id, "RoleRegistered 事件发布失败");
    }
} else {
    // 无 EventBus 时保留 tracing 日志(测试场景)
    info!(role_id = %role_id, role = %role_name, "角色注册完成 (RoleRegistered, 无 EventBus)");
}
```

**(e) 添加 import**(文件顶部):
```rust
use event_bus::{EventBus, EventMetadata, NexusEvent};
```

**(f) 更新模块文档注释**(line 10-11,移除 TODO):
```rust
//! - 角色注册后发布 `RoleRegistered` 事件(通过 EventBus);
//!   无 EventBus 连接时回退到 tracing 日志(测试场景)
```

**验证**:
- `cargo test -p parliament` — 确保无回归
- 新增 1-2 个单元测试验证 with_event_bus + register 发布事件
- `cargo clippy -p parliament -- -D warnings`

---

### 3.3 W7-5: CHANGELOG Week 5 "9 个事件"修正

**目标**:修正 CHANGELOG Week 5 章节的事件数量描述

**修改文件**: `CHANGELOG.md`(Week 5 章节,需定位 "9 个事件" 描述)

**修改内容**:W7-1 完成后,RoleRegistered 已实际发布,9 个事件的描述变为准确。因此:
- 若 W7-1 已完成:保留 "9 个事件" 描述,在 Week 6 修复记录中补充 "RoleRegistered 事件发布已修复"
- 在 Week 6 章节末尾或新增 "Week 6 复审修复" 小节,记录此次修复

**验证**:读取 CHANGELOG 确认描述准确

---

### 3.4 W7-6: week5 checklist 状态修正

**目标**:修正 week5-parliament-security-budget/checklist.md 中 RoleRegistered 检查项状态

**修改文件**: `.trae/specs/week5-parliament-security-budget/checklist.md`(需定位 RoleRegistered 相关项)

**修改内容**:W7-1 完成后,将相关检查项标注为 "✅ 修复于 Week 6 复审(2026-06-27)"

**验证**:读取 checklist 确认状态一致

---

### 3.5 P4: spec §6.2 #[ignore] 描述修正

**目标**:消除 spec 描述与实现的不一致

**修改文件**: `.trae/specs/week6-adaptation-evolution-multimodal/spec.md:267`

**修改内容**:
```markdown
<!-- 修改前 -->
| SSRA 融合延迟(p95) | ≤ 20ms | criterion 基准 + #[ignore] 标记 |

<!-- 修改后 -->
| SSRA 融合延迟(p95) | ≤ 20ms | criterion 基准(cargo bench 运行) |
```

**验证**:读取 spec.md:267 确认

---

### 3.6 W7-3: qeep-protocol proptest 新增

**目标**:为 qeep-protocol 添加属性测试,验证核心不变量

**新建文件**: `crates/qeep-protocol/tests/proptest.rs`

**内容要点**:
- 使用 proptest 1.11.0 块状命名语法(避免闭包形式兼容性问题,参考 project_memory 教训)
- 不变量:
  1. 超时值 > 0 时,entangle 正常操作应成功
  2. 孤儿检测器在 entangle 被 drop 后应报告孤儿
  3. pending_count + completed_count + orphan_count ≤ 总操作数
  4. EntangledCallId 唯一性

**验证**:`cargo test -p qeep-protocol --test proptest`

---

### 3.7 W7-2 + W7-4: E2E 事件流测试(合并)

**目标**:新增 E2E 测试验证 Week 5 事件发布链路 + DegradedModeRejected 错误路径

**新建文件**: `tests/e2e/week5_event_flow.rs`

**Cargo.toml 修改**: 在根 `Cargo.toml` 添加 `[[test]]` 注册:
```toml
[[test]]
name = "week5_event_flow"
path = "tests/e2e/week5_event_flow.rs"
```

**dev-dependencies 补充**(若需): `decb-governor` / `parliament` 已在 Cargo.toml 中

**测试用例**:
1. `test_role_registered_event_flow` — RoleRegistry::with_event_bus → register() → 订阅接收 RoleRegistered 事件
2. `test_consensus_reached_event_flow` — Parliament 审议 → ConsensusReached 事件发布
3. `test_red_team_audit_event_flow` — AHIRT 审计 → RedTeamAudit 事件发布
4. `test_degraded_mode_rejected_e2e` — DECB 预算超限 + Degraded 模式 → DegradedModeRejected 错误(Week 7-4)
5. `test_budget_adjusted_event_flow` — DECB 预算调整 → BudgetAdjusted 事件发布

**验证**:`cargo test --test week5_event_flow`

---

## 4. 团队分工(并行执行策略)

### 4.1 专家子代理分配

| 专家 | 子代理类型 | 负责任务 | 预计工作量 |
|------|-----------|---------|-----------|
| Lead Architect | rust-architecture-expert | W7-1 RoleRegistered 集成(核心) | ~30 行修改 |
| GSOE Specialist | general_purpose_task | P3 gsoe unwrap 修复 + P4 spec 描述 | ~15 行修改 |
| QEEP Specialist | general_purpose_task | W7-3 qeep proptest 新增 | ~60 行新增 |
| QA/E2E Specialist | general_purpose_task | W7-2 + W7-4 E2E 事件流测试 | ~110 行新增 |
| Doc Specialist | general_purpose_task | W7-5 CHANGELOG + W7-6 checklist(串行依赖 W7-1) | ~5 行修改 |

### 4.2 并行执行波次

**Wave 1(并行,5 个子代理)**:
- Lead Architect: W7-1 RoleRegistered
- GSOE Specialist: P3 + P4
- QEEP Specialist: W7-3
- QA/E2E Specialist: W7-2 + W7-4
- Doc Specialist: 等待 W7-1

**Wave 2(W7-1 完成后)**:
- Doc Specialist: W7-5 + W7-6

**Wave 3(全部完成后)**:
- 最终验证:`cargo check --workspace && cargo clippy --workspace -- -D warnings && cargo test --workspace`

---

## 5. 假设与决策

### 5.1 假设
- parliament crate 的 event-bus 依赖已稳定(Cargo.toml:10 确认)
- RoleRegistered 事件类型定义完整(event-bus/types.rs:853 确认,三处 match 分支已覆盖)
- tests/e2e/ 目录已存在(Week 6 E2E 测试已在此目录)
- 根 Cargo.toml 已配置 [package] + [dev-dependencies](Week 6 已配置)

### 5.2 关键决策
1. **EvolutionPolicy::default() 参数**:使用 engine.rs:50 原始参数(0.1/1.5/0.2/8),保证行为不变
2. **RoleRegistry event_bus 字段**:使用 `Option<EventBus>` 而非直接持有,保持 new() 向后兼容(与 SSRA/GSOE 模式一致)
3. **register() 事件发布**:使用 `publish_blocking`(同步方法,与 CMT/DECB 模式一致),发布失败仅 warn 不阻塞
4. **W7-2 + W7-4 合并**:DegradedModeRejected E2E 与事件流测试合并为一个文件,减少文件碎片
5. **proptest 语法**:使用块状命名形式(`fn test_name(x in 0..100u32)`),避免 proptest 1.11.0 闭包兼容性问题

---

## 6. 验证步骤

### 6.1 单 crate 验证(每个任务完成后)
```powershell
# P3 验证
cargo test -p gsoe-evolution
cargo clippy -p gsoe-evolution -- -D warnings

# W7-1 验证
cargo test -p parliament
cargo clippy -p parliament -- -D warnings

# W7-3 验证
cargo test -p qeep-protocol --test proptest

# W7-2 + W7-4 验证
cargo test --test week5_event_flow
```

### 6.2 全量回归验证(全部完成后)
```powershell
# 环境变量设置(磁盘空间缓解)
$env:CARGO_TARGET_DIR = 'D:\Chimera CLI\target'
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"

cargo check --workspace --jobs 1
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --jobs 1
```

### 6.3 文档验证
- 读取 `CHANGELOG.md` 确认 Week 5 事件描述准确
- 读取 `.trae/specs/week5-parliament-security-budget/checklist.md` 确认状态一致
- 读取 `spec.md:267` 确认 #[ignore] 描述已移除

---

## 7. 验收标准

| 编号 | 验收项 | 判定方法 |
|------|--------|---------|
| P3 | gsoe-evolution 业务代码 unwrap = 0 | Grep 搜索 src/ 排除 #[cfg(test)] |
| P4 | spec.md:267 无 "#[ignore]" | Grep 搜索 |
| W7-1 | register() 发布 RoleRegistered 事件 | 单元测试 + 代码审查 |
| W7-2 | E2E 事件流测试 ≥ 4 个用例 | cargo test --test week5_event_flow |
| W7-3 | qeep proptest ≥ 3 个属性 | cargo test --test proptest |
| W7-4 | DegradedModeRejected E2E 覆盖 | E2E 测试含相关用例 |
| W7-5 | CHANGELOG Week 5 事件描述准确 | 读取确认 |
| W7-6 | week5 checklist 状态一致 | 读取确认 |
| 全局 | cargo check/clippy/test --workspace 全通过 | 命令输出 0 errors |
