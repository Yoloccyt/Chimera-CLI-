# Checklist

## 阶段一:分布式深度审计

### 审计覆盖完整性
- [ ] 审计覆盖全部 34 个 crates 的 src/**/*.rs
- [ ] 审计覆盖全部 34 个 crates 的 tests/**/*.rs
- [ ] 审计覆盖全部 34 个 crates 的 Cargo.toml 依赖方向
- [ ] 审计覆盖 workspace 根 Cargo.toml 配置一致性
- [ ] 审计覆盖 .github/workflows/ CI 配置(release.yml + fuzz.yml)
- [ ] 审计覆盖 Dockerfile 发布产物一致性
- [ ] 每个 crate 至少有 1 条审计记录(即使无问题)

### 维度 A:架构一致性
- [ ] 十层架构映射与实际 crate 布局一致
- [ ] 依赖方向 100% 合规(无 L(N)→L(N+1) 违规)
- [ ] nexus-core 保持最小依赖(无上层 import)
- [ ] 跨层通信仅走 EventBus(无直接跨层 import)
- [ ] OMEGA 四定律在代码中有体现
- [ ] ADR-001~025 决策已执行
- [ ] 命名模式规范一致

### 维度 B:并发安全
- [ ] 无"持锁跨 await"违规
- [ ] 所有 tokio::spawn 任务有显式 await 或 join 管理
- [ ] DashMap 写锁释放后再调用 async 方法
- [ ] broadcast subscribe 在 spawn 前同步调用
- [ ] mpsc channel recv() 与 Sender drop 正确配对
- [ ] async fn 满足 Send + 'static + 'async
- [ ] 无竞态条件(TOCTOU)

### 维度 C:性能瓶颈
- [ ] Top-K 选择使用 select_nth_unstable
- [ ] FuturesUnordered 用于并发操作收集
- [ ] spawn_blocking 用于阻塞操作
- [ ] 热路径无不必要 clone
- [ ] benchmark 覆盖关键 crate
- [ ] 性能基线数据完整(WAL/三层路由/SSRA/KVBSR)

### 维度 D:代码质量
- [ ] 无 TODO/FIXME/HACK(或已登记技术债)
- [ ] Week 1-4 伪实现(MTPE/FaaE/RepoWiki)已替换或登记
- [ ] 生产代码无 unwrap/expect/panic
- [ ] 硬编码常量已评估配置化必要性
- [ ] 单函数 ≤200 行
- [ ] 错误处理一致(库层 thiserror、应用层 anyhow)
- [ ] WHY 注释覆盖隐藏约束

### 维度 E:测试覆盖
- [ ] qeep-protocol 测试 ≥20 个(Week 1-4 Major-2 核验)
- [ ] decay-engine 测试 ≥15 个
- [ ] 边界条件覆盖(超时、空输入、最大值、并发峰值)
- [ ] 跨周集成测试覆盖
- [ ] proptest 覆盖核心 crate
- [ ] stress_test 标记 #[ignore] 合理

### 维度 F:安全
- [ ] #![forbid(unsafe_code)] 覆盖全部 34 crates(100%)
- [ ] SecCore 沙箱执行所有外部命令
- [ ] Decay 能力衰减模型正确
- [ ] 输入校验覆盖系统边界
- [ ] OWASP Top 10 测试无回归
- [ ] 四大 Critical 事件经 mpsc 通道发布
- [ ] BudgetExceeded 事件 severity() 为 Critical
- [ ] AHIRT 5 分钟周期与 0.95 检测率可配置(Week 5 遗留 P2)

### 维度 G:文档同步
- [ ] CODE_WIKI.md 与实际 crate 布局一致
- [ ] CHANGELOG.md Week 1-8 章节完整
- [ ] 各 crate lib.rs 文档注释与实现一致
- [ ] project_memory.md 教训与代码状态一致
- [ ] CI 配置与实际构建产物一致
- [ ] Dockerfile 与 binary 命名(aether → chimera)一致
- [ ] docs/ 下文档与代码状态一致

### 历史问题追踪
- [x] Week 1-4 cross-review 12 项核验完成(逐项标注状态) — 11 已修复 + 1(MTPE 伪预测)按计划保持,修复率 91.7%
- [x] Week 3 third-round deep-review 遗留项核验完成 — 22 项 SubTask 全部 [x] 通过,5 项关键遗留项(HCW 权重/get_arc/MLC 迁移锁/KVBSR select_nth/FaaE Top-K)全部已修复
- [x] Week 4 deep-review 遗留项核验完成 — Task 30-36 全部通过,PVL/GQEP/QEEP/SCC/EDSB 全部已修复,MTPE 按 Week 7 计划保持(已超期)
- [x] Week 5 deep-review 遗留项核验完成 — C1(8 事件 EventBus)✅/C2(BudgetExceeded severity)❌ 标记不实/M4(ttg.rs 7 expect)✅/M6(BudgetAdjusted 注释)⚠️ 部分修复/P2(AHIRT 配置化)✅
- [x] Week 8 limitations deep-remediation 3 项限制核验完成 — 限制1(cargo-fuzz CI)🔄 委托验证/限制5(clippy 根因 OOM)✅/限制6(上游 issue 草稿)✅
- [x] Week 9 spec(若存在)重叠项核验完成 — Week 9 spec 重叠项已核验并标注
- [x] project_memory.md "✅ FIXED" 标记与代码实际一致 — 6 个 ✅ FIXED 标记核验:4 项一致、1 项不实(C2 BudgetExceeded)、1 项部分一致(M6 BudgetAdjusted)、1 项 P2 过时(实际已修复但标记未更新)

