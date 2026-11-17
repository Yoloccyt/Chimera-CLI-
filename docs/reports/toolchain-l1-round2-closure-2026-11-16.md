# 工具链+L1 重构 · 第二轮批次收口报告（ADR-318 明细载体 · 中段）

> 批次：T0 门禁修复 / V 验证补强 / W5 / W6 / W4（heredoc 根除+cron+release nextest）/ N1+N2 微批
> 前置基线：第一轮 7 提交（HEAD `0a43ae78`）+ 在途 L1-B 线（19 脏文件+4 untracked，全程未触碰）
> 日期：2026-11-16 · 分支：`refactor/l0-w4-compute-extract` · 收口提交：`82fdafb6`（本报告随收尾批入册）

## 1. 提交清单（第二轮 4 提交 + 本报告）

| Commit | 批次 | 一句话 |
|---|---|---|
| `3d7d0f08` | T0 | check_perf_redlines 四红修复（I8×3 路径/I5 floor/ignored 台账），check job Perf redline 门自 `ad6d7a62` 起在本分支的红恢复绿 |
| `89abf21a` | W5 | audit 命令机器真值源化（`scripts/audit_cmd.toml`，--check-ci 入 CI）+ `.gitattributes` 补 `*.toml text eol=lf` 根治 taplo 双面门 |
| `d2c77014` | W6 | `check_coe_observability.py` COE 观察期机器化（漏翻转才红）+ agents.md §10.1 补登 perf_redline.yml + 行号引用改步骤名 |
| `82fdafb6` | W4+N1+N2 | bench_check 475 行 heredoc 根除（阈值单源化）+ cron 02:30 错开 + release test→nextest + 等价测试 fmt 收敛 + cargo 间接调用探测 |

## 2. 验证证据（全部本地实测）

- **V1 编译面**：`cargo check --workspace --all-targets` rc=0（3m22s）——第一轮 R2/R4 对 54 个消费方（event-bus 35 + nexus-core 19）零破坏。
- **V2 性能对照**（bench-lite 档，CI release-profile 复核留推送后 bench_check 门）：
  - `bus_bench`（R4.2 宏收敛面）max median = **47.05µs** < 阈值 200µs（余量 4.25×）PASS；
  - `cross_layer` max median = **16.22µs** < 阈值 100µs（余量 6.2×）PASS（criterion 报 2 mild/1 severe 离群，中位数口径不受影响）。
- **门禁复跑**：`check_perf_redlines.py --static-only` rc=0（selftest 79 断言，含 W4a 新增 verdict-json 四例）；`audit_cmd_sync.py --selftest` 12 断言 + `--check-ci` 真树逐字等价；`check_coe_observability.py --selftest` 13 断言；manifest schema 绿（159 门）；actionlint 双 yml OK；探针 3 测 + semaphore 11 测 + govd 90 测零回归；`git check-attr eol` 对六处 toml 全部 lf。
- **V3 快速测试面**：`cargo nextest run --workspace --profile ci-fast --exclude chimera-e2e-tests -E 'not binary(/stress/)' --build-jobs 2 --no-fail-fast` ——**11937 run：11935 in-ceiling 通过 + 2 above-ceiling 通过（239.4s/244.8s，慢非挂）+ 72 skipped；功能回归为零**。
  - 事件链（按 ADR-225 D1 资源/回归分离判定）：首轮 2 条 `composition_root_e2e` prop 测试 180s 超时 → 带 CI env 块重试仍超时 → **无 slow-timeout 鉴别实验两条均 PASS**（239.4s/244.8s），定性=本机环境敏感慢测试（proptest 256 case × 全量组合根装配；该测试与组合根构建路径均未被本轮提交触碰）；期间 D 盘 100% 引发一次编译失败（os error 112），清理本会话产物 `target/bench-lite`/`target/criterion` 与可再生的 `target/debug/incremental`（~4GB）恢复，与代码回归分开判。

## 3. 度量口径与【待验证】项

- bench 数值为 bench-lite 档（opt-level=1）本机实测；与 CI release-profile 的绝对差值未测量【待验证：推送后 bench_check 门的 Run benchmarks 步输出对照】。
- heredoc 根除的量化出口（compare 连续 5 轮逐键一致）需 nightly 运行积累【待验证：5 个 schedule 周期后读取 tmp/perf_verdict.json artifact 序列】。
- COE 门真实运行依赖 gh 凭据（本机未登录→rc=2 实测映射不可判定语义）【待验证：CI/带凭据环境 --dry-run 首跑】。

## 4. 延期与等待项（前置清晰）

- **W2（govd 翻转 CI 阻塞）**：ADR-228 §3.6 门槛「W-T1 连续 7 日绿」需 gh 凭据核验；本机不可判定，保持等待不绕。
- **W3a（needs 假设施删除）**：施工规格书已备（`tmp/w3a_needs_removal_spec.md`，逐文件逐符号编辑清单+验证步骤+回滚）；前置=在途 govd 测试线（脏 `test_govd.py`/`run_gate_manifest.py`）落地。弃活 patch 取规格书：RK-15（在途线正脏同面，重放冲突概率高）。
- **R3/R5/R6（L1 死面退役/通道权威统一/telemetry bench）**：设计已细化至可施工（第二轮方案 §5.1-5.3，含 L1-B-4 freeze 协调协议与差集-4 常量登记）；前置=在途 L1-B 线落地。
- **R8 全量收口**（`--release` 压测档 + doctests 全量 + 逐包 fmt）：随 v2.30 发布窗口。

## 5. 与在途线的协调记录

- 全程未触碰 19 个脏文件 + 4 个 untracked（逐批提交前 `git status` 过滤复核）。
- T0 修复面（check_perf_redlines.py/ignored 台账）在途线未动手（提交前 diff 复核）。
- 在途线新增 G-161/G-162（census 157→159）未入册期间，本批所有门禁验证以「159 门 = 152+7 现值」为口径；其落地后 census 自锁由其批次同步。

## 6. 教训登记（本批新增）

1. **拆分提交改变门禁事实源时，selftest 夹具可能锚定被删对象**（W4a 的 11j/11k 锚定真 yml heredoc 行）——heredoc 删除必须与 selftest 基座改造同批，代理已处置并记录。
2. **「头注计数刻意不登记」的文件（bench_check.yml）与「头注登记已漂」的文件外观相似**，订正前必须先读其 WHY——本批 W6c③ 经复核为前者，未误改。
3. **gh 无凭据环境下 COE 类门首跑必 rc=2**——登记 gate_manifest 前必须带凭据预跑，已在 W6 提交注记。
4. **本地复跑 CI 测试面必须完整携带 ci.yml 的 env 块**（`CHIMERA_TEST_TIMEOUT_SCALE=0.1` 等）且慢超时判红后先做「无 slow-timeout 鉴别实验」再定回归/环境；磁盘满引发的编译失败（os error 112）一律先按资源性故障处置（清理可再生产物），与回归分开判（ADR-225 D1 实例）。
