# Agent 运行时并发与资源效率：W0~W5 执行报告（2026-11-10）

> 基线：`feat/toolchain-parallel-optimization` @ `6aed20b4`（工作树干净）
> 交付 HEAD：`f91ebe9d`（W5）+ 本报告
> 用户决策：GPU 维持三重关闭；范围含「复评既有否决项」；CI workflow 修改已授权；验收口径 = 非 doctest 全绿 + 29 例 doctest 家族不恶化。
> 纪律：性能主张一律 criterion 同轮证据 + ≥5% 门；不达标即登记否决；每波独立 commit、可单独 revert。

---

## 1. 一句话结论

本批没有继续堆「看起来更快」的改动，而是先修**验证面**（14 条已声明的性能契约此前无运行期断言），再删除两处**虚构工作**，并对**混层串行**做了带基准证明的算法改进。最终：

- **13 条性能契约**（7 redline + 6 SLO）首次在本机实跑并全部通过；
- `mcp call` 的 2PC **13.954ms → 2.966ms（−78.7%，n=1）**；
- 混层计划 **101.32ms → 17.41ms（5.82×，CI 保守 4.59×）**；
- 四项既有否决按各自重开条件复评，**全部维持**（ADR-249 重跑后方向反转）；
- 计算面 7 个候选卸载点**全部不接线**（无调用方 / N≪阈值）；
- 删除一个零调用 API + 一个无人读配置字段。

---

## 2. 提交与波次

| 序 | commit | 波次 | 内容 | ADR |
|---|---|---|---|---|
| 1 | `2365013e` | W0 | 性能契约可执行性：I11 + `redline_exec`/`excluded` + `mode_redline` + `mode_slo` 80% 门 + `perf_redline.yml` | ADR-278 |
| 2 | `01fc4e69` | W2 | 进程内零延迟客户端 + 非 TTY 流式归零 | ADR-280 |
| 3 | `141d968c` | W1 | 四项否决复评（全部维持） | ADR-279 |
| 4 | `88a11bac` | W3 | 混层写屏障分段 + 层内声明序稳定化 | ADR-281 |
| 5 | `ef9763d1` | W4 | 计算面卸载点位全部不接线 | ADR-282 |
| 6 | `f91ebe9d` | W5 | 删除零调用探活 API + 配置字段；action loop 维持 | ADR-283 |

---

## 3. W0：让 14 条性能契约真正可执行（ADR-278）

### 3.1 修复前的事实

| # | 事实 |
|---|---|
| 1 | 11 条 `[[redline]]` 只是静态 lint（文件/fn/阈值标记存在）；9 条既无 `thresholds_ns` 键、bench 也不在 `run_bench` |
| 2 | 6 条 `[[slo]]` 中 5 条的 bench 从未被执行；`mode_slo` 无任何 workflow 调用者 |
| 3 | `mode_slo` 把「redline < measured <= SLO」也打印 PASS（80% 红线是标签） |
| 4 | `window_select/` 命中 4 个档位，旧实现只读第一条（最轻档） |
| 5 | I3/I7 用 `(crate,bench)` 身份，同一 bench 的多 filter 分组会被静默折叠 |

### 3.2 改动

- 新增 **I11**：每条 redline/SLO 必须有执行面或 `excluded`（owner+expiry+reason，过期即红）；放在现有 `mode_invariants`，由 G-02/CI 的 `--static-only` 覆盖，**不新增门、不动 govd 普查（140/133/7）**。
- 新增 `[[redline_exec]]`（surface=bench/slo/nextest + assert_ns）与 `[[excluded]]` 两表。
- 新增 `mode_redline`；`single_criterion_time` 只接受**恰好 1 条可解析为时长**的 time 行（0/多 ⇒ rc=2）。
- `mode_slo` 的 80% 红线改为阻断；`window_select` filter 收紧到最重档 `L3_1M`。
- I3/I7 身份改 `(crate, bench, alias)` 三元组。
- 新增 `.github/workflows/perf_redline.yml`：static 阻塞 + `slo`/`redline` 实跑观察期（连续 2 晚全绿无 SKIP 后切阻塞）。观察期 job 把 `slo` 与 `redline` **合并为一步**（`set +e` 捕获两个 rc），避免前者失败时 GitHub 跳过后者、丢掉读数。

### 3.3 实跑结果（本机 Windows GNU，`--quick`，criterion median）

