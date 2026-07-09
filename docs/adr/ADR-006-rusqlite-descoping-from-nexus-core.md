# ADR-006: rusqlite 依赖从 nexus-core 下沉到 L3(方案 E:L1 trait abstraction)

- **Status**: Accepted
- **Date**: 2026-07-04
- **Decision Maker**: Chimera CLI Team(用户决策采纳方案 E)
- **Supersedes**: 无
- **Superseded by**: 无

> ⚠️ **编号冲突说明**:`docs/architecture/adr_index.md` 中 ADR-006 已分配给"异步运行时 — Tokio"。本文件按 F2 任务要求的文件名创建,建议维护者在合并时重新编号为 **ADR-027**,并同步更新 `adr_index.md`。

---

## Context(背景)

### 问题来源

`nexus-core`(L1 Core)违背了 §2.2 依赖铁律中的"L1 必须保持最小依赖,不能直接 import 上层任何 crate"原则 —— 它直接依赖 `rusqlite`,仅为承载一个 56 行的工具模块 `sqlite_pragma`。该模块仅暴露一个公开函数 `apply_performance_pragmas(&Connection)`,设置 5 个性能 PRAGMA(`synchronous=NORMAL` / `cache_size=-65536` / `mmap_size=268435456` / `temp_store=MEMORY` / `wal_autocheckpoint=1000`)。

### 当前现状(已审计)

`nexus-core` 中的 rusqlite 使用共 4 处:

1. `crates/nexus-core/Cargo.toml:18-19` — `rusqlite = { workspace = true }`
2. `crates/nexus-core/src/lib.rs:39` — `pub mod sqlite_pragma;`
3. `crates/nexus-core/src/sqlite_pragma.rs` — 1 个 pub fn + 3 个单元测试
4. `crates/nexus-core/src/error.rs:36-42` — `NexusError::SqliteError(#[from] rusqlite::Error)`

下游调用点 3 处,均通过 `nexus_core::sqlite_pragma::apply_performance_pragmas(conn)` 委托,再用 `.map_err()` 转换为各自的库层错误类型:

| 调用点 | 层级 | 文件 |
|--------|------|------|
| `cmt-tiering::warm` | L3 | `file://d:/Chimera%20CLI/crates/cmt-tiering/src/warm.rs:464` |
| `cmt-tiering::cold` | L3 | `file://d:/Chimera%20CLI/crates/cmt-tiering/src/cold.rs:539` |
| `mlc-engine::l3_procedural` | L2 | `file://d:/Chimera%20CLI/crates/mlc-engine/src/l3_procedural.rs:415` |

3 个调用方本身已直接依赖 `rusqlite`(各自做 SQL 操作),下沉 `sqlite_pragma` 不改变它们的依赖集,只改变 import 路径。

### 为什么必须决策

v1.1.0 路线图 F2 明确要求:将 `nexus-core` 中的 `rusqlite` 依赖下沉,使 L1 回归"最小依赖"。RC 阶段 §3.1 规则要求"禁止跨层重构",但 F2 是已规划的下沉,且改动面小(1 个函数 + 3 处调用),属于"架构让步回滚"而非"新架构让步",可执行。

核心张力:**消除 L1 对 rusqlite 的依赖** × **消除重复(DRY)** × **遵循依赖方向(§2.2)** —— 三个目标不可能同时满足,必须舍弃一个。本 ADR 的本质是选择舍弃哪一个。

初稿(方案 D)选择舍弃 DRY,代价是 5 行 PRAGMA 代码在 3 个 crate 中重复。复审阶段用户提出方案 E(L1 trait abstraction),可在不违反依赖铁律的前提下保留 DRY,故本 ADR 改为推荐方案 E。

---

## Decision(决策)

**采纳方案 E:L1 trait abstraction。**

在 `nexus-core` 中定义 `PragmaCapable` trait(不引用 `rusqlite` 类型),并提供基于该 trait 的泛型函数 `apply_performance_pragmas<T: PragmaCapable>(conn: &T)`。下游 crate(`cmt-tiering` / `mlc-engine`)在自己的源码中为 `rusqlite::Connection` 实现 `PragmaCapable` trait,然后调用 L1 的泛型函数。

### 核心代码骨架

```rust
// crates/nexus-core/src/storage_traits.rs(L1,不依赖 rusqlite)
use crate::error::NexusError;

/// PRAGMA 能力抽象 — 由下游 crate 为 rusqlite::Connection 实现
pub trait PragmaCapable {
    fn pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError>;
    fn pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError>;
}

/// 应用 SQLite 性能优化 PRAGMA(连接级,影响所有附加数据库)
///
/// 必须在 `journal_mode=WAL` 设置之后调用。
pub fn apply_performance_pragmas<T: PragmaCapable>(conn: &T) -> Result<(), NexusError> {
    conn.pragma_update_string("synchronous", "NORMAL")?;
    conn.pragma_update_int("cache_size", -65536)?;
    conn.pragma_update_int("mmap_size", 268435456)?;
    conn.pragma_update_string("temp_store", "MEMORY")?;
    conn.pragma_update_int("wal_autocheckpoint", 1000)?;
    Ok(())
}
```

