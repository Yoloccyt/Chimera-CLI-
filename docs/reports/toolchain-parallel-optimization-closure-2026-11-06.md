# 工具链深度性能优化与全环节并行化重构 — 收官验收报告

- **变更 ID**: `optimize-toolchain-dispatch-parallelization`
- **分支**: `feat/toolchain-parallel-optimization`（基于在途基线 `29d3b60` 之前的 `85361cc` 快照）
- **基线版本**: `v2.29.0-omega`
- **规格**: `.trae/specs/optimize-toolchain-dispatch-parallelization/{spec,tasks,checklist}.md`
- **ADR**: ADR-239（阈值基线与棘轮）、ADR-240（四环链生产接线）、ADR-241（并行化深化 + 无锁读否决）、ADR-242（回滚开关台账）
- **验收日期**: 2026-11-06

---

## 1. 本次交付了什么

| 波次 | 交付物 | 状态 |
|------|--------|------|
| Wave 0（前置，在途） | 批量派发骨架、沙箱策略缓存、分层归因基准、CLI 启动优化 D1/D4/D5、回滚开关 | 已固化为本分支基线，**未回滚任何他人改动** |
| Wave 1 | 6 个 bench 登记 + 22 个性能阈值 + 4 份 ADR 之一 | ✅ 完成 |
| Wave 2 | 四环链接入组合根（沙箱层在生产路径真实生效） | ✅ 完成 |
| Wave 3 | 异步探针 IO、借用化派发契约、**无锁读候选否决** | ✅ 完成 |
| Wave 4 | 回滚开关台账、L0 治理门登记收口 | ✅ 完成 |

**提交序列**（每提交独立可回滚）：

| 提交 | 内容 |
|------|------|
| `bf0e697` | Wave 1：性能阈值基线与棘轮登记（ADR-239） |
| `4c1aaba` | Wave 2：四环链接入组合根，沙箱层真实生效（ADR-240） |
| `29d3b60` | Wave 3：异步探针 IO、借用化契约、无锁读否决（ADR-241） |
| `ab09056` | Wave 4：L0 治理门登记 + 回滚开关台账（ADR-242） |

---

## 2. 关键量化结论

### 2.1 四环链分层归因（首次有了客观数字）

| 环 | 单点实测 | 链级残差 | 偏差 |
|----|---------|---------|------|
| 传输地板 | 58 ns | （基准） | — |
| 策略层（`ExecPolicy::evaluate`） | 465 ns | 539 − 58 = 481 ns | 3.4% |
| 沙箱层（`SeccoreSandboxGate`） | 1179 ns | 1698 − 539 = 1159 ns | 1.7% |

两组独立测点交叉印证（偏差 ≤3.4%）⇒ 归因可信。

### 2.2 并行化收益

| 场景 | 串行 | 有界并行（bound 8） | 加速比 |
|------|------|-------------------|--------|
| 64 请求 × 1ms 模拟延迟 | 493 ms | **112 ms** | **4.4×** |
| 64 请求 × 0 延迟 | 146 µs | 165 µs | 0.88×（预期：并行仅付调度开销） |

零延迟下并行略慢是**预期行为**（无收益可摊，仅调度开销增量 ≈19µs）——已在 ADR-239 D6 记录，防止后续误判为 bug。

### 2.3 沙箱缓存收益（Wave 0 在途成果，本次取得量化证据）

| 测点 | 旧实现 | 当前实现 | 降低 |
|------|--------|---------|------|
| 单点 | 3094 ns | 1179 ns | **61.9%** |
| 链级端到端 | 3694 ns | 1698 ns | **54.0%** |

### 2.4 CLI 启动分段

| 段 | 实测 |
|----|------|
| clap 解析（help / run / doctor） | 131 / 110 / 104 µs |
| 配置按需判定 | ~104 µs |
| runtime 构建（current_thread / multi_thread） | 4.3 µs / **765 µs** |
| 启动端到端（parse + 分类 + runtime） | **961 µs** |

multi_thread runtime 构建（765µs）是 one-shot 命令启动税的主要来源。

