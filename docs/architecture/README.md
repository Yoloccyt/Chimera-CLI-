# 架构文档索引 — NEXUS-OMEGA

> 本目录包含 Chimera CLI (NEXUS-OMEGA) 项目的架构设计文档,与 [CODE_WIKI.md](../../CODE_WIKI.md) 互为补充。
> 维护原则:与代码同步,与 AETHER_NEXUS_OMEGA_ULTIMATE.md 一致。

---

## 文档清单

| 文档 | 说明 | 受众 |
|------|------|------|
| [ten_layers.md](./ten_layers.md) | 10 层架构详解(L1-L10),含依赖方向规则与每层职责 | 架构师 / 新成员 |
| [data_flow.md](./data_flow.md) | 端到端数据流图,从用户输入到 Wiki 沉淀 | 架构师 / 开发者 |
| [adr_index.md](./adr_index.md) | ADR(架构决策记录)索引,ADR-001 到 ADR-025 + ADR-SIMD-001 | 架构师 / 维护者 |

---

## 快速导航

### 想了解整体架构?
→ 阅读 [ten_layers.md](./ten_layers.md),从 L1 Core 到 L10 Interface 逐层了解。

### 想了解请求处理流程?
→ 阅读 [data_flow.md](./data_flow.md),查看用户输入到 Wiki 沉淀的完整链路。

### 想了解为什么这样设计?
→ 阅读 [adr_index.md](./adr_index.md),查看 26 个架构决策记录的背景与理由。

---

## 架构红线

1. **依赖方向**:L(N) → L(N-1) 允许;L(N) → L(N+1) 禁止
2. **跨层通信**:只能走 Event Bus(`event-bus` crate)
3. **跨进程通信**:只能走 MCP Mesh(`mcp-mesh` crate)
4. **内存安全**:`#![forbid(unsafe_code)]` 34/34 crate 全覆盖
5. **单函数 ≤ 200 行**:超过必须拆模块

---

> **维护者**:NEXUS-OMEGA 团队
> **文档版本**:Week 8 同步(2026-06-27)
