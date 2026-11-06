# ADR-243 -- R2/R1 Critical 事件生产可达性判定（Wave B 错误→事件转换层的优先级裁决）

- 状态：Accepted（2026-11-06）
- 决策者：Spec Task A3（安全结论判定批）
- 关联：ADR-042（R2 冻结）、ADR-043（R1 影子模式回滚决策）、ADR-159（审计遗留治理）、ADR-193（架构减法批次 A：物理删除 `auto-dpo`）、ADR-053（ADR-053-rev4 影子模式治理）、ADR-224（ADR-225 阈值双源）
- 影响面：本文件（判定与落档）；`docs/architecture/adr_index.md`（新增一行索引）
- **本 ADR 只做判定与落档，不实现转换层、不改任何 Rust 生产代码、不新增 crate。**

## 1. 背景与问题

`crates/event-bus/src/types.rs` 中有四个 NexusEvent 变体既已定义、已进 `crates/event-bus/src/bus.rs::is_critical_mpsc_event()` 的 Critical mpsc 旁路清单、severity 也定级为 Critical，却**在生产路径没有任何发布点**：

| 变体 | 事件定义 | severity 定级 | mpsc 旁路 |
|------|---------|--------------|----------|
| `R2FreezeViolation` | `crates/event-bus/src/types.rs:2045` | `crates/event-bus/src/registry.rs` 注册表 | `crates/event-bus/src/bus.rs:166` |
| `R2FreezeRollbackFailed` | `crates/event-bus/src/types.rs:2060` | `crates/event-bus/src/registry.rs:299`（Critical, Knowledge） | `crates/event-bus/src/bus.rs:167` |
| `R1ShadowRollbackFailed` | `crates/event-bus/src/types.rs:1971` | `crates/event-bus/src/registry.rs` 注册表 | `crates/event-bus/src/bus.rs:177` |
| `FailureRegistered` | `crates/event-bus/src/types.rs:89` | `crates/event-bus/src/registry.rs:375`（Critical, Security） | `crates/event-bus/src/bus.rs:190` |

对应的错误类型住在 `crates/chimera-mas/src/error.rs`（`MasError::R2FreezeViolation` / `MasError::R1ShadowRollbackFailed`）。该文件里 `EventBus` 的两处命中（`crates/chimera-mas/src/error.rs:290`、`crates/chimera-mas/src/error.rs:380`）**均为文档注释**，无代码级引用 —— 即错误发生时不发布 Critical 事件。

**本次要判定的核心问题**：这些错误（尤其 R2 冻结违规、R1 影子回滚失败）在**默认生产路径**（默认 cargo feature 集合、无 `--features` 特殊开关）上**是否可达**？

- 若**可达** ⇒ 这是「正在发生的安全缺口」，Wave B（错误→Critical 事件转换层）优先级立即升为 P0。
- 若**不可达**（仅在某个 feature 门控下可达）⇒ 属于「潜在缺口」，Wave B 保持既定优先级。

## 2. 判定结论

> ## ✅ 结论：**默认生产路径不可达（仅 feature 门控下可达）**
>
> 更强的表述：**四个变体在当前工作树中，生产路径零发布点 —— 连「feature 门控下可达」这一档都不成立。**
>
> - `MasError::R2FreezeViolation` / `MasError::R1ShadowRollbackFailed` 的**每一个构造点都在 `#[cfg(test)]` 内**（`crates/chimera-mas/src/error.rs:911` / `:917` / `:940` / `:982`，测试模块自 `:659-661` 起）。
> - `R2FreezeRollbackFailed` 与 `FailureRegistered` 的**历史生产发布方已在 ADR-193 批次 A 被物理删除**（`crates/auto-dpo/` 目录不存在，`git ls-files crates/auto-dpo/*` 为空），现存的同名文件只存在于 `.tower/worktrees/wt-15|16|17` 这三个**过期 worktree** 中，不在 workspace、不是生产代码。
> - `r2_path` feature **默认关闭**（`crates/gsoe-evolution/Cargo.toml:12-13`：`default = []` / `r2_path = []`），且**全仓无任何 crate 启用它**；唯一的门控点 `crates/gsoe-evolution/src/engine.rs:129-134` 用的是 `if cfg!(feature = "r2_path") { panic!(...) }` 形式（注释 `:127-128` 自陈：不用 `debug_assert!` 是因 `cfg!` 返回编译期常量会触发 `clippy::assertions_on_constants`），**且 panic 而非构造 `MasError`、更不发布事件**。
> - `chimera-mas` crate **根本没有 `[features]` 段**（`crates/chimera-mas/Cargo.toml` 全文无 `[features]`），因此 `MasError` 侧不存在任何 feature 门控。
>
> ⇒ **这是「潜在缺口」，不是「正在发生的安全缺口」。Wave B（错误→Critical 事件转换层）保持既定优先级，不升 P0。**

