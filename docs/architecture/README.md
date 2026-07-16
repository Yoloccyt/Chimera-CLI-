# 架构文档索引 — NEXUS-OMEGA

> 本目录包含 Chimera CLI (NEXUS-OMEGA) 项目的架构设计文档。
> 完整文档分类索引请参阅 [INDEX.md](./INDEX.md)（7 分类：权威源 / 工程实施升级 v3 / 工程实施 v2 / 创新演进 / 模块级优化 / 深研报告 / 原有文档）。

---

## 📚 文档权威层级

| 优先级 | 文档 | 说明 | 状态 |
|--------|------|------|------|
| ⭐⭐⭐ **权威源** | [CODE_WIKI.md](./CODE_WIKI.md) | **唯一权威参考** — 34 crate完整索引、核心类型、事件系统、依赖铁律、数据流、设计模式、工程红线、构建指南 | ✅ v1.7.0-omega 已更新 |
| ⭐⭐⭐ **文档索引** | [INDEX.md](./INDEX.md) | 架构文档统一索引（7 分类组织所有架构文档） | ✅ 已更新 |
| ⭐⭐ 辅助参考 | [nuxus规则.md](../../.trae/rules/nuxus规则.md) | 全局规则、架构硬约束、async/SQLite/安全红线 | ✅ 已同步 |
| ⭐⭐ 辅助参考 | [CLAUDE.md](../../.claude/CLAUDE.md) | 环境设置、常用命令、CI/CD与发布 | ✅ 已同步 |

---

## 🚀 新成员快速入门

1. **首先阅读** → [CODE_WIKI.md](./CODE_WIKI.md) (本目录)
   - §1 项目概览 + §2 十层架构 → 建立整体认知
   - §3 34 Crate索引 → 了解模块职责
   - §9 工程红线 → 避免踩坑

2. **然后查看** → [CLAUDE.md](../../.claude/CLAUDE.md)
   - 环境设置
   - 常用cargo命令

3. **开始编码前** → 运行 `cargo check --workspace` 确认环境正常

---

## CODE_WIKI.md 章节导航

| 章节 | 内容 |
|------|------|
| §1 | 项目概览、OMEGA四定律、术语速查 |
| §2 | 十层架构详解与分层映射 |
| §3 | **34个crate完整索引**(架构层/职责/关键类型/关键文件/依赖) |
| §4 | 核心领域类型(UserIntent/Quest/Task/Checkpoint/CLV/ThinkingMode) |
| §5 | 事件系统(双通道架构/Critical事件清单/核心事件表) |
| §6 | 依赖关系铁律(方向规则/硬约束/已修正违规) |
| §7 | 端到端数据流图 |
| §8 | 关键设计模式(枚举分发/Arc共享/spawn_blocking等) |
| §9 | **工程红线与实战教训**(尸检红线/Week 1-8红线/async反模式) |
| §10 | **构建、测试与运行**(环境/命令/Docker/发布清单) |
| §11 | 架构决策记录(ADR-001~005) |
| §12 | 目录结构索引 |

---

## 架构红线(摘要)

1. **依赖方向**: L(N) → L(N-1) 允许; L(N) → L(N+1) 禁止
2. **跨层通信**: 只能走 Event Bus (`event-bus` crate)
3. **跨进程通信**: 只能走 MCP Mesh (`mcp-mesh` crate)
4. **内存安全**: `#![forbid(unsafe_code)]` 34/34 crate 全覆盖
5. **单函数 ≤ 200 行**: 超过必须拆模块
6. **禁止持锁.await**: DashMap/Mutex写锁必须在.await前释放
7. **rusqlite必须spawn_blocking**: 禁止在async上下文直接调用
8. **broadcast先subscribe再spawn**: 否则事件静默丢失

> 完整红线与实战教训见 [CODE_WIKI.md §9](./CODE_WIKI.md#9-工程红线与实战教训)

---

> **文档版本**: v1.7.0-omega (2026-07-15)
> **权威源**: [CODE_WIKI.md](./CODE_WIKI.md)
