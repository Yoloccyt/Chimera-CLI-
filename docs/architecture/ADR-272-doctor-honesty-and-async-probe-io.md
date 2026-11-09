# ADR-272 -- doctor 探针诚实化 + 异步路径探测 + 门补盲（W6）

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W6
- 关联：ADR-241（D1：doctor 探针同步 FS → `tokio::fs`；本 ADR 补齐其判据面）、ADR-234（证明力分层：L1 结构判据 > 运行时观测）、ADR-239（先实测后登记 / 收益不显著即否决）、ADR-242（回滚开关台账）
- 影响面：`crates/chimera-cli/src/commands/doctor/probes.rs`、`crates/chimera-cli/src/commands/doctor.rs`、`crates/chimera-cli/tests/doctor_async_probe.rs`
- 行为影响：LLM 维结论由随机变确定、文案自陈 mock；doctor 墙钟 −238ms；5 个路径探针判定结果逐字不变

## 1. 背景（F-06 / F-07）

两处缺陷，性质不同：

| # | 缺陷 | 位置（旧） | 后果 |
|---|---|---|---|
| F-06 | LLM 探针 mock 伪装真实 RTT：`sleep(238ms)` + `SystemTime` 纳秒奇偶 50/50，成功文案 `ping 238ms` | `probes.rs:225-244` | doctor 判决不可信（50% 随机 Warn）；每次诊断白付 238ms |
| F-07 | 6 个路径探针内 7 处 `Path::exists`（3 个 async 探针各 1 处；2 个 sync 探针各 2 处） | `probes.rs:19/64/125/461/462/549/550` | async 探针在 worker 上做阻塞 stat |
| F-07b | 门判据只禁 `std::fs::`，`Path::exists` 不在禁列 | `tests/doctor_async_probe.rs:48-55` | 7 处缺陷全部在门内**恒绿** —— 门绿由缺陷撑着 |

F-07b 是这条缺陷最危险的部分：**没有它，F-07 会在第一次回归里被发现**。
一个只锚「错误写法之一」的门，比没有门更糟 —— 它提供虚假的安全感。

## 2. 决策

### D1 LLM 维诚实化：删睡眠与随机，保留 Ok 但文案自陈 mock

- 删 `sleep(238ms)` 与纳秒奇偶 50/50；探针只做它真正能做到的事：
  解析默认 Provider 名（`cfg.model_router.providers` 首位 → `FALLBACK_PROVIDER_NAMES`）。
- 成功文案改为：`LLM: default=<name>（仅校验配置可解析;连通性未探测 — mock 探针,真实探测待接 mca-gateway）`。
- **不实装真实 ping**：探针内直调 `llm::execute` 会引入 doctor → llm::execute 的间接依赖，
  mca-gateway 接入后有回环风险；真实探测属独立波次，须有独立入口。
- **为什么不降为 Warn**：本探针确实验证了「配置可解析」这一事实。把它降成 Warn 会让
  配置完全正确的机器常态告警 —— 那只是把不可信从「过乐观」换成「过悲观」。
  ★ 诚实化的目标是**结论可被正确解读**，不是**状态码必须悲观**。

### D2 异步路径探测：`try_exists` 取代 `exists`，写盘走 `spawn_blocking`

| 探针 | 旧 | 新 |
|---|---|---|
| `check_config_file` | `config_path.exists()` + `init_config_file`（同步写盘） | `tokio::fs::try_exists(..).await.unwrap_or(false)`；`--fix` 生成走 `spawn_blocking` |
| `check_cargo_lock` | `lock_path.exists()` | `tokio::fs::try_exists(..).await` |
| `check_sqlite_path` | `parent_dir.exists()` | `tokio::fs::try_exists(..).await` |
| `check_storage_health` | sync fn + 2×`Path::exists` | **async** fn + 2×`try_exists`；`collect_storage_health` 改为「就绪布尔注入」纯函数 |
| `check_model_assets` | sync fn + 2×`Path::exists` | **async** fn + 2×`try_exists`；`collect_model_asset_health` 同样改纯函数 |

- `try_exists` 的错误分支（如权限拒绝）经 `unwrap_or(false)` 映射为 false —— 与 `exists()`
  的语义**等价**（后者在出错时也返回 false），故判定结果不变。
- 保留 sync 的三个探针（`check_auth_keys` 读 env / `check_seccore` 纯 CPU /
  `check_scc_cache` 内存统计）：零文件 IO，已在文档登记，不引入无意义的 `async`。
- **不用 `spawn_blocking` 包 `Path::exists`**：一次线程跳转的成本高于一次 µs 级 stat，
  是负优化；`tokio::fs::try_exists` 才是正解。`spawn_blocking` 只用于**真同步写盘**
  （`init_config_file` 含 `create_dir_all` + 整文件写）。

### D3 门补盲：判据从「禁一种写法」扩为「禁整类语义」

`tests/doctor_async_probe.rs` 新增模块级判据，覆盖 `probes.rs` 全文：

```text
!text.contains(".exists(")                 // 阻塞 stat 的任一拼法
!text.contains("std::fs::")                // 同步 FS 的任一调用
text.matches("tokio::fs::try_exists") >= 5 // 5 个路径探针都在用异步原语
```

- 覆盖面由 1 个探针（sqlite_path）扩到 5 个（另加 config_file / cargo_lock /
  storage_health / model_asset_health）。
- **不设 collector 豁免**：collector 已改为纯布尔注入，源码里不再有 `.exists(` ——
  豁免越少，门越硬。这与 ADR-266「台账 verdict 必须由机器派生」同一纪律。

### D4 不为「并发层」再造一个弱判据

