# ADR-219: Python 治理工具面健康门（G-121/G-122）——判 Rust 的 120 道门自身无人判

> **Status**: ✅ Accepted (2026-10-16)
> **决策日期**: 2026-10-16
> **关联 ADR**: ADR-198（`.ps1` BOM 与门自证纪律）/ ADR-167 决策 4（三处命令串逐字等价）/ ADR-033 追加节与 ADR-212 决策 3（CLAIM 化头注的直接先例）/ ADR-218 D-44（「让产出方自己报数」范式）
> **关联交付**: `docs/reports/PY-TOOLING-HEALTH_2026-10-16.md`（十维探针读数 + 逐条取证 + 自我更正记录）
> **权威源**: 代码实测（`scripts/*.py` 92 个、`.github/workflows/*.yml` 11 个、`git ls-files --eol`）

---

## 1. 背景

本仓有 120 道机器门判 Rust 代码的健康度，但**实现这些门的 Python 自身没有任何门**：
`grep -riE "ruff|flake8|black|mypy|pylint|pyright"` 在 `.github/workflows/` 与
`scripts/*.toml` 全域 **0 命中**，而 Rust 侧有 fmt + clippy + typos + taplo 四把。

这个不对称在本轮审计中已造成三类可复现缺陷（全部实测，非推测）：

| 症状 | 证据 | 为何只有 Python 侧会这样 |
|---|---|---|
| 8 处模块级死导入长期无人拦 | AST 扫描（带植入正对照）报 8 处 / 7 文件；逐条 `grep -c` 现证「全文仅出现 1 次且就在 import 行」后才删 | 无 lint |
| 3 个治理 `.sh` 在本地是 CRLF，`bash scripts/check_doc_drift.sh` 死在 `$'\r': command not found`，而同一条门在 Linux CI 绿 | `git ls-files --eol scripts/` = 89 `w/crlf` / 86 `w/lf` / 3 `w/mixed`，而 blob 侧 **100% `i/lf`** | 无 `.gitattributes` ⇒ 行尾由每个开发者本地 `core.autocrlf` 与文件来源（checkout vs 工具写）共同决定 |
| 跑门的解释器随 runner 镜像浮动 | 4 个 job 调 python3 且 4 个都没钉版本；9 个脚本 `import tomllib`（3.11+），其中含门清单执行器 `run_gate_manifest.py` | Rust 侧已为同形问题钉死 1.97.1（ci.yml:50-53 的 WHY 写明 `-D warnings` 会被新版 clippy 新 lint 突发红），Python 侧是同一个病未治 |

**关键约束**：91→92 个脚本经 AST 扫描确认**零 PyPI 依赖**（所谓非标准库 import 全是
`scripts/` 内同目录兄弟模块：`gate_rc` ×64、`check_l0_purity` ×7 等）。因此
(a) 钉解释器不会断任何门的依赖；(b) 新门**自己也不能**引入第三方库——CI 钉定的
3.11 解释器没有 site-packages，而开发机常有（本机 `yaml` 可导入即为实证），
用 `import yaml` 写的门会「本地绿、CI 崩」，正是本 ADR 要消灭的类别。

## 2. 决策

### 决策 1：新增 `scripts/check_py_tooling_health.py`，初版判据五条（G-121），自证（G-122）

> 第二轮（§7）把判据从 5 条扩到 **7 条**（新增 H6 绝对路径 / H7 环境失败退码），
> 本节保留初版形态以留痕演进；下表五行即初版。

| 轴 | 判据 | 违规 | 不可判定（rc 2） |
|---|---|---|---|
| H1 COMPILE | 每个 `scripts/*.py` 必须能 `compile()` | 点名文件 + 行号 | — |
| H2 DEADIMPORT | 模块级 import 的名字必须在文件中被引用 | 点名 `文件: 行: 名` | — |
| H3 REPOEOL | `git ls-files --eol scripts/` 不得出现 `i/crlf` / `i/mixed` | 点名文件 | git 不可用 |
| H4 FLOOR | 解释器低于 `PYTHON_FLOOR=(3,11)` 时拒绝判定 | — | 退 2（绝不「旧解释器上跑绿」） |
| H5 PINPARITY | 任何调用 python 的 **job** 必须钉版本，且所有钉值 == FLOOR | 点名 `workflow: job: 实钉值` | — |
| 地板 | `PY_FILES_FLOOR=80` / `WORKFLOW_FLOOR=8`（实测 92 / 11） | — | 低于地板即「扫描致盲」⇒ 退 2 |

