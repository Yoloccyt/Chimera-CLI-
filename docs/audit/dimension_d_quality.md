# 维度 D:代码质量与技术债审计报告

## 1. 执行摘要

- **审计日期**: 2026-06-28
- **审计范围**: 34 个 crates 的代码质量与技术债
- **审计工具**: Grep / Glob / Read / PowerShell 脚本(花括号匹配)
- **总体评价**: **良好**
- **问题数量**: Critical 0 / Major 3 / Minor 6

### 关键结论

| 维度 | 评价 | 说明 |
|------|------|------|
| `#![forbid(unsafe_code)]` | ✅ 优秀 | 34/34 crate 全覆盖(含 tests/benches) |
| 单函数 ≤200 行 | ✅ 优秀 | 生产代码 0 个超长函数 |
| workspace 级依赖 | ✅ 优秀 | 34/34 crate 全部 `workspace = true` |
| 错误处理一致性 | ✅ 优秀 | 33 个 thiserror + 1 个 anyhow(应用层) |
| WHY 注释覆盖 | ✅ 优秀 | 482 处 WHY 注释,覆盖 100 个文件 |
| TODO/FIXME | ⚠️ 良好 | 27 处,全部有明确 Week 计划 |
| 伪实现 | ⚠️ 待替换 | 3 处伪实现,均按计划待 Week 6-8 替换 |
| 硬编码常量 | ✅ 良好 | Week 1-4 Minor-3 已修复(权重/TTL 已配置化) |

---

## 2. TODO/FIXME/HACK 清单

全量扫描 `d:\Chimera CLI\crates` 下所有 `.rs` 文件,共发现 **27 处** TODO 注释,**0 处** FIXME/HACK/XXX/WORKAROUND。

### 2.1 按 crate 分类

| crate | 文件:行号 | TODO 内容 | 严重程度 | 修复时机 |
|-------|----------|----------|---------|---------|
| mtpe-executor | `src/predictor.rs:29` | SIMULATED_INFERENCE_DELAY 伪实现 | Major | Week 7 |
| mtpe-executor | `src/predictor.rs:248` | 伪预测实现,替换为真实模型多步预测 | Major | Week 7 |
| faae-router | `src/edsb.rs:340` | 伪随机概率实现,评估替换为 rand crate | Major | Week 8 |
| repo-wiki | `src/generator.rs:66` | 占位嵌入实现,NMC 实现后替换 | Major | Week 6 |
| gsoe-evolution | `src/policy/mutation.rs:17` | 接入 MCP Mesh 真实模型,用 logits 引导变异 | Minor | Week 7 |
| gsoe-evolution | `src/policy/grpo.rs:14` | 接入 MCP Mesh 真实模型 | Minor | Week 7 |
| gsoe-evolution | `src/policy/grpo.rs:66` | 接入 MCP Mesh 真实模型,用 logits 采样 | Minor | Week 7 |
| gsoe-evolution | `src/policy/fitness.rs:10` | 接入 MCP Mesh 真实模型,用验证集准确率 | Minor | Week 7 |
| nmc-encoder | `src/perceptors/audio.rs:15,42,45` | 接入 ort ONNX Runtime 实现音频编码 | Minor | Week 7/8 |
| nmc-encoder | `src/perceptors/image.rs:15,43,46` | 接入 ort ONNX Runtime 实现图像编码 | Minor | Week 7/8 |
| nmc-encoder | `src/perceptors/desktop.rs:17` | 结合截图字节实现多模态桌面编码 | Minor | Week 7/8 |
| nmc-encoder | `src/perceptors/text.rs:19,56` | 接入 ort ONNX Runtime 实现语义嵌入 | Minor | Week 7/8 |
| nmc-encoder | `src/perceptors/video.rs:15,42,45` | 接入 ort ONNX Runtime 实现视频编码 | Minor | Week 7/8 |
| seccore | `src/asa.rs:146` | 替换为 Critic PPO 模型 | Minor | Week 6 |
| seccore | `src/asa.rs:215` | history_failure_weight 用于 Critic PPO 加权 | Minor | Week 6 |
| seccore | `src/asa.rs:357` | 替换为基于 PVL Verifier 的语法检查 | Minor | Week 6 |
| parliament | `src/ahirt.rs:496` | 集成到 event-bus,发布 RedTeamAudit 事件 | Minor | Week 5 Task 37 |
| parliament | `src/ahirt.rs:570` | 集成到 event-bus,发布 AhirtProbeCompleted 事件 | Minor | Week 5 Task 37 |
| decb-governor | `src/error.rs:21` | 集成到 event-bus,发布 BudgetExceeded 事件 | Minor | Week 5 Task 37 |