## 3. 证据

### 3.1 错误产生点（`MasError` 变体的每一个构造点）

| 构造点 | 位置 | 上下文 | 是否生产可达 |
|--------|------|--------|-------------|
| `MasError::R1ShadowRollbackFailed { .. }` | `crates/chimera-mas/src/error.rs:911` | `#[cfg(test)] mod tests`（`:659-661` 起，模块至 `:997` 止）内的变体计数测试 | ❌ 仅测试 |
| `MasError::R2FreezeViolation { .. }` | `crates/chimera-mas/src/error.rs:917` | 同上 | ❌ 仅测试 |
| `MasError::R1ShadowRollbackFailed { .. }` | `crates/chimera-mas/src/error.rs:940` | `#[cfg(test)]` Display 断言测试 | ❌ 仅测试 |
| `MasError::R1ShadowRollbackFailed { .. }` | `crates/chimera-mas/src/error.rs:982` | `#[cfg(test)]` trigger_type 遍历测试 | ❌ 仅测试 |
| `MasError::R2FreezeViolation { .. }` | `tests/e2e/r2_freeze_violation_e2e.rs:408` / `:442` / `:446` / `:450` | 根 package `[[test]]` target（`Cargo.toml:453-455` 注册） | ❌ 仅测试 |
| `MasError::R2FreezeViolation { .. }` | `Cargo.toml:448`、`:711` | 注释与 dev-dependency 声明 | ❌ 非代码 |

生产代码中 `MasError::R2FreezeViolation` / `MasError::R1ShadowRollbackFailed` 的**全部命中**只有三处，且均非构造：
- `crates/chimera-mas/src/error.rs:646-647` —— `is_recoverable()` 的 match 臂（只读判定）；
- `crates/chimera-mas/src/error.rs:484` / `:489` / `:526` —— 文档注释（`EventBus` 两处命中中 `:290`/`:380` 亦为文档注释）。

### 3.2 `R2FreezeRollbackFailed` / `FailureRegistered`：历史生产发布方已被物理删除

- `R2FreezeRollbackFailed` 的**唯一历史生产发布方**是 `crates/auto-dpo/src/freeze_guard.rs::attempt_rollback_with_guard()`（`bus.publish_critical_blocking(event)`，见该文件 `:129-149`）。该 crate 已由 **ADR-193 批次 A 物理删除**：
  - `Test-Path crates\auto-dpo` → `False`；
  - `git ls-files "crates/auto-dpo/*"` → 空；
  - 根 `Cargo.toml` 的 `workspace.members`（`:27-` 起）已无 `crates/auto-dpo`；
  - 现存同名文件只在 `.tower/worktrees/wt-15|16|17/crates/auto-dpo/src/freeze_guard.rs`（过期 worktree，不在 workspace、非生产代码）。
  - ADR-159 记的「B1 freeze_guard 双向接线 R2FreezeRollbackFailed」是 v2.28.1 在途批次的登记，**其载体 crate 随后被架构减法删除**，该接线在当前树已不存在。
- `FailureRegistered` 的生产侧只有**契约与数据结构**，没有事件发布点：`crates/nexus-contracts-safety/src/safety/calibration.rs:124`（`from_failure`）、`:136`（`from_record`）、`crates/nexus-contracts-safety/src/safety/adjudication.rs:131`（`FailureRecord`）均为 L0 纯类型/纯逻辑，**不依赖 event-bus、不发布事件**。
- `crates/event-bus/src/bus.rs:2152`（`FailureRegistered`）与 `:2199` / `:2204` / `:2223`（`R2FreezeViolation` / `R2FreezeRollbackFailed` / `R1ShadowRollbackFailed`）位于 `#[cfg(test)] pub(crate) mod tests_helpers`（`:2131-2133` 起）内，注释 `:2130` 自陈「仅测试构建存在(cfg(test)),生产零足迹」；`:2559` / `:2564` / `:2585` 位于 `#[cfg(test)] mod tests`（`:2302-2304` 起）内。
- 生产代码中另有两类**非发布点**命中，登记以免误读：
  - `crates/event-bus/src/bus.rs:166` / `:167` / `:177` / `:190` —— `is_critical_mpsc_event()` 的 match 臂（判定「该变体是否走 mpsc 旁路」），不是发布；
  - `crates/event-bus/src/membrane.rs:356-358` —— `EventCategory::Critical` 的 match 臂（分类判定），不是发布；
  - `crates/gsoe-evolution/src/engine.rs:24` 与 `crates/omega-learner/src/replay_pool.rs:42` —— 模块级文档注释（描述「若违反则应发布」），不是发布。

