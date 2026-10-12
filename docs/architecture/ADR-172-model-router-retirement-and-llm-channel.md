# ADR-172: model-router 显式退役与生产 LLM 通道唯一真值源声明

- **状态**: Accepted（2026-09-03，方向 A 模型调用面统一诊断终裁）
- **决策者**: 架构批次（spec: `.trae/specs/map-arch-refactor-directions`）E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-160（crate 生产可达性棘轮；freeze.txt 只减不增）
  - ADR-171 T9（`auto-dpo → model-router` 接受性伪可达生产边）
  - `scripts/crate_reachability_freeze.txt`（model-router 冻结登记）
  - `docs/reports/arch-direction-A-llm-surface-2026-09-03.md`（诊断证据报告）

> **编号说明**: 承接 ADR-171（2026-09-02）顺延为 **ADR-172**，属决策/登记型 ADR（本次仅文档、清单与注释动作，零生产代码改动）。

## 背景

诊断（arch-direction-A 报告,2026-09-03）确证**生产图中不存在任何真实 LLM/HTTP 调用路径**:

1. **生产零 LLM 调用证据**:对 `crates/*/src`(排除 tests/benches)grep `reqwest|http|post|openai|anthropic|api_key` 零命中;唯一携带真实 `reqwest` 的 `mca-gateway/src/transport.rs`(POST JSON + 重试/熔断/限流/域名白名单/env API key)处于无消费者 feature-gated 冻结态。`chimera-cli` `/llm` 命令(`chat.rs:580`)为"待真实 API 接入"占位;`pvl-layer/src/verifier.rs:236` 用本地二元启发式产出 `PredictionVerified` 喂经验循环;ComputeBridge 为纯 CPU 池(禁 LLM)。
2. **model-router 为纯路由契约、空转**:仅 `route→model_id`(`router.rs:215`)与抽象 `ModelProvider` seam(不内置厂商适配器),自身无 HTTP/凭据。生产唯一入边为孤岛 auto-dpo(`ModelRouterJudgeClient`);quest-engine 仅 dev-dep 消费。
3. **model-router 与 mca-gateway 功能重叠**:路由/亲和/成本/健康多处语义重复(arch-direction-A 报告 A3 节);真实通道能力在 mca-gateway 已实现未装配。

## 决策

1. **显式退役 model-router 的"生产路由契约"地位(悬挂声明)**:不删除 crate(保留其契约源码与孤岛 E2E,避免破坏 auto-dpo/quest-engine 的 dev-依赖与孤立测试),但**在生产链路上的地位归零并书面声明**:生产 LLM 调用面 = 无(现状)或未来 mca-gateway,model-router 不参与。
2. **登记 mca-gateway 为唯一真实 LLM 通道真值源**:任何未来生产 LLM 接线必须锚定 mca-gateway(经 ADR-065 决策 6 / ADR-171 T8 的 M4 独立 PR 路径),**禁止并行建设第二条通道**;model-router 的退役声明即"阻断双通道建设"的治理闸。
3. **保持 model-router 在 freeze.txt 的冻结登记不动**:其行注释维持 ADR-160/171 记录(只减不增原则;退役=登记理由完善,不除名)。冻结清单行为由 ADR-175 批次统一管理。
4. 在 `docs/reports/arch-direction-A-llm-surface-2026-09-03.md` 落地诊断证据(已完成),后续任何"CAF 已落地"类声明须以本 ADR + 该报告为准,不得表述为生产可达。

## 否决项

> ⚠ **2026-09-21 现状注记：本节"否决直接删除 model-router 源码"已被后续实施推翻（决策记录本身不改写）**。
> 事实：v2.29 批次 A（提交 `38c72e6`，见 `docs/reports/arch-refactor-final-report-v2.29.0.md`）**物理删除了 `model-router` 与 `auto-dpo` 两个 crate**，二者已不在 `Cargo.toml` 的 `workspace.members`（实测 41 成员，`git show 94499b4:Cargo.toml` 的 members 段仍含二者）。
> 本 ADR 原否决理由（"破坏 auto-dpo/quest-engine dev 链路与既有契约测试"）中，`auto-dpo` 一侧亦随其自身被删除而失效；`quest-engine` 侧的 dev 引用在同批被移除。
> **治理缺口（待实现）**：截至本注记，**没有任何 ADR 记录本项否决被推翻**——`model-router` 只有 ADR-172（否决删除）、`auto-dpo` 无专属退役 ADR（仅 ADR-175 死模块清单与批次报告）。按 ADR-160 三条偿还路径的"退役须 ADR 记录"口径，应补一份 ADR 或在 ADR-172 上追加 supersedes 决议；本审查只登记不代拟。

