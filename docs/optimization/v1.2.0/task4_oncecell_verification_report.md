# Task 4: chimera-cli OnceCell 懒加载验证报告

> **任务**:Task 4 — chimera-cli OnceCell 懒加载 [E1]
> **日期**:2026-07-09
> **架构层**:L10 Interface(`chimera-cli` crate)
> **对应定律**:Ω-Compress(压缩 — 按需解析消除启动期未使用 section 的解析开销)
> **状态**:代码实现 + 单 crate 验证全部通过;workspace 级验证由主控代理统一执行

---

## 1. 任务目标

将 `chimera-cli` 的 14 个顶层配置 section 从 eager 加载(`figment.extract::<ChimeraConfig>()` 一次性全量反序列化)改为 `OnceLock` 懒加载(首次访问对应 getter 时按路径反序列化该 section),消除启动期未使用 section 的解析开销。公开 API 向后兼容。

**验收门槛**:
- 14 section 全部改为懒加载
- 向后兼容(公开 API 不变)
- `cargo test -p chimera-cli` 通过
- `cargo test --workspace` 通过

---

## 2. 实现摘要

### 2.1 新增/修改文件

| 文件 | 类型 | 说明 |
|------|------|------|
| `crates/chimera-cli/src/config.rs` | 修改 | 新增 `LazySection<T>` 辅助类型 + `LazyConfig` 容器 + 14 个 section getter + `extract_section` 辅助函数 + `to_chimera_config` 聚合方法 |
| `crates/chimera-cli/tests/config_test.rs` | 新增 | 5 核心测试(向后兼容 / 懒加载隔离性 / 缓存命中 / 14 section 全覆盖 / to_chimera_config 聚合) |
| `crates/chimera-cli/tests/config_lazy.rs` | 新增 | 17 测试(3 核心 + 14 section 等价性,JSON 字符串比对) |

### 2.2 核心设计:`LazySection<T>` 辅助类型

```rust
struct LazySection<T> {
    /// WHY 缓存 Err:配置文件格式错误不会因重试自愈,缓存错误既避免
    /// 重复解析坏 section,也保证"懒加载只算一次"的语义一致。
    cell: OnceLock<Result<T, String>>,
}

impl<T> LazySection<T> {
    fn get_or_try_init<F>(&self, init: F) -> Result<&T>
    where F: FnOnce() -> std::result::Result<T, String>
    {
        match self.cell.get_or_init(init) {
            Ok(value) => Ok(value),
            Err(msg) => Err(anyhow::anyhow!("配置 section 解析失败: {msg}")),
        }
    }
}
```

封装 `OnceLock` + "首次解析、后续缓存"模式,使 14 个 getter 各自缩为一行,避免样板重复。

### 2.3 `LazyConfig` 容器

```rust
pub struct LazyConfig {
    figment: Figment,  // 合并后的 provider(defaults > file > env)
    nexus: LazySection<NexusConfig>,
    quest: LazySection<QuestConfig>,
    thinking_toggle: LazySection<ThinkingToggleConfig>,
    repo_wiki: LazySection<RepoWikiConfig>,
    model_router: LazySection<ModelRouterConfig>,
    osa: LazySection<OsaConfig>,
    kvbsr: LazySection<KvbsrConfig>,
    pvl: LazySection<PvlConfig>,
    mtpe: LazySection<MtpeConfig>,
    gqep: LazySection<GqepConfig>,
    seccore: LazySection<SeccoreConfig>,
    mcp: LazySection<McpConfig>,
    evolution: LazySection<EvolutionConfig>,
    monitoring: LazySection<MonitoringConfig>,
}
```

`LazyConfig::new` 只构建 provider 链不 extract,各 getter 首次调用时通过 `Figment::extract_inner` 按 key 路径反序列化对应 section 并缓存。

### 2.4 Section 级懒加载(14 个 getter)

