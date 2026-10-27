# ADR-225: L0-L10 全层重构 R2 收口 —— 验证面实证、环境根因订正、审计守护面补强与 M49 拆单

> **Status**: ✅ Accepted（2026-10-26，业主批准《L0-L10 全层重构 R2 实施方案》并逐条裁决 §2 决策点；本 ADR 为批次收口落档）
> **决策日期**: 2026-10-26
> **关联 ADR**: ADR-214~223（L0 专项十轮裁决，**全部继承不重开**）/ ADR-224（L0-L10 全层重构 R1，本批**继承其 D1~D7 不重开**，并对其 §6 总账与 §7.2 状态表做**事实订正**）/ ADR-222（L0-A/L0-B 双亚层，本批沿用其分层口径；附录 §9「L0-B 暂不设冻结门」维持）/ ADR-198（G-85 PS 5.1 parity，本批 D1 解除其红）/ ADR-108（session-store CBMR，D4 前置依据）/ ADR-042+053 rev4+054（R2 冻结期与 RL 闸门，本批继续生效）/ ADR-195（GPU 仍否决）/ ADR-193（架构减法，方向反转已由 ADR-222 登记）/ ADR-215（closed/open 分级，D6 判据来源）
> **关联交付**: 六件套 `docs/reports/l0l10-r2-full-recon-2026-10-26/`（00 事实基线 / 01 冲突裁决台账 / 02 问题登记册 / 03 技术方案 / 04 测试验收与回滚 / 05 ADR 同步与待裁台账）+ 基线报告 `docs/reports/l0l10-r2-baseline-2026-10-26.md` + 登记册 `tmp/l0l10-r2-issues.md`（M39– 与 L47– 续编；**本行不登记水位**，已用至哪号以登记册现取为准，否则本 ADR 自己就变成第四份必漂总账；tmp 与 docs/reports 多数不入库，故关键句按 D5 引用进本 ADR）
> **权威源**: 代码实测与命令读数以本 ADR 引用的 file:line 与 `docs/reports/l0l10-r2-*` 现取为准；事件定级/常量以 `crates/event-bus/src/{registry.rs,bus.rs}` 现取为准；门集以 `py -3 scripts/run_gate_manifest.py` 的 `[SUM]` 自报为准
> **编号合法性**: 落盘前现取 ADR 物理文件 **119** 份、磁盘主编号 max **224**（`ADR-224` 已由 C-60 `8432504` 入库）；`ADR-225` 全库 **0 命中**（`Select-String docs/architecture/*.md,agents.md,CONTRIBUTING.md,CHANGELOG.md -Pattern 'ADR-225'` → 0）。门 max **G-134**、`[[gate]]` 134 条，`G-135` 0 命中（D3 预留）。变更单**自 C-57 起续编**（R1 用尽 C-01..C-56；落盘时现取 `C-57..C-99` 在 R1 施工图/进度报告/git log 三处 0 命中；本批实际用到哪一号以 §4 与 `git log` 现取为准，不在本行登记区间终点）。事件变体**零新增**（154 不变）

---

## 1. 背景

R1（ADR-224）自陈病灶形状为「门在、但承诺面与投递面/装配面脱节」。R2 首轮取证发现 **R1 自身留下了同型病灶，且落在最要命的一环——验证面从未被验证**：

- R1 进度报告 §1.3 原文「全量验证门未过，验收不能就此关闭」；其 §3.2/3.3 记 workspace 级 cargo 五套件**全部 rc101**，release 压测档**零测试执行**。
- R1 收口只跑 §8.2 十二道门 + §8.3 受影响门（约 **24/134**）即判「门禁面全绿 26 项 PASS」。
- R2 首次整体跑全 134 门即抖出：**G-85（PS 5.1 UTF-8 BOM 门）当前红，且该步骤位于 `ci.yml` 的 check job L143-146（与依赖铁律门、G-99、G-125 同 job）⇒ 是一条 PR 阻断红**，自 2026-10-25 沉默。逐字节取证：`scripts/measure_startup.ps1` 在其**全部历史上从未带 BOM**（`e3e0046^` blob first3=`23 20 3d`），真正的带入者是 R1 携带提交 **`0d39b50`**（「chore: 携带在途工作树改动」，该文件以 87 行纯新增入库）；门自身 `--selftest` rc=0（4 fixtures）证明是「被检对象坏」非「检测器坏」。
- R2 首次跑通真全量测试：**447 个 test target / passed=11375 / failed=4**，而该项目此前任何一次「全量测试」都未抵达终点。

同时，ADR-224 §6/§7.2 的总账与 `git log` 已显著漂移：其记 8 单 blocked「未提交落地」，实测其中 5 单已提交（`a4438ca`/`c67137d`/`7c659ef`/`bd68767`/`e3e0046`），另 C-27 内容已在 HEAD（搭 `7b7e571`=C-45 落地）、C-42/44/47/48/54/55 六单目标态经内容级复测确认已达成 ⇒ **R1 所谓「17 blocked」实际只剩 C-29 一单**。

## 2. 决策

### D1（环境根因订正）workspace 级命令一律显式 `--jobs 2`；不改 `.cargo/config.toml`

- **决策**：R1 的「15GB 机型不可建立全量绿基线」判据**作废**。实证：`cargo check --workspace --all-targets --jobs 2` → **rc=0**（0 warning，5m26s）；`cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` → **rc=0**（5m27s）；实测 rustc 工作集 86–197 MB/进程、free 内存稳定 >3.1 GB。根因是 `.cargo/config.toml` 固化 `[build] jobs = 4`。release 压测档采「构建与运行解耦」：先 `cargo build --workspace --release --tests --jobs 1` 产工件，再 `cargo test --workspace --release -- --ignored --test-threads=1`，且**禁止**为降内存改 release profile（`lto`/`codegen-units`/`opt-level` 会污染 SLO 阈值测量）。
- **被否反方案 ①**：把 `config.toml` 的 jobs 改成 2 —— 该值是 rustc/链接器 `0xC0000409` 的历史修复产物且被 CI runner 依赖；静默降并发会把「构建慢」这一可感知代价无告警地转嫁给所有人，而命令面纪律可逐条审计。**被否反方案 ②**：全量 `cargo clean` 从零重建以根除 60 个 0 字节 `.rmeta` —— 54,816 文件从零重建的峰值内存风险高于定点 `cargo clean -p`，且摧毁 `target/criterion` bench 缓存，把性能取数窗口拖到不可完成。
- **落地状态**：✅ 已生效（本 ADR 记录 + §3 验收清单引用）。

### D2（收口口径）全门集 `[SUM]` 行是「门禁全绿」的唯一判据

- **决策**：任何批次收口必须跑 `py -3 scripts/run_gate_manifest.py` 并以 `[SUM] ok=N mismatch=0 env-fail=0 unknown=0 skip=M 合计 N+M/总数` 为唯一判据；heavy 门单独申报。**「§8.2 十二道门 + §8.3 受影响门」不再构成「门禁全绿」的充分证据**。同批把「全量测试必须带 `--no-fail-fast`（或直接用 nextest）」写进发布前清单。
- **依据（本轮实证）**：`cargo test --workspace` **默认 fail-fast** —— R2 首跑仅产 67 个 `test result:` 行 / 2327 passed 即中止，而真全量是 **447 target / 11375 passed** ⇒ **376 个 target 被一个红静默吞掉**；本地口径与 CI 口径（nextest + 带 `--no-fail-fast` 的 doctest job）在此不等价。
- **被否反方案**：维持「受影响门」口径 —— 正是该口径让 G-85 沉默了一整天；「门在、但没人跑它」与 R1 已治理的「承诺面/投递面失配」是同一种病。
- **落地状态**：✅ 口径生效（登记册 M39 处置第 2 段 / M56）；文档传导 C-73 待执行。

