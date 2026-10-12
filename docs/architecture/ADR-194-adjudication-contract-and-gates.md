# ADR-194：裁决权分离契约事件 + Certify 白名单门 + 写入敏感扫描门（v2.3 Wave-1 落地治理）

- **状态**：✅ Accepted（2026-09-29）
- **决策者**：E01（架构）+ E02（安全）+ E03（记忆）+ E08（度量）
- **影响面**：
  - `crates/event-bus/src/{types.rs,registry.rs,bus.rs,critical_sink.rs}`、`crates/event-bus/tests/{variant_count_test.rs,credit_flow_test.rs}`
  - `crates/nexus-contracts/src/{adjudication.rs,sensitive.rs,price_vector.rs,calibration.rs}`
  - `crates/mlc-engine/src/{types.rs,mind_model.rs,observation.rs,l3_procedural.rs}`
  - `crates/{session-store/src/segment.rs, repo-wiki/src/{store.rs,error.rs}, quest-engine/src/checkpoint.rs, scc-cache/src/wal.rs, cmt-tiering/src/{warm.rs,cold.rs,ice.rs}, mcp-mesh/src/quantum/wal.rs}`
  - `crates/parliament/src/{adversarial_review.rs,debate/mod.rs}`、`crates/chimera-tui/tests/event_coverage_drift_test.rs`
  - `scripts/{check_certify_whitelist.py,check_write_sensitive_instrumentation.py,check_perf_redlines.py(I8b),gate_manifest.toml(G-83/G-84),event_ownership_baseline.txt,doc_count_freeze.txt}`、`.github/workflows/ci.yml`
- **承袭**：ADR-033（L0 契约层）、ADR-042/053（R2 冻结/影子，本 ADR 不触碰 RL）、ADR-187（内环 α′）、ADR-191（Critical 持久化/背压契约）、ADR-192（MLC L2 分层检索）

## 1. 背景与判据

`Chimera架构重构与优化方案_v2.3` §(b.1)/(b.2)/(b.5) 要求三项机制：
1. **裁决权外置**：agent 不宣布自身成功；Claim（宣布）与 Certify（验证器确认）分通道、分权限。
2. **失败登记一等化**：`FailureRegistry` 走 Critical 旁路（丢失一条 = 全 swarm 重复踩坑）。
3. **写入敏感扫描**：记忆/上下文/会话持久化前拦截私钥/凭证/令牌，无敏感扫描是 v2.3 核验实锤缺口。

仓库红线要求「新增变体/门必须 ADR 立案 + 同步治理锁」。本 ADR 为上述 Wave-1 落地补齐授权与决策记录。

## 2. 决策

- **D1 事件契约（5 变体，六锁同步）**：`NexusEvent` 新增 `ClaimIssued`/`CertifyIssued`/`ClaimRetracted`/`FailureRegistered`/`BlackboardEntry`。`CertifyIssued`/`FailureRegistered` = Critical 且走 mpsc 旁路（`CRITICAL_MPSC_VARIANTS 14→16`、`CRITICAL_TOTAL 18→20`、`LANE_FORBIDDEN_SHARD` +2、`is_critical_mpsc_event` +2）；`ClaimIssued`/`ClaimRetracted`/`BlackboardEntry` = broadcast Normal。同批同步 `variant_count_test` 三层锁（151 计数 / 20-11-120 分布 / 序数表）、`registry.rs` 显式定级、`doc_count_freeze`(151)、`event_ownership_baseline`（无生产者项带证据登记）、TUI `event_coverage_drift_test` 分类臂。
- **D2 权限落地（编译期不可得的替代）**：`publish_claim`/`publish_certify`/`publish_declaim` 语义 API + Certify 白名单静态门（G-83）+ `AdversarialReview` 非参与者审查状态机 + `Parliament::certify_if_reviewed`（审查通过才 `publish_certify`）。
- **D3 写入敏感扫描（引擎 + 8-写点 + 覆盖门）**：`nexus-contracts/sensitive.rs` L0 无-regex 纯函数 `scan`/`redact`/`ScanPolicy`（放 L0 令 L2–L10 写点均可向下依赖）；§(b.5) 8-写点表全量插桩（mlc/session-store/repo-wiki/quest-checkpoint/scc/cmt×3/mcp-mesh/event-bus-critical_sink，采 Block-only 高熵/键名设 Pass 防误报）；覆盖门 G-84 核验插桩在位。
- **D4 记忆二维（§(b.3)）**：`MemoryEntry` 追加正交 `epistemic_type`（+`evidence_ptrs`/`provenance`，全 `#[serde(default)]` 非破坏）；`mind_model.rs` O(1) 预计算答案 + tag 失效；`observation.rs` 归纳/原始分库 + 来源反查；`calibration.rs` 人机一致率锚点（<900‰ 降级为 Claim）。
- **D5 I8b（§(f) 新红线）**：扩 I8 正则覆盖 `LANE_FORBIDDEN_SHARD`+计数行，清历史 `17→18→20` stale 注释，令事件计数漂移不可复现。