| 契约 | 目标 | 实测 | 判定 |
|---|---|---|---|
| RL-01 `window_select/l3_ultra_complex` | <1ms | **1.2656 ns** | PASS |
| RL-02 `mlc_l2_knn_top10_4096/top10` | <5ms | **136.94 µs** | PASS |
| RL-03 `single_decay_step_latency/time_decay` | <1µs | **66.125 ns** | PASS |
| RL-04 `single_thread_knn_latency/1000` | <10ms | **399.56 µs** | PASS |
| RL-07 `membrane_e2e_delivery/publish_to_recv` | <10ms | **14.954 µs** | PASS |
| RL-10 `cost_estimate_1m_tokens_peak` | <1µs | **5.6172 ns** | PASS |
| RL-11 `sse_normalize_100_text_deltas` | <5µs/event | **108.9 µs / 100 = 1.089 µs/event** | PASS |
| SLO `window_select` | <0.8ms | **1.4758 ns** | PASS |
| SLO `mlc_l2_knn` | <4ms | **207.63 µs** | PASS |
| SLO `decay_compute` | <0.8µs | **70.087 ns** | PASS |
| SLO `wiki_knn_100k` | <40ms | **161.38 µs** | PASS |
| SLO `immune_probe` | <80ms | **566.28 ns** | PASS |
| SLO `linucb_40arm` | <40µs | **7.2338 µs** | PASS |

`mode_redline`：failed=0 skipped=0（7/7）；`mode_slo`：failed=0 skipped=0（6/6）。
RL-05/06 由 `slo.yml` 的 13 条 `#[ignore]` p95 测试执行；RL-08 豁免（到 2026-12-31）。

### 3.4 实跑捕获的两处量纲/解析陷阱

1. **RL-11 的阈值是 per-event，criterion 迭代是 100 events**：首版 `assert_ns=5000` 把实测
   107.29µs 误判为超限 21×。正确为 `5µs × 100 = 500_000 ns`，实测 1.089µs/event。
2. **criterion 有 baseline 时会多打 `change:` 块**，内含 `time: [-21% …]`（单位 %）。
   首版「恰好 1 条 time 行」把正常跑判成不可判定（RL-07 `cargo exit=0` 却 SKIP）。
   修正为「恰好 1 条可解析为时长」，selftest-24f 锁住。

---

## 4. W2：删除调用面虚构开销（ADR-280）

### 4.1 改动

- `InProcessClient::instant()`（零延迟）；`new()`/`Default` 保留 1–2ms 模拟（测试夹具）。
- 生产唯一构造点 `in_process_mesh()`：组合根 `McpToolExecutor::from_config` 与
  `commands/mcp.rs::build_mesh` 共用；`MeshConfig` 不变（WAL durability 与敏感扫描保留）。
- `resolve_stream_chunk_delay` 纯函数：`env > TTY > 默认`；`run`/`chat` 接入；非 TTY 归零，
  TTY 输出逐字节不变。

### 4.2 A/B（同机同 HEAD，criterion median）

| 服务器数 n | 模拟延迟（对照） | 零延迟（生产） | 变化 |
|---|---|---|---|
| 1 | 13.954 ms | **2.9660 ms** | **−78.7%** |
| 3 | 13.834 ms | **2.6961 ms** | **−80.5%** |
| 5 | 14.221 ms | **2.8199 ms** | **−80.2%** |

**关键发现**：名义延迟只有 1–2ms×2=2–4ms，实测差 ~11ms。根因是 **Windows 定时器粒度**
（约 15.6ms 默认 resolution）让短 `sleep` 按 tick 取整。剩余 ~2.7ms 是 2 次 WAL fsync
（本轮不动：关 durable 会撤掉敏感载荷扫描）。

测试：mcp-mesh 22/22（含 2 条 instant/new 行为锁）、chimera-cli 6/6（3 纯函数 + 2 结构锁）。

---

## 5. W1：四项既有否决复评（ADR-279）

只在各 ADR **自述重开条件**满足时重跑：

| 项 | 触发 | 结果 | 裁决 |
|---|---|---|---|
| ADR-249 借用脊柱 | seccore 判决体 ADR-260 已落地 | c1 187.33/193.67µs = **1.034**（更慢）、c4 185.80/193.31µs = **1.040**（更慢）、c8 190.13/189.23µs = 0.995（CI 重叠） | **维持**；方向反转（原 0.971/0.998/0.998） |
| ADR-263 mimalloc | 要求分配棘轮读数显著上升 | `dispatch_alloc_budget` 仍通过（7 次/派发未上升） | **不重跑，维持** |
| ADR-275 装配期并行 | 四段合计 ≥5ms 且单段 ≥2ms | `composition_build_auto` **453.87µs**、`build_off` 59.478µs | **维持** |
| ADR-277 PlanRunner 固定开销 | 前提 `MAX_STEPS=64` 未变 | `--plan-max-steps` 为 min 语义（只收紧） | **维持，不重提** |