```rust
// crates/cmt-tiering/src/storage_impl.rs(L3,依赖 rusqlite,newtype wrapper impl trait)
use nexus_core::{NexusError, PragmaCapable};

/// PRAGMA 能力 wrapper — WHY newtype: 避免 Rust coherence 冲突
/// (两 crate 不能同时 impl 同一 trait for 同一 type)
pub struct PragmaConn<'a>(pub &'a rusqlite::Connection);

impl<'a> PragmaCapable for PragmaConn<'a> {
    fn pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError> {
        self.0.pragma_update(None, key, value)
            .map_err(|e| NexusError::SerializationError(format!("SQLite PRAGMA {key}={value} 失败: {e}")))
    }
    fn pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError> {
        self.0.pragma_update(None, key, value)
            .map_err(|e| NexusError::SerializationError(format!("SQLite PRAGMA {key}={value} 失败: {e}")))
    }
}

// 调用点改为泛型分发(warm.rs / cold.rs / l3_procedural.rs)
let wrapper = PragmaConn(conn);
nexus_core::apply_performance_pragmas(&wrapper)?;
```

> ⚠️ **方案 E 实施修正:newtype wrapper**(2026-07-08)
>
> 原方案 E 计划让 cmt-tiering 和 mlc-engine **直接** impl `PragmaCapable for rusqlite::Connection`。但实施时发现 **Rust coherence 规则**禁止两个 crate 同时 impl 同一 trait for 同一 type —— 即使 orphan rule 允许各 crate 独立 impl,当两 crate 被同一 binary/test 链接(根 Cargo.toml E2E 测试同时依赖两者)时,编译器报 `conflicting implementations`。
>
> **修正方案**:采用本 ADR §回滚方案预置的 **newtype wrapper**(F2.1.5.6),每个 crate 定义独立的 `pub struct PragmaConn<'a>(pub &'a rusqlite::Connection)` 并 impl PragmaCapable for 它。各 crate 的 newtype 是不同类型,coherence 不冲突。
>
> 错误变体从原计划的 `SqliteError` 改为 `SerializationError`,因为 F2.3.3 将删除 `SqliteError` 变体,改用 `SerializationError` 避免 F2.3.3 破坏 impl。虽然语义不完美,但该错误是 transient(调用方立即 `.map_err()` 转各自库层错误),可调试性不受影响。

### 关键决策依据(WHY)

1. **三目标同时满足**:方案 E 是唯一能同时满足"消除 L1 对 rusqlite 依赖"+"消除 DRY 违反"+"遵循依赖方向(§2.2)"的方案。
2. **依赖方向合规**:trait 定义在 L1,L2/L3 向下依赖 L1(§2.2 允许),没有任何向上依赖。
3. **L1 最小依赖**:`PragmaCapable` trait 不引用 `rusqlite` 任何类型,`nexus-core` 可彻底移除 `rusqlite` 依赖。
4. **Orphan Rule 合规**:`PragmaCapable` 是 `nexus-core` 的本地 trait,`rusqlite::Connection` 是外部类型,Rust orphan rule 允许"为外部类型实现本地 trait"。
5. **DRY 满足**:5 行核心 PRAGMA 逻辑集中在 L1 一处定义,下游仅写 6 行 trait impl boilerplate(机械转发),不再是逻辑复制。
6. **可扩展性**:未来调整 PRAGMA(如新增 `journal_mode=WAL`)只改 L1 一处;新增存储工具(连接池、WAL 管理)可在 trait 上扩展方法,所有实现自动生效。
7. **与 ADR-011 哲学一致**:`NexusError::SqliteError` 保留在 L1(由 trait impl 通过 `.map_err()` 填充),错误体系不污染下游 crate 的库层错误类型。
8. **回滚风险低**:方案 E 改动集中在 5 文件,`git revert` 单次提交即可回滚;若 orphan rule 实测遇到障碍,可降级到备选方案 D。

---

## Consequences(后果)

### 正面后果

- ✅ `nexus-core` 完全脱离 `rusqlite` 依赖,F2 目标达成
- ✅ 5 行核心 PRAGMA 逻辑单一事实源位于 L1,DRY 完美满足
- ✅ 依赖铁律保持完整,RC 阶段无架构让步
- ✅ `PragmaCapable` trait 为未来扩展(连接池、WAL 管理、备份)预留扩展点
- ✅ 调用方代码体积不变(仍是一行 `apply_performance_pragmas(&conn)?`),错误转换链路与现状一致
- ✅ `cargo tree -p nexus-core` 输出更精简,L1 真正"最小化"
- ✅ 单元测试在 L1 用 mock 实现验证 PRAGMA 顺序与参数,下游保留 PRAGMA 生效测试,双层保障