---

## 3. 门禁验收（逐条实测）

### 3.1 全绿门禁

| 命令 | 结果 |
|------|------|
| `python scripts/check_perf_redlines.py all` | **VERDICT PASS** EXIT=0 |
| `python scripts/check_perf_redlines.py inventory` | `unknown=0` EXIT=0 |
| `python scripts/check_perf_redlines.py invariants` | I1 75/75、I2 1≤1、I7 53 对、I9 75 键 EXIT=0 |
| `python scripts/run_gate_manifest.py` | **ok=135 mismatch=0 env-fail=0 unknown=0 skip=7** EXIT=0 |
| `python scripts/check_ci_manifest_parity.py` | EXIT=0 |
| `pwsh scripts/check_doc_consistency.ps1` | EXIT=0 |
| `pwsh scripts/check_dependency_rules.ps1` | EXIT=0 |
| `bash scripts/check_crate_reachability.sh` | EXIT=0（未新增 crate，棘轮未退化） |
| `python scripts/check_topk_registry.py` | EXIT=0 |
| `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` | EXIT=0 |
| `cargo fmt`（逐包，规避 `--all` 的 os error 206） | EXIT=0 |

### 3.2 不可判定门（如实记录，不伪造绿）

| 命令 | 结果 | 原因 |
|------|------|------|
| `python scripts/coverage_gate.py --check` | **EXIT=2（不可判定）** | 本机 rustc 未带 profiler runtime（`E0463`，Windows GNU 工具链限制），非本次引入；该门由 CI `coverage.yml` 产出 `cargo-llvm-cov` 数据后裁决 |

### 3.3 新增测试（全部绿）

| 测试文件 | 用例数 | 覆盖 |
|---------|--------|------|
| `crates/chimera-cli/tests/tool_runtime_wiring.rs` | 6 | 接线存在 / 沙箱生效 / 短路顺序 / 默认动作 / 拒绝文案真实根因 / 反向断言（禁绕过） |
| `crates/chimera-cli/tests/doctor_async_probe.rs` | 3 | 源码层零 `std::fs::` / 功能层判定逐字不变 / 并发层不阻塞 worker |
| `crates/gqep-executor/tests/tool_runtime_batch_production_invariants.rs` | 9 | spec §4.3 的 6 条 invariant 全覆盖 + 归因一致性 + 超时隔离 + deadline 填充 |
| `crates/gqep-executor/tests/tool_runtime_borrowed_equivalence.rs` | 5 | 借用 vs 拥有派发逐位等价（64-case property）/ 沙箱 `check_ref` vs `check` 判决等价（64-case property）/ 零隐藏分配 / 短路顺序不变 |

### 3.4 回归

```
cargo test --workspace --no-fail-fast
  PASSED=12065  FAILED=0  IGNORED=89
```

**零失败**。其中 89 个 ignored 为已登记在 `scripts/ignored_test_inventory_freeze.txt` 的显式决策（`slo-daily` / `slo-blocking` / `manual-only` 三态）。

> **对比基线**：本次修复了 2 个在途基线遗留的失败（§5.2），故当前是**全绿**而非「红但与本次无关」。

---

## 4. 关键架构决策与其证据