### D3（M49 拆两单）拦截留痕先落地，事件升档独立成批

- **决策**：业主裁决「审计链留痕 + 升 Critical+mpsc 两者都做」，R2 按工程可实现性拆为：**C-64a**（只改 `crates/seccore/src/sandbox.rs` 拒绝分支，在 `return Err` 之前补审计链 append）先行；**C-64b**（Normal→Critical 全套，MPSC 17→18、TOTAL 21→22、三锁 CRITICAL 21→22 / NORMAL 122→121、`LANE_FORBIDDEN_SHARD` +1、旁路臂 +1、**L0-A `ImmutableSurface` 增变体使 `all() -> [ImmutableSurface; 32]` 变 33**、G-117 note 去手抄计数）作为独立批次，前置 = 在途 fuzz 批次收尾 + G-115 closed/open 裁决。
- **依据**：`harness_spec.rs:98` 明文「Critical 事件清单（18 = 17 mpsc 旁路全覆盖 + CheckpointSaved broadcast-only）」且每个 mpsc-Critical 事件是一个独立枚举变体；`fuzz/fuzz_targets/harness_spec_parse.rs` 是该枚举的 fuzz 消费者，本轮实测**处于在途批次未提交态**。TUI 侧经复测确认**不需改**（`panels/event_stream.rs:557`、`panels/log.rs:406` 按 topic 分派、`:605/:454` 按硬编 priority 分派、`event_coverage_drift_test.rs:267` 用 `TuiEventClass::Consumed`，均不读 `severity()`）。
- **被否反方案**：一次做完（八组单一提交）—— 会把最高风险面与他人的未提交工作绑进同一颗提交，违背「整提交可独立 revert」与「禁碰在途改动」两条既定纪律；两单都完成后效果与业主裁决一致。
- **落地状态**：✅ **C-64a 已执行（`cc1bcbc`）**：新增 `AuditChain::append_rejection`（取**被拒绝的原始命令**而非 `CommandSpec`，因两条拒绝路径上规格从未生成）+ 两处拒绝口接线；零枚举变更（用既有 `Failed` 状态与 `Result` 步骤）；新增 4 条断言（含「篡改 outcome 被 `verify()` 检出」与「同命令两次 `command_hash` 恒等」）。验收：`cargo check -p seccore --all-targets` 0 error 0 warning；`cargo test -p seccore` 全 target 0 failed（`sandbox_integration` 19 passed 含新增 4 条）。C-64b 仍转 §5 待裁 T-1。

### D4（M22/C-29）session-store 采「先加固、后接线」

- **决策**：ADR-224 D1 的装配决策维持，但执行前置两单 —— **C-61**（M50：`CbmrInner` 加取消标志 + 托管 `JoinHandle` + 新增 `pub async fn shutdown()` + `impl Drop` 置标志/`abort`/pending 非空 warn，**不改 `new()` 签名**）、**C-62**（M51：`create_dir_all` + `Connection::open` + `pragma_update` + `execute_batch` 收进既有 `run_blocking`，红线文本由「SQLite 写入只经 spawn_blocking」扩为「打开/DDL/写入」），再 **C-63** 按路径 A 接线（`composition.rs:296` 构造 `CbmrWriter`（经 `StoreConfig::with_dir`）注入 `AppServer::with_session_store`、登记新生产边 `chimera-cli|session-store` 入 `dep_edge_freeze.txt`、装配态 DORMANT→PRODUCTION-WIRED、契约头与架构视图重派生）。
- **对 ADR-224 D1 的两处事实订正**：① 原施工图引用的假想 API `SessionStore::new(dir)` **确实不存在**（该 crate 全量仅 `lib.rs:229`/`replay.rs:4` 出现该词），真构造是 `writer.rs:123 pub fn new(config: StoreConfig)` + `types.rs:258 StoreConfig::with_dir`；② `server.rs:293 pub fn with_session_store(config, backend: Box<dyn CoreBackend>, store: CbmrWriter)` **存在**且签名可用（原报告称「两处未闭合设计」之一），但 `nexus-app-server` **不** re-export `CbmrWriter` ⇒ 原「路径 B 最小暴露」并非纯接线而需改 app-server 代码。
- **Δ1「无新增依赖边」承诺的修订**：亲验 `dep_edge_freeze.txt` 现无 `chimera-cli|session-store` ⇒ 任何直接装配路径必新增一条生产边。判为**计划缺陷而非决策缺陷**（该承诺建立在已失效的假想 API 上），由本 ADR 明文修订：C-63 允许新增 L10→L3 向下一条合法向下边，并登记台账。
- **被否反方案 ①**：忽略 M50/M51 直接接线 —— 把一个不可取消的永生刷写循环与运行期阻塞 IO 搬进生产，且 `append` 已返回 Ok（`writer.rs:150-152` 自陈「入队成功，非 fsync 确认」）而 pending 可能永不落盘，与 `segment.rs:288` 红线相悖。**被否反方案 ②**：`new()` 改 async 返回 JoinHandle —— 破坏调用面且制造「双构造路径」病理（同 R1 L46 双 `MtpeConfig` 教训）。
- **落地状态**：✅ **已执行**（三单先后序严厉遵守）：**C-61** `2b70b93`（取消令牌 + 托管句柄 + `shutdown()` + `Drop`）→ **C-62** `9fdc4f3`（打开/DDL 入阻塞池）→ **C-63** `b6f79c4` + `627e256`（组合根接线 + lock 行）+ `65e30d6`（架构视图重派生）。**N-4 降级姿态已在 `build_app_server` 内定义**（`open` 失败只 `warn!` 并退回 `with_backend`，服务照常启动）。运行时证据（非编译证据）：`composition_root_e2e` 跑完后 `~/.chimera/sessions/` 实际产生 `sessions.sqlite3`（98,304 B）与段文件 `g~~r.0.jsonl`（86,212 B）⇒ 双写真生效。

### D5（M43）治理文档入库政策：采「方案乙」，不改 `.gitignore`

- **适用边界（两次逐份实测收窄：C-73b 第一次 / C-91 第二次）**：方案乙只覆盖 `git ls-files -- <path>` 无输出的那批文档。本 ADR 先前把 `docs/architecture/CODE_WIKI.md` 与 v3.4.0 二十三篇权威设计文档归入「不入库」，**该归类是错的** —— 两者均在索引内，订正它们必须直接提交（与 `agents.md` 同理）。逐份实测清单见下方订正注。

