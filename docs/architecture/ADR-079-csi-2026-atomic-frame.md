# ADR-079:CSI 2026 同步输出探测回退链与原子帧写出(Concord W7 波5·原子帧)

- **状态**:Accepted(W7 波次交付,2026-08-11 门槛全绿)
- **日期**:2026-08-11(对应实施波次 2026-08-11 W7)
- **决策发起方**:Concord 重构方案 v2.0(《Chimera_CLI_TUI重构方案_v2.0.md》§5.1 / §5.5)
- **关联**:ADR-061(v3-engine 双轨)、ADR-072(输出链路收口)、方案 §12.3 N6 组合创新
- **影响架构层**:L10 Interface(chimera-tui,引擎输出侧增强,架构层不动)

## 背景(Context)

v3 引擎 diff 输出(16�0�8s 量级)解决"算得快",不解决"画得整":一帧的 ANSI
序列经多次 write 提交,终端可在任意两次 write 之间穿插重绘,产生半帧/撕裂。
pi Agent TUI 框架与 Claude 生态 TUI 设计规范均将"差分渲染 + 同步输出 +
批量单写"列为无闪烁渲染三件套(全必需)。工程实证同时表明:tmux 的 DECRQM
应答来自 tmux 自身状态而非客户端(tmux#5470),VTE 系终端同步输出实现残缺
致持续闪烁(pi Agent 被迫加逃生门)——无条件启用不可行。

## 决策(Decision)

1. **探测回退链三级**(`engine/sync_probe.rs`,启动一次,raw mode 后
   EnterAlternateScreen 前):
   - 一级:环境变量逃生门 `CHIMERA_NO_SYNC_OUTPUT=1` → `ForcedOff`
     (仅 "1" 生效,误拼写不静默触发);
   - 二级:`$TMUX` 存在 → 版本白名单(tmux ≥3.3a 支持,解析 `tmux -V`
     兼容 next-x.y 形态),替代不可信的 DECRQM 应答;
   - 三级:DECRQM 探测(`CSI ? 2026 $ p`,100ms 超时),应答 Ps ∈ {1,2,3,4}
     → `Probed`,超时/未识别/IO 失败 → `Disabled`,绝不阻断启动。
2. **三态 SyncMode**:`Probed`(包裹同步序列)/ `Disabled`(能力缺失)/
   `ForcedOff`(用户显式关)。能力探测不是运行时 feature flag,不违反
   ADR-034;三态区分供遥测区分"不愿用"与"不能用"(方案 §9.2 启用率指标)。
3. **AtomicFrameWriter**(`engine/atomic_frame.rs`):帧级复用缓冲累积全部
   ANSI 序列;`finish_frame` 单次 `write_all` 原子提交(系统调用 O(k)→O(1));
   Probed 时以 `CSI ? 2026 h/l` 包裹。缓冲容量跨帧保留,稳态零再分配。
4. **输出路径统一收口**:`V3Output` 持有 AtomicFrameWriter,`render` /
   `render_diffed` 全量与增量共四处写出统一经 `write_changes`,保证
   "任何帧均经原子写出"不变量;`V3Output::new()` 默认 Disabled 保持
   测试与兼容路径字节级零变化。
5. **FpsGovernor**(`app/fps_governor.rs`):Dashboard 视图绘制帧率封顶
   (TuiConfig.dashboard_fps_cap,默认 30,clamp [5,60],持久化字段);
   键/鼠事件到达即输入抢占,下一帧无条件放行(动画永不阻塞输入);
   Chat 交互视图不受治理;与 Eco tick 正交(绘制节奏 vs 数据节奏)。
   动画预算表内置(选中 0ms / 视图过渡 150ms / 图表过渡 300ms)。

### 备选与否决

| 备选 | 否决理由 |
|---|---|
| 无条件启用 CSI 2026 | tmux 应答歧义 + VTE 残缺实现实证致持续闪烁 |
| 仅 tmux 白名单不探测 | 非 tmux 终端能力无法判定,错失主流终端收益 |
| 每帧新建写出缓冲 | 稳态分配开销;帧缓冲复用零再分配是更优解 |
| Chat 视图一并封顶 | 流式输出与输入反馈优先,观测型与交互型界面分治 |
| 运行时 flag 切换同步输出 | 违反 ADR-034;能力探测 + 配置逃生门已覆盖 |

## 结果(Consequences)

- **性能**(2026-08-11 静默态取样):atomic_frame Probed 33.5�0�8s ≈
  Disabled 33.7�0�8s(包裹开销常数级不可测);增量帧 5.4�0�8s;
  v3_pipeline_full 200x50 = 1.01ms(<16ms 门槛);diff 5% 80x24 = 13.3�0�8s
  (<100�0�8s 门槛)。门槛零破位。
- **正确性**:单帧单 write 不变量由 CountingWriter 单测 + bench 双侧断言;
  探测解析器 proptest 守护(任意字节不 panic,SEC-3);撕裂类缺陷在
  Probed 终端上结构性消除。
- **回归零破**:chimera-tui 807 lib 测试 + 全集成测试绿;Disabled 模式
  输出字节与改造前一致(`disabled_output_keeps_legacy_bytes` 断言)。

## 风险与回滚

- **终端矩阵回归**(R10):任一主流终端闪烁/卡死 → 该终端进 Disabled
  名单(探测链可扩展终端黑名单);≥3 款回归 → 整波回滚,逃生门
  `CHIMERA_NO_SYNC_OUTPUT=1` 用户可自救。
- **帧阈值破位**(R11):render ≥16ms 或 diff ≥100�0�8s 连续两次 →
  冻结波5,SyncMode 全局 Disabled 回退排查。
- FpsGovernor 误伤交互:输入抢占机制保证键/鼠事件后下一帧恒放行,
  理论上限延迟 = 1 个最小帧间隔(30 FPS 下 ≈33ms),实际由抢占消除。