| 决策 | 内容 | 证据 |
|------|------|------|
| ADR-240 D2 | 策略默认动作用 `Allow` 而非零信任 `Deny` | 取证发现生产路径**原本既无策略层也无沙箱层**；改 `Deny` 会把工具调用从「可执行」变成「一律拒绝」⇒ **削减现有功能**。用 `Allow` 保持既有行为，同时让沙箱层成为本次**新增**防护 |
| ADR-240 D3 | 未接线传输环装 `UnwiredToolExecutor`（恒拒绝） | 装「总是成功」的 delegate 等于**虚构功能**，会把「无真实执行器」掩盖成「执行成功」。恒拒绝是 fail-closed 方向 |
| ADR-241 D3 | 借用路径与拥有路径走**同一套四环** | 另建借用链意味着四层各两个实现，行为可能分叉 ⇒ 借用路径成为绕过安全校验的旁路 |
| ADR-241 D5 | 无锁读候选（`DashMap`）**不采纳** | 同轮对照：c4 242.72µs(`RwLock`) vs 294.86µs(`DashMap`) **慢 21.5%**；c8 476.87µs vs 545.60µs **慢 14.4%**。沙箱门读临界区极短，锁争用成本低于分片哈希固定成本。`dashmap` 仅作 dev-dependency，**不进生产依赖面** |
| ADR-241 D6 | **诚实记录借用化净收益为 0** | 调用方省下的 2 次分配被入站「借用→拥有」转换抵消；链内 `Command` 构造仍需分配（`seccore::validate_command` 要求拥有型）。本次是**接口准备**而非有效性能优化 |
| ADR-242 D4 | 安全语义的降级面**明确声明为「不提供」** | 沙箱层/策略层/路由层/`缺环构造`均无开关 —— 环境变量是部署侧可控的，而沙箱不可降级是 `seccore` 6 条不可学习红线之一 |

---

## 5. 未决事项与如实记录

### 5.1 本次**未**解决（明确留给后续批次）

1. **生产 `ToolExecutor` 实现仍缺** —— 传输环仍为未接线状态。真实工具执行器（进程调用 / MCP 调用）的落地是独立批次。
2. **策略规则的声明式来源仍缺** —— 需新增配置段并联动 `config_sample` 门与 14 段 Figment。
3. **多工具编排点仍缺** —— 取证发现 CLI 侧工具执行面完全空白（`impl ToolExecutor for` 的生产命中仅 `ExecPolicyToolExecutor` 与 `ToolRuntime` 自身），无可接入的编排点；批量契约已锁定，待真实执行器落地后接入。
4. **链内全借用化** —— 需 `seccore::validate_command` 提供借用入参变体（其内部 `build_full_command_string` 本身就要构造字符串，收益上限受此约束）。
5. **ADR 文档面不随 git 分发** —— `docs/**/*.md` 受 `.gitignore` 的 `*.md` 排除，本报告与四份 ADR 均不入跟踪。修复需改 `.gitignore` 豁免规则（独立变更）。

### 5.2 既有失败（已修复 + 既有 flaky 证伪）

全量回归首跑暴露 3 个失败，用 `git stash` 在**无本次改动的基线**上复跑证伪：

| 失败测试 | 判定 | 处置 |
|---------|------|------|
| `quest-engine/critical_bypass_backpressure_test::critical_events_within_capacity_all_delivered` | **既有失败**（基线同样失败）—— 根因是在途基线把 `CRITICAL_MPSC_VARIANTS` 从 16 提到 18，而该测试的期望集仍是 10 项 | **已修复**：期望集扩至 18 项（引用 `CRITICAL_MPSC_VARIANTS` 常量），构造清单扩至 26 项（18 旁路 + 4 severity-only + 4 Normal），broadcast 侧期望集扩至 27（旁路与 broadcast 是并集非二选一）。关键推导：18 个旁路变体全部同时是 severity Critical |
| `quest-engine/critical_bypass_backpressure_test::each_critical_variant_delivered_individually` | 同上 | 同上 |
| `repo-wiki/hnsw_store::test_top_k_returns_sorted_by_score_desc` | **并发顺序敏感的既有 flaky**（单独跑通过；全量并发时失败） | 未修复（属既有 flaky，非本次引入） |

修复提交：`7676d83 test(event-bus): 同步 Critical 旁路测试期望集至 18 项`

### 5.3 本次引入并已修复的门禁红

收口门禁清单首跑 `ok=129 mismatch=6` —— 6 个红**全部**由本次 L0 新增类型 `DispatchRequestRef` 触发：

