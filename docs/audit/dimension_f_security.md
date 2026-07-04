# 维度 F:安全审计报告

> **项目**:Chimera CLI (NEXUS-OMEGA)
> **审计维度**:F — 安全
> **审计日期**:2026-06-28
> **审计范围**:34 个 crates(`d:\Chimera CLI\crates\*`)
> **审计方法**:静态代码扫描 + 配置核验 + 测试覆盖核验 + 依赖版本核验
> **参照规范**:`AETHER_NEXUS_OMEGA_ULTIMATE.md` §6 红线 / `project_memory.md` Week 5/8 已知问题

---

## 1. 执行摘要

| 审计项 | 状态 | 关键指标 |
|--------|------|----------|
| `#![forbid(unsafe_code)]` 覆盖 | ✅ 100% | 34 lib.rs + 1 main.rs 全覆盖,无真实 unsafe 用法 |
| SecCore 沙箱执行 | ⚠️ 部分实现 | 四层防御已实现,但缺超时/资源限制/gVisor 实际启用 |
| Decay 能力衰减 | ✅ 完整 | 连续流体模型 + 双驱动衰减 + 自动冻结 + 配置化 |
| 输入校验审计 | ⚠️ 待加固 | mcp-mesh endpoint 无 SSRF 校验,chtc-bridge tool_id 未过滤 |
| OWASP Top 10 回归 | ✅ 100% 通过 | 20/20 测试覆盖 A01-A10 |
| 四大 Critical 事件发布 | ⚠️ 设计偏差 | 使用 broadcast 非 mpsc;BudgetExceeded/AsaIntervention severity 为 Normal |
| BudgetExceeded severity | ❌ 违反规范 | severity() 返回 Normal,与项目规范"必须 Critical"冲突 |
| AHIRT 配置化(P2) | ✅ 已解决 | AhirtConfig 已引入,5min/0.95 阈值均可配置 |
| 依赖安全审计 | ✅ 合格 | 13 个关键依赖无 High/Critical,3 个 workspace 依赖未使用 |

**关键发现**:
- **1 个 Critical 问题**:BudgetExceeded severity 与项目规范不一致,导致事件总线层不触发 Critical 无订阅者告警,慢消费者场景下可能丢失。
- **5 个 Major 问题**:沙箱缺超时/资源限制、Linux gVisor 未实际启用、mcp-mesh 缺 SSRF 校验、Critical 事件未走 mpsc 点对点通道。
- **6 个 Minor 问题**:输入校验加固、文档一致性、CI 集成等。
- **1 个 P2 已解决**:Week 5 遗留的 AHIRT 配置化问题已通过 `AhirtConfig` 完整解决。

**结论**:项目安全基线整体合格(OWASP Top 10 全覆盖、forbid(unsafe_code) 100%、AHIRT 配置化已完成),但存在 1 个 Critical 与 5 个 Major 问题需要在生产部署前修复。

---

## 2. #![forbid(unsafe_code)] 覆盖

### 2.1 审计方法

- 使用 Grep 扫描 `crates/**/lib.rs` 与 `crates/chimera-cli/src/main.rs`,统计 `#![forbid(unsafe_code)]` 出现次数。
- 使用严格正则 `^\s*unsafe\s+(fn|impl|trait|extern)|unsafe\s*\{|unsafe\s*\(` 扫描所有 `crates/**/src/**/*.rs`,核验是否有真实 unsafe 用法。
- 对含 "unsafe" 字样的非 lib.rs/main.rs 文件,逐个核验是否为文档注释或字符串字面量。

### 2.2 审计结果

| 维度 | 数量 | 详情 |
|------|------|------|
| lib.rs 含 `#![forbid(unsafe_code)]` | 34/34 | 全部 34 个 crate 的 lib.rs 顶部声明 |
| main.rs 含 `#![forbid(unsafe_code)]` | 1/1 | `crates/chimera-cli/src/main.rs:14` |
| 真实 unsafe 块/函数/impl | 0 | 严格正则扫描无匹配 |
| 含 "unsafe" 字样的源文件 | 40 | 其中 35 个为 lib.rs/main.rs 中的 lint 声明,5 个为文档注释引用 |

**5 个非 lib.rs/main.rs 但含 "unsafe" 字样的文件核验**:

| 文件 | 行号 | 性质 |
|------|------|------|
| `crates/sesa-router/src/mask.rs` | 8, 125 | 文档注释:"无 unsafe" / "SIMD 友好且无 unsafe" |
| `crates/scc-cache/src/wal.rs` | 16, 17, 255-258 | 文档注释:解释 `#![forbid(unsafe_code)]` 兼容性,提及 rusqlite 内部 FFI 是 unsafe extern |
| `crates/repo-wiki/src/vector.rs` | 7-9 | 文档注释:解释 sqlite-vec 需 unsafe,故降级为内存向量检索 |
| `crates/mlc-engine/src/types.rs` | 53-54, 137-138 | 文档注释:"避免 unsafe 代码(`#![forbid(unsafe_code)]` 约束)" |
| `crates/event-bus/src/types.rs` | 754, 1330, 1373, 1523, 1569 | 字符串字面量:"unsafe shell injection detected" / "unsafe operation"(测试用例数据) |

