# ADR-315: 门禁清单域化与 govd `exclusive` 资源互斥调度（W-T0/W-T1）

- 状态: Accepted（W-T0/W-T1 已实施；W-T2 = CI 拓扑翻转未做，前置为「W-T1 连续 7 日绿」，归 ADR-316）
- 日期: 决策与落地 2026-11-16（`f38021fc`，2026-11-16T13:03:03+08:00）；本文件落档 2026-11-18
- 批次: W-T0（BC-11 清单域化）+ W-T1（BC-13 legacy timeout / BC-14 资源互斥链）
- 决策者: NEXUS-OMEGA Team
- 决策明细载体: `docs/reports/toolchain-parallel-scheduling-wt0-wt1-2026-11-16.md`（本文件为独立物理档，二者互为索引）
- 关联提交: `f38021fc`（W-T0/W-T1 主体）、`9600b12b`（登记 ADR-315 明细载体）、`ffd71390`（第二轮批次收口报告入册）、`58497ba2`（L1-B 波次追加 G-161/G-162，门数演进至 159，**不属本 ADR 变更**）
- 关联: ADR-319（工具链并行调度伞；D2 承袭本 ADR 的"清单喂料"路线）、ADR-316（govd 提为 CI 阻塞面）、ADR-300 D4（禁止并发 cargo）、ADR-310 D6（台账补登）、ADR-222 D6（台账与代码同批）、ADR-228 §3.6（翻转门槛）、ADR-235 S1（词表收口先例）

---

## D1 — 背景：调度器有能力，清单没喂料

`scripts/run_gate_manifest.py`（legacy 执行器）与 `scripts/govd/`（并行调度器）
长期只消费清单的 6 个必需键（`scripts/govd/manifest.py` 的 `REQUIRED`：
`id` / `spec_ref` / `cmd` / `expect` / `kind` / `note`）。由此产生三个病灶：

1. **互斥靠"声明"，不是靠"执行"**：清单虽有 `needs` 字段，但
   `scripts/gate_manifest.toml` 头注实测记录——`needs` 只买得到
   **入列顺序**；`jobs>1` 时执行环把 pending 整批 submit 进线程池，
   全程不再读 `needs`。阳性/阴性对照锁在
   `scripts/govd/tests/test_govd.py::NeedsSemanticsTests`。
   ⇒ 多个 cargo 类 heavy 门可同时编译同一 workspace，导致
   `target/` 内同 crate rlib 哈希分叉与 `E0460/E0462/E0463/E0786`
   （`scripts/govd/model.py` 的 `Task` 字段注释、ADR-300 D4）。
2. **清单无法表达资源域**：没有任何字段能说明"这道门占 CPU 还是 IO"、
   "它的并发上限是多少"、"哪些门绝不能重叠"。调度器即使具备能力也无从得知。
3. **legacy 路径会挂死**：`scripts/run_gate_manifest.py` 的 legacy 分支调用
   `subprocess.run` 时**不传 `timeout=`**，而清单/`govd/policy.py::timeout_for`
   早已声明 300s（light）/1800s（heavy）——超时语义只存在于并行引擎，
   串行路径上一道挂死的门会把整轮跑批无限期卡住。

⇒ 决策方向：**清单喂料，不重写调度器**（R12 §6.2 路线 C 已否决并行实现），
把调度语义显式写进唯一权威面 `scripts/gate_manifest.toml`，由 govd 消费。

---

## D2 — 决策

### D2.1 W-T0：清单域化（每门 5 个调度字段）

- `scripts/gate_manifest.toml` 每门注入：
  `group`（并发作用域）/ `concurrency`（域内并发上限）/
  `resource`（宿主资源类）/ `exclusive`（互斥键）/ `needs`（保留，前置门）。
  > `needs` 是本批之前**已存在**的字段，本批只是把它纳入域化模型；
  > **`ci_exempt` 不是本批注入的调度字段**——它是既有的 CI 豁免键，
  > 现取 `Select-String -Path scripts/gate_manifest.toml -Pattern '^ci_exempt'`
  > = **20 处**，只出现在部分门；全部门都带的是 `group` / `concurrency` /
  > `resource`（各 159 处，见 D4 取数）。