### 2.2 TODO 质量评估

**优点**:
- **100% 有明确修复计划**: 所有 27 处 TODO 都标注了目标 Week(如 `TODO(Week 7)`、`TODO(Week 5 Task 37)`),无模糊 TODO
- **集中度高**: 伪实现类 TODO 集中在 3 个 crate(mtpe/faae/repo-wiki),便于批量替换
- **依赖明确**: nmc-encoder 的 TODO 明确依赖 ort ONNX Runtime,gsoe-evolution 依赖 MCP Mesh

**风险**:
- Week 5 Task 37 的 event-bus 集成 TODO(parliament/decb-governor)若未完成,会影响事件链路完整性
- nmc-encoder 的 5 个 perceptor 全部为占位实现,Week 7/8 工作量较大

---

## 3. Week 1-4 伪实现追踪

### 3.1 MTPE 伪预测

**状态**: ⚠️ **仍为伪实现**(按计划 Week 7 替换)

**代码位置**:
- `crates/mtpe-executor/src/predictor.rs:31` — `const SIMULATED_INFERENCE_DELAY: Duration = Duration::from_micros(50);`
- `crates/mtpe-executor/src/predictor.rs:115` — `tokio::time::sleep(SIMULATED_INFERENCE_DELAY).await;`
- `crates/mtpe-executor/src/predictor.rs:119` — `let predicted_tokens = generate_pseudo_predictions(n, context_hash);`
- `crates/mtpe-executor/src/predictor.rs:249` — `fn generate_pseudo_predictions(n: usize, context_hash: u32) -> Vec<Token>`

**伪实现细节**:
```rust
// 伪预测:基于上下文哈希生成 N 个确定性 token
fn generate_pseudo_predictions(n: usize, context_hash: u32) -> Vec<Token> {
    (0..n)
        .map(|i| {
            let confidence = (1.0 - (i as f32 * 0.05)).max(0.0);
            Token {
                text: format!("pred_{}_{}", i, context_hash),
                confidence,
            }
        })
        .collect()
}
```

**WHY 注释质量**: ✅ 优秀
- `predictor.rs:23-28` 解释了固定延迟的 WHY(模拟真实推理启动开销)
- `predictor.rs:54-56` 解释了固定种子的 WHY(确定性输出)
- `predictor.rs:246-247` 解释了置信度递减的 WHY(误差累积)

**建议**: 按 Week 7 计划接入 NMC 编码器 + 真实模型推理。

### 3.2 FaaE 伪随机

**状态**: ⚠️ **仍为伪实现**(按计划 Week 8 评估替换)

**代码位置**:
- `crates/faae-router/src/edsb.rs:341` — `fn pseudo_random_probability() -> f32`
- `crates/faae-router/src/edsb.rs:186` — 调用点 `let random_val = pseudo_random_probability();`

**伪实现细节**:
```rust
// WHY:不引入 rand 依赖,用 SystemTime 纳秒做简单概率判断
fn pseudo_random_probability() -> f32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos as f32) / 1_000_000_000.0
}
```

**WHY 注释质量**: ✅ 良好
- `edsb.rs:336-339` 解释了不引入 rand 依赖的 WHY(简单概率判断,非密码学安全)

**风险评估**: Minor
- 该函数用于 EDSB 熵均衡的概率判断,非密码学场景
- SystemTime 纳秒精度足够,但在高并发下可能产生相同值(纳秒级冲突)
- 真正的 rand crate 替换是改进项,非阻塞