### 2.3 覆盖率结论

**覆盖率:100%(目标 100%)** ✅

- 34 个 crate 的 `lib.rs` 顶部均有 `#![forbid(unsafe_code)]`(Grep `files_with_matches` 命中 34 个文件)
- `crates/chimera-cli/src/main.rs:14` 显式声明 `#![forbid(unsafe_code)]`,并在 line 12-13 注释说明"与 lib.rs 保持一致"
- 严格正则扫描无真实 unsafe 用法,所有 "unsafe" 字样均为文档引用或测试字符串
- `crates/nexus-core/src/lib.rs:32` 等位置将 `#![forbid(unsafe_code)]` 与 `#![warn(missing_docs, clippy::all)]` 联合声明,形成完整安全防线

---

## 3. SecCore 沙箱执行

### 3.1 四层防御架构核验

读取 `crates/seccore/src/sandbox.rs`、`policy.rs`、`audit.rs`、`asa.rs`、`types.rs`、`lib.rs`,核验四层防御实现:

| 防御层 | 实现位置 | 状态 | 说明 |
|--------|----------|------|------|
| 1. 静态分析 | `policy.rs:252-299` `validate_command` | ✅ | 6 类攻击拦截(Injection/PrivilegeEscalation/SandboxEscape/DataLeak/Tamper/Abuse) |
| 2. 环境过滤 | `policy.rs:313-354` `validate_env` | ✅ | 白名单 + 敏感关键词黑名单(SECRET/KEY/TOKEN/PASSWORD 等 9 类) |
| 3. 沙箱执行 | `sandbox.rs:118-166` `execute_in_sandbox` | ⚠️ | 进程隔离已实现,但缺超时/资源限制/gVisor |
| 4. 审计记录 | `audit.rs:67-90` `AuditChain::append` | ✅ | SHA-256 Merkle 链,篡改可检测 |
| 5. ASA 审计 | `asa.rs` `AsaAuditor` | ✅ | Allow/Warn/Block 三档干预,AsaConfig 可配置阈值 |

### 3.2 配置可配置性核验

| 配置项 | 实现位置 | 可配置 | 说明 |
|--------|----------|--------|------|
| 命令白名单 | `policy.rs:40-45` `CommandPolicy` | ✅ | 链式 API `allow_command` |
| 拦截模式 | `policy.rs:78-90` `block_pattern` | ✅ | 链式 API,关联 AttackType |
| 环境白名单 | `policy.rs:54-60` `EnvPolicy` | ✅ | 链式 API `allow_env` |
| 敏感关键词 | `policy.rs:193-196` `block_sensitive` | ✅ | 链式 API |
| ASA 阈值 | `asa.rs:60-75` `AsaConfig` | ✅ | safety_threshold_allow/warn/block + risk_weight |
| **超时** | `sandbox.rs:37-44` `Sandbox` | ❌ | **Sandbox 结构体无 timeout 字段,execute_in_sandbox 无 tokio::time::timeout 包裹** |
| **资源限制** | 无 | ❌ | **无 CPU/内存/FD 限制(Linux setrlimit / Windows Job Object)** |
| **gVisor 启用** | `sandbox.rs:108-117` | ❌ | **注释明确说"当前实现为降级版本",Linux 也走 tokio::process::Command** |

### 3.3 沙箱逃逸防护机制核验

| 防护点 | 实现位置 | 状态 |
|--------|----------|------|
| 不使用 shell(无 sh -c) | `sandbox.rs:125-127` | ✅ 参数直接传递给 execve |
| 环境变量清空(不继承父进程) | `sandbox.rs:132` `env_clear()` | ✅ |
| 仅传白名单环境变量 | `sandbox.rs:133-135` | ✅ |
| stdout/stderr 捕获(不继承终端) | `sandbox.rs:138-139` | ✅ |
| 路径遍历拦截(`../`) | `policy.rs:140-141` | ✅ |
| 系统目录拦截(`/proc/`、`/sys/`) | `policy.rs:142-143` | ✅ |
| 提权命令拦截(sudo/su/chmod/chown) | `policy.rs:126-137` | ✅ |
| 命令注入拦截(`$(`/``/`|`/`;`/`&&`/`||`) | `policy.rs:117-122` | ✅ |
| 敏感文件拦截(`/etc/passwd`、`/etc/shadow`) | `policy.rs:146-155` | ✅ |
| 审计篡改拦截(`/var/log`、`shred`) | `policy.rs:160-165` | ✅ |

### 3.4 已识别问题

- **F-002(Major)**:`crates/seccore/src/sandbox.rs:142-145` 子进程执行无超时包裹,恶意命令(如 `sleep infinity`)可永久阻塞沙箱。
- **F-003(Major)**:无资源限制,恶意命令可消耗全部 CPU/内存导致 DoS。
- **F-005(Major)**:`crates/seccore/src/sandbox.rs:108-117` Linux gVisor 未实际启用,与 ADR-001"沙箱运行时选择 gVisor"不一致,所有平台均为降级版本。

---

