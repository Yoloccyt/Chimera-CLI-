# 维度 E:测试覆盖审计报告

## 1. 执行摘要

- **审计日期**: 2026-06-28
- **审计范围**: 34 个 crates 的测试覆盖(含 workspace 根 E2E 测试)
- **总体评价**: **良好** — 测试基础设施完善,覆盖广度优秀,但存在 1 个 Major 遗留问题与若干 Minor 改进点
- **问题数量**: Critical 0 / Major 1 / Minor 6
- **测试总量**: 约 2,940 个(crate 测试 2,817 + E2E 测试 123),较 Week 1-4 基线(~1,599)增长 83.8%

### 1.1 关键结论

| 维度 | 评价 | 依据 |
|------|------|------|
| 测试数量分布 | 良好 | 32/34 crate ≥ 20 测试,仅 decay-engine(9)、chimera-cli(15)低于阈值 |
| qeep-protocol 修复 | ✅ 已修复 | Week 1-4 Major-2 已解除:50 测试(40 单元 + 10 proptest),远超 ≥20 目标 |
| decay-engine 修复 | ❌ 未修复 | 仍为 9 测试,未达 ≥15 目标(Major 遗留) |
| 边界条件覆盖 | 中等 | 超时边界覆盖优秀(qeep-protocol),但 Duration::MAX/usize::MAX 缺失 |
| 跨周集成测试 | 良好 | 3/4 已覆盖,仅 SSRA + MLC 缺失 |
| proptest 覆盖 | 良好 | 19/34 crate 使用 proptest,语法符合项目规范 |
| stress_test | 优秀 | 1000 次压测 6 项断言全通过,p95=4ms,文档完善 |
| 测试隔离性 | 优秀 | 无全局可变状态,TempDir 隔离,无环境变量依赖 |

---

## 2. 测试数量分布统计

### 2.1 统计方法

- **工具**: Grep `#[test]` 与 `#[tokio::test]` 标记,count 模式
- **范围**: `crates/*/src/**/*.rs`(单元测试) + `crates/*/tests/**/*.rs`(集成测试)
- **排除**: `benches/**/*.rs`(criterion 基准测试不计入测试覆盖)
- **E2E**: `tests/e2e/*.rs` + `tests/security/*.rs` + `tests/stress/*.rs`(workspace 根)

### 2.2 34 crates 测试分布表

| # | Crate | 层级 | src 测试 | tests/ 测试 | 总计 | 评价 |
|---|-------|------|---------|------------|------|------|
| 1 | nexus-core | L1 | 41 | 27 | 68 | 良好 |
| 2 | event-bus | L1 | 39 | 24 | 63 | 良好 |
| 3 | model-router | L1 | 54 | 35 | 89 | 良好 |
| 4 | mlc-engine | L2 | 10 | 111 | 121 | 良好 |
| 5 | hcw-window | L2 | 58 | 69 | 127 | 良好 |
| 6 | nmc-encoder | L2 | 69 | 10 | 79 | 良好 |
| 7 | scc-cache | L3 | 45 | 17 | 62 | 良好 |
| 8 | lsct-tiering | L3 | 61 | 13 | 74 | 良好 |
| 9 | cmt-tiering | L3 | 97 | 126 | 223 | 优秀 |
| 10 | seccore | L4 | 40 | 24 | 64 | 良好 |
| 11 | qeep-protocol | L4 | 0 | 50 | 50 | 良好(已修复) |
| 12 | decay-engine | L4 | 0 | 9 | **9** | ⚠️ 薄弱 |
| 13 | repo-wiki | L5 | 50 | 25 | 75 | 良好 |
| 14 | gsoe-evolution | L5 | 69 | 11 | 80 | 良好 |
| 15 | auto-dpo | L5 | 39 | 0 | 39 | 良好 |
| 16 | osa-coordinator | L6 | 40 | 69 | 109 | 良好 |
| 17 | kvbsr-router | L6 | 48 | 69 | 117 | 良好 |
| 18 | faae-router | L6 | 37 | 17 | 54 | 良好 |
| 19 | gea-activator | L6 | 47 | 45 | 92 | 良好 |
| 20 | gqep-executor | L6 | 34 | 24 | 58 | 良好 |
| 21 | sesa-router | L6 | 76 | 14 | 90 | 良好 |
| 22 | pvl-layer | L7 | 43 | 22 | 65 | 良好 |
| 23 | mtpe-executor | L7 | 31 | 15 | 46 | 良好 |
| 24 | ssra-fusion | L7 | 42 | 13 | 55 | 良好 |
| 25 | csn-substitutor | L7 | 76 | 18 | 94 | 良好 |
| 26 | parliament | L8 | 200 | 49 | 249 | 优秀 |
| 27 | decb-governor | L8 | 89 | 19 | 108 | 良好 |
| 28 | acb-governor | L8 | 47 | 0 | 47 | 良好 |
| 29 | quest-engine | L9 | 84 | 50 | 134 | 良好 |
| 30 | gea-activator(注:已在 L6) | - | - | - | - | - |
| 31 | efficiency-monitor | L9 | 73 | 16 | 89 | 良好 |
| 32 | chimera-cli | L10 | 2 | 13 | **15** | ⚠️ 薄弱 |
| 33 | chimera-tui | L10 | 52 | 0 | 52 | 良好 |
| 34 | chtc-bridge | L10 | 58 | 1 | 59 | 良好 |
| 35 | mcp-mesh | L10 | 51 | 10 | 61 | 良好 |

