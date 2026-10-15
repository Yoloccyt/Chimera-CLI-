# ADR-217: L0 第三轮收口 —— 值级 facade 两条不变量锁、`select_top_k` 单元 bench 与登记项证据化

- **Status**: ✅ Accepted (2026-10-15)
- **关联 ADR**: ADR-033（L0「纯类型 + 受控纯逻辑」口径）/ ADR-212（D-7 双 Top-K 共存裁决、§7.1）/ ADR-213（门四件同批、R8.3 轴分工禁互抄基线）/ ADR-214（D4 消费者四口径、§6.3「无 clean-runner 不标定」）/ ADR-215（档 2 与三候选）/ ADR-216（D-34 登记项——本 ADR 收口其 bench 缺口与「213µs」）
- **权威源**: 代码实测（2026-10-15，分支 `feat/c1-compute-plane-governance`，基线 `b4d0ff6`）。三个只读审计子代理（结构面 / 行为面 / 一致性面）+ 主会话逐条复测；逐条证据与命令见 `docs/reports/L0-round3-facade-and-bench_2026-10-15.md`。
- **编号合法性**: 落档前实测 `ls docs/architecture/ | grep -c ADR-217` = 0、磁盘主编号最大为 ADR-216、`adr_index.md` 无 217 预留行；本 ADR 编号在 R3-A（`af29d1a`）即以指针形式写入代码注释，实档于本批补齐（同批闭环，不留悬空指针）。

## 1. 背景与范围裁决

ADR-215/216 已收口 L0 两轮。本轮按十维度清单做第三轮，重点补做前两轮未覆盖的**维度 3 专项（公共 API 泄漏 / 值级 facade 一致性）**，并把 ADR-216 D-34「已登记但含糊」的维度 8 残余项**以证据闭合或改写触发条件**。

**范围裁决（最高优先级结论）**：残余项中**没有任何一项能靠只改 L0 清零**——档 3（`ExperienceCard` 61 处字面量，跨 9 crate）、D-27 `schema_version` 铺开（跨层 + 触 `HarnessSpec::canonical_merkle_input` Merkle 基线）、lane-only 3 Critical 入 ImmutableSurface（跨 event-bus + 需改 ADR-212 §7.1 承诺）、S7 161 豁免分桶（新门轴）、L0 覆盖率 floor（本地 `cargo llvm-cov` E0463，工具链缺件）全部维持 defer，并逐项写前置。本轮只做**真 L0 内可闭合项**，改动面严格限于 `crates/nexus-contracts/**` + `scripts/bench_inventory_freeze.txt` + 文档，**零 `.github/workflows/` 变更**（故无需新增授权）。

## 2. 决策

### 决策 1（D-35）：prelude 改为「顶层高频子集」口径 + 两条不变量机器锁（P2 → 修复）