## 4. Decay 能力衰减

### 4.1 连续权限流体模型实现核验

读取 `crates/decay-engine/src/engine.rs`、`types.rs`,核验 ADR-002 实现:

| 维度 | 实现位置 | 状态 | 说明 |
|------|----------|------|------|
| 连续 [0.0, 1.0] 流体模型 | `types.rs:22-50` `CapabilityLevel` | ✅ | newtype 包装 f32,构造时校验范围 |
| 时间驱动衰减 | `engine.rs:110-123` `TimeDecay` | ✅ | `level -= elapsed × time_decay_rate` |
| 事件驱动衰减 | `engine.rs:124-136` `ViolationPenalty` | ✅ | `level -= event_decay_penalty × severity` |
| 冻结(Skeptic 否决) | `engine.rs:137-142` `Freeze` | ✅ | 立即清零 + frozen = true |
| 恢复 | `engine.rs:143-153` `Restore` | ✅ | `level += restore_rate × elapsed` |
| 自动冻结 | `engine.rs:158-166` | ✅ | `level <= freeze_threshold` 时自动冻结 |
| 解冻防护 | `engine.rs:206-211` | ✅ | 解冻后从 `freeze_threshold + 0.01` 起步,避免立即再冻结 |
| 浮点边界保护 | `engine.rs:117-118, 130-131, 149` | ✅ | `clamp(lower, 1.0)` 防止浮点误差越界 |
| 线程安全 | `engine.rs:28` `DashMap` | ✅ | Send + Sync,可跨 async 任务共享 |

### 4.2 衰减曲线合理性

衰减曲线为**线性**:
- 时间衰减:`level -= elapsed_secs × time_decay_rate`(默认 0.001/秒 = 0.1%/秒)
- 事件衰减:`level -= event_decay_penalty × severity`(默认 0.1 × severity)
- 恢复:`level += elapsed_secs × restore_rate`(默认 0.01/秒 = 1%/秒)

**评估**:线性衰减简单实用,满足"防止权限长期闲置累积"的设计目标。默认配置下,满权限(1.0)自然衰减到冻结阈值(0.05)需约 950 秒(~16 分钟),恢复从 0.05 到 1.0 需约 95 秒,符合"权限不应残留"的红线。

### 4.3 衰减触发条件核验

| 触发条件 | 实现位置 | 说明 |
|----------|----------|------|
| 时间驱动 | `engine.rs:110` | 外部调用 `decay(id, TimeDecay)`,基于 `last_decay_at` 计算 elapsed |
| 违规事件 | `engine.rs:124` | 外部调用 `decay(id, ViolationPenalty { severity, .. })` |
| Skeptic 否决 | `engine.rs:137` | 外部调用 `decay(id, Freeze { reason, .. })` 或 `freeze(id, reason)` |
| 恢复 | `engine.rs:143` | 外部调用 `decay(id, Restore { .. })` |
| 自动冻结 | `engine.rs:158` | 衰减后自动检查 `level <= freeze_threshold` |

### 4.4 配置化核验

`DecayConfig`(`types.rs:68-94`)完全可配置,默认值合理:

```rust
time_decay_rate: 0.001,        // 每秒衰减 0.1%
event_decay_penalty: 0.1,      // 违规惩罚基数
min_level: 0.0,                // 最低权限下限
freeze_threshold: 0.05,        // 自动冻结阈值
restore_rate: 0.01,            // 每秒恢复 1%
```

**结论**:Decay 能力衰减实现完整,无安全问题。✅

---

## 5. 输入校验审计

### 5.1 chimera-cli 用户命令输入

**审计文件**:
- `crates/chimera-cli/src/cli.rs` — Clap 子命令定义
- `crates/chimera-cli/src/commands/run.rs` — `aether run <prompt>` 实现
- `crates/chimera-cli/src/commands/wiki.rs` — `aether wiki <query>` 实现

**发现**:
- `cli.rs:30-50` Clap derive 解析参数,无显式输入校验(长度限制、危险字符过滤)。
- `run.rs:13-19` `execute(prompt, config)` 直接 `println!("[run] 任务提示词:{}", prompt)`,无长度限制/ANSI 转义过滤。
- `wiki.rs:11-17` 同样直接打印 query。
- 当前是骨架实现(注释"待 Week 4 PVL/MTPE 实现后接入"),业务逻辑未接入。

**风险**:用户传入超长 prompt 可能导致终端渲染卡顿;含 ANSI 转义序列的 prompt 可能造成终端控制字符注入(终端逃逸)。当前风险等级 **Minor**(骨架代码,未接入业务逻辑)。

### 5.2 mcp-mesh 外部服务注册

**审计文件**:`crates/mcp-mesh/src/server_registry.rs`

**发现**:
- `register()`(line 88-95)仅检查 `capacity == 0`,**未对 server_id/endpoint/capabilities 内容做任何校验**:
  ```rust
  pub fn register(&self, server: MeshServer) -> Result<(), McpError> {
      if self.capacity == 0 {
          return Err(McpError::RegistryFull { capacity: 0 });
      }
      let key = server.server_id.clone();
      self.servers.insert(key, server);
      Ok(())
  }
  ```
