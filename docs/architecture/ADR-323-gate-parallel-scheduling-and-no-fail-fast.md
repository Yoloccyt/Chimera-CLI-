# ADR-323：收口入口的并行解锁、并发度三档与 `--no-fail-fast` 全量档

- **编号**：ADR-323
- **日期**：2026-11-18
- **状态**：Accepted
- **决策者**：L1 深度重构 + 工具链并行调度波次（用户裁决 Q-03 = 并发度 4）
- **关联提交**：`f38021fc`（清单域化 + exclusive 互斥链）、`9f3fc53b`（解除串行降级 + `--no-fail-fast` + A/B 实测）
- **关联 ADR**：**ADR-319（工具链并行调度重构伞决策 —— 本 ADR 是其下的实施面子决策）**、ADR-315（门禁资源分级与 `exclusive` 互斥链）

---

## Context

`scripts/govd/` 调度器**早已具备**并发能力：

- `scripts/govd/cli.py` 的 `ThreadPoolExecutor(max_workers=jobs)`
- `exclusive` 键分区链（`cli.py` 的 `exclusive_groups` / `run_exclusive_chain`），保证同资源键的门串行、不同键并行
- `policy.toml` 的 `[profiles.pr] jobs = 4`

但唯一的收口入口 `scripts/run_gate_manifest.py` 在 `run_govd()` 内**硬编码** `argv += ["--jobs", "1"]`，其注释写「The adapter deliberately runs serially so that the parity run compares like with like」。

后果：159 条门中的 152 条 light 门**全部串行**执行，其中 97% 是零 cargo 的静态门，彼此无互斥依赖。这是纯调度浪费 —— 能力已建成，闸门被自己关上。

同时，`govd` 的真红即停（fail-fast）是硬编码的：一旦出现 mismatch，未启动的门全部记 `CANCELLED`。这让两件事无法进行：

1. **存量债普查**：fail-fast 让后续门永远 CANCELLED，看不清债务总量；
2. **A/B 判定不变性**：同轮配对需要两档都**跑满**，否则五态分布不可比。

## Decision

### 决策 1：解除收口入口的串行硬编码

`run_gate_manifest.py::run_govd()` 不再钉死 `--jobs 1`：

- 新增 `--jobs N` 旗标（缺省 0 = 交给 profile + 资源上限决定）
- **`--jobs 1` 仍然是合法的安全回退态**（配置级回滚，零代码改动）
- `KNOWN_FLAGS` / `USAGE` 同步；未知旗标仍退 2（不静默降级）

### 决策 2：并发度三档短路

```
最终 jobs = 显式 --jobs（>0 时压倒一切）
          > profile 声明值（policy.toml）
          > 资源上限压制（min(floor(cores/2), 4)；mem<16GB 或 disk<30GB 降为 2）
```

**资源上限只做向下压制，不得把低配机的 profile 值往上抬。** cargo 域保持全局单写（`exclusive = "cargo"` + cargo 自带 target-dir 锁），不得放大 cargo 命令自身的 `--jobs`。nextest `slo` 档的 `test-threads = 1` 不被调度器覆盖（时序判据在并行下必假失败）。

### 决策 3：新增 `--no-fail-fast` 全量档

缺省行为**不变**（真红即停）。`--no-fail-fast` 时 mismatch 不再 set stop，跑完全部门，用于：
- 存量债普查（拿到「全部门 × 全部判定」的一张完整台账）
- A/B 同轮配对（判定不变性需要一个跑满的基线）

`resume` 子命令同批接受该旗标 —— 否则 resumed run 会静默重新武装 fail-fast。

## Consequences

### 正面

- **实测收益 2.71×**（152 条 light 门，同轮配对 3 轮 median：398.9 s → 147.0 s）
- **判定完全不变**：三轮两档 `[SUM]` 五态逐字一致（ok=132 / mismatch=14 / env-fail=6 / unknown=0 / skip=7）
- **回滚零成本**：`--jobs 1` 即退回串行
- 存量债可一次看清（23 条预存红全部登记，不掩盖）

### 负面 / 约束

- 并发档受同机其它进程影响更大：B 档三轮极差 31 s（21%），A 档仅 1.3 s（0.3%）。因此**任何并发收益声明必须同轮配对**，跨日对比无效。
- 加速比低于理论上限 4×：门集合不均匀，少数长门仍构成关键路径。【待验证】`--jobs 8` 能否继续压缩，需同时验证不 OOM（本机 15.19 GB）。

## Rollback

```powershell
# 配置级（首选，零代码）
py -3 scripts/run_gate_manifest.py --jobs 1
# 代码级
git revert 9f3fc53b
```

## Evidence

| 事实 | 证据（文件 + 符号） |
|---|---|
| 硬编码串行曾存在 | `scripts/run_gate_manifest.py::run_govd`（历史注释 + `argv += ["--jobs", "1"]`） |
| 调度器并发能力 | `scripts/govd/cli.py`（`ThreadPoolExecutor`、`run_exclusive_chain`） |
| 互斥链语义 | `scripts/govd/model.py::Gate.exclusive`、`scripts/govd/cli.py` exclusive 分区 |
| 并发度三档 | `scripts/govd/policy.py::derive_jobs` / `probe_resources`、`cli.py::resolve_jobs` |
| 全量档 | `scripts/govd/cli.py`（`--no-fail-fast` / `dest="no_fail_fast"`） |
| 实测数据 | `docs/reports/toolchain-parallel-scheduling-2026-11-18.md`（按 ADR-166 不入库） |
| 行为测试 | `scripts/govd/tests/test_govd.py`（90 用例，含区间重叠阴性/阳性对照） |

---

*本 ADR 的所有数字均为 2026-11-18 现取；未实测项已在报告中标注【待验证】。*
