# ADR 索引 — NEXUS-OMEGA

> 架构决策记录(ADR)索引,记录 Chimera CLI (NEXUS-OMEGA) 项目的重要架构决策。
> 完整 ADR 内容参见 `AETHER_NEXUS_OMEGA_ULTIMATE.md` §10.3。

---

## ADR 总览

| ADR | 主题 | 决策 | 影响范围 |
|-----|------|------|---------|
| ADR-001 | 沙箱运行时选择 | gVisor(Linux)+ 进程隔离(Windows 降级) | seccore |
| ADR-002 | 能力衰减模型 | 连续权限流体(非离散档位) | decay-engine |
| ADR-003 | Event Bus 实现选型 | Tokio broadcast(非 mpsc) | event-bus |
| ADR-004 | 消息序列化协议 | MessagePack(非 JSON) | 全局(rmp-serde) |
| ADR-005 | 持久化存储选型 | SQLite + 向量(repo-wiki) | repo-wiki / cmt-tiering |
| ADR-006 | 异步运行时 | Tokio(非 async-std) | 全局 |
| ADR-007 | 配置加载框架 | Figment 多源合并 | chimera-cli |
| ADR-008 | CLI 框架 | Clap derive | chimera-cli |
| ADR-009 | TUI 框架 | ratatui + crossterm | chimera-tui |
| ADR-010 | 日志框架 | tracing + tracing-subscriber | 全局 |
| ADR-011 | 错误处理策略 | 库层 thiserror + 应用层 anyhow | 全局 |
| ADR-012 | 内存安全 | `#![forbid(unsafe_code)]` 全覆盖 | 全局(34/34 crate) |
| ADR-013 | 并发原语 | DashMap(非 RwLock<HashMap>) | 全局 |
| ADR-014 | 向量检索 | 纯 Rust KNN(非 Faiss) | repo-wiki / mlc-engine |
| ADR-015 | 模型路由策略 | CACR 成本感知(Allow/Downgrade/Block) | model-router |
| ADR-016 | 议会权重 | Architect=0.25/Skeptic=0.30/Optimizer=0.20/Librarian=0.15/Bard=0.10 | parliament |
| ADR-017 | Skeptic 否决权 | 25 条规则,5 类攻击,辩论前否决 | parliament |
| ADR-018 | AHIRT 红队 | 100 载荷,4 类探测,探测率 < 95% 告警 | parliament |
| ADR-019 | TTG 思考模式 | 三级(Fast/Standard/Deep),预算联动 | quest-engine |
| ADR-020 | CHTC 跨平台桥 | enum dispatch 静态分发(非 Box<dyn>) | chtc-bridge |
| ADR-021 | MCP Mesh 量子事务 | 2PC 占位实现,状态机 5 状态 | mcp-mesh |
| ADR-022 | SSRA 黏液式适配 | 预编译模板 + 运行时融合 | ssra-fusion |
| ADR-023 | CSN 能力替代 | 余弦相似度 Top-K,多级降级链(≥ 3 级) | csn-substitutor |
| ADR-024 | SESA 稀疏激活 | 256-bit 位向量掩码,稀疏度 < 40% | sesa-router |
| ADR-025 | GSOE 在线进化 | GRPO 风格(DeepSeek V4 启发) | gsoe-evolution |
| ADR-SIMD-001 | SIMD 优化决策 | 保持 `#![forbid(unsafe_code)]`,不引入显式 SIMD | sesa-router / nexus-core |

---

## 关键 ADR 详解

### ADR-001:沙箱运行时选择

- **决策**:Linux 生产环境用 gVisor + seccomp;Windows 开发环境降级为进程隔离
- **理由**:gVisor 提供最强隔离,但仅 Linux 可用;Windows 降级依赖策略层静态分析
- **影响**:`seccore` crate 的 `Sandbox` 实现
- **当前状态**:Windows 降级模式(进程隔离 + 白名单 + 静态分析)

### ADR-002:能力衰减模型

- **决策**:连续权限流体(非离散档位)
- **理由**:离散档位(如 0/1/2)无法表达渐进式衰减;连续流体更符合实际安全需求
- **影响**:`decay-engine` crate 的 `CapabilityDecay` 实现

### ADR-003:Event Bus 实现选型

- **决策**:Tokio broadcast(非 mpsc)
- **理由**:broadcast 支持多订阅者,适合跨层广播;mpsc 仅 1:1 通道
- **影响**:`event-bus` crate 的 `EventBus` 实现
- **约束**:Critical 级事件无订阅者时记录 `warn`

### ADR-004:消息序列化协议

- **决策**:MessagePack(非 JSON)
- **理由**:MessagePack 二进制格式,体积更小,序列化更快;JSON 体积大但可读
- **影响**:全局 `rmp-serde` 依赖;LHQP 检查点持久化用 MessagePack

### ADR-005:持久化存储选型

- **决策**:SQLite + 向量扩展
- **理由**:SQLite 嵌入式无需独立数据库服务;WAL 模式支持高并发;向量扩展支持语义检索
- **影响**:`repo-wiki`、`cmt-tiering`、`mlc-engine` L3 程序记忆

### ADR-012:内存安全

- **决策**:`#![forbid(unsafe_code)]` 34/34 crate 全覆盖
- **理由**:从 Claude Code 尸检教训,unsafe 是内存安全漏洞根源;forbid 是编译期保证,比 deny 更强
- **影响**:全局;fuzz crate 独立于主 workspace,不影响覆盖率
- **验证**:34 个 lib.rs + 1 个 main.rs + 1 个 owasp_top10.rs = 36 个核心文件全覆盖

### ADR-SIMD-001:SIMD 优化决策(Week 8 新增)

- **决策**:保持 `#![forbid(unsafe_code)]`,不引入显式 SIMD
- **理由**:
  1. `std::simd` 是 nightly-only,项目用 stable Rust,不可用
  2. 第三方 SIMD 库(wide/pulp)会引入新依赖,且可能破坏 `#![forbid(unsafe_code)]` 34/34 覆盖
  3. 三层路由 p95 = 78.79µs,远低于 2ms 目标(25× 余量),无优化必要
  4. `cosine_similarity_slices` 循环结构可向量化,编译器自动向量化已足够
  5. `popcount` 已用 `u8::count_ones` 内建方法,编译器自动展开为 POPCNT 指令
- **影响**:`sesa-router`、`nexus-core` 的核心计算路径
- **未来优化路径**(若性能成为瓶颈):
  - 评估 `RUSTFLAGS="-C target-cpu=native"` 启用更激进的自动向量化
  - 评估 `wide` crate 的 Safe API 子集(若全部 Safe)
  - 在 ADR 中记录特批后再引入

---

## ADR 维护规则

1. **新增 ADR**:每个重要架构决策必须记录 ADR,编号递增
2. **ADR 不可删除**:已废弃的 ADR 标记为"已废弃",但保留历史记录
3. **ADR 必须包含**:背景、决策、理由、影响、替代方案
4. **ADR 与代码同步**:代码变更涉及架构决策时,必须同步更新 ADR

---

> **维护者**:NEXUS-OMEGA 团队
> **文档版本**:Week 8 同步(2026-06-27)
> **ADR 总数**:26 个(ADR-001 到 ADR-025 + ADR-SIMD-001)
