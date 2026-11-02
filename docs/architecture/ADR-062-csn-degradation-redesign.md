# ADR-062: CSN 降级链重设计 — Level N 与候选选择关联

## 状态

Proposed — 2026-07-30

## 上下文

`csn-substitutor`（L10 Interface）在 v2.8.0-omega 及之前的降级链实现存在一个核心设计缺陷：**降级链层级与替代候选选择完全脱钩**。

### 问题根因

`trigger_substitution` 在原始能力不可达时被调用,其内部流程为：
1. 调用 `advance_or_create_chain` 推进或创建降级链（返回 level N）
2. 调用 `find_substitutes(original_id, top_k)` 查找候选

**缺陷**：第 2 步的 `top_k` 固定为 `config.top_k`（默认 5），与 level N 完全无关。无论降级链推进到 level 0 / 1 / 2，`find_substitutes` 始终返回 Top-K 候选中的第 1 个（`candidates[0]`），导致：
- 降级链推进后，替代候选不变化（始终是 Top-1）
- 降级链失去意义——"逐级回退"实际只是重复选择同一候选
- MCP 失败重试时无法获得不同的替代方案

### 配置类型缺陷

`default_degradation_levels` 类型为 `Vec<String>`，例如 `["primary", "secondary", "tertiary"]`。这些字符串仅是层级名称，不携带候选 ID 信息，无法表达"level 1 应使用显式指定的 cap-X"的语义。

### 其他相关问题

1. **无相似度阈值过滤**：`find_substitutes` 返回所有 Top-K 候选，包括相似度极低（如 0.1）的"伪替代"，可能导致功能不可用
2. **无向量维度校验**：`register_capability` 不校验 `semantic_vector.len()` 与 `config.vector_dimension` 的一致性，维度不匹配时余弦相似度计算取 min 长度，产生静默错误
3. **MCP 失败推进误伤**：MCP 事务失败事件无 `capability_id` 字段时，`start_degradation_listener` 推进所有降级链，可能推进无关的链
4. **无 TTL 清理**：降级链集合无自动清理机制，长期运行会积累过期链导致内存泄漏
5. **ChainExhausted 后链未移除**：降级链耗尽后仍保留在 `chains` 中，占用内存且后续 `advance_degradation` 仍返回错误

### 约束

- `#![forbid(unsafe_code)]` 合规
- 依赖铁律：L10 → L1 单向依赖，不引入新 crate
- 向后兼容：旧配置格式（字符串数组）需平滑迁移
- 性能：降级链推进延迟 < 1µs（SubTask 0.5.11 验收标准）
- 架构红线：降级链深度 ≥ 3 级

## 决策

重设计降级链机制，将 level N 与候选选择关联，并修复 5 个相关问题。

### 1. 配置类型升级：`Vec<String>` → `Vec<Vec<String>>`

`default_degradation_levels` 类型从 `Vec<String>` 改为 `Vec<Vec<String>>`：

```rust
pub default_degradation_levels: Vec<Vec<String>>,
```

**语义**：
- 外层 Vec 长度 = 降级深度（≥ 3 级，架构红线）
- 内层 Vec 为该级的显式候选 ID 列表：
  - **空 Vec** → 触发 Top-(N+1) 模式（level 0 返回 Top-1，level 1 返回 Top-2 的第 2 个，...）
  - **非空 Vec** → 按显式 ID 列表查找该级的替代候选

**默认值 `[[], [], []]`**：3 级降级，每级用 Top-(N+1) 模式。

### 2. 向后兼容反序列化

