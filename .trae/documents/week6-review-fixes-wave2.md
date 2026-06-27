# Week 6 复审修复 — Wave 2 剩余工作计划

> **本计划范围**:完成 Week 6 复审 8 个问题修复的收尾阶段(Wave 2 + Wave 3)
> **前置状态**:Wave 1 已全部完成(P3 / P4 / W7-1 / W7-2 / W7-3 / W7-4 共 6 个问题已修复并单 crate 验证通过)
> **执行原则**:长期主义、高质量文档、严格回归验证

---

## 1. 现状分析(Phase 1 探索结论)

### 1.1 Wave 1 已完成成果确认

| 编号 | 状态 | 关键证据 |
|------|------|---------|
| P3 (gsoe unwrap) | ✅ 已修复 | `gsoe-evolution/types.rs` 新增 `Default` impl,`engine.rs:48-51` 改用 `EvolutionPolicy::default()` |
| P4 (spec #[ignore]) | ✅ 已修复 | `spec.md:267` 已改为 "criterion 基准(cargo bench 运行)" |
| W7-1 (RoleRegistered EventBus) | ✅ 已修复 | `parliament/roles.rs` 已含 `with_event_bus` 构造器 + `event_bus: Option<EventBus>` 字段 + `register()` 通过 `publish_blocking` 发布事件 + 2 个单元测试(line 399-471) |
| W7-2 + W7-4 (E2E 事件流测试) | ✅ 已修复 | `tests/e2e/week5_event_flow.rs` 已创建(332 行,4 个 E2E 测试),根 `Cargo.toml` 已注册 `[[test]]` |
| W7-3 (qeep proptest) | ✅ 已修复 | `qeep-protocol/tests/proptest.rs` 已创建(181 行,6 个属性测试) |

### 1.2 剩余工作清单

| 编号 | 级别 | 任务 | 工作量 |
|------|------|------|--------|
| W7-5 | Minor | CHANGELOG Week 6 章节新增"复审修复"记录 | ~15 行 |
| W7-6 | Minor | week5 checklist 标注 RoleRegistered 修复状态 | ~2 行 |
| 最终验证 | Critical | workspace 全量回归(check + clippy + test) | 0 行(仅命令) |

### 1.3 关键发现(影响 W7-5 执行策略)

**CHANGELOG Week 5 "9 个事件"描述现状**:
- Line 200: `### 新增事件类型(9 个)` — 列表中包含 `RoleRegistered`
- Line 258: `event-bus | 1 | 9 个新事件类型 + severity/metadata 扩展`

**重要结论**:由于 W7-1 已修复完成,RoleRegistered 事件现在**确实会被发布**,"9 个事件"的描述变为**准确**,无需修正为 "8 个"。W7-5 的实际工作是在 Week 6 章节末尾新增"复审修复"小节,记录此次修复历程,而非修改 Week 5 章节的事件数量。

---

## 2. 执行步骤

### 2.1 W7-5: CHANGELOG 新增"Week 6 复审修复"小节

**目标**:在 CHANGELOG.md Week 6 章节末尾(line 130 "5. proptest 1.11.0 语法"之后,line 132 "## Week 5" 之前)新增"Week 6 复审修复"小节,记录 Wave 1 修复成果。

**修改文件**: `d:\Chimera CLI\CHANGELOG.md`

**插入位置**: Week 6 章节"关键经验教训"小节之后(line 130 之后),"## Week 5"标题(line 132)之前

**插入内容**:

```markdown

### Week 6 复审修复(2026-06-27)

针对 Week 6 三维度深度复审(架构 / 后端诊断 / 通用审计)发现的 8 个问题(2 个 Week 6 内部 + 6 个 Week 7 结转),组建精英专家子代理团队按优先级修复,全部 8 项已闭环。

#### 修复清单

| 编号 | 级别 | 问题 | 修复方式 |
|------|------|------|---------|
| P3 | Minor | `gsoe-evolution` engine.rs 业务代码 unwrap | 为 `EvolutionPolicy` 实现 `Default` trait,改用 `EvolutionPolicy::default()` |
| P4 | Cosmetic | spec §6.2 `#[ignore]` 标记描述与实现不符 | 修正为 "criterion 基准(cargo bench 运行)" |
| W7-1 | Major | `RoleRegistered` 事件未实际发布(仅 TODO + tracing) | `RoleRegistry` 新增 `with_event_bus` 构造器 + `event_bus: Option<EventBus>` 字段,`register()` 通过 `publish_blocking` 发布事件(与 SSRA/GSOE/DECB/LSCT/CHTC 模式一致) |
| W7-2 | Should | Week 5 E2E 事件流测试缺失 | 新增 `tests/e2e/week5_event_flow.rs`(4 个 E2E 测试:RoleRegistered / BudgetAdjusted / BudgetExceeded / DegradedModeRejected) |
| W7-3 | Should | `qeep-protocol` proptest 缺失 | 新增 `crates/qeep-protocol/tests/proptest.rs`(6 个属性测试,块状命名语法) |
| W7-4 | Should | DegradedModeRejected E2E 覆盖缺失 | 合并入 W7-2 的 E2E 测试文件 |
| W7-5 | Minor | CHANGELOG Week 5 "9 个事件"描述状态 | W7-1 完成后描述已准确,本小节记录修复历程 |
| W7-6 | Minor | week5 checklist RoleRegistered 勾选但未实现 | 标注"✅ 修复于 Week 6 复审(2026-06-27)" |

#### 关键设计决策

1. **`with_event_bus` 模式一致性**:RoleRegistry 采用与 SSRA/GSOE/DECB/LSCT/CHTC 相同的 EventBus 注入构造器模式,保留 `new()` 向后兼容(测试场景无 bus)
2. **`publish_blocking` 同步发布**:`register()` 为同步方法,使用 event-bus 官方同步 API `publish_blocking`(与 CMT/DECB 一致),事件发布失败仅 warn 不阻塞注册流程
3. **RoleRegistered 事件字段**:实际定义 4 个字段(`metadata` / `role_id` / `role_name` / `voting_weight`),修复时通过读取 `event-bus/types.rs:853-862` 补全 `voting_weight: f32`
4. **broadcast 时序约束**:`bus.subscribe()` 必须在 `publish()` 之前调用(broadcast 不缓存历史),单元测试中先订阅再注入 bus
5. **proptest 1.11.0 语法**:使用块状命名形式 `fn test_name(x in 0..100u32)` 避免闭包形式解析失败

#### 验证结果

- `cargo test -p parliament` ✓ 13 passed(含 2 个新增 RoleRegistered 测试)
- `cargo test -p gsoe-evolution` ✓ 81 passed
- `cargo test -p qeep-protocol --test proptest` ✓ 6 passed
- `cargo test --test week5_event_flow` ✓ 4 passed
- `cargo clippy` 各 crate 零警告

```

### 2.2 W7-6: week5 checklist 标注修复状态

**目标**:在 `.trae/specs/week5-parliament-security-budget/checklist.md` 的 line 17 标注 RoleRegistered 修复状态。

**修改文件**: `d:\Chimera CLI\.trae\specs\week5-parliament-security-budget\checklist.md`

**修改位置**: Line 17

**修改前**:
```markdown
- [x] 30.2 角色注册后发布 `RoleRegistered` 事件
```

**修改后**:
```markdown
- [x] 30.2 角色注册后发布 `RoleRegistered` 事件 <!-- ✅ 修复于 Week 6 复审(2026-06-27):parliament/roles.rs 新增 with_event_bus 构造器,register() 通过 publish_blocking 发布事件 -->
```

**决策依据**:保留 `[x]` 勾选状态(因为 W7-1 修复后该检查项确实已通过),仅通过 HTML 注释补充修复历史,既保持 checklist 简洁又留有审计追溯线索。HTML 注释在 Markdown 渲染时不可见,不影响阅读。

### 2.3 最终验证(Wave 3)

**目标**:workspace 全量回归,确保 8 个问题修复无任何副作用。

**环境准备**(PowerShell):
```powershell
$env:CARGO_TARGET_DIR = 'D:\Chimera CLI\target'
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
```

**验证命令**(顺序执行,前一步失败则终止):
```powershell
# Step 1: 类型检查(快速失败)
cargo check --workspace --jobs 1

# Step 2: Lint(零警告)
cargo clippy --workspace --all-targets -- -D warnings

# Step 3: 全量测试(回归)
cargo test --workspace --jobs 1
```

**预期结果**:
- `cargo check`: 0 errors
- `cargo clippy`: 0 warnings
- `cargo test`: 全部通过(Week 1-6 累计 ~2400+ 测试 + Wave 1 新增 12 测试)

---

## 3. 假设与决策

### 3.1 假设
- Wave 1 的 4 个子代理修复均已落盘(基于 Read 工具确认 parliament/roles.rs 状态)
- CHANGELOG.md Week 6 章节结构稳定(line 130-131 为 Week 6 末尾)
- week5 checklist.md line 17 内容如探索所示

### 3.2 关键决策
1. **W7-5 不修改 Week 5 章节**:由于 W7-1 已修复,"9 个事件"描述准确,仅需在 Week 6 章节新增修复记录小节
2. **W7-6 使用 HTML 注释**:保持 `[x]` 勾选 + HTML 注释补充修复历史,兼顾简洁与可审计
3. **最终验证用 `--jobs 1`**:遵循 project_memory 教训,避免内存压力导致编译失败
4. **不执行 `cargo build --release`**:Week 6 验收已通过 release 构建,本次仅文档级 + 测试级修改,无需重复

---

## 4. 验收标准

| 验收项 | 判定方法 |
|--------|---------|
| W7-5 | CHANGELOG.md Week 6 章节含"Week 6 复审修复"小节,8 个问题清单完整 |
| W7-6 | week5 checklist line 17 含"修复于 Week 6 复审"标注 |
| 最终验证 | cargo check / clippy / test --workspace 全部 0 errors |

---

## 5. 执行顺序

1. **Step 1**: 读取 CHANGELOG.md 确认 line 130-132 实际内容(防止文件已变更)
2. **Step 2**: 执行 W7-5 — 在 CHANGELOG.md Week 6 章节末尾插入"Week 6 复审修复"小节
3. **Step 3**: 读取 week5 checklist.md line 17 确认内容
4. **Step 4**: 执行 W7-6 — 修改 line 17 添加 HTML 注释
5. **Step 5**: 设置环境变量,执行最终验证三连(check → clippy → test)
6. **Step 6**: 向用户返回最终完成报告
