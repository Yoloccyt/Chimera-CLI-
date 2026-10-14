# ADR-069: Token 效率优化（客户端域五项优化）

**状态**: 已验证（v2.20.0-omega — 集成测试 + 基准集 Driver + SMART 指标采集闭环）  
**日期**: 2026-08-02  
**层级**: L0/L3/L6/L2/L10 跨层协同  
**接缝**: CapabilityToken S9TokenEfficiency

## 1. 背景

Chimera 作为 API 客户端，Token 成本优化分两个世界：
- **客户端可控域**：Prompt 结构、厂商缓存亲和、语义缓存、上下文预算、输出治理
- **服务端不可控域**：PagedAttention/KV 量化等（仅适用于自部署通道，以部署 Profile 下发）

本 ADR 覆盖客户端可控域的五项优化。

## 2. 算法选型对比

### 2.1 语义缓存向量索引

| 方案 | 延迟 | 合规性 | 适用规模 | 决策 |
|------|------|--------|---------|------|
| 暴力扫描 + RwLock | 7.2μs@256 | forbid(unsafe_code) 合规 | <1000/ns | **选用** |
| HNSW (hnsw_rs) | <1μs@10K | 内部 unsafe，违反 forbid | >10K | 拒绝 |
| IVF + PQ | ~5μs@100K | 需额外依赖 | >100K | 拒绝(YAGNI) |

**决策理由**：per-namespace 容量硬限 256，暴力扫描 256×28ns = 7.2μs，远低于 TTFT P95 5% 预算（~5ms）。无需引入 unsafe 或额外依赖。

### 2.2 Prompt 压缩

| 方案 | 压缩率 | 合规性 | 决策 |
|------|--------|--------|------|
| LLMLingua-2 Python sidecar | 2-5× | forbid(unsafe_code) 合规（进程间通信） | **选用** |
| Rust 原生 extractive | 1.5-2× | 合规但压缩率低 | 备选 |
| 无压缩 | 1× | — | fallback |

**决策理由**：LLMLingua-2 是 SOTA 压缩模型，Rust 无等价物。sidecar 通过 stdin/stdout JSON IPC，零 unsafe。失败时 graceful degradation 到原文。

## 3. 回本公式推导

### 3.1 Anthropic 族（显式 cache_control）

```
写入溢价: 25%（首次写入 = 输入价 × 1.25）
命中折扣: 90%（命中价 = 输入价 × 0.10）
回本请求数: ceil(25 / (100 - 10)) = ceil(25/90) = 1
```

**结论**：Anthropic 族 1 次命中即回本，始终启用显式缓存。

### 3.2 豆包（存储费感知）

```
存储费: ~50% 输入价/天
命中折扣: ~50%（缓存命中价 = 输入价 × 0.5）
回本请求数: ceil(50 / 50) = 1 次/天
保守阈值: 3 次（覆盖存储费 + 不确定性余量）
```

**结论**：会话预期轮次 < 3 时不启用显式缓存，退化为纯粘性路由。

## 4. 回滚方案

通过 **CapabilityToken S9TokenEfficiency 四态**实现（禁止运行时 feature flag，ADR-034）：

```
Provisional → Authorized → Cooldown → Frozen
     ↑                        │
     └────── 冷却期结束 ────────┘
```

- **30 秒降级**：`trigger_asa_intervention()` → Cooldown → `allows_learned_policy()` = false → bypass 全部缓存
- **永久冻结**：连续 3 次 ASA 触发 → Frozen → 手动 `unfreeze()` 恢复
- **Fail-open**：Cooldown/Frozen 态 lookup 返回 None（不阻塞请求，仅 token 消耗上升）

## 5. 缓存键覆盖

```rust
TokenCacheKey {
    model,              // 模型名
    model_version,      // 模型版本（防静默升级）
    tool_schema_hash,   // SHA-256(工具 JSON)
    system_prompt_hash, // SHA-256(Layer1 + Layer2)
    thinking_tier,      // Fast/Standard/Deep
}
```

五维覆盖确保：任何维度变更 → 缓存自动失效（cache hit ≠ correctness）。

## 6. 架构归属

| 模块 | 层级 | crate |
|------|------|-------|
| TokenCacheKey / OutputBudget | L0 | nexus-contracts |
| VendorCacheProfile / CacheHitTracker | L3 | scc-cache |
| SemanticResponseCache | L3 | scc-cache |
| NormalizedPrompt / PromptCompressor | L10 | mca-gateway |
| negotiate_budget / apply_output_budget | L10 | mca-gateway |
| compute_token_budget | L6 | osa-coordinator |
| trim_to_budget | L2 | hcw-window |
| ContextBudgetAllocated / SemanticCacheHit | L1 | event-bus |

依赖方向全部合法：L10→L3→L1→L0，L6→L2→L1→L0。

## 7. 性能红线

| 指标 | 红线 | 验证方式 |
|------|------|---------|
| 语义查询 256 条目 | < 50μs | criterion bench |
| 语义插入 | < 20μs | criterion bench |
| SHA-256 x2 | < 5μs | criterion bench |
| TTFT P95 增幅 | ≤ 5% | E2E 压测 |
