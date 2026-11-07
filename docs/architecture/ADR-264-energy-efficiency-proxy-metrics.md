# ADR-264: 能效代理指标体系（R5）— 三代理定义 + 取证基线 + TUI 空闲 Eco 设计冻结

- **状态**：Partially Accepted（2026-11-08，`feat/toolchain-parallel-optimization` R5 波次；R5-1/R5-2 采纳，R5-3 **设计冻结、实施暂缓待真机验证**）
- **上游**：已批准方案「工具链系统性深度优化重构方案」R5 节（用户裁决口径：代理指标 = 硬验收，真实功耗 = 参考证据）
- **关联**：ADR-262 §4（轮询取证）/ ADR-263（分配预算棘轮，代理①的载体）/ ADR-260（零分配锁）

## 1. D1 · 三代理指标定义（硬验收口径）

| # | 代理指标 | 载体 | 基线 |
| --- | --- | --- | --- |
| ① | 每请求堆分配数 | R4-1 分配预算棘轮（`dispatch_alloc_budget.rs`：单发 7 / 批量 68，只降不升） | ADR-263 §1 |
| ② | 每请求 CPU-ms | criterion 同轮读数（spol/tbatch/dchain 现键体系） | ADR-260 §4 等 |
| ③ | 空闲唤醒率（静态生产轮询点数） | `scripts/idle_wakeup_audit.py`（dev-only advisory，exit 0） | **生产区 32 点**（裸 grep 149 的测试污染 99 处被分类剥离；分布=正当长周期定时器 chimera-cli 8 / mcp-mesh 7 / 其余 1~2） |

WHY 代理而非瓦特：Windows 宿主无可编程 RAPL；CPU-ms/分配数/唤醒率与能耗
单调相关且**可机器验证**（可入 CI 棘轮），真实功耗噪声大且依赖外部环境。

## 2. D2 · 证据工具（R5-2 采纳，dev-only 参考旁证）

`scripts/energy_profile.ps1`（参数化时长/进程）：整包 CPU% 采样 + 进程 CPU
时间/工作集 + 电池报告（笔记本自动检测）。**不入 CI 门禁**；输出自带定位
声明（硬验收 = 代理指标）。

## 3. D3 · TUI 空闲 Eco 自动降档（R5-3 设计冻结，实施暂缓）

**动机**（取证结论，ADR-262 §4）：生产空闲唤醒面 = TUI tick（Normal 250ms
tick + 100ms 事件 poll + 60FPS + sysinfo 5s）——唯一真实可治理面。

**冻结设计**（实施时照此施工，勿重设计）：
1. `TuiConfig` 增 `idle_eco_minutes: u32`——**默认 0（禁用）**，真机验证
   通过后按方案改 5；配置模板（`config.rs::omega_yaml_template`）同步 +
   parity 脚本重生成（G-68/G-69 纪律）。
2. 转移函数（纯函数可测，置于 event_loop.rs）：
   `idle_eco_transition(idle, threshold, current, auto_active) -> Option<TickMode>`——
   `(Normal, false)` 且 idle ≥ threshold → `Eco`；`(Eco, true)` 且 idle <
   threshold → `Normal`；手动 Eco（`auto_active=false`）不被自动恢复覆盖。
3. 接线点：主循环 poll 超时分支（event_loop.rs ≈:1704）与 step 循环
   `ev.is_none()` 分支（≈:1766）检查降档；`Event::Key/Mouse` 处理处重置
   `last_interaction_at` 并按 `auto_active` 恢复 Normal。
4. **验收红线（用户规则）**：TUI 改动必须真实终端人工验证（空闲降档触发 +
   交互恢复 + 手动 Eco 不被覆盖三场景）后才可设默认值 5——
   **本波未验证，故不实施**；实施波必须附验证记录。

## 4. D4 · 治理结算

- R1（判决体）与 R2（批量装箱）本身即最大能效项：分配数下降 + CPU-ms 大降
  （ADR-260 §4：策略评估 1µs→66ns/双评估）。
- 代理③基线 32 生产点全部为正当周期任务（60s 报告器/heartbeat/1s 采样），
  无激进 Notify 化空间（ADR-262 §4 证据表）。
- 治理前后对比工具就位：`idle_wakeup_audit.py` 基线 + `energy_profile.ps1`
  旁证，后续任何能效改动以此对账。

## 5. 回滚

两脚本为纯增量 dev-only 工具（revert 无涟漪）；R5-3 未实施故无回滚面。