### 3.3 feature 门控：默认关闭且无任何 crate 启用

| 项 | 证据 | 判定 |
|----|------|------|
| `r2_path` feature 声明 | `crates/gsoe-evolution/Cargo.toml:11-13`（`[features]` / `default = []` / `r2_path = []`） | 默认**关闭** |
| 门控点实际形态 | `crates/gsoe-evolution/src/engine.rs:129-134`：`#[cfg(debug_assertions)] if cfg!(feature = "r2_path") { panic!(...) }` | panic，**不构造 `MasError`、不发布事件**；且仅 debug build 生效 |
| 门控形态的文档一致性 | `crates/chimera-mas/src/error.rs:522`、`crates/event-bus/src/types.rs:2043` 写的是 `debug_assert!(!cfg!(feature = "r2_path"))`；`crates/gsoe-evolution/src/engine.rs:127-128` 自陈改用 `if + panic!` 的原因（`clippy::assertions_on_constants`） | 文档措辞与实现不同形，但**语义等价**（都只在 feature 开启时触发） |
| `chimera-mas` 自身 feature | `crates/chimera-mas/Cargo.toml` 全文**无 `[features]` 段** | `MasError` 侧**零 feature 门控** |
| workspace 根 features | `Cargo.toml:23-24`（`[features]` / `mca = []`） | 不含 `r2_path` |
| `chimera-cli` features | `crates/chimera-cli/Cargo.toml:9-25`（`rl-client` / `mca` / `r2_unfreeze` / `r2_island_repayment`） | **不含** `r2_path`；`r2_unfreeze` 只拉 `decay-engine` + `omega-learner` |
| `r2_path` 的全仓启用面 | `git grep -n "r2_path" -- "*/Cargo.toml" "Cargo.toml"` → 只命中 `crates/gsoe-evolution/Cargo.toml` 的声明与注释、workspace 根 `Cargo.toml:449` 的注释 | **无任何 crate 启用**；`gsoe-evolution` 的 4 个消费者（`chimera-cli:52` / `chimera-mas:63` / `faae-router:32` / `parliament:51`）一律 `{ workspace = true }`，不带 feature |
| `r2_unfreeze` 的实际语义 | `crates/chimera-cli/src/composition.rs:286-295`：`#[cfg(feature = "r2_unfreeze")]` 才装配 `EvolutionOrchestrator`；默认分支 `let _evolution_orchestrator: Option<()> = None;` | 进化编排器默认不装配（与 `r2_path` 无关，是另一道门） |

### 3.4 调用链可达性：没有生产调用方

- **`R2FreezeViolation` 的三条宣称触发场景**（`crates/chimera-mas/src/error.rs:520-524`、`crates/event-bus/src/types.rs:2041-2044`）逐条核验：
  1. *CI 检测*（`r2-freeze-guard` job 扫描源码）—— `.github/workflows/` 全目录 grep `r2|freeze|R2` **无该 job**（仅命中无关的 `r2_` 批次标记与 `Nexus-core dependency freeze gate` 等）；且其实现载体 `auto-dpo` 已删除，扫描目标之一不存在。
  2. *运行时检测*（`evolve_once()` 入口 panic）—— `crates/gsoe-evolution/src/engine.rs:129-134`，`r2_path` 默认关闭且无人启用 ⇒ 该分支恒不进入；且即便进入也只是 panic，不构造 `MasError`、不发事件。
  3. *审计检测*（`AsaAuditor` 扫描 R2 激活痕迹）—— `crates/seccore/src/asa.rs` grep `R2|r2_|freeze|Freeze` **零命中**；该文件只发布 `NexusEvent::AsaIntervention`（`:354`），无 R2 相关扫描。
