# ADR-171: 治理登记——T8 mca 空特性装配口径 与 T9 两条接受性伪可达生产边

- **状态**: Accepted（2026-09-02，治理登记；T8/T9 审议终裁）
- **决策者**: E01(首席架构师)提案；用户终裁
- **关联**:
  - ADR-065（MCA 总纲，决策 6：编译期 feature `mca` 双轨隔离 + M4 独立 PR 装配路径）
  - ADR-160（crate 生产可达性棘轮；freeze.txt 只减不增）
  - ADR-161（冻结孤岛分批偿还路线，批次 3 mca-gateway 曾指向本 ADR 的前身编号暂称）
  - `Cargo.toml` `[features] mca = []`（根 package 空特性，2026-08 后无任何 optional dep 挂靠）
  - `crates/crate_reachability_freeze.txt`（本 ADR 对冻结清单追加引用注记）
  - `.trae/rules/nuxus规则.md` §5（P0 红线 12 跨层依赖方向 + ADR-160 冻结孤岛清单机理）

> **编号说明**: 主编号最新为 ADR-170（2026-09-02，收录 Ω₁₀/Ω₁₁），本 ADR 承接顺延为 **ADR-171**，属治理/登记型 ADR（不改代码、只登记口径）。编号权威以磁盘文件名为准。

## 背景

ADR-160 引入 crate 生产可达性棘轮后，15 个冻结孤岛进入 `scripts/crate_reachability_freeze.txt` 只减不增。其中两份待审议题（T8、T9）此前处于悬而未决状态，本 ADR 予以审议并最终登记：

### T8：mca-gateway 空特性装配口径

根 `Cargo.toml` `[features] mca = []` 为**空特性**——**不挂任何依赖、没有任何 optional dep 声明指向 mca-gateway**。mca-gateway 因此在任何消费者 manifest（含 chimera-cli / chimera-tui 等下游 crate）中均**无声明**，仅由 CI 旁路 job（`--features mca`）与 `tests/e2e/` 中被 `#![cfg(feature = "mca")]` 门控的 E2E 测试体编译引用。此姿态与 ADR-065 决策 6"编译期 feature 双轨隔离 + M4 独立 PR 装配"路径一致。

### T9：两条单向生产依赖边

在 freeze.txt 冻结清单基础上，审计发现两条**真实存在的生产依赖边**（非 dev-dep，亦非 optional 边）：

- `mca-gateway → scc-cache`（生产边 L10→L3）：`CacheAffinityIntegration` 依赖。
- `auto-dpo → model-router`（生产边 L5→L1）：`ModelRouterJudgeClient` 依赖。

这两条边方向合法（向下依赖），但其**消费者端**（mca-gateway / auto-dpo）自身均为 feature-gated 孤岛或历史孤岛，故在产品装配图中这些边并不使端点进入"生产可达"。即：边真实存在，但无法经由默认 binary 装配触达——称为"**接受性伪可达生产边**"。

## 决策

1. **T8 终裁——维持 mca-gateway feature-gated 孤岛姿态，以 ADR 登记装配口径**:
   - 维持根 `Cargo.toml` `[features] mca = []` 为空特性现状，**不新增** optional dep 挂靠，mca-gateway 保持无可装配消费者。
   - 维持 freeze.txt 对 mca-gateway 的冻结登记，但追加本 ADR 引用注记。
   - 装配路径沿用 **ADR-065 决策 6**（M4 独立 PR）：由 `chimera-cli` 声明 `optional = true` 依赖 + `"mca"` feature 链，组合根装配调用**列为 M4 独立 PR 计划**，不在本登记周期内实施。
   - 该姿态不与 ADR-160 可达性棘轮冲突（feature-gated 孤立被视为"可见债务"，登记即合规）。

2. **T9 终裁——两条边登记为"接受性伪可达生产边"，不去边**:
   - `mca-gateway → scc-cache`（L10→L3，`CacheAffinityIntegration`）：真实设计依赖，方向合法，保留。
   - `auto-dpo → model-router`（L5→L1，`ModelRouterJudgeClient`）：真实设计依赖，方向合法，保留。
   - 在 freeze.txt 中为两端 crate 条目追加本 ADR 引用注记，标注"接受性伪可达"性质。
   - **不去边**（不删除依赖、不断接线），因两端均为真实代码依赖，移除将破坏实现。

## 否决项

- **否决方案 A：将 mca-gateway 从冻结清单除名（视作已有 `mca` feature 即算装配）**：空特性 `mca = []` 不产生任何真实装配依赖，除名会虚增生产可达数、破坏棘轮口径。否决，维持登记。
- **否决方案 B（T9）：对两条伪可达边执行去除（拆除 scc-cache / model-router 依赖）**：两依赖为真实功能（CacheAffinityIntegration / ModelRouterJudgeClient）所需，拆除属破坏性改动，非本登记型 ADR 职责。否决，仅登记。

## 后果

- **正向**: T8/T9 议题获得 ADR 级明确登记，冻结清单"特征 + 理由 + ADR 引用"自证完整；后续偿还/装配（M4 独立 PR）可直接锚定本 ADR。
- **正向**: 两条伪可达边的真实依赖性质被显式记录，避免未来被误判为"幽灵生产边"或触达性错误扩容。
- **无负向**: 本 ADR 不修改任何代码，仅作登记口径；freeze.txt 判定（不除名、不去边）不变，`scripts/check_crate_reachability.sh` 不新增报警。

## 验证

- [x] 根 `Cargo.toml` `[features] mca = []` 为空特性确认（无 optional dep 挂靠）
- [x] freeze.txt 四 crate 条目（mca-gateway / scc-cache / auto-dpo / model-router）已追加本 ADR 引用注记
- [x] T11 孤岛偿还批次一登记在 freeze.txt 注释区完成（详见 freeze.txt T11 批次一登记块）
- [x] `scripts/check_crate_reachability.sh` 判定不变（不除名、不去边，无新增报警）