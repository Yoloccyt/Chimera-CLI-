# ADR-224: L0-L10 全层重构 R1 整改批次收口——D1~D7 裁决、破坏性面登记、M32 例外退役与 NexusError closed→open 重分类

> **Status**: ✅ Accepted（2026-10-25，业主批准《L0-L10 全层重构技术方案（R1，R2 修订版，R3 复核定稿）》`tmp/l0l10-r1-plan.md`；实施轮 W0~W4 变更单按图执行，本 ADR 为批次收口落档，未竟事项全量转入 §7 待裁议程）
> **决策日期**: 2026-10-25
> **关联 ADR**: ADR-214~223（L0 专项十轮裁决，全部**继承不重开**，本批次裁决不触碰其已裁结论）/ ADR-222（L0-A/L0-B 双亚层，本批全层审计的 L0 分层口径）/ ADR-215 R-S1/R-S2（可扩展性分级，§5 重分类的判据来源）/ ADR-185 D4（向下边例外收编先例，§4 M32 的依据）/ ADR-221 F-7（事件量纲演化先例，D2 的依据）/ ADR-002（DecayEngine 惩罚语义，D6/Δ6 依据）/ ADR-141（session-store 会话事件流）/ ADR-065 决策 4（流式数据面 bounded mpsc 不进 event-bus，C-12 的精神源头）/ ADR-053 rev4 + ADR-054（R2 冻结期与 RL 闸门，§7 多项议程的前置约束）/ ADR-061（v3-engine 双轨，D5 重授权对象）
> **关联交付**: 施工图 `tmp/l0l10-r1-plan.md`（变更单 C-01..C-56；tmp/ 不入库，历史依据以 git log 各单提交信息与本 ADR 为准）
> **权威源**: 实施结果以 git log 各单提交与工作树现取为准；事件定级/常量以 `crates/event-bus/src/{registry.rs,bus.rs}` 现取为准；错误契约可扩展性台账以 `scripts/l0_extensibility_class_freeze.txt` 现取为准；契约头与架构视图以 `scripts/crate_contract.py --check` 与 `emit_architecture_views.py --verify` 现取为准
> **编号合法性**: 落盘前现取 ADR 物理文件 max **223**（`ls docs/architecture/ | grep -E '^ADR-22[0-9]'` → 220~223 四件，2026-10-25 亲验）；库内既有 `ADR-224` 字样全部为本批实施单注记的**预留挂点**（如 `crates/pvl-layer/src/concurrency.rs` 装配态自审注记三处），无冲突占用。门号新增 **G-133/G-134**（C-05 落地，落盘前 max G-132）。事件变体**零新增**（D2 仅改既有变体定级；常量现取 `CRITICAL_MPSC_VARIANTS=17`、`CRITICAL_TOTAL=21`，全量计数以 `variant_count_test` 三重锁与 `doc_count_freeze.txt` 为准）

---

## 1. 背景

登记册 `tmp/l0l10-r1-issues.md`（open 84 条）经 R1 全层重审 + 两轮红队评审收敛为 56 张变更单（C-01..C-56，施工图 §3），覆盖 46 crate 全层（L0 按 ADR-222 双亚层口径）。既有 ADR-214~223 十轮 L0 专项裁决默认继承不重开，本批新裁决按施工图 §0.1 收口到**一份 ADR-224**（即本文）。本批的病灶形状高度一致：**门在、但承诺面与投递面/装配面脱节**（契约头派生器系统性虚报 M03、ShadowBreakerTripped 定级与旁路 API 失配 M09、unbounded 通道 M05、零装配死模块 M34、DORMANT 装配态 M22 等），故整改以「诚实化 + 显性化 + 有界化」为主，不含任何新特性。

## 2. 决策 D1~D7（含被否反方案与落地状态）

### D1（M22）session-store 收口 = 组合根装配激活（承载单 C-29）

- **决策**：chimera-cli composition 构造 `SessionStore` 经 `AppServer::with_session_store` 构造参数注入，生产双写启用；装配态订正块 DORMANT → PRODUCTION-WIRED。依据 `nexus-app-server/Cargo.toml` 在码设计注记「用构造参数而非 feature 标志控制灰度」。
- **被否反方案 ①**：降 optional 消死重——optional=feature 门控，与在码设计决策正面冲突，且为一条已在册生产边制造 G-09 棘轮扰动。**被否反方案 ②**：维持现状等产品裁决——DORMANT 自陈「收口需 ADR」，全层重审正是收口时机。
- **落地状态**：**本轮阻塞（C-29 未落地，工作树无施工痕迹），转入 §7.2 议程**；决策本身维持，待后续批次执行。

