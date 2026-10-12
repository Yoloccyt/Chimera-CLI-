# ADR-160: 生产可达性棘轮门禁与 event-bus 分层子枚举镜像退役

- **状态**: Accepted(2026-08-29,代码与门禁已落地并通过本地验证)
- **决策者**: 冗余与死代码深度审计(第一轮全局诊断 + 第二轮契约层)+ 用户口径裁决
- **关联**: ADR-159(上一轮 `audit-redundancy-deadcode` 遗留项治理)、ADR-054(Phase 9 三环重组 / 决策 5 依赖铁律闸门)、ADR-065 决策 6(MCA feature 双轨)、ADR-033(L0 契约层)、ADR-145/146(mas-sched / nexus-hook 影子先行)、红线 §2.2(依赖方向)、红线 §3.4.1-3(TDD 守恒:不删测试)

## 背景

用户要求对 43-crate workspace 做"代码冗余与死代码深度审计"。传统 grep 式判据在本仓库失效:
`cargo test --workspace` 11,482+ 全绿、依赖铁律闸门"零违规",但**全绿只证明"被测试引用",不证明"被产品装配"**。

审计改用**可达性判据**:从组合根 `chimera-cli`(唯一发布 bin)沿各 crate `[dependencies]` 的
**非 optional 生产边**做 BFS,`dev-dependencies` / `build-dependencies` 一律不计入。实测:

- workspace members = **43**,生产可达 = **28**,**不可达孤岛 = 15**,合计 **50,604 src LOC(15.2%)**;
- 其中仅 `mas-sched`(ADR-145)/`nexus-hook`(ADR-146)有"Cargo.toml 自陈无消费者 + 预留 ADR"双重证据;
  `mca-gateway` 只被根 `mca` feature + `ci.yml` mca job 编译,无任何生产 crate 声明它(连 optional 都没有);
  其余 12 个(model-router / auto-dpo / scc-cache / csn-substitutor / gea-activator / sesa-router /
  decb-governor / acb-governor / chtc-bridge / lsct-tiering / mtpe-executor / ssra-fusion)为
  Week 6/7 交付后从未接线的**遗留孤岛**;
- 附带发现 A(阻断级):`scripts/check_dependency_rules.sh` 的层图停在 38 而 `.ps1` 已升 43,
  **CI 调用的是 `.sh`** → 依赖铁律 job 实测 `EXIT=1`(5 条 `[GAP-C] disk crate <…> not registered`);
- 附带发现 B:`mas-sched` / `nexus-subagent` 为凑"内部依赖 ≤6"的**注释自述门禁**声明了源码零引用的
  `event-bus` / `nexus-core` / `tracing` / `thiserror`(machete 实测),即"声明先于实现"反模式;
  `mcp-mesh` 声明未用的 `nexus-contracts`;根 `Cargo.toml` 的 `once_cell` 为孤儿声明。

契约层(第二轮)发现 C:`crates/event-bus/src/event_types.rs`(1,358 行 / 8 个 `pub enum` / 110 变体 /
8 个 `impl EventClassification`)是 polish-v2.7(`af1b3af`,2026-07-29)发起的"NexusEvent 渐进式转
wrapper"重构**半成品,委托从未接上**:

- `types.rs` 的 `metadata()` 文档注释宣称"委托给子枚举 `EventClassification::metadata()`",但函数体
  是手写 144 分支 match,零委托;
- `types.rs` / `classification.rs` / `bus.rs` 对 8 个子枚举的引用数均为 **0**;全仓唯一代码消费者是
  `crates/event-bus/tests/tui_handshake_test.rs` 的 2 个"双表同步"测试(覆盖 2/110 变体);
- 镜像已比权威源少 34 个变体(110 vs 144),连文件头自陈分组数(88)都与同文件实测(110)不一致;
  `types.rs` 在 `9c9b802`(2026-08-19 Phase 10 §16)新增变体时未回写镜像 → 结构性腐烂。

即 **1,358 行镜像的全部现存价值 = 防止 2 个变体的 tag 漂移**,且它构成与
`classification.rs:46 severity()` 竞争的第二份严重度真值源。

## 决策

1. **可达性判据入库为棘轮门禁(不删代码)**:新增 `scripts/check_crate_reachability.py`(单一实现)
   + `.sh` / `.ps1` 薄包装 + `scripts/crate_reachability_freeze.txt`,挂载于 `ci.yml` 的 `check` job
   (紧随依赖铁律闸门)。判据:dev-dep 与 optional 边不计入装配面;仅被 optional 边引用者判
   `[GATED]`(feature 门控是文档允许的还债路径之一,不得同时被判为孤岛);已知孤岛登记为
   **可见债务**,只减不增。
   **理由**:用户口径明确"先立门禁 + 冻结清单",删除或接线都需独立路线图与验收;棘轮能在零风险
   的前提下阻止债务继续增长,并把度量固化进 CI 日志。
