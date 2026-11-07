# ADR-260: seccore 判决体域优化 — 索引化 + 零分配 glob + 无锁统计（R1）

- **状态**：Accepted（2026-11-08，`feat/toolchain-parallel-optimization` R1 波次；L4 硬门六对全部数量级通过）
- **上游**：已批准方案「工具链系统性深度优化重构方案（承接 W7-R2 终稿 + 阶段四 R1~R5）」R1 节；终稿 §3.4 结论 2（「seccore 判决体 2 次/派发与 to_lowercase 2k 属另立 ADR 的 seccore 域优化」——本 ADR 即该另立）
- **关联**：ADR-249（借用化否决后本 ADR 为派发热路径主优化）/ ADR-239（先实测后登记）/ ADR-246（声明式规则源，规则数用户可控 ⇒ O(R) 扫描随配置增长）

## 1. 背景（热点清单 #1/#2/#4/#11 收编）

`ExecPolicy` 判决是派发链最内层热路径（每派发**双重评估**，runtime.rs:6-18
防御性设计），改造前每次评估的成本结构：

| 成本项 | 量级 | 来源 |
| --- | --- | --- |
| 规则试探 `program.to_lowercase()` | **k 次 String 分配**/评估 | 旧 `matches` 逐规则归一（execpolicy.rs 旧 :153） |
| 规则扫描 | O(R) 线性 | `rules: Vec` 逐条 `matches`（先命中先裁决） |
| glob 匹配 | 3 重堆分配（`Vec<char>`×2 + n·m bool DP 表） | 旧 `glob_match`（其注释「性能非关键路径」已过时） |
| 统计记账 | std Mutex 锁 ×2/派发 | `DecisionStats::record`（并发批量下同一 `Arc<ExecPolicy>` 的共享争用点） |

ADR-246 声明式配置源（W1）使规则数用户可控——O(R) 扫描与 2k 分配随配置线性放大。

## 2. 决策

**D1 · program 索引（O(R)→O(1) 定位）**：`ExecPolicy` 增私有
`index: HashMap<Box<str>, Vec<u32>>`（`add_rule` 构造期维护，键统一小写）。
语义等价论证：program 匹配是**精确**小写比较 ⇒ 跨 program 规则互斥，命中
只可能来自同 program 规则 ⇒ 桶内保持先添加序 = 全局先匹配序，索引查找与
线性扫描判决逐位等价。非 `RulePattern::new` 构造的直接字面量 program（未
小写）在旧实现下本就永不命中（输入被归一），按小写分桶后同样永不命中，
边界语义一致。`rules: Vec` 保留为顺序与统计权威。`has_matching_rule`
（Plan 模式）同走索引判定。

**D2 · 归一单次化 + `*` 短路（消 2k 分配）**：评估入口对 program 做一次
归一——全 ASCII 且无大写 ⇒ 已是规范小写，**零分配借用视图**直传；否则一次
`to_lowercase` 复用（整次评估恰 1 次分配，不随规则数放大）。`RulePattern`
拆出 `matches_normalized`（热路径，收已归一 program；判决体与公共 `matches`
单源）。`args_glob == "*"` 短路：通配一切 ⇒ join_args 结果与 DP 均不影响
判决，短路消掉主力白名单规则形态的 join_args 分配（与既有空参特例同族）。

**D3 · glob 零分配（三重堆分配→O(1) 空间）**：`glob_match` 改迭代式双指针
回溯（仅 `*` 元字符，字节级比较——UTF-8 多字节序列不含 ASCII 字节 ⇒ 字节
相等 ⇔ 字符相等）；原 DP 实现保留为 `#[cfg(test)]` 参考体，256-case
proptest 对拍锁定逐位等价。

**D4 · DecisionStats 无锁化**：Mutex+StatsInner → 三桶 `AtomicU64`
（Relaxed fetch_add）。**快照语义取舍（显式登记）**：并发下逐桶 load 组成
的快照是近似瞬时值（不要求跨桶原子一致）——审计场景只要求各桶单调收敛，
全仓快照读数仅诊断/测试面，无消费者受损；换取每次派发消 2 次锁。