- **问题**：`lib.rs` 的值级公共面有两层（顶层 `pub use <leaf>::{…}` 与 `pub mod prelude`），18 处分组注释一律声称「与顶层导出同集」，而**该承诺无任何机器判据**——实测已失守两处：prelude 收了 `behavior_contract` 的伴生类型却漏收主类型 `BehaviorContract`；`formal_props` 收了 5 项漏收 `EmptyFormalProvider`/`FormalResultProvider`/`VerifiedWithStrength`。
- **证据（本轮现取，由锁测试自身产出）**：`cargo test -p nexus-contracts --test prelude_surface_test -- --nocapture` → `[facade-scale] stmt_top=41 stmt_prelude=37 value_top=213 value_prelude=191 top_only=22 pre_only=0 absent_families=0 missing_by_family={"blueprint":4,"scheduler_contract":9,"tool_plan":7,"variant":2}`。修复前 `top_only=26` → 补 4 名后 22（`grep -c "与顶层导出同集"` 修复前后同为 18，语义由全集陈述收窄为分组级陈述，见 `lib.rs:689` 的口径行）。
- **违反条款**：ADR-033 L0 门面可预期性；本仓反复治理的「承诺在散文、判据为零」反模式（ADR-216 D-30 同形）。
- **处置=修复**：① prelude 补齐两族（`behavior_contract` 4 项 / `formal_props` 8 项，与顶层逐名相等）；② 口径订正为「prelude = 顶层值级导出的**高频子集**」；③ 新增 `tests/prelude_surface_test.rs` 把两侧关系锁成两条不变量——**(a) 无孤儿**（prelude 每个值名必须在顶层出现）、**(b) 同族完备**（某 leaf 模块在 prelude 有任一成员，则该模块在两侧的成员集合必须相等）；整族缺席 4 模块 22 项（`tool_plan`/`blueprint`/`scheduler_contract`/`variant`）**不判**，登记为有意取舍。
- **锁的牙口**：植入式正对照（夹具造一名孤儿 + 一处同族缺项，必须双双判出——恒假守卫等于没有守卫）+ 论域自证（真树解析取空即红，防「两侧都空即绿」）+ 规模闸门（`value_top>150 && value_prelude>100`，低于即判解析器静默漏读）。判定函数 `facade_violations` 由真树与夹具**共用同一份**，不写测试专用副本。
- **为什么落在 crate 测试而不是新增门**：G-107 只管两段式**模块级** `pub use <group>::<leaf>;`，值级不在其辖域；值级同集性是 L0 内部一致性问题，另建门需复制第二份解析器（违反 ADR-213 R8.3 轴分工与「手抄夹具测不到生产码」纪律）。本轮**零新增门**，`gate_manifest` 118 条不动。
- **反方案**：①「把 22 项也补进 prelude 达成真同集」——否决：prelude 是便捷 glob，`ToolOp`/`Lease`/`Quota`/`Priority`/`PlanError` 等泛用名入 glob 会扩大 36 个声明消费者的名字冲突面，而当前外部对这些名字零消费 ⇒ 收益零、风险为编译级；②「只改注释不加锁」——否决：注释正是本次失守之物，无锁的下轮仍回到人工；③「写成 python 门脚本」——否决：见上条轴分工，且门脚本要复制 lib.rs 花括号感知解析，双份解析器必然漂移。

### 决策 2（D-36）：`SparseMask::select_top_k` 单元 bench（dev-only）+ 三函数以证据收尾（P3 → 修复 + 证据化登记）

- **生产热路径证据（本轮现取）**：限定调用 `SparseMask::select_top_k` 在 L0 外 **14 处 / 2 文件 / 唯一生产 crate `osa-coordinator`**（`coordinator.rs` + `parallel.rs:211/220/235/259/272` 五维各一次），其上游是 `chimera-mas/src/context/manager.rs` 每请求构建提示词链路；测试面 `osa-coordinator/tests/{masks,proptest}.rs`；L0 内 `sparse/omni_masks.rs:136`。别名导入形态经查零命中（无 `SparseMask as …`）。
- **补的是 L0 单元级基线**：端到端形态已由 osa-coordinator 的三个 bench 覆盖，故本文件与它们不重叠——双组对照 `select_top_k`（`select_nth_unstable_by` 路线）vs `full_sort_control`（红线 #8 禁止形态的**回退哨兵**：实现哪天退成全排，(1) 读数即贴到 (2)），另加 `ToolId` newtype 泛型实例化组（消费方真实形态）。分数源用 LGC 确定性伪随机，**不引入 `rand`**（L0 依赖白名单铁律）。
- **登记态 = dev-only**：`scripts/bench_inventory_freeze.txt` 一行 `nexus-contracts|sparse_select_top_k|dev-only|…ADR-217 D-36`（dev-only 61→62，I6 下限 60 满足）；`Cargo.toml` 第三个 `[[bench]] harness=false`。**读数不入 SLO 三表**（ADR-214 §6.3「无 clean-runner 不标定」；先例 `chimera-cli|composition_root_bench`）。本轮只跑 `cargo bench --no-run` 编译门，**未产出任何 ns 数字**。
- **三函数不补 bench，改为证据化收尾**（替代 ADR-216 D-34 的含糊待办）：`fuse_ranked_lists` / `sinkhorn_project` / `project_weights` 在本仓**零生产调用方**，指针为——
  - `fuse_ranked_lists`：唯一调用点 `mlc-engine/src/recall_fusion.rs:122`（在 `pub fn fuse_recall_paths` :107 内）← `engine.rs:1000`（在 `MlcEngine::recall_hybrid` :947 内）；`recall_hybrid` 的 src 面调用方 0，其唯一外部消费者是 `mlc-engine/tests/recall_hybrid_test.rs`。
  - `sinkhorn_project`：唯一调用点 `parliament/src/voting.rs:343`（`mix_signals_mcsm` :341 体内）；`project_weights`：`voting.rs:353`（`normalize_vote_weights_mcsm` :352 体内）；三个 `_mcsm` 入口在 `voting.rs` 之外全库 0 命中，其使用位（:1080 起）全部落在 `#[cfg(test)]`（起 :602）之后。
  - **触发条件改写**：`recall_hybrid` 或 mcsm 任一入口接入生产事件路径时，随该批补 bench（而不是长期挂「待补」）。为无调用方的函数造性能证据属装饰，且其读数会被误当 SLO 依据。
