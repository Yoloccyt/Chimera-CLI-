# v1.6.0-omega 分布式精英团队执行设计

## 1. 背景与目标

### 1.1 项目上下文

- **项目**：NEXUS-OMEGA / Chimera CLI
- **根目录**：`D:\Chimera CLI`
- **目标 spec**：`.trae/specs/v1-6-0-omega-comprehensive-deep-optimization/`
- **当前基线版本**：`1.5.0-omega`
- **目标版本**：`1.6.0-omega`

### 1.2 当前进度

截至 2026-07-11，已确认：

- `cargo check --workspace` 退出码 0（全部 35 crate 编译通过）
- `cargo check -p mlc-engine/nmc-encoder/repo-wiki/hcw-window/parliament` 均通过
- Phase I Task 1-5（5 个 crate 编译基线修复）已完成
- Phase II Task 7-8、Phase V Task 26 已提前实现

剩余工作：Task 6（全量基线验证）+ Task 9-33，共 28 个任务。

### 1.3 目标

以分布式精英子代理团队方式，按照优先级推进 v1.6.0 全部 33 个 Task，确保：

1. 所有 P0/P1 阻塞项与风险项完成
2. 创新深化（Phase IV）有 ADR 记录
3. 性能优化有 bench 数据支撑
4. 文档与经验沉淀同步更新
5. 最终通过完整验证套件并发布 `v1.6.0-omega`

---

## 2. 精英专家子代理团队

### 2.1 团队角色

| 角色 | 代号 | 核心职责 | 主要交付物 |
|---|---|---|---|
| **技术架构师** | `architect` | 依赖方向审查、ADR 设计、接口契约、跨 crate 影响分析 | 架构影响评估、ADR 草稿、接口变更清单、依赖铁律检查 |
| **核心开发专家** | `developer` | Phase II-V 核心代码实现、TDD 测试、bench | 实现代码、单元/集成测试、性能基准报告 |
| **测试与质量专家** | `qa` | 测试矩阵、回归测试、proptest、fuzz、质量门控 | 测试报告、回归清单、覆盖率分析 |
| **产品与发布专家** | `product` | CHANGELOG、CODE_WIKI、版本同步、发布清单 | 文档更新、版本号同步、发布就绪检查表 |
| **长期主义审计员** | `auditor` | 技术债务监督、YAGNI 复核、复杂度控制、反短视审查 | 技术债务报告、简化建议、clone/unwrap 审计 |

### 2.2 协作原则

- **任务优先级为核心**：所有工作按 P0 → P1 → P2 → P3 顺序推进
- **独立分析，集体决策**：Round 2 各领域专家独立输出，Round 3 由我汇总并识别冲突
- **代码质量不可妥协**：任何优化必须附带测试或 bench，禁止为进度牺牲可维护性
- **长期主义**：每个修改需回答"是否为未来埋下技术债"
- **验证驱动**：每个 checkpoint 必须通过 `cargo` 工具链或 Read/Grep 验证

---

## 3. 多轮结构化思考流程

### Round 1：资料收集（已完成）

- 读取 `spec.md`、`tasks.md`、`checklist.md`
- 验证当前编译基线：`cargo check --workspace`
- 确认已提前完成的任务：Task 7-8、Task 26

### Round 2：专家独立分析

每个专家子代理从各自视角审阅 v1.6.0 spec，输出结构化报告：

- **技术架构师**：依赖图、ADR 需求、接口变更风险、跨层依赖风险
- **核心开发专家**：每个 Task 的实现难点、估算工时、建议实现顺序
- **测试与质量专家**：测试缺口、回归风险、需要新增的 proptest/fuzz/bench
- **产品与发布专家**：文档更新清单、版本号同步点、CHANGELOG 结构
- **长期主义审计员**：每个 Task 是否引入不必要复杂度、是否有更简单方案

### Round 3：集体共识

我（主代理）汇总 Round 2 输出，识别：

- 冲突点（如不同专家对同一 Task 的优先级判断不一致）
- 遗漏项（如未考虑的回归风险）
- 需要用户决策的关键问题

本轮输出：《v1.6.0 执行共识文档》，包含：

- 最终任务优先级
- 每个 Wave 的输入/输出
- 关键设计决策（已确认：r2d2 连接池、激进投机 DAG、每次 wave 报告）

### Round 4：验证

在执行每个 Task 前后，通过以下方式验证理解正确：

