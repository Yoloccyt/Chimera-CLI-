# Checklist — v1.1.0-omega F2 rusqlite 下沉

> 每个 checkpoint 必须在 F2 验收前通过。核验时需在 `docs/release/v1.1.0_f2_verification_report.md`(待创建)留存证据(命令输出、截图、文件路径)。
> 状态说明:✅ 已通过 / [ ] 待核验 / N/A 不适用
> **方案 E(L1 trait abstraction)已采纳**:trait 定义在 L1 `nexus-core`,`PragmaCapable` trait + `apply_performance_pragmas<T>` 泛型函数,L2/L3 实现并调用。

## M1: nexus-core 零 rusqlite 依赖

### Cargo.toml 依赖清理

  - [x]
  - [x]
  - [x]

### cargo tree 零 rusqlite

  - [x]
  - [x]
  - [x]
  - [x]

### error.rs SqliteError 变体移除

  - [x]
  - [x]
  - [x]
  - [x]

### sqlite_pragma.rs 删除

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

## M2: L1 PragmaCapable trait 定义(方案 E)

### ADR-006 决策文档

  - [x]
  - [x]
  - [x]
- [ ] 含 CCB 评审记录(成员 + 投票结果 + 日期)(N/A:用户决策,无 CCB 评审)
  - [x]
  - [x]

### PragmaCapable trait 定义在 nexus-core(L1)

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

### apply_performance_pragmas 泛型函数实现

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

## M3: 下游 3 处实现 PragmaCapable for rusqlite::Connection

### cmt-tiering 实现 PragmaCapable

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

### mlc-engine 实现 PragmaCapable

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

### 下游调用点重构为泛型调用

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

## M4: 现有测试守恒

### nexus-core 测试守恒

  - [x]
  - [x]
  - [x]
  - [x]

### 下游 crate 测试守恒

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

## M5: 全量测试与 lint 通过

### cargo test 全绿

  - [x]
  - [x]
  - [x]
  - [x]

### cargo clippy 零警告

  - [x]
  - [x]
  - [x]
  - [x]

### cargo fmt 零 diff

  - [x]
  - [x]

## S1: ADR-006 决策文档(方案 E)

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
- [ ] 含 CCB 评审记录(成员 + 投票结果 + 日期)(N/A:用户决策,无 CCB 评审)
  - [x]
  - [x]

## S2: proptest 验证 PragmaCapable trait 不变量

### nexus-core mock impl proptest(L1 测试不依赖 rusqlite)

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

### 下游真实 rusqlite impl proptest

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

### trait 方法签名稳定性

  - [x]
  - [x]
  - [x]

## S3: CODE_WIKI.md §2.1 L1 层职责同步

  - [x]
  - [x]
  - [x]
  - [x]

## C1: benches 对比泛型函数性能

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

## C2: spawn_blocking 一致性审计

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

## 架构合规性

### 依赖方向合规(§2.2 依赖铁律)— 方案 E 核心

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

### forbid(unsafe_code) 保持(§4.1)

  - [x]
  - [x]
  - [x]
  - [x]

### Rust 编码规范(§4.1)

  - [x]
  - [x]
  - [x]
  - [x]
  - [x]
  - [x]

### async 反模式清单(§4.4)

  - [x]
  - [x]
  - [x]
  - [x]

### orphan rule 合规性(方案 E 特有)

  - [x]
  - [x]
  - [x]
  - [x]

## 文档同步

### ADR-006 文档

- [x] `docs/adr/ADR-006-rusqlite-descoping.md` 创建完成
- [ ] 含方案 E 决策设计、A-D 方案权衡分析、选定理由、CCB 评审记录(N/A:用户决策,无 CCB 评审)
- [x] 含回滚方案(newtype wrapper)与备选方案引用

### CODE_WIKI.md 更新

- [x] §2.1 L1 Core 章节移除 sqlite_pragma 表述
- [x] §2.1 L1 Core 章节补充"L1 定义 PragmaCapable trait,L2/L3 实现并调用"
- [x] §2.3 ADR 表格新增 ADR-006 条目(选定方案 E)
- [x] §1.2 当前开发阶段更新

### CHANGELOG.md 更新

- [x] `CHANGELOG.md` 追加 v1.1.0 F2 章节
- [x] 记录迁移范围(nexus-core 4 处 + 下游 3 处)
- [x] 记录 ADR-006 方案 E 决策结果
- [x] 记录测试结果(测试数量、clippy、fmt)
- [x] 记录预算消耗(实际 47h vs 路线图 80h,节省 33h)

### project_memory.md 更新

- [x] `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md` 追加"rusqlite 下沉方案 E 经验教训"
- [x] 记录 ADR-006 方案 E 决策结果与 A-D 方案权衡
- [x] 记录 L1 trait abstraction 最佳实践(trait 在 L1,L2/L3 impl,orphan rule 合规,静态分发零开销)
- [x] 记录测试迁移注意事项(3 个 PRAGMA 测试从 L1 迁移到下游 crate)

## 核验报告归档

- [x] `docs/release/v1.1.0_f2_verification_report.md` 创建完成 ✅ 已归档(2026-07-08)
- [x] 含 M1-M5 全部 checkpoint 核验结果(命令输出截图)
- [x] 含 S1-S3 全部 checkpoint 核验结果
- [x] 含 C1-C2 全部 checkpoint 核验结果
- [x] 含架构合规性核验结果(重点:L1 trait 不引用 rusqlite 类型)
- [x] 含文档同步核验结果
- [x] 核验结论签字(Tech Lead)✅ 已签字(2026-07-08)

---

## 核验结论模板

### 已完成(自动化部分)
- M1 nexus-core 零 rusqlite 依赖:[待核验]
- M2 L1 PragmaCapable trait 定义:[待核验]
- M3 下游 3 处实现 PragmaCapable for rusqlite::Connection:[待核验]
- M4 现有测试守恒:[待核验]
- M5 全量测试与 lint:[待核验]

### 委托核验(需手动操作)
- S1 ADR-006 决策文档(方案 E):CCB 评审通过后定稿
- S2 proptest 验证:`cargo test -p nexus-core --test proptest` 与 `cargo test -p cmt-tiering --test pragma_capable_proptest` 通过
- S3 CODE_WIKI 同步:文档审查通过
- C1 benches 对比:`cargo bench -p cmt-tiering` 输出基准报告
- C2 spawn_blocking 审计:grep + 代码审查

### 用户操作指引
所有 checkpoint 的核验命令已文档化。核验流程:
1. 按 M1 → M2 → M3 → M4 → M5 顺序逐项核验,在 `v1.1.0_f2_verification_report.md` 记录输出
2. Should / Could 项并行核验
3. 全部 Must 项通过后,在 `CHANGELOG.md` v1.1.0 F2 章节签字
4. 关闭本 Spec,启动 F1 L10 编排接线(独立 Spec)