- **与既有裁决的边界**：L0 存在两条 Top-K 路线（本文件的 `SparseMask::select_top_k` 与 `util::xts_top_k{,_by}`，后者由既有 `benches/xts_top_k.rs` 覆盖）。**两者共存是 ADR-213 D-7 的现行裁决**，本 ADR 不提议合并，并已在 bench 头写明分工，防止下一轮把它当重复实现再提一次。
- **反方案**：「既然缺 bench 就顺手把 `select_top_k` 收敛到 `xts_top_k_by`」——否决：与 Accepted 的 ADR-213 D-7 相冲，且 `select_top_k` 是 L0 类型 `SparseMask<T>` 的内联构造函数（见 D-36 台账项 7 的 orphan-rule 论证），合并属破坏性 API 改写而非优化。

### 决策 3（D-37）：「xts_top_k 实测 213µs」按事实登记，不改数字（P3 → 登记）

- **出处判定（本轮全仓检索）**：**产出方唯一** = `.github/workflows/bench_check.yml:265` 的注释自身；其余 4 处命中（`ADR-216:45`、`adr_index.md:216`、二轮报告 `:47`/`:85`）**全部是转述**（对上条注释的登记），不构成第二独立出处。⇒ 「213µs」没有可追溯的 clean-runner 记录，属**自证注释**。
- **阈值真值**：判定输入是 `scripts/perf_thresholds.toml:62` `xts_top_k = 500000` 与 yml 内 `500_000`（两处双写，数值对等**无门覆盖**——ADR-214 已登记 I1 只比键集、I7 只比 `run_bench` 行集，本 ADR 不新增该轴）。
- **本轮现读**：`target/criterion/xts_top_k n=100000/new/estimates.json`（文件 mtime **2026-10-13 20:45:44**，即本轮**读取既有产物**、非本轮重跑）median = **365,594 ns**（95% CI 353,329–372,548）；`n=10000` median = 28,084 ns。同机历史读数：355µs（`L0-contracts-refactor-audit_2026-10-12` §10）与 360µs（ADR-216 D-34）⇒ 本机波动区间约 355–366µs。
- **结论与处置**：213µs 与本机现读差约 **1.72×**，但这**不构成缺陷判据**——判定发生在 CI `bench-check` job 的 clean runner，本机读数不得当基准。处置 = 随 SLO 批次复核该注释出处，或改注为「历史 clean-runner 读数（日期?）」。**本轮不改数字**：改 `bench_check.yml` 注释触 `.github/workflows/`，需另批授权（本 ADR 的范围裁决即排除它）。
- **反方案**：把注释改成本机 365,594——否决：用无 clean-runner 标定的本机读数覆盖标定注释，等于伪造基准来源，且会诱导后续把 500µs 阈值下调到本机噪声带。