- `cargo check/test/clippy/fmt/audit`
- `Read`/`Grep` 验证文件实际内容
- `proptest` / `criterion` bench 验证行为与性能
- 交叉检查 `CODE_WIKI.md`、`CHANGELOG.md`、`.trae/rules/nuxus规则.md`

---

## 4. 执行计划（Wave 1→7）

### Wave 1：全量基线验证

- **Task 6**：全量编译与测试基线验证
- **输入**：Task 1-5 已完成
- **输出**：基线测试数量、已知限制清单
- **验证**：
  - `cargo check --workspace` 退出码 0
  - `cargo test --workspace --jobs 1` 全部 passed / 0 failed
  - 记录测试数量（>= v1.5.0 基线 3400+）

### Wave 2：Phase II P0/P1 修复

- **Task 9**：SQLite 连接池（cmt-tiering + scc-cache）→ 使用 `r2d2` 连接池
- **Task 10**：优先级残差事件流 → Normal/Warning/Critical/Priority 四级队列
- **Task 11**：L6 路由链路顺序保证 → `RoutingPipeline` 代码级顺序
- **Task 12**：AuditChain 并发化 → `DashMap` 或 `RwLock`

**关键决策确认**：

- Task 9 使用 `r2d2` 连接池（用户已确认）
- Task 10 需要新增 ADR-011
- Task 11 不破坏现有事件总线架构

### Wave 3：Phase III YAGNI 重新评估

- **Task 13**：NexusState Arc 共享 → bench 决策
- **Task 14**：TaskProfile Hash trait → bench 决策
- **Task 15**：EDSB 次优选择策略 → 实现"非最热候选中相似度最高"
- **Task 16**：cosine_similarity 优化 → bench 决策
- **Task 17**：NMC Perceptor 并行化 → 评估决策
- **Task 18**：gsoe spawn_blocking → 评估决策

**原则**：每个 Task 必须有 bench 数据支撑的 go/no-go 决策。若 go，则实现；若 no-go，记录原因并延后。

### Wave 4：Phase IV 创新深化

- **Task 19**：Speculative DAG 执行 → **激进投机策略**（用户已确认）
- **Task 20**：CLV 分层压缩（L0=512 / L1=256 / L2=128）
- **Task 21**：GRPO 自适应任务评分
- **Task 22**：OS-Memory Wiki 元遗忘
- **Task 23**：CACR 非对称预算控制
- **Task 24**：主动安全不变量检查

**ADR 需求**：

- ADR-011：事件优先级设计
- ADR-012：Speculative DAG 设计
- ADR-013：CLV 分层压缩类型变更

### Wave 5：Phase V 性能微优化

- **Task 25**：热路径 clone 减少（mlc-engine / cmt-tiering / scc-cache）
- **Task 27**：双格式序列化完成（event-bus msgpack/json 自动选择）
- **Task 28**：Prometheus 指标扩展（event-bus + efficiency-monitor）
- **Task 29**：heuristic_scores() 真实实现

### Wave 6：Phase VI 文档与经验沉淀

- **Task 30**：更新 `CODE_WIKI.md`
- **Task 31**：更新 `CHANGELOG.md`（新增 v1.6.0-omega 章节）
- **Task 32**：`project_memory.md` 新增原则 23+

### Wave 7：Phase VII 全量验证与交付

- **Task 33**：完整验证套件 + 综合报告
- **验证清单**：
  - `cargo check --workspace` 退出码 0
  - `cargo test --workspace --jobs 1` 全部 passed / 0 failed
  - `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 零警告
  - `cargo fmt --all -- --check` 零 diff
  - `cargo audit --deny warnings --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436` 通过
  - `Cargo.toml [workspace.package].version` 同步为 `1.6.0-omega`
  - 镜像体积 < 100 MB，binary 体积 < 50 MB

---

## 5. 关键设计决策（已确认）

| 决策项 | 选项 | 理由 |
|---|---|---|
| WikiStore 读写分离 | `r2d2` 连接池 | 真实并发读，连接池管理生命周期，虽然引入新依赖但收益明确 |
| Speculative DAG 策略 | 激进投机 | 在可回滚前提下最大化并行度，适合 v1.6.0 创新深化目标 |
| 进度报告频率 | 每 Wave 结束 | 平衡实时性与汇总价值，避免信息过载 |

---

## 6. 质量门控

### 6.1 代码级门控

- 所有 crate 顶层保持 `#![forbid(unsafe_code)]`
- 禁止向上依赖（`L(N) → L(N+1)`）
- `rusqlite` 调用必须 `spawn_blocking`
- 禁止持锁跨 `.await`
- 风险规则列表为空时返回 `RiskLevel::Unknown`

