# ADR-081:ContextCurator 客户端上下文策展与摘要接缝(Concord W9 波7·策展)

- **状态**:Accepted(W9 波次交付,2026-08-12 门槛全绿)
- **日期**:2026-08-12(对应实施波次 2026-08-12 W9)
- **决策发起方**:Concord 重构方案 v2.0(《Chimera_CLI_TUI重构方案_v2.0.md》§5.3)
- **关联**:ADR-078(NewlineGate,策展前置 flush 的守恒对象)、ADR-080(PaceGate,
  命令信道同构先例)、方案 §12.3 N8 组合创新
- **影响架构层**:L10 Interface(chimera-tui 数据层 + 命令层;零新增事件,132 不变(权威口径当时 136,现 144))

## 背景(Context)

v1.0 的 /compact 登记为 Orchestrated 层但未接线(诚实反馈 HonestTodo)。
2026 前沿已把上下文压缩推进到"策展即动作"(MemAct ACL Findings 2026、
MemAgent ICLR 2026):上下文更新建模为策略动作,keep/summarize/evict 三动作
空间,价值密度驱动取舍。方案 §5.3 裁决:客户端语境下的"压缩"真实语义是
**选择哪些消息段进入下一次请求**——本地压缩**模型**(LLMLingua 本体)被否决
(纯 Rust / 无 GPU / 不引入推理依赖红线)。

## 决策(Decision)

1. **五段分类诚实映射**(`data/curator.rs`):System=预留恒空(当前无消息源);
   Pinned=User 轮次(用户消息不可驱逐,对齐主流 /compact 语义);
   Recent=末 `recent_turns×2` 条窗口;其余 Assistant=候选。不为不存在的
   pin/system UI 伪造交互。
2. **价值密度打分**:`α·recency + β·ref_count + γ·pinned �6�1 δ·token_cost`
   (f32 全程不转 f64);默认权重经"保留价值 ≥0.85 固定语料验证集"标定
   (α=1.0 β=0.3 γ=0.5 δ=0.2,budget 4096,recent 4 轮)。
3. **0-1 背包预算分配**:强制保留段先扣预算,候选段在剩余容量内
   滚动双行 dp + 决策矩阵回溯,O(n·W);实测 n=500/W=4096 = 1.90ms
   (<10ms 门槛,5× 余量)。
4. **执行通道**:有界 mpsc 命令信道(容量 16)入 DataPipeline 任务,
   `tokio::select!` tick/命令双分支——命令分支不占 tick 名额,策展对数据
   节奏零干扰;报告经快照 `last_compact_report + seq` 回传,app 判新刷
   状态栏(与 action_feedback 同范式)。策展前 `flush_pending` 冲刷配速
   队列(守恒:不得丢失 W8 节奏闸内未到期行)。
5. **摘要接缝(T9.4)**:`SummaryBackend` trait + `ExtractiveSummary` 默认
   实现(纯 Rust 抽取式:落选高价值段逐条取首 N 字符合并、总量封顶,
   组装"[上下文策展摘要]" assistant 消息)。**主 API 异步摘要诚实推迟**:
   orchestrator 仅消费 TuiChatSubmitted 且会显示为用户轮次,隐藏摘要通道
   当前不存在——接缝预留 `ApiSummary` 位,通道就绪后接线,不在本波伪造。
6. **RL 预留**:`CurationPolicy` trait 为策略接缝,`RuleCurationPolicy`
   规则实现首参;MemAgent/MemAct 学习策略未来替换规则表不改调用方
   (方案 §5.3 备注)。
7. **/context 网格(T9.5)**:复用 `PopupKind::Detail`(不新增 PopupKind),
   内容 = 对当前历史现场 classify 的四段分布条(Unicode 块字符 + 百分比 +
   token 估算)+ 末次策展报告;空历史诚实提示。

### 备选与否决

| 备选 | 否决理由 |
|---|---|
| 本地压缩模型(LLMLingua 本体) | 纯 Rust/无 GPU 红线;引入推理依赖;客户端真实语义是段选择 |
| 主 API 摘要本波接线 | 隐藏摘要通道不存在(TuiChatSubmitted 会显示用户轮次),伪造违反诚实原则 |
| /compact 改登记 Instant 层 | 词表脚本与主流语义对齐(compact 属会话域编排语义);tier 不变,执行路径走 Instant 效果并注释 WHY |
| EventBus 事件传递策展请求 | 零新增事件原则(132→134 配额留给 W10 握手(权威口径当时 136,现 144));进程内 mpsc 更简 |
| 同步执行策展于 app 线程 | ChatSync 归管道任务单一所有权;跨线程直改违反所有权模型 |

## 结果(Consequences)

- **性能**(2026-08-12 静默态取样):curate_500_msgs_budget_4096 = 1.90ms
  (<10ms);classify_500_msgs 与紧预算形态同档(<2ms);策展为触发式,
  对每帧热路径零影响。
- **正确性**:curator 21 测(含 4 proptest:候选保留 ≤ 容量、预算单调性、
  分类全覆盖零丢失、候选三类分流守恒)+ 保留价值 ≥0.85 固定语料验证集 +
  E2E 管道回环 6 测(历史替换/报告回传/Pinned 零驱逐/摘要插入/Stub 拒绝/
  非法参数用法)。chimera-tui lib 847 passed / 0 failed。
- **回归**:W2 的 HonestTodo 示例测试(/compact)按接线事实改例 /vim,
  诚实反馈断言不变。

## 风险与回滚

- **R13 策展价值跌破**:保留价值 <0.85 且调权两轮无改善 → 降级为
  "/compact 原语义 + /context 网格"两步交付(方案熔断条款)。
- **命令信道满**:try_send 失败 → 状态栏诚实反馈"信道忙",用户重试;
  容量 16 吸收误连击,不引入背压复杂度。
- **select 循环改造风险**:既有 live_data_test/configurable_tick_test 全回归
  覆盖;命令分支 continue 不推进 Eco 计数,行为零变化。
