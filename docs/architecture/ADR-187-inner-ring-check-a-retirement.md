# ADR-187：内环界 Check A 退役（Q3 α′ 裁决）与十层依赖律优先级定序

- **状态**：✅ Accepted（2026-09-24，用户批准《Chimera 零影响架构收敛设计》方案后落地；实现为纯治理 + 脚本，零生产 Rust 码）
- **取代/裁决**：本 ADR 落地 `Q3-adjudication-inner-ring-gate-2026-09-21.md` 推荐的 **α′ 方案**，并裁决 `agents.md §2.2` 十层依赖律与 ADR-054 决策 4 内环边界 Check A 的规范冲突。
- **编号选取（遵循 `ADR-XXX-inner-ring-bug-fix.md` 头部取号规则与 `ADR-188` §编号选取的实测记录）**：
  186 归 **M13 decb/scc 重路由批次**预留（`CODE_WIKI.md` 注记），187 归 **Q3 内环界 α′ 裁决草案**
  （`gate_manifest.toml` G-39 `spec_ref` 原即指向 ADR-187），本 ADR 据此落 **187**，与 `ADR-188`
  取号记录一致，无撞号。M13 保留 186 预留，G-38 `spec_ref` 已同步由 "ADR-186" 修正为 "ADR-187"。
- **关联**：ADR-054（三环重组，决策 2/4）· ADR-048（gqep→qeep 向下边先例："接受现状 + 显式登记"）·
  ADR-033（L0 契约层）· ADR-084（Phase 6 W0 层图订正）· ADR-185（双源重新等价即收编证据）·
  ADR-186（M13，同号段共存但语义无关）· ADR-188（装饰岛退役前置治理，本 ADR **不触碰**其范围）
- **作废指针**：`ADR-XXX-inner-ring-bug-fix.md` 所记录的 2026-09-18 `.ps1` "FIXED ... ADR-XXX" 变更
  是占位号、未入 `adr_index.md`、且只改非 CI 侧 —— 本 ADR 宣告其**不构成有效治理变更**，予以取代。

---

## 1. 背景与实测事实（当轮现取，非引用旧文档）

### 1.1 冲突本体

`gsoe-evolution`(L5) → `decay-engine`(L4) 这条**向下边**同时被两套规则判定相反：

| 规范 | 出处 | 对该边的裁决 |
|------|------|--------------|
| 十层依赖律 `L(N)→L(N-1) ✓` | `agents.md §2.2` | **允许**（`crates/gsoe-evolution/Cargo.toml:22` 据此自辩） |
| 内环边界 Check A | ADR-054 决策 2/4，实现于 `check_dependency_rules.sh` | **禁止**（decay 属 L4 外环，不在内环 base/whitelist） |

CI 实跑体是 `.sh`（`ci.yml:89` / `gate_manifest` G-08），其 Check A 无层比较、直接 fall-through
禁止该边 ⇒ 依赖铁律门**当前为红**。

### 1.2 三条决定性判据（本会话逐条独立复验为真）

1. **环分区在代码中从未存在**：`grep -rn "CoreLoopEvent" crates/**/*.rs` → **0**；内环/外环唯一
   强制点是治理脚本名单（`is_inner_ring` / `is_inner_base`）。用未实现的设计目标否决现行法允许的边，方向反了。
2. **该边是只读 fail-closed 安全前置读**：`gsoe-evolution/src/engine.rs:255 breaker.is_tripped()` /
   `:260/:268 breaker.trip_cause()`（均 `&self`，无副作用、非热循环）。事件化会引入异步取数失败路径，
   等于把安全门改成 fail-open —— 与 ADR-054 决策 3 对 `parliament→seccore` 否决事件化的理由同源。