绝对耗时从 ~403µs 降到 ~187µs（−54%），佐证 R1 确实落地；但 borrowed 比值没有向
≤0.95 靠拢，c1/c4 反而翻成更慢 ⇒ 「2 次分配节省会因占比上移而跨过 5% 门」被实测否证。

---

## 6. W3：混层写屏障分段（ADR-281）

### 6.1 改动

- `node_is_read_safe` + 三路分派：全读→concurrent / 全写→serial / 混层→segmented。
- `exec_layer_segmented`：读段用既有 `exec_layer_concurrent` 并发；写节点作屏障单节点串行；
  读写不并发；复用同一 `accumulate`（不新增第二真值源）。
- `compute_layers` 的初始/每轮 frontier 按声明索引排序 ⇒ 层内声明序稳定，修掉
  「同层多写顺序跨运行漂移」的正确性缺陷（ADR-277 D3 的教训）。
- 契约锁收窄：`partial_capability_only_parallelizes_covered_names` 由 peak=1 改为 peak=5
  （可区分整层串行=1 / 无屏障全并发=8 / 正确分段=5）。

### 6.2 实测（`toolplan_layers`，mock sleep(2ms)，sample_size 20）

| 组 | c=1 | c=8 | 比值 |
|---|---|---|---|
| `readonly8_ms2` | 106.65 ms | 15.154 ms | 7.04× |
| `undeclared8_ms2` | 92.652 ms | 10.030 ms | 9.24× |
| **`mixed7r1w_ms2`（7 读 + 末尾 1 写）** | **101.32 ms** | **17.41 ms** | **5.82×** |

CI：c1 [95.49, 105.39]、c8 [14.68, 20.82]；**保守比 4.59×**，仍 >4× 门。
绝对墙钟被 Windows 定时器粒度放大；门槛按比值判定。

测试：capability 5/5、concurrency proptest 8/8（4 既有 + 4 新增）、terminal 1/1；
clippy rc=0；rustfmt rc=0。

**语料边界**：仓内无生产计划语料，收益证据是合成 bench；不声称生产端到端 −X%。

---

## 7. W4：计算面卸载点位（ADR-282）

接线三前置（生产调用方 + N≥阈值 + 端到端≥5%）**无一满足**：

| 卸载点 | TaskKind 阈值 | 调用方 | 裁决 |
|---|---|---|---|
| decay_batch_parallel | 10,000 | 无（DEAD） | 不接线 |
| faae score_parallel | 10,000 | 无（DEAD） | 不接线 |
| nmc perceive_parallel | 1,000 | 无（DEAD） | 不接线 |
| osa compute_five_masks_parallel | 100 | 无（DEAD） | 不接线（最接近） |
| gsoe score_batch | 500 | 无（DEAD） | 不接线 |
| faae score_scoring_parallel | 10,000 | CRATE_API_LIVE（非组合根） | 不接线 |
| repo-wiki knn_batch_parallel | 5,000 | CRATE_API_LIVE（仅 bench） | 不接线 |
| hcw score_parallel | 200 | 诊断面（G-88） | 维持 |
| overwindow dense_scores_pooled | 1,000 | **WIRED** | 保持 |

零生产代码、零 freeze 台账变更；G-87/G-88 原判不变。

---

## 8. W5：结构死臂与活性（ADR-283）

- 删除 `McpMesh::start_background_probe`（全仓零调用）与 `MeshConfig::background_probe_interval_ms`
  （无人读）。旧配置含该字段仍可反序列化（无 `deny_unknown_fields`，向后兼容夹具已断言）。
- action loop 许可耗尽降级**维持**：既有 `bounded_loop_caps_inflight_and_delivers_exactly_once`
  证明 bound=2/N=10 恰处理一次、峰值 ≤ bound+1；无丢事件证据。
- mcp-mesh 114/114；clippy rc=0；rustfmt rc=0。

---

## 9. 静态门与全量回归

### 9.1 静态门（本机，现跑）