使用 serde `untagged` enum 自动识别新旧格式：

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum LevelsFormat {
    New(Vec<Vec<String>>),
    Legacy(Vec<String>),
}
```

旧格式 `["primary", "secondary", "tertiary"]` 自动转换为 `[[], [], []]`（每级空 Vec，触发 Top-K 模式）。旧字符串仅是层级名称，不影响候选选择，与新设计的空 Vec 语义等价。

**兼容期**：2 个版本周期（v2.9 - v2.10），v2.11 起移除旧格式支持。

**WHY 2 个版本周期**：参考 ADR-060 ExitCode 矩阵的 BREAKING 处理模式（`--legacy-exit-code` flag + 2 版本迁移），给部署方足够时间迁移配置文件。

### 3. Level N 与候选选择关联（核心重设计）

`trigger_substitution` 重设计为根据 level N 选择候选：

```rust
pub async fn trigger_substitution(&self, original_id: &str) -> Result<SubstitutionCandidate, CsnError> {
    let level = self.advance_or_create_chain(original_id)?;
    let candidate = self.select_candidate_for_level(original_id, level as usize)?;
    self.publish_substitution(original_id, &candidate, level).await;
    Ok(candidate)
}
```

`select_candidate_for_level` 选择逻辑：
1. 若 `default_degradation_levels[level_idx]` 非空 → 按显式 ID 列表查找
2. 否则 → Top-(N+1) 模式：level 0 → Top-1 第 1 个，level 1 → Top-2 第 2 个，...

**候选不足处理**：Top-(N+1) 模式下若候选数 < N+1，返回最后一个可用候选（避免降级链提前终止）。

**WHY Top-(N+1) 而非 Top-K[N]**：每次推进应返回不同的候选。Top-(N+1) 的第 N+1 个是新候选，确保降级链推进时候选确实变化。若用 Top-K[N]（返回第 N 个），level 0 会返回 Top-K 的第 0 个（即 Top-1），与 Top-(N+1) 一致；但 level 1 返回 Top-K 的第 1 个（即 Top-2 的第 1 个），与 Top-(N+1) 的"第 2 个"不同——Top-(N+1) 模式确保每次推进都向下取一个新候选，语义更清晰。

### 4. 相似度阈值过滤

新增 `similarity_threshold: f32` 字段（默认 0.5），`find_substitutes` 过滤低于阈值的候选：

```rust
pub fn find_substitutes(&self, capability_id: &str, top_k: usize) -> Vec<SubstitutionCandidate> {
    let mut candidates = self.registry.find_substitutes(capability_id, top_k);
    let threshold = self.config.similarity_threshold;
    candidates.retain(|c| c.similarity_score >= threshold);
    candidates
}
```

**WHY f32 直接比较**：与 `similarity_score: f32` 类型一致，避免 §4.4 #6 f32→f64 精度膨胀问题。

**取值范围 [0.0, 1.0]**，`config.validate()` 校验边界。

### 5. 向量维度校验

`register_capability` 新增维度校验：

```rust
pub fn register_capability(&self, cap: CapabilityDescriptor) -> Result<(), CsnError> {
    if cap.semantic_vector.len() != self.config.vector_dimension {
        return Err(CsnError::InvalidCapability {
            reason: format!("语义向量维度不匹配:期望 {},实际 {}", ...),
        });
    }
    self.registry.register(cap)
}
```

**WHY 配置层校验而非注册表层**：注册表本身支持运行时动态维度调整，但 CsnSubstitutor 绑定固定 `vector_dimension`，应在配置层拦截不一致，而非运行时静默错误。

### 6. MCP 失败精准推进

利用 Task 0.7 新增的 `McpMeshTransactionCompleted.capability_id: Option<String>` 字段，实现精准推进：

```rust
let target_chain_id = capability_id.or_else(|| {
    tx_map.get(&transaction_id).map(|r| r.value().clone())
});

if let Some(chain_id) = target_chain_id {
    // 精准推进:只推进该 chain_id 的链
} else {
    // 向后兼容:capability_id 为 None 且无映射时,推进所有链
}
```

**三级回退策略**：
1. 优先用 `capability_id`（若 Some）作为 chain_id 推进
2. 否则用 `transaction_id` → chain_id 映射（`record_transaction` 记录）
3. 都无 → 推进所有链（向后兼容 v2.8.0 行为）

**WHY 保留向后兼容**：旧版 mcp-mesh 不填充 `capability_id`，保持兼容避免破坏现有部署。

### 7. TTL 清理 + ChainExhausted 自动移除

`DegradationChain` 新增 `last_advanced_at: Instant` 字段（`#[serde(skip)]`，手动实现 `PartialEq` 忽略此字段）：

```rust
pub fn is_expired(&self, ttl: Duration) -> bool {
    self.last_advanced_at.elapsed() > ttl
}
```

`CsnSubstitutor::cleanup_chains(ttl: Duration) -> usize` 按需清理过期链。

`advance_degradation` 在 `ChainExhausted` 时自动移除链：

```rust
if matches!(result, Err(CsnError::ChainExhausted { .. })) {
    self.chains.remove(chain_id);
}
```

**WHY 显式 cleanup_chains 而非后台任务**：遵循 §4.4 #7 fire-and-forget 评估框架，TTL 清理是幂等操作，但显式调用更可控（避免后台任务 panic 影响数据一致性）。建议调用方周期性调用（如每小时一次）。

### 8. 层级语义统一（spec.md C13 修复）

`degradation_chain.rs` 注释统一为：
- Level 0: 首次降级（primary substitute）— 原始能力不可达时的第一级替代
- Level 1: 次级降级（secondary substitute）— primary 替代失败后的回退
- Level 2: 末级降级（tertiary substitute）— secondary 替代失败后的回退

**WHY 统一语义**：原文档将 level 0 描述为"原始能力"，但实际 `trigger_substitution` 在原始能力不可达时才触发，首次调用即处于 level 0（已降级到 primary substitute）。