3. **Check A 已无判别力，且其"修复"落在不执行的一侧**：2026-09-18 把 `.ps1` Check A 改成
   `depLayer > crateLayer ∧ depLayer ≥ 2`（Check B `depLayer > crateLayer` 的**真子集**），使 `.ps1` 侧
   Check A 逻辑死亡；`.sh`（CI 侧）未同步 ⇒ 双源对同一 Check 判出**相反结论**，`agents.md` 曾宣称的
   "双源均 EXIT=0" 自此半真半假。

### 1.3 "变绿不是放宽"的可机检自证（非修辞）

selftest 夹具 `mlc-engine`(L2) → `scc-cache`(L3) 是**向上**边，删 Check A 断言后 **GAP-B 仍命中同一条边**。
⇒ Check A 从未拥有任何"只被它抓到"的负控样本，退役净损失 **0 个可检测违规类**。本会话实测：退役后
`.ps1 --SelfTest` 输出 GAP-A=0、GAP-B=2（该边由 GAP-B 捕获），对拍门 `check_gate_predicates_parity.py`
由红转绿。

---

## 2. 决策

1. **退役 Check A（α′ 核心）**：从 `check_dependency_rules.sh` 与 `.ps1` **双侧**删除 Check A 循环及其
   唯一支撑表 `is_inner_ring` / `is_inner_base`（`.ps1`：`$innerRing` / `$innerBase`）。十层依赖律
   （`agents.md §2.2`）优先级 **高于** ADR-054 内环分区。
2. **环分区归位为 NOT-ENFORCED 意图**：`agents.md §2.2` 分区意图表保留，但每行强制标注
   `STATUS: NOT-ENFORCED (ADR-187)` / `ENFORCEMENT: none`，与 §3.4.5 的 5 份前瞻蓝图同栏共存，
   不再被当作可执行法。
3. **登记 gsoe→decay 边**：在 `scripts/dep_edge_freeze.txt` 以"L5→L4 向下、合法"理由显式登记（沿用
   ADR-048 "接受现状 + 显式记录"先例），使装配面边棘轮由 8 NEW → 7 NEW。
4. **判据对拍门转正**：`check_gate_predicates_parity.py` 的 G-38/G-39 由设计性红 `expect=1` 翻 `expect=0`；
   `P3|Check A` 分歧随 Check A 退役自然消除；余 4 条 arity 差异按语言惯例继续登记
   `gate_predicate_baseline.txt`（只减不增）。
5. **重开三条件（齐备方可让 Check A 复活）**：`CoreLoopEvent` 变体进入 `event-bus/src/types.rs` +
   B 侧内环延迟 criterion 基线入 CHANGELOG + 内环 crate 之间出现真实共享态容器。**缺一不得复活**。
6. **净增外环→内环 only-decrease 棘轮门**（补上 ADR-054 另一半此前无门的检测缺口）：
   新建 `scripts/check_outer_inner_ratchet.py` + `scripts/outer_inner_ratchet_baseline.txt`
   （gate_manifest **G-75/G-76**，已接入 `ci.yml`）。它冻结“外环非装配根 crate → 内环 crate 的**直接**
   生产边”（这类是**向下**边，Check B 只拦向上边永远看不到），基线 = 当前实测 **9** 条
   （`chimera-mas`→6 + `faae-router`/`mca-gateway`/`mtpe-executor` 各 1），组合根
   `chimera-cli`/`nexus-app-server` 按 Check E 同一口径豁免。**非 Check A 复活**（那条管内环→base）；
   只减不增，不强制现有边归零，与环分区 NOT-ENFORCED 定位兼容。⇒ 门的总数与强度**净增**。

## 3. 否决方案（各附理由，供后人不重蹈）

- **α（裸删环分区文档）**：丢弃仍具指导价值的三环重组设计意图，过度。α′ 归位而非删除。
- **β（把 decay-engine 提入 `is_inner_base`）**：把 L4 安全层塞进内环 base，概念稀释且同类冲突必复发。
- **γ（事件化该边）**：见 §1.2 判据 2，降低安全性（fail-closed → fail-open）。
- **δ（契约下移 L0）**：`ShadowModeCircuitBreaker` 持 `Option<EventBus>` 并 `publish_blocking`，是带副作用
  的状态机；`nexus-contracts` 明文禁依赖任何 workspace crate，下沉只能"为绕过闸门而虚构 trait 接缝"，
  属过度工程（决策 3 当年下沉的是本就存在的契约，情形不同）。