### D2（M09）ShadowBreakerTripped 定级升级 Critical+mpsc（承载单 C-13）

- **决策**：全套同步升级（常量/旁路臂/LANE 名单/L0 ImmutableSurface/测试三锁+字面断言/三构造夹具/I8c 机锁散文/双清单守护散文/G-117 note 八组，**单一提交**缺一即红）；两生产方调用点不动。机制表述：gsoe 侧走 `publish_critical`（无条件 `send_critical_mpsc` + broadcast），decay 侧走 `publish_blocking` → `dispatch_one_impl` 按分类路由——升级把「API 决定旁路」的失配收敛为「分类决定旁路」的一致语义，broadcast 双通道不减。
- **被否反方案 ①**：gsoe 改走 `publish()` 保 Normal——与 fail-closed 熔断留痕语义冲突，投递保障弱于同门 FormalVerificationFailed。**被否反方案 ②**：维持现状——两生产方语义永久分裂，G-117/118「承诺面/投递面对拍」精神持续违背。
- **落地状态**：已落地（提交 `45cbb34`）。现取锚点：`registry.rs` `ShadowBreakerTripped => Critical, Security`；`bus.rs` `CRITICAL_MPSC_VARIANTS=17`、`CRITICAL_TOTAL=21`。

### D3（M03）契约头派生器修复语义 vs 登记「方言口径」（承载单 C-03）

- **决策**：修复 `scripts/crate_contract.py` 事件收集循环（复用 G-99 词法器家族 `blank_rust` + `test_tail_mask` 做注释/字符串剥离与 cfg(test) 尾遮蔽；模式位识别不计构造；补三类反例夹具测试），同批 `--install` 重派生 46 头 + 架构视图重派生，人工逐 crate 复核 diff。
- **被否反方案**：方言口径（登记「PRODUCERS 含测试构造/模式行」）——头字段承诺是「constructed in src/」生产语义，方言化=把撒谎制度化；且 G-70 视图会把失真派生传播进 `layer_contracts.txt`。
- **落地状态**：已落地（提交 `7ef3564`）。

### D4（M33）MTPE 真实预测 = 登记 DEFERRED，不接线

- **决策**：契约头已含 DEFERRED 且与代码内注记一致；接线前置条件（with_model 组合根装配、编码器复用、sync-offload 纳 G-88、双 MtpeConfig 合一）登记 §7.1 M33 议程。
- **被否反方案**：本轮接线——with_model 全 workspace 零调用、生产构造为零；模型工件链路缺席；R2 冻结 + RL 闸门节奏；给死码包 spawn_blocking 是纯 churn。
- **落地状态**：按裁决登记待裁（无变更单，符合 R2 冻结红线）。

### D5（L29）v3-engine 双轨 = 明文重授权窗口，不立即移除回退

- **决策**：**本 ADR 重授权兼容窗口——到期锚 v3.1.0-omega 或 2026-12-31 先到者**，到期另立收口裁决单（届时须有全面板等价性验证证据方可移除回退与逃生舱）；`docs/tui/README.md` 现状失实描述的订正由 C-54 承载。
- **被否反方案**：本轮移除回退与逃生舱——无等价性验证证据；删除不可逆；逃生舱是泄压阀，无替换观测手段前不拆。
- **落地状态**：窗口裁决由本 ADR 记录生效；C-54 在 W5 队列（见 §6）。

### D6（M18）perf 阈值 matched=0 = 默认 advisory + `--strict` 判红（承载单 C-07）

- **决策**：`check_perf_redlines.py` thresholds 模式对零匹配键打 `[SKIP-GHOST]` advisory（非致命）；`--strict` 计入 fails；CI bench_check 不加 --strict，零阻塞面变化。
- **被否反方案**：无条件判红——全红会重蹈「门禁噪声→被迫放宽整体门禁」覆辙（AGENTS §7.2 前车）。
- **落地状态**：已落地（提交 `fbb03b2`）。

### D7（L24）NexusError 加变体 + 台账 closed→open 重分类（承载单 C-23）