**建议**: Week 8 评估时,若引入 rand crate 用于其他场景(如 GSOE 变异),可一并替换此处。

### 3.3 RepoWiki 占位嵌入

**状态**: ⚠️ **仍为伪实现**(按计划 Week 6 替换)

**代码位置**:
- `crates/repo-wiki/src/generator.rs:67` — `fn placeholder_embedding(content: &str) -> Vec<f32>`
- `crates/repo-wiki/src/generator.rs:43` — 调用点 `let embedding = Self::placeholder_embedding(&task.description);`

**伪实现细节**:
```rust
// 算法:32 字节哈希 → 每字节重复 16 次 → 归一化到 [0, 1]
// 32 × 16 = 512,正好填满 CLV 维度
fn placeholder_embedding(content: &str) -> Vec<f32> {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
    let mut embedding = Vec::with_capacity(512);
    for &byte in hash.iter() {
        let val = byte as f32 / 255.0;
        for _ in 0..16 {
            embedding.push(val);
        }
    }
    embedding
}
```

**WHY 注释质量**: ✅ 优秀
- `generator.rs:9-14` 模块级文档解释了占位嵌入的 WHY(Week 2 无 NMC,用确定性占位验证 sqlite-vec 集成)
- `generator.rs:59-65` 函数级注释解释了算法选择(32×16=512 维度对齐)

**建议**: Week 6 NMC 编码器实现后,替换为真实 CLV 嵌入(`nexus_core::CLV::from_text(...)`)。

---

## 4. unwrap/expect/panic 扫描

### 4.1 生产代码(src/)扫描结果

| 模式 | 生产代码出现次数 | 评估 |
|------|----------------|------|
| `.unwrap()` | 0(全部在 `#[cfg(test)]` 内) | ✅ 符合规范 |
| `.expect()` | 0(全部在 tests/benches) | ✅ 符合规范 |
| `panic!()` | 0(全部在 tests) | ✅ 符合规范 |
| `unreachable!()` | 0 | ✅ 符合规范 |
| `todo!()` | 0 | ✅ 符合规范 |
| `unimplemented!()` | 0 | ✅ 符合规范 |
| `.unwrap_or_default()` | 16(含 WHY 注释) | ✅ 合理降级 |

### 4.2 unwrap_or_default() 评估

生产代码中的 16 处 `unwrap_or_default()` 均为合理的优雅降级,典型场景:

| 位置 | 用途 | WHY |
|------|------|-----|
| `chtc-bridge/src/bridge.rs:104` | `serde_json::to_vec(value).unwrap_or_default()` | 序列化几乎不会失败,失败时哈希空字节 |
| `gea-activator/src/activator.rs:302` | `serde_json::to_string(task).unwrap_or_default()` | f32 不实现 Hash,用 JSON 序列化做缓存 key |
| `kvbsr-router/src/router.rs:358` | `routed_tools.first().cloned().unwrap_or_default()` | Top-K 可能为空,降级为空字符串 |
| `mlc-engine/src/l3_procedural.rs:434,437` | JSON 反序列化降级 | 失败时降级为空数组/空统计,不阻断查询 |
| `repo-wiki/src/store.rs:497` | `serde_json::from_str(&tags_json).unwrap_or_default()` | SQLite tags JSON 反序列化降级 |
| `scc-cache/src/cache.rs:196` | `self.stats.read().map(...).unwrap_or_default()` | 读锁竞争失败时降级 |

**结论**: 无错误掩盖,所有 `unwrap_or_default()` 都有明确的降级语义和 WHY 注释。

### 4.3 错误吞没(`let _ = ...`)评估

生产代码中的 `let _ = ...` 模式(排除 tests/benches):

