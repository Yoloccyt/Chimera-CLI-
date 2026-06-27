# Fix Week 8 Carryover Spec

## Why

Week 7 验收通过(119/120 检查项,99.17%),遗留 6 项 Minor 级结转项(W7-Carryover-1\~5 + Task 10.9 cargo doc warnings)。本 spec 将这 6 项正式化为可执行任务,在进入 Week 8 主体规划前先消除技术债。

## What Changes

* **W7-Carryover-1**:验证 KVBSR+SESA+FaaE 三层路由组合 p95 ≤ 2ms(若 KVBSR/FaaE 基准不可用,补全基准并运行联调)

* **W7-Carryover-2**:修复 mcp-mesh criterion 基准 mock 服务器不可达 panic(`mesh_benchmark.rs` 第 53-58 行 `execute_transaction` 因 mock 服务器无心跳响应而 panic)

* **W7-Carryover-3**:WAL 真实持久化 — 评估 `SqliteWal` 实现可行性(rusqlite unsafe 传播验证),若不可行则强化 `InMemoryWal` 为崩溃恢复模拟实现

* **W7-Carryover-4**:`crates/chimera-cli/src/main.rs` 添加 `#![forbid(unsafe_code)]` 声明(1 行修复)

* **W7-Carryover-5**:创建 Grafana 仪表盘 JSON 配置文件(对接 efficiency-monitor `/metrics` 端点)

* **W7-Carryover-6**(Task 10.9):修复 `cargo doc --workspace` 5 个 warnings(seccore/asa.rs 2 个 + nexus-core 3 个)

## Impact

* Affected specs: `week7-mesh-monitoring-integration`(结转清单清零)

* Affected code:

  * `crates/chimera-cli/src/main.rs`(W7-Carryover-4)

  * `crates/mcp-mesh/benches/mesh_benchmark.rs`(W7-Carryover-2)

  * `crates/scc-cache/src/wal.rs`(W7-Carryover-3,可能新增 `sqlite_wal.rs`)

  * `crates/seccore/src/asa.rs`(W7-Carryover-6)

  * `crates/nexus-core/src/lib.rs` 或子模块(W7-Carryover-6)

  * `docs/grafana/dashboard.json`(W7-Carryover-5,新建)

  * `tests/e2e/week8_carryover_verification.rs`(新增验证测试)

## ADDED Requirements

### Requirement: 三层路由组合基准验证

系统 SHALL 提供 KVBSR+SESA+FaaE 三层路由组合的 criterion 基准,在 1000 工具规模下 p95 ≤ 2ms。

#### Scenario: 三层路由联调

* **WHEN** 运行 `cargo bench -p sesa-router --bench router_benchmark -- --ignored` 或新增的 `three_layer_routing` 基准

* **THEN** 输出 p95 延迟,若 ≤ 2ms 则通过;若 KVBSR/FaaE 基准不可用,标注为 Week 8 主体任务

### Requirement: MCP Mesh 基准 mock 修复

系统 SHALL 在 `mesh_benchmark.rs` 中提供可运行的 mock 服务器,使 `cargo bench -p mcp-mesh` 不再 panic。

#### Scenario: 基准运行

* **WHEN** 运行 `cargo bench -p mcp-mesh --bench mesh_benchmark`

* **THEN** 基准完成 50 次采样,无 panic,输出 p50/p95/p99 延迟

### Requirement: main.rs forbid 声明

`crates/chimera-cli/src/main.rs` SHALL 在文件顶部声明 `#![forbid(unsafe_code)]`。

#### Scenario: 编译时检查

* **WHEN** `cargo check -p chimera-cli`

* **THEN** 编译通过且 forbid 属性生效

### Requirement: cargo doc 零 warnings

`cargo doc --workspace --no-deps` SHALL 输出 0 warnings。

#### Scenario: 文档构建

* **WHEN** 运行 `cargo doc --workspace --no-deps --jobs 1`

* **THEN** exit 0 且无 warning 输出

## MODIFIED Requirements

### Requirement: WAL 持久化(W7-Carryover-3)

原 Week 7 Task 9.2 提供 `InMemoryWal` 占位实现。本周评估 `SqliteWal` 可行性:

* 若 rusqlite 的 `#![forbid(unsafe_code)]` 兼容(ffi 模块隔离),实现 `SqliteWal`

* 若不兼容,强化 `InMemoryWal` 为"崩溃恢复模拟"(添加 `simulate_crash_recovery()` 测试接口),并在 spec.md 记录决策

## REMOVED Requirements

无

## 范围约束

* 本 spec 仅处理 6 项结转,不涉及 Week 8 主体任务(性能/安全/文档/发布打磨)

* 每项任务工作量 ≤ 8 小时

* 修复完成后更新 `week7-mesh-monitoring-integration/checklist.md` 对应项为 ✅

