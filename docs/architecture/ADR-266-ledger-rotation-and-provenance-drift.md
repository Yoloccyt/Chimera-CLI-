# ADR-266 -- 「四件同批」违规追认：gated-wiring 台账旋转、依赖豁免回收与两处漂移订正

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W0（残余收口首波，阻塞项）
- 关联：ADR-258（nexus-hook 注册面闭合，工单 F）、ADR-259（U 项三态裁决 + 五回归锁）、ADR-188（岛屿退役/接线权威）、ADR-160（可达性棘轮）、ADR-244（ToolRuntime 单一装配点）
- 影响面：`scripts/gated_wiring_baseline.txt`、`scripts/declared_dep_exceptions.txt`、`crates/chimera-cli/src/commands/doctor/probes.rs`、`crates/chimera-cli/src/exec_plan.rs`、新增 `crates/chimera-cli/tests/ledger_parity_locks.rs`
- 行为影响：**零**（纯台账 + 注释 + 测试；无 Rust 逻辑改动）

## 1. 背景：两道门真红，且同源同批

当轮现跑（HEAD `62198d33`，`py -3`；本机 `python` 不在 PATH，rc=127）：

| 门 | rc | 失败行 |
|---|---|---|
| `check_gated_wiring_substance.py` | 1 | `[GAP] {chtc-bridge,csn-substitutor,mtpe-executor,nexus-hook,sesa-router,ssra-fusion}\|PARTIAL is not allowed by the ledger` + 6 行 `[STALE] …\|DECORATIVE` |
| `check_declared_dep_usage.py` | 1 | `[STALE] chimera-cli -> nexus-hook` |

**根因**：`06581d15`（ADR-258 / 工单 F）在 `composition.rs:358-363` 落了
`nexus_hook::event_bridge::HookEventBridge::register_all` 的真实生产调用，
但**未同批**回收两张台账：

1. `gated_wiring_baseline.txt` 仍写 6 行 `DECORATIVE`；
2. `declared_dep_exceptions.txt:26` 的豁免理由仍声称「src/tests/benches 零引用」，
   而该前提已被同一 commit 推翻。按该表头部规则
   「an exception must not outlive the debt it excused」，该行必须回收。

这是「四件同批」（代码 + 测试 + 台账 + ADR）纪律的一次实质违规：
接线本身是对的，漏的是**让机器能看见接线已发生**的那 half。

### 1.1 为什么 6 个岛一起变 PARTIAL（判据口径，不是债务增长）

`--emit` 机器读数：

```
chtc-bridge|PARTIAL|3|0|r2_island_repayment
csn-substitutor|PARTIAL|3|0|r2_island_repayment
decay-engine|PARTIAL|11|0|r2_unfreeze
mca-gateway|WIRED|10|6|mca
mtpe-executor|PARTIAL|3|0|r2_island_repayment
nexus-hook|PARTIAL|3|0|r2_island_repayment
omega-learner|PARTIAL|11|0|r2_unfreeze
sesa-router|PARTIAL|3|0|r2_island_repayment
ssra-fusion|PARTIAL|3|0|r2_island_repayment
```

`r2_island_repayment` 是 **feature 级共享 enabler**（`crates/chimera-cli/Cargo.toml`
一次 enable 6 个 optional 内部 dep），门按 enabler 汇总 cfg 位点
（`check_gated_wiring_substance.py:178`）。ADR-258 落的 3 个 cfg 位点
（`composition.rs:131/:358/:423`）因此使**全部 6 个** dep 同判 `cfg=3 ci=0` = PARTIAL。

**关键区分（本 ADR 的核心判断）**：这 6 个 PARTIAL 里只有 **nexus-hook 是真实接线**；
其余 5 岛（chtc/csn/mtpe/sesa/ssra）的 src 引用仍为零，只是被共享计数连带抬升。
把「计数抬升」读成「5 岛已接线」会导致下一步错误地回收它们的预留位或宣称可达性改善。
台账 reason 因此**分别**书写两种成因，并由文本锁强制要求 5 岛行含「零引用」字样。

`ci=0` 的含义：`r2_island_repayment` 在任何 CI workflow 中都未被 `--features` 启用
（全域 grep 只命中 `mca`），故这些边在默认构建与 CI 中均不生效。

## 2. 决策

### D1：台账旋转 6 行 `DECORATIVE` → `PARTIAL`，verdict 由 `--emit` 派生

`--emit` 打印的是 5 字段诊断形（`dep|verdict|cfg|ci|enablers`），
而台账要求 `dep|verdict # reason`（`load_ledger:192-208`，reason 必填否则退 2）。
故做法是：**verdict 取机器读数**（不凭记忆手填），reason 手工撰写并区分两种成因。
`decay-engine`/`omega-learner` 两行本就是 PARTIAL，不动；
`mca-gateway` 是 WIRED ⇒ 不属 debt ⇒ **不得**有台账行。

### D2：回收 `chimera-cli -> nexus-hook` 豁免行，并订正该表计数漂移

删除后剩 5 条。同时订正一处**既存**计数漂移：说明段原写「7 条」，
而当轮实际只有 6 条数据行（chtc/csn/mtpe/nexus-hook/sesa/ssra）——
即该注释在本批之前就已失真。本批把注释与数据行都对齐到 **5**，
并由文本锁断言二者一致（防再次漂移）。
其余 5 行 reason 里的「已按 DECORATIVE 登记」同步改为 PARTIAL，
否则豁免表与台账互相矛盾。

### D3：订正两处会误导后续规划的文档漂移