- **`R1ShadowRollbackFailed`**：全仓 `RollbackTriggerType` 的构造点除 `crates/event-bus/src/types.rs:1984`（字段类型声明）与 `crates/chimera-mas/src/error.rs:506`（字段类型声明）外，**全部在测试内**（`crates/event-bus/tests/*`、`crates/quest-engine/tests/*`、`crates/chimera-mas/src/error.rs:913`/`:942`/`:975-979`）。生产侧 `crates/chimera-mas/src/shadow/` 全目录 grep `rollback|Rollback` **零命中** —— 影子模式模块（`batch.rs` / `orchestrator.rs` / `config.rs` / `readiness.rs` / `evidence_gate.rs` / `stats.rs`）**没有任何回滚失败路径**，只发布 Normal 级的 `R1ShadowRegressionDetected`（`crates/chimera-mas/src/shadow/batch.rs:157`）与 `R1ShadowPromotionReady`（`crates/chimera-mas/src/shadow/orchestrator.rs:402`），二者 severity 均为 Normal（`crates/event-bus/src/registry.rs:288-289`）。
- **`ShadowModeOrchestrator` 无生产调用方**：`ShadowModeOrchestrator::` / `checkpoint_advice` 的调用点全部在 `crates/chimera-mas/src/shadow/orchestrator.rs` 的 `#[cfg(test)]` 内与 `tests/e2e/shadow_orchestrator_e2e.rs` 内；`crates/chimera-cli/src/composition.rs` 只装配 `ShadowModeCircuitBreaker`（`:291`，且在 `#[cfg(feature = "r2_unfreeze")]` 分支内），**不装配 `ShadowModeOrchestrator`**。
- **ADR-042 声明的 R2 路径本身不存在**：`git grep "constrained_rl|r2_policy|train_r2|GsoeAutoDpoRL|evolve_with_constrained_rl"` 的命中全部落在文档注释与测试内（`crates/gsoe-evolution/src/engine.rs:22`、`crates/chimera-mas/src/error.rs:919`、`tests/e2e/r2_freeze_violation_e2e.rs`），**生产代码零实现** —— 这正是 ADR-042 冻结有效的证据。

### 3.5 交叉验证（旁证）

- `python scripts/check_gated_wiring_substance.py` → **EXIT=0**，`[OK] gating substance matches the ledger`。其输出中 `r2_unfreeze` 只被记为 `decay-engine` / `omega-learner` 的门控来源（`PARTIAL`），`r2_path` 不在治理台账内 —— 与 §3.3「无人启用」一致。
- `bash scripts/check_crate_reachability.sh` → `[R] workspace members: 46, production reachable from chimera-cli: 38, unreachable: 0`；`[GATED]` 列表含 `omega-learner` 等 8 个 crate。`gsoe-evolution` 与 `chimera-mas` 本身生产可达，但**可达的是 crate 而非本 ADR 所问的错误路径** —— 可达性棘轮不覆盖「某 crate 内某变体是否有生产发布点」，二者不能互推。
- `git grep "freeze_guard|FreezeGuard"` 在 `crates/` 下**零命中**（只在 `.tower/worktrees/` 过期 worktree 与 `docs/reports/audit-followup/*.txt` 历史回归日志里），与 §3.2 一致。

## 4. 对 Wave B 优先级的影响

| 判定 | 影响 |
|------|------|
| 默认生产路径不可达（且当前树无任何生产发布点） | **Wave B（错误→Critical 事件转换层）保持既定优先级，不升 P0。** 本缺口是「潜在缺口 / 防御纵深缺口」，不是「正在发生的安全缺口」。 |

配套的三条处置口径（不改代码，仅登记判据）：

1. **不得以「四个 Critical 变体无生产发布点」为由新增装饰性发布方。** 若为消除「零生产者」观感而在 `chimera-mas` 侧凭空造 `MasError → NexusEvent` 转换，会引入永不触发的转换层与虚假覆盖信号；这与 `.claude/CLAUDE.md` §8「不得先加零生产者装饰变体」同源。
2. **Wave B 落码时的触发判据（何时才算值得升 P0）**：出现下列任一情形即重新判定为「可达」，Wave B 立即升 P0 ——
   - 任何生产代码路径构造 `MasError::R2FreezeViolation` / `MasError::R1ShadowRollbackFailed`；
   - `r2_path` 或任何等价 feature 被任一 crate 启用（`gsoe-evolution` 门控失效）；
   - `ShadowModeOrchestrator` 被接入组合根（即 `crates/chimera-cli/src/composition.rs` 装配它），或 `crates/chimera-mas/src/shadow/` 出现回滚失败路径；
   - ADR-042 解冻（FormalVerifier 落地 + 新 ADR 评审通过 + 影子模式 2 周验证）被批准。