- **决策**：在 `nexus-contracts-safety/src/safety/errors.rs` 的 `NexusError`（L0-A 契约，serde derive，协议面 `AppEvent::Error{code}` 取变体名）新增 `SwarmCapacityExceeded { active, limit }`；同提交完成 `Recoverable` 穷尽 match 补臂（FailFast）、wire_roundtrip 代表集补新变体、`nexus-subagent/runtime.rs` 满载路径改返回新变体（与真嵌套可区分）、台账重分类（详见 §5）。
- **被否反方案 ①**：复用现有变体表达满载——10 变体无一契合「容量满」语义（EventQuotaExceeded 专指动态事件命名空间配额，误用会污染其恢复策略语义与协议 code 面）。**被否反方案 ②**：改 spawn 签名返回 crate 内 `SubAgentError`——波及全部调用方，破坏面大于加变体，且无协议面。**降级路径**：重分类被否则整体转待裁，禁止在 closed 类上静默加变体（重分类已获记录，见 §5）。
- **落地状态**：已落地（提交 `22e2a6c`；台账行 reason 字段现引本 ADR）。

## 3. 破坏性面显式记录（M05 / M09 / M22 / M10 / L36 / L24）

> 以下六项是施工图明文要求的破坏性面登记。除 M22 外均已落地；wire 协议格式**零变化**（M09/L24 变更均在单二进制 workspace 内自洽，无跨版本 wire 实害）。

| 来源 | 破坏性面 | 兼容性论证 | 落地状态 |
|---|---|---|---|
| **M05**（C-12） | ExperienceCardBus 订阅签名：`UnboundedSender/UnboundedReceiver` → `Sender/Receiver`（workspace 内 pub API 签名变更） | 唯一调用方 `chimera-cli/src/experience_loop.rs` 随提交适配；wire 零变化；洪峰从无界内存变 4096 硬上限 + 饱和丢弃计数 + 节流 warn | 已落地 `b7cc753` |
| **M09**（C-13） | `NexusEvent::ShadowBreakerTripped` 定级 Normal → Critical：对穷尽 match severity 的消费方为行为变更；mpsc 旁路集 +1（常量 16→17、20→21，现取亲验） | 投递纯增益（mpsc 保障 + broadcast 双通道不减）；八组清单同一提交；TUI decay 面板经 latest_events 消费不受影响；wire 格式零变化 | 已落地 `45cbb34` |
| **M22**（C-29/D1） | 生产行为新增：会话双写落盘（每 turn 一次段文件追加） | 行为级新增非 API 破坏；回滚=revert（已落盘段文件无 schema 依赖）；无新增依赖边（生产边已在册） | **本轮阻塞未实施，转 §7.2** |
| **M10**（C-18） | `DecayError` +1 变体 `InvalidSeverity`（穷尽 match 适配面）；负/非有限 severity 由「惩罚变奖励（权限提升面）」改为拒绝 | 破坏限于「依赖负 severity 提权」——该行为即缺陷，无合法迁移；M2 e2e severity=1.0 不受影响 | 已落地 `5904287` |
| **L36**（C-17） | 删除 event-bus pub API `log_publish_with_size`（workspace 内删除 pub API=破坏性） | 全仓零调用方（施工图亲验），无迁移；消除「误接热路径即整事件堆分配」脚枪；ADR-223 §6「281 pub fn 不入门」口径不变（281→280 仅事实记录，以 G-105/106 棘轮现取为准） | 已落地 `e986edb` |
| **L24**（C-23/D7） | L0-A 契约类型 + serde/wire 面：`NexusError` +1 变体（旧读者反序列化不识别新 code） | 单二进制 workspace 无跨版本 wire 实害；迁移机械（穷尽 match 由编译器全暴露）；**禁止在 closed 类上静默加变体**——重分类经 §5 显式记录 | 已落地 `22e2a6c` |

## 4. M32 例外退役记录（承载单 C-09）

- **裁决**：退役 `check_dependency_rules.{sh,ps1}` 双源 ADR 例外表中 `pvl-layer(L7) → seccore(L4)` 条目。理由：seccore 自 2026-08-18（提交 `d710699`）起已是 pvl-layer 正式生产依赖，旧注记「feature 门控」失实；Check B 只 flag 向上依赖，该向下边（L7→L4）无豁免天然合法——例外条目是 no-op 安全垫，保留反而误导后人。收编先例 = ADR-185 D4（gqep-executor→qeep-protocol 条目同法退役）。例外表**机制保留**：未来真需豁免的向上边仍须 ADR 记录特批再入表。
- **落地状态**：**本轮阻塞（C-09 未提交落地）**。2026-10-25 工作树观察：双源施工内容已在盘（`.ps1`/`.sh` 例外表清空 + 失实注记更正，diff 注释自标「C-09/M32, 2026-10-25」），但未提交 = 未收口，双源四模式验证与提交归后续裁决。**转入 §7.2 议程**。