- **决策**：本轮**不**改 `.gitignore`。改为硬规定：**凡订正「确未入库」的权威 md（`docs/architecture/adr_index.md`、`docs/tui/README.md`、`CHANGELOG.md`、十六源手册 v3.0、`.claude`/`.qoder` 规则副本、`docs/reports/l0l10-r2-*`、`tmp/*`），必须同批把订正后的关键句原文引用进可入库的 ADR**，使 ADR 成为证据副本；而 **tracked 的权威 md（`agents.md`、`CONTRIBUTING.md`、`docs/architecture/CODE_WIKI.md`、v3.4.0 二十三篇等）必须直接 `git add` 提交**，不得拿「反正 ADR 里有副本」当借口不提交。本 ADR 的 D1~D4 与本批各单提交消息即按此执行。
- **依据**：`.gitignore:57` 为 `*.md`，反白仅三条（`:60 !docs/architecture/ADR-*.md`、`:64`、`:65`）。**忽略规则既不取消已跟踪文件，也不命中本就未被排除的路径** ⇒ 「某份 md 可否被审」只能逐份 `git ls-files -- <path>` + `git check-ignore -v -- <path>` 实测，**不得按目录或文件名模式概括**。就 R2 复测「已达成」的六单（C-42/44/47/48/54/55）而言：`gate_manifest.toml`（G-09 note）、`agents.md`、`docs/architecture/CODE_WIKI.md` **均可入库可审**；仅 `adr_index.md`、`docs/tui/README.md`、十六源手册 v3.0 **确为 git-ignored 本地文件**，无 blob、无提交、无 diff、`git status` 也不报 ⇒ 那部分的「文档已订正」无法被第二人复核，也无法被任何门验证 —— 这才是方案乙要填补的那一块。
- **订正注（C-91，登记册 M68）**【亲验 2026-10-26】：本 D5 初版的适用面判错，现逐份列出。① **实为 tracked 但曾被列入不可审**：`docs/architecture/CODE_WIKI.md`、`Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md`、`Chimera架构重构与优化方案_v3.5-全栈终局总账.md`、`docs/architecture/Chimera_CLI_九源交叉融合彻底重构工程手册V1.0.md`、`docs/architecture/Chimera_CLI_v4.0_架构重构与算法优化一体化方案（六编重构终版）.md`、`docs/architecture/fusion-concept-map.md`。② **`docs/reports/` 整目录不可一概而论**：`toolchain-analysis-report.md` 与 `v3.5-baseline-measurement-2026-10-10.md` 两份**非反白名单**报告已在库 ⇒ 「点名把单份报告纳入版本控制」（`git add -f`）在本仓有先例，构成 T-4 的新证据。③ **两个凭空名字删除**：根 `CLAUDE.md` 盘上不存在；`AGENTS.md` 与 `agents.md` 是**同一物理文件**（磁盘实名 `AGENTS.md`、索引实名 `agents.md`，Windows 大小写不敏感 FS），把它当两份分归两栏是错的。④ 全仓 tracked md 实测 **135** 份，其中非 ADR **14** 份。
- **被否反方案**：全量 md 入库 —— 会把数百个本地草稿/报告一次性推入仓库，与既有「配置收口/不入库」治理意图正面冲突。窄口径反白 —— 会直接改变 `check_doc_consistency.sh`/`adr_index_sync.py`/G-37 现明文「0 skipped by gitignore *.md policy」的断言语义，属独立政策授权，不在本 ADR 范围；且订正注已证明仓内存在更轻的先例（逐份 `git add -f`，不动规则文件）。
- **落地状态**：✅ 规定生效（登记册 M43 + M68 第二次适用面订正）；方案甲与六件套是否入库转 §5 待裁 T-4。

### D6（M37→新门 G-135）公共 API 签名泄漏门，判据上界取实测台账

- **决策**：新增 **G-135**（`scripts/check_pub_api_leak.py`，含 `--selftest` 植入例 + 台账 + `gate_manifest.toml` 两条 + `ci.yml` 一步，四件同批）扫描 `pub fn` 签名的 extern-crate 前缀，**先 L0-B 试点**；判据必须覆盖**返回位 / 参数位 / 泛型界**三类；依赖上界**只取 `scripts/l0b_dep_freeze.txt` 实测集**，不得取 `gate_manifest.toml` G-125 note 或 ADR-222 §9 的散文白名单。基线采「先 `--emit` 出台账、再判红」⇒ 零行为变更。
- **依据（M46/M47 实证）**：`check_l0b_dep_surface.py` 自报 `external_deps=27`，而台账 27 行中命中 `{event-bus, tokio, tracing}` 者仅 16 ⇒ 若以三值集为上界，首跑即产 **11 条伪红**；泄漏面亲验含返回位 9 站点 + **参数位 `shard.rs:261-266`**（`sender: tokio::sync::broadcast::Sender<NexusEvent>` 与 `handle: &tokio::runtime::Handle`，另 `:303`/`:448`），R1 M37 因只统计返回位而**漏记了耦合最强的一处**；`G-131` note 自陈「L0-B public fn 面不在本门辖区」= 轴缺口。
- **反方案**：沿用 G-125 note 白名单（否，理由同上，且正是本仓反复治理的「承诺在散文、判据为零」）。
- **落地状态**：⏳ 待执行 C-66；`impl From<第三方> for EventBusError`（`error.rs:55/68/75/82` 四条）是否计入判据，转 §5 待裁。

### D7（M48）审计链补校验侧：启动 + 导出双点，零新增事件变体

- **决策**：`AuditChain::verify()`（`seccore/src/audit.rs:469`）接入两个冷路径点（沙箱初始化 + 审计导出），失败发**既有** `AsaIntervention`（Critical+mpsc）并返回错误；**不进每命令热路径**（链长 O(n) 重算）。R2 目标恒为**零新增事件变体**——为拦截留痕新设专用事件属本仓明令禁止的「零生产者装饰变体」。
- **依据**：亲验 `crates/*/src/**`（排除 `seccore/src/audit.rs`）内 `.verify()` 命中 = **0**；`audit.rs` 内 8 处命中全在 `mod tests`（起点 `:791`）之后 ⇒ 红线 #2「Merkle 审计链完整性不可篡改」当前**只有写入侧、无校验侧**。
- **被否反方案**：每次执行后校验 —— 无基准证据下把 O(n) 重算放进热路径，违反「性能可证伪」；且本轮实测 `ExperienceCardBus` 等面已存在零基准同类问题（M54），不宜再造。
- **落地状态**：⏳ 待执行 C-65；若无稳定导出入口则退化为「启动 + 关账」双点并回写本 ADR。

### D8（反方案登记）R1 的 M49/M48 类问题未被 R1 发现的成因判定

- **决策**：判为**方法论缺陷**而非个人疏漏，并以 D2 的口径变更作为制度性修复。附带结论：R1 §7.1 待裁 28 项与 §7.2 8 项中，凡状态判据依赖「git log 无对应提交」者，本 ADR 一律要求现场重跑，不得继承。

### D9（T-2/T-3 裁后落文档）两份权威文档的愿景行处理与引文备份

