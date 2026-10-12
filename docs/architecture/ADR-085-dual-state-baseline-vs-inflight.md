# ADR-085:文档双态治理——已发布权威基线 vs 在途开发

> **🕊️ 收编解决标注 (2026-08-20 追加)**:本 ADR 定义的"已发布权威基线(v2.26.0-omega 136)vs 在途
> (v2.27.0-omega 144)"双态,已随 **v2.27.0-omega(2026-08-19)正式发布**合并:权威口径升级为
> **v2.27.1-omega(144 NexusEvent · 10836 tests)**,在途 8 事件并入 `CODE_WIKI.md §5.5/§5.6`,
> `check_doc_consistency.ps1` GAP-F2 随收编自然消除。本 ADR 保留为**历史治理记录**(双态语义、
> 收编契约、诚实数据原则仍有效);v2.27.1-omega(2026-08-20)为 GPG 补发 + MCA E2E 超时加固,无新增 ADR。
>
> - **状态**:Accepted(2026-08-19 治理签署,由 `check_doc_consistency.ps1` GAP-F2 实况触发)
- **日期**:2026-08-19
- **决策发起方**:文档生命周期治理(`trae-remote-official:staff-engineer-mode` 的
  `documentation-lifecycle` 专家 + `superpowers` + `praxis` + `brooks-lint` 分布式深度分析)
- **关联**:`docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md`、
  `scripts/check_doc_consistency.ps1`、`docs/architecture/CODE_WIKI.md §5.6`、
  `docs/reports/phase10-cross-layer-closure-report.md`、`CHANGELOG.md [Unreleased]`
- **影响架构层**:文档层(全部 .md),含 SoT 权威源(CODE_WIKI / adr_index / INDEX /
  nuxus规则 / CHANGELOG)与巡检工具

## 背景(Context)

2026-08-19 进行全代码库 + 全部 md 计划文档的深度一致性对账时,`scripts/check_doc_consistency.ps1`
持续报告 **`[GAP-F2]`**:`CODE_WIKI.md` 声明 **136** 个 NexusEvent 变体,而脚本以
`crates/event-bus/src/types.rs`(工作区)实测为 **144**。

深挖后确认冲突根源是**文档校验工具的"真值口径"**与**文档内容的"发布口径"**不一致:

1. **已提交权威基线(HEAD)** = `v2.26.0-omega`:38 crates · **136 NexusEvent** · 9954 tests ·
   85 ADR。`CODE_WIKI` / `adr_index` / `INDEX` / `nuxus规则` / `CHANGELOG` 均已与 HEAD 对齐,
   **文档本身是准确的**。
2. **工作区在途开发(未提交)** = Phase 10 §16 跨层协同闭环审计修复(W1-W7,目标 **v2.27.0-omega**):
   `event-bus` 新增 8 个 NexusEvent(`AssessmentUpdated` / `BusThroughputReported` /
   `ErrorSignatureMatched` / `ParentSelected` / `SecurityInterceptionReported` |
   `StopRulingIssued` / `TokenLedgerRecorded` / `VariantApproved`),136 → **144**;另新增
   `experience_loop`/`quest_loop`/`variant_subscriber`/`long_term_credit`/`memory_sync_hook`/
   `interception_stats` 等模块。

`check_doc_consistency.ps1` 的 F 类对账以**工作区代码为真值**,因此在批次在途开发提交前必然
误报 GAP-F2。若盲目把文档改成 144,则把**未提交、未验收**的在途代码错误固化为"已发布权威基线"。
这正是"文档病历之根":**校验工具与文档权威之间缺少对"进行中"状态的显式表达**。

## 决策(Decision)

### 一、双态口径定义(核心裁决)

1. **权威基线(Authoritative Baseline)**:跟随 `git HEAD` 与已发布版本存在的最终事实。
   - 权威源:Cargo.toml `workspace.package.version`、`GHANGELOG.md` 最新已发布条目。
   - 事件/类目计数以**已提交代码 + metadata() 双重验证**为准(当前 v2.26.0-omega = **136**)。
   - 变更需 ADR 治理签署 + 版本发布(per `DOCUMENT_LIFECYCLE_POLICY` §6)。

2. **在途开发(In-flight,⏳ 进行中)**:位于未提交工作区、未经验收的开发。
   - **不得**并入权威基线声明(不污染"已发布"语义)。
   - **必须**独立落入专属章节并标注 `⏳ 进行中` + `目标版本` + `尚未提交、未验收`。

3. **双态并存规则**:任一文档若同时存在权威基线与在途差异,**两份声明必须并存且明确互斥标注**:
   - 权威口径行:固定当前发布版本与计数;
   - 在途小节:列出推进中的变更,明确指向未提交代码位置,并标注发布时收编进权威口径。

### 二、`check_doc_consistency.ps1` 对账语义

