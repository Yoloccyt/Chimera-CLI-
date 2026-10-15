# ADR-216: L0 第二轮审计收口 —— ImmutableSurface↔mpsc 对拍门（G-117/118）、G-99 SHARED-STATE 第六类杂质、契约测试补强与文档订正

- **Status**: ✅ Accepted (2026-10-15)
- **关联 ADR**: ADR-033（L0 纯度口径）/ ADR-212（§7.1 ImmutableSurface Critical 覆盖 6→17 承诺）/ ADR-213（门四件同批、轴分工禁互抄基线）/ ADR-214（档 2）/ ADR-215（第一轮收口，本 ADR 为其二轮审计的继承）
- **权威源**: 代码实测（2026-10-15，分支 `feat/c1-compute-plane-governance`）。三个审计子代理（依赖与结构 / 事件并发安全 / 存储性能测试文档）+ 主会话逐条复测。

## 1. 背景

ADR-215 收口后，对 L0 做十维度系统审计（审计清单与逐条证据见 `docs/reports/L0-round2-ten-dimension-audit_2026-10-15.md`）。合计 2 个 P2 级、9 个 P3 级真实发现（另有 3 条审计初判经复测降级为「非缺陷」）。其中最重的一条是**跨 crate 对拍缝**：ImmutableSurface 的 Critical 覆盖由 ADR-212 §7.1 承诺（17 = 16 mpsc + CheckpointSaved），但 bus 侧五把锁全部活在 event-bus 域内——新增 mpsc-Critical 变体时 L0 侧静默漏同步，spec 可悄悄降级该事件的通道保障。这正是本仓反复治理的「承诺在散文、判据为零」形态。

## 2. 决策

### 决策 1（D-30）：建 G-117/118 —— ImmutableSurface ↔ bus mpsc 双向集合对拍门（含 CI 接线，用户授权）

- 脚本 `scripts/check_immutable_surface_parity.py`（四件同批：脚本 + `gate_manifest.toml` 两条 + `ci.yml` 一步，接线已获用户明确授权并应用）。
- **判据两向皆红**：MISSING（mpsc 变体不在 ImmutableSurface Critical 面）/ EXTRA（L0 覆盖既非 mpsc 又无 broadcast-only 登记行——任何此类新增都是刻意动作，须付出带理由的登记行成本，同提交）。
- **解析同源**：注释/字符串经 G-99 同一 `blank_rust` 词法器剔除；bus 臂集与同文件 `CRITICAL_MPSC_VARIANTS` 计数常量互锁；`RedlineCriticalEventMpsc` 因不以 `Critical` 开头天然排除。
- **不可判定退 2**：输入缺失 / 锚点查无 / 解析数低于地板（10/10）/ allowlist 行理由 <24 字符——盲解析不得报「对拍通过」。
- **现取实况（2026-10-15）**：mpsc=16、Critical=17=16+CheckpointSaved，双向零缺口；CheckpointSaved 的 broadcast-only 身份经带理由 allowlist 行登记。
- **selftest 接线前咬出三处实现缺陷**（全部修复后才入 CI）：① 变体正则缺 `re.MULTILINE` 致 real run surface 恒 0；② 工作树 CRLF（743/743 行）致 `find("\n}\n")` 块尾定位失效；③ 两条断言串与消息格式错位（门逻辑正确、断言写错）。此为「自造扫描器必须先植入坏例自检」纪律的又一次实证。
- **披露边界**：lane-only 的 3 个 broadcast Critical（`ConsensusReached`/`SlowConsumerDropped`/`OrphanCallDetected`）按 ADR-212 口径**不在** ImmutableSurface 面（17 = 16+1 的承诺边界），本门不追；若未来要把它们纳入，须同批扩 allowlist 行（刻意动作）。
- **反方案**：①「并入 G-113/G-109 之一」——否决，轴分工禁互抄基线（ADR-213 R8.3），本门分母 = 事件名集合，与字段序/孤儿文件互不相交；②「不建门靠 code review」——否决，漏同步不产生任何编译错/测试红（bus 五锁全在域内），review 是唯一防线时的失败模式是静默通道降级。

