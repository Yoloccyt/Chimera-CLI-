# 实时数据驱动 TUI 面板系统报告

> 版本:v1.4.0-omega
> 日期:2026-07-13
> 范围:crates/chimera-tui + crates/event-bus(新增事件变体)

## 1. 背景与目标

`chimera-tui` 原为静态占位渲染,5 面板(Quest / Parliament / Budget / Log / Help)显示硬编码文本。本次改造将 TUI 接入实时数据源,通过 event-bus 订阅 NexusEvent 流,实现面板动态数据展示。

### 设计约束

- **依赖方向**:chimera-tui(L10) 只能向下依赖 L1 event-bus / nexus-core,禁止直接依赖 L9 quest-engine / efficiency-monitor(§2.2 依赖铁律)
- **跨层通信**:所有数据通过 NexusEvent 事件流获取,不引入新的跨层依赖
- **安全哲学**:所有 crate 顶层 #![forbid(unsafe_code)]

## 2. 架构设计

### 2.1 数据流

```
EventBus ─publish─> EventSubscriber ─try_recv─> DataPipeline ─snapshot─> TuiApp.update()
                                                                        ↓
                                                            TuiState { quest_list, budget, latest_events }
                                                                        ↓
                                                            render() → Quest / Parliament / Budget / Log / Help
```

### 2.2 核心组件

| 组件 | 职责 | 关键设计 |
|------|------|---------|
| EventSubscriber | 后台转发事件到环形缓冲区 | 1024 容量,溢出丢弃最旧;先 subscribe 再 spawn(§4.4 反模式 #3) |
| QuestSync | 维护本地 Quest 列表 | 消费 QuestListUpdated / QuestCompleted |
| BudgetSync | 维护本地 BudgetMetrics | 消费 BudgetMetricsUpdated |
| DataPipeline | 聚合多源事件生成快照 | 250ms tick 批量消费;同 tick 去重;实现 TuiDataSource |
| TuiApp.update() | 每帧拉取快照刷新 state | 非阻塞;失败时保留旧状态避免闪烁 |

### 2.3 新增 NexusEvent 变体

在 `crates/event-bus/src/types.rs` 新增:

- `QuestListUpdated { metadata, quests: Vec<Quest>, source: String }` — quest-engine 周期性发布完整列表
- `QuestCompleted { metadata, quest_id: String, status: QuestStatus }` — 标记 Quest 结束
- `BudgetMetricsUpdated { metadata, metrics: BudgetMetricsPayload }` — efficiency-monitor 发布结构化预算指标

配套类型:
- `QuestStatus` 枚举(Completed / Failed / Cancelled)
- `BudgetMetricsPayload` 结构体(与 chimera-tui::BudgetMetrics 字段对齐)

## 3. 面板渲染

### 3.1 Quest 面板
- 显示每个 Quest 的 title(加粗)、ID、thinking mode、任务统计(done 绿色 / pending 灰色)
- 空态显示 "No active quests"

### 3.2 Budget 面板
- 显示 tier / coefficient / consumption / remaining / utilization
- 30 字符利用率进度条(已用 = Cyan,剩余 - Gray)
- is_exceeded=true 时 Status: EXCEEDED 红色加粗;Alert 行红色加粗

### 3.3 Parliament 面板
- 从 latest_events 筛选 VoteCast / ConsensusReached / SkepticVeto / RedTeamAudit / AsaIntervention
- 最多显示最近 10 条
- 颜色:SkepticVeto 红 / AsaIntervention 黄 / RedTeamAudit 浅黄 / ConsensusReached 绿 / VoteCast 默认

### 3.4 Log 面板
- 主面板:逆序显示最近 10 条事件,格式 [HH:MM:SS] [source] type_name
- 底部固定面板:动态显示最近 1-5 条(受区域高度限制)
- 4 类 Critical 事件(SkepticVeto / RedTeamAudit / AsaIntervention / BudgetExceeded)红色高亮

## 4. 测试与验证

### 4.1 测试基线

| 命令 | 结果 |
|------|------|
| cargo test -p chimera-tui | 65 unit + 2 data_test + 15 integration + 5 subscriber + 1 doc-test(1 ignored) |
| cargo test --workspace | 3461 passed / 0 failed / 57 ignored |
| cargo clippy --workspace --all-targets --jobs 2 -- -D warnings | 0 warnings |
| cargo fmt --all -- --check | 0 diff |

### 4.2 新增测试

- `tests/data_test.rs`:多源事件对齐、同 tick 去重、1000 事件/秒性能测试(#[ignore])
- `tests/subscriber_test.rs`:先订阅后发布不丢失、try_recv 消费、shutdown 终止、lag 优雅处理、缓冲区溢出丢弃最旧
- `tests/integration.rs`:Quest/Budget/Parliament/Log 四面板数据驱动渲染集成测试

### 4.3 性能基准

新增 `benches/data_pipeline_bench.rs` 与 `benches/render_bench.rs`:

- **data_pipeline_snapshot_latency**:1000 事件/秒压力下单次快照延迟,目标 P95 < 100ms
- **data_pipeline_throughput**:125/250/500 事件/tick 三档对比(500/1000/2000 事件/秒)
- **render_all_panels**:5 面板完整渲染帧时间,目标 P95 < 16ms(60 FPS)
- **render_each_panel**:Quest/Parliament/Budget/Log/Help 分面板渲染耗时定位热点

## 5. 依赖变更

| crate | 变更 |
|-------|------|
| chimera-tui | 新增 chrono(workspace)、criterion(dev-dependency);tokio features 扩展为 ["sync", "time"] |
| event-bus | 新增 3 个 NexusEvent 变体 + QuestStatus 枚举 + BudgetMetricsPayload 结构体 |

## 6. 教训与启示

1. **tick 驱动 vs 事件驱动**:EventSubscriber 仅提供同步 try_recv,DataPipeline 用 interval + select! 单分支驱动;未来可扩展 async recv() 实现事件到达立即唤醒
2. **同 tick 去重**:状态更新层只保留最后一个 QuestListUpdated / BudgetMetricsUpdated;日志流不去重保留完整历史
3. **Box<dyn TuiDataSource>**:trait object 避免泛型污染 TuiApp 签名,动态分派开销可忽略(每帧仅一次 update)
4. **Text<'static> 替代 String**:支持 Span::styled 实现颜色/加粗,Paragraph::new 天然接受
5. **TuiState 派生调整**:含 f32/f64 字段不能派生 Eq,改为仅 PartialEq

## 7. 后续演进

- **事件驱动唤醒**:为 EventSubscriber 增加 async recv(),DataPipeline select! 增加第二分支,降低延迟至事件到达即刷新
- **更多面板数据源**:Parliament 面板可扩展显示实时议员状态(需新增 ParliamentStateUpdated 事件)
- **交互式操作**:支持在 TUI 中直接操作(如取消 Quest、调整预算),通过反向事件发布到 event-bus
- **配置化 tick**:`TuiConfig` 增加 `data_tick_interval_ms` 字段,允许用户调整刷新频率
