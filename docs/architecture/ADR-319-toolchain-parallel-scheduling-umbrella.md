# ADR-319: 工具链并行调度重构伞决策（W2~W7 批次群）

- 状态: Accepted（分批实施中：W3b/W3c/W5/W6/W4 已落地，W2 待 ADR-316 门槛）
- 日期: 2026-11-16
- 批次: 工具链并行调度重构（第一卷方案 TL-W* 批次群）
- 设计源: 《Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md》工具链相关要求（T1）+ 全工具链八域取证（第一卷 §2.2/§2.3）
- 关联: ADR-315（W-T0/W-T1：清单域化 + govd exclusive 互斥链，本伞的底座）、ADR-316（govd 从 shadow 提为阻塞执行面——本伞 W2 批的决策归属，本文件不重复记载）、ADR-228 §3.6（翻转门槛）、ADR-225 D1（资源性故障与回归分离判定）、ADR-304（测量纪律）

> **编号沿革（撞号记录）**：本伞批次群在实施期间（两轮提交
> `56cc4f14`~`82fdafb6`）的代码注释、门台账与收口报告曾引用
> **ADR-318** 作为本伞编号。2026-11-16 复核发现
> `docs/architecture/adr_index.md` 已登记 ADR-318 =「`edition.workspace`
> 一致性收口」（在途线实施批，索引行为已提交内容）。按「索引为编号权威」
> 与「先登记者优先」裁决：**318 权属让渡在途线**，本伞改号 **319**；
> 全部可改引用在改号批同批更正（`scripts/govd/executor.py`/
> `policy.toml`/`check_manifest_schema.py`/两个 govd 测试文件/第二轮收口报告/
> v3.4.0 文档注记），不可改的两轮 commit message 由本节承载史实。

---

## D1 — 已落地批事实表（截至 2026-11-16，HEAD=`82fdafb6`）

| 批次 | 提交 | 内容 | 验证摘要 |
|---|---|---|---|
| W3b | `56cc4f14` | `scripts/ws_snapshot.py`：workspace 知识统一快照（指纹失配无损重建） | selftest 14 断言；真实树 47 成员/99 依赖键 |
| W3c | `387b5825` | govd `cargo_semaphore=1` 应用层互斥（finally 释放/超时 UNKNOWN）+ schema EXEC_KEYS 扩展 | 11 新测 + govd 90 零回归 + schema 40 断言 |
| W5 | `89abf21a` | `scripts/audit_cmd.toml` 机器真值源（ADR-167 决策 4 的 CI 等价性缺口根治）+ `.gitattributes` 补 `*.toml text eol=lf` | audit selftest 12 断言；--check-ci 真树逐字等价 |
| W6 | `d2c77014` | `check_coe_observability.py`（COE 观察期机器化，漏翻转才红）+ 文档登记 | selftest 13 断言（含 gh 缺席 rc=2 实测） |
| W4+N1+N2 | `82fdafb6` | bench_check 475 行 heredoc 根除（阈值单源化，P9 病灶根治）+ compare verdict-json + cron 02:30 + release test→nextest + fmt 漂移收敛 + cargo 间接调用探测 | perf static 79 断言；I7 58 对绿；actionlint 双 OK |
| T0 | `3d7d0f08` | check_perf_redlines 四红修复（I8 路径/I5 floor/ignored 台账随册） | static+selftest 全绿；恢复 check job Perf redline 门 |

## D2 — 调度架构核心决策

1. **清单喂料，不重写调度器**（承袭 ADR-315 路线 C）：gate_manifest.toml 是唯一权威面，govd 消费清单。
2. **互斥的唯一机制是 `exclusive`**；W3c 的 `cargo_semaphore` 是与 `.cargo/config.toml jobs=4` 互补的进程级双保险（防「多门 × 多 rustc」链接峰值内存叠加），first-red-line 为 finally 无泄漏。
3. **阈值单源化**：bench_check.yml heredoc 表体已物理删除（`82fdafb6`），唯一真值源 = `scripts/perf_thresholds.toml`；heredoc 删除的量化出口 = compare 步 `CHIMERA_PERF_VERDICT_JSON` 连续 5 轮逐键一致。
4. **门禁三态 0/1/2 不变**（2=不可判定≠通过）；COE 类门「漏翻转才红」。
5. **并发度规则**：本地静态门 jobs≤4（内存墙实测 16 并发→0GB 前科）；CI 2 核 runner `--build-jobs 2` 显式覆盖；slo 档 test-threads=1 为硬约束。

## D3 — 未落地与等待项

- **W2（govd 翻转 CI 阻塞）**：决策归属 ADR-316；前置 =「W-T1 连续 7 日绿」（ADR-228 §3.6），需 gh 凭据核验，不满足不绕。
- **W4b**：mca-feature 三次编译压缩为两次（删首步 check），需一轮 CI 对照验证覆盖面不缩水后执行。
- **W7**：收益测量报告（§4.6 六指标基线/现值对照），依赖 CI 侧数据回填。
- **W3a**：gate_manifest `needs` 字段删除——施工规格书已备（`tmp/w3a_needs_removal_spec.md`），前置 = 在途 govd 测试线落地。

## D4 — 回滚

逐 commit `git revert`（上表各批均为独立 commit）；W4a 的 heredoc 可从 git 历史恢复；semaphore 与探针均有 env/配置显式关断路径（`cargo_semaphore=false` / 探针默认关）。