### 决策 4（D-38）：维度 3 的 P3 文档面订正 + 两项待裁登记

- **L0 文档点名上层实现类型名（P3 → 修复）**：`vector_math/vector.rs` 的 trait 文档原写死 `MemoryKnnStore`/`HnswStore`/`WikiError`/`HnswError`/`WikiEntryMeta`（10 处，实现方在 `repo-wiki`），上层重命名即让 L0 契约文档静默失真。**无门可依**是本轮实测事实：`check_doc_symbol_claims.py` 的论域只有 `docs/architecture/CODE_WIKI.md` 表格行（本轮现取 `doc=docs\architecture\CODE_WIKI.md`，460 次符号出现、12 gaps 全登记），**不覆盖 `crates/**/src` 文档注释**，故 L0→上层方向的语义对应零判据 ⇒ 改为按角色描述（「HNSW 近似实现 / 内存 KNN 实现」），并在文件内写明该理由。
- **顺带抖出一条幽灵行为注记（本轮新发现 → 修复）**：`VectorStoreExt::compact()` 文档的「SqliteVec: VACUUM 操作」——全仓 `VACUUM` **仅此 1 行**且**无实现方**（`repo-wiki/Cargo.toml:57-62` 明写 sqlite-vec 已降级、依赖为注释态；ADR-005）。原句会让读者以为存在该后端实现 ⇒ 改为显式标注「本仓无实现方，此行只描述将来接入该后端时的义务」。
- **两条实现侧行为主张经现读核对为真（保留不改）**：`hnsw_store.rs:652` 的 `compact()` 确为快照 + 重建图、`memory_knn_store.rs:265` 确为 `Ok(())` 无操作——即「先复测再采纳」也用于自己即将保留的句子。
- **`test_scale.rs` 宏契约订正**：文档原称 `scaled_timeout!`「接受字面量 secs」，matcher 实为 `$secs:expr`（内部 `let __s: u64 = $secs;`），传变量/函数调用同样合法；文档承诺过窄既误导读者，也掩盖真实求值时机。
- **待裁 1（不改码）**：`VectorStore::default()` 与 `std::default::Default::default` 的语义歧义（`vector.rs` 附近已有测试注释自证规避 E0034）。重命名属破坏性变更（触 36 个声明消费者）⇒ 待裁，需另立 ADR 并给迁移窗口。
- **待裁 2（不建门）**：`sparse/ids.rs:31` 自陈 `id_newtype!` 与 nexus-core 同名宏「完全一致，保证序列化兼容性」——这是**文档级镜像契约**，无任何机器判据；但跨 crate 宏体比对是一条全新门轴，成本与收益需先有真实漂移事件支撑 ⇒ 登记待裁，不在本轮造轴。

## 3. 复测与口径裁决台账（三类终态都登记）