| 位置 | 代码 | 评估 |
|------|------|------|
| `csn-substitutor/src/lib.rs:264` | `let _ = chain.next_level();` | ✅ 推进降级链,已耗尽时保持当前层级(语义合理) |
| `cmt-tiering/src/coordinator.rs:166` | `let _ = std::fs::remove_dir_all(&ice_tmp_dir);` | ✅ 防御性清理,失败无所谓(有 WHY 注释) |
| `mcp-mesh/src/mesh.rs:169` | `let _ = self.rollback_phase(&participants).await;` | ✅ 超时回滚,best-effort(合理) |
| `mtpe-executor/src/predictor.rs:137` | `let _ = self.event_bus.publish(event).await;` | ✅ 无订阅者时事件丢弃(有 WHY 注释) |
| `efficiency-monitor/src/lib.rs:516` | `let _ = monitor.check_alerts();` | ✅ 在 `#[cfg(test)]` 内(非生产代码) |

**结论**: 无错误吞没问题,所有 `let _ =` 都有合理上下文。

---

## 5. 硬编码常量盘点

### 5.1 Week 1-4 Minor-3 复核

| 原问题 | 当前状态 | 证据 |
|--------|---------|------|
| HCW 压缩器权重(0.4/0.3/0.3) | ✅ **已配置化** | `compressor.rs:249` 从 `config.compressor_weights` 读取 |
| GEA 缓存 TTL=5s | ✅ **已配置化** | `gea-activator/src/config.rs:42` `pub cache_ttl_secs: u64`(serde default) |
| HCW get() 返回 clone(Minor-4) | ✅ **已优化** | `window.rs:176` 新增 `get_arc()` 返回 `Arc<ContextEntry>` |

### 5.2 全量 const/static 盘点

扫描所有 `const`/`static` 声明(排除 tests/benches),按类别分类:

#### 权重类(应配置化)
| 位置 | 常量 | 值 | 评估 |
|------|------|-----|------|
| `gsoe-evolution/src/policy/grpo.rs:25` | `OPTIMAL_ACTION` | `1.0` | ✅ 测试用常量,无需配置化 |

**结论**: 生产代码无硬编码权重(原 HCW 权重已配置化)。

#### TTL 类(应配置化)
| 位置 | 常量 | 值 | 评估 |
|------|------|-----|------|
| `faae-router/src/edsb.rs:37` | `DECAY_INTERVAL_SECS` | `300`(5 分钟) | ⚠️ Minor — 可考虑配置化 |
| `gea-activator/src/activator.rs:29` | `CACHE_STATS_INTERVAL` | `100` | ✅ 统计间隔,内部常量 |

#### 阈值类
| 位置 | 常量 | 值 | 评估 |
|------|------|-----|------|
| `lsct-tiering/src/tiering/profile.rs:17` | `PROMOTION_THRESHOLD` | `0.7` | ⚠️ Minor — 可考虑配置化 |
| `lsct-tiering/src/tiering/profile.rs:19` | `DEMOTION_THRESHOLD` | `0.3` | ⚠️ Minor — 可考虑配置化 |
| `decb-governor/src/overflow.rs:19,21,23` | `OVERFLOW_WARN_RATIO`/`DEGRADE_RATIO`/`CRITICAL_RATIO` | `0.5`/`0.8`/`1.0` | ⚠️ Minor — 可考虑配置化 |

#### 容量类
| 位置 | 常量 | 值 | 评估 |
|------|------|-----|------|
| `cmt-tiering/src/coordinator.rs:40` | `DECAY_BATCH_SIZE` | `1024` | ✅ 批处理大小,内部常量 |
| `mlc-engine/src/config.rs:28,35,42` | `L0/L1/L2_CAPACITY_MAX` | `1024`/`65536`/`65536` | ✅ 配置校验上界,合理 |

#### 内部常量(无需配置化)
| 位置 | 常量 | 评估 |
|------|------|------|
| `mtpe-executor/src/predictor.rs:57` | `CONTEXT_HASH_SEED`(`0x4D54_5045`) | ✅ FNV-1a 种子,伪预测用 |
| `mtpe-executor/src/predictor.rs:31` | `SIMULATED_INFERENCE_DELAY` | ⚠️ 伪实现常量,Week 7 替换 |
| `gsoe-evolution/src/policy/grpo.rs:19,22` | `EPS`/`ACTION_DIM` | ✅ 算法常量 |
| `decb-governor/src/governor.rs:33,35` | `ONE_HOUR_SECS`/`ONE_DAY_SECS` | ✅ 时间换算常量 |
| `pvl-layer/src/verifier.rs:27` | `DANGEROUS_KEYWORDS` | ✅ 危险关键词黑名单 |
| `parliament/src/ahirt.rs:669` | `PATTERNS` | ✅ 攻击模式列表 |

