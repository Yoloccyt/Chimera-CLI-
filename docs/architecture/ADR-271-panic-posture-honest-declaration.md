# ADR-271 -- panic 姿态裁决：诚实声明优先，维持 `panic = "abort"`

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W5
- 关联：ADR-239（阈值/证据纪律）、ADR-120/127/199（唯一派发入口）、根 `Cargo.toml` 的 Release Profile 段、`release.yml` 的 <50MB / Docker <100MB 体积门
- 影响面：`crates/gqep-executor/src/toolplan_runner.rs`（模块头声明）、根 `Cargo.toml`（release 段注释）、新增 `crates/gqep-executor/tests/panic_posture_lock.rs`
- 行为影响：**零**（纯声明 + 文本锁，无 Rust 逻辑改动）

## 1. 背景：两件事同时成立，导致「隔离」是假的

1. 工具派发热路径**没有任何 panic 隔离**：`catch_unwind` / `AssertUnwindSafe`
   在 `gqep-executor` 与 `chimera-cli` 的工具路径上 0 命中（命中的都在
   `nexus-core/compute`、`decay-engine/parallel` 等别的面）。工具路径只有
   **Err 级 fail-soft**（缺字段 / 执行失败折叠成节点错误文本）。
2. `Cargo.toml` 的 `[profile.release]` 开 **`panic = "abort"`**
   （体积 SLO 的刻意权衡，原注释自陈「避免 unwind 表展开，代价是 panic 直接退出」）。

⇒ 结论：**在 release 档加 `catch_unwind` 抓不住 panic**（abort 不走 unwind）。
于是「加个 catch_unwind 就隔离了」是一个**假命题**：代码变复杂、行为没变。

## 2. 三案对照

| 维度 | 案 A：诚实声明（**采纳**） | 案 B：改 `unwind` + catch_unwind | 案 C：子进程隔离 |
|---|---|---|---|
| 体积 | 0 | unwind 表 + landing pad，典型 +10~30% | 子进程二进制 / IPC 栈，显著增大 |
| 体积门风险 | 无 | `release.yml` <50MB、Docker <100MB **有越线风险** | 同上且更重 |
| 影响面 | 局部注释 + 测试 | **全 46 crate** 的全局 profile | 新派发面 |
| 架构红线 | 无 | `AssertUnwindSafe` 包 async future 有 UB 风险面 | **实质是第二派发/调度面** ⇒ 撞 ADR-120/127/199 |
| 有效性 | 如实告知「panic 即中止」 | release 下**可**捕获（unwind 生效） | 真隔离，但每调用 fork+IPC，延迟数量级上升 |
| 回滚 | trivial | profile 回滚 + 移除全部包装 | 极重 |

**采纳案 A 的理由（按权重）**：

1. `panic = "abort"` 是**刻意**的体积权衡，不是疏忽；改它等于用一个已记录的
   全局权衡去换一个局部收益，须重开体积 SLO 裁决 —— 不能顺手做。
2. 案 C 的名字是「隔离」，实质是**新增第二执行面**：工具不再在同进程四环链上跑，
   而是 fork/IPC。这与本仓「唯一派发入口」与「四环顺序即安全边界」两条硬约束冲突。
3. 工具路径已有 Err 级 fail-soft；panic 属于**内部代码信任**范畴
   （与 `osa-coordinator` parallel 面「不为不可能失败的场景写防御」同一哲学）。

## 3. 决策

### D1：维持 `panic = "abort"`，把后果写成可机检的声明

- `toolplan_runner.rs` 模块头新增「panic 姿态（ADR-271，诚实声明）」段：
  承诺 Err 级 fail-soft、**不承诺** panic 隔离、说明 release 下 `catch_unwind` 无效。
- 根 `Cargo.toml` 的 release 注释在 `panic=abort` 一行后追加连带后果与
  「不得在本行悄悄改成 unwind」的告诫。

### D2：三条反向锁（锁「不做什么」）

| 锁 | 断言 | 防的是什么 |
|---|---|---|
| `release_profile_panic_abort_is_deliberate` | `[profile.release]` 段内含 `panic = "abort"`，且文件含 ADR-271 与「catch_unwind 在 release 下**无效**」 | 有人为「修 panic」悄悄改 unwind，破体积 SLO |
| `tool_path_has_no_catch_unwind` | 七个工具路径实现文件**均不含** `catch_unwind` | 加一个 release 下无效、却制造「已隔离」错觉的包装 |
| `posture_declaration_present` | 模块头含「panic 姿态（ADR-271」「不承诺」+「panic 隔离」「Err 级 fail-soft」 | 声明被删/被改成含糊其辞 |

**为什么锁「不做」**：本轮勘察里最容易出现的错误动作就是「加个 catch_unwind
显得更稳」。锁住这条比锁「做了什么」更有价值 —— 它防止的是**虚假安全感**。

## 4. 验收（现跑）

见 §6 追记。

## 5. 未采纳

| 项 | 结论 |
|---|---|
| 改 `[profile.release] panic = "unwind"` | 否决（本轮）：影响全 46 crate、体积门有越线风险；若未来采纳须先实测体积并重开 SLO 裁决 |
| 子进程隔离工具执行 | 否决：实质第二派发面，撞 ADR-120/127/199 |
| 给工具路径加 `catch_unwind` | 否决：release 下无效，只会掩盖「panic 会中止进程」这一事实 |
| 什么都不做（不加声明） | 否决：现状是「文档看起来像有隔离、实际没有」的隐性漂移 |

## 6. 实测追记（已填）

| 判据 | 结果 |
|---|---|
| `cargo test -p gqep-executor --test panic_posture_lock` | **3 passed / 0 failed** |
| `cargo clippy -p gqep-executor --all-targets -- -D warnings` | 通过 |
| 体积门 | 本批零 Rust 逻辑改动 ⇒ binary 体积零变化（`release.yml` <50MB / Docker <100MB 仍由 CI 裁决） |

**一次真实的误报（如实记录，因为它说明锁的边界）**：`tool_path_has_no_catch_unwind`
首跑红，原因是判据用了**裸词** `contains("catch_unwind")`，而本 ADR 的模块头声明
恰好在散文里写着「零 `catch_unwind`」—— 锁把「解释为什么不加」误判成「加了」。
改为匹配**调用形态** `catch_unwind(` 并补 `AssertUnwindSafe` 检查后转绿。

教训：文本锁的判据必须锚在**语法形态**上（调用 / 属性 / 字段），不能锚在裸标识符上 ——
否则「文档解释」与「代码事实」会互相污染，锁会朝两个方向同时误判
（假红：解释被当成使用；假绿：拼接字符串绕过）。
