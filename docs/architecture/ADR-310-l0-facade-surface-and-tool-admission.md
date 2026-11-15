# ADR-310: L0 重构 W5 —— facade 值级面单源化 + 域聚合面 + 批派发准入契约 + L0 门单入口

- 状态：**Accepted（2026-11-16，施工与裁决同批）**
- 层：L0 Contracts（nexus-contracts umbrella）+ 工程侧工具链（scripts/、ci.yml、gate_manifest.toml）
- 前置：ADR-033（受控纯逻辑口径）/ ADR-212 / ADR-214（G-115 开档）/ ADR-215（D2 零消费符号裁决）/ ADR-218（G-119 逻辑台账）/ ADR-222（1+6 拆分与路径承诺）/ ADR-309（W4 compute 提取，同树在途）
- 任务书依据：rust-pro 极限重构任务「L0 层极致深度重构 + 工具链并行调度」之 L0 治理批次

## 1. 背景与动机

三个已实测的维护性病灶：

1. **facade 值级双镜像**：umbrella `lib.rs` 顶层 `pub use <leaf>::{…}`（~148 行）与 `pub mod prelude`（~129 行）是两份手工镜像清单，靠 `tests/prelude_surface_test.rs` 解析源码抓漂移——ADR-217 批次记录的 2026-10-15 失守事件（prelude 收伴生漏主类型）证明"事后解析对拍"不是结构消除；新增一个契约类型需同步改 3-4 处。
2. **组目录 ≠ 消费域**：ADR-222 拆分的六个组目录是 W3 批次面而非消费域面（`NamespaceQuota` 落 memory 组、`scheduler_contract` 落 spec 组），找类型靠记忆。
3. **L0 无批级调度契约**：执行面批/窗口/许可语义散在 L7 三个模块 + L10 一个模块（tool_plane 双车道、toolplan_runner 在飞窗口、tool_loop 双信号量），L0 只有单发语义的 `tool_dispatch`；L8/L9 想按依赖铁律消费"调度语义"没有 L0 类型可用。
4. **门禁入口碎片化**：10 个 `check_l0_*` 脚本各自为政，ci.yml ~20 处近重复调用行各自漂移。

## 2. 决策

**D1（L0-A）：值级面单源宏 `l0_surface!`**。新增 `crates/nexus-contracts/src/facade_surface.rs`：每个 leaf 模块的导出符号只声明一份，条目尾标记 `prelude`（进 glob 面）/`top`（只进顶层）；宏同时生成顶层 `pub use` 与 prelude 再导出。"无孤儿/同族完备"由构造保证。声明顺序与重构前逐字一致。

**D2（L0-B）：模块级 re-export 文档收敛**。~53 处根 `pub use <group>::<leaf>;` 的长文档注释收敛为一行摘要（rustdoc 继承叶子模块文档）；umbrella 头表 mca 文件数笔误修正（5→6）。

**D3（L0-C）：域导向聚合面 `faces`**。新增 `crates/nexus-contracts/src/faces.rs`：`faces::{domain,execution,governance,learning}` 四聚合面，全部是既有符号的 additive 再导出，既有 `<leaf>`/`<group>::<leaf>` 路径逐字不变。有意不收入任何面的族（稀疏掩码/向量数学/MCA/Harness spec）在文件头列明理由。

**D4（L0-C2 否决记录）：死代码清理维持不删**。任务初案据零消费 grep 提议删除 `safety::calibration` 月度评测家族 7 符号 + `memory::quota::QuotaLimits`；施工前复核发现 **ADR-215 D2（2026-10-15 Accepted）已对这 12 个候选中的相关符号逐个裁决为 DEFERRED 保留**（`rm_distillation_eligible` 语义绑定 `JudgeTier::RewardModel` 与 RL 开发闸门同步；月报三表簇为前瞻面且 G-91 基线已登记）。零消费事实正是该 ADR 的输入而非新证据，**无新依据推翻 Accepted ADR ⇒ 不删除**，本 ADR 记录该否决以免下轮再提案。

**D5（P-A）：批派发准入契约 `tool_admission`**。新增 `crates/nexus-contracts/src/spec/tool_admission.rs`：`Admission`/`EffectClass`/`DenyReason`(open)/`TruncationCause`/`CallOutcome`(closed) 五枚举 + `PlannedCall`/`BatchPolicy`/`BatchPlan`/`BatchReport` 四载体（均 open，构造必经 `new()`）；四段守恒（succeeded+failed+denied+not_started==total，对齐 gqep gatherer 先例）；`EffectClass::Unknown` 必串行的 fail-closed 判据（`PlannedCall::is_read_safe`）。`ToolDispatcher::dispatch_batch` additive 默认实现（顺序逐发，15 个既有实现方零改动）。`BatchPolicy` 默认值与 gqep `PlanGuards` 基线对齐（8/30s/fail-soft）但**两处数字永不互相写入**（沿 `PlanGuards::max_parallel`/`concurrency` 分设先例）。