### 5.3 配置化建议

**Minor-5**: 以下阈值/间隔建议配置化(当前为硬编码 const):
- `lsct-tiering/src/tiering/profile.rs:17,19` — PROMOTION/DEMOTION_THRESHOLD
- `decb-governor/src/overflow.rs:19,21,23` — OVERFLOW_*_RATIO
- `faae-router/src/edsb.rs:37` — DECAY_INTERVAL_SECS

---

## 6. 单函数长度核验

### 6.1 扫描方法

使用 PowerShell 脚本扫描所有 `.rs` 文件,通过花括号深度匹配计算函数体行数(排除字符串/注释中的花括号干扰,虽然不完美但足够准确)。

### 6.2 扫描结果

| 类别 | 超过 200 行的函数数 | 评估 |
|------|-------------------|------|
| 生产代码(src/) | **0** | ✅ 符合项目铁律 |
| 测试代码(tests/benches) | 1 | ✅ 可接受 |

**唯一超长函数**(测试代码):
- `crates/osa-coordinator/tests/e2e.rs:240` — `async fn test_e2e_performance_benchmarks()`(>200 行)

### 6.3 结论

✅ **生产代码 100% 符合 ≤200 行铁律**,无超长函数需要拆分。测试代码中的超长函数为 E2E 性能基准测试,逻辑连贯,拆分反而降低可读性,可接受。

---

## 7. 错误处理一致性

### 7.1 库层错误类型(thiserror)

扫描结果:**33/34 crate 都有 `src/error.rs`**(仅 `chimera-cli` 例外,因为它是应用层)。

抽查样本:

| crate | error.rs | thiserror | 错误分类 | 评估 |
|-------|----------|-----------|---------|------|
| nexus-core | ✅ `src/error.rs` | ✅ `#[derive(Debug, Error)]` | 7 类(InvalidClvDimension/QuestNotFound/...) | ✅ 优秀 |
| mtpe-executor | ✅ `src/error.rs` | ✅ `#[derive(Debug, Error)]` | 3 类(InvalidN/PredictionFailed/RollbackFailed) | ✅ 优秀 |
| faae-router | ✅ `src/error.rs` | ✅ | — | ✅ |
| hcw-window | ✅ `src/error.rs` | ✅ | — | ✅ |

### 7.2 应用层错误类型(anyhow)

| crate | 错误类型 | 评估 |
|-------|---------|------|
| chimera-cli | `anyhow::Result<()>` | ✅ 符合 §4.1 规范 |

**证据**:`crates/chimera-cli/src/main.rs:26` — `async fn main() -> anyhow::Result<()>`

### 7.3 错误处理模式

- **`?` 操作符**: 生产代码广泛使用 `?` 传播错误,符合规范
- **`#[from]` 自动转换**: `nexus-core/src/error.rs:34,42` 使用 `#[from] rusqlite::Error` / `#[from] std::io::Error`
- **错误上下文**: `predictor.rs:104-108` 返回结构化错误(`InvalidN { n, max }`),调用方可 match 处理

### 7.4 结论

✅ **错误处理一致性优秀**,严格遵循"库层 thiserror + 应用层 anyhow"规范。

---

## 8. WHY 注释覆盖

### 8.1 覆盖率统计

- **WHY 注释总数**: 482 处
- **覆盖文件数**: 100 个 `.rs` 文件
- **平均密度**: 约 4.8 处/文件

### 8.2 覆盖类别评估

| 类别 | 覆盖情况 | 示例 |
|------|---------|------|
| 隐藏约束 | ✅ 优秀 | `predictor.rs:113` 解释固定延迟与 N 无关 |
| 变通方案 | ✅ 优秀 | `compressor.rs:57-61` 解释接受 `&[ContextEntry]` 而非 `Vec` 的原因 |
| 反直觉行为 | ✅ 优秀 | `generator.rs:86` 解释按 `char` 而非 `byte` 截断 |
| 性能优化 | ✅ 优秀 | `compressor.rs:134-143` 解释 `select_nth_unstable_by` 替代全排序 |
| 降级处理 | ✅ 优秀 | `bridge.rs:103` 解释序列化失败时哈希空字节 |