### 审计报告产出
- [x] docs/audit/dimension_a_architecture.md 已生成 — 429 行,Critical 0/Major 0/Minor 5
- [x] docs/audit/dimension_b_concurrency.md 已生成 — 664 行,Critical 4/Major 1/Minor 3(识别 faae-router 4 处持锁跨 await)
- [x] docs/audit/dimension_c_performance.md 已生成 — 518 行,Critical 2/Major 5/Minor 6(识别 spawn_blocking 缺失 2 处)
- [x] docs/audit/dimension_d_quality.md 已生成 — 470 行,Critical 0/Major 3/Minor 6(识别 3 处伪实现)
- [x] docs/audit/dimension_e_testing.md 已生成 — 505 行,Critical 0/Major 1/Minor 6(decay-engine 测试不足)
- [x] docs/audit/dimension_f_security.md 已生成 — 533 行完整报告,含 12 章节(执行摘要+9 项审计+问题清单+长期主义建议),识别 1 Critical+5 Major+7 Minor,P2 已解决
- [x] docs/audit/dimension_g_documentation.md 已生成 — 515 行,Critical 4/Major 4/Minor 11(识别 AETHER §6.2 层级漂移)
- [x] docs/audit/historical_issues_tracking.md 已生成 — 657 行,24 项核验,修复率 70.8%
- [x] docs/audit/week1-8_global_audit_report.md 已生成(汇总) — 377 行,74 问题(11 Critical+19 Major+44 Minor),含 7 章节
- [x] 问题清单按优先级排序(Critical → Major → Minor,同级按 L1→L10) — §4 已按优先级排序,同级按 L1→L10,文档/跨 crate 问题列于末尾

## 阶段二:按优先级修复

### Critical 级修复
- [ ] 所有 Critical 问题 100% 修复
- [ ] 每个 Critical 修复有复现测试(RED → GREEN)
- [ ] Critical 修复未引入新问题
- [ ] Critical 修复记录到 remediation_log.md

### Major 级修复
- [ ] Major 问题 ≥90% 修复
- [ ] 历史 Major(MTPE/FaaE/RepoWiki 伪实现、qeep 测试)已处理
- [ ] 每个 Major 修复遵循项目编码规范
- [ ] Major 修复记录到 remediation_log.md

### Minor 级修复
- [ ] Minor 问题 ≥70% 修复
- [ ] 高价值 Minor(硬编码、文档不一致、测试补充)优先
- [ ] Week 1-4 遗留 Minor 处理完成
- [ ] Minor 修复记录到 remediation_log.md

### 修复质量
- [ ] 单函数 ≤200 行(项目铁律)
- [ ] #![forbid(unsafe_code)] 未被移除
- [ ] 无新增 unwrap/expect/panic
- [ ] WHY 注释已添加(隐藏约束)
- [ ] workspace 级依赖声明
- [ ] async fn 满足 Send + 'static

## 阶段三:系统性验证

### 全量回归
- [ ] cargo check --workspace 退出码 0
- [ ] cargo clippy --workspace --all-targets --jobs 2 -- -D warnings 退出码 0(Windows: CARGO_INCREMENTAL=0)
- [ ] cargo test --workspace 全部通过
- [ ] cargo doc --workspace --no-deps 无警告
- [ ] cargo fmt --all -- --check 通过(或记录需 fmt 的文件)

### 性能基线
- [ ] scc-cache WAL 恢复 benchmark 1000 次零丢失
- [ ] sesa-router 三层路由 benchmark p95 ≤ 100µs
- [ ] ssra-fusion benchmark ≤ 20ms
- [ ] kvbsr-router scale benchmark 10× 加速
- [ ] 修复前后性能对比无退化(±5% 内)

### 文档同步
- [ ] CODE_WIKI.md 与实际 crate 布局一致(若有变更已更新)
- [ ] CHANGELOG.md 追加"Week 1-8 全局深度审计与修复"章节
- [ ] project_memory.md 追加新经验教训
- [ ] 各 crate lib.rs 文档注释与实现一致
- [ ] spec checklist.md 逐项打勾

### 验收报告
- [ ] docs/audit/week1-8_remediation_log.md 已生成(修复日志)
- [ ] docs/audit/week1-8_acceptance_report.md 已生成(验收报告)
- [ ] 验收报告含全量回归验证证据
- [ ] 验收报告含性能基线对比
- [ ] 验收报告含文档同步确认
- [ ] 验收报告含问题修复统计(Critical 100% / Major ≥90% / Minor ≥70%)
- [ ] 验收决议明确(通过/有条件通过/不通过)

## 全局质量基准
- [ ] 审计报告覆盖全部 34 个 crates
- [ ] 历史问题追踪表 100% 核验
- [ ] Critical 问题 100% 修复
- [ ] Major 问题 ≥90% 修复
- [ ] Minor 问题 ≥70% 修复
- [ ] cargo check + clippy + test 全部通过
- [ ] 关键 benchmark p95 延迟不退化(±5% 内)
- [ ] 三份审计文档(报告/日志/验收)完整且证据充分
- [ ] CODE_WIKI / CHANGELOG / project_memory / lib.rs 文档同步
