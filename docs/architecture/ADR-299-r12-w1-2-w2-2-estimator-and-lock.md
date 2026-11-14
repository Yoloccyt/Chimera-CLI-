# ADR-299: R12 W1-2 / W2-2 —— token 真值源注入 + L2 锁竞争取证（后者否决改锁）

- 状态: Accepted
- 日期: 2026-11-14
- 批次: R12 W1-2、W2-2
- 关联: ADR-296(W0 度量地基) / ADR-297(W2 建图阈值；**本 ADR 推翻其 §3 的重开判据**) / ADR-298(W1-1 装配接线，B1 边界) / ADR-144(上下文装配内核)
- 影响面: `crates/hcw-window/src/{assemble,lib}.rs`、`crates/mca-gateway/src/{conversation_trim,lib}.rs`、`crates/mlc-engine/{Cargo.toml,benches/l2_lock_contention.rs}`（**`mlc-engine/src/**` 零改动**）

---

## 1. W1-2：token 估算改为可注入（解开 ADR-298 B1 的第一道前置）

### 1.1 决策

`crates/hcw-window/src/assemble.rs`：

```rust
pub fn assemble_context_with(
    items: Vec<ContextItem>, contract: &ContextAssemblyContract, tier_capacity: usize,
    estimate: &dyn Fn(&str) -> usize,          // 注入点
) -> AssembledContext;

pub fn assemble_context(                        // 既有签名与行为逐字不变
    items: Vec<ContextItem>, contract: &ContextAssemblyContract, tier_capacity: usize,
) -> AssembledContext {
    assemble_context_with(items, contract, tier_capacity, &estimate_tokens)
}
```

**WHY 注入而不是改 `estimate_tokens` 的值**：改常量会同时改变 17 处 `assembly_contract` 断言的语义，并把 L2 内核绑死到 L10 的度量需求上；注入让"用哪把尺子"成为调用方的选择，而内核的默认行为零变化。

**WHY 不用 trait**：`&dyn Fn` 足够且零分配；trait 会引入一个只有两个实现者的抽象（过度设计）。

### 1.2 唯一调用点核验（本波最关键的检查）

`assemble.rs` 内 `estimate_tokens` 的全部出现逐条核对：

| # | 位置 | 性质 | 是否注入 |
|---|---|---|---|
| 1 | `:164` 函数定义 | 默认估算器本身（**值冻结**） | 作为默认注入项 |
| 2 | `:196` 薄壳内的唯一调用 | 委托 | ✅ |
| 3 | `per_item_tokens`（唯一逐条计量点） | 预算累计 / 驱逐扣减 / `used_tokens` 三处共用 | ✅ |
| 4 | `effective_budget` / `density` | **不含 token 计量**（仅 `min()` 与价值分） | 无需注入 |

判据：`estimate_tokens` 在 `assemble.rs` 内**只有 1 个调用点**（`:196`），且它只出现在薄壳里。抽样实测确认。

### 1.3 变异验证（测试有牙的实证，非自我断言）

| 变异 | 期望 | 实测 |
|---|---|---|
| 把**驱逐扣减**改回默认估算器 | 注入测试红 | **3 failed** |
| 把**预算累计**改回默认估算器 | 注入测试红 | **4 failed** |
| 逐字还原 | 全绿且文件哈希逐位相同 | 6/6 绿、哈希一致 |

### 1.4 **不解开**的那一半：表示层缺口（重要，不许当已修）

内核吃到的 `content` 是 `dedup_content(message)` 的**扁平化键**（`标记 + 字节长度 + ':' + 原文`），不是消息块原文。因此即使尺子同了：

- `Text{"abc"}` 按块 = 0 token（3 字节 / 4 floor）；扁平化键 `t3:abc` = 1 token；
- ⇒ **`used_tokens` ≠ `estimate_tokens(messages)`，这是结构性的，不是 bug**。

本波因此**只**断言 `used_tokens == Σ estimate(传给内核的那份 content)`（3 条独立用例，含"默认估算器在中文上与注入器读数不同"作为阳性对照），**不**断言与 mca 侧整会话估算相等。

**结论**：`max_tokens = usize::MAX` **继续保持**。启用内核驱逐还差第二道前置——让 `dedup_content` 与块表示对齐（或改为传块结构）。这是独立的一步，登记为开放项。

> 全仓现状（供后续统一）：{`hcw-window` :164、TUI `curator.rs:173`、`mlc-engine` `l2_semantic.rs:605`} = **字符/4 ceil**；{`mca-gateway` `token_estimate.rs:49`} = **字节宽/4 floor**。本波只统一了 hcw ↔ mca；TUI 与 mlc 未动（后者禁改域），登记后续。

---

## 2. W2-2：L2 读锁竞争取证 —— **判定不改锁**

### 2.1 测量设计（先说能被反驳的部分）

`crates/mlc-engine/benches/l2_lock_contention.rs`（692 行，criterion `iter_custom` + Flat 采样）：
读者数 `c ∈ {0,1,2,4,8}`，主线程做 `insert` 并记录每次延迟分位；**三组对照**：

1. **阳性对照**（`assert_positive_control`）：`c=0` 的写者 p95 必须远小于 `c=8`；
2. **`cpu_only_c8`**：8 读者打**另一个实例** ⇒ 若罚金来自 CPU 抢占，这一组也该慢；
3. **`lock_c1_repeat`**：与 `c=1` 同形，验证可复现。