H5 取 **job 粒度**而非文件粒度：文件级检查会让「在未钉的 job 里新增 python 步骤」
静默通过——而这正是本次缺口的成因。

### 决策 2：H2 的豁免必须是「有理由的签字动作」

`# side-effect-import: <理由 ≥24 字符>` 才豁免；标记在但理由不足 ⇒ 照判红。
长度阈值与 `MIN_EXEMPT_REASON_LEN` 直接复用 `check_l0_purity.py`，不另立口径。
**理由**：裸标记会成为新隐身位；`import gate_rc` 这类全靠导入时机生效的写法确实存在。

### 决策 3：不引入 ruff/mypy 等外部工具

否决反方案 A「pip install ruff 入 CI」：它会 (a) 把 PyPI 供应链引入本以为是
stdlib-only 的门面，与 `deny.yml`/`audit.yml` 只管 Cargo 的既有口径错位；
(b) 需要网络，而本仓有工具安装受限于网络的实录；(c) 规则集远大于当前实测缺陷面，
首轮会产出上百条待裁决项，把「补牙」变成「搬家」。
采纳反方案 B 的最小版本：用 stdlib `ast` 只判**已被证明存在过的**五类缺陷，
基线现值即 0 违规 ⇒ 本门**无需任何豁免台账就能以阻塞态上线**（最干净的准入形态）。
将来出现 `ast` 判不动的类别（如类型错误）再另立 ADR 引入工具链。

### 决策 4：`.gitattributes` 只做前瞻，不做顺带重写

`/scripts/** text eol=lf` + 全域 `*.sh`。因这些位置 blob 本就是 LF，声明不会触发
renormalize 风暴（实测新增后 status 项数不变、内容差异 0/0）。**明确不在本批执行**
`git add --renormalize .`——那会一次性重写盘上 89 个 CRLF 工作副本，属全有动作，
须人签字。因此 H3 判的是 blob/CI 视图（真正影响判定一致性的那一侧），
工作树视图由 `.gitattributes` 前瞻收敛。

### 决策 5：钉 Python = 3.11，四 job 同值

`actions/setup-python@v5` + `python-version: '3.11'` 加入 ci.yml/check、
bench_check.yml/bench-check、coverage.yml/coverage、slo.yml/slo-release-gate。
取下界 3.11 而非更高：下限由 `tomllib` 决定，上限取本机已验证值（3.11.9，L0 全门 rc=0），
使 CI == 开发端。slo.yml 原话「浮动 stable 会让红线随 runner 镜像漂移」对读数判定脚本同样成立。

## 3. 验证（全部用 CI 自己的口径，不自立标准）

```
py -3 scripts/check_py_tooling_health.py --selftest   # 19 条断言，植入正负双向
py -3 scripts/check_py_tooling_health.py              # 实况 rc=0
actionlint .github/workflows/*.yml                    # 本地恰为 CI 钉的 1.7.12，rc=0
zizmor . --min-severity=high                          # 逐字取自 zizmor.yml，rc=0
py -3 scripts/check_gates_provenance.py               # G-103 方向 B：新门已登记
py -3 scripts/audit_gate_parity.py                    # expect=0 且已接 workflow
```
实况读数：`py_files=92 imports=452 eol_rows=178 workflows=11 python_jobs=4` 零违规。

## 4. 本轮由该门/探针自曝的实现缺陷（先于接线被抓，全部记录）

1. **`from __future__ import annotations` 被判死导入**——首版没排除 `__future__`
   指令，8 个真实文件因一个「绑定的名字天然不被引用」的编译器指令而红。
   修复 + 新增同名植入**负例**夹具（该假阳是夹具当初没覆盖的形态）。