## 5. NexusError closed→open 重分类记录（C-23/D7，随本 ADR）

- **对象**：`scripts/l0_extensibility_class_freeze.txt` 中 `ENUM | type=safety/errors::NexusError` 行，`class` 由 `closed` 改为 `open`（行号会漂移，以 grep `type=safety/errors::NexusError` 现取为准；同提交 `22e2a6c` 完成，台账行 reason 字段引本 ADR）。
- **理由**：跨层统一错误契约属 **ADR-215 R-S1「演化型边界载体」**，closed 类（R-S2「定型 value 类型」）对错误契约属错位分类；该枚举变体本就随 WI 演进追加（如 WI-21 `EventQuotaExceeded`）与本批 C-23 `SwarmCapacityExceeded`，历史行为与 closed 定位不符。
- **边界**：本重分类为一次性记录性变更，**仅解除「closed 类禁改」与现实的错位**；后续对该枚举再加变体仍须走 ADR 记录（不豁免 AGENTS §2.3 决策纪律），协议面 `AppEvent::Error{code}` 值域变化须同步 wire_roundtrip 代表集。

## 6. 实施结果总账（W0~W4，2026-10-25 收口时点）

- **已落地 33 单**（git log 亲取，提交短哈希）：C-01 `d460c57`、C-02 `b3eccfc`、C-03 `7ef3564`、C-04 `79946da`、C-05 `a325e53`（G-133/G-134 双源门入册并接 ci.yml）、C-06 `af71a75`、C-07 `fbb03b2`、C-10 `4c6234b`、C-11 `d79d085`、C-12 `b7cc753`、C-13 `45cbb34`、C-14 `eb02c9e`、C-15 `c133997`、C-16 `82966b4`、C-17 `e986edb`、C-18 `5904287`、C-19 `143c0c7`、C-20 `b22ca58`、C-21 `76ca2f0`、C-22 `477bc6b`、C-23 `22e2a6c`、C-26 `941cdf1`、C-28 `7fbc5e2`、C-56 `13fd6b9`、C-30 `ecb524b`、C-32 `95f737d`、C-33 `2ea63da`、C-34 `93fd2eb`、C-35 `5789a26`、C-37 `eb8507e`、C-38 `54165c1`、C-39 `5c2155f`、C-40 `905958f`。
- **阻塞/跳过 8 单**：C-08、C-09、C-24、C-25、C-27、C-29、C-31、C-36——git log 无对应提交（2026-10-25 亲取），全部转入 §7.2 议程；其中 7 单的工作树存在**未提交施工内容**（观察记录见 §7.2 表注），最终落地与验证归后续裁决。
- **W5 在途**：C-41（本 ADR）先行落档；C-42..C-55 在 W5 队列，其中 C-54（D5 的 README 订正）与 C-43/C-47/C-48（adr_index 行）为文档面后续单。本总账不预写其结果。

## 7. 待裁议程（全量登记）

### 7.1 继承待裁台账（施工图 §5 全量转录，28 项）

> 「建议方向 / 前置触发条件」为施工图裁决时点的登记值；处置时以现场现取为准。**M27 为 R2 解冻前必清项（议程首位）**。

