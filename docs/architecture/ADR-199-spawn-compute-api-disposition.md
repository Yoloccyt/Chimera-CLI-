# ADR-199：单任务卸载 API（`ComputeBridge::spawn_compute`）去留决议请求

- **状态**：✅ **Accepted（2026-10-07，所有者认证：采纳候选 B）**；原 🔶 Proposed / 待架构委员会裁决（2026-10-02）经所有者路径完成认证（§7 认证登记）。候选 B 已于 2026-10-03 经所有者指令执行（§1 执行状态注）
  > **执行状态注（2026-10-05 追加）**：候选 B（删除 `spawn_compute`）已于 2026-10-03 经用户明确指令执行——饱和/panic/取消契约测迁入 batch 路径、台账行退役、`Cancelled` 改由 batch None 兜底构造；证据见 `docs/reports/cpu-plane-phase0-c0-2026-10-02.md` §8。上方 §1 事实基线与 §4 实施清单为决策时点快照，按「历史只加注不改写」原则保留；本 ADR 的正式认证权仍属委员会（Q1-Q3 待批）。
- **来源批次**：C1 计算面治理收口（`docs/reports/` 同批进度报告 §4.6）；该批次的既定纪律是"契约已承诺、闭环未接上"这一类缺陷必须在批次内被处置或被正式登记为待决
- **承袭**：ADR-095~099（Phase 1 双运行时与 ComputeBridge）、ADR-101/102/106（确定性与禁手写 SIMD）、ADR-103/128（HTS-CPU 阈值）、ADR-120/127（L-f 为唯一调度入口）、ADR-194（Claim/Certify 分离）、ADR-197（A1 饱和三态与 A6 契约台账）、ADR-198（Phase B1 校准闭环）
- **不触碰**：`NexusEvent` 变体集（152 锁定）、crate 数（39 锁定）、`perf_redlines.toml` 现有六条 `[[slo]]`、R2 冻结面（ADR-042/052/053）

---

## 1. 事实基线（2026-10-02 现测，非引述）

| 事实 | 取证 |
|---|---|
| `ComputeBridge::spawn_compute` 生产调用点 **0 个** | `crates/nexus-core/src/compute/bridge.rs` 全部 4 处调用（L660 / L673 / L980 / L1002）均位于该文件 `#[cfg(test)]` 测试尾之后；`scripts/check_dead_contract_apis.py` 主判定同报 `DEFERRED / uses=0` |
| 批量入口 `spawn_compute_batch` **生产调用点 7 个** | hcw-window / nmc-encoder / faae-router / decay-engine / osa-coordinator / repo-wiki / gsoe-evolution，各带 `TaskKind` |
| 批量入口是**同步**的 | `bridge.rs:404` 签名返回 `Vec<Result<T, ComputeError>>`，非 `Future`；内部为 `rayon::scope` 同步阻塞至全部完成 |
| `ComputeError::Cancelled` 的**唯一生产构造点**在 `spawn_compute` 内 | `bridge.rs:426` 的 `async move { rx.await.map_err(\|_\| ComputeError::Cancelled)? }` |
| `Cancelled` 的第二构造点是**理论不可达**兜底 | `bridge.rs:537-542`，注释自陈"scope 同步阻塞至全部任务完成，槽位必然已填"，`None` 兜底用 `Cancelled` 仅为保持库代码零 panic |
| 该 API 的取消语义无法被生产代码行使 | 取消需要「调用方在 await 前 drop future」；而唯一的生产卸载面是同步批量入口，不存在 await 边界 |

**推论**：单任务卸载 API 承担的两项契约——「池内 panic 永不跨线程传播到调用方」与「调用方取消 → `Cancelled`」——在前者已由批量入口的逐任务 `catch_unwind` 覆盖（`bridge.rs:519-522` 语义），后者则因批量入口无 await 边界而**在生产中无路径**。

---

## 2. 三个候选

### 候选 A —— 接一个真实调用方

找一处真实需要单任务卸载的热点，改用它。

- **优点**：保留 API 与 `Cancelled` 契约，无需删改。
- **代价**：本仓的 CPU 热点已由 P1-T14（WI-34）在 7 个点位逐个注入并默认开启，**当前无已知未接桥的单任务热点**；为保留 API 而制造调用方，等于反向制造"调用方"。审计点名的 8 个「应评估」相似度文件规模未证，且 osa 在 2210 项时并行**慢 3×** 是现成反例——无拐点实测即接桥是负收益优化。
- **风险**：以"保留 API"为动机接的调用方，很可能既无性能收益又增加一处契约面。

### 候选 B —— 删除 API 并回写契约

移除 `spawn_compute`，连带处置 `Cancelled` 的文档与 4 处测试调用；批量入口成为 L-a 唯一生产卸载面。

- **优点**：消除本批的核心病灶形态。其零生产调用者正是"契约已承诺、闭环未接上"的原型；删除后 `scripts/check_dead_contract_apis.py` 该行可直接退役，契约台账少一条 DEFERRED。`nexus-core` 属 L1 内部层，删除面限于 workspace 内。
- **代价**：`ComputeError::Cancelled` 随之不可达（`bridge.rs:426` 随函数一并消失；`:542` 的兜底本就理论不可达）。需同步：枚举定义与 `#[error]` 文案、4 处测试、以及 `bridge.rs` 模块头对 `spawn_compute` 的契约描述。
- **风险**：若未来确实出现单任务卸载需求，需重新新增 API。**但这是可接受的**——新增一个带真实调用方与测试的 API，成本低于长期维持一个零调用者的公开面。

### 候选 C —— 保留并显式登记为未接线