### 负面后果

- ❌ 每个 L2/L3 crate 需写 6 行 trait impl boilerplate(机械转发,无逻辑),但远比方案 D 的 5 行 PRAGMA 复制 + 1 个测试复制更轻
- ❌ 引入一个新 trait,API 表面微增(L1 多一个 `storage_traits` 模块)
- ❌ `NexusError::SqliteError` 变体仍保留在 L1(被 trait impl 引用),无法删除 —— 但其 `#[from] rusqlite::Error` 可移除(改为构造时显式 `.map_err()`),`nexus-core` 仍可不依赖 `rusqlite`

### 中性后果

- ➖ `nexus-core/src/sqlite_pragma.rs` 重构为 `storage_traits.rs`(模块边界更通用,语义从"SQLite 专用"扩展为"存储抽象")
- ➖ 下游调用方需 `use nexus_core::storage_traits::PragmaCapable;`( trait 必须 import 才能调用方法),比当前多 1 行 import
- ➖ 测试分布:L1 保留 3 个单元测试(改用 mock 实现 `PragmaCapable`),下游保留 PRAGMA 生效测试,测试总数不变

---

## Alternatives Considered(备选方案)

### 方案 A:迁移 `sqlite_pragma` 到 `scc-cache`(L3)

- **依赖方向**:❌ **违规**。`mlc-engine`(L2)→ `scc-cache`(L3)是 L2→L3 向上依赖,违反 §2.2。
- **测试影响**:`scc-cache` 需新增 `pub fn` + 测试;`mlc-engine` 与 `cmt-tiering` 需改 import 路径。约改 5 文件。
- **长期维护**:未来若 `repo-wiki`(L5)也需 PRAGMA,会产生 L5→L3 向上依赖,同样违规。扩展性差。
- **代码复用率**:高(单一定义)。
- **实施工作量**:中等(3 处 import 改写 + 1 处模块迁移)。
- **回滚风险**:低。
- **结论**:依赖方向硬伤,**否决**。

### 方案 B:迁移到 `cmt-tiering`(L3)

- **依赖方向**:❌ **违规**。`mlc-engine`(L2)→ `cmt-tiering`(L3)同样是 L2→L3 向上依赖。`scc-cache`(L3)→ `cmt-tiering`(L3)同层互引合规,但 mlc-engine 问题无解。
- **测试影响**:与方案 A 类似,约改 5 文件。
- **长期维护**:`cmt-tiering` 职责膨胀(它本应只管"能力记忆分层",现要承担"PRAGMA 工具库"职责),违反单一职责。
- **代码复用率**:高。
- **实施工作量**:中等。
- **回滚风险**:低。
- **结论**:依赖方向硬伤 + 职责污染,**否决**。

### 方案 C:新建 `storage-shared` crate(L3)

- **依赖方向**:❌ **违规**。`storage-shared` 若放 L3,`mlc-engine`(L2)依然不能依赖它。若强行放 L1(与 `nexus-core` 并列),则 L1 仍然间接承载 rusqlite 依赖,F2 目标未达成 —— 只是把问题从 `nexus-core` 搬到 `storage-shared`,本质未变。
- **测试影响**:需新建 crate 骨架(`Cargo.toml` / `lib.rs` / `error.rs` / `tests/`),约 8-10 文件。
- **长期维护**:扩展性好(未来可放连接池、WAL 管理等),但与 `scc-cache::wal` 的 `SqliteWal` 实现职责重叠,需进一步抽象。
- **代码复用率**:高。
- **实施工作量**:大(新建 crate + 3 处 import 改写 + ADR 索引更新 + workspace `Cargo.toml` members 更新)。
- **回滚风险**:中(新建 crate 后回滚需删除 crate + 还原 import)。
- **RC 阶段约束**:§3.1 明确"禁止引入新 crate",本方案与 RC 阶段规则冲突。
- **结论**:依赖方向硬伤(放 L3)或 F2 未达成(放 L1)+ 违反 RC 禁令,**否决**。

### 方案 E:L1 trait abstraction(采纳)