- **否决方案 ①(接线 model-router 进组合根)**:生产无调用点可迁(证据 1),为接线而接线会新增空转装配面。
- **否决方案 ②(契约迁至已接线 crate)**:生产 LLM 能力实现在 mca-gateway(feature-gated),"已接线 crate"承载路由契约为远期,非本轮。
- **否决直接删除 model-router 源码**:破坏 auto-dpo/quest-engine dev 链路与既有契约测试,且与 ADR-160"可见债务"姿态相抵触。

## 后果

- **正向**: LLM 调用面断裂点书面化;未来通道唯一化(mca-gateway)获得治理闸;防"双通道漂移"。
- **正向**: "CAF 已落地"失实表述被权威终结,文档治理口径统一。
- **无负向**: 零生产代码改动;freeze.txt 不增不减;可达性棘轮不报警。

## 验证

- [x] `docs/reports/arch-direction-A-llm-surface-2026-09-03.md` 已落地且含 ≥3 生产路径证据点(A1)
- [x] freeze.txt model-router 行未改动(登记保持)
- [x] `scripts/check_crate_reachability.sh` 判定不变(不除名、不去边)

---

## 附：退役执行追认与 supersedes 决议（2026-10-07 追加；W4-0 T0-5 / GAP-1；只加注，不改写上方决策文本）

依上文「2026-09-21 现状注记」登记的治理缺口（model-router 物理删除无 superseding ADR；auto-dpo 无专属退役 ADR），现按 ADR-160 三条偿还路径之「ADR 记录」口径，于本 ADR 追加追认决议：

1. **supersedes 声明（对「否决直接删除 model-router 源码」）**：v2.29 批次 A（提交 `38c72e6`；证据 `docs/reports/arch-refactor-final-report-v2.29.0.md`）对 `model-router` 的物理删除**正式取代本 ADR 否决项 ③ 的 model-router 一侧**。追认理由：其生产路由契约地位已由本 ADR 决策 1 归零（零生产入边，唯一入边为同批删除的 auto-dpo）；空转契约的物理删除与 ADR-193 追认先例（router-traits / lsct-tiering）同构。**本 ADR 决策 1/2/3/4（mca-gateway 唯一 LLM 通道 / freeze.txt 登记 / 诊断证据口径 / 双通道阻断闸）全部继续有效。**
2. **auto-dpo 退役追认**：同批次物理删除的 `auto-dpo`（L5 Knowledge）自本注记起获得正式退役记录。依据：组合根口径零生产入边（ADR-160 棘轮体系）；其删除后的引用残留已由 ADR-190 原子清理（bench_check.yml 4 引用面 + `perf_redlines` 幽灵项）。事件契约面按 ADR-188 D1 口径保留并登记（`DpoPairGenerated` 等仍在 `crates/event-bus/src/types.rs`），不因 crate 退役而删事件。原承载于 auto-dpo 的 RHI-CG 通道 A（`RhiChannelA`）随删除退役、通道 B（gsoe-evolution `CiGate`）留存——该能力面收窄的后果以批次 A 报告（`docs/reports/arch-refactor-final-report-v2.29.0.md`）为准。
3. **登记面同步（2026-10-07）**：`scripts/retired_crates.txt` 已含 `auto-dpo` / `model-router` 两行（本追认不新增登记）；该表尚缺 `router-traits` / `lsct-tiering`（ADR-193 批次），补登归 W4-5 WI-G14。`adr_index.md` 顶部注记的「待实现：补一份 ADR 或追加 supersedes 决议」随之闭合。