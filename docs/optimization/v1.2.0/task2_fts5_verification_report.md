# Task 2: repo-wiki FTS5 全文索引验证报告

> **任务**:Task 2 — repo-wiki FTS5 全文索引 [N15]
> **日期**:2026-07-09
> **架构层**:L5 Knowledge(`repo-wiki` crate)
> **对应定律**:Ω-Compress(压缩 — O(log n) 倒排索引替代 O(n) 全表扫描)
> **状态**:代码实现 + 单 crate 验证全部通过;workspace 级验证受并行执行约束延后

---

## 1. 任务目标

将 `repo-wiki` 的全文检索从 `LIKE '%query%'` 全表扫描(O(n))升级为 FTS5 倒排索引 `MATCH` 查询(O(log n)),在 1000+ 文档规模下显著降低检索延迟。FTS5 不可用时自动降级到 LIKE,保证功能可用性。

**验收门槛**:
- FTS5 可用时走 `MATCH` 查询
- FTS5 不可用时降级到 `LIKE`
- bench 对比 `MATCH` vs `LIKE` 性能
- `cargo test -p repo-wiki` 通过

---

## 2. 实现摘要

### 2.1 新增/修改文件

| 文件 | 类型 | 说明 |
|------|------|------|
| `crates/repo-wiki/src/fts.rs` | 新增 | FTS5 虚拟表管理 + 索引同步 + MATCH 查询 + LIKE 降级 + 查询安全化 + 8 单元测试 |
| `crates/repo-wiki/src/store.rs` | 修改 | `WikiStore` 集成 FTS5:`fts_capability` 字段 + `search_fulltext` 优先 FTS5 + insert/delete 同步索引 |
| `crates/repo-wiki/tests/fts_test.rs` | 新增 | 6 集成测试(召回 / 降级 / 同步 / capability / UPSERT / 安全化) |
| `crates/repo-wiki/benches/fts_bench.rs` | 新增 | 对比 FTS5 MATCH vs LIKE 在 1000 文档规模的延迟 |

### 2.2 核心设计:`FtsCapability` 枚举

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsCapability {
    Available,    // FTS5 扩展可用,entries_fts 虚拟表已就绪
    Unavailable,  // FTS5 不可用,降级到 LIKE
}
```

`Copy` 语义:在 `WikiStore` 中作为只读字段缓存,clone 时零开销。

### 2.3 FTS5 虚拟表 schema

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    entry_id UNINDEXED,  -- 仅用于 JOIN/DELETE,不进倒排索引
    title,
    content,
    tokenize='unicode61'  -- Unicode 分词,支持中文
);
```

### 2.4 索引同步策略

FTS5 不支持 `INSERT OR REPLACE`(无 PRIMARY KEY 约束),采用 DELETE + INSERT 保证 UPSERT 幂等性:

```rust
pub fn sync_fts_insert(conn: &Connection, entry: &WikiEntry) -> Result<(), WikiError> {
    // 先删除可能存在的旧索引(幂等:无匹配行时 DELETE 不报错)
    conn.execute("DELETE FROM entries_fts WHERE entry_id = ?1;", params![entry.entry_id])?;
    // 插入新索引
    conn.execute("INSERT INTO entries_fts(entry_id, title, content) VALUES (?1, ?2, ?3);",
        params![entry.entry_id, entry.title, entry.content])?;
    Ok(())
}
```

### 2.5 查询优先级与降级

```
WikiStore::search_fulltext(query)
    ├─ fts_capability == Available?
    │   ├─ YES → search_fts(MATCH) → 成功? 返回结果
    │   │                        └─ 失败? 降级 search_like(LIKE)
    │   └─ NO  → search_like(LIKE)
```

### 2.6 查询安全化

FTS5 MATCH 语法对特殊字符(`*`, `"`, `:`, `(`, `)`)敏感,`sanitize_fts5_query` 将每个 token 包裹为 `"token"` phrase(字面量),防止注入:

```rust
pub(crate) fn sanitize_fts5_query(query: &str) -> String {
    query.split_whitespace()
        .map(|token| token.replace('"', ""))  // 移除内部双引号
        .filter(|t| !t.is_empty())
        .map(|token| format!("\"{token}\""))  // 包裹为 phrase
        .collect::<Vec<_>>()
        .join(" ")  // FTS5 隐式 AND
}
```

### 2.7 初始化回填

`init_fts_table` 创建虚拟表后,用 `NOT IN` 增量回填已有数据(适用于"已有数据的库首次启用 FTS5"场景):

```sql
INSERT INTO entries_fts(entry_id, title, content)
SELECT entry_id, title, content FROM entries
WHERE entry_id NOT IN (SELECT entry_id FROM entries_fts);
```

回填失败不阻断初始化(补救措施,失败时新插入仍正常索引)。

---

## 3. 测试覆盖

### 3.1 单元测试(`src/fts.rs`,8 个)

| 测试 | 验证点 |
|------|--------|
| `test_sanitize_simple_query` | 多 token 用空格连接,每个包裹为 phrase |
| `test_sanitize_strips_double_quotes` | 用户输入含双引号时移除,防止 phrase 提前闭合 |
| `test_sanitize_empty_query` | 空输入返回空字符串 |
| `test_sanitize_whitespace_only` | 纯空白返回空字符串 |
| `test_sanitize_single_token` | 单 token 包裹为 `"token"` |
| `test_sanitize_chinese_token` | unicode61 tokenizer 对中文按字符分词 |
| `test_fts_capability_is_available` | `is_available()` 返回值正确 + Copy 语义 |
| `test_fts_capability_equality` | `Available == Available`, `Available != Unavailable` |

### 3.2 集成测试(`tests/fts_test.rs`,6 个)

| 测试 | 验证点 |
|------|--------|
| `test_fts5_search_returns_relevant_docs` | FTS5 MATCH 查询返回相关文档 |
| `test_fts5_fallback_to_like_when_unavailable` | FTS5 不可用时降级到 LIKE |
| `test_fts5_index_document_synced` | index_document 时 FTS5 索引同步写入 |
| `test_fts5_fallback_handles_invalid_query` | FTS5 语法错误降级到 LIKE |
| `test_fts5_capability_detected` | 运行时检测 FTS5 可用性 |
| `test_fts5_upsert_no_duplicate_index` | UPSERT 不产生重复索引 |

### 3.3 bench(`benches/fts_bench.rs`)

对比 FTS5 `MATCH`(O(log n) 倒排索引)vs `LIKE`(O(n) 全表扫描)在 1000 文档规模下的查询延迟,`sample_size=10`(criterion 最小值)。

---

## 4. 验证结果

### 4.1 单 crate 验证(2026-07-09 执行)

| 验证项 | 命令 | 结果 |
|--------|------|------|
| 编译检查 | `cargo check -p repo-wiki --all-targets` | exit 0(0.95s,stable-x86_64-pc-windows-gnu) |
| 全量测试 | `cargo test -p repo-wiki` | **35 passed / 0 failed**(6 fts_test + 12 iscm + 1 proptest + 14 store + 2 doctest) |
| Clippy | `cargo clippy -p repo-wiki --all-targets -- -D warnings` | exit 0,零警告(修复 2 处 doc_lazy_continuation) |
| Fmt | `cargo fmt -p repo-wiki -- --check` | exit 0,零 diff |
| Bench 编译 | `cargo bench -p repo-wiki --bench fts_bench --no-run` | exit 0,编译通过 |

### 4.2 workspace 级验证

延后(并行执行约束:只运行单 crate 命令避免冲突)。workspace 级 `cargo test --workspace` / `cargo clippy --workspace` / `cargo fmt --all --check` 由主控代理统一执行。