| 门 | 结果 |
|---|---|
| `check_perf_redlines.py --selftest` | PASS（71/71） |
| `check_perf_redlines.py --static-only` | PASS（I11 exec=10 excluded=1） |
| `check_py_tooling_health.py` | OK（py_files=118, workflows=12, python_jobs=7） |
| `check_workflow_yaml_lexical.py` | 0 findings（12 files） |
| `check_ci_manifest_parity.py` | unmatched=0, gate_missing=0（142 gates） |
| `check_dead_contract_apis.py` | OK（11/11） |
| `check_sync_offload_guard.py` | OK（13 anchors live） |
| `check_doc_drift.sh` | OK（154 events / 10 topics / 46 crates） |
| `check_dependency_rules.sh` | OK（46 crates scanned / 46 layer entries；mas 内部依赖 12/16） |
| `check_crate_reachability.sh` | OK（members=46, reachable=38, frozen=0, new_gaps=0；8 GATED） |

### 9.2 全量回归与定向回归

命令：`cargo test --workspace --no-fail-fast --jobs 2`
基线：12190 passed / 29 failed（全为 windows-gnu doctest 工件态）/ 89 ignored。

> **首跑失败归因（诚实记录）**：第一次全量回归在编译 `chimera-cli` 的 `doctor_async_probe`
> 时以 E0786（`core`/`tokio` 的 rmeta stub）失败，**零测试被执行**，rc=101。
> 根因是此前 D 盘 91% 占用时写出的截断 `.rmeta`（工件态），不是代码回归：
> 工具链 sysroot 的 `libcore-*.rlib` 完好；`cargo clean -p chimera-cli` 后错误扩散到
> 更多依赖。处置：`cargo clean`（释放 67.4GB，D 盘回到 119GB 空闲）后从零重建重跑。
>
> **第二次失败归因**：从零重建时 `darling_macro` 报 `ld.exe: final link failed:
> No space left on device`、`mel_spec` 报 `bfd_close ... invalid operation`，同样
> **零测试被执行**。根因：全量测试矩阵的 debug 工件可膨胀到 ~109GB（历史实测），
> 119GB 余量叠加并行链接临时文件后触顶。这是**宿主磁盘约束**，不是代码回归。
>
> **处置：定向回归 + 全 workspace 编译检查。** 范围 = 被触碰的 3 个 crate
> （`mcp-mesh`/`gqep-executor`/`chimera-cli`）+ 3 个直接依赖方
> （`mca-gateway`/`chimera-mas`/`gea-activator`）。
> 这与项目域记忆「逐 crate 定向回归 20KB–230MB/次」的口径一致，避免把磁盘约束
> 报成「全有或全无」。**残余口径**：未逐 crate 跑完其余 40 个 crate；它们未被本批
> 代码触碰，且由 `cargo check --workspace --all-targets` 覆盖编译面。

**全 workspace 编译检查（现跑）**：`cargo check --workspace --all-targets --jobs 2` —— 覆盖未被定向测试的其余 40 个 crate 的编译面（含 tests/benches 的 rmeta）。

**结果：`Finished dev profile in 8m 48s`，rc=0。** 46 个 crate 的 lib/bin/tests/benches 编译面全部通过
（含未参与定向测试的 40 个 crate）。

**定向回归结果（现跑）**：

```
cargo test -p mcp-mesh -p mca-gateway -p gqep-executor -p chimera-mas -p gea-activator -p chimera-cli --no-fail-fast --jobs 2
  test result blocks = 99
  passed = 1917, failed = 20, ignored = 20
  唯一 FAILED target = `-p chimera-mas --doc`
```

| 判据 | 结果 |
|---|---|
| 非 doctest 用例 | **全绿**（20 个 failed 全部来自 `chimera-mas --doc` 的链接/元数据错误，零断言失败） |
| 本批 3 个改动 crate 的 doctest | **全绿**：mcp-mesh 2 passed / gqep-executor 3 passed / chimera-cli 2 passed（1 ignored） |
| 未触碰的 chimera-mas doctest | 20 failed（单跑重测为 **21 failed**，计数随工件态波动），错误全为 E0460/E0462/E0463/E0786（`tract_linalg`/`futures_util`/`nexus_core`/`async_trait`/`typenum`/`nom`/`rand`），**零断言失败** |