- **决策**：业主裁 T-2（允许 loom，仅限 dev-dependency）与 T-3（不允许新外部依赖）后，R2 不重写两份文档的正文，而是按仓内既有纪律「历史文档只加注不改写」，在十六源手册头部追加两条批注（紧接已有的编号空间批注与 forbid 计数批注）。因该手册被 `.gitignore:57` 排除且从未入库（`git ls-files` 无输出），**按 D5 方案乙必须把订正后的关键句原文引用进本 ADR**，以下即证据副本。
- **引文①（记忆契约三要素批注，手册 `:14`）核心句**：「本手册第 8/10/14 章（`:66`、`:386`、`:809`、`:1484`、`:1628`）的 Memory Capsule v2.1 `schema_version` / `content_hash` / verification 三要素，采纳状态经 R2 逐要素实测，**不是「全未采纳」也不是「全未做」**：① BLAKE3 内容哈希——不采纳；② `schema_version` 与 `content_hash` 字段名——已在盘上使用 5 处；③ 真正缺的是**跨 crate 的宽度/类型约定**（u16 / u32 / u64 并存；`content_hash` 同时有 u64 与 String 两种）与 `ExperienceCard` 上的该字段。⇒ 本章不得被读作「尚未实现的新字段」，也不得被引用为「已统一的契约」；设计目标只到「先统一 L0-A 内部范式」为止（不依新 crate）。」
- **引文②（loom 愿景行批注，手册 `:15`）核心句**：「本手册 `:89`「~20,000（含 GPU 双后端基准与 12 个 loom 场景）」、`:1453`、`:1468`「loom 无锁场景 12」在盘上为零实现（`crates/*/Cargo.toml` 内 loom dev-dependency 0 命中）。⇒ 该行**不作废、转为待落地**，首批落 `hcw-window`/`mlc-engine` 两处持锁-广播；前置 = 另立 ADR 并按 ADR-222 附录 §9 补四项最小性论证。落地前，本手册与两份 tracked 手册中所有 loom 行均只应读作**目标/验证方法**而非现状。」
- **配套实测【亲验】**：（i）两份 tracked 文档（`docs/architecture/Chimera_CLI_九源交叉融合彻底重构工程手册V1.0.md`、`docs/architecture/Chimera_CLI_v4.0_架构重构与算法优化一体化方案（六编重构终版）.md`）共 20 处 loom 逐条复测：均为「怎么验 / W3–W8 门禁 / 目标测试总量」语气，**无现在时态失实句** ⇒ 本批不开文档订正单（只靠上面引文把口径定住）。（ii）`schema_version`/`payload_version`/`content_hash`/`format_version` 的 pub 字段位全仓实测 = **8**（探针 `tmp/r2_schema_versions.py`），否定了 M55 的「两套」口径⇒ 登记 M69。（iii）本仓 tracked md 全表法计数 =135 / 非 ADR =14，同时暴露 `core.ignorecase=true` 下 `git ls-files -- AGENTS.md` 返回空而 `git status` 判 clean 的**判定坑**（登记 L55：入库性判定必须用全表法，不得拿单一大小写拼法的 pathspec 无输出当结论）。
- **被否反方案**：按 T-3 字面把 `schema_version`/`content_hash` 整项登记为「本仓不采纳」——实测证明字段名已在用，那样做会留下一条与盘上相反的权威声明（恰好是 R2 立论要治的「文档-代码脱节」）。**本 ADR 的做法是窄化而不推翻裁决**：不采纳的只是 BLAKE3 算法与把它升为统一契约字段。
- **落地状态**：✅ C-92 已执行（手册两条批注 + 本 D9 引文 + T-3 行窄化 + 新增 T-14）。

## 3. 破坏性面显式记录

| 来源 | 破坏性面 | 兼容性论证 | 落地状态 |
|---|---|---|---|
| C-58（承 R1 C-24） | `DecbConfig::validate()` 新增拒绝态（NaN/±inf 的 `total_budget_limit` 与 8 个因子/成本/溢出键） | 非破坏：靠 NaN 绕过校验的输入本即缺陷，无合法迁移需求；`DecbError::ConfigError` 变体未增，穷尽 match 面不变；置最前是因为 NaN 参与比较恒 false 会穿透下方 `<= 0`/`< 0`/倒挂检查，±inf 同理穿透上限，NaN 的 total_budget_limit 会使 `check_overflow` 恒走 Degraded（服务不可用 + Critical 事件风暴） | ✅ 已落地 `5af76c9`；`cargo test -p decb-governor` rc=0 |
| C-57（M39） | 无 —— 仅前置 3 字节 BOM | 脚本语义不变；PS 5.1 下由 ANSI 误解码变为正确解码，属修复；正文逐字节不变（len 3737→3740、`git diff --numstat` = 1 1） | ✅ 已落地 `c8fcd1c`；G-85 `RESULT: OK (26/26)` |
| C-59 / C-60 | 无 | 纯注释 / 纯入库 | ✅ `b043674` / `8432504` |
| **C-64b（预登记）** | `NexusEvent::SandboxViolation` 定级 Normal→Critical：对穷尽 match severity 的消费方为行为变更；mpsc 旁路集 +1（17→18）、Critical 总数 +1（21→22）；**L0-A 契约类型 `ImmutableSurface` 增变体使 `all()` 数组 arity 32→33** | 投递纯增益（mpsc 保障 + broadcast 不减）；wire 格式零变化；TUI 侧经复测确认按 topic/priority 分派，不受定级影响；**若 `ImmutableSurface` 属 closed 须先记 closed→open 重分类**（ADR-224 §5 先例） | ⏳ 待裁 T-1（前置：在途 fuzz 批收尾） |
| **C-63（预登记）** | 生产行为新增：会话双写落盘（每 turn 一次段文件追加）+ **新增 1 条生产边 `chimera-cli\|session-store`** | 行为级新增非 API 破坏；回滚 = revert，已落盘段文件无 schema 依赖、留存无害；新边为 L10→L3 合法向下单，须同批登记 `dep_edge_freeze.txt` 并复跑 G-32/33 live 与 G-75/76 基线 | ⏳ 待执行（前置 C-61/C-62） |

## 4. 实施结果总账（2026-10-26 收口时点）

> 本总账是 `git log` 的函数。下列哈希均为 `git log --oneline` 实测在册项；后续批次引用时须现场重跑，不得引用本行计数。

- **已落地 7 单**（逆序 revert 即回滚路径）：
  | 单 | 提交 | 内容 | 件数 |
  |---|---|---|---|
  | C-57 | `c8fcd1c` | `scripts/measure_startup.ps1` 字节级前置 UTF-8 BOM，解 G-85 这条 `ci.yml` check job 阻断红（M39） | 1 |
  | C-58 | `5af76c9` | 承 R1 C-24：`decb-governor` `validate()` 两组 `is_finite()` fail-closed + 2 测试 | 1 |
  | C-59 | `b043674` | 承 R1 C-53：`nexus-contracts-seams` 去 `VectorStore` 类比失实（纯注释） | 1 |
  | C-60 | `8432504` | 承 R1 C-41：ADR-224 纳入版本控制 | 1 |
  | C-66 | `351b589` | **新门 G-135/G-136 四件同批**：`check_pub_api_leak.py` + `l0b_pub_signature_freeze.txt` + `gate_manifest.toml` 两条 + `ci.yml` 一步（D6） | 4 |
  | C-75 | `f1e960b` | 本 ADR-225 落档 | 1 |
  | C-76 | `66733e7` | ADR-224 追加 §10 R2 状态订正注（只加注不改写；M40/M41/M42） | 1 |