| # | 来源 | 摘要 | 建议方向 | 前置/触发条件 |
|---|---|---|---|---|
| 1 | **M27** | GSOE 三条生产腿断——**R2 解冻前必清项（议程首位）** | 按 R2 解冻评估窗口裁决 | R2 解冻评估窗口；基线已记账 |
| 2 | M08 | 4 个 Critical tripwire 事件零生产构造（mpsc 保障空转） | 补生产接线或显式裁决「契约预留」 | R2 解冻评估窗口 |
| 3 | M13 | loom 零落地（三态） | 补则 L2 hcw/mlc 两处持锁-广播场景首批；或作废愿景行 | Windows GNU 工具链约束 + dev-dep 预算 |
| 4 | M14 | bench 阈值覆盖未随增长 | 实测锚定后补键；无 SLO 诉求者明示豁免 | CI clean-runner 取数；含 L04 阈值键议题 |
| 5 | M16 | coverage 基线 46-crate 形态未重测 | nightly 出数后抬表；登记「拆分波次应附读数回写」 | 本地 llvm-cov E0463 不可代跑 |
| 6 | M19 | 六条 [[slo]] 无 CI 载体 | mode_slo 挂 slo-daily 或标注「本地工具」量纲 | A2 观测档排期 |
| 7 | M20 | 十六源 §9.3/§13 性能目标三态 | 100K×100 立项补 bench 或 ADR 作废；CLV CPU 批量档裁决 | 与 M14 同批取数 |
| 8 | M24 | chimera-mas INV-7/8/9 运行期强制点零接线 | 接线或 ADR 订正「已实现未装配」 | R2 解冻前必清清单 |
| 9 | M25 | quest-engine CheckpointWal 零调用、spec 清单已标 [x] | 倾向订正注记「可选储备模块未接线」+ .trae 清单订正 | 低成本可捎带 |
| 10 | M26 | scc-cache SqliteWal 零生产装配 | 接线（先裁崩溃保护必要性）或补订正注记 | 同上 |
| 11 | M28 | DECB 预算治理面生产空转 | 组合根接 StreamSessionCompleted→DecbCostCallback 或显式登记 | 与 C-29（§7.2）联动评估 |
| 12 | M29 | WikiUpdated 生产路径零触发 | 裁决「沉淀写入接入命令面」时机 | 产品裁决 |
| 13 | M30 | ISCM IscmAnchor/Layer 零跨层消费者 | 收缩 iscm.rs 为内部模块或补接线 | 不建议本轮改码 |
| 14 | M31 | nexus-hook GATED-UNWIRED（Noop sink） | 维持在制品债；接线须新 ADR；Noop→session-store 为前置 | C-29 落地后前置半边就绪 |
| 15 | M33 | MTPE 真实预测接线（D4 裁决登记 DEFERRED） | 前置=with_model 装配、编码器复用、sync-offload 纳 G-88、双 MtpeConfig 合一（L46） | R2/RL 节奏 |
| 16 | **M34（残留）** | VerifyConcurrency 接线/GATED/退役**三选一**（C-56 已显性化：模块头零装配自审注记 + 失实自述订正已落地 `13fd6b9`） | 接线须先立吞吐对拍基准证 2× 主张（并发度默认 4 的依据、与 PVL mpsc 主管线的关系一并裁决）；退役则连 lib.rs:84 导出行与 Cargo.toml 相关注释一并清理 | 与 PVL mpsc 主管线关系裁决 |
| 17 | **M36** | Memory Capsule v2.1 schema 三要素三态（L0 experience_card 无 schema_version/content_hash；L3 三 crate 零命中、等价载体 payload_version/source/parent_id；L5 四表无版本列） | 三态登记：是否把 schema_version/content_hash 提升为 nexus-contracts-memory 契约字段（BLAKE3 引入需新依赖，涉 G-99 白名单评估） | 手册设计落点 nexus-memory-pool 本仓不存在；GPU/RL 节奏外可独立评估 |
| 18 | M37 | pub API 签名第三方类型泄漏门缺失（L0-B tokio 暴露面不改码） | 中立「pub fn 签名 extern-crate 前缀白名单扫描门」，先 L0-B 试点 | 复用 G-105/106 抽取器；ADR-223 §6 预留轴 |
| 19 | L01 | L0-A 六 sibling 装配面均为 facade 单消费者 | v3.4/v3.5 让 ≥1 个 L1+ 消费者直连，或裁决 facade 单路径终态 | G-129 only-decrease 翻案需 ADR |
| 20 | L02 | G-123 台账零消费者豁免全部无理由 | wiring-or-prune 批次 | 不本轮机器强制 |
| 21 | L19 | DPO 训练对生成后即弃、DpoPairGenerated 零生产者 | (a) 辩论侧补发（RL 闸门后）或 (b) 退役删透传（涉事件字段须 ADR） | RL 闸门 |
| 22 | L21 | parliament 两个零装配模块 + crossbeam-deque 仅喂死模块 | 删模块或补自审注记 | G-123 轴面外 |
| 23 | L25 | join_next_typed JoinError 路径 nesting_guard 残留 | 快照对账或 in-flight 计数 | 外层 panic 罕见 |
| 24 | L37 | critical_channel_capacity/sample_critical_dropped_count 零调用不在 dead-contract 台账 | 接线批次同批登记 + 删陈旧注记 | 与 M31/M28 联动 |
| 25 | L38 | R2 关键词扫描器非 CI 门且仅覆盖 2 文件 | CI 化议题或裁决维持双腿现状 | R2 解冻前必复核 |
| 26 | L39 | cmt-tiering upsert_card 覆盖语义与审计不可变边界张力 | 登记语义边界；硬保证=审计行或收窄 INSERT OR IGNORE | 幂等容忍面裁决 |
| 27 | **L41** | 十六源 §12 六 trait 契约库零载体（contracts.rs 不存在、wgpu 0 文件） | 登记三态=未落地/空洞满足，随 GPU 议题冻结（ADR-195 D1 继承）；不为未裁决愿景出变更单 | GPU 议题解冻；签名层门缺口另见 M37 |
| 28 | L46 | 双 MtpeConfig 同名分裂（nexus-core vs mtpe-executor 字段集不一致） | 合一或补显式转换+对拍测试 | B3-B4 接线前置（low）；M33 前置 |