### 8.3 注释质量评估

抽查典型 WHY 注释:

**优秀示例 1**(`predictor.rs:23-28`):
```rust
/// WHY 固定延迟:真实推理中,模型启动/上下文编码的开销远大于生成单个
/// token 的开销,且此开销与 N 无关(一次推理可产出 N 个 token)。
/// MTPE 的核心优势就是减少推理启动次数。伪预测中加入此延迟,
/// 使加速比测试能反映真实场景的加速效果(1000×N=5 vs 5000×N=1)
```
→ 解释了 WHY(模拟真实场景)而非 WHAT(睡 50 微秒)。

**优秀示例 2**(`compressor.rs:54-61`):
```rust
/// WHY:至少保留 1 个条目 — 避免压缩后上下文为空,此时 compressed_size
/// 可能 > target_size,调用方(HcwWindow)据此触发窗口升级
///
/// WHY 接受 `&[ContextEntry]` 而非 `Vec<ContextEntry>`(SubTask 19.4):
/// 原实现要求调用方 `state.entries.clone()` 全量 clone 1000 条目后传入,
/// 现接受借用引用,内部仅 clone 保留的 Top-N 条目(通常 ≤ 100),
/// 消除 900+ 次无用 clone。
```
→ 同时解释了设计决策和性能优化历史。

### 8.4 结论

✅ **WHY 注释覆盖优秀**,注释质量高,真正解释了 WHY 而非 WHAT,符合"隐藏约束/变通方案/反直觉行为"三类要求。

---

## 9. workspace 级依赖声明

### 9.1 扫描方法

1. 全量 Grep `^\w+\s*=\s*\{\s*version\s*=\s*"` 查找独立版本声明 → **0 匹配**
2. 全量 Grep `^version\s*=\s*"|^edition\s*=\s*"` 查找独立 version/edition → **0 匹配**
3. 抽查 4 个 crate 的 Cargo.toml 验证

### 9.2 抽查结果

| crate | version | edition | 依赖声明 | 评估 |
|-------|---------|---------|---------|------|
| chimera-cli | `workspace = true` | `workspace = true` | 全部 `workspace = true` | ✅ |
| nexus-core | `workspace = true` | `workspace = true` | 全部 `workspace = true` | ✅ |
| mtpe-executor | `workspace = true` | `workspace = true` | 全部 `workspace = true` | ✅ |
| faae-router | `workspace = true` | `workspace = true` | 全部 `workspace = true` | ✅ |

### 9.3 workspace 根配置

`Cargo.toml` 的 `[workspace.package]` 定义:
```toml
[workspace.package]
version = "1.0.0-omega"
edition = "2021"
authors = ["Aether CLI Team <team@aether.dev>"]
license = "Apache-2.0"
```

`[workspace.dependencies]` 收录 20+ 共享依赖(tokio/serde/anyhow/thiserror/...)。

### 9.4 结论

✅ **workspace 级依赖声明 100% 合规**,无独立版本声明,完全遵循 §4.1 规范。

---

## 10. 问题清单

