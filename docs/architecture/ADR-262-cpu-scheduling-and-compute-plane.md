# ADR-262: CPU 调度与计算面治理 — 双池旋钮 + 双池例外定界 + 轮询取证（R3）

- **状态**：Accepted（2026-11-08，`feat/toolchain-parallel-optimization` R3 波次）
- **上游**：已批准方案「工具链系统性深度优化重构方案」R3 节；终稿 §4.6（5.3，OQ-09）
- **关联**：ADR-251（5.3 旋钮移交本 ADR）/ ADR-195 + C-15（GPU 三重关闭）/ ADR-251 §4（先实测后登记）

## 1. D1 · `CHIMERA_RT_WORKERS`（R3-1 采纳，OQ-09 关闭）

`chimera-cli/src/main.rs::build_runtime` 的 multi_thread 分支增 worker 数显式
旋钮（`resolve_rt_workers` 表驱动纯函数）：未设/`""`/`"auto"` = tokio 默认
（available_parallelism，**零行为变更**）；合法正整数 N = 显式 N；`0`/负数/
非数字 = 回退默认 + warn。WHY 旋钮而非固定值：CLI 常驻与构建/其他负载共存
时，worker 数是 CPU 争用与能耗的第一调节面；one-shot 启动税场景由既有
`CHIMERA_RT_FLAVOR=auto`（NoContext → current_thread）承担，两旋钮正交。
测试：`resolve_rt_workers_matrix`（bin target，9 案例）。

## 2. D2 · `CHIMERA_COMPUTE_THREADS`（R3-2b 采纳）

`nexus-core/src/compute/bridge.rs::compute_threads` 增同族覆盖旋钮：合法
正整数（≥`MIN_THREADS`=2）直接采用，未设/非法回退既有口径
（num_cpus−2，下限 2；零行为变更）。WHY：计算池（rayon 专用池）与 tokio
worker 的核数划分是共宿主场景（桌面开发机/容器）的第一能耗与争用调节面，
两旋钮对称构成完整的核数分配接口。测试：`compute_threads_env_override_matrix`
（6 案例）。

## 3. D3 · 双池并存 spike 结论 = 选项②「长期有界例外」维持（零代码）

**spike 取证**（2026-11-08）：破口 = `bridge.rs:27-33` 已登记的已知例外——
`hnsw_rs` 的 search/insert 内部直接派发 **rayon 全局池**，本桥的
`pool.install`/`scope` 无法捕获（全局 spawn 不继承外层 scope），迁桥
「因 install 无法排除而不可行，或需上游改造」。调用面 = `mlc-engine`
（memory_graph >1K 路径、l2_semantic）+ `repo-wiki`（HnswStore）。

**三选一裁决**：
- ❌ `build_global` 统一：全局池替换影响**所有** rayon 用户（含测试并行），
  且无收益证据（下）——零盲改纪律否决；
- ❌ 迁出 hnsw_rs：需替代 KNN 库评估 + 数据迁移，无痛点驱动（下）；
- ✅ **选项② 维持**：无 SLO 红线证据（perf_redlines 全绿）、`hnsw_tiered`
  2ms 阈值绿（I10 修复后真实断言中）、线程上界有界
  （全局池 cores + 专用池 cores−2 + tokio cores ≈ 3×cores 峰值，仅大批量
  KNN 与并行计算同时活跃时短暂出现）。**义务保持**：bridge.rs:27-33 的
  「接线前须显式决定，未决定前不得默认它已合规」继续有效；重评估触发 =
  `hnsw_tiered`/`memory_graph_recall_*` 键越阈值且池旋钮无法救回。

## 4. D4 · 轮询治理试点 = 取证后零改造（R3-3，证伪本点的前提出场假设）

方案预设「149 处轮询点」是空闲唤醒/能耗大头。**逐点取证（2026-11-08）证伪**：

| 候选（勘察报告 top） | 现场核实 | 判定 |
| --- | --- | --- |
| exec_tool_executor.rs:330（20ms poll） | 测试 `DeclaringExecutor` 的 sleep 助手（mod tests） | 非生产 |
| quest_loop.rs:352/:373（100/150ms） | 测试事件等待循环 | 非生产 |
| chimera-mas budget.rs:419 / orchestrator.rs:734（5ms） | 测试 deadline-poll 断言循环 | 非生产 |
| exec_plan.rs:433/:585（200/100ms） | 测试 SlowExecutor（mod tests :417） | 非生产 |
| chat.rs:890（50ms） | 测试订阅者同步等待（mod tests :721） | 非生产 |
| mcp-mesh mesh.rs:1079 | **生产** heartbeat 循环 | 正当周期探活任务 |

**结论**：静态普查的「149 处」严重测试污染；真实生产轮询面 = ①长周期定时器
（60s 报告器族 / 5s·60s experience_loop / 1s efficiency-monitor 采样 /
heartbeat）——全部是正当周期任务，非空闲浪费；②TUI tick（Normal 250ms /
Eco 1s + 60FPS）——唯一真实的空闲唤醒面，由 **R5-3（TUI 空闲 Eco 自动降档）**
处置；③event-bus 生产路径已事件驱动（`select!{sleep, notified}`）。
**不实施任何 Notify 化改造**——不为测试代码与正当定时器写防御性重构。

## 5. D5 · GPU 三门重开就绪度评估报告（R3-4，纯文档）

`docs/reports/gpu-unseal-readiness-2026-11.md`：三重关闭逐项现取复核 +
G-A/G-B/G-C 逐门对照（均不满足）+ 监测仪表三条触发线 + R1 优化使 GPU 边际
收益进一步收窄的量化。**维持 C-15 封印**（用户裁决）。

## 6. 验收

chimera-cli bin 3/3（含新矩阵测试）+ lib 221/0 + nexus-core 206/0；
clippy -D warnings 0 告警；fmt 收敛。零事件变体/零新依赖/零 crate 变更。