- **域划分判据：按 `cmd` 里的脚本名正则分 9 桶**，不按 `note` / `spec_ref`
  ——`note` 是自由散文会漂，`spec_ref` 是工单号与职责域无对应，`cmd` 里的脚本名
  是稳定标识（`scripts/gate_domainize.py` 的迁移判据）。
- 域分布（现取口径见 D4；9 个在用域，合计 159 门）：
  `lint-static` 36 / `l0-contract` 35 / `doc` 20 / `dep-layer` 18 /
  `tool-plane` 15 / `governance` 13 / `security` 8 / `perf` 7 / `rust-build` 7。
  7 个 heavy 门全部落在 `rust-build` 域，`concurrency=1`。
- 门数：**156 → 157**（新增 G-160 = exclusive 互斥负控门），
  light=150 / heavy=7（`f38021fc:scripts/gate_manifest.toml` 现取）。

### D2.2 W-T0：schema 门同批扩展（方案假设被证伪）

- 方案 BC-11 假设「G-138 schema 门不设白名单 ⇒ 新增字段零成本」——
  **实测证伪**：`scripts/check_manifest_schema.py` 的 `KNOWN_KEYS` 确为白名单，
  不扩展将触发 **472 项 `UNKNOWN_KEY`** 红（B3 提交说明实测）。
- 同批处置：
  - `KNOWN_KEYS` 加入 `group` / `concurrency` / `resource` / `exclusive`；
  - 新增闭集词表 `GATE_GROUPS`（10 个域，含当前零使用的 `misc`）与
    `GATE_RESOURCES`（`cpu` / `io` / `net`）；
  - 新增类型与词表校验 + **5 条负例夹具**（`--selftest` **39 断言**全持）。
  - **WHY 用闭集**：开放词表会让拼写错误静默变成新域（域计数漂移且无门报警）。

### D2.3 W-T1：`exclusive` 资源互斥链（per-key，而非全局锁）

- `scripts/govd/model.py::Task` 增加 `group` / `concurrency` / `resource` /
  `exclusive` 四字段（`needs` 原已存在）。
- `scripts/govd/cli.py` 的实现分两步：
  1. 先按 `exclusive` 键分组：每个键一条链，**同键链内严格串行**；
  2. 键外任务保持并行，按 `group` 的 `concurrency` 上限运行。
- **WHY 不做全局锁**：全局锁会把并行收益归零——本仓绝大多数门无 `exclusive` 键。
- **WHY `exclusive` 而不是 `needs`**：`needs` 只买入列顺序，不是互斥锁
  （`model.py` 与 `cli.py` 的就地注释都记录了实测结论）。
- 现取互斥键分布：`exclusive = "cargo"` 共 **4 门**（全部为 cargo 类门）。

### D2.4 W-T1：manifest 解析 fail-closed

- `scripts/govd/manifest.py::_task_from` 对 4 个新字段做类型与范围校验，
  非法值抛 `ManifestError`（退出 2），**不静默取默认**。
- **WHY**：清单写 `concurrency=0` 若被静默接受，调度器的并发上限即失去意义，
  且没有任何错误浮现。

### D2.5 W-T1：legacy 路径 timeout 补齐（BC-13）

- `scripts/run_gate_manifest.py` 的 legacy 分支：
  门自带 `timeout_s` 优先，缺省 `light=300s` / `heavy=1800s`
  （与 `scripts/govd/policy.py::timeout_for` 对齐）。
- 新增 `subprocess.TimeoutExpired` 分支：记为 **`unknown`（不可判定）**，
  既不视为通过，也不视为红——超时是"判据未产出裁决"，不是绿。

### D2.6 W-T1：负控门 G-160（证明并发断言有牙）

`scripts/check_exclusive_mutex_negative.py`：

1. 复制 `scripts/govd` 到临时目录；
2. 变异副本 `cli.py`：`if t.exclusive:` → `if False:`（移除互斥判定）；
3. 以 `scripts.govd` 包名加载**副本**；
4. **断言被测 `cli.__file__` 指向副本**（防真实包覆盖副本导致变异从未生效）；
5. 期望：互斥断言变红（区间 `(1,3)` / `(2,4)` 重叠），其余三条保持绿。