## 3. 后果与代价（诚实登记）

- 写入扫描在已接写点采 **Block-only**（拒绝含硬标记载荷落盘），未做逐点 `redact` 打码（`redact` 原语已具备，接线按点在后续）；session-store 采拒整条而非脱敏，避免破坏 JSONL/round-trip。
- `Observation`/`MindModel` 为可测数据结构；`MlcEngine::recall` 按 `epistemic_type` 过滤已由 `partition_by_epistemic`+`recall_hybrid` 认识路实现；`/bench`+审查队列+黑板浏览面板、认领制 feature-gated 试点、tier-2（经逐文件证据甄别：mca-gateway 会话轮次为唯一真实敏感文本面已插桩，其余 R3-1 候选为 numeric/test-only/binary 非敏感面，不强插防伪报）已落地（**⚠ 该甄别结论已被推翻**，见 §3.2 更正）；价格向量成本感知路由（二期）仍属 Wave 4 触发条件。
- `certify_if_reviewed` 走仓库既有 default-off + injector 范式，**不改 `Parliament::new` 默认**（向后兼容）；`BlackboardEntry` 黑板持久化**转正**（L1→L2 沉降压测）仍属 Wave 3 待运行条件。

- 不减二进制、不加快启动；RL 面（omega-learner）维持 R2 冻结影子不动。

### 3.1 遗留决策定稿（v2.3 §(g)，2026-09-30 用户确认）

- **§(g)#2 FileWalCriticalSink**：**保持默认关（仅 env `CHIMERA_CRITICAL_WAL_PATH` 启用）+ 不加显式 fsync，现状即终态**。判据：Critical **必达**由 mpsc 旁路（4096 有界、零丢失经慢消费者混沌测证实）保证；WAL 持久化定位为尽力而为的审计补充，强 fsync 属未验证的性能回退风险。如未来出现强持久合规需求，另立 ADR 重开。
- **§(g)#4 认领制转正边界**：**保持 `claim_pilot` feature-gated（默认 off），现状即终态**。转正/回滚双判据以 `ClaimBroker::rollback_verdict` 代码为唯一事实源（distinct served peer > 1 且 频率护栏拒绝占比 < 50% ⇒ Proceed；否则 Rollback 回派单）；转正启动需**真实多消费者需求**作为外部触发条件，未出现前不接 chimera-cli 组合根（无病灶不建全量分发面）。

### 3.2 更正：tier-2 甄别结论被实勘推翻（v3.4 W5-1 R-1，2026-10-08）

**原结论（§3 “tier-2 已落地”括号内）**：“mca-gateway 会话轮次为唯一真实敏感文本面”。

**现修正为**：全量持久面实勘（大小写敏感全信号集，`crates/*/src/**/*.rs`，排除 tests/benches/examples）得到
**三处**含用户/模型可控自由文本且**未插桩**的落盘面，已于本批全部接入 G-84（清单 11→14）：

| 落点 | 自由文本来源 | 插桩后语义 |
|---|---|---|
| `crates/cmt-tiering/src/experience_card_storage.rs` | `token_evidence.payload` = `ToolCallRecord.arguments`/`.result`（工具入参与执行结果原文） | 命中即**逐条跳过**（本表无事务包裹，部分落盘是既有语义） |
| `crates/chimera-tui/src/types.rs` | `TuiState::save_to_file` 序列化**整只**状态（含 `input_history`/`quest_list`/`latest_events` 等无 `serde(skip)` 的运行字段） | 命中即**整份拒写**（保留既有文件） |
| `crates/chimera-tui/src/data/snapshot.rs` | `/export` CSV 落 `Quest.title`、JSON 落整只 `quest_list`（含 `Task.description`） | 命中即拒写导出文件 |

同类盲区另有一族：**入口级**。`crates/repo-wiki/src/store.rs` 全文仅 2 处 `scan`，却有 5 个 `pub` 写入口；
本批已补齐 `insert_with_contradiction_check` 与 `insert_distilled_insight` 两处（同一 `WikiEntry` / 自然语言正文），
并把四处策略抽为单点 `wiki_scan_policy()`（与抽取前逐字等价）。