### 4.3 FTS5 编译配置

**关键发现**:`libsqlite3-sys 0.30.1` bundled 的 `build.rs` 第 129 行硬编码 `-DSQLITE_ENABLE_FTS5`,FTS5 在当前编译中**默认可用**,无需修改 `.cargo/config.toml`。但仍保留运行时检测作为系统边界校验(跨平台/非 bundled rusqlite / schema 损坏 / 磁盘权限)。

---

## 5. 设计决策摘要

### 5.1 standalone 而非 external content

**决策**:FTS5 虚拟表用 standalone 模式(自存文本副本),而非 external content 模式(`content='entries'`)。

**理由**:external content 需配合触发器同步,逻辑复杂且 DELETE 语义在 UPSERT 场景下易出错。standalone 虽多存一份文本(FTS5 倒排索引体积约为原文 50%),但同步逻辑清晰可控,在 1000+ 文档规模下存储开销可接受,换取代码可维护性与正确性。

### 5.2 运行时检测而非编译时假设

**决策**:`init_fts_table` 尝试创建虚拟表,失败则标记 `Unavailable`,不中断初始化。

**理由**:虽然 `libsqlite3-sys 0.30.1` bundled 默认启用 FTS5,但运行时检测仍保留,因为:1) 跨平台/非 bundled rusqlite 可能行为不同;2) 已有数据库文件可能 schema 损坏;3) FTS5 虚拟表创建可能因磁盘/权限失败。运行时检测是系统边界校验,符合"只在系统边界做校验"的约束。

### 5.3 entry_id UNINDEXED

**决策**:`entry_id` 列标记为 `UNINDEXED`。

**理由**:`entry_id` 仅用于 JOIN 关联和 DELETE WHERE 同步,不参与全文检索。`UNINDEXED` 使该列不进入倒排索引,节省索引体积与写入开销,同时仍可作为普通列读取。

### 5.4 查询安全化(phrase 包裹)

**决策**:用 `sanitize_fts5_query` 将每个 token 包裹为 `"token"` phrase,而非直接透传用户输入。

**理由**:FTS5 MATCH 语法对特殊字符敏感,直接透传可能触发 SQL 语法错误。phrase 包裹将用户输入转为字面量(FTS5 不解析其内特殊字符),兼顾安全性与召回率。多 token 用空格连接(FTS5 隐式 AND 语义)。

### 5.5 降级策略(FTS5 → LIKE)

**决策**:FTS5 不可用或查询语法错误时,静默降级到 LIKE,不返回错误。

**理由**:保证功能可用性优先于性能。FTS5 是性能优化,不是功能前提。降级时记录 warning 日志提示"FTS5 search failed, falling back to LIKE"。

---

## 6. 已知限制

1. **bench 实际运行未执行**:bench 仅验证编译通过(`--no-run`),实际 MATCH vs LIKE 延迟对比需运行 bench。理论上,FTS5 倒排索引在 1000+ 文档规模下应显著快于 LIKE 全表扫描。
2. **workspace 级验证延后**:受并行执行约束,仅执行单 crate 验证。workspace 级回归由主控代理统一执行。
3. **unicode61 tokenizer 对中文按字符分词**:非最优中文分词(无词级语义),但对全文检索召回率可接受。如需更高质量,可后续引入 jieba 等 tokenizer(需 SQLite 扩展)。

---

## 7. 关联文档

- Spec:`.trae/specs/v1-2-0-omega-deferred-optimization/spec.md`
- Tasks:`.trae/specs/v1-2-0-omega-deferred-optimization/tasks.md`(Task 2)
- Checklist:`.trae/specs/v1-2-0-omega-deferred-optimization/checklist.md`(Task 2)
- CHANGELOG:`CHANGELOG.md`(v1.2.0 Task 2 章节)
- 项目记忆:`project_memory.md`(v1.2.0-omega Task 2 教训)