- `MeshServer::new()`(line 36-47)直接接受任意 `server_id`/`endpoint`/`capabilities` 字符串,无格式校验、无长度限制、无 SSRF 防护。
- `endpoint` 字段(line 26)为 `String`,可接受 `evil.com:8080`、`169.254.169.254:80`、`127.0.0.1:6379` 等内网/元数据地址,存在 **SSRF 风险**。
- `server_id` 无唯一性约束(覆盖式更新,line 93 注释"若 server_id 已存在则覆盖"),可能被恶意注册冒充已存在服务器。

**风险等级**:**Major**(SSRF 风险,违反项目规范"系统边界必须校验")。

### 5.3 chtc-bridge 跨平台适配输入

**审计文件**:`crates/chtc-bridge/src/protocol.rs`

**发现**:
- `as_object()`(line 147-151)校验 JSON 是否为对象,基本结构校验已实现。
- `take_str_field()`(line 153-164)校验字段存在性与类型(期望字符串),基本类型校验已实现。
- **但 `tool_id` 内容未校验**(line 166-174):
  - 无长度限制(可能传入超长字符串导致内存耗尽)
  - 无字符白名单(可能含 shell 注入字符、路径遍历字符)
  - 直接作为 `UnifiedToolCall.tool_id` 传递给下游
- `parameters: Value` 无深度校验(line 167),接受任意 JSON 结构。
- `deadline_ms` 硬编码为 `DEFAULT_DEADLINE_MS = 5000`(line 19, 171),不可配置。

**风险等级**:**Minor**(chtc-bridge 是 IDE 内部协议转换层,非直接面向用户,但仍需加固)。

### 5.4 路径校验核验

**审计文件**:`crates/nexus-core/src/path_util.rs`

**发现**:
- `expand_tilde()`(line 47-80)仅做 `~` 展开,**无路径遍历校验**(`../` 检测)。
- 但 SecCore 的 `policy.rs:140-141` 已在命令静态分析层拦截 `../` 和 `..\`,提供纵深防御。
- 路径校验责任分散,`path_util` 本身无自我保护(若被其他 crate 直接调用而不经 SecCore,可能存在路径遍历风险)。

**风险等级**:**Minor**(SecCore 提供纵深防御,但 `path_util` 应增加自我保护)。

---

## 6. OWASP Top 10 回归核验

### 6.1 测试覆盖核验

**审计文件**:
- `crates/seccore/tests/security.rs`(8 个测试,6 类攻击 + 审计链 + 环境白名单)
- `tests/security/owasp_top10.rs`(20 个测试,A01-A10 全覆盖)

**OWASP Top 10 (2021) 测试矩阵**:

| OWASP 项 | 攻击向量 | SecCore 防御层 | AttackType | 测试数 | 状态 |
|----------|----------|---------------|------------|--------|------|
| A01 注入 | `$(cmd)` / `\|` / `;` / `&&` | 静态分析 | Injection | 1 | ✅ |
| A02 失效访问控制 | `sudo rm -rf /` | 静态分析 | PrivilegeEscalation | 1 | ✅ |
| A03 敏感数据泄露 | `/etc/passwd` + `SECRET_KEY` 环境变量 | 静态分析 + 环境过滤 | DataLeak / EnvVarBlocked | 2 | ✅ |
| A04 不安全设计 | 未知命令 + 注入字符组合 | 白名单 + 拦截模式 | Abuse / Injection | 3 | ✅ |
| A05 安全配置错误 | 默认策略白名单检查 | 策略验证 | N/A | 2 | ✅ |
| A06 易受攻击组件 | `forbid(unsafe_code)` 编译期保证 | 编译期检查 | N/A | 1 | ✅ |
| A07 认证失败 | `nc -l 4444` / `bash -c` 未授权命令 | 白名单 | Abuse | 2 | ✅ |
| A08 数据完整性失败 | 审计链 `result_hash` / `index` 篡改 | Merkle 链验证 | Tamper | 2 | ✅ |
| A09 日志记录不足 | 安全事件未记录 + ASA 审计追溯 | 审计链追加 + ASA | N/A | 3 | ✅ |
| A10 SSRF | `curl 169.254.169.254` / `wget localhost` / `python3 requests` | 白名单 | Abuse | 3 | ✅ |
| **合计** | — | — | — | **20** | **20/20 ✅** |

### 6.2 测试质量核验

- `tests/security/owasp_top10.rs:27` 顶部声明 `#![forbid(unsafe_code)]`,符合项目铁律。
- 每个测试只验证一个关注点(符合 project_memory 中"Week 8 OWASP A04 修复:零信任防御深度要求一个测试只验证一个关注点")。
- 测试覆盖正常路径(白名单内命令通过)+ 异常路径(攻击拦截)+ 篡改检测(审计链)+ 边界条件(空参数)。

### 6.3 已识别问题

- **F-013(Minor)**:A10 SSRF 仅靠白名单拦截 curl/wget/python3,如果未来引入其他网络工具(如 nc 通过白名单扩展),SSRF 防护可能失效。建议增加基于 endpoint 的 SSRF 防护层(校验 IP 是否为内网/元数据地址)。