维持现状，在台账 note 中把 DEFERRED 的理据写全（"单任务卸载暂无生产需求；取消契约因批量入口同步而无生产路径"）。

- **优点**：零改动、零风险。
- **代价**：病灶不变。E3 护栏升门时该行仍无法纳入棘轮（DEFERRED 行存在与否本身未决）；且"保留"这一状态无到期日，与本批要清除的缺陷类同构。
- **风险**：低，但收益也低——它把决策推迟而非做出。

---

## 3. 倾向与理由

**倾向候选 B（删除）**。三条理由，按强度排序：

1. **`Cancelled` 因批量入口的同步语义而无生产路径**。这不是"暂时没接线"，而是**结构上不可达**——取消语义需要 await 边界，而生产卸载面没有 await 边界。保留一个结构上不可达的契约，比删除它更糟：它让读者以为系统具备取消能力。

2. **`panic 隔离`契约已由批量入口覆盖**。逐任务 `catch_unwind` 在批量路径上同样存在，因此删除单任务入口**不损失**任何实际保护。

3. **候选 A 会反向制造调用方**。为一个 API 的存续去找调用方，与本项目"有病灶才建分发面"（ADR-194 §3.1 #4）的既有先例相悖。

**但本 ADR 不自决。** 依 ADR-194 D1，实施侧发起 Claim、委员会 Certify；且删除公开 API 属不可逆面，需显式授权。委员会需裁决的是：是否接受 L1 内部层的一次公开面删除，以换取契约台账少一条长期 DEFERRED。

---

## 4. 若委员会批准候选 B 的实施清单（不在本 ADR 授权内）

1. `bridge.rs`：删除 `spawn_compute`（含其 `ComputeError::Cancelled` 生产构造点）与 4 处测试调用；保留 `spawn_compute_batch`。
2. `bridge.rs` 模块头 §「# 骨架边界」中对 `spawn_compute` 的契约描述改为只描述批量入口。
3. `ComputeError` 枚举：移除 `Cancelled` 变体与 `#[error]` 文案；更新其 4 处测试断言。
4. `compute/mod.rs` 的 `pub use` 如有相关再导出则同步。
5. `scripts/check_dead_contract_apis.py`：删除 `ComputeBridge::spawn_compute` 台账行；若 `scripts/gate_provenance_exemptions.txt` 无相关条目则无需改。
6. 回归：`cargo test -p nexus-core` 0 failed；各已卸点位「并行与串行逐元素一致」断测保持绿（删除面不触及它们，但需确认无连带）。

**行为判据**：删除前后 `spawn_compute_batch` 的 `DispatchPlan` 决策逐条相同（阈值不变 ⇒ 可机器比对）；`nexus-core` 测试数减少量应恰为被删测试的条数，不多不少。

---

## 5. 验收（裁决落地时适用）

| # | 判据 | 验证方式 |
|---|---|---|
| V1 | `spawn_compute` 生产调用点与定义同时消失 | `check_dead_contract_apis.py` 中该行退役；全库大小写敏感检索 0 命中 |
| V2 | `Cancelled` 不可达且已从枚举移除 | `cargo test -p nexus-core` 0 failed；枚举中无该变体 |
| V3 | 批量卸载行为零改变 | 7 个已卸点位的「并行 = 串行逐元素一致」断测全绿；`route()` 决策不变表逐条相同 |
| V4 | 账本同步 | `check_doc_consistency.{sh,ps1}` 与 `check_doc_drift.sh` rc=0；`adr_index_sync.py` rc=0 |
| V5 | 无新增门失配 | `check_gates_provenance.py` rc=0（删台账行不影响豁免名单时） |

---

## 6. 移交委员会的问题（本 ADR 不自决）

1. **Q1**：是否接受候选 B（删除 L1 公开 API）？若否，候选 C 的"未接线"登记应写入何处、由谁维护、是否有到期日？
2. **Q2**：`scripts/check_dead_contract_apis.py` 的 E3 升门（候选 G-86，现由 `gate_provenance_exemptions.txt` 显式豁免）是否在本决议落地后一并推进？
3. **Q3**：`bridge.rs` 模块头把 `reduce` 的"错误变体不适用"与批量入口的 `Cancelled` 兜底并列描述，删除后是否需要一次模块头重写以免残留悬空描述？

---

## 7. 认证登记（2026-10-07；W4-0 T0-4 认证推进；只加注）

经所有者 2026-10-07 明确授权（L0-L3 融合打磨方案 W4-0 T0-4），本 ADR 完成正式认证，头部状态由 Proposed 翻为 ✅ Accepted（裁决 = 采纳候选 B，与 §3 倾向一致，且已于 2026-10-03 执行）。§6 三问逐条回执：

1. **Q1（是否接受候选 B）**：接受。执行证据 = §1 执行状态注（饱和/panic/取消契约测迁入 batch 路径、`check_dead_contract_apis.py` 台账行退役、`Cancelled` 改由 batch None 兜底构造；`docs/reports/cpu-plane-phase0-c0-2026-10-02.md` §8）。
2. **Q2（`check_dead_contract_apis.py` E3 升门）**：候选号已过时（G-86 已被加速器排除门占用）。现行路径 = 与 W4 批 A WI-A1 的契约消费者门（G-91 预期号）互认后转阻塞——并入该批裁决，本 ADR 不预授权。
3. **Q3（`bridge.rs` 模块头残留）**：**已核验关闭**（2026-10-07 现测）：模块头仅保留 `spawn_compute_batch` 契约描述；对 `spawn_compute` 的存量提及（`bridge.rs:64-65` 的 `Cancelled` 注释「oneshot 取消路径已随 `spawn_compute` 退役」）为**如实历史注记**，非悬空描述，无需重写。