2. **`judge()` 在 TOOL-ERROR 分支直接 print**，破坏了「返回 verdict、由调用方打印」的
   孪生门约定，后果是自检看不见自己的不可判定消息（一条断言因此假失败）。改为 append。
3. **`WORKFLOW_FLOOR` 声明了但未接线**——恒不触发的地板等于没有地板。补进 `judge()`。
4. **H3 分类器有两份真值**（`repo_eol` 内联判断 vs 自检用的 `_eol_row_is_bad`）
   ⇒ 改为共用同一函数，否则夹具能绿而生产误读。
5. **python 调用检测只认 `python3 `**，会漏 `python x.py`，又会把 WHY 注释里提及的
   `python3` 误判为调用 ⇒ 改为逐行、跳注释、`python3?\b` 双分支，并各加一条夹具断言。

另一处属于**上一批**的缺陷已在本轮闭环：`check_l0_logic_ledger.py` 的自检断言把
一次实测的登记名数焊成 `len(names) == 17`，P1-REG（17→28）后 HEAD 上 G-120 红
（ADR-218 记为 F-10 / T-1）。修复为结构不变量 `len(resolved)==len(names) 且无歧义注记`，
并把 manifest note 里抄死的「29 条断言」改为指向 `--selftest` 自报——本仓第三次
同形病灶，处置一律「让产出方自己报数」（ADR-218 D-44 范式）。

## 5. 非缺陷登记（防止下一轮错误清理）

- **26 个未接进 CI/manifest 的脚本不是死资产**：每一个至少被 1 份 `docs/reports/*.md`
  引用（`nc_*` ×8 植入对照、`lru_*` ×4、`t2_*` ×2、`recover_*` ×2 等），是「报告里的
  数字可复现」的证据链。真孤儿数 = **0** ⇒ 本轮不做任何删除，也不得由下一轮顺手删。
- `derive_l0_extensibility_evidence.py` / `adr_index_sync.py` rc 恒 0/2，是刻意
  的报告型/豁免型，已在 `gate_provenance_exemptions.txt` 登记，不属 H5 缺口。
- 93 处「文本模式写文件未指定 `newline=`」判 **P3 不动**：blob 全 LF、CI 读 LF、
  Python 文本模式对 CRLF 透明，实际风险低；为一个低风险项改 28 个正在工作的门脚本，
  回归风险大于收益。H3 已经在判那个唯一要紧的视图（blob）。

## 6. 回滚

逐笔独立可逆（`git revert <sha>`）：
G-121/G-122 是四件同批（脚本 + manifest 两条 + ci.yml 一步），须同退，否则
G-103 方向 B（带 `--selftest` 未登记）或 `audit_gate_parity`（expect=0 未接 workflow）
必红。`.gitattributes`、Python 钉版本、死导入清理三笔互不耦合，可单独退。

## 7. 第二轮（同日深度审查）：新增 H6/H7 两条判据，并记录门自身踩到的三个陷阱

第一轮的门只判"能不能跑"，第二轮才判"写对了没有"。深度探针（AST，逐轴植入正负对照）
扫完 92 个脚本后，判据从 5 条扩到 7 条，另修 3 处实测缺陷：

| 新增 | 判据 | 使它成形的实测缺陷 |
|---|---|---|
| H6 ABSPATH | 模块级赋值 value 里不得出现绝对路径字面量 | `classify_fmt_vs_semantic.py:18` 把 `ROOT` 钉成 `D:\Chimera CLI` ⇒ **被报告引用的那个数字只能在本机复现** |
| H7 ENVFAIL | `if r.returncode != 0` 不得导向 `sys.exit(1)` | `lru_facts.py:14`、`lru_reachability.py:20` 把环境失败记成判红；`run_gate_manifest.py` 用 `ok = rc == expect` 裁决，借 1 即 gate_rc F33 的假绿通道 |

两处定级细节决定了这两条轴能不能用，记下来免得下轮重犯：

- H6 只扫**模块级赋值的 value**。第一次扫全量常量得到 14 个假阳，全是 `m::alpha`
  这类夹具键被 `^[A-Za-z]:` 误抓；加上"盘符后必须紧跟分隔符"后降到 1 个真缺陷。