> 注:#30 为表格对齐占位,实际 34 crate 见 workspace members。

**E2E 测试(workspace 根)**:

| 测试文件 | 测试数 | 用途 |
|---------|--------|------|
| `tests/e2e/week6_setup.rs` | 6 | Week 6 管线装配 |
| `tests/e2e/week6_main_flow.rs` | 6 | Week 6 主流程 |
| `tests/e2e/week6_security.rs` | 20 | Week 6 安全测试 |
| `tests/e2e/week5_event_flow.rs` | 4 | Week 5 事件流 |
| `tests/e2e/week7_setup.rs` | 4 | Week 7 管线装配 |
| `tests/e2e/week7_main_flow.rs` | 8 | Week 7 主流程 |
| `tests/e2e/week7_security.rs` | 31 | Week 7 安全测试 |
| `tests/stress/week7_stress.rs` | 5 | Week 7 压测 |
| `tests/security/owasp_top10.rs` | 20 | OWASP Top 10 渗透 |
| `tests/e2e/quest_lifecycle.rs` | 4 | Quest 生命周期 |
| `tests/e2e/full_integration.rs` | 6 | 全量集成 |
| `tests/e2e/stress_test.rs` | 1 | 1000 次压测(ignored) |
| `tests/e2e/week8_final_acceptance.rs` | 8 | 8 周验收 |
| **合计** | **123** | — |

### 2.3 薄弱 crate 识别(测试数 < 20)

| Crate | 测试数 | 问题 | 严重程度 |
|-------|--------|------|---------|
| decay-engine | 9 | L4 安全层,能力衰减引擎,测试严重不足 | Major |
| chimera-cli | 15 | L10 接口层,CLI 入口,接近阈值 | Minor |

### 2.4 与 Week 1-4 基线对比

| 项目 | Week 1-4 基线 | 当前(Week 8) | 改善 |
|------|--------------|--------------|------|
| 总测试数 | ~1,599 | ~2,940 | +83.8% |
| qeep-protocol | 8 | 50 | ✅ Major-2 已解除 |
| decay-engine | 9 | 9 | ❌ 无改善 |
| seccore | 19 | 64 | +237% |
| chimera-cli | 15 | 15 | 无变化 |

---

## 3. qeep-protocol 测试核验(Week 1-4 Major-2)

### 3.1 核验结论:✅ 已修复

- **代码位置**: `crates/qeep-protocol/tests/qeep.rs` + `crates/qeep-protocol/tests/proptest.rs`
- **测试数量**: 50 个(40 单元/集成 + 10 proptest),远超 ≥20 目标
- **src 目录测试**: 0(所有测试位于 tests/ 目录,符合集成测试组织规范)

### 3.2 测试覆盖维度

`crates/qeep-protocol/tests/qeep.rs` 的 40 个测试覆盖:

| 维度 | 测试数 | 关键测试 |
|------|--------|---------|
| 基础功能 | 8 | test_entangle_success, test_entangle_timeout, test_entangle_spawn_managed |
| 孤儿检测 | 5 | test_orphan_detection, test_orphan_detector_multiple_orphans |
| 状态/类型 | 5 | test_call_state_transitions, test_entangled_call_id_equality |
| 超时场景(SubTask 36.2) | 8 | test_timeout_1ms/100ms/1s/10s, test_timeout_boundary_just_exceeds/within |
| 孤儿检测(SubTask 36.3) | 5 | test_orphan_all_senders_dropped, test_orphan_partial_senders_dropped |
| 并发与边界(SubTask 36.4) | 7 | test_concurrent_entangled_call_10/50_threads, test_max_futures_1000 |
| 错误传播 | 2 | test_error_propagation_chain, test_error_recovery |

`crates/qeep-protocol/tests/proptest.rs` 的 10 个属性测试覆盖 9 个不变量,包括:
- 协议状态机闭合性(CallState 全变体 match 穷举)
- 超时回滚幂等性(连续超时计数守恒)
- OrphanDetector 报告累积单调性

### 3.3 核心验收测试

`crates/qeep-protocol/tests/qeep.rs:116` `test_zero_orphans_10000_ops`:10000 次操作零孤儿调用,直接对应 Claude Code 尸检中 5.4% 孤儿调用问题的验收标准。

---

## 4. decay-engine 测试核验

### 4.1 核验结论:❌ 未修复(Major 遗留)

- **代码位置**: `crates/decay-engine/tests/decay.rs`
- **测试数量**: 9 个(目标 ≥15,缺口 6 个)
- **src 目录测试**: 0

### 4.2 现有测试清单

| # | 测试名 | 行号 | 覆盖点 |
|---|--------|------|--------|
| 1 | test_capability_registration | :27 | 能力注册 |
| 2 | test_time_decay | :37 | 时间衰减 |
| 3 | test_violation_penalty | :53 | 违规惩罚 |
| 4 | test_freeze_unfreeze | :76 | 冻结/解冻(5 次循环) |
| 5 | test_auto_freeze_below_threshold | :102 | 自动冻结 |
| 6 | test_restore | :121 | 恢复 |
| 7 | test_invalid_level_rejected | :148 | 非法 level 拒绝 |
| 8 | test_continuous_decay_curve | :159 | 连续衰减曲线 |
| 9 | test_freeze_blocks_decay | :190 | 冻结阻塞衰减 |

### 4.3 缺失的测试场景

- 并发衰减(多 spawn 同时 decay)
- 错误路径(get_level/freeze/unfreeze 对未注册能力的错误传播)
- 边界值(level = 0.0、level = 1.0、severity = 0.0)
- 衰减下限保护(min_level 钳位)
- restore_rate 边界(restore 后不超过 1.0)

---

## 5. 边界条件覆盖审计

### 5.1 超时边界(Duration)

| 边界 | 覆盖状态 | 代码位置 |
|------|---------|---------|
| Duration::ZERO | ⚠️ 仅作累加器初始值,非边界测试 | `crates/mtpe-executor/tests/speedup.rs:111,120` |
| Duration::MAX | ❌ 未覆盖 | — |
| 极短超时(1ms) | ✅ 优秀 | `crates/qeep-protocol/tests/qeep.rs:495` test_timeout_1ms |
| 超时边界(刚好超时/未超时) | ✅ 优秀 | `crates/qeep-protocol/tests/qeep.rs:591,614` test_timeout_boundary_just_exceeds/within |
| 参数化多 Duration | ✅ 优秀 | `crates/qeep-protocol/tests/qeep.rs:667` test_timeout_with_different_durations |

**评价**: qeep-protocol 超时边界覆盖优秀,但全项目层面 Duration::MAX 与 Duration::ZERO(作为输入)缺失。

### 5.2 空输入

| 边界 | 覆盖状态 | 代码位置 |
|------|---------|---------|
| 空 Vec | ✅ 良好 | `crates/qeep-protocol/tests/qeep.rs:1008` test_empty_future_list |
| 空 String | ✅ 良好 | `crates/acb-governor/src/governor.rs:166,239` quest_id: String::new() |
| 空 HashMap | ⚠️ 间接覆盖 | `crates/faae-router/tests/proptest.rs:52` make_profiles 空输入 |
| 空候选集 | ✅ 良好 | `crates/csn-substitutor/tests/integration.rs:95` 未注册能力返回空候选 |