### 7.2 本轮阻塞/跳过单转入（8 项，2026-10-25 收口时点登记）

> 状态判据：git log 无对应实施提交（2026-10-25 亲取 `git log --oneline -70`）。表注中的工作树观察为 2026-10-25 `git diff` 事实，**未提交 = 未收口**，其内容不因本表登记而视为已落地。

| 单号 | 内容摘要 | 阻塞状态 | 转入议程后的裁决事项 |
|---|---|---|---|
| C-08（M21） | `check_declared_dep_usage.py` parse_declared 生产节优先 + `dep_edge_freeze.txt` 补登 2 条既有真实边（pvl-layer/quest-engine → nexus-contracts）+ 删两 Cargo.toml 冗余 dev 行 | 未提交落地 | 提交收口与验收（`check_dep_edge_freeze.py` live 自报、`check_outer_inner_ratchet.py` 基线复核、`check_declared_dep_usage.py` 分类复核）；G-32/33 冻结表补登前，「自报=树内」缺口维持已登记态 |
| C-09（M32） | 双源例外表退役 pvl→seccore no-op 条目 + 失实注记更正（裁决内容见 §4） | 未提交落地（施工内容已备工作树） | 提交收口 + 双源四模式验证；收口前 M32 例外条目按「待退役」理解，不得再据旧注记引用「feature 门控」失实表述 |
| C-24（B3 残余） | decb-governor `config.rs` validate 对 total_budget_limit 与四组因子补 `is_finite()` 拒绝 | 未提交落地（施工内容已备工作树，含测试） | 提交收口 + `cargo test -p decb-governor` |
| C-25（L07） | gqep-executor `futures = "0.3"` 内联 → `{ workspace = true }` | 未提交落地（施工内容已备工作树） | 提交收口 + `git diff Cargo.lock` 为空验收（防依赖漂移） |
| C-27（L11 注释半） | seccore `sandbox.rs` 超时分支补清理语义差异注释（gvisor kill_on_drop 差异） | 未提交落地（施工内容已备工作树） | 提交收口（纯注释级；L11 主体已按施工图 §6 否决，仅注释半留存） |
| C-29（M22/D1） | 组合根装配 session-store（D1 全文见 §2） | **未实施（工作树无施工痕迹）** | D1 决策维持；实施时按 C-29 原清单执行（composition 注入 + 装配态追加注 + CODE_WIKI 注记 + 集成测试），装配后联动评估 M28/M31 |
| C-31（L06） | nmc-encoder 三内联版本（tokenizers/mel_spec/hound）收口进根 workspace.dependencies 同值 | 未提交落地（施工内容已备工作树，含根表收口） | 提交收口 + `git diff Cargo.lock` 为空验收 |
| C-36（L33） | `scripts/measure_startup.ps1` 头注订正（阈值 40ms→100ms、四段→五段） | 未提交落地（施工内容已备工作树） | 提交收口（纯注释级） |

## 8. 验证与回滚

