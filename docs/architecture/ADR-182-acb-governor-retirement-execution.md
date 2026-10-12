# ADR-182: acb-governor 生产孤岛退役执行（可达性棘轮净减）

- **状态**: Proposed（待架构委员会/用户终批；`git rm` 删除 + 全量回归须在可提交、clean-disk 环境执行）
- **决策者**: 承 E01 chimera-architect 只读裁决 + 测试体系治理计划 WS-C
- **关联**: ADR-160（可达性棘轮，freeze 只减不增）、ADR-175（死模块删除收编 + 孤岛偿还批次计划，节1 删除先例）、ADR-176（方向 F 厚度决策，本轮"不合并/观察继续"）、ADR-177（mca-gateway GATED 重分类先例）
- **触发**: Phase R 收口与测试体系治理 — Wave 3（孤岛去债务）

## 背景

`check_crate_reachability.py` 自报（2026-09-04 实测）：43 crate = 28 生产可达 + 14 冻结孤岛 + 1 GATED。
`acb-governor` 是 14 孤岛中**唯一"零入边（not even a dev-dep）"**者：

- 生产 `[dependencies]` 指向它的边：**0**（全仓 grep 确认）。
- 源码里出现的 `acb_governor` 仅 2 处，且都是 `quest-engine/src/arbitration.rs:48-49` 的**注释**——
  解释"为何用 L0 nexus-contracts 而**不**依赖 acb_governor::BudgetTier"（ADR-054 决策 3），非真实引用。
- 唯二"装配面"：根 `Cargo.toml` 的 `workspace.dependencies` 声明 + 根 `chimera-e2e-tests` 的
  `[dev-dependencies]`（测试可绕过生产方向，不代表产品装配）。
- 自身资产：`src/`(config/cost_integration/cost_model/downgrade/error/governor/lib/types.rs)、
  `benches/governor_bench.rs`、`tests/`(cost_integration/integration/proptest.rs)。

ADR-175 节1 已确立"零生产消费者 → 删除 → 回归"的 crate/模块级删除先例（4 个模块级死模块，约 2500 行）。
本 ADR 把该模式应用到第一个 **crate 级**退役。

## 决策

退役删除 `acb-governor` 整个 crate，作为"孤岛净减"的第一个可证执行项，达成棘轮
`frozen 14 → 13`（只减不增）。

### 删除清单（blast radius，逐项，缺一即门红）

1. 证据留档：全仓 grep `acb_governor`/`acb-governor` 零**生产**引用落盘到本报告附录。
2. 删目录：`crates/acb-governor/`（src 8 文件 + benches 1 + tests 3）。
3. 根 `Cargo.toml`：`[workspace] members` 去 `"crates/acb-governor"`；`[workspace.dependencies]` 去 `acb-governor = { path = ... }`；根 `[dev-dependencies]` 去 `acb-governor`。
4. `scripts/check_dependency_rules.sh`：`layer_of()` case 表去 acb-governor 行；`layered_crates` 列表去之；`expected_crates` 常量 -1。
5. `scripts/check_dependency_rules.ps1`：`$layerMap` 去 acb-governor；`$expectedCrates` -1。
6. `scripts/check_layer_map_parity.py` 四方互锁随之对齐（members/`.sh`/`.ps1` 三源同步，否则 CI 层图对拍门红）。
7. `scripts/crate_reachability_freeze.txt`：删 acb-governor 行（棘轮只减）。
8. `scripts/bench_inventory_freeze.txt`：删 acb-governor governor_bench 行（若登记）。
9. 文档回写：`CODE_WIKI.md §3.11` 表 + headline、`agents.md`、`.trae/rules/nuxus规则.md §41`、
   `CHANGELOG.md` 的 "28 可达 + 14 孤岛" → "28 可达 + 13 孤岛"（+1 GATED 不变）。
10. 归档引用 `acb-governor` 的其它注释/文档段落一并清理（quest-engine arbitration.rs 注释可保留但改为"历史上曾有 acb-governor，已退役"）。

### 验证（删除协议强制项，缺一不可判"完成"）

- `cargo check --workspace --all-targets` EXIT=0（无悬空引用）。
- `cargo test --workspace`（或至少 `-p` 根 E2E + parliament，去 dev-dep 后其测试仍可编译运行）failed=0。
- `cargo clippy --workspace --all-targets -- -D warnings` EXIT=0。
- `bash scripts/check_dependency_rules.sh` EXIT=0；`bash scripts/check_crate_reachability.sh` EXIT=0 且 `[R] ... unreachable: 13`；`python3 scripts/check_layer_map_parity.py` EXIT=0。
- `cargo machete` 无新增未用依赖告警。

## 否决项

- **否决"登记为已删而未真删"**：ADR-175 明文"登记不等于除名"。本 ADR 的完成态是 freeze 行数净减 + 上述验证全绿，非仅出 ADR。
- **否决跳过全量回归即 `git rm`**：删除协议核心是"先证零消费者→删→回归"，回归步在磁盘门控（G-14 需 ≥20GB）未满足前不得以"看起来没问题"替代。

## 后果

- **正向**：棘轮净化（14→13）；删除 ~2200 src LOC 死代码及其测试/bench 维护税；消除"测试测死代码"价值稀释的一个实证样本。
- **风险与缓解**：crate 级删除牵动 ≥4 静态表 + 全量回归；须在可提交环境执行以获得 git 回滚点；本 ADR 状态为 Proposed，批准方为架构委员会/用户。
- **D-4 联动**：本 ADR 是 ADR-175 批次 D-4（acb/decb/… 在 ADR-176 F 后复核）对 `acb-governor` 的处置落地第一步；decb-governor（仅 parliament dev-dep）可作同类后继候选。

## 执行时机（本会话判定）

本轮 Phase R 收口在途批次**未 commit + 磁盘紧（D 剩 ~13GB）**，删除协议的"全量回归"强制步不可证。
故本会话**只产出本执行 ADR + 完成证据留档 + 精确删除清单**，`git rm` 与全量回归留待：
① 用户/委员会批准本 ADR；② 在可提交、≥20GB 空闲磁盘的环境执行删除 + 全量回归 + 一次 commit 作为回滚点。
这既推进"孤岛净减"的真实终态（ADR + 就绪清单已备），又不以不可验证的破坏性改动污染工作树。
