# ADR-178: StrategyCapGuard 生产激活(推理悖论主动降级闭环,config 门控)

- **状态**: Accepted（2026-09-03，架构重构批次2 Task 3 实施落地）
- **决策者**: 架构批次 E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-063（L8 协调度量接线闭环 + 推理悖论红线策略封顶;3 观测事件 + StrategyCapGuard）
  - ADR-176 E 决策 3（StrategyCapGuard 激活列为独立批次）
  - `crates/parliament/src/strategy_cap.rs`（observe 状态机 + spawn_strategy_cap_subscriber）
  - `crates/chimera-cli/src/experience_loop.rs`（组合根挂载）
  - `crates/nexus-core/src/config.rs` / `crates/chimera-cli/src/config.rs`（开关字段）

> **编号说明**: 承接 ADR-177 顺延为 **ADR-178**。

## 背景

批次1(ADR-176 E)确认悖论测量闭环的"ⓐ 主动行为闭环缺失":`spawn_strategy_cap_subscriber` 已实现(订阅 CoordinationRatioReported → `StrategyCapGuard::observe()` → 封顶变化发布 `ParliamentStrategyCapChanged`),但全库仅测试调用,生产组合根未挂载。批次2 完成激活,默认关(影子模式)保证零行为变更。

## 决策(实际实施形态)

1. **config 开关**:`nexus-core` `ChimeraConfig::enable_strategy_cap: bool`(默认 `false`,`#[serde(default)]`);`chimera-cli` `LazyConfig` 增加 getter(figment `extract_inner`)并在 `to_chimera_config` 同步。默认 off = 与激活前完全一致。
2. **组合根挂载**:`experience_loop::spawn_experience_loop` 增加形参 `enable_strategy_cap: bool`;在 metrics subscriber 挂载后,`if enable_strategy_cap { join_handles.push(parliament::spawn_strategy_cap_subscriber(Arc::clone(&guard), nexus_bus.clone())) }` —— JoinHandle 入 `join_handles`(反模式 #7 合规,无 fire-and-forget)。`commands/tui.rs` 传入 config 值;既有 E2E 装配点补 `false`(签名扩展最小编译改动)。
3. **测试**:`test_config_default_strategy_cap_off`(默认关)、`strategy_cap_guard_fires_when_enabled`(3 次越阈 → full→simplified 观测事件)、`strategy_cap_guard_inert_when_disabled`(默认 off 无副作用);滞后带/降档/驻留边界复用 `strategy_cap.rs` 既有状态机测试,不重复造。
4. **不新增事件变体、不升 Critical**:复用既有 CoordinationRatioReported(入口)+ ParliamentStrategyCapChanged(出口);event-bus 双清单(13)不动。

## 否决项

- **否决默认启用(强制全量部署)**:影子优先(默认 off)使风险可逆;成熟后经观测期再翻转为默认,需另行 ADR。
- **否决修改 strategy_cap.rs 核心**:状态机已完整且有测试,仅接线不重构(防过度工程)。

## 后果

- **正向**: 推理悖论"主动降级闭环"首次生产可用(选择开启时);策略按封顶 min 收敛可在观测事件流中追溯。
- **无负向**: 默认行为零变更;E2E(experience_loop_closure_e2e / coordination_metrics_closure_e2e)回归 0 失败。

## 验证(实测 2026-09-03)

- [x] `cargo check --workspace` EXIT=0(44s)
- [x] `cargo test -p parliament` 全绿;`cargo test -p chimera-cli --lib` 112 passed / 0 failed
- [x] `cargo test --test coordination_metrics_closure_e2e` 4 / 0
- [x] `cargo clippy -p chimera-cli -p parliament --all-targets --jobs 2 -- -D warnings` EXIT=0(修正 clippy::match-like-matches-macro)
- [x] `cargo fmt -p chimera-cli -p parliament -- --check` EXIT=0
- [x] 未改 strategy_cap.rs 核心;未新增事件;用户 dirty 未触碰