2. **退役分层子枚举镜像(方案 A),不补完委托(方案 B)**:删除 `event_types.rs` 整文件 +
   `lib.rs` 的 `pub mod event_types;` 与 9 个符号的 `pub use` re-export;`EventClassification` trait
   随镜像一并退役。分类真值源收敛为单表:`types.rs::metadata()` +
   `classification.rs::severity()` + `topic.rs::topic()`。
   **理由**:方案 B 需先补 34 个缺失变体再重写 8 个 match,收益仅是把一个大 match 拆成 8 个小 match,
   而 `types.rs` 的巨型 match 本来就要维护;且 ADR-159 决策 6 已确立"144 变体守恒 + 类型/severity/
   type_name 一致性由 event-bus 分类与既有断言守护"——本决策与该口径一致,不新增真值源。
3. **测试改造而非删除(守 TDD 守恒红线)**:`tui_handshake_test.rs` 原
   `dual_table_type_names_match` / `dual_table_severity_matches` 两个测试改为单表等价断言,
   函数数保持 7 个、断言数由 8 增至 10,并**加强**判据:type_name 覆盖 `CompatLevel` 三态,
   severity 额外显式断言握手事件不得落入 `Critical`(避免无谓占用 mpsc 旁路配额)。
   新增事件适配触点由四处降为三处(`types.rs` / `classification.rs` / `topic.rs`)。
4. **依赖声明必须后于实现**:删除 `mas-sched`(nexus-core/event-bus/tracing)、
   `nexus-subagent`(nexus-core/event-bus/thiserror/tracing)、`mcp-mesh`(nexus-contracts)
   的零引用生产声明与根 `Cargo.toml` 的 `once_cell` 孤儿声明;两处"事件发布"伪证据注释改写为
   如实的 TODO,并要求将来事件面落地时以 `optional = true` + feature 门控接入(进 `[GATED]` 轨道)。
5. **修复双源漂移并确立"单一实现"约定**:`check_dependency_rules.sh` 层图追平 43 并补齐 `.ps1` 独有的
   Check D(chimera-mas 内部依赖 ≤16,WI-29),两文件文案不再硬编码 crate 数,头部互写 DRIFT WARNING。
   新门禁一律采用"python 单核心 + 平台薄包装",禁止再出现同一规则两份手写实现。
6. **保留 ADR-159 的全部既有裁决,本 ADR 不推翻**:
   - `BenchmarkMetricsCollected` 依 ADR-159 决策 4 **正式预留(禁删)**;本 ADR 撤回审计第二轮中
     "删除该变体"的建议,仅保留事实登记(仍无发布方)。
   - bench 一律**禁止删除**(ADR-159 决策 3 的 gated/registered/dev-only 三态登记),审计后续轮次中
     "删同名 bench 一份"的建议改判为"按三态登记或参数化合并"。

## 与 ADR-159 的偏差登记(待复核,本 ADR 内不处置)

- `GhostMemoryDetected`(`types.rs:2293`)与 `TokenLedgerRecorded` **未被 ADR-159 决策 4 的 13/14
  接线清单覆盖**(该清单已列 13 个真实生产者 + 1 个 `BenchmarkMetricsCollected` 预留):
  前者在 event-bus 外仍**零构造点**,仅 `membrane.rs:459` 的 `is_read_metric` 谓词有分支,
  且 `mlc-engine/src/mem_con/mod.rs:27` 的 ASCII 架构图**声称发布**该事件,而
  `mem_con/controller.rs:341` 实际发布的是 `MemConStrategyAdjusted`,`mem_con/types.rs:51` 是同名但
  无关的私有枚举变体 → 属"遗留余口 + 文档反向不一致",需专项复核。
- `TokenLedgerRecorded` 有发布者(`event-bus/src/token_ledger.rs:139,173`)但
  `append_and_notify*` 在 event-bus 外零调用方 → Phase 10 Wave 4 的 L1→L3 事件化链路悬空。
- 陈旧注释(非缺陷):`bus.rs:80,86,96` 仍写"全量 9 个清单"(实为 13);
  `classification.rs:36-46` 沿用 Week 3-5 口径。`CRITICAL_TOTAL=17` / `CRITICAL_MPSC_VARIANTS=13`
  为 D-8 既定设计,**本 ADR 明确不改通道归属**。

