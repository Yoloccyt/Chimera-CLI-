# ADR-161: ADR-160 冻结孤岛的分批偿还路线与批次化棘轮门禁

* 状态：**Proposed（待架构委员会终批）** — 起草 2026-08-31（Phase R R1-T11）
* 关联：ADR-160（可达性棘轮，本 ADR 承接其"三条偿还路径"承诺）、ADR-145/146/148（刻意预留）、ADR-065 决策 6（feature 双轨装配）、ADR-048（跨层特批先例）、`docs/architecture/CODE_WIKI.md §3.11`
* 编号说明：ADR 主编号已用至 160（`docs/architecture/adr_index.md:4`），本 ADR 取 161；旧稿"自 ADR-135 起编号"作废（135~160 已全部占用）。

## 背景

ADR-160 建立了生产可达性棘轮并如实登记：43 crate 中 **28 可达 / 15 冻结孤岛**（`scripts/crate_reachability_freeze.txt` 实测 15 行），同时给出三条偿还路径（接组合根 / `optional`+feature / 保留并记录理由）。但**没有给出偿还顺序、批次边界与各批门禁**——"有债册、无还债计划"本身就是下一轮"零 Stub ≠ 已装配"的温床。

组合根唯一性已核实：`scripts/check_crate_reachability.py:36` 定义 `ENTRY_POINTS=("chimera-cli",)`，`crates/chimera-cli/Cargo.toml:106-108` 是全仓唯一发布 bin。因此"L10 → 任意下层"恒为向下依赖，**接入动作本身不可能违反方向铁律**；真实风险在别处（见决策 4）。

## 决策

1. **三批次偿还，按"合规风险 × 收益"排序，而非按层级**：
   * 批次 1（10 个，路径 ① 接组合根）：`model-router`(L1)、`scc-cache`(L3)、`lsct-tiering`(L3)、`sesa-router`(L6)、`ssra-fusion`(L7)、`csn-substitutor`(L10)、`chtc-bridge`(L10)、`nexus-hook`(L9)、`decb-governor`(L8)、`acb-governor`(L8)。判据：出边全部向下或同层，且不携带 `reqwest` / `tract-onnx` 等重量级传递依赖进默认产物。
   * 批次 2（3 个）：`auto-dpo`(L5)、`gea-activator`(L9)、`mtpe-executor`(L7)。理由：`auto-dpo` 经生产边 `auto-dpo/Cargo.toml:15` 连带转正 `model-router`（故必须排在批次 1 之后才划算），且触 R2 冻结边界 → 需 **ADR-165** 前置；`mtpe-executor` 经 `nmc-encoder` 携带 `tract-onnx`，须重新量体积红线。
   * 批次 3（2 个）：`mca-gateway`(L10) 走路径 ②（`optional` + `mca` feature，需 **ADR-162/163**）；`mas-sched`(L9) 维持冻结，锚 ADR-145（其事件面**尚未实现**，强接即重演"注释自述门禁 > 源码引用"反模式）。
   * 终态目标：freeze 15 → **1**（仅 `mas-sched`），`[GATED]` 轨道含 `mca-gateway`。
2. **每批四重 Go/No-Go**（缺一即 No-Go）：
   * `scripts/crate_reachability_freeze.txt` 行数按批递减且**只删不加**；`python scripts/check_crate_reachability.py` EXIT=0，零 `[GAP-R]` / `[STALE]`；
   * `cargo tree -p chimera-cli -e normal` **逐个断言**该批 crate 出现在装配面（`-e normal` 天然排除 dev-dep，证明进的是装配面而非测试面）；
   * `bash scripts/check_dependency_rules.sh` EXIT=0（43/43 层图 + Check A-D）；
   * `cargo build --release -p chimera-cli` 产物 **< 50 MB** 且 `bash scripts/check_cli_startup.sh` 启动耗时无回退（基线值在批次开工时实测定点，未测定前不得宣称"无回退"）。
3. **接线姿势：构造参数注入，禁 feature 门控**（先例 `docs/reports/phase4-wave4-closure.md §4`：`ExecPolicyToolExecutor::new` / `SubAgentQuestEngine::new` / `HookEventBridge`）。每 crate 一个 commit，允许 crate 粒度 revert。
4. **freeze.txt 的回登记例外**：revert 某批接线时允许把对应行加回，但必须与 revert **同 PR**，否则棘轮的 `[GAP-R]` 判红。理由：棘轮若不允许 revert，等于用"清单只减不增"绑架工程质量——正确性优先于清单单调性。
5. **事件生产者补全不新增变体**：`ModelRouteSelected` / `CacheHit` / `CacheMiss` / `CsnSubstitutionTriggered` 等由零生产者转真实发布，全部为 Normal 级；`crates/event-bus/src/bus.rs:42,49` 的 `CRITICAL_MPSC_VARIANTS=13` / `CRITICAL_TOTAL=17` 与 `LANE_FORBIDDEN_SHARD` 断言分毫不动。若任一批次需要新增 Critical 变体，立即触发本 ADR 重开并单独走 ADR-164+。
6. **本 ADR 不批准任何跨层直引例外**：`mca-gateway/src/lib.rs:15` 自陈"仅依赖 L0/L1"，但 `mca-gateway/Cargo.toml:40,44,50` 实测存在三条对 L2/L3/L6 的生产边。该矛盾属依赖铁律例外范畴，交 **ADR-163** 独立裁决，不因批次 3 的 feature 门控而默认放行。

## 备选项与否决理由

* **一次性全量接线 15 个**：否决。体积红线（`reqwest`/`tract-onnx` 入默认 bin）、`[GAP-C]` 闸门在海量改动中漏检概率上升、`mas-sched` 事件面未实现强接即造假、组合根 fan-out 骤增致 bisect 灾难。
* **继续全部冻结**：否决。50,604 src LOC 永久不可达，`[ROTATE]` 机制永不触发，"只减不增"退化为"零偿还能力"，且 11,556 全绿继续掩盖"从未接线"这一原始病灶（ADR-160 立规理由）。
* **按层级从 L1 往上接**：否决。层级顺序与"收益 × 风险"无关；`model-router` 虽在 L1 但因解锁 `auto-dpo` 连锁而优先，`mca-gateway` 虽在 L10（最高层，接线最"安全"）却因跨层直引与强制 `reqwest` 反而最后做。

## 后果

正面：装配面从 28 → 38（批次 1+2 后），"零 Stub"与"已装配"第一次在批次粒度上可核验；每批四重门禁使还债进度可被 CI 度量而非靠叙述。

代价与约束：默认产物体积与启动耗时成为**新的常态化红线**（每批必测）；被转正 crate 的 dev-dep E2E 连带转正，nextest 机时上升；`auto-dpo` 的 R2 边界必须在批次 2 前书面澄清，否则批次 2 卡死。

## 验证

批次 1 完成的判据即决策 2 的四重门禁全绿，外加：`docs/reports/phaseR-wave*-closure.md` 记录 freeze 行数变化前后值、`cargo tree -e normal` 断言输出、binary 体积实测数（非门禁目标值）与启动耗时基线对照。