| # | 主张（来源） | 复测命令 / 判据 | 终态 |
|---|---|---|---|
| 1 | 子代理：prelude 与顶层差 26 项、PRE-only=∅、`BehaviorContract` 是**唯一**漏网 | 前三版 Python 解析器各错一次（把 53 条模块级 `pub use` 计入值级 / 单行 `{…}` 语句漏判）；第四版改在 Rust 侧（= 最终锁测试）修复前实测 `top_only=26`、`pre_only=0`（`value_prelude` 现值 191 = 修复前 187 + 本批补入 4 名，4 名逐个可点数，故 187 为同口径推算值） | **部分成立**：PRE-only=∅ 成立，「唯一漏网」否证——`formal_props` 另有 3 项同族缺项（锁测试抓到后补齐） |
| 2 | 主会话中间读数：TOP-only=29 / PRE-only=7 / 「`BehaviorContract` 两侧都不在」 | 同上；该口径按全局名集求差并把模块级语句计入 | **驳回并登记反证**（本 ADR 即为反证载体）；教训：计数必须锚在**被计数对象的语法单元**上，同一把尺两版各错一方向 |
| 3 | 「L0 存在第三方后端类型泄漏（维度 3 主题）」 | `serde_json::Value\|Map` 于 `src/**` 命中 6 处，逐条定位**全在注释/文档句**；`serde_json` 只在 `[dev-dependencies]`；JSON 文本以 `Box<str>` 承载（src 出现 120 处，按出现行计）；L0 src 生产面 `dyn` 0 命中 | **非缺陷**：维度 3 主体通过，泄漏面为零 |
| 4 | 「`InMemoryVectorStore` 是生产面夹具」 | 定义 `vector.rs:342`，所在 `#[cfg(test)]` 起 `:329` | **非缺陷**（G-115 头注已登记该口径） |
| 5 | 「`VectorBackend{Memory,Hnsw,SqliteVec}` 是后端类型泄漏」 | 纯算法侧诊断标签，`as_str()` 给稳定线串；L0 无 `hnsw_rs`/`sqlite-vec` 依赖；文档自陈「仅诊断、非运行时旗」 | **非缺陷**（与 ADR-005/034 降级口径一致） |
| 6 | 「`select_top_k` 无 bench 覆盖」 | osa-coordinator 三个 bench 已覆盖端到端链路（`compute_masks`/`parallel_vs_sequential`/`osa_parallel_bench`） | **降级为部分成立**：缺的是 L0 **单元级**基线 ⇒ 据此收窄 R3-B 目标，不新建端到端重复 bench |
| 7 | ADR-213 D-7 的证据行「`masks::select_top_k` 在 L0 外有 83 处引用、跨 L2–L6（csn-substitutor/gea-activator/osa-coordinator/sesa-router）」 | 限定调用口径：14 处 / 2 文件 / **1 个生产 crate**；bare-token：csn-substitutor 3、gea-activator 11、sesa-router 5，但三家 `SparseMask::` 限定命中**均为 0**（是各自同名私有 helper 与文档提及） | **证据量纲陈旧，裁决结论不动**：`select_top_k` 是 L0 类型的 inherent 构造函数，与类型同 crate 是 Rust 语言约束；若要移上层只能改为「上层 trait + 14 处生产调用点改写」，属破坏性 API 变更而非搬家 ⇒ D-7 继续 Accepted，本表登记为订正指针 |
| 8 | 「L0 两条 Top-K 实现重复，应收敛」 | `masks.rs:142` 未调用 `util::xts_top_k_by`（`util.rs:60/73`）；共存已由 ADR-213 D-7 裁过 | **否决**（与现行裁决相冲）；只在 bench 头写分工，防重复提出 |
| 9 | 「bench 缺口需一次性补四个函数」 | 三函数零生产调用方的逐条指针见 D-36 | **降级为登记**：只为有调用方的 `select_top_k` 补 bench，其余改为触发条件绑定 |

## 4. 影响与兼容性

- **公共 API**：纯 **additive**（prelude 增 4 名），零删除、零改签名、零 `pub fn` 增删 ⇒ G-105 基线文件本轮未动；G-107 的 53 条模块级导出不动；`CONSUMERS: 34` 派生行（`lib.rs:6`，G-59 锁）不动，散文不手抄。
- **零变更面**：NexusEvent 三锁（152 / 20 / 16）、依赖边与白名单（`[dependencies]` 实测恰 serde/chrono/uuid/thiserror 四项）、unsafe、SLO 三表（`perf_thresholds.toml` / `bench_check.yml` / `perf_redlines.toml`）、`gate_manifest.toml`（118 条）、CI workflow。
- **序列化**：零 wire 影响（导出面 / 文档 / bench 三类改动不触及字节形态）。
- **消费者冲击**：36 个声明消费者零改动、无需迁移（`cargo check --workspace --all-targets` rc=0 实证）。

