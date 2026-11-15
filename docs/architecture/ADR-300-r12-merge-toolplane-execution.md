# ADR-300: R12 合流执行——`feat/toolplane-execution` 并入主线

- 状态: Accepted
- 日期: 2026-11-14
- 批次: R12 M 波（本 Spec 的合流波）
- 关联: ADR-229~236（工具执行面/LLM 工具循环/MCP 语义层，来源分支）、ADR-233 追加节一（就绪派发）、ADR-295/296（主线 R11/R12 基线）

## 背景

`feat/toolplane-execution @ 38638396` 与主线 `feat/toolchain-parallel-optimization @ 0fe2a061` 在 33 万行量级上双向大改。`git merge-tree` 预演得 35 文件 / 79 冲突块，其中 T3 高冲突 7 文件（`toolplan_runner.rs` 两侧 483/521 行、`check_perf_redlines.py` 453/150 行）。合流裁决为"先合流"（用户 2026-11-14 明确），本 ADR 记录执行结果。

## 决策

### D1 — 在隔离 worktree 内分批合流，不重写对方实现

- T1 机械级 9 文件：基线/白名单类取并集，`pipeline.rs::new_internal` 取更紧的 465（feat 重测值），`popup.rs::render` 因 feat 已拆分而删行。
- T2 中等 18 文件：保留两侧新增 job/key/gate；`perf_thresholds.toml` 吸收 feat 的 `tool_plan` 与 `llm_tool_loop_bench`；govd 断言改为从 manifest 现取计数，不硬编码。
- T3 高冲突 7 文件：`toolplan_runner.rs` 保留主线混层写屏障/审计序语义，同时接入 feat 的 ready-dispatch 与 `plan_runner_eager.rs` 10 例；`mcp-mesh/src/mesh.rs`、`quest-engine/engine.rs` 以主线为主 + feat 增量。
- 治理脚本对拍而非手合：`check_perf_redlines.py` 两侧判定内核全保留，I10 取 feat 三表键集、I11 保留 ADR-278 可执行性、feat 值对拍顺延为 I12；G-143/G-144 重号按"先到先得 + 后到顺延"处理，feat 双门登记为 G-148/G-149。

### D2 — 合流语义必需的外溢修正

- `nexus-core/src/config.rs` 补回 `McpServerConfig.command/args`：feat 的 `StdioTransport` 是真实消费者，否决了 ADR-255"零消费"前提；否则 `tool_plane` 全链路不可编译。
- CLI 调用面补 `endpoint: None` / `..Default::default()`（5 个既有测试文件），仅为编译兼容，不改变断言。

### D3 — 合流提交与主线接入

- 隔离 worktree 提交 `ae0bb844`（merge commit），随后将主线 `0fe2a061` 并入得 `3d464597`，主线 `feat/toolchain-parallel-optimization` 以 `--ff-only` 前进至 `3d464597`。
- 合流规模：**160 文件 / +22,026 / −850**（三点 diff，含来源分支新增文件）。
- 未跟踪目录 `clear/` 未触碰。

### D4 — 合流收口修正（2026-11-15，M5 全量回归暴露）

全量回归暴露 7 个失败 + 2 个文件编码污染，均在**合流裁决面**（非新功能），逐项修正：