> 首版以 `import govd.cli` 加载副本，与被测的 `from scripts.govd import cli`
> 成了两套模块实例 ⇒ 真实包覆盖副本 ⇒ 变异从未生效 ⇒ 负控假失败。
> 修正为包路径 + `__file__` 守卫后 `EXIT=0` 成立。该踩坑记录本身被写进脚本
> 头部注释，作为后续负控门的模板。

### D2.7 同批同步（ADR-222 D6）

| 台账 / 门 | 变更 |
|---|---|
| `scripts/gate_manifest.toml` | 156→157 门；每门 +5 字段 |
| `scripts/check_manifest_schema.py` | `KNOWN_KEYS` +4；`GATE_GROUPS` / `GATE_RESOURCES` 词表；5 条负例夹具 |
| `scripts/tool_registry.toml` | 登记 `gate_domainize.py` 与 `check_exclusive_mutex_negative.py`；补登 ADR-310 D6 遗留的 10 个 `check_l0_*` 子执行体 |
| `scripts/gate_provenance_exemptions.txt` | +11 条豁免 |
| `.github/workflows/ci.yml` | G-147 之后新增 G-160 步骤 |

**WHY 两批合并为一个 commit**：新门（脚本 + 清单条目 + CI 步骤 + 工具登记 +
provenance 豁免）**互为前提**——任缺其一，G-137 parity 与 G-149 provenance 立即
报"脚本有 disposition 缺口 / 清单有门无 CI"。ADR-222 D6 明文规定
"门辖区与被改实体互为前提 ⇒ 中间态必红 ⇒ 必须同 commit"。

---

## D3 — 后果

### 正面

- **真互斥**：共享 `exclusive` 键的门在时间上永不重叠；键外任务仍并行，
  并行收益不被全局锁吞掉。
- **可调度的声明面**：域 / 并发上限 / 资源类 / 互斥键全部在清单里可检索，
  新增门只需按模板声明，不需要改调度器。
- **挂死消除**：legacy 路径获得与 govd 一致的超时语义，超时判不可判定而非绿。
- **断言有牙**：G-160 的变异负控证明 `ExclusiveSemanticsTests` 的并发断言
  不是恒真。

### 负面与破例说明

1. **方案假设被证伪（不是破既有规则，而是破方案原文）**：
   BC-11 假设"schema 门不设白名单"，实测 `KNOWN_KEYS` 确为白名单。
   处置不是删门或放宽校验，而是**同批扩展白名单 + 新增闭集词表 + 负例夹具**。
   - **precedent**：ADR-235 S1（`TEST_KINDS` 词表收口，此前只校验"是字符串"
     导致 4 行写成不存在的 `single_crate` 而全库无门发现）；
     ADR-310 D6（台账漏登记以补登收口，不删门）。
2. **清单体量上升**：`f38021fc` 内 `scripts/gate_manifest.toml` 改动
   +793 / −308 行（`git show f38021fc --stat` 现取），主要来自每门 5 字段的
   注入。这是可读性成本，用域化换取调度语义的可执行性。
3. **`needs` 成为零使用字段**：现取 `^needs` = **0 处**。本 ADR 不删除它
   （删除属破坏式清单变更，需独立裁决）；ADR-319 D3 已登记 W3a 删除规格书，
   前置是在途 govd 测试线落地。
4. **门数漂移**：本 ADR 落地时 **157 门**；现取 **159 门**
   （`58497ba2` 的 L1-B 波次追加 G-161/G-162 = `check_l1_dead_surface.py`
   主门与 selftest）。**新增门沿用 5 字段模型，但不是本 ADR 的变更**——
   引用门数必须现取，不得把 157 当现势。
5. **W-T2 未做**：govd 提为 CI 阻塞执行面（ADR-316）的前置是"W-T1 连续 7 日绿"，
   本次不满足 ⇒ 不绕门、不提前翻转。

---

## D4 — 回滚

