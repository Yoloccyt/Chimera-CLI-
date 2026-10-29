# ADR-227 — L0-L10 R3 全量独立深审：十维复核、与 R2 对拍、五轮去伪与方法论订正

- **状态**：✅ Accepted（2026-10-29）
- **轮次**：R3（独立深审；计划 `L0-L10_R3_深审_ec68fa76.md`）
- **基线**：HEAD `d92e960`，分支 `feat/c1-compute-plane-governance`
- **范围**：L0-L10 全层 **46 crate**（`cargo metadata` 实测 47 package = 46 crate + 根 `chimera-e2e-tests`）；L0 = L0-A(7) + L0-B(3)（沿用 ADR-222 双层口径，机器层号不重排）
- **前置裁决**：V-1 范围以实测为准；V-2 编号加 `R3-` 前缀、新门从 G-137 起、本 ADR = 227；V-3 因磁盘 0.78 GB，**本轮不声称「本地真全量绿」**
- **交付物**（本地权威源，`.gitignore:68` 使 `docs/reports/**` 不入库）：
  `docs/reports/l0l10-r3-deep-audit-2026-10-29/{00 事实基线, 01 机械层读数, 02 问题册, 03 R2对拍差额表, 04 重构技术方案, 05 ADR同步与待裁台账, 06 验证报告与回滚}.md`

---

## 1. 背景与目标

R2（`C-57…C-141`）已收口。R3 的目标**不是复述结论**，而是：

1. 不采信 R2 的数值，重新建立事实基线（W0）；
2. 按 10 维度重新取证，机械层用**脚本现取**、语义层用**抽样 + 真实行号**（W1/W2）；
3. 与 R2 逐条对拍，出「维持 / 推翻 / 漏项」三类结论（W3）；
4. 高严重级问题落码、其余入待裁（W4/W5）。

## 2. 事实基线（W0 现取，全部可复现）

| 项 | 读数 |
|---|---|
| 成员 | 47 package（46 crate + 根 `chimera-e2e-tests`）；内部依赖边 **221**（normal 157 / dev 64 / build 0） |
| 事件面 | 变体 **154**；`severity()=Critical` **22**；mpsc 臂 **18**；`CRITICAL_TOTAL=22`；`LANE_FORBIDDEN_SHARD=22`；`ImmutableSurface=33`（`Critical*`=19） |
| 门 | `[[gate]]` **136**；workflow **11** |
| ADR | 物理 **122** 份，max **226**（本 ADR 落盘后 123 / 227） |
| 冻结台账 | doc_count 5 / event_ownership 18 / dep_edge 158 / **crate_reachability 0（无冻结孤岛）** / ignored 78 / lock_await 9 / l0b_dep 27 |
| 工作树 | 17 条未提交，**全部他批在途**；R3 全程未触碰 |

## 3. 十维结论（浓缩）

| # | 维度 | 结论 |
|---|---|---|
| 1 | 分层与依赖 | **无违规**：依赖双源 rc=0（B 向上 / C 完整性 / D mas 界 / E L10 出边）、L0 家族 12 门 OK |
| 2 | crate 与模块结构 | **无违规**：46/46 有 `lib.rs`；可达性 38 reachable + 8 GATED + **0 孤岛**（自报样本：46/38/0，frozen 0/352186 行） |
| 3 | 接口契约 | **无违规**：`check_pub_api_leak` live `sites=18 rows=18 scanned_pub_fn=408`、selftest **16** 断言；**外部类型上泄 0**（同名本地模块消歧后） |
| 4 | 事件定义 | **自洽**：154/22/18/22/33 四者互相锁定，G-117/118 parity OK、G-66/67 归属 OK；**3 个运维类 Critical 未入冻结面** → `R3-T01` |
| 5 | 并发模型 | **无违规**：G-26/27 持锁跨 await、G-79/80 订阅顺序、G-77/78 SQLite 阻塞 全 OK |
| 6 | 安全治理 | **无违规**：真实 `unsafe` **0**；`#![forbid(unsafe_code)]` **46/46**；**生产面 `unwrap/expect` = 0** |
| 7 | 存储与记忆 | 观察：`content_hash` 57 / `schema_version` 15 / `payload_version` 12 三套命名并存 → `R3-T02` |
| 8 | 性能与资源 | **无违规**：I5 11rl+6slo、I6 62、I8 家族全 PASS；Top-K 形态抽样合法 |
| 9 | 测试与门禁 | **无缺口**：129 门 OK / 7 heavy SKIP / 0 mismatch；fnlen 由 **G-48** 承载（`measured=4 / baseline=4` only-decrease） |
| 10 | 文档一致性 | **3 处缺口**：`R3-F02`（doc-count 锚点 39/152，P2）、`R3-F06`（C-138 无档，P3）、`R3-F07`（R2 两本账条目集不一致，P3） |