| # | 症状 | 根因 | 修正 |
|---|---|---|---|
| 1 | `delegation_peak_inflight_is_bounded`：peak 8 > 4 | `with_max_concurrency` 字段无人读：批处理用了进程级 `delegation_max_concurrent()` | 构造器以进程级值初始化字段，批处理读 `self.max_concurrency`（显式收窄优先；env 管默认） |
| 2 | `cancel_root_singleton_and_propagation`：run.rs 新建令牌 | T3 合流把 `cancel_root()` 换成 `CancellationToken::new()`（ADR-270 语义丢失） | 恢复 `crate::call_budget::cancel_root()` |
| 3 | `help_text_has_no_stale_references`：期望 9，实际 6 | 合流取了 feat 的陈旧断言（9），而 `BUILTIN_SLASH_COMMANDS` 两父提交均为 6 | 断言恢复 6（实现为唯一事实源） |
| 4 | `attribution_behavior_is_not_debug_only`：找不到函数体结尾 | 源码扫描 needle 写死 `"\n}\n"`，Windows CRLF 检出假红 | needle 改为行尾无关的 `"\n}"`（锚点语义不变） |
| 5 | `mcp_discovery_assembly` / `mcp_approval_surface` / `mcp_stdio::configured_env_reaches_the_child` | PATH 上的 `bash` 是 WSL 桩：不把任意 Windows env 带进 Linux 侧，且冷启动超 5s 请求超时 | 测试夹具解析真实 POSIX shell（`CHIMERA_TEST_SHELL` → Git Bash → `bash`），闸门按文件名放行；生产语义不变 |
| 6 | `quest-engine` doctest E0765 + 两文件文档注释乱码 | T1 机械合流对 `quest-engine/src/lib.rs`、`chimera-cli/src/lib.rs` 做了 GBK 转码（有损） | 以主线版本重建文档面，保留合流新增声明（`config::{DagMode, QuestConfig}` 与 `db_open`/`llm`/`quest_exec`/`tool_plane`）；全库 mojibake 扫描 `suspects=0` |
| 7 | doctest E0460 "possibly newer version of crate" | 并行 cargo 会话（全量回归 + 定点修复）在 `target/` 留下多份同 crate 不同 hash 的 rlib | `cargo clean -p chimera-mas -p repo-wiki` 后串行复跑 doctest；禁止在回归运行期并发 `cargo` |

判据纪律（写入本 ADR）：合流后的全量回归必须**串行**执行；任何并行 cargo 会话造成的 E0460 不得当作代码缺陷，也不得用"重跑一次绿了"掩盖——必须记录为取数纪律。



| # | 判据 | 命令 | 结果 |
|---|---|---|---|
| 1 | 无未合并文件、无冲突标记 | `git diff --name-only --diff-filter=U`；`git grep -n "^<<<<<<<"` | 空；零命中 |
| 2 | 核心 crate 类型检查 | `cargo check -p gqep-executor -p mca-gateway -p quest-engine -p chimera-mas -p chimera-cli --jobs 2` | EXIT=0（3m36s） |
| 3 | 就绪派发测试 | `cargo test -p gqep-executor --test plan_runner_eager --jobs 2` | 10 passed / 0 failed |
| 4 | bench 四态完整 | `py -3 scripts/check_perf_redlines.py inventory` | `total=134 gated=11 registered=56 dev-only=67 unknown=0` PASS |
| 5 | `l2_lock_contention` 登记 | `scripts/bench_inventory_freeze.txt` 新增 dev-only 行 | 已登记，重开条件=读锁竞争进入生产 p95 观测面 |
| 6 | 全量回归 | `cargo test --workspace --no-fail-fast --jobs 2` | 见 `docs/reports/toolchain-r12-merge-closure-2026-11-14.md`（后台运行，结果回写） |

## 回滚

- 合流是单一 merge commit：`git revert -m 1 3d464597` 回到 `0fe2a061`；隔离 worktree 可直接 `git merge --abort`（若仍在合并态）或删除 worktree。
- 外溢修正（`nexus-core/config.rs` 与 5 个测试调用面）包含在 merge commit 内，随整包回滚。
- 合流期间未删除任何既有测试、未删除任何 L0 公开字段/事件变体。

## 教训

- 冲突数不等于工作量：真正的决策点是 79 个冲突块，尤其 `toolplan_runner.rs` 的调度主体与治理脚本的判定内核。
- 基线文件（`fnlen_baseline.txt` / `contract_*` / `bench_inventory_freeze.txt`）必须在合流树上**重生成或重测**，直接并集会把 STALE 行带进新树。
- 合流后 `check_perf_redlines.py inventory` 是第一个能区分"合进来了"与"合得能用"的机器判据。