既有 `sqlite_path_probe_does_not_block_worker` 心跳测试**保留**（它是 ADR-241 的既有
验收，不删）。但**不**为其余 4 个探针复制同款心跳测试：路径 `stat` 是 µs 级，
「心跳能否推进」对「是否阻塞 worker」没有判别力（ADR-234 的 L3/L4 判据不成立）。
新增判据全部是 L1 源码结构判据 —— 强判据一个胜过弱判据五个。

## 3. 验收（现跑）

| 判据 | 要求 | 结果 |
|---|---|---|
| `cargo test -p chimera-cli --test doctor_async_probe` | 5/5（既有 3 零修改 + 新增 2） | ✅ 5 passed |
| `cargo test -p chimera-cli --lib commands::doctor` | 15/15 | ✅ 15 passed |
| `cargo clippy -p chimera-cli --all-targets -- -D warnings` | 0 warning | ✅ |
| doctor 墙钟 A/B | 同 checkout、各 5 次热跑中位数 | 见 §6 |
| LLM 维输出 | 不再含 `ping`，含「未探测」与「mock」 | 见 §6 |

## 4. 回滚

单 commit revert。**门与代码必须同批 revert**：只回滚代码而保留新门会立刻红
（这正是设计意图）；只回滚门而保留代码则失去 F-07 的防再发能力。
本波无运行时开关（诚实化与异步化都不改变判定结果，不需要 L1 开关）。

## 5. 未采纳

| 项 | 结论 |
|---|---|
| 实装真实 LLM ping（在 probes 内调 `llm::execute`） | 否决：回环风险；真实探测属独立波次，须独立入口 |
| 把 LLM 维从 Ok 降为 Warn（「未探测」= 不健康） | 否决：过度悲观，把不可信从过乐观换成过悲观 |
| 用 `spawn_blocking` 包 5 个探针的 `Path::exists` | 否决：线程跳转 > µs 级 stat，负优化 |
| 为 collector 保留 `.exists(` 豁免 | 否决：改纯布尔注入后无需豁免；豁免越少门越硬 |
| 为「gather 墙钟收益 <5% 即否决异步化」设门槛 | **不适用**：见 §6.2 —— 本波的异步化不是性能主张，是阻塞纪律 |

## 6. 实测

### 6.1 doctor 墙钟 A/B（同轮对照，Python 单进程计时，各 7 次热跑 + 3 次日志取样）

对照对象：
- **baseline** = `target/debug/deps/chimera-def5eee1a648ba86.exe`（2026-11-08 构建的 deps 产物）。
  该二进制的 `probes.rs` 行为与本 ADR 落地前的 HEAD **逐字相同**（其间唯一改动是 W0
  的 `:402` 注释订正，`git log -- crates/chimera-cli/src/commands/doctor/probes.rs` 可证）；
  实跑输出确实为旧文案 `LLM: ✓ default=claude-opus, ping 238ms`。
- **W6** = 本波重建的 `target/debug/chimera.exe`。

| 指标 | baseline | W6 | 差值 | 区间是否重叠 |
|---|---|---|---|---|
| 进程墙钟（中位数） | **288.9 ms**（282.3–298.8，n=7） | **50.5 ms**（44.3–54.0，n=7） | **−238.4 ms（−82.5%）** | 不重叠 |
| `gather 完成 latency_ms`（中位数） | **246.3 ms**（243.6–255.0，n=3） | **1.25 ms**（1.02–1.49，n=3） | **−245.0 ms（−99.5%）** | 不重叠 |

- 两把尺子互相印证：进程墙钟的降幅（238ms）≈ 被删除的 mock 睡眠（238ms），
  说明收益**完全来自 D1**，不是 D2 的 `try_exists`。
- 11 个探针现在全部是毫秒级快路径（合计 ≈1.25ms），`gather` 的下一瓶颈将是
  组合根装配（日志实测 `assemble_ms=1.19`）与进程启动，不再是探针。

**方法论两条**（本轮踩到，记录以免复发）：
1. **不要用 Git Bash 的 `date +%s%N` 夹测**：每次 `date` 都是一次 Windows 子进程，
   单次 ~50–150ms 抖动，会把 300ms 量级的差异完全淹没（本轮首测得到「−52ms、区间重叠」
   的错误结论，改用 Python `perf_counter()` 后浮出真实差异）。
2. **日志字段有 ANSI 颜色码**：`latency_ms` 与 `=` 之间夹着 `\x1b[3m…\x1b[0m\x1b[2m=\x1b[0m`，
   直接 `grep 'latency_ms='` 会 0 命中。抽取前必须剥离 `\x1b\[[0-9;]*m`。

### 6.2 关于「收益 <5% 即否决异步化」的适用性说明

原方案的 W6 验收写了「`gather` 键同轮对照，墙钟收益 <5% 则异步化正式否决」。
本轮**不套用**该门槛，理由如下（如实登记，不包装）：

1. `cargo bench -p gqep-executor --bench gather` 测的是 gqep 聚集器对**合成 future**
   的调度开销，与 doctor 的 11 个探针集合**无对应关系** —— 用它给 doctor 的
   `try_exists` 改造打分是**错误的尺子**。
2. 5 个 `stat` 共 µs 级，任何 wall-clock 测量都被 238ms mock 睡眠（旧）与进程启动噪音
   淹没；即使测出差异也无法归因。
3. 因此本波把「`exists` → `try_exists`」定位为**阻塞纪律修复（L1 结构判据）**，
   **不主张**任何性能收益；真正可测量的收益来自 D1 删除的 238ms 睡眠（见 6.1）。
   ★ 不主张收益 ≠ 偷偷主张收益：ADR-239 要求的是「主张必须有证据」，
   而不是「没有收益就不许改」——后者会让纪律性修复永远排在性能项之后。