- 仓内三处合法形态（`os.path.isdir` 守卫、`os.name == 'nt'` 守卫、selftest 夹具）
  都在函数体内，因此"只看模块级"这一约束天然把它们排除，无需白名单。

### 本轮由门与探针自曝的三个陷阱（都被固化成断言或纪律）

1. **字符类 `[\\/]` 静默退化成 `[\/]`（只匹配 `/`）**，使绝对路径轴对唯一的真缺陷
   `D:\...` **永久致盲**，并报告"0 命中"。分隔符判断改为下标比较 `s[2] in ("/", "\\")`，
   并把该回归固化为一条断言。
2. **门判自己红**：H6 第一版把本门的 `ABS_HINTS = ("/home/", "/opt/", ...)` 裸前缀
   元组判成 4 条违规。规则修正为"裸前缀常量是匹配器、只有其后还有实际路径段才算钉死默认值"。
   自指假阳是完备性门的固有职业风险，不修就会诱导后人把整条轴关掉。
3. **回滚自身失败**：一次补丁用 `io.open(...).write()` 未关句柄，Windows 上令
   `git checkout HEAD -- <file>` 报 `unable to unlink old ... Invalid argument`，
   留下一个"内容有效但 TOML 非法"的 manifest。此后所有台账写入必须 `with` 关闭句柄、
   **写完立刻 tomllib 复验、不过即恢复备份**，且恢复用参数化 subprocess 不用 `os.system`。