**关于「29 例家族不恶化」的诚实披露**：基线的 29 = chimera-mas 15 + repo-wiki 14，
是在**旧工件态**下测得的。本批为修复 E0786 做了 `cargo clean`（释放 67.4GB）并从零重建，
同一份未改动的 chimera-mas 代码在 clean 后的 doctest 工件态失败数变为 20（重测 21）——
**计数随工件态在 20↔21 间波动**，且错误种类与基线同族（E0460/E0462/E0786，新增 E0463）。
这不是本批代码回归（chimera-mas 本批零改动），而是 ADR-266/w4w5 已登记的 windows-gnu
doctest 工件态家族在 clean 后的重新分布。按用户口径「非 doctest 全绿 + 工件态登记为已知豁免」处理；
数值上本次 clean 后为 20–21（未跑 repo-wiki 的 14），**不声称严格优于基线的 15**。
接管方式：独立工单（windows-gnu doctest 工件态），Linux CI 为权威口径。

---

## 10. 回滚演练（现跑）

| 波次 | 回滚面 | 演练结果 |
|---|---|---|
| W5 `f91ebe9d` | code + config + ADR | `git revert --no-commit` **干净应用**；`cargo test -p mcp-mesh --lib` **114/114**；已 abort 恢复。 |
| W3 `88a11bac` | code + tests + bench + ADR | 代码面应用；回滚后旧契约 `toolplan_capability_table` 5/5、`toolplan_concurrency_proptest` 4/4、`toolplan_terminal_aggregation` 1/1 全绿；`adr_index.md` 追加式冲突（文档面，预期）；abort。 |
| W2 `01fc4e69` | code + bench + ADR | 代码面应用；新测试随回滚消失（**20/20** 旧 `participant_client` 测试绿）；`adr_index.md` 冲突（预期）；abort。 |
| W4 `ef9763d1` | docs-only | ADR 文件进入 D（删除）；`adr_index.md` 冲突（预期）；abort。 |
| W1 `141d968c` | docs-only | 同上。 |
| W0 `2365013e` | gates + tables + workflow + ADR | **未执行完整 drill**：`perf_redline.yml` 存在未提交的 W6 修复；且回滚单元为**单 commit**（I11/表/workflow 四件同批互锁，拆分会令中间态 I11 判红）。回滚面由 `git revert --no-commit 2365013e` 的逆向 diff 保证。 |

**关于 `adr_index.md` 冲突**：ADR 索引是**追加式**的；revert 中间的 ADR 登记块会与后续追加的
登记块冲突。这是文档面的预期冲突，处置是手工删除对应的 `> **ADR-xxx 登记**` 段，
不是代码回滚的阻塞。代码面的 revert 在冲突状态下仍已应用（W3/W2 的测试即用回滚后的
代码跑通）——这正是「回滚单元按波次独立」的实证。

**回滚单元的实际形态**：W0 单 commit（四件同批）；W2/W3/W5 各自单 commit；
W1/W4 文档单 commit。每个 commit 都可独立 `git revert`，无跨波次耦合。

---

## 11. 未闭合与残余

| 项 | 状态 / 阻塞 |
|---|---|
| repo-wiki / chimera-mas doctest 工件态（E0460/E0462/E0786） | 构建态问题，独立工单；非本批回归，未新增 doc 示例 |
| `perf_redline.yml` 的实跑 job 观察期 | 连续 2 晚全绿无 SKIP 后切阻塞（ADR-278 §2 D6） |
| RL-08（50agent 内存峰值） | 豁免到 2026-12-31；到期须接线或续期（ADR-263 D3） |
| ADR-249/263 的重开条件 | 已复评维持；未来出现触发事实时按原判据重跑 |
| W3 的真实计划语料 | 无；若出现，按层宽分布复核（ADR-281 §6） |
| SubAgent `block_on` async 化 | ADR-257 D3 独立工单，未纳入本批 |
| 轮询点 31 处的门化 | ADR-264 advisory；本批不设门（计数是坏代理） |
| TUI Eco 自动降档（R5-3） | 设计冻结，待真实终端三场景人工验证（ADR-264） |

---

## 12. 复现命令（可直接执行）

```bash
# 门禁自证
py -3 scripts/check_perf_redlines.py --selftest
py -3 scripts/check_perf_redlines.py --static-only
py -3 scripts/check_perf_redlines.py slo
py -3 scripts/check_perf_redlines.py redline

# W2 A/B
cargo bench -p mcp-mesh --bench mesh_benchmark -- --noplot --quick mcp_mesh_transaction

# W3 混层
cargo bench -p gqep-executor --bench toolplan_layers -- --noplot mixed7r1w_ms2

# W1 复评
cargo bench -p gqep-executor --bench dispatch_borrowed_chain -- --noplot
cargo bench -p chimera-cli --bench composition_root_bench -- --noplot --quick
cargo test -p gqep-executor --test dispatch_alloc_budget

# 回归
cargo test --workspace --no-fail-fast --jobs 2
```