### 5.3 最大值

| 边界 | 覆盖状态 | 代码位置 |
|------|---------|---------|
| usize::MAX | ❌ 未覆盖 | — |
| u64::MAX | ⚠️ 仅作 min_latency 初始值 | `crates/parliament/tests/csa.rs:136,180` |
| u32::MAX | ⚠️ 仅作除数(生产代码) | `crates/gsoe-evolution/src/policy/grpo.rs:54`, `crates/pvl-layer/src/producer.rs:38` |
| 容量上限(1000 并发) | ✅ 良好 | `crates/qeep-protocol/tests/qeep.rs:1041` test_max_futures_1000 |
| 10000 次操作 | ✅ 优秀 | `crates/qeep-protocol/tests/qeep.rs:117` test_zero_orphans_10000_ops |

**评价**: 大规模并发覆盖良好,但原始类型最大值(usize::MAX/u64::MAX)边界测试缺失。

### 5.4 并发峰值

| 场景 | 覆盖状态 | 代码位置 |
|------|---------|---------|
| 10 线程并发 | ✅ | `crates/qeep-protocol/tests/qeep.rs:939` |
| 50 线程并发 | ✅ | `crates/qeep-protocol/tests/qeep.rs:969` |
| 100 线程并发 | ✅ | `crates/qeep-protocol/tests/qeep.rs:194` |
| 1000 线程并发 | ✅ | `crates/qeep-protocol/tests/qeep.rs:1041` |
| JoinSet 并发 | ✅ | `crates/cmt-tiering/tests/coordinator.rs:838,893` |
| 混合快慢任务 | ✅ | `crates/qeep-protocol/tests/qeep.rs:976` |
| 并发 + 部分失败 | ✅ | `crates/gqep-executor/tests/concurrent.rs:32` test_concurrent_100_partial_failure |
| 并发 + overflow 安全 | ✅ | `crates/decb-governor/tests/concurrent.rs:145` test_concurrent_record_consumption_overflow_safe |

**评价**: 并发测试覆盖优秀,多规模、多场景、含失败注入。

### 5.5 错误传播链

| 场景 | 覆盖状态 | 代码位置 |
|------|---------|---------|
| 超时 → 上层捕获 | ✅ | `crates/qeep-protocol/tests/qeep.rs:639` test_timeout_error_propagation |
| 错误 → 恢复 | ✅ | `crates/qeep-protocol/tests/qeep.rs:1071` test_error_propagation_chain |
| 错误后恢复正常 | ✅ | `crates/qeep-protocol/tests/qeep.rs:1113` test_error_recovery |
| error_paths.rs 专项 | ✅ | 12 个 crate 有 tests/error_paths.rs |
| CSA 错误路径 | ✅ | `crates/parliament/tests/csa.rs`, `crates/gea-activator/tests/csa.rs` |

**评价**: 错误传播链覆盖优秀,有专门的 error_paths.rs 测试文件。

---

## 6. 跨周集成测试核验

### 6.1 核验矩阵

| 跨周协作 | 状态 | 代码位置 | 说明 |
|---------|------|---------|------|
| Week 3 HCW + Week 4 SCC | ✅ 已覆盖 | `crates/hcw-window/tests/integration_scc.rs` | 3 个测试:压缩→缓存、稀疏化+缓存、窗口切换+缓存独立生命周期 |
| Week 5 Parliament + Week 4 PVL | ✅ 已覆盖 | `tests/e2e/full_integration.rs:187` test_parliament_consensus | L8 Parliament + L7 PVL 协作 |
| Week 6 SSRA + Week 3 MLC | ❌ **缺失** | — | ssra-fusion/tests 无 mlc-engine 引用,mlc-engine/tests 无 ssra-fusion 引用 |
| Week 7 MCP Mesh + Week 6 CHTC | ✅ 已覆盖 | `tests/e2e/week7_setup.rs:18,24`, `tests/e2e/week7_main_flow.rs:31,51` | 同时使用 chtc_bridge 与 mcp_mesh |

### 6.2 HCW × SCC 跨周集成测试详情