1. `doctor/probes.rs:402` 原称「生产装配尚未构造 `CbmrWriter`（现测全库零生产构造点）」，
   与 `composition.rs:489`（`CbmrWriter::open` → `AppServer::with_session_store`）
   + `commands/serve.rs:47` + `commands/acp.rs:126` 直接矛盾。
   订正为：**写面已接线**、**读面 `replay_since` 按 ADR-259 D3 维持预留**。
   ⚠️ ADR-259 的 `u08_session_store_note_parity` 锁只锚 `session-store/src/lib.rs`，
   **抓不到 probes 这同一事实的另一份陈述** —— 本批新增锁 3 补该盲区。
2. `exec_plan.rs:372/:421` 把「装饰器顺序即失败归因契约」溯源到 **ADR-257**，
   但 ADR-257 是 nexus-subagent 装配，与装饰器无关。
   `git log -L 370,390:crates/chimera-cli/src/exec_plan.rs` 证明真实落地者是
   **`c6eb9b62`**（Counting/OpTimeout 换位 + 反向对照测试）。订正溯源。
   本轮勘察即因这处误引产生过一次错误归因，故它不是纯文字问题。

### D4：新增四条文本锁（`tests/ledger_parity_locks.rs`）

| 锁 | 断言 | 补的是什么盲 |
|---|---|---|
| 1 | 6 岛 verdict==PARTIAL；5 岛 reason 含「零引用」+ADR-266；nexus-hook reason 含 ADR-258+`register_all`；无残留 DECORATIVE 行 | 防把共享计数抬升误读为已接线 |
| 2 | 无 `-> nexus-hook`；数据行==5；说明段含「**5 条**」；无「已按 DECORATIVE 登记」 | 豁免不得比债务活得久 + 计数诚实 |
| 3 | probes 不含「全库零生产构造点」「生产装配尚未构造」，且含 `CbmrWriter::open`+ADR-259 | **ADR-259 五锁的覆盖面盲区** |
| 4 | exec_plan 不含 ADR-257、含 `c6eb9b62`，且 `op_timeout_is_counted_as_failed_node` 与 `failed_old` 仍在 | 溯源正确 + 防订正时顺手删掉行为锁与反向对照 |

路径一律以 `CARGO_MANIFEST_DIR` 起算（ADR-259 §9.4 missing-11 纪律）。

### D5：防再发条款

**任何把 optional 内部 dep 从「零引用」变为「有 src 引用」的接线批，必须在同一 commit 内完成四件**：
① 代码；② 测试；③ **两张台账**（`gated_wiring_baseline.txt` 的 verdict 旋转 +
`declared_dep_exceptions.txt` 的豁免回收）；④ ADR。
只落 ①② 会让 G-56/G-57 与 G-31 在**下一次跑门时**才红，
而那时改动已进历史、归因成本远高于同批处理。

**共享 enabler 的额外义务**：若接线批只真实接通了共享 feature 下的**部分**岛，
台账 reason 必须逐岛写明「真实接线」还是「共享计数抬升」，
不得用一句统一理由覆盖 6 行（那会让 5 个未接线的岛看起来已接线）。

## 3. 验收（现跑）

| 判据 | 结果 |
|---|---|
| `py -3 scripts/check_gated_wiring_substance.py --selftest` | rc=0 |
| `py -3 scripts/check_gated_wiring_substance.py` | `[OK] gating substance matches the ledger`，rc=0 |
| `py -3 scripts/check_declared_dep_usage.py` | `[OK] every declared internal dependency is referenced by src/ (5 exception(s) honoured)`，rc=0 |
| `cargo test -p chimera-cli --test ledger_parity_locks` | **4 passed / 0 failed**（`gated_wiring_ledger_rotated_with_provenance`、`nexus_hook_dep_exception_reclaimed_and_count_honest`、`probes_storage_note_matches_wired_write_face`、`decorator_order_provenance_points_at_real_commit`） |
| `cargo test -p chimera-cli --test panorama_adjudication_locks -- --test-threads=1` | **5 passed / 0 failed**（ADR-259 五锁未受影响，含 `u08_session_store_note_parity`） |
| `rustfmt --edition 2021 --check`（本批 3 个 Rust 文件） | 全部 fmt-clean（逐文件核验，未做整包 fmt，避免顺手重排他人代码） |

工具链环境注记：本会话 `cargo` 初不在 PATH，按项目规范注入
`CARGO_HOME`/`RUSTUP_HOME`/`TMP`/`TEMP` 与 `.toolchain/cargo/bin` + `mingw64/bin`
后 `cargo 1.97.1` 可用；门脚本一律 `py -3`（`python` 不在 PATH，rc=127）。

## 4. 回滚

单 commit revert 即回原状（台账 + 注释 + 文本锁同批）。
revert 后两道门回到 rc=1 —— 那是**已知的真实红**，不是本批引入的回归；
若需临时安静，正确做法是回滚整个 `06581d15` 接线批，而不是只回滚台账。

## 5. 未采纳的替代

| 替代 | 否决理由 |
|---|---|
| 手填 verdict 而不跑 `--emit` | verdict 必须来自机器读数；手填会把「共享计数抬升」与「真实接线」混为一谈 |
| 给 6 岛写同一条统一 reason | 掩盖 5 岛仍未接线的事实，违 D5 |
| 保留 nexus-hook 豁免行并改其理由 | 该表规则明确：豁免不得比债务活得久；src 已真实引用 ⇒ 无 gap 可豁免 ⇒ 必删 |
| 只改 session-store 的注记（沿 ADR-259 做法） | probes 那份陈述无门可抓，会继续误导规划；故本批补锁 3 |
| 顺手把 5 岛的 optional 边删掉「消除债务」 | 会同时抽掉 B3-B4 接线占位与 ADR-160 可达性棘轮的既有账；退役权威是 ADR-188，本批无权 |
