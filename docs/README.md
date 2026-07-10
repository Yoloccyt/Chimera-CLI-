# 文档中心 — NEXUS-OMEGA

> 这里是 Chimera CLI (NEXUS-OMEGA) 的文档入口。建议先看本页，再按主题进入对应目录。

---

## 快速导航

| 主题 | 入口 | 说明 |
|------|------|------|
| 文档总索引 | [INDEX.md](./INDEX.md) | 按主题汇总全部文档 |
| 架构 | [architecture/README.md](./architecture/README.md) | 10 层架构、数据流、ADR 索引 |
| 发布 | [release/release_guide.md](./release/release_guide.md) | 本地构建、CI 发布、Docker、交叉编译 |
| 安全 | [security/week8_security_report.md](./security/week8_security_report.md) | 安全基线、测试与审计报告 |
| 性能 | [performance/week8_perf_report.md](./performance/week8_perf_report.md) | 性能基准与优化记录 |
| 监控 | [grafana/README.md](./grafana/README.md) | Grafana 仪表盘与 Prometheus 接入 |
| 审计 | [audit/](./audit/) | 文档审计、修复记录、基线追踪 |
| 验收 | [acceptance/](./acceptance/) | 阶段验收与限制整改报告 |
| 开发记录 | [dev/](./dev/) | 开发过程中的根因分析与修复说明 |
| 优化研究 | [optimization/](./optimization/) | 版本优化、验证与调研材料 |

---

## 文档分层建议

### 正式文档
适合对外引用、长期维护：
- `architecture/`
- `release/`
- `security/`
- `performance/`
- `grafana/`

### 历史与过程文档
适合追溯、审计和内部参考：
- `audit/`
- `acceptance/`
- `dev/`
- `optimization/`

---

## 维护规则

1. 新增文档时，优先放入对应主题目录。
2. 需要全局入口时，先更新本页和 `INDEX.md`。
3. 报告类文档建议使用清晰命名，例如 `*_report.md`、`*_guide.md`、`*_roadmap.md`。
4. 历史性材料不要混入正式入口的主导航首屏。

---

> **维护者**:NEXUS-OMEGA 团队
> **文档版本**:Week 8 文档重构草案