`crates/hcw-window/tests/integration_scc.rs` 是跨周集成测试的优秀范例:

- **test_hcw_compress_scc_cache_collaboration**(:48):HCW 压缩 → SCC 缓存 → 命中率验证
- **test_hcw_sparse_mask_scc_cache**(:114):稀疏化后 SCC 缓存不同 file_id 条目
- **test_hcw_window_switch_scc_invalidation**(:194):窗口切换不影响 SCC 缓存(独立生命周期)

### 6.3 其他跨 crate 协作测试

- `crates/gea-activator/tests/e2e.rs:44,53` 同时使用 pvl_layer 与 scc_cache(L6 + L7 + L3)
- `crates/parliament/tests/e2e.rs:30` 使用 quest_engine 的 TtgGovernor(L8 + L9)
- `tests/e2e/week8_final_acceptance.rs` 跨 L1-L10 全栈验收

### 6.4 缺失的跨周集成测试

**SSRA + MLC 协作**(Week 6 + Week 3):SSRA 黏液式适配可能需要从 MLC 四级记忆中检索历史适配模板,但当前无任何测试覆盖此协作路径。

---

## 7. proptest 覆盖核验

### 7.1 proptest 使用统计

- **使用 proptest 的 crate**: 19/34(55.9%)
- **proptest 文件数**: 19 个
- **E2E 测试**: 0 个(未使用 proptest,合理)

### 7.2 已使用 proptest 的 crate

| Crate | 文件 | 测试数 |
|-------|------|--------|
| chtc-bridge | tests/proptest.rs | 1 |
| cmt-tiering | tests/proptest.rs | 5 |
| decb-governor | tests/proptest.rs | 4 |
| faae-router | tests/proptest.rs | 8 |
| gea-activator | tests/property_tests.rs | 9 |
| gqep-executor | tests/proptest.rs | 6 |
| hcw-window | tests/proptest.rs | 5 |
| kvbsr-router | tests/proptest.rs | 5 |
| lsct-tiering | tests/proptest.rs | 5 |
| mlc-engine | tests/proptest.rs | 3 |
| mtpe-executor | tests/proptest.rs | 7 |
| osa-coordinator | tests/proptest.rs | 6 |
| parliament | tests/proptest.rs | 4 |
| pvl-layer | tests/proptest.rs | 9 |
| qeep-protocol | tests/proptest.rs | 10 |
| scc-cache | tests/proptest.rs | 7 |
| seccore | tests/proptest.rs | 4 |
| ssra-fusion | tests/proptest.rs | 6 |
| quest-engine | tests/proptest.rs | 4 |

### 7.3 未使用 proptest 的 crate(15 个)

| Crate | 层级 | 是否需要 proptest | 理由 |
|-------|------|------------------|------|
| nexus-core | L1 | ⚠️ 建议 | CLV/newtype 等核心类型适合属性测试 |
| event-bus | L1 | ⚠️ 建议 | 事件广播不变量适合属性测试 |
| model-router | L1 | ⚠️ 建议 | 路由策略选择适合属性测试 |
| nmc-encoder | L2 | 可选 | 多模态编码,属性测试价值有限 |
| csn-substitutor | L7 | ⚠️ 建议 | 相似度计算适合属性测试 |
| decay-engine | L4 | ⚠️ 建议 | 衰减曲线单调性适合属性测试(且测试薄弱) |
| efficiency-monitor | L9 | 可选 | 监控指标,属性测试价值有限 |
| gsoe-evolution | L5 | ⚠️ 建议 | 进化策略适合属性测试 |
| auto-dpo | L5 | 可选 | 偏好对生成,属性测试价值有限 |
| chimera-cli | L10 | 不需要 | CLI 解析,属性测试不适用 |
| chimera-tui | L10 | 不需要 | TUI 渲染,属性测试不适用 |
| mcp-mesh | L10 | 可选 | 量子网格,属性测试价值有限 |
| sesa-router | L6 | ⚠️ 建议 | 稀疏激活适合属性测试 |
| repo-wiki | L5 | 可选 | Wiki 生成,属性测试价值有限 |
| acb-governor | L8 | ⚠️ 建议 | 预算计算适合属性测试 |

### 7.4 proptest 语法核验