- **新门实测读数**【亲验】：`check_pub_api_leak.py --selftest` rc=0（**14 assertions PASSED**）；live rc=0（`sites=13 rows=13 scanned_pub_fn=408`）；`check_gates_provenance.py` 与其 `--selftest` 均 rc=0（方向 A/B 通过，新脚本已登记非孤儿）。立门前两处自揭的检测缺陷已**当场修判据而非放宽**：只匹配 `ident ::` 路径头会漏过经 `use` 引入的裸名（三处真实返回位被报成干净），以及函数名上的泛型参数列被丢弃——两者均已固定为 selftest 夹具。零生产码改动 ⇒ 无行为变更、无公共 API 扩大。
- **在途改动纪律**：`git status` 现取 14 个已跟踪文件 modified + 2 untracked，全部属**在途批次，R2 全程未代提交、未触碰**；R2 各单均为**逐文件 `git add`**，未用 `-A`/`.`；`git stash`/`reset --hard`/`checkout --` 按 ADR-222 D6 明令禁用；各单提交消息经字节级校验为合法 UTF-8。**本行不登记总单数**（总单数是 `git log` 的函数，写在此处必腐；完整承载清单见下方两段与交付物 02 §4）。
- **待执行 13 单**：C-61、C-62、C-63、C-64a、C-64b、C-65、C-69、C-74、C-78、C-79、C-80、C-81、C-82、C-83（清单与影响面见交付物 03 §3）。
- **第 2 轮追加落地（接上表，共 +8 单）**：
  | 单 | 提交 | 内容 | 承载 ID |
  |---|---|---|---|
  | C-70 | `89ca573` | `bus.rs` 旁路注释去 `UnboundedSender` 失实并界定真实投递语义（有空位必达 / 满载丢弃且计数可见 / 两通道是并集） | M52 |
  | C-71 | `f27ed5d` | `pattern_index.rs` 事件量级散文去数值化，改指 `VARIANT_COUNT_LOCK` | M53 |
  | C-72 | `09a0524` | `session-store/lib.rs` 组合根指针 `:251`→`:296/:298`，改函数名锁定 | L50 |
  | C-84 | `9cd0eaf` | `crate_reachability_freeze.txt` 拆清 mas-sched（非 optional 生产边）与 nexus-hook（feature 门控）的归因 | L53 |
  | C-85 | `a90b30d` | **修本批自引入的门红**：新门缺 `gate_rc` 崩溃守卫→G-44 红；按该门自陈「改脚本而非往基线加一行」修正 | M39 后续 |
  | C-67 | `abc4019` | 补 `hnsw_tiered` 阈值键（两表实测 53==53）+ 订正三处假不变式声明 | M44/M65/L51 |
  | C-68 | `a7aacc9` | `bench_check.yml` 补 `schedule: 0 3 * * *` + 订正 timeout 注释（10 项语义断言 PASS） | M45/M66 |
  | C-86 | `49b9b41` | **I9 不变式**：heredoc 与 `[[thresholds_ns]]` 键集机器对拍；selftest 44→50 全绿；变异反证（删真文 hnsw_tiered 行→I9 精确报出）证明其可拦截 M44 | M44 根修/M65 |
- **第 3 轮追加落地（接上表）**：
  | 单 | 提交 | 内容 | 承载 ID |
  |---|---|---|---|
  | C-87 | `5d0e841` | 本 ADR §4 回齐第 2 轮 8 单 + C-73 入库情况逐份列表 | M56 留证 |
  | C-88 | `e6a7f84` | 订正我自己「所有 md 不可审」的过度概括（D5 适用边界第一次收窄） | M43 初版 |
  | C-89 | `8407444` | G-135 计入 `impl From<第三方>`（业主 T-6）：四类判据齐备、台账 13→18、selftest 14→16 | M37/T-6 |
  | C-90 | `17f968e` | 本 ADR §5 归档 T-2/T-3/T-6 补裁结果；§6 回滚清单去枚举改指针 | 补裁三项 |
  | C-91 | `07a8b07` | D5 适用面第二次订正：`git ls-files -- <path>` + `git check-ignore -v` 逐份双验推翻「`CODE_WIKI.md`/v3.4.0 二十三篇不可审」；§4 推论行与 T-4 行同步；方案甲反白面收窄至两份 | M43/M68 |
  | C-92 | `9802044` | T-3/T-2 落文档：十六源手册追加两条批注（记忆契约三要素逐要素判定 + loom 愿景行转「待落地」）；T-3 行按 M69 窄化；新增待裁 T-14；L55 登记 `core.ignorecase` 判定坑 | M55/M69/M13/T-2/T-3 |
  | C-93 | `96ce892` | **新建 ADR-226**：loom 四项最小性论证 + E1 前置闸门（主路 ubuntu runner、windows 腿定 `generator` 真伪）；§1.1 记 R1 M13 指针漂移，§1.5 记「Linux CI 承接」零载体债 | M70/M71/L56/T-2 |
  | C-64a | `cc1bcbc` | **拦截上 Merkle 审计链**：`append_rejection`（取被拒绝的原始命令而非 `CommandSpec`）+ 两处拒绝口接线 + 4 条断言（篡改可检、哈希可复现）；零枚举变更 | M49（前半）/M72 |
  | C-61 | `2b70b93` | session-store 可取消刷写循环：`closing: AtomicBool` + `flush_loop: Mutex<Option<JoinHandle>>`（挂 `CbmrInner` 而非 `CbmrWriter`——后者 `Clone`，挂它会任一副本析构误杀循环）+ `pub async fn shutdown()` + `impl Drop for CbmrInner`（置标志/abort/pending 非空 warn） | M50 |
  | C-62 | `9fdc4f3` | 抽出 `open_blocking`；新增 `pub async fn open()` 经 `run_blocking`；红线由「SQLite 写入只经 spawn_blocking」扩为「**打开/DDL/写入**」 | M51 |
  | C-83（部分） | `c29334b` | `experience_loop_closure_e2e.rs` 的 `Err(_) => break` → `continue`：单次 50ms 超时不得终止整个 500ms 截止窗口（**真逻辑缺陷**，非测时不稳） | M63（1/6） |
  | C-95 | `7fb55ba` | 本 ADR 回写 C-64a 落地 + §5 新增 T-16/T-17 | M72/M73 |
  | C-96 | `7bd473e` | 两本持锁跨 await 台账行号随 C-61 更新（自造门红 G-27/G-35 修回；不新增豁免、不删代码） | M39 后续 |
  | C-97/C-98 | `887e59d`/`572a59e` | 两处 rustfmt 偏差归位（`cargo fmt --all` 撞 os error 206，改逐包跑才发现） | M39 后续 |
  | C-63 | `b6f79c4` | **组合根接 CBMR 双写**：新增生产边 `chimera-cli\|session-store`、`build_app_server` 转 async 并走 `CbmrWriter::open()`、11 个调用点补 `.await`、N-4 降级姿态定案、DORMANT→PRODUCTION-WIRED | M22/ADR-224 D1 |
  | C-63b | `627e256` | `Cargo.lock` 补登 C-63 的依赖行（只 1 行；不带会被 `--locked` 构建拒） | C-63 同批 |
  | C-99 | `65e30d6` | 架构视图随新边重派生（G-70 修红：5 件视图 `--write` 重派生后 `--verify` rc=0） | C-63 同批 |
  | C-80 + C-81 | `9ce30bf` | 菜单覆盖由 `[PanelId; 27]` 改为派生自 `PanelId::ALL`（+无重复项/全集 30 双断言，旧写法使 4 个守卫测试变成 27/30 的自循环断言）；旁路类数注释由「仅 4 类」改为「`CRITICAL_MPSC_VARIANTS` 全集 17 类 / 告警过滤 4 类」 | M60/M61 |
  | C-102 | `ab250b0` | 五处「loom 在 Windows GNU 不可用」断言经 E1-a′ 探针实测推翻后逐处订正为「取舍」并补真实理由 | M70/L56（文档面） |
  | C-79 | `58ac4ce` | **G-66 第二轴：生产者装配可达性**（M59）。`not_default_assembly_crates()`（optional 边 ∪ gated_wiring DECORATIVE/PARTIAL）+ `assembly_reachability()` 纯函数 + 新台账 `event_assembly_gated.txt`（只减不增）+ `[GAP-A]`/`[GAP-A-S]` 判红 + 4 条 selftest；首次跑出 **15** 条（M59 原判仅 2 条） | M59 |
