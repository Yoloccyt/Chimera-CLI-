# M2 路由策略学习触发条件评估报告

> **评估日期**:2026-07-09
> **任务**:M2(P2 中期演进,条件触发评估)
> **关联 spec**:`.trae/specs/v1-3-0-omega-post-optimization-roadmap/`
> **基线版本**:v1.3.0-omega
> **评估对象**:`crates/model-router/src/moe.rs` HistoryStore + InMemoryHistoryStore + 五维 gate_score

## 1. 触发条件评估

| 触发条件 | 阈值 | 当前状态 | 是否触发 |
|---------|------|---------|---------|
| 历史路由数据规模 | > 10000 条 | **无持久化实现**(InMemoryHistoryStore DashMap,进程重启丢失) | 否 |
| 静态权重次优路由率 | > 5% | 无直接测量(无生产数据);Top-K=5 设计保证召回(`test_gate_includes_best_model_in_top_k` 单元测试通过) | 否(理论上 < 5%) |

**结论**:触发条件**均未满足** → 继续延后,仅做评估,不启动实施

## 2. 当前状态分析

### 2.1 当前路由策略实现
- 实现:静态五维评分(cost 0.3 / latency 0.3 / quality 0.2 / success_rate 0.1 / variance 0.1)
- 历史数据存储:`InMemoryHistoryStore`(DashMap,model_id → HistoryRecord)
- 历史记录结构:`success_count` / `total_count`(累计)+ `latency_samples`(VecDeque<f32> capacity 100 滑动窗口)
- 降级路径:历史 < 100 条时降级三维(0.375/0.375/0.25,保持 3:3:2 比例)
- Top-K 选择:`select_nth_unstable_by`(O(n),符合 §4.1 Engineering Convention)
- 历史维度采集:`HistoryStore::record(model_id, latency_ms, success)` 由调用方在路由完成后回调

### 2.2 历史数据持久化评估
- 当前:`InMemoryHistoryStore` 仅内存实现,**无任何持久化**(`moe.rs:182` 注释仅声明 trait 为 v1.4.0 RL 预留扩展点)
- 问题:CLI 是按需调用的命令行工具(单次进程),重启即丢失全部历史;即使 daemon 化,内存上限与多进程共享均无法保证
- 阻塞:无法在真实部署中累积 10000 条历史数据 → 触发条件 1 物理上不可达
- trait 抽象:`HistoryStore` 已是对象安全 trait(`&self` 方法 + 无泛型 + owned 返回),为未来 SQLite/Redis 实现预留扩展点,**无需修改 `MoeGate::gate()`**

### 2.3 静态权重次优路由率评估
- 当前权重:0.3/0.3/0.2/0.1/1(三维降级 0.375/0.375/0.25)
- 权重设计合理性:cost/latency 主导(0.6)+ quality 补充(0.2)+ 历史维度微调(0.2),已通过 6 个 TDD 测试 + 2 proptest(256 cases)验证不变量
- Top-K=5 设计的召回保护:`gate_score` 与 `route_auto` 完整评分方向一致(倒数 1/(1+x) 与 1-x/max 都是"越小越好→分越高"),粗筛阶段漏掉真正最优模型的概率低
- "次优路由"定义已澄清:
  - **严格定义**:Top-1 与真正最优模型的差距(gate_score 排序失真)
  - **宽松定义**:Top-K 是否包含真正最优模型(K=5 时召回率高)
- 实际测量:无生产环境数据采集,无法直接计算次优率;单元测试 `test_gate_includes_best_model_in_top_k` 在 60+1 模型场景验证 Top-K 始终包含最优模型

## 3. 候选方案

### 3.1 Multi-Armed Bandit(若未来触发,优先推荐)
- **优势**:探索-利用平衡,收敛快,适合短期决策场景; reward 函数直观(成功率 + 延迟倒数 + 成本倒数加权)
- **劣势**:需定义 reward 函数;冷启动阶段探索开销
- **评估**:适合路由决策(每次路由即一次 arm pull),契合 RL 路由本质