4. **`[GAP-F2]` 分类重定义**:F2 仅在权威口径(已提交)与文档声明不一致时判定为"必须修复的缺陷"。
   当差异完全源于未提交在途开发时,F2 标记为 **`[F2-INFLIGHT]`(预期状态,非缺陷,不阻断)**。
   - 实现建议:脚本从 `git diff --name-only` 检测 `crates/event-bus/src/types.rs` 是否被修改;
     若其在未提交集合中,则将事件变体计数差异按在途处理。(当前版本脚本未实现该分支,
     故提交前会持续报 GAP-F2——此为**预期现象**,详见下文"结果"。)

5. **提交-发布-收编契约**:
   - 在途批次**提交**后,权威基线随版本升级,该批次事件并入 CODE_WIKI §5.5 与 §2.1 权威口径;
   - 此时 F2 差异自然消除,GAP-F2 从"预期"转"必须为对齐项"。

### 三、文档落账规范(本次已落地)

6. **CODE_WIKI.md 新增 §5.6**(本 ADR 决策 1-3 的示范落地):列出 8 个在途事件,
   含域归属 / 发布方→订阅方 / Wave,并明确"权威口径保持 136"。
7. **CHANGELOG.md 新增 `[Unreleased] v2.27.0-omega(在途)` 节**:记录 W1-W7 变更,
   发布时收编为正式版本条目。
8. **doc-roles-matrix.md**:基线 v2.20→v2.26、ADR 64→85,登记在途报告为 `⏳ in-progress`。
9. **nuxus规则.md §1.4 / INDEX.md 头部**:追加在途提示,指向 CODE_WIKI §5.6
   与 phase10 报告。

### 四、诚实数据原则(不伪造"已完成")

10. 对于在途开发的"✅"状态,**仅**表示"工作区已装配",**不**等同于"已提交发布"。
    验收以 `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`
    全绿 + 提交为准。任何无真实数据源的度量(L10 用户满意度 / L4 误拦截率 /
    RLTrajectory 下游训练消费)**禁止实施伪造采集**(v4.0 预留)。

### 备选与否决

| 备选 | 否决理由 |
|---|---|
| 把文档计数直接改成 144(跟随工作区) | 把未提交、未验收的在途代码固化权威基线,违反"发布语义";`architecture-audit` 失实的病历重演 |
| 删除/隐藏在途文档,等发布再补 | 提交前无文档追踪,计划-代码-文档脱节,评审与审计失去上下文 |
| 仅改脚本口径为"不校验 136/144" | 失去对账价值;正确做法是对差异**分类标注**而非关闭校验 |
| 为每个在途事件单独建 ADR(v2.27 再并) | 过度建模;本 ADR + CODE_WIKI §5.6 + CHANGELOG 已足够承载,事件随发布收录 |

## 结果(Consequences)

- **正确性**:文档权威口径保持 v2.26.0-omega(136 NexusEvent · 9954 tests · 85 ADR),
  与已提交 HEAD 完全一致;在途 8 事件独立落账于 CODE_WIKI §5.6,诚实反映"进行中"。
- **巡检语义**:本次更新后 `check_doc_consistency.ps1` 仍依赖工作区代码报 GAP-F2
  (脚本未实现 in-flight 分支)。按本 ADR 决策 4,该差异属**预期状态**,通过 CODE_WIKI §5.6
  与 CHANGELOG `[Unreleased]` 显式声明;待 v2.27.0-omega 提交验收后收编并自然消除。
- **可追溯**:双态差异有权威落账点(CODE_WIKI §5.6 / phase10 报告 / CHANGELOG Unreleased),
  任何 agent 或评审者能定位"权威 vs 在途"的分界与收编时机。
- **回归面**:除 §5.6/`[Unreleased]`/矩阵等新增小节外,既有权威声明零改动,历史文档保持只读。

## 风险与回滚

- **风险:脚本持续报 GAP-F2(直至在途提交)**——预期现象已文档化;若团队将 GAP-F2 视为硬阻断,
  可先行实现本 ADR 决策 4 的 in-flight 分支(检测 types.rs 未提交修改)。
- **风险:长期不提交导致在途文档膨胀**——按 DOCUMENT_LIFECYCLE_POLICY §6 触发器(b),
  在途批次应限期收敛;若 W1-W7 久未提交,应拆站为 `docs/architecture/_archive/` 或在
  CHANGELOG Unreleased 标注"已搁置"。
- **回滚**:若本治理决策被否定,可删除 §5.6 与 CHANGELOG `[Unreleased]` 节,恢复单一
  "以工作区为真值"的口径(但将重新引入权威误标风险)。

---

> **历史归档提示**:本 ADR 记录 2026-08-19 全库一致性对账的治理结论;对账过程与证据详见
> `docs/reports/phase10-cross-layer-closure-report.md` 与 CODE_WIKI §5.6。