### 决策 2（D-31）：G-99 增第六类杂质 SHARED-STATE

- 规则（限定模式，防假阳性）：`std::sync` / `Mutex` / `RwLock` / `Rc<` / `RefCell` / `Cell<` / `UnsafeCell` / `Atomic{Bool,I8..Usize,Ptr}` 显式类型名 / `std::thread` / `thread::spawn` / `OnceLock` / `OnceCell` / `LazyLock` / `lazy_static`。
- **为何限定**：裸词会误报真实标识符——`AtomicOperator`/`AtomicCardType`（L0 真实类型名）与 `protocol/app.rs` 的 `thread: Thread` 字段。两条反例已钉进 CLEAN 夹具。
- 真树现状：64 文件 0 命中（此类别是**防回潮的新增准入**，非存量清算，故无豁免行）；DIRTY 夹具每规则一植入行，selftest 全绿。
- **反方案**：把 `panic!`/`unwrap` 并入本类——否决，panic 策略归 E-5 lint 棘轮轴（且 `rl_types.rs:220` 是显式文档化 panic 契约，一刀切会误杀）。

### 决策 3（D-32）：契约测试补强（本轮执行）

- `safety/adjudication.rs` 补 `#[cfg(test)]`：8 类型全部 JSON+MessagePack 双向往返（4 enum 全变体 + 4 payload 含 None 路径）——此前该模块零测试引用。4 个 struct 已 `#[non_exhaustive]`（ADR-215 D-23），跨 crate 禁字面量构造，测试落同 crate 单元测试；`wire_roundtrip.rs` 头部同步登记该覆盖位置（消除「未覆盖也未登记」的静默遗漏）。
- `CommandValidator` 补 L0 侧 mock 调度测试（实现方在 seccore，错误载体 `CommandValidationError` 传递须本 crate 可证）；`DynamicEvent` 补 mock 六方法调度测试（此前仅 Send+Sync 编译断言）。
- `benches/util_micro.rs` 补 `sigmoid(map 1024)` 零分配硬断言（与 scalar/percentile 两路对齐）。
- 实测：lib 752 passed（+4）；clippy `-D warnings` rc=0。

### 决策 4（D-33）：文档订正批（本轮执行）

「144 枚举」陈旧口径 8 处 → 152（真值源 `event-bus/types.rs` 现测，doc_drift 门自报同值）；harness_spec 模块头「13 条红线 + 6 Critical」→「8 条红线 + 17 Critical」（实测 Redline*=8 / Critical*=17）；`lib.rs` sensitive re-export doc 删 `looks_high_entropy` 残留（ADR-215 D-24 后实况）；`SegmentMetadata::new` 补 `# Panics` 文档（E-5 自订口径）；ADR-214 §6.1 补 ADR-215 继任指针（实档）；adr_index ADR-214 行双向化；CODE_WIKI §3.0 补 open 化口径 + §2.1 变更注旧口径订正指针；agents.md §5.1 Checkpoint 字段清单同步（含 description/schema_version）；ADR-215 D-27 补登 `ContentBlock`（mca-gateway SQLite `session_turns` 经 `to_vec_named` 落盘，未版本化）入触发条件点名清单；ADR-215 §6 宏盲区注记（`paste` 仅为 Cargo.lock 传递依赖、workspace 源码零使用，二轮实测）。

### 决策 5（D-34）：降级与待裁登记

