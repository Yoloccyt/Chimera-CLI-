# ADR-185: gea-activator / gqep-executor / qeep-protocol 三岛链接线(wave 3c)

- **状态**: Accepted(2026-09-16,M12)
- **前置**: M11 接线决策报告(wt-11 只读 survey,任务书与证据起点)、ADR-160(生产可达性棘轮)、ADR-161(wave 3 治理)、ADR-048(QEEP 纠缠协议跨层边)、ADR-179(空转判据)、ADR-175(死模块处置日程)
- **影响面**: `crates/chimera-mas`、`crates/chimera-cli`、`crates/gqep-executor`、`scripts/check_dependency_rules.{sh,ps1}`、`scripts/crate_reachability_freeze.txt`、`docs/architecture/CODE_WIKI.md`
- **开工前查重**: `git ls-files` / 主检出文件系统 / wt 文件系统均无 `ADR-185*` 同名(untracked md 落盘沿用 tower 对 M10 的裁定,本文件为 tracked 新建)

## 背景

ADR-160 棘轮登记的冻结孤岛中,`gea-activator`、`gqep-executor`、`qeep-protocol` 三 crate 构成一条"传递不可达链"(freeze.txt 2026-09-11 注记:各自唯一生产消费者即链上的下一环,而链根 gea-activator 本身是孤岛)。M7 验收前瞻要求 wave 3c 完成三岛转正;M11 决策报告(wt-11 @ d1fdf6c,零 diff 留证)完成三候选对评并给出唯一推荐拓扑与执行裁决。本 ADR 将 M11 裁决的六个决策正式落档。

**推荐拓扑(仅 2 条新边,qeep 传递转正)**:

```text
chimera-cli(L10) ──→ chimera-mas(L9) ──→ gea-activator(L9)   [新边1]
chimera-cli(L10) ──→ gqep-executor(L7)                        [新边2]
gqep-executor(L7) ──→ qeep-protocol(L4)                       [既有边,ADR-048 收编]
```

## 决策

### D1: gea-activator 消费者 = chimera-mas(L9→L9 同层)

采纳 M11 候选 A,否决候选 B/C,判据留档:

- **选 mas 的理由**:语义地基已书面存在——`chimera-mas/src/feedback.rs:7-12` 明确"gea_activator::ExpertProfile 携带单专家运行时统计,门控 confidence 直接消费;本注册表是 MAS 层聚合视图;两者互补:gea 侧管'激活倾向',mas 侧管'调度/分配倾向'"。接线 = 兑现既有设计,非新造需求。真实生产调用路径存在:`chimera agent spawn` → `RootOrchestrator::delegate`。现状增益真实:mas 专家选择原为静态象限映射(experts.rs E01-E08 固定表),gea 提供动态门控 + 负载自适应动态阈值 + 反馈进化,是能力升级而非平行空转。
- **否决候选 B(chimera-cli 组合根直接消费)**:gea 是纯生产者(零事件订阅,activate 须被调用)。CLI 无自然调用点;若组合根仅构造句柄而无人调用,即 mca-M4 式"装配态空转"——违 ADR-179"接线=空转"否决判据(ADR-171/M7③ 亦明言"GATED/装配态只消告警非转正")。组合根在最终方案中仅作**构造载体**(AppContext 持句柄注入 mas),不构成独立候选。
- **否决候选 C(gea→gqep 自洽环)**:gea `activate()` 是读锁下同步计算,全 crate 无可聚集 future;为成环而引入 gather = 制造假需求(空转风险最高);且 gea 属内环、gqep 属外环 L7,gea→gqep 会撞内环 Check A(内环禁伸入 L2+ 外环)。三方否决。

### D2: E01-E08 → gea one-hot 桥接策略(64 维词表 + 门控配置重校准)