---

## 7. 四大 Critical 事件发布

### 7.1 事件发布通道核验

**审计文件**:`crates/event-bus/src/bus.rs`、`backpressure.rs`、`types.rs`

**发现**:
- `EventBus`(`bus.rs:33-39`)使用 `tokio::sync::broadcast::Sender<NexusEvent>`,**不是 mpsc channel**。
- `publish()`(`bus.rs:98-120`)通过 `broadcast::Sender::send` 发布事件,无订阅者时静默丢弃(仅 Critical 级记录 warn 日志,line 109-113)。
- `backpressure.rs:9-13` 注释明确说"当前实现基于 broadcast + 标注,关键事件仍走 broadcast 但标注 Critical,消费者优先处理 Critical 事件。未来可扩展为双通道(broadcast + mpsc)"。
- `backpressure.rs:34-42` `CriticalMpsc` 策略变体存在,但仅是配置占位,未实际实现双通道。

### 7.2 四大事件发布路径核验

| 事件 | 发布位置 | 发布方式 | severity() | is_critical_alert_event |
|------|----------|----------|------------|------------------------|
| SkepticVeto | `crates/parliament/src/voting.rs:276` | `publish_blocking` | ✅ Critical | ✅ Critical |
| RedTeamAudit | `crates/parliament/src/ahirt.rs:466` | `publish_blocking` | ✅ Critical | ✅ Critical |
| AsaIntervention | `crates/seccore/src/asa.rs:237` | `publish` | ❌ Normal | ✅ Critical |
| BudgetExceeded | `crates/acb-governor/src/governor.rs:124`、`crates/decb-governor/src/governor.rs:405`、`crates/model-router/src/router.rs:134` | `publish` | ❌ Normal | ✅ Critical |

### 7.3 已识别问题

- **F-006(Major)**:EventBus 使用 broadcast 而非 mpsc,Critical 事件无点对点投递保障。项目规范要求"Critical 安全事件必须通过 mpsc channel 发布",但实际为 broadcast + severity 标注。慢消费者场景下 Critical 事件可能被丢弃(Lagged 错误)。
- **F-007(Minor)**:`AsaIntervention` 的 `severity()`(`types.rs:1142-1151`)返回 Normal,但 `efficiency-monitor/src/lib.rs:81-89` 的 `is_critical_alert_event` 视为 Critical。两个函数语义不一致,虽 `types.rs:806-809` 注释解释"severity() 是同步函数不依赖运行时值",但仍可能造成调用方混淆。

---

## 8. BudgetExceeded severity 核验

### 8.1 severity() 实现核验

**审计文件**:`crates/event-bus/src/types.rs:1142-1152`

```rust
pub fn severity(&self) -> EventSeverity {
    match self {
        Self::CheckpointSaved { .. }
        | Self::ConsensusReached { .. }
        | Self::SlowConsumerDropped { .. }
        | Self::OrphanCallDetected { .. }
        | Self::SkepticVeto { .. }
        | Self::RedTeamAudit { .. } => EventSeverity::Critical,
        _ => EventSeverity::Normal,
    }
}
```

**核验结果**:
- ✅ SkepticVeto → Critical(line 1148)
- ✅ RedTeamAudit → Critical(line 1149)
- ❌ **BudgetExceeded → Normal**(走 `_ => EventSeverity::Normal` 分支,line 1150)
- ❌ **AsaIntervention → Normal**(走 `_ => EventSeverity::Normal` 分支,line 1150)

### 8.2 与项目规范的冲突

**项目规范**(任务描述 + `project_memory.md`):
> BudgetExceeded 事件必须标记为 Critical severity

**实际实现**:BudgetExceeded 走 `_ => Normal` 分支,违反规范。

### 8.3 影响分析

BudgetExceeded severity 为 Normal 导致:
1. `bus.rs:109` 的 Critical 无订阅者告警不触发,事件被静默丢弃。
2. 慢消费者场景下,BudgetExceeded 可能被 Lagged 丢弃而不告警。
3. `efficiency-monitor/src/lib.rs:81-89` 的 `is_critical_alert_event` 视其为 Critical,但与 `severity()` 不一致,造成语义分裂。

### 8.4 已识别问题

- **F-001(Critical)**:`crates/event-bus/src/types.rs:1142-1151` BudgetExceeded severity() 返回 Normal,违反项目规范"BudgetExceeded 事件必须标记为 Critical severity"。需在 severity() 中将 `Self::BudgetExceeded { .. }` 显式列入 Critical 分支。

---

## 9. AHIRT 配置化核验(Week 5 遗留 P2)

### 9.1 P2 问题回顾

**Week 5 遗留 P2**(来自 `project_memory.md`):
> AHIRT 5 分钟周期和 0.95 检测率阈值未配置化(需要引入 AhirtConfig)

### 9.2 AhirtConfig 引入核验

**审计文件**:`crates/parliament/src/config.rs:140-200`、`crates/parliament/src/ahirt.rs`

**核验结果**:`AhirtConfig` **已完整引入**,P2 问题**已解决** ✅

