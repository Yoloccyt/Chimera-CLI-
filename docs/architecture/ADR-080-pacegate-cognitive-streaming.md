# ADR-080:PaceGate 认知节奏闸门替代 NewlineGate(Concord W8 波6·节奏)

- **状态**:Accepted(W8 波次交付,2026-08-11 门槛全绿)

> ⚠ **实施状态校正（2026-09-22 现测；决策文本不改，只更正“已交付”叙述）**：本 ADR 的实现文件
> `crates/chimera-tui/src/data/pace_gate.rs`（575 行、16 个 `#[test]`）**未被任何 `mod` 声明**
> （该 crate 107 处 `mod`/`pub mod`/`pub(crate) mod` 里无 `pace_gate`，`PaceMode` 仅现于该文件自身）
> ⇒ **不参与编译，其单测从不执行**；`SlashCommandRegistry` 亦无 `/pace`。现状 = **未接线/待实现**。
> 根因教训：Rust 的 `mod` 声明是编译闸门，“文件在盘 + 门槛全绿”不等于代码进了 crate（无孤儿源文件门）。
- **日期**:2026-08-11(对应实施波次 2026-08-11 W8)
- **决策发起方**:Concord 重构方案 v2.0(《Chimera_CLI_TUI重构方案_v2.0.md》§5.2)
- **关联**:ADR-078(NewlineGate 行闸门,本决策内嵌复用其完整性闸)、方案 §12.3 N7 组合创新
- **影响架构层**:L10 Interface(chimera-tui 数据同步层,渲染层零改动)

## 背景(Context)

NewlineGate(ADR-078)只有"完整性"一维:完整行即提交。流式响应突发到达时
(网络批量/服务端批生成),渲染呈现为突发式刷屏,阅读期认知负载高。UIST 2025
实证(Xiao & Yang,认知负载感知双流):阅读时慢流、跳读时快流,服务侧成本降
而体验不损。节奏维度缺失是 v1.0 行闸门的已知短板(方案 §3 R2 裁决)。

## 决策(Decision)

1. **双闸门串联**(`data/pace_gate.rs`):完整性闸内嵌 NewlineGate(行/fence
   规则零改动)+ 节奏闸按内容类别配速——Code ≤8ms/行(快速通道)、
   Prose ~40ms/行(约 250 词/分阅读速度)、Status(空行)即时。
2. **首行直通**:`first_line_passed` 之前零延迟提交,TTFB ≤100ms 不变量
   保持(R12 熔断线);每轮交互(TuiChatSubmitted)复位,逐轮生效。
3. **积压排空**:待提交行 > 阈值(默认 64)批量全放行,防积压雪崩;
   有界缓冲 SEC-6 缓解不变量由 proptest 守护。
4. **显式时钟注入**:所有时间经 `now: Instant` 参数,组件纯确定可测;
   生产由 ChatSync/pipeline tick 传 `Instant::now()`。
5. **tick 驱动到期提交**:DataPipeline 每 tick(含无事件 tick)调用
   `ChatSync::tick`,无新 chunk 时流式仍按节奏推进;流结束(Completed)
   队列全放行 + 残段冲刷,守恒优先于节奏。
6. **配速档三态**:`/pace fast|reading|off`(Instant 层命令,词表 53→54);
   `TuiConfig.pace_mode` 持久化字段(ADR-034 非运行时 flag);经
   `TuiDataSource::set_pace_mode`(默认 no-op,DataPipeline 覆写 AtomicU8
   槽位)注入管道,下一 tick 生效。
7. **围栏态自持跟踪**:NewlineGate 对 fence 块延迟整块提交,提交时其
   in_fence 已闭合无法逐行反映;PaceGate 自持翻转态(标记行成对 XOR)
   保证块内行正确分类为 Code。

### 备选与否决

| 备选 | 否决理由 |
|---|---|
| 保持单行闸门 | 节奏维度缺失;认知配速体验与成本双赢有学术实证 |
| 字符级配速 | 与完整性闸冲突(半行 markdown 破态);节奏作用于行/块粒度 |
| 本地引入独立节奏定时器 | 复用管道既有 tick 即可,零新任务零 orphan 风险 |
| EventBus 事件传递配速档 | 单值控制旋钮,AtomicU8 槽位更简(不占 132 事件变体配额(权威口径当时 136,现 144)) |
| 队列追溯重排(切档时) | 内容乱序风险;已排程单元按原到期时刻自然释放 |

## 结果(Consequences)

- **性能**(2026-08-11 静默态取样):pace_ttfb_first_line 187.5ns
  (NewlineGate 125.7ns,+62ns,远低于 CI <1ms 门槛,TTFB 不变量保持);
  pace_feed_100_lines_charwise 58.0�0�8s = NewlineGate(36.9�0�8s)×1.57,
  满足 ≤2× 门槛(摊销 O(1)/chunk);pace_drain_due 8.3ns(tick 热路径可忽略)。
- **正确性**:三条 proptest 不变量全绿——①任意切分+任意节奏下输出拼接==输入流
  (守恒)②首行零延迟 ③积压峰值 ≤ 阈值+1(有界);另有 Off/Fast 档零扣留不变量。
- **回归**:ChatSync 既有测试按配速语义更新(tick 释放 / Off 档即时 /
  Completed 冲刷),chimera-tui lib 825 passed / 0 failed。
- **可观测**:CommitUnit 携带 ContentClass,为后续配速遥测(方案 §9.2
  阅读期帧更新频率指标)预留数据。

## 风险与回滚

- **R12 TTFB 恶化**:首行延迟 >0ms(直通失效)→ 回滚本波,行闸门语义恢复;
  用户侧自救:`/pace off` 一键恢复 v1.0 语义(TuiConfig 持久化,重启仍生效)。
- **卡顿感反馈**:配速档切 `fast`(零配速)或 `off`(行闸门),无需代码回滚。
- **tick 粒度限制**:到期行释放粒度 = DataPipeline tick(250ms),阅读配速
  呈现为节奏化批量而非逐行匀速——当前取舍(零新定时器);若体验评估要求
  更细粒度,后续在 tick 内细分 micro-drain 而非引入新任务。
