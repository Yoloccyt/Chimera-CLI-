# 工具链并行调度重构 — W-T0 / W-T1 批次报告（ADR-315）

> 批次：W-T0（BC-11 门禁清单域化）+ W-T1（BC-13 legacy timeout / BC-14 govd 资源互斥链）
> 决策归属：**ADR-315**（无独立物理文件，按 `docs/architecture/adr_index.md` 索引行登记，
> 决策明细载体为本报告）。
> 日期：2026-11-16
> 分支：`refactor/l0-w4-compute-extract`

---

## 1. 范围与不做什么

**做**：

- 给 `scripts/gate_manifest.toml` 的每一门注入 5 个调度字段
  （`group` / `concurrency` / `resource` / `exclusive` / `needs`）；
- 让 govd 调度器按 `exclusive` 资源键实现**真正的互斥**；
- 给 legacy 执行路径补上 timeout 传递；
- 给并发断言加**变异负控门**（证明断言非恒真）。

**不做**（沿方案 §1.2 减法表）：

- 不重写 govd 调度器（R12 §6.2 路线 C：清单喂料而非重写调度器，已否决并行实现）；
- 不用 `needs` 表达互斥（实测 `needs` 只买得到**入列顺序**，不是互斥锁）；
- 不把 govd 提为 CI 阻塞 job（W-T2，前置是「W-T1 连续 7 日绿」，本次未满足 ⇒ 未做）。

---

## 2. 域化（W-T0）

### 2.1 分类判据：按 cmd 脚本名正则分桶，**不**按 note / spec_ref 关键词

WHY：`note` 是自由散文（会随人工编辑漂移）；`spec_ref` 是工单号（与职责域无对应）。
cmd 里出现的脚本名是**稳定标识**——脚本改名必同步改清单 cmd，桶归属随之稳定。

### 2.2 域划分（9 域）

| 域 | 门数 | 域 | 门数 |
|---|---|---|---|
| `lint-static` | 35 | `l0-contract` | 33 |
| `doc` | 20 | `dep-layer` | 18 |
| `tool-plane` | 15 | `governance` | 12 |
| `security` | 8 | `perf` | 7 |
| `rust-build` | 7 | | |

7 个 **heavy** 门全部落在 `rust-build` 域（`concurrency=1` + `resource=cpu`）。

### 2.3 门禁数：156 → **157**（新增 G-160 = exclusive 互斥负控门）

`light=150 / heavy=7`。

### 2.4 ⚠️ 方案假设被证伪：G-138 schema 门**确实设白名单**

方案 BC-11 假设「schema 门不设白名单 ⇒ 新增 5 字段兼容」。实测
`scripts/check_manifest_schema.py` 的 `KNOWN_KEYS` **确设白名单** ⇒ 不扩展会触发
**472 项 `UNKNOWN_KEY` 红**。

**同批处置**：扩展 `KNOWN_KEYS` 加 4 个新字段（`needs` 本已在清单里），新增
`GATE_GROUPS`（10 域闭集）与 `GATE_RESOURCES`（cpu/io/net 词表），并补类型与
词表校验（`GROUP` / `CONCURRENCY` / `RESOURCE` / `TYPE`）+ **5 条负例夹具**。
schema 门 selftest **39 断言全持**。

---

## 3. govd 资源互斥链（W-T1，BC-14）

### 3.1 为什么必须是 `exclusive` 而不是 `needs`

实测：`needs` 在 `jobs > 1` 时**只决定入列顺序**（执行环整批 submit、全程不读
needs）⇒ 它买不到互斥。互斥必须由**调度器按资源键**实现。

### 3.2 实现：per-key 分组，而非全局锁

- 按 `exclusive` 键分组：**同键链内严格串行**；
- **不同键之间、以及无键任务，保持并行**。

WHY 不做全局锁：全局锁会把并行收益归零（本仓 157 门里绝大多数无 exclusive 键）。

### 3.3 fail-closed 解析

`scripts/govd/manifest.py::_task_from` 对 4 个新字段做**类型与词表校验**，非法值
抛 `ManifestError`（退出 2），**不静默取默认**。
WHY：清单里写 `concurrency=0` 若被静默接受，调度器的并发上限就失去意义且无任何
错误浮现。

### 3.4 legacy 路径 timeout 补齐（BC-13）