### 6.2 测试级门控

- 每个 Task 必须附带测试或 bench
- P0/P1 Task 必须有集成测试或回归测试
- 创新深化 Task 必须有 proptest 验证边界条件
- 性能优化 Task 必须有 bench 数据支撑

### 6.3 文档级门控

- 新增功能更新 `CODE_WIKI.md`
- 架构变更写 ADR
- 每个 Phase 结束更新 `CHANGELOG.md`
- 经验沉淀写入 `project_memory.md`

### 6.4 长期主义门控

- 每个 PR/修改必须回答：
  - 是否为未来埋下技术债？
  - 是否有更简单的方案？
  - 这个抽象是否过早？
  - 是否引入未要求的特性？

---

## 7. 进度跟踪与风险预警

### 7.1 实时任务板

使用 `TaskCreate`/`TaskUpdate` 维护：

- 每个 Wave 作为一个父任务
- 每个 Task 作为子任务
- 状态：`pending` → `in_progress` → `completed`
- 阻塞关系通过 `addBlockedBy` 管理

### 7.2 Wave 结束报告模板

每个 Wave 结束输出：

```markdown
## Wave X 进度报告

### 已完成
- Task X: 简述 + 验证命令输出

### 进行中
- Task Y: 当前状态 + 预计完成时间

### 阻塞/风险
- 风险描述 + 应对措施 + 需要用户决策的问题

### 计划调整
- 与原始计划相比的偏差及原因

### 质量数据
- cargo check/test/clippy/fmt/audit 结果
- bench 数据摘要
- 新增/修改代码行数
```

### 7.3 风险分级

| 级别 | 定义 | 响应 |
|---|---|---|
| P0 | 阻塞 Wave 完成或最终发布 | 立即升级，暂停后续 Wave，集中解决 |
| P1 | 影响质量或引入回归 | 在 Wave 内解决，必要时调整范围 |
| P2 | 可延后或 YAGNI | 记录决策，进入下一 Wave 或 backlog |

---

## 8. 工具授权与协作流程

### 8.1 授权工具

- **构建与验证**：`cargo`、`clippy`、`fmt`、`test`、`bench`、`audit`
- **代码编辑**：`Read`、`Edit`、`Write`
- **代码搜索**：`Grep`、`Glob`
- **长调研**：`Agent` / `Explore`
- **多代理编排**：`Workflow`
- **数据可视化**：`dataviz` skill
- **任务管理**：`TaskCreate`/`TaskUpdate`/`TaskList`

### 8.2 工具使用规范

- 优先使用专用工具（Read/Edit/Grep）而非 shell 命令
- 所有 `cargo` 命令使用项目内工具链
- 修改前必须 Read 文件，修改后必须验证
- 多代理并行时，每个代理在独立 worktree 或明确划分的文件范围内工作

### 8.3 子代理输出规范

每个子代理返回：

- 执行摘要（3-5 句）
- 修改文件清单
- 验证结果（命令输出/文件内容证据）
- 风险与后续建议

---

## 9. 依赖关系与并行策略

```text
Wave 1: Task 6
  ↓
Wave 2: Task 9, 10, 11, 12（可并行）
Wave 3: Task 13-18（可并行）
Wave 4: Task 19-24（可并行）
Wave 5: Task 25, 27, 28, 29（可并行，Task 26 已完成）
  ↓
Wave 6: Task 30, 31, 32（可并行）
  ↓
Wave 7: Task 33
```

---

## 10. 成功标准

1. 全部 33 个 Task 完成，checklist 所有 `[ ]` 变为 `[x]`
2. 完整验证套件通过
3. `Cargo.toml` 版本号同步为 `1.6.0-omega`
4. `CHANGELOG.md` 有 v1.6.0-omega 汇总章节
5. `CODE_WIKI.md` 反映 v1.6.0 变更
6. `project_memory.md` 新增原则 23+
7. 无新增技术债，或新增技术债已记录并计划偿还
8. 用户确认发布就绪，可推送 `v1.6.0-omega` tag