每个 getter 缩为一行,通过 `extract_section` 辅助函数从 Figment 按路径提取:

```rust
pub fn nexus(&self) -> Result<&NexusConfig> {
    self.nexus.get_or_try_init(|| extract_section(&self.figment, "nexus"))
}
```

`extract_section` 使用 `Figment::extract_inner::<T>(key_path)` 按点分路径从合并 provider 树提取子树反序列化。

### 2.5 `to_chimera_config` 聚合

```rust
pub fn to_chimera_config(&self) -> Result<ChimeraConfig> {
    Ok(ChimeraConfig {
        nexus: self.nexus()?.clone(),
        quest: self.quest()?.clone(),
        // ... 14 sections
    })
}
```

用于需要完整配置的场景(如 `aether config dump`)。

### 2.6 向后兼容

| 既有 API | 状态 |
|---------|------|
| `config::load(path)` | 不变,仍全量 `extract::<ChimeraConfig>()` |
| `config::default_config()` | 不变 |
| `config::default_config_path()` | 不变 |
| `config::init_config_file(path)` | 不变 |
| `ChimeraConfig::default()` | 不变 |
| `LazyConfig::new(path)` | **新增** |
| `LazyConfig::<section>()` | **新增**(14 个 getter) |
| `LazyConfig::to_chimera_config()` | **新增** |

---

## 3. 测试覆盖

### 3.1 核心测试(`tests/config_test.rs`,5 个)

| 测试 | 验证点 |
|------|--------|
| `test_backward_compatible_api` | 既有 API(default_config / load / init_config_file)签名与行为不变 |
| `test_lazy_load_unaccessed_section_not_parsed` | 错误探针:malformed quest 不影响 nexus 访问,证明 section 级懒加载 |
| `test_repeated_access_returns_cached` | `std::ptr::eq` 验证重复访问返回同一引用(OnceLock 缓存命中) |
| `test_all_14_sections_lazy_accessible` | 14 section 逐个首次访问成功 |
| `test_lazy_to_chimera_config` | 聚合方法产出完整 ChimeraConfig,未配置 section 回退默认值 |

### 3.2 Section 等价性测试(`tests/config_lazy.rs`,17 个)

| 测试 | 验证点 |
|------|--------|
| `test_lazy_config_section_not_extracted_until_accessed` | malformed quest 不影响 nexus,证明懒加载隔离性 |
| `test_lazy_config_repeated_access_returns_cached` | `std::ptr::eq` 验证缓存命中(nexus + quest) |
| `test_backward_compatible_api_unchanged` | 既有 eager API 链路 + LazyConfig 与之等价 |
| `test_lazy_section_<name>` × 14 | 逐 section 比对 lazy vs eager JSON 字符串等价(nexus/quest/thinking_toggle/repo_wiki/model_router/osa/kvbsr/pvl/mtpe/gqep/seccore/mcp/evolution/monitoring) |

### 3.3 等价性验证策略

14 个 section 类型未派生 `PartialEq`(定义在 nexus-core,RC 阶段不修改核心类型),用 JSON 往返字符串比对替代:

```rust
fn assert_section_eq<T: Serialize>(lazy: &T, eager: &T, name: &str) {
    let l = serde_json::to_string(lazy).expect("序列化 lazy section 失败");
    let e = serde_json::to_string(eager).expect("序列化 eager section 失败");
    assert_eq!(l, e, "section {name}: lazy 与 eager 不一致");
}
```

与现有 `test_default_config_roundtrip` 同策略,零侵入验证等价性。

---

## 4. 验证结果

### 4.1 单 crate 验证(2026-07-09 执行)

| 验证项 | 命令 | 结果 |
|--------|------|------|
| 编译检查 | `cargo check -p chimera-cli --all-targets` | exit 0 |
| 全量测试 | `cargo test -p chimera-cli` | **41 passed / 0 failed**(5 config_test + 17 config_lazy + existing + doctest) |
| Clippy | `cargo clippy -p chimera-cli --all-targets -- -D warnings` | exit 0,零警告 |
| Fmt | `cargo fmt -p chimera-cli -- --check` | exit 0,零 diff |

