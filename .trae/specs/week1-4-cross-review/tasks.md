# Week 1-4 横向复审问题修复 — 执行计划

> 基于 [review-report.md](file:///d:/Chimera%20CLI/.trae/specs/week1-4-cross-review/review-report.md) 的 12 个问题,按优先级分两波执行。

---

## Wave 1: P0 修复 (Major 级)

### Task F1: Major-1 伪实现标记 (3 文件, ~5 分钟)

**问题**: MTPE 伪预测、FaaE 伪随机、RepoWiki 占位嵌入无 TODO 追踪标记。

**修复方案**: 在已有 WHY 注释旁添加 `// TODO(Week 6-7)` 标记,不影响代码行为。

**修改文件**:
1. [mtpe-executor/src/predictor.rs](file:///d:/Chimera%20CLI/crates/mtpe-executor/src/predictor.rs#L29)
   - 行 29: `const SIMULATED_INFERENCE_DELAY` — 添加 `// TODO(Week 7): 替换为真实模型推理延迟`
   - 行 246: `fn generate_pseudo_predictions` — 添加 `// TODO(Week 7): 替换为真实模型多步预测`

2. [faae-router/src/edsb.rs](file:///d:/Chimera%20CLI/crates/faae-router/src/edsb.rs#L343)
   - 行 343: `fn pseudo_random_probability` — 添加 `// TODO(Week 8): 评估替换为 rand crate 真随机`

3. [repo-wiki/src/generator.rs](file:///d:/Chimera%20CLI/crates/repo-wiki/src/generator.rs#L66)
   - 行 66: `fn placeholder_embedding` — 添加 `// TODO(Week 6): NMC 编码器实现后替换为真实 CLV 嵌入`

**验证**: `cargo check -p mtpe-executor -p faae-router -p repo-wiki`

---

### Task F2: Major-2 qeep-protocol 测试补充 (1 文件, ~30 分钟)

**问题**: qeep-protocol 仅 8 个测试,不足以覆盖所有边界条件。

**修复方案**: 在现有 [tests/qeep.rs](file:///d:/Chimera%20CLI/crates/qeep-protocol/tests/qeep.rs) 中添加 12+ 个新测试,总测试数 ≥20。

**新增测试**(12 个):
1. `test_orphan_detector_clear` — 验证 OrphanDetector::clear() 清空孤儿报告
2. `test_orphan_detector_multiple_orphans` — 验证多个孤儿调用的检测与计数
3. `test_call_state_transitions` — 验证 CallState 枚举完整性
4. `test_entangled_call_id_equality` — 验证 EntangledCallId 的 Eq/Hash
5. `test_orphan_report_fields` — 验证 OrphanReport 字段完整性
6. `test_error_display` — 验证所有 QeepError 变体的 Display 实现
7. `test_default_timeout_constant` — 验证 DEFAULT_TIMEOUT = 30s
8. `test_entangle_error_propagation` — 验证错误传播链
9. `test_orphan_detector_default` — 验证 OrphanDetector::default()
10. `test_concurrent_entangle_spawn` — 并发 entangle_spawn 无竞态
11. `test_pending_count_after_abort` — abort 后 pending_count 归零
12. `test_orphan_report_reason` — 验证孤儿报告原因字符串

**验证**: `cargo test -p qeep-protocol`

---

## Wave 2: P1 修复 (Minor 级)

### Task F3: Minor-3 硬编码常量配置化 (4 文件, ~20 分钟)

**问题**: HCW 压缩器权重(0.4/0.3/0.3)和 GEA 缓存 TTL(5s)硬编码。

**修复方案**:
- HCW: 在 HcwConfig 添加 `compressor_weights` 字段,compressor 从配置读取
- GEA: 在 GeaConfig 添加 `cache_ttl_secs` 字段,activator 从配置读取

**修改文件**:
1. [hcw-window/src/types.rs](file:///d:/Chimera%20CLI/crates/hcw-window/src/types.rs) — HcwConfig 添加字段
2. [hcw-window/src/config.rs](file:///d:/Chimera%20CLI/crates/hcw-window/src/config.rs) — Default 实现 + validate
3. [hcw-window/src/compressor.rs](file:///d:/Chimera%20CLI/crates/hcw-window/src/compressor.rs) — 从配置读取权重
4. [gea-activator/src/config.rs](file:///d:/Chimera%20CLI/crates/gea-activator/src/config.rs) — GeaConfig 添加 cache_ttl_secs
5. [gea-activator/src/activator.rs](file:///d:/Chimera%20CLI/crates/gea-activator/src/activator.rs) — 从配置读取 TTL

**验证**: `cargo check -p hcw-window -p gea-activator && cargo test -p hcw-window -p gea-activator`

---

### Task F4: Minor-4 HCW get_arc() 优化 (1 文件, ~10 分钟)

**问题**: `HcwWindow::get()` 返回 `Option<ContextEntry>`,热路径 clone 开销。

**修复方案**: 添加 `get_arc()` 方法返回 `Option<Arc<ContextEntry>>`,保持原 `get()` 不变(向后兼容)。

**修改文件**:
1. [hcw-window/src/window.rs](file:///d:/Chimera%20CLI/crates/hcw-window/src/window.rs#L157) — 添加 `get_arc()` 方法

**验证**: `cargo check -p hcw-window && cargo test -p hcw-window`

---

### Task F5: Minor-5 跨周集成测试 (1 新文件, ~20 分钟)

**问题**: 缺少 HCW(Week 3) + SCC(Week 4) 跨周协作测试。

**修复方案**: 新建集成测试,验证 HCW 压缩后 SCC 缓存的协作流程。

**新建文件**:
1. `crates/hcw-window/tests/integration_scc.rs` — HCW + SCC 跨周集成测试

**验证**: `cargo test -p hcw-window`

---

### Task F6: Minor-7 CHANGELOG 同步 (1 文件, ~5 分钟)

**问题**: CHANGELOG.md 未记录 Week 1-4 横向复审结果。

**修复方案**: 在 CHANGELOG.md 追加 "Week 1-4 横向深度复审" 章节。

**修改文件**:
1. [CHANGELOG.md](file:///d:/Chimera%20CLI/CHANGELOG.md) — 添加复审章节

**验证**: 文件格式正确,内容完整

---

## 验证阶段

### Task V1: 全量验证

```powershell
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings 2>&1
```

### Task R1: 修复报告

生成 [fix-report.md](file:///d:/Chimera%20CLI/.trae/specs/week1-4-cross-review/fix-report.md)