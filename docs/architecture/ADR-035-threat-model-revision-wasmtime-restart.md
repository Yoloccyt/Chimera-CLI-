# ADR-035: 威胁模型下修 + wasmtime 沙箱重启路径(ADR-001 真实降级程度修正)

## 状态

已批准 (Accepted, 2026-07-23)

## 背景

### 历史决策与现状偏差

- **ADR-001 沙箱运行时选择**:原决策采用 gVisor(runsc)作为沙箱运行时,Linux 生产环境优先,Windows/macOS 降级为进程隔离 + 白名单。该决策记录于 `docs/architecture/` ADR-001,亦在项目规则 `.trae/rules/nuxus规则.md` §2.3 ADR 表中标注"⚠️ 降级(seccore `sandbox.rs:474` 注释'当前实现为降级版本')"。
- **ADR-001 真实降级程度**:经 P0-W0.1.4 事实核验(详见下文"关键事实核验"),当前 `seccore` 沙箱实现**完全基于 `tokio::process::Command + 策略过滤`**,无任何 gVisor / seccomp / wasmtime 依赖。即原决策中"Linux 生产环境通过 gVisor"与"Windows/macOS 降级"的二分并未落地 —— 实际全平台均为进程级隔离 + 静态策略过滤的降级方案。
- **v5.0 设计文档 C5 决策**:威胁模型下修,wasmtime 沙箱重启路径。v5.0 设计文档 §10 安全架构明确要求修正 ADR-001 的真实降级程度,使威胁模型与代码事实对齐,并为高风险场景提供 wasmtime 升级路径。

### 关键事实核验(P0-W0.1.4 完成)

> **本节为 ADR-035 的立论基础,所有结论经代码与依赖图实测核验,非主观判断。**

经对 v2.3.1-omega 代码基线的四源交叉核验,确认以下事实:

1. **seccore 完全无 wasmtime 依赖**
   - [seccore/Cargo.toml](file:///D:/Chimera%20CLI/crates/seccore/Cargo.toml) 的 `[dependencies]`(L6-L22)与 `[dev-dependencies]`(L24-L28)均无 `wasmtime` 条目。
   - 实际依赖仅:`sha2` / `hex` / `thiserror` / `serde` / `tokio` / `chrono` / `tracing` / `event-bus`(dev:`criterion` / `proptest`)。

2. **sandbox.rs 完全基于 `tokio::process::Command + 策略过滤`**
   - [sandbox.rs:181](file:///D:/Chimera%20CLI/crates/seccore/src/sandbox.rs#L181) `execute_in_sandbox` 是唯一执行入口,内部使用 `TokioCommand::new`(L190)。
   - 四层防御措施:`env_clear()`(L195,零信任不继承父进程环境)/ `kill_on_drop(true)`(L206,超时后强制 SIGKILL)/ `tokio::time::timeout`(L212,DoS 防护)/ `compute_audit_hash`(L253,SHA-256 审计哈希)。
   - [sandbox.rs:174](file:///D:/Chimera%20CLI/crates/seccore/src/sandbox.rs#L174) 文档注释明确自认"当前实现为降级版本"。
   - 无 gVisor runsc 调用、无 seccomp 过滤器、无 wasmtime Engine/Store。

3. **前序子代理报告 `wasmtime = "0.39"` 为捏造**
   - 经 git 历史与依赖图实测,`crates/seccore/Cargo.toml` 从未声明过任何版本的 `wasmtime`。
   - 前序子代理报告中"seccore 曾依赖 wasmtime 0.39"的陈述**无代码与版本控制证据支持**,判定为捏造(幻觉)。
   - 本 ADR 据此纠正该错误信息,避免后续决策基于虚假事实。

4. **根 Cargo.toml L58 存在 workspace 级孤儿声明**
   - [Cargo.toml:58](file:///D:/Chimera%20CLI/Cargo.toml#L58) 声明 `wasmtime = "22.0"`(workspace.dependencies 级)。
   - 经全 workspace 扫描,**无任何 crate 的 Cargo.toml 通过 `wasmtime = { workspace = true }` 引用该声明**,属孤儿声明(造成依赖图误导,使人误以为 wasmtime 已被使用)。
   - 该孤儿声明需在 P1 阶段清理(见决策 3)。

### 触发本 ADR 的问题

- **威胁模型虚高**:ADR-001 原决策与项目规则均暗示"Linux 有 gVisor 内核级隔离",但实际代码全平台为进程级隔离。威胁模型若基于"gVisor 已生效"进行风险评估,将低估真实风险。
- **孤儿声明误导**:根 Cargo.toml 的 `wasmtime = "22.0"` 孤儿声明使依赖审查者误判 seccore 已集成 wasmtime,偏离 §3.4.1 第 6 条"性能可证伪"与依赖铁律的可审计性要求。
- **高风险场景升级路径缺失**:当前进程级隔离对单机低风险 CLI 场景足够,但对多租户/不可信代码执行等高风险场景防护不足,缺乏明确的升级路径。
- **高危操作审计前置缺失(D6)**:当前 ASA 审计为周期性,高危操作(risk_level ≥ 71)未强制实时前置审计与人工升级,存在审计窗口期风险。

## 决策

经 E01 首席架构师 + E02 安全架构师 + E05 生产系统专家分布式深度分析与多轮交叉验证,对威胁模型下修与 wasmtime 沙箱重启路径作出以下 5 项架构决策:

### 决策 1: 威胁模型下修 — ADR-001 真实降级程度修正

将 ADR-001 记录的威胁模型从"gVisor(Linux)+ 进程隔离(Windows/macOS)二分"下修为与代码事实一致的单一模型:

- **修正后威胁模型**:进程级隔离(`env_clear` + `kill_on_drop` + `timeout`)+ SHA-256 审计哈希(Merkle 链)+ 静态策略过滤(`validate_command` / `validate_env`)。全平台一致,无内核级隔离。
- **下修原因**:
  1. gVisor 为 Linux 专有(runsc 需 Linux 内核支持),与项目"Windows 11 + GNU 工具链"主开发环境不兼容。
  2. gVisor 部署复杂度高(需安装 runsc + 配置 OCI runtime),违反单机 CLI 的零依赖部署原则。
  3. gVisor 与 `#![forbid(unsafe_code)]` 哲学存在张力(虽 gVisor 本身是 Go 实现,但其集成路径需系统级配置,偏离 crate 级安全哲学)。
- **风险接受声明**:
  - **低风险场景(单机 CLI,信任用户输入)**:进程级隔离 + 策略过滤足够,风险可接受。
  - **高风险场景(多租户 / 不可信代码执行 / 服务端部署)**:进程级隔离不足,需 wasmtime 沙箱重启(见决策 2)。
- **文档同步**:本 ADR 生效后,ADR-001 与 `.trae/rules/nuxus规则.md` §2.3 ADR 表中 ADR-001 条目的"降级"标注应同步引用本 ADR,明确"降级程度 = 全平台进程级隔离,非 Linux gVisor + Windows 进程隔离二分"。

### 决策 2: wasmtime 沙箱重启路径(P3-W12.1 PoC)

为高风险场景提供 wasmtime 沙箱升级路径,以 PoC(Proof of Concept)形式在 P3-W12.1 阶段落地:

- **引入 wasmtime**:使用 wasmtime safe API(公开稳定 API),核实不违反 `#![forbid(unsafe_code)]`(unsafe 封装在 wasmtime crate 内部,不传播到 seccore,见决策 5)。
- **新建模块**:在 `crates/seccore/src/sandbox_wasm.rs`(新建)实现 WASM 沙箱 PoC,提供 `WasmSandbox` 类型与 `execute_in_wasm()` 方法,作为 `execute_in_sandbox()` 的高风险场景替代入口。
- **Linux CI 验证 gVisor(ADR-001 重启路径)**:在 Linux CI 矩阵中验证 gVisor runsc 路径(Linux 专有),作为 ADR-001 原决策的"重启而非废弃"路径。gVisor 验证仅限 Linux CI,不在 Windows 开发环境强制。
- **Windows 单机 CLI 兜底**:Windows 保持 `tokio::process::Command` 兜底(现状不变)。wasmtime 在 Windows 仅作 PoC 验证可编译性,不强制启用(避免 wasmtime 在 Windows GNU 工具链的编译负担影响单机 CLI 体验)。
- **启用策略**:通过 `SandboxBackend` 枚举(`Process` / `Wasm` / `Gvisor`)选择后端,默认 `Process`(向后兼容),高风险场景显式切换 `Wasm`。

### 决策 3: P1 阶段清理根 Cargo.toml 孤儿声明

- **删除孤儿声明**:删除 [Cargo.toml:58](file:///D:/Chimera%20CLI/Cargo.toml#L58) 的 workspace 级 `wasmtime = "22.0"` 声明。
- **理由**:该声明未被任何 crate 引用,造成依赖图误导(审查者误以为 wasmtime 已集成),违反 §3.4.1 第 6 条"性能可证伪"隐含的依赖可审计性。
- **P3-W12.1 wasmtime PoC 时的声明策略**:由 `crates/seccore/Cargo.toml` 自身显式声明 wasmtime 依赖(`wasmtime = "22.0"`,直接版本声明或 `wasmtime = { workspace = true }` 若届时重新加入 workspace),**不使用 workspace 级声明作为默认**,避免再次孤儿化。若 P3-W12.1 确认 wasmtime 需多 crate 共享,再评估是否恢复 workspace 级声明。

### 决策 4: 高危操作强制升级通道(修复 D6)

针对高危操作审计窗口期风险(D6),引入分级强制升级通道:

| `risk_level` 区间 | 强制措施 | 审计要求 |
|-------------------|---------|---------|
| `[0, 40]` 低风险 | 默认路径(进程级隔离 + 策略过滤) | 周期性 ASA 审计(现状) |
| `[41, 70]` 中风险 | 默认路径 + ASA 审计频率提升 | 周期性 ASA 审计(缩短周期) |
| `[71, 90]` 高风险 | **强制 Parliament 完整辩论 + 自白通道复核** | **ASA 自适应审计前置**(实时,非周期性) |
| `[91, 100]` 极高风险 | **强制 `EscalateToHuman`**(返回特定错误类型,阻断自动执行) | 高危操作决策链**全量上 Merkle 链** |

- **ASA 自适应审计前置**:高危操作(risk_level ≥ 71)在执行前触发 ASA 实时审计(非周期性轮询),确保审计窗口期为零。
- **Parliament 完整辩论**:risk_level ∈ [71,90] 强制经 Parliament 完整辩论流程(SkepticVeto 等机制全启用),并经自白通道( confession channel )复核。
- **EscalateToHuman**:risk_level ∈ [91,100] 强制升级至人工,返回 `SecCoreError::EscalationRequired` 错误类型,阻断自动执行链路,等待人工确认。
- **Merkle 链全量上链**:高危操作决策链(风险评估 → Parliament 辩论 → 审计记录 → 执行结果)全量追加到 `AuditChain` 的 SHA-256 Merkle 链,确保不可篡改。

### 决策 5: wasmtime 安全性合规 — safe API + crate 级 forbid 保持

- **wasmtime 自身 `#![forbid(unsafe_code)]` 不适用**:wasmtime 是第三方 crate,其内部 unsafe(FFI binding 等)不传播到 seccore。`#![forbid(unsafe_code)]` 是 crate 级属性,只约束当前 crate 源码,不传播到依赖(见 §4.1:"crate 级属性,只约束当前 crate 源码,不传播到依赖")。
- **seccore crate 级 `#![forbid(unsafe_code)]` 保持**:seccore 的 `src/lib.rs` crate 级 `#![forbid(unsafe_code)]` 保持不变,wasmtime 引入不影响该属性。
- **仅使用 wasmtime 公开 safe API**:PoC 实现仅使用 wasmtime 公开 safe API,包括但不限于:
  - `wasmtime::Engine::default()`(创建 WASM 引擎)
  - `wasmtime::Store::new(&engine, ())`(创建 Store)
  - `wasmtime::Module::new(&engine, wasm_bytes)`(编译 WASM 模块)
  - `wasmtime::Instance::new(&mut store, &module, &[])`(实例化)
  - `wasmtime::Func` / `wasmtime::Memory` 等 safe 访问器
- **不使用 wasmtime unsafe API**:禁止使用 `wasmtime::*` 中标注 `unsafe` 的 API(如 `unsafe fn` 方法)。若 PoC 发现必须使用 unsafe API,则触发 ADR 流程评估是否特批(依据 ADR-030 unsafe 红线不特批原则,默认拒绝)。
- **合规依据**:ADR-030(v5.0 设计文档 §10 安全架构确立的 unsafe 红线不特批原则)—— wasmtime safe API 不触发 unsafe 红线,合规;若需 unsafe API,则违反红线,需 ADR 特批。

## 理由

### 决策 1 理由(威胁模型下修)

- **§3.4.1 第 6 条(性能可证伪)隐含的"事实可证伪"**:威胁模型必须与代码事实一致,不接受"文档说有 gVisor 但代码没有"的虚高模型。P0-W0.1.4 事实核验已证伪 ADR-001 的二分模型。
- **§6.1 架构红线 3(裸奔)**:Claude Code 尸检教训"命令直接在用户 shell 执行,无隔离"。本项目红线"所有外部调用经 SecCore 沙箱 + Decay 衰减"。威胁模型下修不削弱该红线,而是使红线描述与实际防护能力对齐 —— 沙箱仍然存在(进程级隔离 + 策略过滤),只是从"gVisor 内核级"修正为"进程级"。
- **风险接受依据**:单机 CLI 场景下,用户信任本地输入,进程级隔离 + 静态策略过滤(env_clear / kill_on_drop / timeout / SHA-256 审计)已覆盖主要威胁(命令注入 / 环境泄露 / DoS / 审计绕过)。gVisor 的内核级隔离对单机 CLI 属过度防护。
- **诚实性原则**:ADR-001 的"降级"标注长期暗示"gVisor 部分生效",误导安全评估。下修使威胁模型诚实反映代码能力,符合长期主义(§9 工作要求)。

### 决策 2 理由(wasmtime 沙箱重启路径)

- **高风险场景需求**:多租户 / 不可信代码执行 / 服务端部署场景下,进程级隔离不足以防止沙箱逃逸(子进程仍可访问父进程可见的系统资源)。wasmtime 提供 WASM 虚拟机级隔离(能力模型,显式声明可访问资源),安全性显著高于进程级。
- **§3.4.1 第 7 条(学术支撑落地)**:wasmtime 基于 WebAssembly 标准与 Cranelift JIT,有 Mozilla / Bytecode Alliance 工业级尸检证据支撑其安全性。WASM 能力模型(Capability-based Security)是学术共识的沙箱范式。
- **ADR-001 重启而非废弃**:gVisor 路径在 Linux CI 验证,作为 ADR-001 原决策的"重启"路径(非废弃)。这保留了 ADR-001 的设计意图(Linux 内核级隔离),只是将落地从"主路径"降为"CI 验证路径"。
- **PoC 而非强制启用**:P3-W12.1 仅作 PoC,不强制启用 wasmtime。这避免 wasmtime 编译负担影响单机 CLI 体验(wasmtime 编译时间较长),同时为高风险场景预留升级路径。
- **Windows 兜底原则**:项目主开发环境为 Windows 11 + GNU 工具链,wasmtime 在 Windows GNU 工具链编译负担较重。Windows 保持 `tokio::process::Command` 兜底,符合"单机 CLI 零依赖部署"原则。

### 决策 3 理由(清理孤儿声明)

- **§3.4.1 第 6 条(性能可证伪)的依赖可审计性**:workspace 级孤儿声明使 `cargo tree` / 依赖审查产生误导,违反可审计性。
- **依赖铁律零违规(§1.2)**:项目规则明确"依赖铁律零违规"。孤儿声明虽非直接违规(未被引用),但造成依赖图噪声,应清理以维持依赖图清洁。
- **避免再次孤儿化**:P3-W12.1 wasmtime PoC 时由 seccore 自身 Cargo.toml 声明,避免 workspace 级声明再次孤儿化(若其他 crate 暂未使用 wasmtime)。
- **P1 阶段优先级**:孤儿声明清理归为 P1(核心级),因其影响依赖审查准确性,但非阻断级(不影响功能),可在 P1 阶段与其他核心任务并行。

### 决策 4 理由(高危操作强制升级通道)

- **§6.1 架构红线 3(裸奔)**:Claude Code 尸检教训"命令插值 + auth 跳过"。高危操作若仅经进程级隔离而无实时审计前置,存在审计窗口期风险(操作执行后审计才记录,可被绕过)。
- **§5.3 事件总线**:SkepticVeto / AsaIntervention / BudgetExceeded 为 Critical 级事件,必须用 mpsc channel 确保送达。高危操作强制 Parliament 辩论复用 SkepticVeto 机制,符合 §6.2 红线。
- **ASA 自适应审计前置**:当前 ASA 审计为周期性(N5 修复已将审计改为 pre-execution,但 ASA 审计仍为周期性轮询)。高危操作前置实时审计消除窗口期,符合"零信任"哲学。
- **EscalateToHuman 的 Fail-Safe 原则**:risk_level ∈ [91,100] 强制升级至人工,遵循 Fail-Safe(故障安全)原则 —— 宁可阻断合法操作等待人工确认,也不放行极高风险操作。
- **Merkle 链全量上链**:§6.1 架构红线 4(竞态)与红线(审计绕过)要求审计链不可篡改。高危操作决策链全量上 Merkle 链,确保审计完整性。

### 决策 5 理由(wasmtime 安全性合规)

- **§4.1 编码规范**:"所有 crate 必须 `#![forbid(unsafe_code)]` — crate 级属性,只约束当前 crate 源码,不传播到依赖"。wasmtime 作为依赖,其内部 unsafe 不影响 seccore 的 crate 级 forbid。
- **ADR-030(v5.0 设计文档 unsafe 红线不特批)**:wasmtime safe API 不触发 unsafe 红线,合规。若需 unsafe API,则违反 ADR-030,需 ADR 特批(默认拒绝)。
- **rusqlite bundled 先例**:项目已有 `rusqlite = { version = "0.32", features = ["bundled", "chrono"] }`(根 Cargo.toml L59),rusqlite 内部含 unsafe C binding(bundled SQLite),但 seccore 等 crate 使用 rusqlite safe API 不违反 forbid(unsafe_code)。wasmtime 同理。
- **safe API 足够**:wasmtime 公开 safe API(Engine / Store / Module / Instance / Func)已覆盖 PoC 所需的 WASM 模块加载与执行能力,无需 unsafe API。

## 影响

### 新增内容

- **新增模块**: `crates/seccore/src/sandbox_wasm.rs`(P3-W12.1,新建)
  - `WasmSandbox` 类型(封装 wasmtime Engine / Store)
  - `execute_in_wasm()` 方法(WASM 沙箱执行入口)
  - `SandboxBackend` 枚举(`Process` / `Wasm` / `Gvisor`,后端选择)
- **新增错误变体**: `SecCoreError` 新增
  - `EscalationRequired { risk_level: u8, reason: String }`(决策 4,极高风险升级)
  - `WasmSandboxError(String)`(决策 2,wasmtime 执行失败)
- **新增测试**: wasmtime PoC 测试用例(WASM 模块加载 / 执行 / 超时 / 能力限制验证)
- **新增 ADR**: 本文档(ADR-035)
- **新增 CI 矩阵**: Linux CI 新增 gVisor runsc 验证 job(ADR-001 重启路径)

### 修改内容

- **根 `Cargo.toml`**: 删除 L58 `wasmtime = "22.0"` 孤儿声明(决策 3,P1 阶段)
- **`crates/seccore/Cargo.toml`**: P3-W12.1 新增 `wasmtime = "22.0"` 依赖(直接声明,非 workspace 级)
- **`crates/seccore/src/sandbox.rs`**: `execute_in_sandbox` 文档注释更新,引用 ADR-035 说明降级程度;新增 `SandboxBackend` 分发逻辑
- **`crates/seccore/src/lib.rs`**: 新增 `sandbox_wasm` 模块声明 + `SandboxBackend` / `WasmSandbox` 重导出
- **`crates/seccore/src/error.rs`**: 新增 `EscalationRequired` / `WasmSandboxError` 变体
- **`docs/architecture/ADR-001`**: 补充引用 ADR-035,明确降级程度修正
- **`.trae/rules/nuxus规则.md` §2.3**: ADR-001 条目"降级"标注更新,引用 ADR-035
- **`CHANGELOG.md`**: 新增 ADR-035 决策摘要章节

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35(不变,wasmtime PoC 在 seccore 内) |
| 依赖变更 | P1 删除孤儿声明(减);P3-W12.1 新增 wasmtime(增,仅 seccore) |
| 编译时间 | P3-W12.1 后 seccore 编译时间增加(wasmtime 较重);Windows 仅 PoC 不强制启用,影响可控 |
| Docker/binary 体积 | P1 阶段不变(删除孤儿声明);P3-W12.1 后 release binary 若启用 wasmtime feature 则体积增加(需评估 < 50MB 约束) |
| NexusEvent 变体数 | 不变(复用 SkepticVeto / AsaIntervention / EscalationRequired 走错误返回非事件) |
| SecCoreError 变体数 | 新增 2 个(EscalationRequired / WasmSandboxError) |
| 测试覆盖 | P3-W12.1 新增 wasmtime PoC 测试;决策 4 高危操作升级通道测试 |
| CI 矩阵 | Linux 新增 gVisor runsc 验证 job |
| 威胁模型 | 下修为进程级隔离(与代码一致);高风险场景预留 wasmtime 升级路径 |

## 考虑的方案

### 方案 A: 威胁模型下修 + wasmtime PoC + tokio::process::Command 兜底(采纳)

- **内容**: 决策 1-5 的完整组合。威胁模型下修与代码一致;wasmtime PoC 为高风险场景提供升级路径;Windows 单机 CLI 保持进程级兜底;P1 清理孤儿声明;决策 4 修复高危操作审计窗口期。
- **采纳理由**:
  1. 威胁模型与代码事实一致(诚实性)
  2. wasmtime PoC 为高风险场景预留升级路径(前瞻性)
  3. Windows 兜底保证单机 CLI 可用性(实用性)
  4. 高危操作强制升级通道修复 D6(安全性)
  5. 孤儿声明清理维持依赖图清洁(可审计性)

### 方案 B: 维持 gVisor 决策不变(否决)

- **内容**: 不下修威胁模型,维持 ADR-001 的"gVisor(Linux)+ 进程隔离(Windows/macOS)二分"描述,假设 gVisor 在 Linux 已生效。
- **否决理由**:
  1. **与代码事实不符**:P0-W0.1.4 已证伪 seccore 有 gVisor 依赖,威胁模型虚高。
  2. **违反 §3.4.1 第 6 条(性能可证伪)的"事实可证伪"**:威胁模型不接受主观假设,必须有代码证据。gVisor 无代码证据,违反可证伪。
  3. **安全评估误导**:基于虚高威胁模型的安全评估将低估真实风险(误以为有内核级隔离而放松进程级防护),反而降低安全性。

### 方案 C: 完全去除沙箱(仅 tokio::process::Command,无策略过滤)(否决)

- **内容**: 既然 gVisor 未落地,进一步简化为仅 `tokio::process::Command` 执行,去除策略过滤与审计链。
- **否决理由**:
  1. **违反 §6.1 架构红线 3(裸奔)**:去除策略过滤等于"命令直接在用户 shell 执行",回归 Claude Code 尸检教训。
  2. **高风险场景防护不足**:进程级隔离 + 策略过滤是低风险场景的最低防护,完全去除将使单机 CLI 也暴露于命令注入风险。
  3. **违反 §6.1 架构红线(审计绕过)**:去除审计链使操作无追溯,违反审计完整性。

## 合规性

- **§6.1 架构红线 3(裸奔)**: 符合。所有外部调用仍经 SecCore 沙箱(进程级隔离 + 策略过滤)+ Decay 衰减。威胁模型下修不削弱沙箱,仅修正描述。
- **§6.1 架构红线 4(竞态)**: 符合。高危操作决策链全量上 Merkle 链,审计不可篡改。
- **§6.2 Week 1-8 实战新红线**: 符合。`#![forbid(unsafe_code)]`(seccore crate 级)保持;wasmtime safe API 不触发 unsafe 红线;Critical 安全事件用 mpsc(SkepticVeto / AsaIntervention);不持锁 .await。
- **§4.1 编码规范**: 符合。`#![forbid(unsafe_code)]` crate 级保持;库层 thiserror(SecCoreError 新增变体);无生产路径 unwrap/expect;Top-K 用 `select_nth_unstable`(不涉及)。
- **§3.4.1 第 6 条(性能可证伪)**: 符合。威胁模型下修基于 P0-W0.1.4 事实核验(代码与依赖图实测),非主观判断。wasmtime PoC 需 criterion benchmark 证据。
- **§3.4.1 第 7 条(学术支撑落地)**: 符合。wasmtime 基于 WebAssembly 标准 + Cranelift JIT(Bytecode Alliance 工业级尸检证据);WASM 能力模型(Capability-based Security)是学术共识沙箱范式。
- **ADR-001 沙箱运行时选择**: 本 ADR 修正其降级程度(从"gVisor + 进程隔离二分"修正为"全平台进程级隔离"),不废弃 ADR-001。gVisor 路径在 Linux CI 重启验证(决策 2)。
- **ADR-030(v5.0 设计文档 unsafe 红线不特批)**: 符合。wasmtime safe API 不触发 unsafe 红线;若需 unsafe API,则违反 ADR-030,需 ADR 特批(默认拒绝)。
- **ADR-005 持久化存储选型(sqlite-vec 禁用先例)**: 类比合规。sqlite-vec 因 unsafe binding 禁用,改内存 KNN;wasmtime 虽含内部 unsafe,但 seccore 仅用 safe API,不引入 unsafe 到 seccore 源码,合规。
- **§10.2 Docker 镜像约束**: P1 阶段不受影响(仅删除孤儿声明);P3-W12.1 启用 wasmtime feature 后需评估 release binary 体积 < 50MB 约束(wasmtime 可能增加体积,需 feature gate 控制)。
- **§2.2 依赖铁律**: 符合。wasmtime 为外部依赖,seccore(L4 Security)引入外部 crate 不违反依赖方向规则(无向上依赖)。

## 相关文档

- **ADR-001**: 沙箱运行时选择(gVisor → 降级为 tokio::process::Command,本 ADR 修正其降级程度)
- **ADR-030**: unsafe 红线不特批(v5.0 设计文档 §10 安全架构确立,wasmtime safe API 合规依据)
- **ADR-005**: 持久化存储选型(sqlite-vec 禁用先例,unsafe binding 类比)
- **ADR-012**: 内存安全(forbid unsafe 全覆盖哲学,wasmtime 不传播依据)
- **v5.0 设计文档**: §10 安全架构(ADR-001 真实降级程度修正)/ §14 实施路线图(P3-W12.1 wasmtime PoC)/ C5 决策(威胁模型下修)
- **项目代码**:
  - [sandbox.rs](file:///D:/Chimera%20CLI/crates/seccore/src/sandbox.rs)(沙箱执行核心,L174 降级注释 / L181 execute_in_sandbox / L195 env_clear / L206 kill_on_drop / L212 timeout / L253 compute_audit_hash)
  - [seccore/Cargo.toml](file:///D:/Chimera%20CLI/crates/seccore/Cargo.toml)(零 wasmtime 依赖核验依据)
  - [Cargo.toml](file:///D:/Chimera%20CLI/Cargo.toml)(L58 孤儿声明 `wasmtime = "22.0"`,P1 阶段清理目标)
- **规则**: `.trae/rules/nuxus规则.md` §2.3(ADR 表,ADR-001 降级标注同步)/§4.1(编码规范,forbid unsafe)/§6.1(架构红线)/§6.2(Week 1-8 新红线)/§10.2(Docker 镜像约束)
- **CODE_WIKI.md**: `docs/architecture/CODE_WIKI.md` §2.3(ADR 表,本 ADR 同步更新)
- **ADR 索引**: `docs/architecture/adr_index.md`(本 ADR 同步更新)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-23
> **基线版本**: v2.3.1-omega(创建时)
> **决策者**: E01 首席架构师 + E02 安全架构师 + E05 生产系统专家(分布式评审)
> **事实核验**: P0-W0.1.4(seccore 零 wasmtime 依赖 / sandbox.rs 全平台进程级隔离 / `wasmtime = "0.39"` 捏造纠正 / 根 Cargo.toml L58 孤儿声明)
> **实施路线图**: P1(清理孤儿声明)→ P3-W12.1(wasmtime PoC + Linux gVisor CI 验证 + 决策 4 高危操作升级通道)
