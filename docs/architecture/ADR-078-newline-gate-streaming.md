# ADR-078:NewlineGate 行闸门——流式输出的行级提交(Concord W3 决策补档)

- **状态**:Accepted(W3 波次实施落地,本 ADR 于 W6 补档,方案 §15.3 索引对齐)
- **日期**:2026-08-11(决策实施于 2026-08-10 W3)
- **决策者**:Concord 重构波 3 实施(《Chimera_CLI_TUI重构方案_v1.0》§6 算法章与 §14 路线图)
- **相关**:ADR-061(v3-engine 默认渲染)、ADR-072(token 效率);方案 §15.4 N3 创新点落档
- **对应架构层**:L10 Interface(chimera-tui,数据累积层)

## 背景(Context)

流式对话输出面临渲染稳态与性能的两难:字符级直通导致半行文本在 diff 渲染中
反复闪烁(markdown 半行解析错乱);整段缓冲又牺牲首 token 可见时间(ttfb)。
Codex newline-gated streaming 实证了行级提交是稳态与性能的最优折中。
Chimera 的 v3 diff 引擎(16μs 级)位于行闸门下游——行级提交喂给微秒级 diff,
稳态与性能兼得(N3 组合创新点,竞品 diff 路径无公开同构设计)。

## 决策(Decision)

1. **行级提交**:`NewlineGate` 纯函数状态机置于 ChatSync 累积层
   (`data/newline_gate.rs`):流式增量按 `\n` 切出完整行提交给消息内容,
   半行残段留存 pending;`TuiChatCompleted` 时冲刷残段(不丢内容)。
2. **fence 块级放宽**:markdown 代码围栏(```)内的行放宽为块级提交——
   进入围栏后行暂存,围栏闭合才整块提交,避免代码块半块闪烁。
3. **渲染层零改动**:闸门在 v3 diff 引擎上游,渲染/布局/diff 路径不感知,
   保住 diff <100μs 阈值。
4. **内容守恒守护**:proptest 不变量——任意切分的 chunk 序列,
   feed 输出拼接 + flush == 原始流(单测 10 例含 2 proptest)。

### 备选与否决

| 备选 | 否决理由 |
|---|---|
| 字符级直通 | 半行闪烁 + markdown 半行解析错乱(方案 R3 病理) |
| token 级缓冲 | 无 markdown 稳态语义,缓冲边界任意,闪烁仍存 |
| 整段缓冲后提交 | ttfb 灾难(方案熔断项:ttfb 恶化 >100ms 即回滚) |

## 性能证据(实测)

- ttfb_first_line:W4 实测 118-220ns(bench_check.yml CI 阈值 <1ms,8 倍余量);
- feed_100_lines_charwise:≈34-64μs(逐字符最坏切分,CI 阈值 <5ms);
- diff_engine 下游阈值未受冲击(W3/W4/W5/W6 历次门槛 <100μs)。

## 熔断预案(方案 §14 对齐)

ttfb 恶化 >100ms → 闸门降级为配置项回字符级;行闸门为纯函数模块,
降级仅需旁路 feed 直通,ChatSync/渲染层契约不变。

## 后果(Consequences)

**正面**:流式渲染稳态(markdown 行级稳态);ttfb 纳秒级开销;
内容守恒 proptest 常绿守护;渲染层零冲击面。

**代价**:半行内容在行完成前不可见(设计取舍,换稳态);
fence 未闭合时块内容延迟到闭合或流结束(诚实语义,冲刷兜底)。

## 验证(Verification,历次门槛记录)

- W3 波次:行闸门 proptest 守恒过,ttfb bench 达标(方案 W3 验收标准);
- W4/W5/W6 波次:streaming_newline_gate 基准组常驻 bench_check.yml CI 阈值守护;
- chimera-tui 集成用例覆盖半行暂存/fence 稳态/冲刷兜底
  (chat_stream_holds_partial_line_until_newline /
  chat_stream_fence_block_commits_as_whole 等)。