另一条与门无关但同样致命的操作教训：**本环境的 heredoc 会把 `\\` 压成 `\`**，
因此含反斜杠的内容（TOML 转义、正则字符类、Windows 路径）必须走文件写入而非内联
heredoc——本轮两次 TOML 解析失败都源于此，而非逻辑错误。

### 全门实跑结论（不是"应该都绿"，是跑完读日志）

`py -3 scripts/run_gate_manifest.py` 全 light 档：**ok=114 / mismatch=1 / env-fail=0 /
skip=7（heavy 档按设计需显式 `--with-heavy`），合计 122/122**。唯一不符是
**G-16 `audit_spec_coverage.py` rc=1**，经查为**既有且已裁决**项：
`docs/reports/L0-contracts-residual-debt_2026-10-13.md:178` 记为 D-13——仓外 Qoder 计划
缓存 `Phase_R_*.md` 仍指 W3 前的 `crates/nexus-contracts/src/util.rs`，裁决"维持不处理
（仓内代改即污染他库）"，且 G-16 属 `ci_exempt`（本地形态，CI 不跑）。本轮**不翻案**。

值得留档的推论：一个已知为红、又留在 `expect = 0` 里的门，会让聚合实跑**永久红**，
而永久红的聚合读数会训练人去忽略它——本仓对这类情形的既有处置是改 `expect = 1` 或
`ci_exempt`，G-16 已取后者，故其红只出现在本地全量实跑中。

### 本轮登记但刻意不修的三项

| 项 | 证据 | 级别 | 不修的理由 |
|---|---|---|---|
| `coverage_gate.py` 历史基线不可读时返回 `([], [note])` | `:266` `except Exception: continue` + `:274 if not vals: return [], [...]` ⇒ 若 `tomllib.loads` 全面失败，棘轮轴**静默不判**且无违规 | ~~P2~~ **第三轮已修，见 §8** | — |
| 3 处内联 `except Exception: pass` 的 stdout 编码兜底（`audit_gate_parity` / `check_topk_registry` / `coverage_gate`）绕过了 `gate_rc._utf8_stdio()` | 直读原文确认是刻意兜底，非判据吞异常 | **P3** | 属"双份实现"轴；为风格动三个正在工作的门，回归风险大于收益 |
| `lru_reachability.py` 把 `lru-0.12.5` 钉在 glob 里 | 版本升级即静默找不到源码 | **P3** | 一次性取证脚本，且 H7 修复后其失败已转 rc=2 可见 |

### 本轮提交与回滚补充

四笔（`git revert` 各自独立）：`99a54aa` 死导入 / `511df34` G-120 计数 /
`e9d861b` `.gitattributes` / `9657a7d` 钉 Python / `d963289` G-121-G-122 初版；
第二轮的判据扩展（H6/H7 + 三处修复 + note/spec_ref 同步）与初版同属一个门族，
回滚须连同 `scripts/check_py_tooling_health.py`、`gate_manifest.toml`、
`classify_fmt_vs_semantic.py`、`lru_facts.py`、`lru_reachability.py` 一并退，
否则 H6/H7 的 note 与代码会互相矛盾（note 说七条而门判五条，正是本轮在别的门上
刚修过的那类"散文比代码先腐烂"）。

## 8. 第三轮：关掉 §7 唯一那条 P2，并把「不可判定」接上真通道

`check_min_ratchet` 原把两种空态折叠成同一种结果：**无历史**（合法引导态）与
**有历史但零条解析成功**（键名/文件契约已变）。后者返回
`([], ["R1: no readable historical min; ratchet self-activates after first commit"])`
⇒ 零违规 + 一条听起来完全正常的 INFO，于是 `coverage_baseline.toml` 一旦改键名或搬家，
min 棘轮轴就**静默失效而门照样报 [OK]**。可达性有现场佐证：该轴当前只在 **1 个**提交值上
判单调（`R1: min ratchet monotonic over 1 committed values`），样本为 1，格式再变一次即归零。

修法对齐同文件既有范式而非新立口径——`mode_check` 早已对「没有可用记录」打
`[UNDECIDABLE]` 并退 2（`:338-340`），只有棘轮轴没沿用：

* 新增 `GateUndecidable`；`commits` 非空而 `vals` 为空即抛，消息带双计数
  （`N commit(s) touch <path> but 0 yielded a numeric min_line_coverage`），因为它要先
  回答的问题是「是历史为空，还是我读不出来」。
* 部分解析（`0 < len(vals) < len(commits)`）不再无声：追加 INFO 说明单调性是在这个
  子样本上判的，而非整段历史。
* `main()` 的 `--validate-config` 分支捕获后打印 `[UNDECIDABLE]` 退 2。CI 侧
  `ci.yml:318-321` 是裸 `run:` 块（无 `|| true`）⇒ 非零即阻塞；`run_gate_manifest.py`
  把 rc=2 归 ENV-FAIL、**既不记红也不记绿**，正是本仓对「判据没跑完」的既定方言
  （`coverage.yml` 头注显示「阈值算了、结论被丢」同病已在覆盖率真实裁决那侧修过）。
* 顺带修同函数一条「消息文本说谎」：引导态提示把文件名写死成 `coverage_baseline.toml`，
  传入别的 `baseline_rel` 时会指认一个与本次判定无关的文件。

**守卫双向验过才算有牙**（只测一个方向＝没测，恒假守卫是本仓反复登记的病灶）：正对照用
仓内真实文件 `scripts/gate_manifest.toml`（25 个提交、TOML 可解析、但无
`min_line_coverage` 键）⇒ 必须抛；负对照用无历史路径 ⇒ 必须保持 INFO 不抛。两者固化为
`--selftest` case 13/14，断言里**不写死提交数**（会腐烂），只锚消息中必然存在的措辞片段。

复验：`--selftest` 由 12 项增至 **14 项全过**、`--validate-config` 仍 rc=0（无行为回归）、
G-121/G-122 仍 rc=0。

### 全 122 门聚合实跑（本轮在无并发污染下补跑）

上轮聚合跑受主会话在途编辑污染而报 `env-fail=2`——G-119/G-120 rc=2 的成因是
`required_keys` 签名与调用点未同步，属**未提交中间态**（HEAD 自身自洽，且我当时单独实测
为 rc=0）。其落地后本轮重跑：**ok=114 / mismatch=1 / env-fail=0 / unknown=0 / skip=7**
（heavy 档按设计需显式 `--with-heavy`），合计 **122/122**。唯一不符仍是 **G-16**（既有
已裁决 D-13：仓外 Qoder 计划缓存指 W3 前旧路径，「维持不处理」，且属 `ci_exempt` 本地
形态）⇒ 不翻案。