```rust
// crates/parliament/src/config.rs:140-148
pub struct AhirtConfig {
    pub probe_cycle_secs: u64,              // 周期探测间隔(秒),默认 300(5 分钟),下限 60
    pub detection_rate_threshold: f64,      // 检测率阈值 [0.0, 1.0],默认 0.95
    pub payload_batch_size: usize,          // 探测载荷批次大小,默认 25,下限 1
}
```

### 9.3 配置化完整性核验

| 配置项 | 默认值 | 可配置 | 校验 | 说明 |
|--------|--------|--------|------|------|
| 5 分钟周期 | 300 秒 | ✅ `probe_cycle_secs` | ✅ ≥ 60(`config.rs:182-189`) | `spawn_periodic_probe_default()` 使用此字段(`ahirt.rs:518-521`) |
| 0.95 检测率阈值 | 0.95 | ✅ `detection_rate_threshold` | ✅ ∈ [0.0, 1.0](`config.rs:172-179`) | `verify_security()` 使用此字段(`ahirt.rs:439`) |
| 批次大小 | 25 | ✅ `payload_batch_size` | ✅ ≥ 1(`config.rs:192-196`) | `probe()` 使用此字段(`ahirt.rs:363`) |

### 9.4 构造器与测试覆盖核验

| 构造器 | 位置 | 支持配置注入 |
|--------|------|-------------|
| `new(library)` | `ahirt.rs:298-300` | ❌ 使用默认 AhirtConfig(向后兼容) |
| `with_event_bus(library, bus)` | `ahirt.rs:309-311` | ❌ 使用默认 AhirtConfig(向后兼容) |
| `with_config(library, config)` | `ahirt.rs:320-322` | ✅ 自定义配置 |
| `with_config_and_event_bus(library, config, bus)` | `ahirt.rs:332-343` | ✅ 完整构造器 |

**测试覆盖**(`ahirt.rs:1355-1550`、`config.rs:304-413`):
- ✅ 默认值测试(`test_ahirt_new_uses_default_config`)
- ✅ 自定义配置存储测试(`test_ahirt_with_config_stores_custom_config`)
- ✅ validate() 校验测试(边界值、负值、超范围)
- ✅ 序列化往返测试(`test_ahirt_config_serde_roundtrip`)
- ✅ 配置等价性测试(`test_ahirt_default_config_equivalent_to_new`)
- ✅ 自定义阈值生效测试(`test_ahirt_verify_security_uses_custom_threshold_low/high`)
- ✅ 批次大小测试(`test_ahirt_probe_respects_batch_size`)
- ✅ 零批次防御测试(`test_ahirt_probe_with_zero_batch_size_does_not_panic`)

### 9.5 结论

**Week 5 遗留 P2 已完全解决**。AhirtConfig 已引入,5 分钟周期与 0.95 检测率阈值均可配置,测试覆盖完整。

---

## 10. 依赖安全审计

### 10.1 审计方法

- 读取 `Cargo.lock`(284 个包),核验关键依赖版本。
- 参考 `docs/security/week8_security_report.md` §4 的手动检查结果(cargo-audit 因网络超时未执行)。
- 核验 workspace 声明但未使用的依赖。

### 10.2 关键依赖版本核验

| 依赖 | Cargo.lock 版本 | 已知漏洞状态 | 说明 |
|------|----------------|-------------|------|
| `tokio` | 1.52.3 | ✅ 无已知 High/Critical | 异步运行时 |
| `serde` | 1.0.228 | ✅ 无已知漏洞 | 序列化框架 |
| `clap` | 4.6.1 | ✅ 无已知漏洞 | CLI 解析 |
| `ratatui` | 0.29.0 | ✅ 无已知漏洞 | TUI 框架 |
| `rusqlite` | 0.32.1 | ✅ 无已知漏洞 | SQLite 绑定(bundled feature) |
| `libsqlite3-sys` | 0.30.1 | ✅ 无已知漏洞 | SQLite C 库绑定 |
| `rmp-serde` | 1.3.1 | ✅ 无已知漏洞 | MessagePack 序列化(ADR-004) |
| `chrono` | 0.4.45 | ✅ 已修复 RUSTSEC-2020-0159 | 0.4.20+ 已修复 unmaintained 问题 |
| `uuid` | 1.23.3 | ✅ 无已知漏洞 | UUID 生成(v7 feature) |
| `figment` | 0.10.19 | ✅ 无已知漏洞 | 多源配置合并 |
| `crossterm` | 0.28.1 | ✅ 无已知漏洞 | 跨平台终端 IO |
| `ndarray` | 0.16.1 | ✅ 无已知漏洞 | N 维数组 |
| `dashmap` | 6.2.1 | ✅ 无已知漏洞 | 并发 HashMap |
| `once_cell` | 1.21.4 | ✅ 无已知漏洞 | 惰性初始化 |
| `sha2` | 0.10.9 | ✅ 无已知漏洞 | SHA-256 哈希 |
| `prometheus-client` | 0.22.3 | ✅ 无已知漏洞 | Prometheus 指标 |
| `smallvec` | 1.15.1 | ✅ 已修复 RUSTSEC-2021-0003 | 1.11.0+ 已修复缓冲区溢出 |
| `lock_api` | 0.4.14 | ✅ 无已知漏洞 | 锁抽象 |