项目规范使用 `proptest!` 宏的块状命名语法 `fn test_name(x in 0..100u32)`,而非闭包形式 `|x in 0..100|`。

核验结果:✅ **全部符合规范**

- `crates/qeep-protocol/tests/proptest.rs:51` `fn test_entangle_succeeds_with_positive_timeout(timeout_ms in 1u64..10000)`
- `crates/qeep-protocol/tests/proptest.rs:204` `fn test_call_state_machine_closure(state_idx in 0u8..5)`
- `crates/qeep-protocol/tests/proptest.rs:253` `fn test_timeout_rollback_idempotent(k in 1u32..8)`

所有 proptest 均使用 `#[test]` + `tokio::runtime::Runtime::new()` 模式(因 `proptest!` 宏不兼容 `#[tokio::test]`),并在注释中说明原因(如 `crates/qeep-protocol/tests/proptest.rs:22-23`)。

---

## 8. stress_test 核验

### 8.1 `#[ignore]` 标记测试清单

| 文件 | 测试名 | 类型 | ignore 文档 |
|------|--------|------|------------|
| `tests/e2e/stress_test.rs:68` | test_stress_1000_iterations | 1000 次全链路压测 | ✅ `#[ignore]` + 注释说明 `--ignored` |
| `crates/qeep-protocol/tests/qeep.rs:567` | test_timeout_10s | 长超时测试 | ✅ `#[ignore = "slow: run with --ignored"]` |
| `crates/mtpe-executor/tests/speedup.rs:38,75,105` | 3 个 speedup 测试 | 性能加速比 | ✅ `#[ignore]` + 注释说明 |
| `crates/kvbsr-router/tests/scale.rs:96,132` | 2 个 scale 测试 | 1000 工具规模 | ✅ `#[ignore = "perf: run with --ignored"]` |
| `crates/cmt-tiering/tests/coordinator.rs:698` | test_run_decay_cycle_batch_benchmark | 批量衰减基准 | ✅ `#[ignore = "perf: run with --ignored"]` |
| `crates/cmt-tiering/tests/warm.rs:266` | test_warm_query_latency_under_10ms | 延迟基准 | ✅ `#[ignore = "perf: run with --ignored"]` |
| `crates/cmt-tiering/tests/cold.rs:234,271` | 2 个 cold 基准 | 延迟/索引基准 | ✅ `#[ignore = "perf: run with --ignored"]` |
| `crates/decb-governor/tests/concurrent.rs:269,303,336` | 3 个 perf 测试 | 性能断言 | ✅ `#[ignore = "perf: run with --ignored"]` |
| `crates/csn-substitutor/tests/integration.rs:431,462` | 2 个 perf 测试 | 替代延迟 | ✅ `#[ignore = "perf: run with --ignored"]` |
| `crates/efficiency-monitor/benches/monitor_benchmark.rs:224,253,281` | 3 个 perf 测试(注:在 benches 目录) | 监控延迟 | ✅ `#[ignore = "perf: run with --ignored"]` |

### 8.2 1000 次压测核验

**测试位置**: `tests/e2e/stress_test.rs:68` test_stress_1000_iterations

**核验结论**: ✅ **合理且已验证**

| 维度 | 设计 | 实测 | 评价 |
|------|------|------|------|
| 迭代次数 | 1000 | 1000 | ✅ 符合 |
| 测试耗时 | — | 3.39s | ✅ 合理 |
| p95 延迟 | < 2000ms | 4ms | ✅ 远低于阈值(500× 余量) |
| 最大延迟 | < 2000ms | 29ms | ✅ 远低于阈值 |
| 首次 vs 末次退化 | < 50% | 0.00%(last < first) | ✅ 无退化 |
| Arc 引用泄漏 | strong_count = 1 | 1 | ✅ 无泄漏 |
| Wiki 累积 | 3000 条 | 3000 条 | ✅ 正确 |

**三重替代验证方案**(因 `#![forbid(unsafe_code)]` 无法实现 GlobalAlloc):
1. Arc strong_count 探针
2. 延迟稳定性(首次 vs 末次)
3. 资源可重建性(1000 次后仍能创建新管线)

**文档支撑**: `docs/performance/week8_stress_test_report.md`(8 章节详细报告)