**D5 · 双重评估保持**：runtime.rs:6-18 的防御性双重评估（链头 PolicyLayer +
ExecPolicyToolExecutor 各一次）**不消除**——安全纵深设计与派发判决解耦
不属于性能优化范围；R1 后单次评估降至哈希级（hit 33ns/miss 26ns），双重
评估总成本 ≈66ns/派发可接受。若未来要消除，属安全语义变更，须另立 ADR。

**D6 · spol_* 阈值键移交 X2**：`spol_eval` bench 保留为 dev-only 裁决基准
（freeze 登记行）；`spol_*` 阈值键随 X2 唯一 bench 期实测后按五处双写 +
I10 对拍入册（先实测后登记，ADR-239；同 ADR-251 §4 纪律）。

## 3. 语义等价证据（L1）

1. **既有测试零修改全绿**：seccore 153 项（含 `evaluate_ref_matches_owned_
   verdict_bitwise` 双形态判决逐位一致、glob 严格语义、ModePolicy 全套）+
   gqep-executor 107 项（消费方回归）。
2. **新增等价 proptest（256-case × 2）**：`index_verdict_matches_linear_
   reference`（随机规则集 × 随机查询，索引判决 == 线性参考判决）/
   `glob_zero_alloc_matches_dp_reference`（零分配 glob == DP 参考体）。
3. **零分配锁**（`tests/policy_zero_alloc.rs`，CountingAlloc 复用
   util_micro.rs 先例）：deny-miss（零信任 dominant path）/ allow-hit
   （`*` 规则带参/无参）**全部 0 次堆分配**；非规范输入（大写）恰 1 次
   归一分配（上界断言）。非 `*` 通配规则的 join_args 分配**明示不纳入**
   零分配主张（ADR-260 §2-D2）。

## 4. L4 同轮裁决（2026-11-08 本机，`spol_eval` 12 测点，`--noplot`，sample 30）

| 测点对 | opt median（95% CI） | ref median（95% CI） | 比值 | 改善 |
| --- | --- | --- | --- | --- |
| hit_R4 | 33.43 ns [33.19, 33.71] | 516.6 ns [514.5, 518.5] | 0.065 | 93.5% |
| hit_R32 | 33.04 ns [32.87, 33.22] | 511.3 ns [509.4, 513.8] | 0.065 | 93.5% |
| hit_R256 | 32.55 ns [32.44, 32.71] | 511.9 ns [509.5, 514.7] | 0.064 | 93.6% |
| miss_R4 | 26.06 ns [25.97, 26.18] | 145.0 ns [144.6, 145.5] | 0.180 | 82.0% |
| **miss_R32** | 25.90 ns [25.80, 26.03] | 1.099 µs [1.094, 1.107] | **0.024** | **97.6%** |
| **miss_R256** | 25.97 ns [25.80, 26.17] | 8.660 µs [8.623, 8.699] | **0.003** | **99.7%** |

**硬门（R=32/256 miss 对 opt ≤ 0.70 × ref）六对全部通过且 CI 数量级分离**
（门限 30% 的放宽实证：miss 参考臂随 R 线性增长 145ns→8.66µs，优化臂恒定
~26ns —— O(1) 索引按设计工作；hit 双臂均 O(1) 但消 join/DP/归一后 15.6×）。
原始日志 `tmp/r1_bench.log`；复现
`cargo bench -p seccore --bench policy_eval -- --noplot`。

**派发链级影响（量级推断）**：以终稿「全链 ≈1-3µs」口径,双重策略评估
2×516ns ≈ 1µs 占全链 30-60%;R1 后 ≈66ns——工具链派发链端到端延迟预期
下降 ~30-50%（X2 期以 `policy_evaluate_allow`/`chain_*` 现键实测收敛）。

## 5. 回滚

单 commit revert（索引/glob/统计三改同文件域，revert 无交叉）；等价 proptest
与零分配锁随改随回。回滚后复跑 seccore+gqep-executor 全量测试。