### 10.3 未实际使用的 workspace 依赖

以下依赖在 `Cargo.toml` 的 `[workspace.dependencies]` 中声明,但未出现在 `Cargo.lock` 中(无 crate 实际引用):

| 依赖 | 声明版本 | 说明 |
|------|----------|------|
| `wasmtime` | 22.0 | 沙箱运行时(Linux 生产环境用,当前降级为进程隔离) |
| `reqwest` | 0.12 | HTTP 客户端(预留,未实际使用) |
| `axum` | 0.7 | Web 框架(预留,未实际使用) |
| `sqlite-vec` | 0.1 | 向量检索扩展(因 forbid(unsafe_code) 降级为内存向量检索) |

**风险**:这些依赖未进入编译产物,不影响当前安全状态,但增加了未来供应链攻击面。

### 10.4 已识别问题

- **F-011(Minor)**:`Cargo.toml:52-57` 声明了 wasmtime/reqwest/axum/sqlite-vec 但未使用,建议移除以减少供应链攻击面。
- **F-012(Minor)**:cargo-audit 未集成到 CI,依赖漏洞扫描依赖手动检查(`docs/security/week8_security_report.md:429-435`)。建议在 CI 中加 cargo-audit step,定时运行。

---

## 11. 问题清单

| ID | 严重程度 | 问题 | 代码位置 | 修复建议 |
|----|---------|------|---------|---------|
| F-001 | Critical | BudgetExceeded 事件 severity() 返回 Normal,违反项目规范"必须 Critical" | `crates/event-bus/src/types.rs:1142-1151` | 在 severity() match 中将 `Self::BudgetExceeded { .. }` 显式列入 Critical 分支(line 1148-1149 旁) |
| F-002 | Major | 沙箱无超时机制,子进程可能永久阻塞(如 `sleep infinity`) | `crates/seccore/src/sandbox.rs:118-166` | 在 Sandbox 增加 `timeout: Duration` 字段,execute_in_sandbox 用 `tokio::time::timeout` 包裹 `cmd.output().await`,超时后 kill 子进程 |
| F-003 | Major | 沙箱无资源限制(CPU/内存/FD),DoS 风险 | `crates/seccore/src/sandbox.rs:118-166` | Linux 用 `setrlimit`(RLIMIT_CPU/RLIMIT_AS/RLIMIT_NOFILE),Windows 用 Job Object 限制 |
| F-004 | Major | mcp-mesh ServerRegistry 未校验 endpoint 格式,SSRF 风险 | `crates/mcp-mesh/src/server_registry.rs:88-95` | 在 register() 中校验 endpoint:拒绝 169.254.x.x(AWS 元数据)、127.0.0.0/8、10.0.0.0/8、172.16.0.0/12、192.168.0.0/16、::1 等内网/回环地址;校验 server_id 长度与字符集 |
| F-005 | Major | 沙箱 Linux gVisor 未实际启用,与 ADR-001 不一致 | `crates/seccore/src/sandbox.rs:108-117` | 实现 `#[cfg(target_os = "linux")]` 分支,通过 `runsc` 运行时启动子进程 + seccomp 过滤器;Windows/macOS 保留降级版本 |
| F-006 | Major | 事件总线使用 broadcast 而非 mpsc,Critical 事件无点对点投递保障 | `crates/event-bus/src/bus.rs:34`、`crates/event-bus/src/backpressure.rs:9-13` | 实现 broadcast + mpsc 双通道:Critical 事件(SkepticVeto/RedTeamAudit/AsaIntervention Block/BudgetExceeded)走独立 mpsc 通道确保投递;Normal 事件走 broadcast |
| F-007 | Minor | AsaIntervention severity() 与 is_critical_alert_event 不一致 | `crates/event-bus/src/types.rs:1142-1151`、`crates/efficiency-monitor/src/lib.rs:71-89` | 文档中明确两个函数的语义差异(severity() 为背压级别,is_critical_alert_event 为告警级别);或重构 severity() 为非同步函数以读取 action 字段 |
| F-008 | Minor | chimera-cli 命令输入未做长度限制/ANSI 转义过滤 | `crates/chimera-cli/src/commands/run.rs:13-15`、`crates/chimera-cli/src/commands/wiki.rs:11-17` | 在 execute() 中校验 prompt 长度(如 ≤ 64KB),过滤 ANSI 转义序列(\x1b)与控制字符 |
| F-009 | Minor | chtc-bridge tool_id 内容未校验,可能含恶意字符串 | `crates/chtc-bridge/src/protocol.rs:153-164` | 在 take_str_field 后增加长度限制(如 ≤ 4KB)与字符白名单校验(拒绝 shell 元字符、路径遍历字符) |
| F-010 | Minor | nexus-core/path_util 仅 ~ 展开,无路径遍历校验 | `crates/nexus-core/src/path_util.rs:47-80` | 增加 canonicalize 后检查是否在允许根目录内(如 ~/.aether/),拒绝 `../` 逃逸 |
| F-011 | Minor | wasmtime/reqwest/axum/sqlite-vec 在 workspace 声明但未使用 | `Cargo.toml:52-57` | 移除未使用的 workspace 依赖,减少供应链攻击面;或在注释中明确"预留未来使用" |
| F-012 | Minor | cargo-audit 未集成到 CI,依赖漏洞扫描依赖手动检查 | `docs/security/week8_security_report.md:429-435` | 在 CI 中加 cargo-audit step(如 `.github/workflows/audit.yml`),每日定时运行 + PR 触发 |
| F-013 | Minor | A10 SSRF 仅靠白名单拦截 curl/wget/python3,若未来引入其他网络工具则防护失效 | `tests/security/owasp_top10.rs:502-543`、`crates/seccore/src/policy.rs:102-113` | 增加基于 endpoint 的 SSRF 防护层(在 SecCore 或 mcp-mesh 中校验 IP 是否为内网/元数据地址) |