- **桥接词表(权威定义,`chimera-mas/src/gea_bridge.rs` 模块文档同表)**:

  | 维区间 | 语义 | 专家向量 | 任务 CLV |
  |---|---|---|---|
  | dims[0..4] | 象限 one-hot(Q1→0…Q4→3) | 主责象限=1.0 | 各激活象限=复杂度 |
  | dim[4] | 强度维 | 编制权重(Tier 映射) | 复杂度评分 |
  | dim[5] | 风险维 | — | 风险等级/100 |
  | dim[6] | 保留 | 0 | 0 |
  | dims[7..64] | 能力标签 FNV-1a 稳定哈希 | 各标签→1.0 | task_type 同哈希→复杂度 |

- **映射规则**:复杂度取四档带宽中点(Simple 0.25/Medium 0.5/Complex 0.75/VeryComplex 1.0);风险以调度优先级为代理(Low 10/Medium 30/High 60/Critical 90);亲和词表 = 小写象限名(与 gea `compute_affinity` 归一化匹配对表);任务象限激活矩阵复用 `QuadrantPlan::from_complexity`(既有权威语义,桥接只翻译不新造)。
- **同名类型消歧**:mas `experts::ExpertProfile`(&'static 权限模型)与 gea `ExpertProfile`(动态向量模型)是两个领域类型,代码全限定路径书写,禁合并(领域类型稳定性红线)。
- **门控配置重校准(`mas_gea_config`)**:gea `GeaConfig::default()` 面向稠密 512 维 CLV 调参(bias 0.5、w4=0);桥接稀疏 one-hot 的 relevance 上限更低,且接线目的本身要求 confidence 反馈闭环真正影响门控。故 w1=0.3/w2=0.2/w3=0.25/w4=0.25/bias=0.35(权重和=1.0 过 gea 校验)。高负载冷启动下激活趋保守(动态阈值随负载/未命中率抬升)——这是 Ω-Sparse 的稀疏激活语义,非缺陷。

### D3: gqep-executor 消费者 = chimera-cli doctor 8 探针并行 gather

- **gather 泛型化**:原 `gather` 硬编码 `GqepFuture<String>` 且 `GatherResult` 仅统计不保留个体值。新增 `gather_collected<T>`(私有 `gather_generic<T>` 内核承接原本体;`gather` 收敛为 `.stats` 薄包装,签名/语义/事件契约零变化),`GatherCollected<T>{stats, values}`。
- **doctor 并行化**:8 探针以 `(索引, HealthCheck)` tag 化,经组合根共享 `GqepExecutor` 并行 gather;完成后按索引还原注册序——**输出内容与串行版逐字节一致,仅时序并行化**。探针 future 恒 `Ok`(检查失败是结果而非错误),gqep 超时 tail 以 FAIL 占位兜底,报告恒 8 项(该 tail 实践中不可达:LLM 探针自带 3s 内部超时,其余快路径)。
- **否决 mas 路径**:DelegationExecutor 需返回 `Vec<TaskResult>` 个体结果,而 gather 返回 GatherResult(仅统计)——语义不匹配,`delegation.rs:726` 书面判词继续有效;为成环而 gather = 制造假需求。
- **装配收敛**:AppContext 增 `gea` + `gqep` 句柄(对齐 M4/M9 方向);doctor 由"进程内 ephemeral 自检"迁入共享 AppContext(与 chat/run 等七命令同先例),命令层 thin wrapper 可独立 revert。

### D4: ADR-048 例外边收编(升级收编,不维持豁免、不消除)

- **事实**:Check B 只 flag 向上依赖(dep_layer > layer),gqep-executor(L7)→qeep-protocol(L4) 向下边**无豁免也天然合法**;is_adr_exception 中该条目当前已是 no-op 安全垫。P9-T6 复审附录已裁"外环内部 L7→L4 向下依赖合法";全仓 N→N-k 向下边普遍(chimera-mas→decay-engine L9→L4、chimera-cli→seccore L10→L4)。
- **裁决**:"维持豁免"最低效——例外名义已无执法对象,挂着反而误导后人以为该边仍异常;"消除"=Event Bus 解耦,ADR-048 §3.2 已否决且性能证据未变(entangle 实测 0.41µs,10~100× 慢于同步路径的解耦方案不可接受)。故**升级收编为装配面正常向下边**。
- **执行**:`.sh`/`.ps1` 的 is_adr_exception($adrExceptions)移除该条目;selftest mock 图保留该边并断言"不得冒出 GAP"——移除后 normal + selftest 双门 EXIT=0 = 收编的可执行证据。**INV-GQEP-1~4 不变量继续有效**(转正不削弱守护)。
- **ADR-048 文件注记**:该文件为 untracked(`.gitignore *.md` 政策)且不在 wt-12 检出,无法就地追加 wave 3c 复审注记;收编全貌以本 D4 为权威记录,合并后由主检出侧补注记(见 M12 完成报告披露项)。

### D5: 零新增 NexusEvent 变体(ADR-161 决策 5 合规)

复用既有三变体 `ExpertActivated` / `ActivationThresholdAdjusted` / `ActivationCacheStats`(registry.rs 均 Normal 级,TUI 已分类 NotForTui)。**145 变体锁定分毫不动**;`CRITICAL_MPSC_VARIANTS=13` / `CRITICAL_TOTAL=17` 与 variant_count 测试零扰动;事件 schema(GatherCompleted/GatherTimedOut/OrphanCallDetected)与发布点不变。

### D6: 棘轮 shrink-only 删 3 行 + 口径回写

- freeze.txt 删 `gea-activator`、`gqep-executor`、`qeep-protocol` 三行,留 REMOVED 注记(M12 批次原因+ADR-185 引用);`check_crate_reachability.sh` 实测 **reachable=30 frozen=11 new_gaps=0**(基线 27/14)。
- CODE_WIKI §3.11/§1(头部块、三方一致性、可达性标注、§3 索引、★Insight、目录注释、§13 分析基线)口径回写 27→30 / 14→11,ADR 表登记本文件。

## 后果

- **正向**:三岛链全量转正(42 = 30 生产可达 + 11 冻结 + 1 GATED);mas 专家选择从静态映射升级为动态门控 + Ω-Evolve 反馈闭环;doctor 8 探针并行化(LLM 238ms 睡眠与其余探针并发,诊断时延下降);gqep gather 泛型化为后续带值聚集消费者铺路。
- **代价**:chimera-mas 内部依赖 11→12(Check D ≤16 界内);chimera-cli 新增 2 条生产边(依赖面均轻——gea/gqep 的外部依赖全部已在装配面,无新增传递面);release 体积预估增量 <0.5MB(7004 LOC + 零新外部传递依赖,<50MB 红线余量充足)。
- **回滚预案**(ADR-161 决策 3/4):依赖添加/接线/桥接每 crate 一 commit 可独立 revert;freeze.txt 回登记与 revert 同 PR;doctor gather 化单 commit revert 即恢复串行;gea/gqep 接线均为 Optional 句柄 + serde(default) 字段,对外零行为变化(additive)。

## 验证证据(指向)

- mas 单测:gea_bridge 词表布局/Tier 权重/注册幂等;delegation 回填三态;feedback 转发;orchestrator ExpertActivated 事件。
- mas 集成(`tests/gea_wiring_test.rs`):①委托路径 ExpertActivated 断言;②record_outcome 后 confidence 升降循环影响门控断言。
- mas 属性(`tests/proptest.rs`):桥接 TaskProfile 布局不变量、quadrant_dim 双射、Tier/Profile 往返、构建确定性。
- gqep(`src/gatherer.rs` 测试):gather_collected 值完整性/struct 穿透/失败剔除/事件契约/空批/gather 回归守护。
- cli(`composition.rs`/`doctor.rs` 测试):gea/gqep 组合根装配;③doctor gather GatherCompleted(total=8) 断言。
- 治理门:dependency rules .sh+.ps1(normal+selftest)EXIT=0;check_crate_reachability reachable=30/frozen=11/new_gaps=0;层图 parity / machete / declared_dep_usage 同口径见 M12 完成报告。