### 8.3 性能测试时序敏感性(Minor-3)

**测试位置**: `crates/kvbsr-router/tests/scale.rs:133` test_scale_speedup_vs_full_scan

**核验结论**: ⚠️ 阈值已降至 2.0×

- **代码位置**: `crates/kvbsr-router/tests/scale.rs:166` `assert!(speedup > 2.0, ...)`
- **理论加速比**: ~9×(1000 工具全量扫描 vs 两级路由 110 次)
- **实际阈值**: 2.0×(原 3.0×,任务建议 5.0×)
- **降低原因**(注释 `crates/kvbsr-router/tests/scale.rs:125-130,162-164`):
  - 1000 工具规模下两级路由固定开销占比高
  - workspace 整体测试时资源竞争加剧亚毫秒级测量噪声
  - 实测 min-of-10 约 2.5-4×
  - > 2.0× 验证核心价值同时消除 flake

**评价**: 阈值降低有详细注释说明,采用了 min-of-10 减少噪声,但 2.0× 阈值相对理论值 9× 偏低,长期建议提升至 3.0×。

---

## 9. 测试隔离性审计

### 9.1 共享状态(全局变量)

**核验结论**: ✅ **优秀**

- 搜索 `static mut` / `lazy_static` / `once_cell::sync::Lazy`:**未发现全局可变状态**
- 所有共享状态均通过实例级的 `Arc<RwLock<T>>` 或 `Arc<Mutex<T>>` 保护
- 示例:`crates/hcw-window/src/window.rs:90` `state: Arc<RwLock<HcwState>>`(实例级,非全局)
- 示例:`crates/faae-router/src/router.rs:74` `expert_registry: Arc<RwLock<HashMap<...>>>`(实例级)

### 9.2 文件系统依赖

**核验结论**: ✅ **良好**

- **使用 TempDir 的测试**:
  - `crates/cmt-tiering/tests/cold.rs:158` `tempfile::tempdir()`
  - `crates/cmt-tiering/tests/warm.rs:162,183`
  - `crates/cmt-tiering/tests/migrator.rs:37,336,551`
  - `crates/chimera-cli/tests/cli.rs:12` `use tempfile::TempDir`
  - `tests/e2e/stress_test.rs:76` `TempDir::new()`
- **评价**: 每个测试使用独立临时目录,并行执行安全

### 9.3 环境变量依赖

**核验结论**: ✅ **优秀**

- **测试代码**: 未发现 `std::env::var` 在测试中的使用
- **生产代码**: `crates/chimera-cli/src/config.rs:815-816` 使用 `std::env::var("HOME")` / `USERPROFILE`(配置加载,非测试)
- **E2E 测试**: 未发现环境变量依赖

### 9.4 时序依赖

**核验结论**: ⚠️ **中等(有降低阈值的测试)**

- **时序敏感测试**:
  - `crates/decay-engine/tests/decay.rs:42` `std::thread::sleep(Duration::from_millis(50))` 确保时间流逝
  - `crates/qeep-protocol/tests/qeep.rs:91,98` `tokio::time::sleep(Duration::from_millis(10/50))` 等待任务注册
  - `crates/kvbsr-router/tests/scale.rs:166` 加速比阈值降至 2.0×
- **缓解措施**: qeep-protocol 使用确定性的 abort + sleep 组合,而非纯时序竞争
- **风险**: 在高负载 CI 环境下,sleep 10ms 可能不足以确保任务注册,导致偶发失败

### 9.5 并行执行安全性

**核验结论**: ✅ **良好**

- 无全局可变状态 → 无数据竞争
- TempDir 隔离 → 无文件系统冲突
- 每个测试创建独立 EventBus/实例 → 无跨测试状态污染
- `crates/qeep-protocol/tests/qeep.rs:677` 注释明确:"每次创建新协议,保证测试隔离"

---

## 10. 问题清单