## 4. 问题与撤销（完整链见交付物 02）

| 编号 | 结论 | 级别 |
|---|---|---|
| `R3-F02` | **doc-count 门自己的锚点写着过期数字并以 registered drift 放行**：`DOC-AUDIT-FACTS_2026-09-20.md:175/176` = `39 crates` / `152 变体`，实测 46/154；且注册理由是「.gitignore'd」——但 `.gitignore:76` 有反白 `!docs/reports/DOC-AUDIT-FACTS_*.md`、`git ls-files` 命中 ⇒ **豁免理由基于失效前提** | **P2** |
| `R3-F04` | 生产面 `Box<dyn>` **78**（45 为 `Fn/Future/Any`，33 为注入接缝） | **P4 记录**（见 D1） |
| `R3-F07` | **R2 的两本账条目集不一致**：`02` 索引 53 条 vs `tmp` 登记册 59 条，11 个只在登记册、5 个只在索引 | **P3** |
| `R3-F06` | `C-138` 提交 `d47e575` 消息无编号、ADR-225 无落地行，仅被 T-1 叙述性引用 | **P3** |
| ~~`R3-F01`~~ | ~~「fnlen 红线无门」~~ → **撤销**：G-48 承载（`test_fnlen_smoke.sh` + `fnlen_baseline.txt`），4 条债已封 cap（生产 2：467 / 215） | 撤销 |
| ~~`R3-F03`~~ | ~~生产面 unwrap/expect 311~~ → **撤销**：真生产面 **0**（4 处残留全部落在 `#[cfg(test)] fn`） | 撤销 |
| ~~`R3-F05`~~ | ~~Top-K 退化 19 文件~~ → **撤销**：均为「`select_nth_unstable` 取前 K，再对前 K 排序」的合法形态 | 撤销 |

## 5. 与 R2 对拍（完整表见交付物 03）

- **维持 6 项**（抽查）：事件四锁 22/18/22/33、`CODE_WIKI §5.4` 22/18、C-107 locale 自 pin（111 处）、C-124 `ci-fast` 并发 4、C-127 DORMANT-ONLY + 红线收窄、Top-K 无退化。
- **推翻 1 项（D-1）**：R2 称「`audit_fnlen --check` 辖内唯一条目是测试面」；实测 `measured=4 / baseline=4`，**含 2 条生产债**。
- **R2 漏项 2 项**：`R3-F02`（P2）、`R3-F06`（P3）。
- **R3 自撤 2 项**：`R3-F03` / `R3-F05`（保留撤销痕迹，防止下轮重犯）。

## 6. 决策（D1–D4）

- **D1（口径澄清，P4）**：`Box<dyn Trait>` 的使用边界——
  **接缝注入点**（构造期注入、运行期只读调用、需跨 crate 替换实现）**允许**；
  热路径（每次调用过 dyn 分派、可分派数固定）优先 `enum dispatch`；纯内部组合优先 `impl Trait`。
  依据：生产面 78 处中 33 处为接缝（`CandidateRetriever`/`ExternalStorage`/`SandboxProvider`/`CoreBackend`），改泛型会丢运行时替换能力并扩散调用面。