- **外环→内环「must-be-zero 硬阻塞」门（原 §2.6 设想的最强形式）**：**否决该最强形式，采纳其弱化版 = only-decrease 冻结棘轮，已落地（见 §2 第 6 项）**。
  理由：① 基线数不可复现 —— 本会话实测"外环→内环非装配根直边"豁免组合根后实测 **9** 条
  （`faae-router→gsoe-evolution`、`mtpe-executor→nmc-encoder`；`chimera-cli/chimera-mas/mca-gateway/
  nexus-app-server` 为组合根/宿主门面须豁免，`repo-wiki→nmc-encoder` 属内环内部边），与 Q3 报告写的"15"
  在干净定义下不符；② 在把环分区**同时**降级为 NOT-ENFORCED 的当口新立一道**硬阻塞**门自相矛盾；
  ③ 本机 bash 不可执行（WSL 拒绝、Git/MSYS cygwin signal pipe error 5），无法对新增 CI 阻塞门做本地
  验证，贸然塞入违背"零 CI 影响"。⇒ Check A 退役的正当性由 §1.3 机证独立成立（不依赖此新门）；
  （外环→内环棘轮已以 only-decrease 冻结形式落地为 §2 第 6 项；纯 python 静态门，本会话实跑绿，基线由 --emit 从同仓生成 ⇒ 本地==CI。早期“仅 2”初估漏计 chimera-mas/mca-gateway，已订正为 9）。

## 4. 后果

**正面**：依赖铁律门由红转绿；双源 `.sh`/`.ps1` 谓词重新等价（对拍门绿）；文档不再把未实现的设计意图
当法；`gsoe→decay` 边从"未登记 NEW"转正。
**代价**：失去一道名义上的内环界门（但其判别力已塌缩为 Check B 子集，实质损失 0）；外环→内环方向暂无
硬门（与退役前一致，非新增损失）。
**范围边界**：本 ADR **不触碰** 7 条 `chimera-cli→装饰岛` NEW 边（属 Q2 / ADR-188 B3/B4 批次），
也不裁决其去留；仅保证登记 `gsoe→decay` 一条。

## 5. 验证（本会话已执行，可复跑）

```bash
python scripts/check_gate_predicates_parity.py            # 期望 EXIT=0（P3|Check A 消除）
python scripts/check_gate_predicates_parity.py --selftest # 期望 EXIT=0（10 检测器）
python scripts/check_layer_map_parity.py                  # 期望 EXIT=0（层表四源对拍未受影响）
python scripts/check_dep_edge_freeze.py                   # gsoe→decay 消失；余 7 岛边 NEW（Q2 在途）
# .sh 侧本机 bash 不可执行，由 .ps1 同构 + 对拍门交叉验证；CI 上 bash scripts/check_dependency_rules.sh 为终判
```

## 6. 受影响清单

- `scripts/check_dependency_rules.{sh,ps1}`：删 Check A + 内环表 + selftest GAP-A 期望 + OK 消息。
- `scripts/dep_edge_freeze.txt`：登记 `gsoe-evolution|decay-engine`。
- `scripts/gate_manifest.toml`：G-38/G-39 `spec_ref`→ADR-187、G-39 `expect 1→0`、★注释转过去时。
- `agents.md`：§1.2 删"双源均 EXIT=0"、§2.2 环表标 NOT-ENFORCED、§3.4.5 Phase 9 交付改述。
- `docs/architecture/adr_index.md` / `CODE_WIKI.md §11`：ADR-187 落档，186/187 撞号调和记录。