- **第 4 轮追加落地（接上表）**：
  | 单 | 提交 | 内容 | 承载 ID |
  |---|---|---|---|
  | C-100 | `c1ba350` | 本 ADR 回写 C-63 全链落地与第六轮门集读数 | M22/M51/留证 |
  | C-101 | `7e735e4` | 本 ADR 新增「收口时点验收读数」表（门集/全量/doctest/release 档/依赖双源/派生/格式七行） | 留证 |
  | C-103 | `7b43006` | 本 ADR 回写 C-80/C-81/C-102 与第 9 轮门集读数 | M60/M61/M70 |
  | C-104 | `5af8d14` | 本 ADR 回写 C-79 第二轴与第 10 轮门集读数 | M59 |
  | C-94 | `5fa2cea` | **E1 载体落地**：`[workspace.dependencies] loom = "0.7.2"`（只进 dev 面）+ `mlc-engine/tests/loom_card_system.rs`（`#![cfg(loom)]`，2 写 1 读锁段模型检验）+ `ci.yml` 新增 `loom` job（ubuntu，步骤级 `RUSTFLAGS='--cfg loom'`，只跑单 test binary）；`[workspace.lints.rust]` 的 check-cfg 加 `cfg(loom)`（crate 级声明会被 `cannot override workspace.lints` 拒） | ADR-226 D1 / E1-a′/E1-c |
  | C-105 | `8e0009e` | ADR-226 §7 回填实施状态表（E1-a′ ✅ / E1-c ✅ / E1-a ⏳待 CI / E1-b ⏳） | ADR-226 |
  | C-106 | `632cb5a` | `session_store_dir()` 支持 `CHIMERA_SESSION_DIR` 覆盖 + 纯函数 `session_dir_from(Option<OsString>)` + 1 条单测（非空覆盖 / 空串回落 / 未设同值）——消 C-63 附带发现的「测试与生产共用 `~/.chimera/sessions/`」 | M75/L64 |
| C-107 | `127b11f` | **面板测试去「隐式全局 locale 依赖」**：5 条断言按 En 文本写却靠同 bin 他测的 `set_locale` 遗留（nextest 进程级隔离下必红）⇒ 改为自 pin `locale_test_guard()`+`set_locale(En)`；65 处 `&content[..content.len().min(N)]` 按**字节**切 UTF-8（实测 `end byte index 300 is not a char boundary`——把看得见的断言失败变成看不见的 panic）⇒ 全改按字符截断；`sysinfo` 首采预算 3s→5s（实测并跑 4.24s / 串行 1.3-1.7s，逐处给判别间距） | M77 |
| C-78 | `a2cf55e` | 第 6 维 `budget_discipline` 零消耗取 NEUTRAL（实测写侧 `record_consumption` 生产调用 **0**，`DecbCostCallback` 无生产构造点）+ 两处「非空转生产调用路径」现在时态订正 + 两条测试按新语义改写（先注入真实消耗再断言） | M58（诚实降级路径） |
| C-82 | `844671d` | `subagent_engine` 装配态订正为 **GATED-UNWIRED**（零生产构造；`chimera-cli` 内 `subagent` 0 命中）+ `dep_edge_freeze.txt` 该边加 2026-10-27 实测注记（沿该文件 mas-sched 行先例，行保留） | M62 |
| C-69 | `1c98ae2` | `ExperienceCardBus` 四索引上界**明示豁免**：实测「谁兜底 = 没人」（上游 `add_card` 同样无上限），写清三条不淘汰理由 + 两条开工条件；零代码语义 | M54 |
| C-109 | `83fa7ba` | **修 C-107 自引入的 clippy 红**：65 处 `&content.chars()...collect::<String>()` 去多余 `&`（`cargo test`/`nextest`/`fmt` 全不报，只有 `clippy --all-targets -- -D warnings` 报 16 条 `useless_borrows_in_formatting` ⇒ CI「fmt · check · clippy · test」步会拦）；并把 `sysinfo` 时间断言从**负载量**改钉到**承诺量**：render < 250ms 硬管（面板设计承诺「构造即采集、render 不阻塞」），OS 采集只留实现级回退上界 15s（实测 1.3-1.7s 串行 / 4.24s 51 条并跑 / 6.40s 1765 条并跑，任何常量阈值都会产「负载红牌」） | M77 后续 / T-20 |
  > 本表是收口时点快照；「已落地多少单」的真值源是 `git log --oneline`，不在本表登记总数。

**收口时点验收读数（2026-10-27，均在 C-63/C-83 全链落地之后现取）**：

