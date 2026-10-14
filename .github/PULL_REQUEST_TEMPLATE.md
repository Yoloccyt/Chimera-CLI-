# Pull Request 模板

> 本模板存在的一条硬理由：**本仓反复出现"虚假还债"**——用 `optional = true`、feature 数组或注释
> 把"未接线"宣布成"已接线"。因此这里把**两账分开**列为必答项：
> `reachable`（能被链接到）≠ `wired`（生产链上有构造点）。

## 1. 一句话动机

改了什么 / 为什么现在改。若本次是**还债**（销掉某条台账或主张），必须写出债的原始编号
（`C-xx`、`WI-xx`、`ADR-xxx`、`G-xx`）与所在文档。

## 2. 两账分开（必填，缺一按未接线处理）

| 项 | 内容 |
|---|---|
| 声明面（账 A） | 本 PR 修改/新增的台账行：`scripts/dep_edge_freeze.txt`、`scripts/dev_dep_freeze.txt`、`scripts/fnlen_baseline.txt`、`scripts/entry_contract_baseline.txt`、`scripts/doc_count_freeze.txt`、`scripts/test_surface_freeze.txt`、`scripts/bench_inventory_freeze.txt` 等（写"改了哪几行、从几到几"） |
| 装配面（账 B） | 生产构造点证据：`path/to/file.rs:LINE` + 被谁构造/调用。**只有测试构造点（`#[cfg(test)]` 区内）的，状态写 `DORMANT` 或 `DECORATIVE`，不得写 `WIRED`** |
| 二者是否一致 | 一致 / 不一致（不一致时必须说明登记为哪一态，以及不一致的债由谁在何时销） |

**还债的定义**：还债 = **消掉台账行**，或在台账里把状态改为可证伪的较低态。
以下动作**不算还债**：加豁免、改注释措辞、把依赖设成 `optional`、给 feature 数组加一项、
新增 `[dev-dependencies]` 同名录项（生产边已在册时那是冗余项）。

## 3. 棘轮方向（只减不增）

- 任何"只减不增"型台账：本 PR 只能让条目变少或不变；确需新增时，附**理由 + 去向（何时销）**，
  并预期被审计（原则 4：登记册变更方向不可反）。
- 判据本体能修好的，**优先改判据，不加登记豁免**（G-27/G-84/G-93 先例）。
- 行号键（`path:line`）的续期本身就是复发源：能用重构消掉形状就别新增一条键。

## 4. 门与证据（提交前自查）

```bash
py -3 scripts/run_gate_manifest.py            # 期望末行 [SUM] 各类计数之和 == 门清单数，且 rc=0
py -3 scripts/check_entry_contract.py         # 新增/改动 scripts/ 下脚本时必跑
py -3 scripts/crate_contract.py --check       # 改了 crate 契约块时必跑（勿手改派生字段，用 --install 再生）
py -3 scripts/check_doc_symbol_claims.py      # 文档写了符号名时必跑
pwsh -NoProfile -File scripts/check_doc_consistency.ps1
```

- **ENV-FAIL ≠ 通过**：运行器报 `env-fail` 时要去修环境或如实登记，不得当绿。
- **工具自己崩 ≠ 门给出红**：必须断言"判决行数 == 预期门数"；`0/0 clean`、"零命中"、
  "无 diff 输出"这类结果，先证明**被查对象存在**再下结论。
- 新门须自带牙齿：`--selftest` 夹具 + 一次真变异（故意破坏 ⇒ 立刻翻红 ⇒ 字节还原），
  并在 `scripts/gate_manifest.toml` 的 `note` 里写清判据与现读值。

## 5. 主张的可证伪性

- 写"已落地/已接线"必须能指到**生产构造点**；指不到就用 `DESIGN-NAME-NO-CARRIER` / `DORMANT`。
- 写进文档的**计数**要么带"来源 + 日期 + 可自校验的配对形式"，要么交给工具自报；
  不要往文档里抄一串无主数字（原则 5：计数只入登记面）。
- 未实测的性能数字一律标【待验证】，**禁止写入** `scripts/perf_redlines.toml`、
  `scripts/perf_thresholds.toml`、`.github/workflows/bench_check.yml`。
- `LANDED`（有载体）≠ `WIRED`（已接线）；`reachable` ≠ `wired`；导出（`pub mod`）≠ 装配。

## 6. 红线（触碰即需新 ADR，不得在本 PR 内自行放行）

新增 crate / 新增跨层直连（只禁向上；L(N)→L(N-1) 合法）/ 新增或删除 `NexusEvent` 变体（需六锁同批）/
`unsafe` 或 `allow(unsafe_code)` / 扩 `UNLEARNABLE_SECURITY_RULES` / R2·RL·GPU·Python 服务解冻 /
修改 `MATURITY`、`CONSUMERS` 等由 `crate_contract.py` **机器派生**的字段。

## 7. 回滚

写清回滚路径（单 commit 可 revert / 需同时撤销的台账行 / 数据面是否可逆）。
批次拆分原则：每波 ≤ 8 个工作项，逐 commit 独立可 revert，触发文件不外溢。