### 2.2 读数（三次静默跑，同形）

| 量 | 值 |
|---|---|
| H = 一次 `recall_by_clv` 墙钟（= 读锁持有时长，10k 条目） | **4.33 / 4.58 / 5.17 ms** |
| `c=1` 时读锁占空比 | **0.95~1.15** ⇒ 单个读者几乎全程持锁 |
| 写者 p95(c=8)/p95(c=0) | **309× / 589× / 297×**（11.5 / 26.9 / 11.8 ms vs 37 / 46 / 40 µs） |
| **归一化后** p50(c=8) | **1.1~1.4 × H** |
| **归一化后** p95(c=8) | **2.3~5.9 × H** |
| 被阻塞 >1ms 的比例 | c=0: 0.5~1% → c=4: 5~15% → c=8: **92~99%** |

### 2.3 **本 ADR 推翻 ADR-297 §3 的重开判据**

ADR-297 写的重开条件是「**p95 随读者数线性劣化**」。实测：

| c | 0 | 1 | 2 | 4 | 8 |
|---|---|---|---|---|---|
| p95 倍数（A/B/C 三跑） | 1.00 | 1.12~1.20 | 108.9~1068.9 | 410~716 | 296~589 |

**非单调，峰在 c=4，c=8 反而回落**。原判据的前提（单调）**事实不成立** ⇒ 该判据作废。替代判据由本 ADR 给出（§2.5）。

### 2.4 修法的**无条件成本**（这是否决的关键）

"锁外快照"要把 `Vec<(SharedCLV, MemoryId)>` 克隆出来。实测克隆成本 **p50 0.30 / 0.39 / 0.92 ms**，而无竞争插入只要 **26~34 µs** ⇒ 克隆是插入的 **9~35 倍**，且**每次插入都要付**；而收益只在碰撞时兑现。**成本确定、收益条件性** ⇒ 不应改。

### 2.5 替代判据（本波自提，可复核）

| 判据 | 内容 | 现状 |
|---|---|---|
| G1 | `p95(c=8) ≥ 3×H` | 实测 2.3~7.1×H ⇒ **边界，不定论** |
| G2 | 碰撞率 × (W_c − W_0) ≥ 3×C（C=克隆成本） | 折算为**读占空比 ≳ 20%**（盈亏平衡实测 5.8~21%）⇒ 仅 c≥4 过线 |
| G3 | **生产**读占空比必须实测 | **未测** ⇒ 不能据 G2 就改 |

**判定：不改锁。** 罚金是"等一到几次扫描"（p50≈1×H、p95≈2~7×H），不是无界排队；且修法成本 9~35× 无条件付出。

> 顺带修正一个此前的口径：生产调用面是 **N+1 次短读锁**形状（逐条查），本 bench 用"一次长持有"，因此**本 bench 的罚金是上界**。

---

## 3. 未决项（本 ADR 不解释，交给后续）

| # | 现象 | 状态 |
|---|---|---|
| **U1** | `c ≤ 2` 档出现 **0.62~3.56 s** 的病态单次 `insert`，而同期读者单次最大仅 38~132 ms（与"写者优先"语义冲突）；三次跑复现，`at_idx` 多为 0 | **未定性**（已作为异常登记） |
| U2 | 近似层（HNSW）10k 档未测 | 未覆盖 |
| U4 | 生产形状是 N+1 次读锁，bench 是上界 | 已登记为口径说明 |

---

## 4. 验收（机证）

| 判据 | 命令 | 实测 |
|---|---|---|
| hcw-window 全量 | `cargo test -p hcw-window --jobs 2` | 339 lib + 16 + 6（新注入）+ 25 doctest，**0 failed** |
| mca-gateway 全量 | `cargo test -p mca-gateway --jobs 2` | 258 lib + 11 接线 + 3 口径，**0 failed** |
| clippy | `cargo clippy -p hcw-window -p mca-gateway --all-targets -D warnings` | rc=0 |
| 锁竞争 bench 可编译 | `cargo bench -p mlc-engine --bench l2_lock_contention --no-run` | rc=0 |
| 生产码零改动（W2-2） | `git diff --stat crates/mlc-engine/src` | 仅 `memory_graph.rs`（属 ADR-297） |

证据日志：`tmp/final/{1..4}-*.log`（W1-2）、`tmp/mut1.log` `tmp/mut2.log`（变异）、`tmp/r12-w2-2/{runA,runB,runC}.log` + `*.meta.txt`（锁竞争三跑；run1/run2 因重定向截断与采样差异**弃用**）。

---

## 5. 减法

| 不做 | 理由 |
|---|---|
| 把 `estimate_tokens` 改成 mca 公式 | 会改 17 处断言语义 + 让 L2 绑死 L10 需求 |
| 本波启用内核驱逐 | 表示层缺口（§1.4）未解，启用即"两个不同字符串用同一把尺子量" |
| 改 L2 锁结构（`Arc` 快照 / COW） | 无条件成本 9~35×，收益需生产占空比 ≳20%（未测） |
| 给 `used_tokens` 加告警阈值 | 注入后数值"变贵 3 倍"，会报假信号（已在代码与报告写明） |
| 统一 TUI / mlc 的估算公式 | 越出本波；TUI 有自己的预算语义，mlc 禁改域 |
