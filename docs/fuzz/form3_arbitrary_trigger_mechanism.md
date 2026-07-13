# Form 3 (Arbitrary trait) Fuzz Target 触发条件机制

> 本文档定义 Chimera CLI (NEXUS-OMEGA) 项目中何时使用 Form 3 (Arbitrary trait) fuzz target 签名的评估标准、决策流程与决策依据。
>
> 文档版本: v1.0.0
> 创建日期: 2026-07-13
> 适用基线: workspace.package.version = 1.5.5-omega

---

## 目录

- [1. 背景与术语](#1-背景与术语)
- [2. 当前状态分析](#2-当前状态分析)
  - [2.1 现有 fuzz target 清单](#21-现有-fuzz-target-清单)
  - [2.2 Form 2 覆盖范围评估](#22-form-2-覆盖范围评估)
  - [2.3 Form 3 不必要性分析](#23-form-3-不必要性分析)
- [3. 三种签名形式对比](#3-三种签名形式对比)
- [4. 触发条件](#4-触发条件)
- [5. 评估标准 (Checklist)](#5-评估标准-checklist)
- [6. 决策流程](#6-决策流程)
- [7. 决策依据 (ADR)](#7-决策依据-adr)
  - [7.1 ADR-FUZZ-001: 当前选择 Form 2 的决策依据](#71-adr-fuzz-001-当前选择-form-2-的决策依据)
  - [7.2 ADR-FUZZ-002: 实施 Form 3 的决策边界](#72-adr-fuzz-002-实施-form-3-的决策边界)
- [8. 关联的 stub 宏使用说明](#8-关联的-stub-宏使用说明)
  - [8.1 stub 宏对 Form 3 的支持](#81-stub-宏对-form-3-的支持)
  - [8.2 Form 3 target 模板](#82-form-3-target-模板)
  - [8.3 Cargo.toml 配置要点](#83-cargotoml-配置要点)
- [9. 参考文献](#9-参考文献)

---

## 1. 背景与术语

### 1.1 cargo-fuzz 签名形式

cargo-fuzz (libfuzzer-sys 0.4) 支持三种 `fuzz_target!` 宏签名:

| 形式 | 语法 | 参数类型 | 适用场景 |
|------|------|---------|---------|
| Form 1 | `\|bytes\| { ... }` | `&[u8]` (隐式) | 快速原型,无类型标注 |
| **Form 2** | `\|data: &[u8]\| { ... }` | `&[u8]` (显式) | **字节级变异,当前项目 6 个生产 target 均使用** |
| Form 3 | `\|data: CustomType\| { ... }` | `CustomType: Arbitrary` | 结构化输入,由 libFuzzer 自动从字节反序列化 |

### 1.2 核心约束

- **平台限制**:本项目 Windows 环境使用 MinGW (GNU),libFuzzer 的 `FuzzerExtFunctionsWindows.cpp` 仅适配 MSVC,MinGW g++ 无法编译。因此:
  - 本地 (Windows-GNU):**无法实际运行 cargo-fuzz**,仅通过 stub 宏验证语法
  - 实际 fuzz 执行:委托 Linux CI (`.github/workflows/fuzz.yml`)
  - 详见 `.trae/rules/nuxus规则.md` §10.3 与 `.claude/CLAUDE.md` §6
- **Arbitrary trait**:Form 3 依赖 `libfuzzer_sys::arbitrary::Arbitrary` trait,需要 `derive(Arbitrary)` 或手动实现,且需 Rust nightly 工具链
- **stub 宏兼容**:本地 stub 宏 (`fuzz/src/lib.rs`) 已支持 Form 3 签名的语法验证,但**不检查 Arbitrary trait bound**

---

## 2. 当前状态分析

### 2.1 现有 fuzz target 清单

当前共 8 个 fuzz target (6 生产 + 2 回归测试):

| # | Target 名称 | 被测 crate | 架构层 | 签名形式 | 输入类型 | 模糊目标 |
|---|-------------|-----------|--------|---------|---------|---------|
| 1 | `quest_parse` | `nexus-core` | L1/L9 | Form 2 | `&[u8]` → JSON/MessagePack serde | Quest/UserIntent/MultimodalInput 反序列化 |
| 2 | `seccore_sandbox` | `seccore` | L4 | Form 2 | `&[u8]` → String lossy | validate_command/validate_env |
| 3 | `event_serialize` | `event-bus` | L1 | Form 2 | `&[u8]` → JSON/MessagePack serde | NexusEvent/EventMetadata 往返序列化 |
| 4 | `cacr_budget_parse` | `model-router` | L1 | Form 2 | `&[u8]` → JSON/MessagePack serde | CacrConfig 反序列化 + guard check |
| 5 | `checkpoint_deserialize` | `nexus-core` | L1 | Form 2 | `&[u8]` → JSON/MessagePack serde | Checkpoint 反序列化 + 往返一致性 |
| 6 | `config_section_parse` | `nexus-core` | L10 | Form 2 | `&[u8]` → JSON/MessagePack serde | ChimeraConfig 多 section 反序列化 |
| 7 | `stub_form1_test` | _(回归测试)_ | — | Form 1 | `&[u8]` (隐式) | 验证 stub 宏 Form 1 兼容性 |
| 8 | `stub_form3_test` | _(回归测试)_ | — | **Form 3** | `Vec<u8>` | 验证 stub 宏 Form 3 兼容性 |

### 2.2 Form 2 覆盖范围评估

所有 6 个生产 target 均使用 Form 2 字节级变异,其覆盖模式如下:

```
字节输入 → serde反序列化(JSON/MessagePack)
         → 成功: 字段访问 + 往返不变量 + 特定断言
         → 失败: 优雅 Err,无 panic
```

**覆盖的优势**:

- serde 反序列化本身就是一种"结构化解析"——libFuzzer 的字节级变异通过 serde 的 JSON/MessagePack 解码器"自然"转化为结构化输入
- 6 个 target 覆盖了 4 个关键 crate (`nexus-core`、`event-bus`、`seccore`、`model-router`),涵盖 4 个架构层 (L1、L4、L9、L10)
- 每个 target 同时测试 JSON 和 MessagePack 两种编码格式,覆盖了 ADR-004 的序列化协议
- 300s × 6 target 的 CI 运行时间 (`.github/workflows/fuzz.yml`) 在可接受范围内

### 2.3 Form 3 不必要性分析

**当前阶段无需 Form 3 的原因**:

1. **serde 作为自然的结构化解析器**:所有 6 个生产 target 的模糊目标都是 serde 反序列化路径。`serde_json::from_slice::<T>` 和 `rmp_serde::from_slice::<T>` 已经提供了从字节到结构化类型的转换,libFuzzer 的字节级变异天然能覆盖边界情况(畸形 JSON、截断数据、超大数字等)

2. **无原生字节级解析路径**:项目中没有需要测试的**非 serde 的字节→结构体解析**路径。如果存在类似 `parse_quest_from_bytes(raw: &[u8]) -> Result<Quest>` 的手动解析函数,Form 3 才有优势

3. **Arbitrary derive 的额外维护成本**:
   - 需要为每个被测类型添加 `derive(Arbitrary)` 或手动实现 `Arbitrary` trait
   - 这些类型已经实现了 `Deserialize`,再添加 `Arbitrary` 会产生重复的 schema 定义
   - 当被测类型字段变更时,`Arbitrary` 实现和 `Deserialize` 实现需要同步更新

4. **stub 宏的 trait bound 检查限制**:在 Windows-GNU 下,stub 宏 (**`fuzz/src/lib.rs:64-68`**) 不检查 `Arbitrary` trait bound,仅验证类型名称有效性和 body 类型安全性。这意味着 Form 3 的语法验证在本地是**不完全的**,真正的 trait bound 检查只能在 Linux CI 上完成

5. **CI 运行时间预算**:6 target × 300s 是 CI 的合理上限。如果引入 Form 3 target,需要额外的编译时间和运行时间,且可能增加 CI 失败率

---

## 3. 三种签名形式对比

| 维度 | Form 1 | Form 2 | Form 3 |
|------|--------|--------|--------|
| 签名 | `\|bytes\|` | `\|data: &[u8]\|` | `\|data: CustomType\|` |
| 参数类型 | `&[u8]` (隐式) | `&[u8]` (显式) | `T: Arbitrary` |
| 变异粒度 | 字节级 | 字节级 | 结构体字段级 |
| serde 解耦 | 需要手动 serde | 需要手动 serde | 自动 Arbitrary 反序列化 |
| 依赖 | 无 | 无 | `libfuzzer_sys::arbitrary` |
| 本地验证 | ✅ stub 宏完全支持 | ✅ stub 宏完全支持 | ⚠️ stub 宏支持语法但不检查 trait bound |
| 适用场景 | 快速原型 | **通用场景(推荐)** | 结构化输入深度测试 |
| 当前使用 | 1 个回归测试 | 6 个生产 target | 1 个回归测试 |

---

## 4. 触发条件

以下条件**任一满足**时,应重新评估是否需要引入 Form 3 fuzz target:

### 条件 A: 新增 target 需要解析结构化输入

> 新增的 fuzz target 需要测试**非 serde 的字节→结构体解析**路径,且该结构体实现了 `Arbitrary` trait。

**典型场景**:
- 新增了手动编写的 `parse_protocol_buffer(raw: &[u8]) -> Result<Message>` 函数
- 新增了 `from_bytes` / `try_from` 等手动字节解析方法
- 新增了需要测试**字段间约束关系**的反序列化逻辑(如 `check_amount >= check_fee + check_tax`)

**反例**(不需要 Form 3):
- 新增的 target 仍然通过 serde 反序列化 → 保持 Form 2 即可
- 新增的 target 解析纯文本格式(如 CSV 行) → 保持 Form 2,用 `String::from_utf8_lossy` 处理

### 条件 B: Form 2 字节级变异覆盖率停滞

> 某个现有 Form 2 target 的代码覆盖率已连续多轮无增长,且通过增加 corpus 种子文件无法改善。

**判断依据**:
- 使用 `cargo +nightly fuzz coverage <target>` 检查覆盖率报告
- 覆盖率报告的**关键路径** (如验证函数、边界检查分支) 覆盖率 < 70%
- 向 `fuzz/corpus/<target>/` 添加了 10+ 个精心构造的种子文件后,覆盖率仍无显著提升

**典型场景**:
- serde 反序列化后的 `if` 分支(如 `CacrConfig` 的阈值检查)有未覆盖的路径
- `Checkpoint` 字段间的逻辑约束(如 `serialized_state` 长度与 `memory_snapshot_hash` 的对应关系)未被变异探索到

### 条件 C: 需要测试特定数据结构的不变量

> 需要测试某个数据结构在**所有可能的字段组合**下仍保持不变量,且 serde 反序列化因"全有或全无"策略无法覆盖部分字段异常。

**典型场景**:
- `Checkpoint` 的 `created_at` 时间戳应小于当前时间 — 但 serde 反序列化时时间戳在 `i64` 范围内均合法,Form 2 变异需要大量迭代才能生成"未来时间戳"的组合
- 某个配置结构体有字段间依赖(如 `A > B` 且 `B > C`),Form 2 的随机变异生成有效组合的概率极低

**注意**:条件 C 在实践中**非常罕见**。对于大多数 serde 反序列化场景,Form 2 的字节级变异 + 合理的 corpus 种子文件已足够覆盖。只有在明确观察到因"字段间约束"导致覆盖率瓶颈时,才考虑条件 C。

---

## 5. 评估标准 (Checklist)

当任意触发条件满足时,使用以下 checklist 评估是否实际实施 Form 3:

### 5.1 前置条件

- [ ] **是否有明确的 Arbitrary 类型可用?**
  - 目标类型是否已实现 `Arbitrary` trait? 或可通过 `#[derive(Arbitrary)]` 派生?
  - 手动实现 `Arbitrary` 的成本是否可接受?(需实现 `arbitrary::Arbitrary` trait 的 `size_hint` 和 `arbitrary` 方法)
  - 如果目标类型是枚举,所有变体是否能被 `Arbitrary` 自然覆盖?

- [ ] **Form 2 字节级变异是否已充分?**
  - 6 个生产 target 各 300s 的 CI 运行是否已充分覆盖?
  - 是否已通过 `cargo +nightly fuzz coverage` 检查过覆盖率报告?
  - 是否已尝试过增加 corpus 种子文件?(`fuzz/corpus/<target>/` 目录)
  - 覆盖率停滞的具体位置是否明确?(哪条分支、哪个函数)

- [ ] **新增 Arbitrary 类型的维护成本是否合理?**
  - 目标类型是否属于核心领域类型 (`UserIntent`/`Quest`/`Checkpoint`/`ChimeraConfig` 等)?核心类型变更频繁,Arbitrary 实现需同步更新
  - 目标类型有多少字段?字段越多,Arbitrary 实现的维护成本越高
  - 目标类型是否包含非 `Arbitrary` 字段(如 `Vec<u8>`, `String` 等标准类型自动支持;自定义类型需要额外实现)

- [ ] **是否与现有 stub 宏兼容?**
  - 本地 Windows-GNU 下 `cargo check --manifest-path fuzz/Cargo.toml` 能否通过?
  - stub 宏 (`fuzz/src/lib.rs:64-68`) 的 Form 3 分支是否支持目标类型?
  - 注意:stub 宏**不检查 `Arbitrary` trait bound**,仅验证类型名称和 body 类型安全

### 5.2 决策矩阵

| 条件 | 前置条件满足 | 前置条件不满足 |
|------|-------------|---------------|
| 条件 A: 结构化输入 | ✅ 实施 Form 3 | ⚠️ 先实现 `Arbitrary`,再评估 |
| 条件 B: 覆盖率停滞 | ⚠️ 先尝试增加 corpus;若仍无效再实施 Form 3 | ✅ 保持 Form 2,优化 corpus |
| 条件 C: 字段不变量 | ⚠️ 先尝试单元测试 + proptest;若需 fuzz 则实施 Form 3 | ✅ 使用 proptest 替代 |

### 5.3 否决条件

**出现以下任一情况,直接否决 Form 3,保持 Form 2**:

- 目标类型未实现 `Arbitrary` 且手动实现需要大量 unsafe 代码
- 目标类型的字段超过 15 个(Arbitrary 组合爆炸,维护成本 > 收益)
- 目标类型变更频率高于月均 2 次(核心领域类型变更时,Arbitrary 实现需同步更新)
- 引入 Form 3 后,CI fuzz job 总运行时间超过 30 分钟(当前 6 target × 300s ≈ 20min)

---

## 6. 决策流程

```
[识别触发条件]
       │
       ▼
┌─────────────────────────────────────────────────┐
│ 步骤 1: 识别需要 Form 3 的 target               │
│                                                 │
│ 识别触发条件(见 §4):                            │
│   A. 新增结构化输入 target?                     │
│   B. Form 2 覆盖率停滞?                         │
│   C. 需测试字段间不变量?                        │
│                                                 │
│ 输出: 明确的目标 target 名称 + 模糊目标            │
└─────────────────────────────────────────────────┘
       │
       ▼
┌─────────────────────────────────────────────────┐
│ 步骤 2: 评估方案 A vs 方案 B                    │
│                                                 │
│ 方案 A(Form 2 + 自定义 corpus):                 │
│   - 保持现有 Form 2 签名                        │
│   - 在 body 中手动解析字节为结构体                 │
│   - 增加精心构造的 corpus 种子文件               │
│   - 优点:无额外依赖,stub 宏完全兼容              │
│   - 缺点:覆盖率可能受限于 serde 反序列化路径      │
│                                                 │
│ 方案 B(Arbitrary derive):                       │
│   - 使用 Form 3 签名                           │
│   - 为类型添加 #[derive(Arbitrary)]             │
│   - 优点:结构化变异,覆盖字段间组合               │
│   - 缺点:维护成本,stub 宏不完全检查              │
│                                                 │
│ 使用 §5 checklist 评估                          │
└─────────────────────────────────────────────────┘
       │
       ▼
┌─────────────────────────────────────────────────┐
│ 步骤 3: 实施决策记录                            │
│                                                 │
│ 无论选择方案 A 还是方案 B,均需记录:               │
│   - 决策日期与决策者                              │
│   - 触发条件 (A/B/C)                             │
│   - 选择方案及理由                                │
│   - 否决的原因(如果选择了方案 A)                  │
│   - checklist 评估结果                           │
│                                                 │
│ 记录位置: 本文档 §7 ADR 部分                    │
└─────────────────────────────────────────────────┘
       │
       ▼
┌─────────────────────────────────────────────────┐
│ 步骤 4: 回归测试验证                            │
│                                                 │
│ 如果选择方案 B (Form 3):                        │
│   1. 添加目标类型到 fuzz_targets/               │
│   2. 更新 fuzz/Cargo.toml [[bin]] 声明          │
│   3. 本地运行 cargo check 验证语法                │
│   4. (可选)Linux CI 上运行 300s 验证             │
│   5. 更新 scripts/check_fuzz_config.ps1/sh      │
│      (添加新的 expected target 到列表)          │
│   6. 更新本文档 §2.1 target 清单                │
│                                                 │
│ 如果选择方案 A (Form 2 + corpus):               │
│   1. 添加 corpus 种子文件到 fuzz/corpus/        │
│   2. 验证 corpus 种子文件有效性                   │
│                                                 │
│ 最终验证:                                       │
│   cargo check --manifest-path fuzz/Cargo.toml    │
│   scripts/check_fuzz_config.ps1                 │
└─────────────────────────────────────────────────┘
```

### 决策流程速查表

| 步骤 | 操作 | 输出 | 责任人 |
|------|------|------|--------|
| 1 | 识别触发条件 | target 名称 + 模糊目标 | 开发者 |
| 2 | 评估方案 A vs B | checklist 评估结果 | 开发者 + Reviewer |
| 3 | 记录决策 | ADR 条目 | 开发者 |
| 4 | 回归验证 | 通过检查清单 | 开发者 |

---

## 7. 决策依据 (ADR)

### 7.1 ADR-FUZZ-001: 当前选择 Form 2 的决策依据

| 字段 | 内容 |
|------|------|
| **ADR 编号** | ADR-FUZZ-001 |
| **标题** | 在可用时选择 Form 2 而非 Form 3 作为默认 fuzz target 签名 |
| **日期** | 2026-07-13 |
| **状态** | ✅ 已确认 |
| **背景** | 项目 6 个生产 fuzz target 全部使用 Form 2 (`\|data: &[u8]\|`) 签名。所有 target 的模糊目标都是 serde 反序列化路径,serde 的 JSON/MessagePack 解码器自然地将字节级变异转化为结构化输入。 |
| **决策** | 保持 Form 2 作为默认签名形式。Form 3 仅在有明确触发条件时引入。 |
| **理由** | 1. serde 已提供足够的结构化解析能力,Form 2 字节级变异通过 serde 解码器自然覆盖结构化输入边界<br>2. 避免 `Arbitrary` derive 的维护成本(核心类型变更时需同步更新)<br>3. 避免 stub 宏不完全检查 `Arbitrary` trait bound 带来的本地验证盲区<br>4. 6 target × 300s 的 CI 预算已充分覆盖当前模糊目标 |
| **后果** | 正面:低维护成本,本地验证完全,CI 运行时间可控<br>负面:当需要测试字段间约束时,Form 2 的变异效率可能低于 Form 3 |
| **关联文件** | `fuzz/fuzz_targets/*.rs` (6 个生产 target 全部 Form 2) |

### 7.2 ADR-FUZZ-002: 实施 Form 3 的决策边界

| 字段 | 内容 |
|------|------|
| **ADR 编号** | ADR-FUZZ-002 |
| **标题** | 定义何时实施 Form 3 的决策边界 |
| **日期** | 2026-07-13 |
| **状态** | ✅ 已确认 |
| **背景** | 虽然当前无需 Form 3,但未来项目演进可能引入需要结构化输入测试的场景。需要明确的决策边界以避免:1) 过早引入 Form 3 增加维护成本;2) 过晚引入导致覆盖率盲区。 |
| **决策** | 当且仅当 §4 触发条件之一满足,且 §5 checklist 全部通过时,才实施 Form 3。 |
| **理由** | 1. 明确的决策边界防止"为了用而用"的过度工程化<br>2. checklist 确保引入 Form 3 的收益大于成本<br>3. 否决条件防止在核心类型频繁变更时引入额外维护负担 |
| **后果** | 正面:清晰的决策路径,避免资源浪费<br>负面:需要定期重新评估触发条件(建议每次新增 fuzz target 时评估) |
| **决策边界** | 见 §4 触发条件 + §5 评估 checklist |

---

## 8. 关联的 stub 宏使用说明

### 8.1 stub 宏对 Form 3 的支持

文件: `fuzz/src/lib.rs` (第 64-68 行)

```rust
// Form 3: |data: CustomType| 任意 Arbitrary 类型
// 对应 libfuzzer-sys 0.4 的 `(|$data:ident: $dty:ty| $body:block)` 规则。
// WHY 闭包不执行:Arbitrary 类型需 libFuzzer 运行时反序列化原始字节,
// stub 环境无运行时支持。$dty 出现在闭包签名中让编译器验证:
// 1. 类型名称有效 2. body 中对 $data 的操作类型安全。
// 不检查 Arbitrary trait bound(libfuzzer_sys 不可用,无法引用 trait),
// 真正的 trait bound 检查由非 Windows-GNU 环境的 libfuzzer-sys 完成。
(|$data:ident: $dty:ty| $body:block) => {
    fn main() {
        let _probe = |$data: $dty| $body;
    }
};
```

**关键限制**:

| 限制 | 说明 |
|------|------|
| **不检查 Arbitrary trait bound** | stub 宏仅验证类型名称有效性和 body 类型安全,不验证 `$dty: Arbitrary`。真正的 trait bound 检查由 libfuzzer-sys 在非 Windows-GNU 环境下完成 |
| **不执行 body** | 闭包 `_probe` 从未被调用,stub 仅验证语法 |
| **仅限语法验证** | 如果 `$dty` 是一个不存在的类型,编译器会报错;但如果 `$dty` 存在但未实现 `Arbitrary`,编译器在 stub 环境下不会报错 |

### 8.2 Form 3 target 模板

```rust
//! Fuzz target: <简述>
//!
//! 对应架构层:L?
//!
//! # 模糊目标
//! 验证 <target_type> 的 Arbitrary 反序列化与字段不变量:
//! 1. 不 panic(内存安全)
//! 2. <字段不变量> 在所有输入下成立
//! 3. <其他目标>
//!
//! # 运行方式(需 nightly)
//! ```bash
//! cargo +nightly fuzz run <target_name>
//! ```

// 条件 use:Windows-GNU 用 stub 宏,其他平台用 libfuzzer-sys
#[cfg(not(all(target_os = "windows", target_env = "gnu")))]
use libfuzzer_sys::fuzz_target;

#[cfg(all(target_os = "windows", target_env = "gnu"))]
use chimera_fuzz::fuzz_target;

use <crate>::<TargetType>;

// Form 3 签名:参数类型为 TargetType(需实现 Arbitrary)
fuzz_target!(|data: <TargetType>| {
    // 字段访问与不变量检查
    // ...

    // 注意:Form 3 的 data 已经是结构化类型,
    // 无需手动 serde 反序列化
});
```

### 8.3 Cargo.toml 配置要点

新增 Form 3 fuzz target 时,需在 `fuzz/Cargo.toml` 中添加:

```toml
[[bin]]
name = "<target_name>"
path = "fuzz_targets/<target_filename>.rs"
test = false
doc = false
```

同时更新 `scripts/check_fuzz_config.ps1` 和 `scripts/check_fuzz_config.sh` 中的 `$expectedTargets` 列表。

---

## 9. 参考文献

### 9.1 项目内文档

| 文档 | 引用章节 | 说明 |
|------|---------|------|
| `.trae/rules/nuxus规则.md` | §10.3 | fuzz 与 cargo-audit 委托模式,平台限制说明 |
| `.claude/CLAUDE.md` | §2, §5, §6 | 常用命令、发布前检查清单、本地无法运行的限制 |
| `fuzz/Cargo.toml` | — | fuzz crate 依赖配置与 [[bin]] 声明 |
| `fuzz/src/lib.rs` | — | stub 宏实现与三种签名形式支持 |
| `fuzz/fuzz_targets/stub_form3_test.rs` | — | Form 3 stub 宏回归测试 |
| `scripts/check_fuzz_config.ps1` | — | fuzz 配置静态验证脚本(Windows) |
| `scripts/check_fuzz_config.sh` | — | fuzz 配置静态验证脚本(Linux) |
| `.github/workflows/fuzz.yml` | — | CI fuzz workflow,6 target × 300s |

### 9.2 外部参考

- [libfuzzer-sys 0.4 文档](https://docs.rs/libfuzzer-sys/0.4) — `fuzz_target!` 宏签名定义
- [cargo-fuzz 文档](https://rust-fuzz.github.io/book/cargo-fuzz.html) — cargo-fuzz 使用指南
- [Arbitrary trait 文档](https://docs.rs/arbitrary/latest/arbitrary/trait.Arbitrary.html) — `#[derive(Arbitrary)]` 使用说明

### 9.3 版本历史

| 版本 | 日期 | 变更说明 | 作者 |
|------|------|---------|------|
| v1.0.0 | 2026-07-13 | 初始版本,定义 Form 3 触发条件、评估标准、决策流程与 ADR | — |