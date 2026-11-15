# ADR-304: 度量地基——压缩率、检索 SLO、并行率与 RSS 采集器

- 状态: Accepted（RSS 基线待复测）
- 日期: 2026-11-14
- 批次: 本 Spec D0 波
- 关联: ADR-296（R12 W0 取数纪律）、ADR-234（判据四层）、ADR-301（A 波接线）

## D1 — 生产压缩率基准

- 新增 `crates/hcw-window/benches/assembly_dedup.rs`（`[[bench]] harness=false`）。
- 固定语料 200 条：80 条唯一（60 对话 + 20 长工具响应）+ 60 条精确重复 + 60 条规范化重复；启动即断言 `original/used ≥ 2.5`。
- 实测（2026-11-14，本机 release，同轮）：
  - `original_tokens=4760`、`used_tokens=1780`、`ratio=2.674`、`reduction=62.6%`；
  - `deduped=120`、`deduped_normalized=60`、`kept=80`；
  - 装配耗时 `[101.11 µs, 101.93 µs, 102.80 µs]`。
- 登记：`scripts/bench_inventory_freeze.txt` 新增 `hcw-window|assembly_dedup|dev-only`，重开条件=A5 压缩率门进入 CI 后升格 `registered`。
- 诚实边界：该数值是 **HCW 装配内核路径**；mca 生产裁剪按 ADR-298 D2 只对 `Tool` 消息去重，端到端压缩率取决于工具输出重复度，不能直接外推。

## D2 — 检索 SLO 基线

| 组 | 实测 | 登记阈值 | 结论 |
|---|---|---|---|
| `l2_recall_top10_100_entries` | 35.45 µs | 5 ms | PASS |
| `l2_recall_top10_4096_entries` | 141.28 µs | 5 ms | PASS |
| `memory_graph/recall_2k_depth2` | 1.272 ms | 4 ms | PASS |
| `memory_graph/build_semantic_edges/999` | 437.81 ms | — | **断崖仍在（见下）** |
| `.../1000` | 51.94 ms | — | HNSW 路径 |
| `.../1001` | 52.48 ms | — | HNSW 路径 |

- **已知边界**：R12 W2 把判据从 `>` 改为 `>=`，只把断崖从 1000 挪到 999（码内注释已自陈）。本批未获授权改变 512–999 区间的建边语义（精确→近似），故不消除；A6 的"三档单调"判据**未通过**，登记为待独立 ADR。

## D3 — 并行率判据

- 新增 `crates/gqep-executor/tests/parallel_rate.rs`：8 个独立只读节点，20 ms 服务时间（避开 Windows 15.6 ms 定时器粒度）。
- 实测：`cap=8` 在飞峰值 = 8（重叠率 1.0）；`cap=4` 峰值 = 4（满额且不超卖）。
- 证据：`cargo test -p gqep-executor --test parallel_rate` 2 passed。

## D4 — 峰值 RSS 采集器

- 新增 `scripts/measure_rss.ps1`：`Start-Process` + 100 ms 轮询 `PeakWorkingSet64`，输出 `tmp/rss_<name>.json`。
- 首次实测 `plan64`（64 节点只读计划，provider 未注册导致 exit=4）：`peak=2.36 MB`、`duration=0.437s`。
- **判定：不可用**。2.36 MB 对 1.18 GB debug 二进制不合理，怀疑 `Start-Process` 进程对象未反映真实峰值或测量窗口过短。D0-3/B4 不勾选，需改用 ETW / `Get-Counter '\Process(...)\Working Set - Peak'` 或进程内 `GetProcessMemoryInfo` 复测。
- 不伪造基线，不以该值计算 -30%。

## D5 — 取数纪律

沿用 ADR-296 D4：先看 mtime；跨会话 `change:` 不作因果证据；冷启动首轮只作量级锚；编译期间不采集。本 ADR 所有数值均注明命令与同轮条件。

## 回滚

- 三个新增文件（bench / test / script）均可独立删除；`bench_inventory_freeze.txt` 对应行删除即恢复。
- 未修改任何生产语义（A 波接线见 ADR-301）。