- **原子回滚**：`git revert f38021fc`（12 个文件同一提交，含 CI 步骤与 G-160）。
- **回滚的顺序约束（必须遵守）**：`58497ba2` 追加之 G-161/G-162 已使用
  `group` / `concurrency` / `resource` 三字段，而本 ADR 的 revert 会同时撤销
  `check_manifest_schema.py` 的 `KNOWN_KEYS` 扩展 ⇒ **若直接 revert，schema 门
  会以 `UNKNOWN_KEY` 转红**。正确路径：
  1. 先把 G-161/G-162 的调度字段摘除（或同时 revert 引入它们的提交）；
  2. 再 `git revert f38021fc`。
- 回滚后 govd 仍可按旧清单运行（新字段有默认值），但互斥语义随之消失——
  cargo 类 heavy 门恢复为"可以并发编译"的原状态，属预期。
- 无不可逆点：全部为 git 跟踪文件，历史完整保留。

---

## D5 — 证据索引

| 事实 | 文件 + 符号（行号仅辅助） |
|---|---|
| 调度字段模型 | `scripts/govd/model.py` + `Task.group` / `Task.concurrency` / `Task.resource` / `Task.exclusive`（:54-71） |
| 必需键集合 | `scripts/govd/manifest.py` + `REQUIRED`（:10） |
| fail-closed 解析 | `scripts/govd/manifest.py` + `_task_from`（:52-67） |
| 互斥链实现 | `scripts/govd/cli.py` + `run_exclusive_chain` / `exclusive_groups`（:363-426） |
| legacy 超时 | `scripts/run_gate_manifest.py` + `timeout_s` 分支（:585-608） |
| 超时缺省对齐 | `scripts/govd/policy.py` + `timeout_for`（:206） |
| schema 白名单与词表 | `scripts/check_manifest_schema.py` + `KNOWN_KEYS` / `GATE_GROUPS` / `GATE_RESOURCES`（:26-46） |
| 负例夹具 | `scripts/check_manifest_schema.py` + 域化字段负例（:351） |
| 负控门 | `scripts/check_exclusive_mutex_negative.py` + `WHY 需要这个门（ADR-315 W-T1...`（:4） |
| 清单头注（needs ≠ mutex） | `scripts/gate_manifest.toml` + 「needs」字段说明（:14-22） |
| 域化迁移工具 | `scripts/gate_domainize.py` + 「调度器有能力，清单没喂料」（:8） |
| 门条目示例 | `scripts/gate_manifest.toml` + `G-01`（`group="perf"` / `concurrency=4` / `resource="io"`）（:31-40） |
| G-160 条目 | `scripts/gate_manifest.toml` + `G-160`（:1938 附近 `spec_ref`） |
| 明细载体 | `docs/reports/toolchain-parallel-scheduling-wt0-wt1-2026-11-16.md` |

**取数命令（本文所有数字的来源，全部于 2026-11-18 现取）**：

```powershell
# 门数现取（当前 159；本 ADR 落地时为 157）
(Select-String -Path scripts/gate_manifest.toml -Pattern '^\[\[gate\]\]').Count
git show f38021fc:scripts/gate_manifest.toml | Select-String -Pattern '^\[\[gate\]\]' | Measure-Object
# 域分布（9 域，合计 159）
Select-String -Path scripts/gate_manifest.toml -Pattern '^group = "([^"]*)"' |
  ForEach-Object { $_.Matches[0].Groups[1].Value } | Group-Object | Sort-Object Count -Descending
# 互斥键分布（exclusive="cargo" 4 门）
Select-String -Path scripts/gate_manifest.toml -Pattern '^exclusive = "[^"]+"' | Group-Object Line
# 字段覆盖（group/concurrency/resource 各 159；needs 0；ci_exempt 20）
(Select-String -Path scripts/gate_manifest.toml -Pattern '^concurrency = ').Count
(Select-String -Path scripts/gate_manifest.toml -Pattern '^resource = "[^"]+"').Count
(Select-String -Path scripts/gate_manifest.toml -Pattern '^needs').Count
(Select-String -Path scripts/gate_manifest.toml -Pattern '^ci_exempt').Count
# heavy / light 分布
(Select-String -Path scripts/gate_manifest.toml -Pattern '^kind = "heavy"').Count
(Select-String -Path scripts/gate_manifest.toml -Pattern '^kind = "light"').Count
```