| 门 | 缺口 | 处置 |
|----|------|------|
| G-91 | 新 L0 def <2 消费者未登记 | 登记 6 个 dispatch 契约 def（5 个为 ADR-238 遗留 + 1 个本次新增），每条附人工审阅理由 |
| G-105 | L0 pub fn 面棘轮 | 登记 `spec/tool_dispatch::to_owned_request` |
| G-115 | L0 可扩展性分类缺行 | `DispatchRequestRef` 声明 `class=closed`（无 `#[non_exhaustive]`，按锁规则必须 closed），CLAIM 行同步刷新 |
| G-116 | 台账自证（依赖 G-115） | 随 G-115 一并转绿 |
| G-123/G-124 | 零消费者台账 CLAIM 与实况不符 | 机器重新生成（`--write`，手工编辑被禁止） |

修复后复跑：`ok=135 mismatch=0 env-fail=0 unknown=0 skip=7` EXIT=0。

---

## 6. 功能不削减的证据

| 判据 | 实测 |
|------|------|
| 既有测试零删除 | 相对 Wave 1 基线 `bf0e697`：全仓 69 文件 / **+6723 / 零删除行**；`git diff --name-status` 无 `D` 状态 |
| `Commands` 枚举变体集合不变 | `git diff` 对 `cli.rs` 的变体行**零命中**（穷尽 match 编译期保证） |
| L0 契约既有签名不变 | `tool_dispatch.rs` diff **只有新增行**（`DispatchRequestRef`）；`ToolDispatcher::dispatch` / `DispatchRequest` 签名逐字未改 |
| 退出码矩阵不变 | ADR-060 矩阵不变；`--legacy-exit-code` 保留；`NotImplemented` → 退出码 2（有既有测试锁定） |
| 沙箱语义不变 | 沙箱 `check_ref` 与 `check` 判决逐位等价（64-case property）；既有 `cache_does_not_change_verdicts` 等 5 个缓存语义测试全绿 |
| 批量语义不变 | spec §4.3 的 6 条 invariant 全部被测试锁定 |

---

## 7. 回滚路径

### L1 · 特性开关（秒级）

台账见 [ADR-242](../architecture/ADR-242-rollback-switch-registry.md) §3 —— 覆盖 20 项开关，每项含「作用 / 回滚到的行为 / 影响范围 / 验证命令」。

**安全语义的降级面明确为「不提供」**：沙箱层 / 策略层 / 路由层 / 缺环构造均无开关。排查工具调用被拒时读错误前缀（`sandbox:` / `policy:` / `route:`）即可获知具体拦截原因。

### L2 · 提交粒度（小时级）

4 个提交（Wave 1~4）各自独立，逆序撤销即可回退到任一波次边界；每撤销一个跑一次 §3.1 门禁集。

### L3 · 分支/发布（天级）

- 分支：`feat/toolchain-parallel-optimization`（独立于在途的 `feat/c1-compute-plane-governance`）
- 发布：按 `CONTRIBUTING.md` §6；回滚 = 重打 tag + Homebrew Formula 同步（`Formula/chimera.rb` 的 version 与 sha256 须与 Release `checksums.txt` 逐字符一致）

---

## 8. 本次的方法论纪律（供后续复用）

1. **先实测后登记** —— 22 个阈值全部来自本机实测 median，余量按项目既有分级范式；登记前用 `tmp/check_new_thresholds.py` 逐键核验「真实命中而非静默落进幽灵桶」（22/22，零幽灵）。
2. **同轮对照裁决** —— `DashMap` 候选与 `RwLock` 现状在同一进程同一轮内测量（跨轮会被机器状态污染）。**「不采纳」与「采纳」同等重要**：它证明改造决策有客观依据，而非凭直觉。
3. **证伪优先于归因** —— 3 个既有失败用 `git stash` 在基线上复跑证伪，才敢判定「与本次无关」。
4. **失败要可见** —— 覆盖率门 EXIT=2 如实记录为「不可判定」并说明原因，不伪造绿。
5. **诚实记录负收益** —— 借用化净收益为 0 写进 ADR-241 D6，而不是把它包装成「性能优化」。
6. **安全前置不可降级** —— 不提供关闭沙箱层的开关；排查走诊断信息而非关安全能力。

---

*本报告依 `superpowers:verification-before-completion` 的「证据先于断言」编写：每条结论均附实测命令与输出；未决事项、既有失败、不可判定门禁均如实列出。*