- **bench 覆盖缺口（defer）**：`fuse_ranked_lists`（recall_fusion）/`sinkhorn_project`/`project_weights`（mcsm）/`SparseMask::select_top_k`（masks，83 处外部引用）全仓 benches 零覆盖。不新增 bench 的理由：本仓纪律「无 clean-runner 不标定」（SLO 三表零改动，P-2/F-1），本地读数不得入三表；触发条件：SLO/bench 批次立项时随批补 bench 并走 dev-only 或 registered 台账。
- **`bench_check.yml:265` 注释「xts_top_k 实测 213µs」出处不明（待复核）**：全仓 docs/scripts 无第二出处；本机既有基线为 360µs（n=100000，机器相关）。不改数字（无 clean-runner 不标定），登记待 SLO 批次复核出处或重测。
- **MetadataBridge 全仓零 impl**：已由 G-91 基线 + ADR-215 D-24 DEFERRED 承载，本轮不动。
- **ImmutableSurface 3 个 lane-only Critical 未覆盖**：与 ADR-212 口径一致（17=16+1），本 ADR 披露登记，不视为缺陷。
- **G-91 基线 161 行中与本轮无关的其他低消费者行**：超本轮审计范围，未逐个复核（诚实登记）。

## 3. 影响

| 维度 | 影响 |
|---|---|
| 代码 | 5 个 src/test/bench 文件（adjudication 测试模块、command_validation 与 event_v2 mock 测试、util_micro 断言、8 处注释订正）；零生产语义变更 |
| 门 | 新增 G-117/118（manifest 116→118、ci.yml +1 步骤）；G-99 规则 13→19 条（+6 SHARED-STATE）+ 类别集 2→3；其余门零变更 |
| 台账 | 零台账行变更（新门系新增准入，无存量行；G-115/G-105/G-107 基线不动） |
| 依赖/事件/API | 零变更（152/20/16 三锁不动；crate 数 39；公共 API 零增删） |
| unsafe | 零新增 |

## 4. 验收（全部 2026-10-15 实测）

- `audit_gate_parity` rc=0（"no parity gap"）；`check_gates_provenance` rc=0 双向。
- G-117/118 selftest（11 断言）+ 真树 rc=0；G-99 selftest（含 6 条 SHARED-STATE 植入行 + 2 条反例）rc=0、真树 0 命中。
- `cargo test -p nexus-contracts --lib` 752 passed / 0 failed；`--test wire_roundtrip` 15 passed；clippy `-D warnings` rc=0；fmt 绿。
- ci.yml 编辑后 YAML 解析通过；G-115/G-105/G-107/G-113 门全绿。

## 5. 回滚

逐提交 `git revert`（e8032d4 R2-A / 4bc7437 R2-B / b3ec83c R2-C / 本 ADR 批次）：G-117/118 与 manifest/ci 同批，revert 即门与脚本同进退（防 G-103 方向 A 死判据）；ci.yml revert 后复跑 `audit_gate_parity` + `check_gates_provenance` 防 YAML 连带失明。禁 `git checkout --`/`restore`/`stash`/`add -A`。

## 6. 未做 / 待裁（诚实登记）

- 十维度审计中的三项「初判」经复测为**非缺陷**（登记以正视听）：`paste` 宏盲区（传递依赖、源码零使用）；G-91 基线 161 行的规模（与 ADR-214 §6.2 的 S7 分桶同题，本轮未分桶）；`dyn VectorStore`（ADR-215 D-28 已 reject）。
- bench 缺口与「213µs」复核见 D-34 触发条件。
- 审计代理报告一处引文笔误（把 ImmutableSurface 台账来源写成 ADR-213 §7.1，实为 ADR-212 §7.1:99）已在报告订正，代码/文档无此错。
- lane-only 3 个 broadcast Critical 是否纳入 ImmutableSurface：留待治理批次裁决（纳入须同批扩 allowlist 行）。

## 7. 执行记录

| 提交 | 内容 |
|---|---|
| e8032d4 | R2-A 文档订正批（144→152、harness_spec 头、lib.rs doc、#Panics、ADR/CODE_WIKI/adr_index/agents.md/ADR-215 补登） |
| 4bc7437 | R2-B 测试补强批（adjudication cfg(test)、两 trait mock、map 零分配断言、wire_roundtrip 覆盖登记） |
| b3ec83c | R2-C 新门批（G-117/118 四件同批 + G-99 SHARED-STATE） |
| （本批） | ADR-216 落档 + adr_index + 审计报告 |

审计报告：`docs/reports/L0-round2-ten-dimension-audit_2026-10-15.md`（十维度逐条：问题—证据—违反条款—严重级—处置）。