3. **ADR-042 冻结期内的正确防线仍是既有的三重防护**，不因本 ADR 而改变：CI 关键词扫描 + 运行时 feature panic + 审计人工复盘。Wave B 的定位是**解冻后的接续面**，应排在 ADR-042 解冻条件达成之后，而不是现在抢跑。

## 5. 复现命令

```powershell
# E1 错误构造点全量枚举（应只命中 #[cfg(test)] 内的 4 处 + E2E 测试 4 处）
git grep -n "MasError::R2FreezeViolation\|MasError::R1ShadowRollbackFailed" -- "*.rs"

# E2 生产代码中零发布点的证明（bus.rs 的命中全部在 tests_helpers / tests 内）
git grep -n "NexusEvent::R2FreezeViolation\|NexusEvent::R2FreezeRollbackFailed\|NexusEvent::R1ShadowRollbackFailed\|NexusEvent::FailureRegistered" -- "crates/*/src/*.rs"

# E3 feature 门控：r2_path 默认关闭且无人启用
git grep -n "r2_path" -- "*/Cargo.toml" "Cargo.toml"
# → 只命中 crates/gsoe-evolution/Cargo.toml 的声明与注释

# E4 chimera-mas 无 [features] 段（MasError 侧零 feature 门控）
git grep -n "\[features\]" -- "crates/chimera-mas/Cargo.toml"   # 无输出

# E5 门控点的实际形态（panic，非 MasError 构造、非事件发布）
git grep -n -A4 'cfg!(feature = "r2_path")' -- "crates/gsoe-evolution/src/engine.rs"

# E6 历史生产发布方已随 ADR-193 物理删除
git ls-files "crates/auto-dpo/*"          # 空
git grep -n "freeze_guard" -- "crates/"    # 零命中

# E7 影子模式模块无回滚失败路径（生产侧零命中）
git grep -n "rollback\|Rollback" -- "crates/chimera-mas/src/shadow/"   # 零命中

# E8 交叉验证
python scripts/check_gated_wiring_substance.py    # EXPECT EXIT=0
bash scripts/check_crate_reachability.sh          # EXPECT [OK] crate reachability ratchet holds
```

## 6. 后果

### 6.1 正向

- Wave B 的优先级依据从「主观怀疑」转为**带行号的静态取证**，避免为一个不可达路径抢跑 P0 并挤占解冻影子期的治理带宽。
- 缺口本身被登记为显式条目（而非留在一句「无生产发布点」的模糊描述里），Wave B 落码时有明确的前置条件清单（§4 第 2 条的四个重新判定触发器）。
- 把 ADR-159 遗留登记（「B1 freeze_guard 双向接线 R2FreezeRollbackFailed」）与当前树实况的差异显式落档 —— 该接线的载体 crate 已被 ADR-193 删除，登记与实况的缺口在本 ADR 关闭。

### 6.2 负向与代价

- 本判定基于**当前工作树**的静态调用链。若后续新增 crate 恰好构造这些 `MasError` 变体，判定即失效 —— 故 §4 第 2 条把四个触发器写成可机器检查的清单（E1/E3/E7 命令可直接复用）。
- 本 ADR 不解决「零生产者装饰性 Critical 变体」这一**结构性观感问题**：四个 Critical 变体仍在 mpsc 旁路清单中占据位置（这在 bus.rs 的双清单一致性测试中是必需的），但**没有生产发布方**。该结构性问题属 ADR-193 架构减法的已知代价面，不在本任务范围。

### 6.3 明确不做的事

- **不实现**错误→Critical 事件转换层（Wave B 本体，不在本次范围）。
- **不修改** `Cargo.toml` 的 `workspace.package.version`（发布动作由发布流程负责）。
- **不新增** crate。
- **不触碰**工作树中他人已修改的文件（`.dockerignore` / `.github/workflows/ci.yml` / `.gitignore` / `.trae/rules/nuxus规则.md` / `Aegis` / `Cargo.lock`）。
