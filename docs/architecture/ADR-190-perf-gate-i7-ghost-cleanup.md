# ADR-190：性能门 I7 幽灵与"门失效化"反模式治理

- **状态**：✅ Accepted（2026-09-25）
- **背景（本轮实测）**：性能门 `check_perf_redlines.py --static-only`（gate_manifest **G-02，expect=0**）长期 **rc=1**，
  唯一 FAIL 为不变式 **I7**（`bench_check.yml run_bench 行 ⇔ perf_thresholds.toml [[run_bench]] 表`）。
  根因：`auto-dpo` crate 已于 `38c72e6`（批次 A）物理删除，`perf_thresholds.toml` 于 2026-09-20 删了它的
  `rhi_judge` 阈值键（仅留 L57 注释），但 **bench_check.yml 侧的 4 个引用面未同步清理**
  （run_bench 执行行 L136、Assert-thresholds heredoc 的 `thresholds` 字典 L239 与 `path_keywords` 字典 L329、
  及 perf_redlines.toml `[[slo]] rhi_judge` 块），形成"CI 跑一条不存在的 bench、表侧已删"的单侧漂移 → I7 恒红。
  同时 `[[slo]] rhi_judge` 是 `check_perf_redlines.py:268-274`（F60）已记录的"SLO 幽灵"——
  `[slo]` 表不参与盘上枚举，幽灵条目**静默抬高 I5 的 `n_slo >= 7` 计数**，使该不变式的"绿"由幽灵撑着。

## 1. 问题定性：门失效化反模式

一个登记为 `expect=0`（应阻塞）的门若长期判红，会滑向两种恶化路径：
1. **常态化豁免**：在 CI 里给它挂 `continue-on-error` / 从阻塞集摘出，红线静默失效；
2. **改判据凑绿**：像 2026-09-18 幻影 `ADR-XXX` 单侧改 `.ps1` 那样，只动不执行的一侧让本地变绿。
两者都让"门存在"沦为名义。I7 红是**真实漂移**（bench_check.yml 与表两侧对不平），不可豁免，只能真清。

## 2. 决策（原子清 4 引用面 + floor 校准，禁单侧改）

1. **bench_check.yml**（3 处）：`run_bench rhi_judge` 执行行、heredoc `thresholds["rhi_judge"]`、
   `path_keywords["rhi_judge"]` → 全部转为带理由的注释（保持 YAML/py 合法）。
2. **perf_redlines.toml**：删除 `[[slo]] name="rhi_judge" crate="auto-dpo"` 幽灵块。
3. **perf_thresholds.toml**：无需改（2026-09-20 已删其键），L57 注释保留为历史取证。
4. **I5 floor 校准 7→6**：幽灵删除后真实 SLO=6，把 `n_slo >= 7` 改 `>= 6`——
   **变绿非放宽**（旧的 7 含 1 条幽灵，诚实基线就是 6），与 ADR-187 §1.3"删门不损判别力"同理，附注释说明。

**双源同 commit 纪律**（承 ADR-185/187）：bench_check.yml 与 perf_*.toml 属同一"CI 真值三表"，
任一侧改必须同 commit 对平，由 I7 机器断言兜底（本 ADR 让 I7 从"被动报警"转"被满足"）。

## 3. 验收（本会话实测，可复跑）

```
python scripts/check_perf_redlines.py invariants    # I5 11rl/6slo PASS、I7 33 pairs PASS，rc=0
python scripts/check_perf_redlines.py --selftest     # rc=0（35 夹具断言，ghost-fatal 断言仍有效）
python scripts/check_perf_redlines.py --static-only  # G-02 VERDICT PASS rc=0（门从长期红转真绿）
python scripts/check_perf_redlines.py inventory      # 无 SLO-GHOST advisory，rc=0
```

## 4. 推广（深审登记的反模式族）

本 ADR 确立"门失效化"检测范式，纳入四维深审报告 §风险矩阵。同类待治项（同报告）：
- 可观测：`bus.rs:461 logger:None` → L1 全套 Prometheus 指标生产死码（门绿但线未接）；
  MetricsDashboard 生产 10 cell 未 bind；EfficiencyMonitor 从未装配（Critical 丢弃告警断链）。
- 可靠性：`backpressure.rs` 的 `SlowConsumerDetector`/`BackpressurePolicy` 生产零接线；
  `bus.rs` Lagged 静默丢弃；Critical 无 WAL。
- 测试面：32 个 `MUTED/NEVER_COMPILED` 测试被"已登记"当通过；`coverage_per_crate_floor.toml`
  兜底 floor 仍停 45（文件自陈"禁止长期停在兜底值"）。
判据同 I7：**"门 expect=0 却 rc≠0" 或 "机制在、生产零调用点" 一律视为待清债务，不得豁免常态化。**

## 5. 后续
- ✅ 已落地（2026-09-25）：“注释/账本中的事件计数须等于权威常量”——`check_perf_redlines.py` 新增不变式 **I8**，断言 shard.rs 注释 "N 个 Critical 事件" == bus.rs `CRITICAL_TOTAL`（防 shard 类漂移回潮）。
- SLO 数值取证（`cargo bench` 生成 `target/criterion`）仍待（耗时）；本 ADR 只清结构性幽灵。