- **D2（方法论，强制）**：**遍历型静态检查必须打印样本量并做去伪**——本轮五次去伪使 `unwrap/expect` 从 6166 塌到 0、`forbid` 覆盖从「17/46」改为「46/46」、`unsafe` 从 51 塌到 0、外部上泄从 2 塌到 0、`Box<dyn>` 从 214 塌到 78。五类失真机理已写入交付物 02 §1。
- **D3（门覆盖判据）**：判定「某红线是否有门」**不能只 grep 门的 `cmd` 串**——`G-48` 的 `cmd` 是 `test_fnlen_smoke.sh`，`audit_fnlen` 只出现在其 `note` 内。必须以 `gate_manifest.toml` 的 `note` + 被调脚本内部引用为准。
- **D4（可追溯，强制）**：任何 `C-xxx` 单必须**双落**——提交消息含编号 **且** ADR 落地表有行；只满足其一的，按 `R3-F06` 方式补记。

## 7. 待裁（R3-T）

| 编号 | 议题 | 默认值 |
|---|---|---|
| `R3-T01` | 3 个运维类 Critical（`ConsensusReached`/`OrphanCallDetected`/`SlowConsumerDropped`）是否入冻结面 | **不入**，把「只冻结安全类 Critical」写成明文规则（零扩散；避免再触发 fuzz 计数链） |
| `R3-T02` | `schema_version`/`payload_version`/`content_hash` 是否统一命名 | **不统一**，在文档写清三者适用面 |
| `R3-T03` | 「库层 thiserror」对纯契约层的适用边界（10 个 crate 无 `src/error.rs`） | **纯契约层豁免**，写入 ADR |
| `R3-T04` | fnlen 的 2 条生产债（467 / 215 行）是否本轮拆 | **不拆**，另立专项（拆 467 行函数需独立评审 + 回归面） |
| `R3-T05` | 是否批准本轮落 `R3-C01`（doc 锚点 46/154 + 删 2 条注册）与 `R3-C02`（补 C-138 落地行） | **批准**（3 文件、零代码；验证命令见交付物 04） |

## 8. 收口读数（W5，同 HEAD；R3 未改代码）

| 检查 | 读数 |
|---|---|
| 门集（light） | `ok=129 mismatch=0 env-fail=0 unknown=0 skip=7 / 136`，rc=0 |
| 四道文档门 | `adr_index_sync` / `check_doc_drift` / `check_doc_consistency` / `check_gates_provenance` 全 rc=0 |
| 依赖双源 | `.sh` rc=0 / `.ps1` rc=0 |
| lint 棘轮 | rc=0（46/46/46） |
| 性能红线静态面 | rc=0（I5/I6/I8 家族全 PASS） |
| 契约泄漏门 | selftest 16 断言 rc=0；live `18 sites / 408 pub fn` rc=0 |
| 可达性 | `reachable=38 frozen=0 new_gaps=0` rc=0 |
| 宽面类型检查 | `cargo check --workspace --all-targets` rc=0 |
| 静态检查 | `cargo clippy --workspace --all-targets -D warnings` rc=0 |
| fuzz 语法 | `cargo check --manifest-path fuzz/Cargo.toml` rc=0 |
| 格式 | 47 包逐包 `fmt --check` bad=[] |
| **未跑（如实）** | heavy 7 门（G-12/13/14/15/19/49/50）、真全量 `cargo test --workspace`、release `--ignored` 档、`cargo audit`、coverage 实测 ⇒ **交 CI 载体**（磁盘 free 0.29 GB） |

## 9. 回滚与入库边界

- **入库面 = 本文件单文件**（`.gitignore:71 !docs/architecture/ADR-*.md` 反白使 ADR 可入库）。
- **`adr_index.md` 是本地派生索引**：实测 `git check-ignore -v` 命中 `.gitignore:68 (*.md)`、`git ls-files --error-unmatch` 报未跟踪
  ⇒ 本轮为其新增的 `| ADR-227 |` 行**不入库**（与 R2 的 A-7「索引行本地」同一政策，非缺陷）。
- 回滚 = `git revert` 本文件所在提交；若需一并清理本地索引，再手动删 `adr_index.md` 对应行。
- R3 未改任何代码或既有文档，**无代码回滚面**；`R3-C01/C02` 若获批再按其单独立提交与回滚。