### 3.2 在线梯度下降
- **优势**:实时适应,增量更新权重
- **劣势**:需要学习率调优,可能不稳定; gate_score 当前为线性加权,梯度下降可行但需谨慎避免权重爆炸/坍缩
- **评估**:适合数据流稳定场景,但调试复杂度高于 Bandit

### 3.3 离线训练 + 模型部署
- **优势**:训练充分,稳定性高,可离线评估多组权重
- **劣势**:需要离线数据集,更新延迟;部署流程复杂
- **评估**:适合数据积累充分后的批量优化,不适合实时演进

### 3.4 保留静态权重 + 人工调优(当前建议,未触发)
- **优势**:零复杂度,可解释性强,已通过 256 cases proptest 验证不变量
- **劣势**:无法自动适应新模型加入 / 模型性能漂移
- **评估**:当前规模(无持久化数据)足够,无需 RL;待历史数据持久化落地后再评估

## 4. 建议与后续行动

### 4.1 当前建议(未触发)
- **继续使用静态五维权重**,延迟评估到历史数据持久化实现后
- 触发条件 1(> 10000 条)在当前内存实现下物理不可达,优先级 P3(延后至 v1.5.0+ 评估)
- 触发条件 2(> 5% 次优)无生产数据采集,待持久化后随数据积累自然成熟

### 4.2 前置依赖(若未来触发)
- **历史数据持久化(必须)**:实现 `SqliteHistoryStore` 或 `FileHistoryStore`
  - SQLite 路径:rusqlite bundled(已有依赖),spawn_blocking 包装(§4.4 #2),无 unsafe 传播
  - File 路径:JSONL / MessagePack append-only,简单但并发性差
  - 推荐:SQLite,契合 ADR-005 持久化选型
- **Reward 函数定义**:成功率(0.5)+ 延迟倒数(0.3)+ 成本倒数(0.2)加权,值域 [0,1]
- **数据采集钩子**:在 `route_auto_with_gate` 返回 `RoutingDecision` 后,调用方需异步回调 `HistoryStore::record`

### 4.3 触发条件监控
- 实现历史数据持久化后,定期评估数据规模(`SELECT COUNT(*) FROM history`)
- 当历史 > 5000 时启动预警(进入"接近触发"状态)
- 当历史 > 10000 且静态权重次优率 > 5%(需新增 bench 测量)时触发实施

### 4.4 实施前置条件(若未来触发)
- 新建独立 spec:`.trae/specs/v1-4-0-omega-rl-routing/`
- 候选方案优先级:**Bandit > 在线梯度 > 离线训练**(收敛速度 + 实时性优先)
- 预估工时:40h+(spec 设计 4h + 历史持久化 12h + RL 算法实现 16h+ + 测试/bench 8h+)
- 风险评估:
  - 学习率不当导致权重爆炸 / 坍缩(需 clamp + 正则化)
  - 冷启动阶段探索开销(需 ε-greedy 或 UCB 缓解)
  - `#![forbid(unsafe_code)]` 约束:Bandit 算法纯 Rust 实现可行,无需 unsafe
  - 与 v1.3.0 五维评分向后兼容:RL 权重作为"调整项"叠加在静态权重上,而非完全替代

## 5. 关联文档

- v1.2.0 Task 3 MoE 报告:`docs/optimization/v1.2.0/task3_moe_verification_report.md`
- v1.3.0 S2 MoE 五维报告:`docs/optimization/v1.3.0/s2_moe_history_report.md`
- v1.3.0 综合报告:`docs/optimization/v1.3.0/full_post_optimization_report.md`
- 代码权威源:`crates/model-router/src/moe.rs`(HistoryStore trait L190-198 / InMemoryHistoryStore L216-242 / gate_score L337-362)
- spec 路径:`.trae/specs/v1-3-0-omega-post-optimization-roadmap/tasks.md`(Task M2 行 132-136)