## 5. 验收（全部 2026-10-15 实测）

- `cargo check --workspace --all-targets` rc=0；`cargo clippy -p nexus-contracts --all-targets --jobs 2 -- -D warnings` rc=0（首跑 2 警：`inconsistent_digit_grouping` + `doc_lazy_continuation`，已修后复绿）；`cargo fmt -p nexus-contracts -- --check` rc=0；`taplo fmt -c .taplo.toml --check` rc=0。
- `cargo test -p nexus-contracts`：**868 passed / 0 failed**（15 个测试 target，含新锁测试 4 条）+ **58 doctests / 0 failed**。
- `cargo bench -p nexus-contracts --no-run` rc=0，产出 3 个 bench 可执行体（`sparse_select_top_k` / `util_micro` / `xts_top_k`）——本轮**未跑 bench 数值**。
- 门：G-99 rc=0（64 src 文件、0 杂质、2 豁免）、G-101/105/107/113/115/91 rc=0、G-117/118 rc=0；`check_perf_redlines.py --selftest` / `--static-only` / 默认 classify 全 rc=0 且新行判为 **dev-only**（`unknown=0`，dev-only 62）；`audit_gate_parity`、`check_gates_provenance` 双向 rc=0；文档四门（含 `check_doc_symbol_claims` 12/12 登记）与 `adr_index_sync` rc=0；`git status --porcelain` = 0。

## 6. 回滚

逐提交 `git revert`（禁 `checkout --` / `restore` / `stash` / `add -A`）：`af29d1a`（R3-A：lib.rs 两族补齐 + 口径注释 + 新锁测试）→ `cdce22e`（R3-B：bench 文件 + `Cargo.toml` + freeze 行**三件同批**）→ `8bdff4b`（R3-B 补：clippy + fmt）→ `218473a`（R3-C：vector.rs + test_scale.rs）→ 本批（R3-D：ADR + adr_index + CHANGELOG + 报告）。**bench 三文件必须同批回**：只回 bench 文件会留下指向不存在 bench 的 freeze 行，`check_perf_redlines.py` 立即判 STALE 红。

## 7. 未做 / 待裁（诚实登记，含阻塞前提）

1. 档 3 剩余与 D-27 `schema_version` 铺开 —— 触 `HarnessSpec::canonical_merkle_input`（95 行 Merkle 证据链，加字段即改哈希基线）与 mca-gateway `session_turns` 落盘面 ⇒ 跨层批次。
2. lane-only 3 Critical 入 ImmutableSurface —— 需同批扩 allowlist 行 + 改写 ADR-212 §7.1「17=16+1」承诺边界。
3. S7 161 豁免分桶 —— 第三轴；禁复用 G-91/G-105 基线（G-91 只 walk `crates/*/src`，对 `tests/**` 零可见性）。
4. L0 覆盖率专项条目 —— 本地 `cargo llvm-cov` 报 E0463（工具链缺件，非选择），只能由 CI nightly 出数后填，禁估算。
5. `bench_check.yml:265` 注释订正 —— 触 `.github/workflows/`，需另批授权。
6. `VectorStore::default()` 改名、`id_newtype!` 镜像契约门 —— 见 D-38 两项待裁。
7. 新锁测试的规模闸门（`>150`/`>100`）是**下限**保护；若未来有意把 prelude 收缩到 <100 名，须同批改闸门值——这是刻意动作的成本，不是缺陷。

## 8. 执行记录

`af29d1a`（R3-A）/ `cdce22e`（R3-B）/ `8bdff4b`（R3-B 补）/ `218473a`（R3-C）/ 本批（R3-D 落档）。审计报告：`docs/reports/L0-round3-facade-and-bench_2026-10-15.md`。
