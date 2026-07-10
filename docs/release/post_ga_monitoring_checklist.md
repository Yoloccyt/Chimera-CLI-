# Post-GA Monitoring Checklist — v1.0.0-omega

> 本清单用于 v1.0.0-omega GA 发布后的持续监控与跟进。每个监控项需指派负责人,定期核验。

## 1. GA 后 24 小时监控项(T+0 到 T+24h)

### 1.1 GitHub Release 健康度
- [ ] Release 页面访问正常,5 个 binary + checksums.txt 附件完整可下载
- [ ] Release 下载量统计(GitHub web UI: Insights → Traffic → Releases)
- [ ] 无用户在 issue tracker 报告 "下载失败" / "SHA256 校验失败" / "binary 无法执行"
- [ ] Release 页面无 negative feedback 评论

### 1.2 Docker GHCR 镜像健康度
- [ ] `docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.0-omega` 成功
- [ ] `docker run --rm <image> --version` 输出正确
- [ ] GHCR 镜像拉取量统计(GitHub web UI: Packages → chimera-cli- → Insights)
- [ ] 无用户报告 "镜像拉取失败" / "镜像运行异常"

### 1.3 CI/CD 持续健康度
- [ ] audit.yml 次日 UTC 02:00 自动运行成功(cargo audit --deny warnings 退出码 0)
- [ ] 无新的 RustSec advisory 影响 Cargo.lock 中 13 个关键依赖
- [ ] master 分支 push 触发的 CI 运行正常(若有新 commit)

### 1.4 安装脚本可用性
- [ ] `curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh` 在 Linux x86_64 成功
- [ ] `iwr -UseBasicParsing https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1 | iex` 在 Windows x86_64 成功(需 GITHUB_TOKEN)
- [ ] 安装脚本 SHA256 校验逻辑生效,不空转

### 1.5 用户反馈监控
- [ ] GitHub issue tracker:无新增 Critical/Major severity issue
- [ ] GitHub Discussions:监控用户提问与反馈
- [ ] 社交媒体(Twitter/Reddit/HackerNews):搜索 "Chimera CLI" 或 "NEXUS-OMEGA" 关键词,监控社区反响

## 2. GA 后 7 天跟进项(T+1d 到 T+7d)

### 2.1 v1.1.0 路线图发布
- [ ] 在 docs/release/ 新增 `v1.1.0_roadmap.md`,列出 6 项延后到 v1.1 的功能:
  - chimera-cli L10 真实编排接线
  - nexus-core rusqlite 下沉到 L3
  - seccore 内核级沙箱(ADR-001 完整实现)
  - GSOE 真实强化学习策略
  - NMC 多模态感知器 ONNX 接入
  - MCP Mesh 2PC 真实跨进程通信
- [ ] 在 README.md 添加 v1.1.0 路线图链接
- [ ] 在 GitHub Project board 创建 v1.1.0 milestone

### 2.2 用户反馈收集与分类
- [ ] 汇总 7 天内 GitHub issues,按类型分类(bug / feature request / question / docs)
- [ ] 汇总社区反馈(Twitter/Reddit/Discord),提取共性需求
- [ ] 形成《v1.0.0-omega 用户反馈分析报告》,作为 v1.1.0 优先级排序输入

### 2.3 已知限制文档化
- [ ] 确认 README.md "已知限制"章节覆盖 6 类占位实现:
  - MTPE pseudo_predictions
  - SecCore ASA rule-based
  - GSOE policy rule-based
  - NMC multimodal perceptors
  - RepoWiki placeholder_embedding
  - SCC InMemoryWal
- [ ] 每项占位实现注明精确代码位置与 v1.1+ 替换计划
- [ ] 在 docs/ 新增 `known_limitations.md` 详细文档,链接到 README

### 2.4 性能与稳定性监控
- [ ] 收集 7 天内用户报告的性能问题(binary 启动慢 / 内存占用高 / 崩溃)
- [ ] 若有崩溃报告,分析 backtrace(RUST_BACKTRACE=1 输出)
- [ ] 必要时发布 v1.0.4-omega 热修版本

### 2.5 安全监控
- [ ] audit.yml 每日运行结果汇总(7 天内是否有新 advisory)
- [ ] 关注 RustSec Advisory Database 中 rusqlite / tokio / serde 等 13 个关键依赖的新漏洞
- [ ] 若有 Critical 漏洞,立即评估影响范围并决定是否热修

## 3. GA 后 30 天里程碑(T+30d)

- [ ] v1.1.0 路线图定稿,与社区沟通开发计划
- [ ] v1.0.0-omega 累计下载量统计(GitHub Release + GHCR Docker pull)
- [ ] 形成《v1.0.0-omega 发布后 30 天回顾报告》:
  - 用户采用情况(下载量 / Docker pull 量 / star 增长)
  - 主要问题与解决方案
  - 经验教训沉淀到 project_memory.md
  - v1.1.0 优先级调整建议

## 4. 监控负责人与频率

| 监控项 | 负责人 | 频率 | 工具 |
|--------|--------|------|------|
| GitHub Release 健康度 | Release Manager | 每日 | GitHub Web UI |
| Docker GHCR 健康度 | DevOps | 每日 | GHCR Web UI + docker pull |
| CI/CD 健康 | SRE | 每日 | GitHub Actions |
| 安装脚本 | DevOps | 每周 | curl / iwr 手动执行 |
| 用户反馈 | Community Manager | 每日 | GitHub Issues + 社交媒体 |
| 安全监控 | Security | 每日 | audit.yml + RustSec |

## 5. 异常处理流程

- Critical 问题(数据损坏/安全漏洞):立即启动回滚流程,参见 `rollback_runbook.md`
- Major 问题(功能不可用):24h 内评估,决定回滚或热修 v1.0.4-omega
- Minor 问题(文档错误/体验优化):记录到 v1.1.0 backlog

## 6. 文档维护

- 创建日期:2026-07-04
- 维护者:Chimera CLI Team
- 更新频率:GA 后 30 天内每周更新,之后每月更新
- 相关文档:`rollback_runbook.md` / `v1.0.0-omega_release_notes.md` / `CHANGELOG.md`