`scripts/run_gate_manifest.py` 的 legacy 路径此前**不传 timeout**（门可无限挂死）。
现在：门自带 `timeout_s` 优先，缺省 light=300s / heavy=1800s（与 govd
`policy.timeout_for` 对齐），并新增 `subprocess.TimeoutExpired` 分支记为
`unknown`（**判不可判定**——既不视为通过，也不视为红）。

---

## 4. 负控门 G-160（证明并发断言有牙）

`scripts/check_exclusive_mutex_negative.py`：

1. 把 `scripts/govd` 复制到临时目录；
2. 变异副本的 `cli.py`：`if t.exclusive:` → `if False:`（**移除互斥判定**）；
3. 以 `scripts.govd` 包名加载**副本**；
4. **断言被测 `cli.__file__` 指向副本**（防真实包覆盖副本导致变异从未生效）；
5. 期望：互斥断言**变红**、其余三条保持绿。

**踩坑记录（方法论价值高于结论）**：首版负控以 `import govd.cli` 加载副本，而被测
测试用 `from scripts.govd import cli` ⇒ **两套不同模块实例**，真实包覆盖了副本，
**变异从未生效**，负控假失败。修正为 `scripts.govd` 包路径 + `__file__` 副本守卫后，
变异后互斥断言精确变红（区间 `(1,3)` / `(2,4)` 重叠），其余三条保持绿。

⇒ 负控门 `EXIT=0` 成立。

---

## 5. 门禁与台账的同批同步

| 项 | 变更 |
|---|---|
| `scripts/gate_manifest.toml` | 156 → 157 门；每门 +5 字段 |
| `scripts/check_manifest_schema.py` | `KNOWN_KEYS` +4 字段；新增 `GATE_GROUPS` / `GATE_RESOURCES` 词表；新增 5 条负例夹具 |
| `scripts/tool_registry.toml` | 登记 `gate_domainize.py`（一次性迁移工具）与 `check_exclusive_mutex_negative.py`（负控门）；另补登 ADR-310 D6 遗留的 10 个 `check_l0_*` 子执行体 |
| `scripts/gate_provenance_exemptions.txt` | +11 条豁免（10 个 ADR-310 薄调度器子执行体 + `gate_domainize.py`） |
| `.github/workflows/ci.yml` | G-147 步骤后新增「exclusive mutex negative control (G-160)」步骤 |

**ADR-310 D6 遗留的补登**：那 10 个 `check_l0_*` 脚本经 `check_l0_surface.py:53-62`
委托调用，**不是孤儿脚本**，此前只是漏登记（provenance 门方向 C 会报）。
本批补登后 provenance 三方向全绿。

---

## 6. 验证记录（实测）

| 门 | 结果 |
|---|---|
| `py -3 scripts/check_manifest_schema.py --selftest` | 39 断言全持 |
| `py -3 scripts/check_manifest_schema.py` | [OK] manifest schema valid (157 gates) |
| `py -3 scripts/check_ci_manifest_parity.py` | [OK] CI and manifest agree（workflows=5 / ci_refs=163 / manifest_gates=157 / matched=69 / unmatched=0 / gate_missing=0） |
| `py -3 scripts/check_gates_provenance.py` | 三方向全绿（173 脚本各有归属） |
| `py -3 scripts/govd/tests/test_govd.py` | 90/90 通过（含 `ExclusiveSemanticsTests` 4 条特征锁） |
| `py -3 scripts/check_exclusive_mutex_negative.py` | EXIT=0（变异后互斥断言精确变红） |
| `py -3 scripts/check_workflow_yaml_lexical.py` | findings=0（12 files） |

---

## 7. 遗留

1. **W-T2（CI 拓扑改造，BC-12）**：前置是「W-T1 连续 7 日绿」，本次未满足 ⇒ **未做**。
   届时的双向对拍要求（`G-137` parity）已在 W-T0 后保持绿。
2. **W-T3（并发度实测标定）**：方案目标值 `jobs=8`（CI pr 档 4→8）**待实测标定**
   【待验证】。本机实测：15.2GB RAM 下 8 并发会使 nextest 假红（默认并发 535s
   且每轮轮换 1 条假红），本地须保持 4 ⇒ 未改 `policy.toml`。
3. **W-T4（编译面去重，可选）**：未做，需先有 C-3 裁决与 W-T3 的实测基线。