## 影响

### 受影响代码

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `crates/csn-substitutor/src/config.rs` | MODIFIED | `default_degradation_levels` 类型升级 + 向后兼容反序列化 + `similarity_threshold` 字段 + `validate()` 扩展 |
| `crates/csn-substitutor/src/degradation_chain.rs` | MODIFIED | `last_advanced_at` 字段 + `is_expired()` 方法 + 手动 `PartialEq` + 层级语义注释统一 |
| `crates/csn-substitutor/src/error.rs` | MODIFIED | 新增 `InvalidCapability` 变体 |
| `crates/csn-substitutor/src/lib.rs` | MODIFIED | `trigger_substitution` 重设计 + `select_candidate_for_level` + `find_substitutes` 过滤 + `register_capability` 校验 + `transaction_chain_map` + `cleanup_chains` + MCP 精准推进 |
| `crates/csn-substitutor/benches/degradation_chain.rs` | ADDED | 降级链推进延迟基准（< 1µs 验收） |
| `crates/csn-substitutor/Cargo.toml` | MODIFIED | 新增 `degradation_chain` bench target |
| `crates/event-bus/src/types.rs` | MODIFIED (Task 0.7) | `McpMeshTransactionCompleted` 新增 `capability_id: Option<String>` 字段 |

### 受影响规格

- `spec.md` C12（降级链 level 与候选选择关联）
- `spec.md` C13（层级语义统一）

### 测试覆盖

- `config.rs` 单元测试：旧格式兼容、新格式解析、`validate()` 边界
- `degradation_chain.rs` 单元测试：TTL、`is_expired`、`PartialEq` 语义
- `lib.rs` 单元测试：Top-(N+1) 推进、显式 ID、维度校验、阈值过滤、精准推进、transaction_id 映射、向后兼容、cleanup_chains
- `benches/degradation_chain.rs`：`advance_degradation` / `degradation_level` / `cleanup_chains` 延迟基准

### 性能影响

- `advance_degradation`：DashMap get_mut + next_level，预期 < 100ns（基准验证 < 1µs）
- `find_substitutes`：新增 `retain` 过滤，O(K) 开销可忽略
- `register_capability`：新增维度比较，O(1) 开销可忽略
- `cleanup_chains`：O(N) 遍历，N = chain 数量，典型 < 10

## 决策依据

### 学术支撑

- **Token Budgets (Khan, 2026)**：降级链是 token 预算控制的一部分，level N 对应不同"预算层级"的替代候选
- **Zero-trust LLM Agents (Kushnerov, 2026)**：MCP 失败精准推进避免误伤无关链，符合零信任"最小影响范围"原则
- **GraphBit (arXiv:2605.13848)**：TTL 清理机制参考图数据库的过期节点回收策略

### 工业尸检参考

- **Claude Code 尸检**：5.4% 孤儿调用教训 → MCP 失败推进必须有明确的 chain_id 归属
- **Qoder 骨骼**：配置向后兼容是 CLI 工具的工程惯例（2 版本周期迁移）

## 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| 旧配置文件未迁移导致行为变化 | 中 | 中 | 向后兼容反序列化自动转换，2 版本周期迁移 |
| Top-(N+1) 模式候选不足 | 低 | 低 | 返回最后一个可用候选，避免降级链提前终止 |
| TTL 清理误删活跃链 | 低 | 中 | TTL 默认 1 小时，调用方按需调用（非后台自动） |
| `Instant` 不可序列化 | 确定 | 低 | `#[serde(skip)]` + `default = "Instant::now"`，反序列化视为"刚推进" |

## 验收标准

- [x] `default_degradation_levels` 类型为 `Vec<Vec<String>>`
- [x] 向后兼容解析（旧字符串数组自动转换）
- [x] `trigger_substitution` 根据 level N 返回 Top-(N+1) 候选或显式 ID 列表
- [x] `degradation_chain.rs` 层级语义注释统一（C13）
- [x] `similarity_threshold: f32` 字段（默认 0.5）
- [x] `find_substitutes` 过滤低于阈值的候选
- [x] `register_capability` 校验向量维度
- [x] MCP 失败推进根据 `capability_id` 精准推进
- [x] chains TTL 清理 + ChainExhausted 自动移除
- [x] `benches/degradation_chain.rs` 验证推进延迟 < 1µs
- [x] `cargo test -p csn-substitutor` 全绿

## 后续工作

- v2.11.0-omega：移除旧格式（`Vec<String>`）反序列化支持
- 监控：在生产环境观察 `cleanup_chains` 调用频率与清理数量，评估 TTL 默认值是否需要调整
- 可选：若 TTL 清理需求频繁，考虑引入后台定时清理任务（需评估 §4.4 #7 fire-and-forget 风险）