## 后果

- **CI**:`check` job 新增 `Crate reachability ratchet` 步骤(selftest + 真实裁决双跑);
  依赖铁律 job 由红转绿(实测两脚本双模式 EXIT=0)。
- **可见债务**:15 个孤岛 / 50,604 src LOC 进入 `crate_reachability_freeze.txt`,
  每行附一行理由与出处;还债路径三条,还清后由门禁 `[ROTATE]` 提示删条目。
- **公开 API 收缩**:`event-bus` 减少 9 个 `pub` 符号(8 枚举 + 1 trait)。仓库无公开 API 快照门禁,
  由 `cargo clippy --workspace --all-targets -- -D warnings` 兜底。
- **代码量**:`-1,358` 行(event_types.rs)+ 三个 crate 清单瘦身;`crates/*/src` 由 333,598 → 332,229 行。
- **验证证据**:`cargo check -p event-bus -p nexus-contracts --all-targets` 干净;
  `cargo test -p event-bus` **558 passed / 0 failed**(21 个 test target + doctest,
  `tui_handshake_test` 仍 7 passed);`cargo check -p mas-sched -p nexus-subagent -p mcp-mesh
  --all-targets` EXIT=0;`cargo machete` 零输出;`cargo fmt -p event-bus -p nexus-contracts -- --check` 干净。
- **工具链约束**:门禁依赖 python ≥3.11(`tomllib`);`load_toml` 必须容忍 BOM —— 仓库内 6 个入库
  `.toml`(根 `Cargo.toml`、`audit.toml`、`chimera-tui` / `gqep-executor` / `mcp-mesh` /
  `nexus-app-server` 的 Cargo.toml)带 UTF-8 BOM,cargo 容忍而 `tomllib` 直接抛
  `TOMLDecodeError`。另:`scripts/check_doc_consistency.sh` 硬编码 `python3` 无回退,
  在仅有 `py` 启动器的 Git Bash 下 EXIT=127(本地限制,CI ubuntu 不受影响)。
- **后续轮次进度**(2026-08-29 第三轮审计后更新):
  - ✅ **微观重复收敛已交付**(本条原列为"不属本 ADR"的未来项):`select_top_k_desc` ×3 →
    `xts_top_k_by`、`sigmoid` ×2 → `nexus_contracts::util::sigmoid`、`percentile` **实测 14 处**
    (非原估 15)分层收敛 → 新增 `nexus_contracts::util::percentile_sorted<T: Copy>`。
    索引口径统一为多数派 `round((n-1)*p)`;`tests/` 6 处委托共享工具、`benches/` 7 处依
    ADR-159 决策 3 三态登记(不删)、`efficiency-monitor/src/affinity_metrics.rs` 1 处标注
    有意独立实现。验收:9 crate `cargo check --all-targets` EXIT=0、percentile 定向 7 passed(含 2 proptest)、14 crate
    `clippy --all-targets -D warnings` EXIT=0、**release `--ignored` p95 SLO 红线 16 项全绿**
    (证明口径统一未削弱延迟守护)、`cargo machete` 零输出、`cargo fmt -- --check` 干净。
  - ⏳ **`osa-coordinator/src/cheap_index.rs:178` 余弦行为分叉**(本 ADR 正式登记;须独立 ADR + TDD):
    与权威 `nexus-core/src/clv.rs::cosine_similarity_slices` 存在 **6 维分叉** —— 输入类型
    (`&[f64]` vs `&[f32]`)、**不等长语义(直接返回 `0.0` vs 取 min-len)**、NaN 传播、
    零向量判定(软阈值 `< 1e-12` vs 精确 `== 0.0`)、输出 clamp(**无** vs `[-1.0, 1.0]`)、
    性能(3 pass 无 SIMD vs 1 pass `chunks_exact(4)`,实测 4.2×)。**根因判定为 f64 特征向量的
    类型隔离遗留,非有意设计**。最高风险:不等长语义相反,可能使 `SharedResultCache::get`(:123)
    的缓存闸门产生假阳性复用。修复涉及 `features: Vec<f64>` → f32 破坏性变更 + TIE-SWA 缓存
    正确性论证,且现有 `cosine_defensive` 测试(:479)全用等长输入不覆盖该分歧 → 本 ADR 仅登记不处置。
  - ⏳ 仍待处置:`nexus-test-support` crate 缺失、E2E 镜像参数化、
    `hcw-window/src/recall/eval/mod.rs:134` 缺 `cfg(test)` 门控致评测 harness 编入 release。