**D6（L0-D）：L0 门单入口 `check_l0_surface.py`**。薄调度器（无裁决逻辑、verdict 精确透传、`all` 模式 no-fail-fast 且 undecidable 优先于 red），ci.yml 20 处调用与 gate_manifest.toml 20 条 cmd 同步改指。**冻结合并 toml 延期**：合并会强制每个门同批改 BASELINE 路径并使针对旧路径种缺陷的 `--selftest` 夹具失明，收益不抵风险——记录于此，条件成熟（需要动 freeze 的新批次）时再立 ADR。

## 3. 备选方案与否决理由

| 备选 | 否决理由 |
|---|---|
| build.rs 生成导出代码 | 引入构建期复杂度与生成物漂移面；宏是编译期同一机制且 rustdoc/doctest 原生可用 |
| 维持双清单 + 加强解析测试 | 现状的病灶正是"解析对拍"——ADR-217 失守事件已证明 |
| 大规模 breaking（组目录重排/改名/Box<str> 全局替换） | ADR-222 D4 路径承诺 + 34 消费 crate + 9 份冻结清单的机械锁定；收益不抵迁移与门禁同步成本（Ω₉-Preserve 对齐） |
| 新建 `tool-scheduler` crate / L0 承载调度逻辑 | G-99 扫描 tokio/async 直接判红（污染契约层纯度）；S1-S6 行为锁定测试使物理合并=重写而非重构（违背可回溯） |
| 双运行时（tool_plane/tool_runtime）物理合一 | agent 调用图证实的最大单点归一风险；采用"调度真值源归一、执行链保持两态"（后续 P-B 阶段） |
| L0-C2 执行删除 | ADR-215 D2 有效裁决（见 D4） |

## 4. 兼容性影响

- **零 breaking**：所有变更 additive 或源码内部重组；公共 re-export 面经基线等价锁证明逐字等价。
- 消费方面：34 crate 零改动（workspace check 验证至 chimera-cli lib；`windows` 依赖 crate 的 STATUS_STACK_BUFFER_OVERRUN 为 16 核宿主已知 OOM，与本次变更无关）。
- umbrella src 面 21→25（+facade_surface +faces +tool_admission 及口径重数）。

## 5. 台账同批清单（治理门禁登记）

| 门禁 | 动作 |
|---|---|
| G-99 purity | 自动通过（75 文件 0 不洁） |
| G-105 pub fn 棘轮 | 260→268（--emit 候选晋升 8 新 fn） |
| G-107 根面 | +`spec::tool_admission`（沿 tool_dispatch 先例带理由） |
| G-115 可扩展性 | +9 类型行（5 open 带理由 / 4 closed），CLAIM 236→245 |
| G-119 逻辑台账 | +`segment_counts` 行（--emit 机采字段）+ 模块例外登记（lib.rs 登记表与 `tests/l0_exception_registry.rs` 期望集同批）；CLAIM 刷新 |
| G-125/G-126/G-152/G-99b 等 | 无漂移，自动绿 |
| ci.yml / gate_manifest.toml | 20+20 调用点改指 `check_l0_surface.py`（双侧一致，parity 门绿） |
| CODE_WIKI §3.0 | umbrella 关键文件/设计模式登记 |

## 6. 验证证据

- `cargo test -p nexus-contracts`：370 passed / 0 failed（含 `prelude_surface_test` 基线等价锁、`faces_surface_test` 4 面锁、`l0_exception_registry` 4 不变量）。
- `check_l0_surface.py all`：10/10 绿。
- `check_ci_manifest_parity` / `check_py_tooling_health`(H1-H9) / `check_gate_predicates_parity`：全绿。
- 已知存量红：`audit_gate_selfreport` GAP-B1（crate 数 47 未进三份文档）属 ADR-309 W4 在途工作面，非本批次引入。

## 7. 回滚路径

逐 commit revert：L0-A/B（facade 宏化 + 文档收敛）、L0-C（faces）、P-A（tool_admission + dispatch_batch + 台账）、L0-D（调度器 + 接线）。各 commit 互不依赖破坏面；基线等价锁保证 revert 后导出面仍与重构前一致。

## 8. 后续（不在本 ADR）

- **P-B**：L7 调度真值源归一（S4 下沉 InteractiveDispatcher / legacy `PlanRunner::run` 改派 `run_with` / 四环链 strangler 迁入）——依赖本 ADR D5 契约。
- **P-C**：MCP 并行批量入口、turn 内局部序、CPU 感知默认值、观测闭环（tracing span / Runtime Auditor 并行度维度 / BatchTrace）。