| 项 | 读数 | 与首轮对比 |
|---|---|---|
| 全门集 `run_gate_manifest.py` | **第 13 轮 `[SUM] ok=129 mismatch=0 env-fail=0 unknown=0 skip=7 / 136`**（第 9 轮复跑，在 C-80/C-81/C-102 之后） | 与首轮 `ok=126 mismatch=1` 对比；累计 7 次复跑均 `mismatch=0`（含本轮两次自造门红 G-27/G-35、G-70 已修回） |
| 全量测试（`--exclude chimera-e2e-tests --lib --bins --tests --jobs 2`） | **447 target / 11386 passed / 0 failed** | 首轮 447/11375/**4 failed** ⇒ target 全集相同、失败清零、通过数 +11（本轮新增断言），旧 4 条 flaky 本次全绿 |
| heavy 门 `--with-heavy`（单独申报） | **rc=2 / `[SUM] ok=130 mismatch=0 env-fail=0 unknown=6 skip=0 / 136`**；6 条判**不可判定**（G-12/G-13/G-14/G-15/G-49/G-50），原因逐字自报：`磁盘 6.3 GB < 门槛 20 GB / 25 GB —— 不静默跳过，判不可判定` ⇒ **不得当作通过**，载体交 CI；本机 `target/` 实测 109.9 GB、Free 一度 0.00 GB |
| CI doctest job 口径（`--doc --no-fail-fast`，无 `--ignored`） | 全 `test result: ok` | — |
| release `--ignored` 档 | **首次真跑出数**：528 条 `test result:` / 77 passed / 5 failed（全为存量隔离项，M74） | 首轮 `test_result:` = 0（OOM，M73） |
| 依赖双源 | `.sh` + `.ps1` 均 rc=0 | — |
| 契约/视图派生 | `crate_contract --check` rc=0、`emit_architecture_views --verify` rc=0、`dep_edge_freeze` rc=0（157==157） | — |
| 格式 | `cargo fmt --all` 撞 os error 206 ⇒ 逐包：seccore / session-store / chimera-cli / parliament / quest-engine / efficiency-monitor / chimera-mas 全 clean | — |

> **经验沉淀（两次门红都是同一类）**：改了上游却忘重派生 ⇒ G-27/G-35（两本持锁台账行号）与 G-70（views/ 六件）会分别变红。开工前清单：`crate_contract --install` + `emit_architecture_views --write` + 行号变动时的两本台账，三者同批。
- **C-73（M56 口径传导）已执行；入库情况经逐份 `git ls-files --error-unmatch` 实测，并**订正本 ADR 先前的一条错判**：上轮曾记为「六份 md 全被 `.gitignore:57` 排除 ⇒ 无提交可证」，该判断错。现势：
  | 文件 | tracked? | 处理 |
  |---|---|---|
  | `agents.md` | ✅ tracked（且开工前干净） | **已入库 `9a77d47`（C-73b）** |
  | `CONTRIBUTING.md` | ✅ tracked（且开工前干净） | **已入库 `9a77d47`（C-73b）** |
  | `.trae/rules/nuxus规则.md` | ✅ tracked，**但开工前即为他批 ` M`** | 文本已改、**故意不提交**（整文件 add 会把他人未提交工作归到 R2 名下）；待其收口后同批入库 |
  | `.claude/CLAUDE.md` / `.qoder/rules/chimera.md` / `.qoder/rules/chimera2.md` | ❌ ignored | 仅本地权威源，无提交可证 |
  （`.gitignore` 不会取消已跟踪文件，故规则之前入库的 md 仍 tracked —— 这正是上轮错判的原因：从「有条 `*.md` 忽略规则」直推「所有 md 不可审」。）
  **推论：M43 的有效范围比初记的窄**——不可审的是「忽略规则之后未入库的文档」，而非「所有权威 md」。**且本行初版点名的清单本身也有错（C-91/M68 逐份双验推翻）：被列进来的 `docs/architecture/CODE_WIKI.md` 与 v3.4.0 二十三篇权威设计文档均在索引内（tracked），`docs/reports/*` 也不可整目录概括；真正未入库的仅剩 `adr_index.md`、`docs/tui/README.md`、十六源手册 v3.0、`CHANGELOG.md`、`.claude`/`.qoder` 规则副本与本批 `docs/reports/l0l10-r2-*`。D5 方案乙仍成立，因为本轮关键订正确有落在真未入库的文件上（M42 的六单）。
- **对 R1 总账的事实订正（已以 C-76 入库）**：C-08/09/25/31/36 已提交；C-27 内容已在 HEAD 由 `7b7e571`(C-45) 承载；C-42/44/47/48/54/55 目标态已达成 ⇒ **R1 未完面 = 仅 C-29**。
- **基线**：`cargo check` rc=0 / `cargo clippy` rc=0 / 真全量 `cargo test --no-fail-fast` = **447 target / 11375 passed / 4 failed**（4 条逐 target 隔离复测 0 failed ⇒ 并行负载敏感型 flaky，登记 M57 不作回归）；全门集修复前 `ok=126 mismatch=1 skip=7/134`，G-85 解除后与 G-135/G-136 入册后的复跑读数见 `docs/reports/l0l10-r2-baseline-2026-10-26.md`；release `--ignored` 档在采「构建与运行解耦 + `--jobs 1`」执行中，尚未出数。

## 5. 待裁议程（R2 移交）

| # | 事项 | 建议方向 | 前置 / 触发条件 |
|---|---|---|---|
| T-1 | C-64b `SandboxViolation` 升档八组全套 | 独立批次，单一提交 | 在途 `fuzz/fuzz_targets/harness_spec_parse.rs` 收尾 + G-115 closed/open 裁决 |
| T-2 | M13 loom 并发正确性三态 | **已裁（2026-10-26）：允许 loom，但仅作 dev-dependency**；在 `hcw-window` / `mlc-engine` 两处持锁-广播先例首批落地 | 前置 = 按 ADR-222 附录 §9 补四项最小性论证并另立 ADR；不优入生产二进制 |
| T-3 | M36/M55 `ExperienceCard` schema 版本化 | **已裁：不允许新外部依赖** ⇒ 走「作废愿景行」。**但本行初版的措辞已被实测推翻一半，按 M69 逐要素窄化**：① BLAKE3 内容哈希 = **不采纳**（`Cargo.lock` 内 `blake3` 0 命中，G-99 白名单未开）——裁决的可执行部分不变；② `schema_version` / `content_hash` 两个字段名 = **已在盘上使用 8 处 pub 字段位**（`checkpoint.rs:58` u16、`token_evidence.rs:122`+`:260` u16、`routing_history.rs:125` **u64**、`event_metadata.rs:61` `payload_version: u32`、`chimera-mas/archive/dedup.rs:131` `content_hash: u64` vs `nmc-encoder/types.rs:129` `content_hash: String`）⇒ 不得登记为「本仓不采纳项」，只能登记「作为 Memory Capsule 统一契约字段 + BLAKE3 算法 = 不采纳」；③ 「先统一 L0-A 内部范式」的实际范围也比 M55 记的大（u16/u32/u64 三宽度 + u64/String 两哈希型），升契约属破坏性变更 ⇒ 不开码改单、转新待裁 T-14 | 手册对应章节已按「只加注不改写」追加两条批注（见 D9 引文）；升 `ExperienceCard` 契约破面实测 67 处（src 32 / tests+benches 35，无 `impl Default`） |
| T-4 | M43 治理文档入库政策（方案甲窄口径反白） | 独立授权批次 | 须同批改 `check_doc_consistency.sh`/`adr_index_sync.py`/G-37 的 gitignore-skip 断言语义；边界已经两次逐份实测收窄（C-91/M68：反白面仅剩 `adr_index.md` + `docs/tui/README.md` 两份，`CODE_WIKI.md` 本就 tracked）。**新证据：仓内已有先例可不动 `.gitignore`**——`docs/reports/toolchain-analysis-report.md` 与 `v3.5-baseline-measurement-2026-10-10.md` 两份非反白名单报告已在库（`git add -f`）⇒ 本批六件套是否入库可选「逐份点名 add -f」，该权限仍属属主，R2 未自行行使 |
| T-5 | M48 审计导出点定位 | 启动 + 导出双点 | 若无稳定导出入口则退化「启动 + 关账」并回写本 ADR |
| T-6 | G-135 判据口径：`impl From<第三方>` 是否计入 | **已裁：计入。✅ 已落地 C-89（`8407444`）**：新增 `implfrom` 位置类（ret/arg/bound/implfrom 四类齐备），台账 13→18 行，selftest 14→16 | 当场多捕出 G1 深审未报的一条泄漏 `nexus-core::error::NexusError::serde_json::implfrom` ⇒ 口径扩张非纸面动作；`rmp_serde` 两条（encode/decode）被多重集正确计为 2 行 |
| T-7 | L01 sibling facade 单消费者翻案 | 维持观察 | G-129 only-decrease 棘轮翻案需 ADR；触发 = 装配面事故 |
| T-8 | M16 coverage 46-crate 基线未重测 | CI nightly 出数后抬表 | 本地 llvm-cov E0463 不可代跑 |
| T-9 | M14/M19/M20 + M44/M45/M46 性能可证伪族 | 补 bench 阈值键 / 给 SLO 找 CI 载体 / 第四表键集对等 / 十六源目标三态 | 需 CI clean-runner 取数；改 release profile 属禁区 |
| T-10 | M24/M25/M26/M28/M29/M30/M31/M33/M34/L21/L37 零装配族（**含 M59 事件生产者装配可达性**） | 逐条「接线 / GATED / 退役」三选一（R2 已出全量清点表，不再逐单发现）；**M59 的第二轴已由 C-79 落地**：台账 `scripts/event_assembly_gated.txt` 现登记 **15** 条「有构造点但默认构建不可达」的事件（跨 9 个非默认构建 crate）⇒ 裁时可直接拿这张表逐条选路 | M27 属 R2 解冻前必清项居首；M33 继承 ADR-224 D4 DEFERRED 不接线；退役需 ADR |
| T-11 | M57 四条时序断言确定性改造 | 固定 `--test-threads=1` 或抽纯函数夹具（沿 ADR-222 R9 `eco_transition` 先例） | 禁改生产语义（Eco 降频是正确的事件风暴保护） |
| T-12 | 六条 `[[slo]]` 的 `continue-on-error` 切换 | 维持现状（R2 复测判为已明文登记的计划债，非疏漏） | `slo.yml:24` 已写切换条件；满足后改 `slo.yml:68`/`:170`/`stress.yml:38` 为 `false` |
| T-13 | M50/M51 之外的 L3 同族（`scc-cache::SqliteWal`、`cmt-tiering::CmtCoordinator`、`hcw-window::spawn_mask_listener` 未接线而文档以现在时声称闭合） | 与 C-63 同批判「接线 or 显式装配态订正注」，并同批重派生契约头 | 各为生产零构造/零装配（R2 复测亲验行号） |
| T-14 | M69：格式版本字段已存在但未统一（`schema_version` u16/u16/u64 + `payload_version` u32 + `content_hash` u64 vs String） | 先只统一**声明处语义**（同一宽度 + 同一哈希类型），再议是否升为 L0-A 契约字段；不引新外部依赖（T-3 已裁） | 属破坏性变更（wire + 67 处构造点），需与 C-63 同族重派生链同批；需 target 锁与属主授权 |
| T-16 | M72：**审计链在生产零写入**——`audit_and_execute` 生产调用 0（唯一调用方是 `tests/security/owasp_top10.rs` 17 处）；`AuditChain` 无持久化/导出 API；真实执行路径 `gqep-executor`(ExecPolicy)/`nexus-hook`(ProcessFence) 不写链 | 三选一：**A（本 ADR 默认）** 保留 C-64a、链登记 DORMANT-ONLY、红线 #2 措辞按实测收窄；B 授权把两条真实执行路径接进 seccore（改生产语义，需新 ADR + 功能回归面）；C 记 DORMANT 并同批订正 `policy.rs:320` 文本 | C-65（`verify()` 双点）**因此不在本轮实现**：空链 verify 恒真 = 装饰门，导出点不存在。任何选 B 的落地需先回答「白名单会拦掉哪些现在能跑的命令」 |
| T-17 | M73：release `--ignored` 档在 `--jobs 1` 下仍 **LLVM OOM**（`free≈423 MB` 时；同单元在 `free≈1.9 GB` 下 10m58s 编成功）⇒ 根因是内存竞争而非代码/flags | **✅ 已出数（2026-10-27）**：单进程重跑 `rc=101`、ELAPSED 2566s、**528** 条 `test result:`、**passed=77 / failed=5**；五红全为**存量隔离项**（4 个 `#[ignore]` 态 doctest + 1 条 `#[ignore]` 标定测试 panic，逐文件 `git log` 已证无一属 C-57..C-95）⇒ 登 M74。**CI 仍绿**：去掉 `--ignored` 的 `ci.yml:809-810` 口径在 debug 下复跑全 `test result: ok` | 仍待裁的口径问题：AGENTS §7.2 第 7 步（`--ignored` 档）在 HEAD 上**永不可能转绿** ⇒ 需裁「清隔离项（`ignore`→`text`/`no_run`）or 改清单口径」；禁改 release profile（D-R2-2） |
| T-18 | M76：`cargo audit` 在当前 advisory DB 下**大批量红**——逐字取 `audit.yml` 的 `--deny unmaintained --deny unsound` + 6 条 `--ignore` 后 **rc=1**：`36 vulnerabilities found!` + `13 denied warnings found!`（扫 700 crate 依赖），命中 13 个 crate：`quinn-proto`/`rustls`/`rustls-webpki`/`webpki-roots`/`wasmtime`/`zip`/`zstd`/`zstd-safe`/`zstd-sys`/**`tokio`（RUSTSEC-2026-0386 `AsyncFd::new` 违反 I/O safety，unsound）**/`chacha20`/`anymap2`/`unsafe-libyaml` | 二选一：**（a）**升 `quinn/rustls/wasmtime/zip/zstd/tokio` 到已修复版（需评 API 破面 + 走 G-08/契约重派生）；**（b）**按 `audit.yml` 现有惯例逐条加 `--ignore` 并写清「不可达/暂无修复/仅构建期」理由，同批改 `AGENTS §7.2` 的 ignore 清单 | 归因为**非 R2 引入**（R2 只加 dev-only `loom`，与上列 13 crate 无交集；RustSec DB 随时间新增 + 仓内版本未动 = 时间依赖存量红）；不擅加 ignore（覆盖面政策变更）、不做无验证依赖升级；同类先例见 M39/M74「门在、但没人跑它」 |
| T-19 | M58 写侧：`budget_discipline` 的生产利用率**恒为 0** —— `record_consumption` 生产调用数 0，`DecbCostCallback` 无生产构造点（C-78 已把该维度降级为「无观测 → NEUTRAL」） | 接写侧：在现有的 `StreamSessionCompleted` 订阅循环里回算 `cost_actual_micro` → `DecbCostCallback::record_cost`（同 `omega-learner::s9_integration` 先例）；接上即删掉 C-78 的零消耗 NEUTRAL 分支 | **改生产语义**：`switch_tier` 降档与 `BudgetExceeded` 事件面会真的被触发 ⇒ 需属主授权 + 新 ADR + 「哪些现有会话会被降档」的回归面；不得以「让维度有数」为名顺手接 |
| T-20 | M77/M78：`cargo nextest run --workspace --profile ci-fast` **rc=100**（11388 tests: 11380 passed / 8 failed / 65 skipped）；8 条逐条归因后 C-107 修掉 6 条，余 **2** 条为负载敏感 | 余 2 条（`history_wiring_test::pipeline_with_history_backfills_and_persists`、`mcp_nodes_panel_test::test_mcp_nodes_panel_no_alert_when_all_online`）按 M57/C-83 同款「轮询至收敛」改造 | 实测：单独跑 1.8s/2.2s 全绿、并跑 8s+ 才红 ⇒ 判负载敏感、非回归；`nextest` 档因此仍非绿，**收口判据仍是门集 `[SUM] mismatch=0`（D2）**，不当它是「CI 红」 |

## 6. 验证与回滚

- **本 ADR 落档验证**：`adr_index.md` 行与磁盘实体名一致；`py -3 scripts/adr_index_sync.py` rc=0（BROKEN LINK 无 / UNINDEXED 无）；`bash scripts/check_doc_consistency.sh` rc=0。
- **回滚**：文档面 = 删本文件 + 删 `adr_index.md` 对应行（两文件同批）。代码面按交付物 04 §3 的耦合组**整提交 revert**，禁半 revert（尤其 C-64b 八组、C-66/C-89 四件与台账、C-63 装配链、C-61/C-62 与 C-63 的先后序）；已落地单的逆序回滚清单见 §4 两段表格（不在此登记数值，总单数是 `git log` 的函数）；任何 revert 后重跑全门集 `[SUM]`（收口判据 = `mismatch=0`）+ `cargo check --workspace --all-targets --jobs 2`。