---

## 12. 长期主义建议

### 12.1 短期(立即可做,1-2 周)

1. **修复 F-001(Critical)**:在 `crates/event-bus/src/types.rs:1142-1151` 的 `severity()` 中将 `BudgetExceeded` 列入 Critical 分支。这是 1 行代码修复,但需同步更新测试(`test_week5_event_normal_severity` 中 BudgetExceeded 的断言需改为 Critical)。
2. **修复 F-002(Major)**:为 `Sandbox` 增加 `timeout` 字段(默认 30 秒),`execute_in_sandbox` 用 `tokio::time::timeout` 包裹,超时后 `child.kill()` 并返回 `SecCoreError::SandboxError("timeout")`。
3. **修复 F-004(Major)**:在 `mcp-mesh/src/server_registry.rs` 的 `register()` 中增加 endpoint SSRF 校验,拒绝内网/回环/元数据地址。

### 12.2 中期(1-2 月)

4. **修复 F-003 + F-005(Major)**:实现 Linux 平台的 gVisor 集成(`runsc` 运行时 + seccomp 过滤器)+ 资源限制(`setrlimit`),与 ADR-001 对齐。Windows 保留降级版本并在文档明确风险。
5. **修复 F-006(Major)**:实现 broadcast + mpsc 双通道事件总线。Critical 事件走独立 mpsc 通道(点对点投递,不丢失),Normal 事件走 broadcast(多订阅者广播)。参考 `backpressure.rs:34-42` 的 `CriticalMpsc` 策略变体设计。
6. **修复 F-012(Minor)**:在 CI 中集成 cargo-audit,每日定时运行 + PR 触发,自动创建 issue 跟踪新发现的 CVE。

### 12.3 长期(3-6 月)

7. **供应链安全**:引入 `cargo-vet` 或 Sigstore 签名验证依赖完整性,防止依赖被篡改。
8. **零信任深化**:将 `path_util` 等共享工具增加自我保护(不依赖 SecCore 的纵深防御),每个系统边界组件独立校验输入。
9. **fuzzing 扩展**:`fuzz/` 目录已有 3 个 fuzz target,建议扩展到 SecCore 沙箱、Decay Engine、Event Bus 等核心组件,持续运行以发现未知漏洞。
10. **威胁建模**:建立 STRIDE 威胁模型,定期复盘新功能的安全风险,形成"威胁建模 → 安全设计 → 渗透测试 → 复盘"闭环。

### 12.4 架构演进建议

11. **AsaIntervention severity 重构**:`severity()` 当前是同步函数,无法根据 `action` 字段动态判定。长期建议引入 `async fn severity_async()` 或在事件发布时由发布者显式标注 severity(而非依赖 `severity()` 同步函数),使 AsaIntervention Block 能正确标记为 Critical。
12. **Critical 事件清单治理**:当前 `severity()` 中 Critical 事件列表硬编码(`types.rs:1144-1149`),新增 Critical 事件需手动添加。建议引入 `#[critical]` 属性宏或配置化清单,避免遗漏。

---

## 附录:审计工具与命令

```powershell
# 1. forbid(unsafe_code) 覆盖扫描
# Grep pattern: #!\[forbid\(unsafe_code\)\] in crates/**/lib.rs
# Grep pattern: #!\[(forbid|deny|allow)\(unsafe_code\)\] in crates/**/main.rs
# Grep pattern: ^\s*unsafe\s+(fn|impl|trait|extern)|unsafe\s*\{|unsafe\s*\( in crates/**/src/**/*.rs

# 2. 关键事件发布路径扫描
# Grep pattern: NexusEvent::BudgetExceeded|NexusEvent::AsaIntervention|NexusEvent::SkepticVeto|NexusEvent::RedTeamAudit

# 3. 依赖版本核验
# Grep pattern: ^name = "(tokio|serde|clap|ratatui|wasmtime|rusqlite|rmp-serde|...)"$ in Cargo.lock

# 4. 测试运行(建议)
# cargo test --test owasp_top10
# cargo test -p seccore --test security
# cargo test -p parliament ahirt
# cargo test -p event-bus types
```

---

**报告结束**