| ID | 严重程度 | 问题 | 代码位置 | 修复建议 |
|----|---------|------|---------|---------|
| E-MAJOR-1 | Major | decay-engine 测试严重不足(9 个,目标 ≥15),Week 1-4 遗留问题未修复 | `crates/decay-engine/tests/decay.rs` | 补充 6+ 测试:并发衰减、错误路径、边界值(min_level 钳位)、restore 上限、severity=0、未注册能力错误传播 |
| E-MINOR-1 | Minor | chimera-cli 测试数(15)接近阈值,缺乏 proptest | `crates/chimera-cli/tests/cli.rs` | 可不修复(CLI 解析属性测试价值有限),但建议补充子命令解析边界测试至 ≥20 |
| E-MINOR-2 | Minor | 跨周集成测试 SSRA + MLC 协作缺失 | `crates/ssra-fusion/tests/`、`crates/mlc-engine/tests/` | 新增 `crates/ssra-fusion/tests/integration_mlc.rs`,验证 SSRA 从 MLC 检索历史适配模板 |
| E-MINOR-3 | Minor | test_scale_speedup_vs_full_scan 阈值降至 2.0×(理论 9×) | `crates/kvbsr-router/tests/scale.rs:166` | 长期目标提升至 3.0×,当前注释说明合理可接受 |
| E-MINOR-4 | Minor | Duration::MAX / usize::MAX 边界测试全项目缺失 | — | 在 qeep-protocol 与 gqep-executor 补充 Duration::MAX 超时不触发(或立即触发)的边界测试 |
| E-MINOR-5 | Minor | 15 个 crate 未使用 proptest,其中 8 个下层 crate(L1-L8)建议引入 | 见 §7.3 | 优先为 nexus-core、decay-engine、event-bus、model-router 引入 proptest |
| E-MINOR-6 | Minor | decay-engine 测试使用 `std::thread::sleep` 而非 `tokio::time::sleep`,在 async 测试中可能阻塞运行时 | `crates/decay-engine/tests/decay.rs:42,129,198` | 若 decay-engine 提供 async API,改用 `tokio::time::sleep` |

---

## 11. 长期主义建议

### 11.1 短期(下一周)

1. **修复 E-MAJOR-1**:为 decay-engine 补充 6 个测试至 ≥15 个,覆盖并发衰减、错误路径、边界值。这是 Week 1-4 遗留问题,应优先解除。

2. **修复 E-MINOR-2**:新增 SSRA × MLC 跨周集成测试,参考 HCW × SCC 的测试模式(`crates/hcw-window/tests/integration_scc.rs`)。

### 11.2 中期(下一月)

3. **proptest 下沉**:为 L1-L4 下层 crate(nexus-core、event-bus、model-router、decay-engine)引入 proptest,验证核心不变量(如 CLV 维度守恒、EventBus 消息顺序、衰减单调性)。

4. **边界值专项**:在全项目范围补充 Duration::MAX、usize::MAX、容量上限边界的测试,确保系统在极端输入下不 panic。

### 11.3 长期(下一季度)

5. **性能测试阈值提升**:随着测试环境稳定,逐步提升 `test_scale_speedup_vs_full_scan` 阈值从 2.0× → 3.0× → 5.0×,逼近理论值 9×。

6. **测试覆盖率工具集成**:引入 `cargo-tarpaulin` 或 `cargo-llvm-cov` 量化行覆盖率,目标 ≥ 80%,识别未覆盖的分支。

7. **变异测试**:引入 `cargo-mutants` 验证测试有效性,确保测试能捕获代码变异(防止"绿色但无效"的测试)。

### 11.4 架构红线对齐

当前测试体系已良好对齐 §6 架构红线:

| 红线 | 测试覆盖 | 评价 |
|------|---------|------|
| 单函数 ≤200 行 | 不适用(测试函数可超) | — |
| 所有异步操作有 GQEP 聚集/超时 | qeep-protocol 50 测试 + gqep-executor 58 测试 | ✅ |
| 所有外部调用经 SecCore 沙箱 | seccore 64 测试 + OWASP Top 10 渗透 | ✅ |
| 所有 async 必须 await 或 spawn 管理 | qeep-protocol 零孤儿测试(10000 次) | ✅ |
| 必须经 HCW 分层 + OSA 稀疏化 | hcw-window 127 测试 + osa-coordinator 109 测试 | ✅ |

---

**审计人**: 测试覆盖审计子代理(GLM-5.2)
**审计耗时**: 单次扫描 + 抽样核验
**置信度**: 高(所有结论引用具体代码位置)