| ID | 严重程度 | 问题 | 代码位置 | 修复建议 |
|----|---------|------|---------|---------|
| Q-01 | Major | MTPE 伪预测占位实现 | `crates/mtpe-executor/src/predictor.rs:31,115,119,249` | Week 7 接入真实模型推理,替换 SIMULATED_INFERENCE_DELAY 与 generate_pseudo_predictions |
| Q-02 | Major | FaaE 伪随机概率实现 | `crates/faae-router/src/edsb.rs:341` | Week 8 评估引入 rand crate 替换 SystemTime 纳秒方案 |
| Q-03 | Major | RepoWiki 占位嵌入 | `crates/repo-wiki/src/generator.rs:67` | Week 6 NMC 实现后替换为真实 CLV 嵌入 |
| Q-04 | Minor | LSCT 升降级阈值硬编码 | `crates/lsct-tiering/src/tiering/profile.rs:17,19` | 将 PROMOTION_THRESHOLD(0.7)/DEMOTION_THRESHOLD(0.3)移入 LsctConfig |
| Q-05 | Minor | DECB 溢出比例硬编码 | `crates/decb-governor/src/overflow.rs:19,21,23` | 将 OVERFLOW_WARN/DEGRADE/CRITICAL_RATIO 移入 DecbConfig |
| Q-06 | Minor | FaaE 衰减间隔硬编码 | `crates/faae-router/src/edsb.rs:37` | 将 DECAY_INTERVAL_SECS(300)移入 FaaeConfig |
| Q-07 | Minor | nmc-encoder 5 个 perceptor 全占位 | `crates/nmc-encoder/src/perceptors/*.rs` | Week 7/8 接入 ort ONNX Runtime,工作量大需提前规划 |
| Q-08 | Minor | GSOE 4 处 TODO 待真实模型 | `crates/gsoe-evolution/src/policy/{mutation,grpo,fitness}.rs` | Week 7 接入 MCP Mesh 后批量替换 |
| Q-09 | Minor | event-bus 集成 TODO 未完成 | `crates/parliament/src/ahirt.rs:496,570` + `crates/decb-governor/src/error.rs:21` | Week 5 Task 37 补齐 RedTeamAudit/AhirtProbeCompleted/BudgetExceeded 事件发布 |

---

## 11. 长期主义建议

### 11.1 伪实现替换路线图

当前 3 个 Major 伪实现(Q-01/Q-02/Q-03)均有明确替换计划,建议按以下顺序推进:

1. **Week 6**: RepoWiki 占位嵌入 → 真实 CLV(依赖 NMC 编码器)
2. **Week 7**: MTPE 伪预测 → 真实模型多步预测(依赖 MCP Mesh)
3. **Week 8**: FaaE 伪随机 → rand crate(可选,非阻塞)

**关键依赖**: NMC 编码器是伪实现替换的核心瓶颈,Week 6 必须优先完成。

### 11.2 配置化演进方向

建议建立"配置化检查清单",在 Week 8 打磨阶段统一处理 Minor-5/6/7:

- 将阈值类常量(PROMOTION/DEMOTION/OVERFLOW_*)移入对应 Config 结构
- 将间隔类常量(DECAY_INTERVAL_SECS)移入 FaaeConfig
- 所有 Config 结构已支持 serde + figment 多源合并,配置化成本低

### 11.3 TODO 治理建议

- **建立 TODO 燃尽图**: 按周跟踪 TODO 数量变化,确保每周净减少
- **TODO 关联 Issue**: 建议 TODO 注释关联 GitHub Issue(如 `// TODO(Week 7, #123): ...`),便于进度追踪
- **TODO Review**: 每周验收时检查 TODO 是否按计划清理,避免遗留

### 11.4 测试代码规范

当前测试代码中的 `unwrap()`/`expect()` 是合理的(测试失败应 panic),但建议:
- **统一错误消息**: `expect()` 消息统一为中英文一致风格(当前混用)
- **测试辅助函数**: 抽取重复的 setup 代码(如 `make_quest_with_tasks`)到 `tests/common/mod.rs`

### 11.5 总体评价

Chimera CLI 项目在代码质量维度表现**良好**,已建立扎实的工程基础:
- ✅ `#![forbid(unsafe_code)]` 100% 覆盖,安全红线不可突破
- ✅ 单函数 ≤200 行铁律 100% 遵守
- ✅ workspace 级依赖声明 100% 合规
- ✅ 错误处理规范统一(库层 thiserror + 应用层 anyhow)
- ✅ WHY 注释覆盖优秀(482 处,质量高)

主要技术债为 3 个伪实现(Major),但均有明确替换计划和 WHY 注释说明,属于"有计划的占位实现"而非"失控的技术债"。建议按 Week 6-8 路线图推进替换,同时补齐 Minor 级配置化改进。