- **本 ADR 落档验证**：adr_index 链接核对——索引行文件名与磁盘实体 `ADR-224-l0l10-r1-remediation-batch.md` 一致；adr_index 三源对账脚本现取通过。各单验证命令见施工图 §8 与各单提交记录。
- **回滚**：本文档面回滚 = 删除本文件 + 删除 adr_index 对应行（两文件同批）；代码面回滚按施工图 §9.2 耦合组**整提交 revert**（C-13 事件全套 / C-08 派生器+冻结表 / C-03/C-45 派生链 / C-23 错误契约 / C-12 通道形态等耦合组禁止半 revert），任何 revert 后重跑施工图 §8.2 十二道门 + §8.3 受影响门全集 + `cargo check --workspace --all-targets`。
- **波次门（W5）**：G-53/G-51/G-37 + G-70 验收随 W5 其余单（C-42..C-55）收口时统一执行，本单不代跑、不预写结果。

---

## 10. R2 状态订正注（2026-10-26，承 ADR-225；历史正文只加注不改写）

> 本节是**追加**，不是对 §6/§7 的改写。§6 总账与 §7.2 表的登记时点是 2026-10-25 收口刻，
> 其自定判据就是「实施结果以 git log 各单提交与工作树现取为准」—— 既然 git log 已变，
> 该表必须可被后续现取推翻。下列逐条均附 2026-10-26 亲验命令读数；引用本 ADR 时以本节为准。

### 10.1 §7.2「本轮阻塞/跳过 8 单」的现势状态

| 单 | §7.2 登记 | 2026-10-26 现势【亲验】 |
|---|---|---|
| C-08 | 未提交落地 | **已落地** `a4438ca`（提交正文自陈「补救轮：主批该单 blocked 未提交，本轮补提交」） |
| C-09 | 未提交落地 | **已落地** `c67137d` |
| C-24 | 未提交落地 | **已落地** `5af76c9`（R2 侧编号 C-58；施工内容即 §7.2 所述工作树备货，复验后提交，未重写） |
| C-25 | 未提交落地 | **已落地** `7c659ef` |
| C-27 | 未提交落地（施工内容已备工作树） | **判定订正：内容已在 HEAD**。`crates/seccore/src/sandbox.rs:736-746` 已含本单要求的全部超时清理语义差异注释，并逐字自陈「(l0l10-r1 L11 处置:否决修复、仅注差异…)」；`git diff --quiet -- crates/seccore/src/sandbox.rs` rc=0；`git log -S "L11 处置" -- crates/seccore/src/sandbox.rs` → **`7b7e571` = C-45**，即由 C-45 批同批落地、无独立 C-27 提交。§7.2 的「未提交」与进度报告 §5.3 的「需重派 14 单」在此单上均与实况不符 |
| C-29 | 未实施（工作树无施工痕迹） | **仍未实施，且裁决面已变**：见 §10.3 |
| C-31 | 未提交落地 | **已落地** `bd68767` |
| C-36 | 未提交落地 | **已落地** `e3e0046` |

### 10.2 §6「W5 在途 C-42..C-55」与 §7.1 继承待裁项的达成情况

- **已达成（目标态经内容级复测确认，无需再出单）**：**C-42**（M06 三处落点 —— `gate_manifest.toml` G-09 note 现为「可达性棘轮（权威值当场由脚本自报，勿写死）」、`agents.md` 已指针化、`CODE_WIKI.md` 三方一致性行已带 ADR-222 取代订正注）、**C-44**（十六源手册头注已有 2026-10-26「编号空间批注」）、**C-47**（`adr_index.md` 已自述 ADR-075~083 九行补录）、**C-48**（`adr_index.md` ADR-015 行已含 `model-router → mca-gateway / CODE_WIKI §3.12`）、**C-54**（`docs/tui/README.md` 已订正 v3-engine 现状描述并链接 `[ADR-224 D5]`；该链接在本节 §10.4 的 C-41 结案前是悬空的，现已闭合）、**C-55**（十六源手册已有 2026-10-26「forbid 计数批注」）。另 **C-41/C-53/C-50 已以提交形式落地**。
- **仍可成立但不宜再称「blocked」**：C-43/C-45/C-46/C-49/C-51/C-52 均已在 §6 的 done 名单，与其一致。
- **因此本 ADR §7.2 的真实未完面 = 仅 C-29 一项**（其余全部或已提交、或目标态已达成、或已由他单承载）。R2 据此不再为上述单据重开变更单。

### 10.3 D1（M22 / C-29）的执行前置已变 —— 决策维持，路径补前置