### 4.2 workspace 级验证

由主控代理统一执行(workspace 级 `cargo test --workspace` / `cargo clippy --workspace` / `cargo fmt --all --check`)。

---

## 5. 设计决策摘要

### 5.1 用 `std::sync::OnceLock` 而非 `once_cell` crate

**决策**:使用 Rust 1.70+ 标准库 `std::sync::OnceLock`,不引入 `once_cell` 外部依赖。

**理由**:
- 零新增依赖,契合 crate 级 `#![forbid(unsafe_code)]` 哲学
- `OnceLock` 是标准库稳定 API,无 unsafe
- 项目 toolchain 已是 stable,`OnceLock` 可用

### 5.2 错误缓存(`Result<T, String>` 而非 `Option<T>`)

**决策**:`LazySection` 缓存 `Result<T, String>`,错误也缓存。

**理由**:配置文件格式错误不会因重试自愈,缓存错误既避免重复解析坏 section,也保证"懒加载只算一次"的语义一致。`get_or_init` 接受 `FnOnce() -> Result<T, String>`,错误转为 `anyhow::Error` 返回。

### 5.3 `Figment::extract_inner` 按 section 提取

**决策**:用 `Figment::extract_inner::<T>(key_path)` 按 section 路径提取,而非全量 `extract::<ChimeraConfig>()`。

**理由**:
- `extract_inner` 按点分路径从合并 provider 树提取子树反序列化
- 实现真正 section 级惰性求值:未访问 section 零解析开销
- 已验证 `extract_inner` 与 `extract` 对同一 provider 链产出等价值

### 5.4 `LazySection<T>` 辅助类型而非裸 `OnceLock`

**决策**:封装 `LazySection<T>` 辅助类型,而非每个 section 直接用 `OnceLock<T>`。

**理由**:
- 统一 "fallible 初始化 + 错误缓存" 模式
- 14 个 getter 各缩为一行(`self.<field>.get_or_try_init(|| extract_section(...))`)
- 避免样板重复(14 × 5 行 = 70 行样板 vs 14 × 1 行 = 14 行)

### 5.5 保留 `figment` 引用而非 extract 后丢弃

**决策**:`LazyConfig` 持有 `Figment` provider 引用,14 个 getter 在首次访问时从同一 provider 按路径取子树。

**理由**:14 个 getter 需在各自首次访问时从同一 provider 按路径取子树,必须长期持有 Figment。若 extract 后丢弃,后续 getter 无法解析。

---

## 6. 已知限制

1. **无 `PartialEq` 直接比对**:14 个 section 类型未派生 `PartialEq`(定义在 nexus-core,RC 阶段不修改核心类型),用 JSON 字符串比对替代。此策略与现有 `test_default_config_roundtrip` 一致。
2. **`to_chimera_config` 克隆开销**:聚合方法 clone 14 个 section。仅用于需要完整配置的场景(如 `aether config dump`),非常规 CLI 启动路径。
3. **懒加载性能未实测**:理论上,按需解析避免启动期未使用 section 的解析开销。实际延迟降低需 bench 测量,但 CLI 启动路径复杂,bench 价值有限。

---

## 7. 关联文档

- Spec:`.trae/specs/v1-2-0-omega-deferred-optimization/spec.md`
- Tasks:`.trae/specs/v1-2-0-omega-deferred-optimization/tasks.md`(Task 4)
- Checklist:`.trae/specs/v1-2-0-omega-deferred-optimization/checklist.md`(Task 4)
- CHANGELOG:`CHANGELOG.md`(v1.2.0 Task 4 章节)
- 项目记忆:`project_memory.md`(v1.2.0-omega Task 4 教训)