**误判根因（可复现）**：大小写不敏感扫描会把 `DecayProfile::`/`RoleProfile::` 计入 `File::`、
把 `RwLock::write()` 计入 `.write(`，将真实盘面误归为 numeric/binary。原甄别如果用了同类扫描，即可解释该结论。

**未推翻部分**：Block-only 不逐点 `redact`（§3 首条）仍成立；条件豁免与表级豁免及其证据链已记入
`scripts/check_write_sensitive_instrumentation.py` 注释区（tree.rs/fts.rs/attribution.rs/quest-engine wal.rs 等）。

**仍待 ADR**：把 G-84 从「文件级字符串存在性」升格为「入口级断言」（清单条目携带函数名）——否则同文件内
新增 `pub` 写入口仍可无声绕过。本轮不动门语义（全仓普查 + 判据重写成本/收益不匹配）。

### 3.3 追加更正：§3.2 尾段「仍待 ADR」已在本批以本节承载（v3.5 P-5，2026-10-10）

**结论**：G-84 自本批起为**双层判据**，不再需要另立新 ADR——§3.2 尾段所指的「ADR 动作」就是本节本身
（追加注记、原文不改，沿 ADR append-only 纪律）。授权来由：v3.5 Spec《需裁决事项》第 6 条对 P-5 的
**默认值即「采纳入口级断言」**，且用户已批准该 Spec；本批开工时曾把它误登为「需 ADR 项」，
错在我把 §3.2/门备注当成了事实源而未回 Spec 取默认值（教训：登记面不得反过来裁决事实面）。

**使该升级成为必需的证据（非推测）**：§3.2 尾段预言的失效形状已真实发生——同一文件内
`insert` 有守卫而 `insert_batch` 没有，共三处：`mlc-engine/src/l3_procedural.rs`、
`cmt-tiering/src/warm.rs`、`cmt-tiering/src/cold.rs`。旧文件级判据对三者均给绿；且
`cmt-tiering/src/coordinator.rs:560/609` 的衰减降级腿**生产态调用该批量入口**，旁路真实可达。
已于同批补齐（整批拒绝，与单条入口同语义），并各有双向测与三个变异体（守卫短路 ⇒ 测必翻红）实证。

**新判据形态**：[F] 文件级 MARKER 存在性 + [E] 每个「可携带自由文本的 `pub*` 写入口」必自带
`sensitive::scan` 或在 `ENTRY_EXEMPT` 带证据豁免（11 条：1 条 UPSTREAM-GUARDED 由门机械核验委托关系，
10 条 STRUCTURAL-WRITE 理由必须指到具体 SQL/列清单）。花括号失衡/承诺文件缺失/豁免表格式错
一律退 2（不可判定），不得静默放行。入口口径取 `pub*`：私有 `writer_*` 是扫描下游
（实测计入会把 repo-wiki 7 处已扫后落盘的正常架构误判为缺口）。门自身有牙的验证已按成对登记
惯例入门（G-92）并接 `ci.yml`；门清单 91→92（light 84→85）。

**本节的未解决面**（与 §3.2「未推翻部分」并列，不得读作已闭合）：
① 签名不携带文本面的新 `pub` 写入口（如 `fn put(id: u64)` 内部拼原文）仍不可见；
② `STRUCTURAL-WRITE` 的「不携带原文」是人对 SQL 的判定，静态判据看不到字段语义未来的变化，
契约变更必须人工复审该表；③ `ScanPolicy` 硬载荷口径仅做到了 **crate 内单源**
（cmt-tiering 曾有 5 份逐字拷贝，现收为 `hard_payload_scan_policy()`），**跨 crate 副本仍多源**，
将其上收到 L0 契约库属 nexus-contracts 职责面变动，本轮不做。

## 4. 验收

event-bus 全测 0 failed、各受影响 crate 测试 0 failed、`cargo check --workspace --all-targets` rc=0、`check_perf_redlines invariants`（I8/I8b）rc=0、`check_event_ownership` rc=0、`audit_gate_parity`/`run_gate_manifest --selftest`/`audit_spec_coverage` rc=0、`check_write_sensitive_instrumentation` 10/10 rc=0、`certify_if_reviewed` e2e 测绿、MLC-2D（字段/MindModel/Observation）测绿、calibration 3 测绿、全部变更 rustfmt `--check` rc=0。