业主 2026-10-26 裁为**先加固再接线**。R2 实测两条新前置（登 ADR-225 D4 / 登记册 M50、M51）：

1. `crates/session-store/src/**` 内 `CancellationToken` / `impl Drop` / `JoinHandle` / `shutdown` / `.abort(` **五个关键词全零命中**，而 `writer.rs:137-139` 无条件 `spawn_flush_loop(inner.clone())` 且 `:485` 不返回句柄、`types.rs:249 spawn_flush_loop: true` 为默认，同时 `writer.rs:150-152` 自陈「Ok 表示入队成功（非 fsync 确认）」⇒ 直接按 D1 接线会把**不可取消的永生刷写循环与丢 pending 数据**一起搬进生产，与 `segment.rs:288` 红线相背。
2. `writer.rs:123 new()` 的 `create_dir_all` + rusqlite `open`/`pragma_update`/`execute_batch` 不进 `spawn_blocking`（同层合规先例：`cmt-tiering/src/experience_card_storage.rs:92-106`）。

故 D1 的执行序改为 **C-61（取消令牌 + shutdown/Drop 刷写）→ C-62（open/DDL 入 spawn_blocking）→ C-63（本 ADR 原定的组合根注入）**。

另对 D1 原文的两处事实订正（不改写只加注）：① 本 ADR 与 R1 施工图所引的 `AppServer::with_session_store` **确实存在**（`crates/nexus-app-server/src/server.rs:293`，签名为 `(config: AppServerConfig, backend: Box<dyn CoreBackend>, store: CbmrWriter) -> Self`），且 `SessionStore` 类型**确实不存在**于 session-store crate（真构造 = `writer.rs:123 CbmrWriter::new(config: StoreConfig)`，dir 入口在 `types.rs:258 StoreConfig::with_dir`）；② 「无新增依赖边」（施工图 Δ1）**不成立** —— `dep_edge_freeze.txt` 现无 `chimera-cli|session-store` 行，且 `nexus-app-server` **不** re-export `CbmrWriter` ⇒ 任何组合根注入路径必新增一条 L10→L3 向下生产边，已由 ADR-225 D4 明文修订（定性为**计划缺陷而非决策缺陷**，因该承诺建立在已失效的假想 API 上）。

### 10.4 §6 总账自身的可追溯性已补齐，且本 ADR 已入库

本文件此前长期为 untracked（R1 的 C-41 未落地），即 §6/§7 的全部断言无法被 `git log`/`git blame` 复核。现由 **C-60 `8432504`** 纳入版本控制，验收读数 `py -3 scripts/adr_index_sync.py` rc=0（BROKEN LINK 无 / UNINDEXED 无）。

### 10.5 §8 验证面的两处订正

1. **§8.1 的 `cargo check/test/clippy` 五项 rc101 不是机型上限**：根因是 `.cargo/config.toml` 固化 `jobs = 4`。R2 全程显式 `--jobs 2`，实测 `cargo check --workspace --all-targets --jobs 2` **rc=0**（0 warning / 5m26s）、`cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` **rc=0**（5m27s），故本 ADR §6「收口验证不完整」的**前提已可解除**（读数见 `docs/reports/l0l10-r2-baseline-2026-10-26.md`）。
2. **§8.3「受影响门」不构成门禁全绿判据**：R1 只跑约 24/134 即判「全绿 26 项 PASS」，而全 134 门整体跑发现唯一真红 **G-85**，且该门步骤位于 `ci.yml` 的 **check job** L143-146 ⇒ 是一条 **PR 阻断红**（责任链：`0d39b50` 携带提交把无 BOM 的 `scripts/measure_startup.ps1` 带入仓；`e3e0046`=C-36 不是丢 BOM 的责任方，`e3e0046^` blob 已无 BOM）。由 R2 **C-57 `c8fcd1c`** 以字节级前置 BOM 解除，G-85 现 `RESULT: OK (26/26)` rc=0。制度性修订见 ADR-225 D2（收口唯一判据 = 全门集 `[SUM]`）。
3. **§8.1 的 `cargo test --workspace` 口径不充分**：该命令默认 fail-fast，R2 首跑仅出 67 个 target / 2327 passed 即中止；带 `--no-fail-fast` 才是真全量（447 target / **11375 passed** / 4 failed，而 4 条逐 target 隔离复测均 rc=0 ⇒ 并行负载敏感型 flaky，非回归）。