- **依赖方向**:✅ **合规**。`PragmaCapable` trait 定义在 L1(`nexus-core`),L2/L3 向下依赖 L1,符合 §2.2。
- **测试影响**:`nexus-core` 保留 3 个单元测试(改用 mock 实现 `PragmaCapable`),下游 crate 保留 PRAGMA 生效测试。约改 5 文件。
- **长期维护**:扩展性极佳。未来调整 PRAGMA 只改 L1 一处;新增存储工具(连接池、WAL 管理)可在 trait 上扩展方法,所有实现自动生效。
- **代码复用率**:高(5 行核心逻辑单一事实源,下游仅 6 行 impl boilerplate)。
- **实施工作量**:中等(定义 trait + 3 处 impl + 3 处调用改为泛型分发 + 1 处模块重命名)。
- **回滚风险**:低(`git revert` 单次提交)。
- **Orphan Rule**:✅ 合规。`PragmaCapable` 是 `nexus-core` 本地 trait,可为外部类型 `rusqlite::Connection` 实现。
- **结论**:**采纳**。三目标同时满足,且为未来存储抽象扩展预留空间。

### 方案 D:各 crate 复制 PRAGMA 代码(备选)

- **依赖方向**:✅ **合规**。无任何跨层依赖,每个 crate 自包含。
- **测试影响**:每个 crate 新增 1 个等价单元测试,测试总数基本不变。
- **长期维护**:新增存储工具时各 crate 各自实现 —— 但 PRAGMA 是极稳定的 SQLite 优化(5 年内不会变),维护频率极低。
- **代码复用率**:低(5 行重复),但重复成本可接受。
- **实施工作量**:小(3 处内联 + 4 处删除 + 1 处 error.rs 清理,共约 8 文件)。
- **回滚风险**:极低(单次 `git revert`)。
- **结论**:**备选方案**。若方案 E 在实施过程中遇到 orphan rule 障碍(理论不会,但需运行时验证),或 trait 抽象引发其它边界问题,可降级到方案 D。方案 D 的实施步骤作为方案 E 的回滚预案保留。

---

## Implementation Steps(实施步骤大纲,方案 E)

1. **nexus-core/src/storage_traits.rs**:新建模块,定义 `PragmaCapable` trait(2 个方法)+ 泛型函数 `apply_performance_pragmas<T: PragmaCapable>`(5 行 PRAGMA 逻辑从 `sqlite_pragma.rs` 迁移)
2. **nexus-core/src/lib.rs**:将 `pub mod sqlite_pragma;` 改为 `pub mod storage_traits;`,导出 `PragmaCapable` 与 `apply_performance_pragmas`
3. **nexus-core/src/sqlite_pragma.rs**:删除整个文件(逻辑已迁移到 `storage_traits.rs`)
4. **nexus-core/src/error.rs**:保留 `NexusError::SqliteError` 变体(供 trait impl 通过 `.map_err()` 构造),但移除 `#[from] rusqlite::Error` 派生(改为显式构造,这样 `nexus-core` 不再需要 import `rusqlite`)
5. **nexus-core/Cargo.toml**:删除 `rusqlite = { workspace = true }` 行
6. **nexus-core/src/storage_traits.rs**:新增 mock 实现的 3 个单元测试(用 `MockConn` 实现 `PragmaCapable`,验证 PRAGMA 调用顺序与参数)
7. **cmt-tiering/src/warm.rs**:新增 `impl PragmaCapable for rusqlite::Connection`(6 行 boilerplate),调用点改为 `nexus_core::storage_traits::apply_performance_pragmas(&conn)?`
8. **cmt-tiering/src/cold.rs**:同上
9. **mlc-engine/src/l3_procedural.rs**:同上
10. **验证 + 文档同步**:`cargo tree -p nexus-core | Select-String rusqlite` 应无输出;`cargo check --workspace` + `cargo test --workspace` 全绿;更新 `docs/architecture/adr_index.md`(重新编号为 ADR-027)、`CODE_WIKI.md` §2.3 ADR 表、`CHANGELOG.md` 记录 F2 完成

---

## References(参考)

- v1.1.0 路线图 F2 — `file://d:/Chimera%20CLI/docs/release/v1.1.0_roadmap.md`
- §2.2 依赖铁律 — `.trae/rules/nuxus规则.md`
- §6.2 Week 1-8 实战新红线 — 同上
- §3.1 RC 阶段规则 — 同上
- ADR-005(持久化存储选型)— `file://d:/Chimera%20CLI/docs/architecture/adr_index.md`
- ADR-011(错误处理策略:库层 thiserror)— 同上
- ADR-012(内存安全:forbid(unsafe_code))— 同上
- Rust Orphan Rule(Reference §3.16 Implementations)— https://doc.rust-lang.org/reference/items/implementations.html
- 现状审计代码:
  - `file://d:/Chimera%20CLI/crates/nexus-core/src/sqlite_pragma.rs`
  - `file://d:/Chimera%20CLI/crates/nexus-core/src/error.rs`(L36-42)
  - `file://d:/Chimera%20CLI/crates/cmt-tiering/src/warm.rs:464`
  - `file://d:/Chimera%20CLI/crates/cmt-tiering/src/cold.rs:539`
  - `file://d:/Chimera%20CLI/crates/mlc-engine/src/l3_procedural.rs:415`
