# ADR-212: L0 契约层重构裁决 —— 保持单 crate + 纯度机器门（G-99）+ 例外登记表单测，否决拆分，落地子模块化（W3）

> **Status**: ✅ Accepted (2026-10-12)
> **决策日期**: 2026-10-12
> **关联 ADR**: ADR-033（L0 建立 + 追加节纯度自陈缺口）/ ADR-049（决策 1「0 新 crate」）/ ADR-054（决策 3/6 类型上提）/ ADR-187（Check A 退役）/ ADR-193（架构减法）/ ADR-195（GPU 否决）
> **关联交付**: `docs/reports/L0-contracts-refactor-audit_2026-10-12.md`（事实基线 / 冲突裁决 / 十维审计）
> **权威源**: 代码实测（`crates/nexus-contracts/`、`scripts/check_dependency_rules.ps1`、`cargo tree`）；两份设计文档仅作意图参照

---

## 1. 背景

L0 层专项重构任务启动前，两份权威文档对「L0 是什么」给出**互斥**定义，且现行代码已把 ADR-033 的「零逻辑」口径订正为「受控纯逻辑」但**无机器门**（ADR-033 追加节 2026-10-09 自陈）。为避免照单施工，先建立事实基线并裁决冲突：

- **代码实测（一手证据）**：
  - L0 = 仅 `nexus-contracts`（`check_dependency_rules.ps1` `$layerMap['nexus-contracts']=0`；`nexus-core`/`event-bus`=L1）。
  - 生产依赖仅 `serde/chrono/uuid/thiserror`（`cargo tree -e normal` 零 workspace 内部依赖）。
  - **53 功能模块 + lib.rs = 54 源文件**；**34 个生产消费 crate**（`[dependencies]` 口径；另 2 个仅 dev）。
  - 红线现状：无 `unsafe`/无 `async`/无 `.await`/无 `serde_json::Value`（用 `Box<str>`）/无 `regex`/f32 仅 `PartialEq`。
  - 测试基线：747 unit + 全部集成测试 0 失败（含在途 doc 订正）。

- **文档冲突**：
  - `十六源手册 v3.0 §8.1`：L0 = `nexus-core`（并发原语/UCL/GpuComputePool）+ 新 `nexus-telemetry`，置于 52/53 crate 的 GPU 异构愿景内。
  - `v3.4.0 §5`：L0 = `nexus-contracts`（与代码一致），但提议 6 个 `*-contracts` 兄弟 crate（"1+6=7"）。

## 2. 决策

### 决策 1：L0 边界 = 仅 `nexus-contracts`（单 crate）
- 采信代码 + v3.4.0 §5；**否决**手册 §8.1 的「L0=nexus-core + nexus-telemetry」重定义（与现行分层矛盾；`nexus-telemetry` 不存在；其 GPU 面被 ADR-195 否决）。
- 手册 §8.1 相关内容登记为**归档蓝图**，不入实施范围。

### 决策 2：不拆分 L0 为多 crate（维持 ADR-049 决策 1）
- **采纳**：保持单契约 crate。
- **否决反方案 A（拆 6 个 `*-contracts` crate）**：①违反 ADR-049 决策 1「0 新 crate + 既有 crate 子模块」；②与 ADR-193「架构减法」（41→39）逆向；③爆炸半径 34 生产消费 crate + workspace 编译图失控；④「契约根」拆成多 crate 后，同为 L0 的兄弟 crate 互引会使 `L(0)→L(M)` 禁止铁律语义模糊、校验复杂；⑤零 API/行为收益。
- **重启条件**：出现明确的独立演进节奏或团队所有权边界证据，且须先立 **supersede ADR-049** 的新 ADR。
- 证据：v3.4.0 §5 自身「落地方案」列即写「`nexus-contracts` 新增模块」，6 兄弟 crate 从未建，反证该拆法未被采纳。

### 决策 3：落地「纯度机器门」G-99，补 ADR-033 追加节自陈缺口
- 新增 `scripts/check_l0_purity.py`（G-99 真实扫描 + G-100 双向 selftest），已入 `ci.yml` check job 与 `gate_manifest.toml`。
- 双判据：
  1. `[dependencies]` ⊆ `{serde, chrono, uuid, thiserror}`（dev-deps 不校，与 ADR-033 一致）——从**依赖可达性**上封死 IO/async/网络/随机 crate 进入 L0。
  2. 逐 `src/*.rs` 先以 Rust 词法器 blank 行注释/可嵌套块注释/普通与 raw 字符串，再对**残余代码体**扫禁用符号 `unsafe`/`async`/`.await`/`tokio`/`std::fs`/`std::net`/`std::io`/`std::process`/`TcpStream`/`rand::`/`getrandom`/`println!` 等。
- **词法器 blank 是门成立的关键**：naive 子串扫描会误判 `forbid(unsafe_code)`、doc 注释里提及的 `tokio::spawn`、红线枚举串 `"redline-subscribe-before-spawn"`、`base_url` 里的 `"https://...//v4"`——这正是 ADR-033 追加节「判有多少逻辑前先确认计数口径」教训的机器化。
- 时间（`chrono::Utc::now`）**故意不列禁用**：ADR-033 红线清单为 IO/async/网络/随机，时间戳构造器属可接受的基础构造器。
- 现读基线：54 源文件 / 0 违规 / 依赖白名单 4 项；selftest 双向（CLEAN 0 误判 + DIRTY 逐条必抓）全绿。
- **例外登记表单测（补 §5 的另一半机器判据）**：lib.rs 新增 `pub const ADR_033_EXCEPTION_MODULES`（唯一机器可读例外源）+ `tests/l0_exception_registry.rs`（4 测）交叉校验：① 每个登记名在 crate 根有对应 `pub use <group>::<leaf>;` 声明；② 五个明确例外恒在册；③ W3 的 10 个组声明存在（结构锁）；④ 去重 + snake_case 卫生。

### 决策 4：不建 L0 纯度门的双/三源孪生（.ps1/.sh）
- 单 `.py` 源，CI 直调 `python3 scripts/check_l0_purity.py`，沿 `check_layer_map_parity.py`/`check_accel_exclusion.py`/`check_ps1_bom_gate.py` 现代先例（这些门均由 CI 直接跑 `.py`）。
- **理由**：双源手写正是 `check_layer_map_parity` 要治理的漂移病灶；单一权威实现更符合门自身的一致性纪律。
- **登记例外**：本门与「铁律审计须 .sh/.ps1 双源」不同——铁律门有 Windows 本地直跑诉求（`check_dependency_rules.ps1`），纯度门为 CI/收口用的静态扫描，无 Windows-only 语义，故单源无漂移暴露面。

### 决策 5：W3 子模块树物理重排（本轮已实施）
- **实施**：`git mv` 将 53 个平铺模块收入 10 个内聚子目录（`src/sparse/`、`spec/`、`seams/`、`core_domain/`、`memory/`、`safety/`、`mca/`、`vector_math/`、`protocol/`、`testutil/`），每组一个 `mod.rs` 声明其子模块；`core_domain` 组名避开 leaf `domain` 的根绑定冲突。
- **facade 保持手段**：lib.rs 将 53 个 `pub mod <leaf>;` 声明改为 `pub use <group>::<leaf>;`（re-export），上方 `///` 原注释转为对 re-export 的文档；facade 的 `pub use crate::<leaf>::{items}` 与 `prelude` **逐字不变**（因 `crate::<leaf>` 经根 re-export 仍解析）。内部 5 处 `crate::<leaf>` 交叉引用同理不变。
- **双路径**：`nexus_contracts::<leaf>::Type`（旧，34 消费方依赖）与 `nexus_contracts::<group>::<leaf>::Type`（新，分组）同时有效；type identity 相同（同一模块两个名字）。
- **验证**：`cargo check --workspace` 39/39 无错；`cargo test -p nexus-contracts` 与 W0 基线一致（747 unit + 全部集成 + 58 doctests，0 失败）；`cargo clippy --all-targets -D warnings` 与 `cargo fmt --check` 绿。
- **W3 意外收获（可追溯）**：嵌套后 leaf 模块经 `crate::<leaf>`（re-export）与 `crate::<group>::<leaf>` 两路可达，`safety/errors.rs` 的 `//!` 内 `- [`Type`]:` 列表项 shortcut 链接解析上下文改变，触发 rustdoc lint「link reference defined in list item」——按 rustdoc 建议将三处改为 `- [`Type`][]:` 空引用形式（纯文档，零 API 变更）。此副作用登记备查：re-export 双路径会影响 intra-doc shortcut 链接解析。

## 3. 影响

| 维度 | 影响 |
|---|---|
| 代码 | W3 落地：53 平铺模块 → 10 内聚子目录（`src/<group>/` + `mod.rs`），crate 根 re-export 保 facade 逐字不变；新增 `scripts/check_l0_purity.py` + lib.rs `ADR_033_EXCEPTION_MODULES` const + `tests/l0_exception_registry.rs`。无 crate/公共 API/依赖方向/行为变更 |
| CI | `ci.yml` check job 增 1 步（selftest + 真实扫描），零编译秒级 |
| 门 | `gate_manifest.toml` 新增 G-99（真实）/ G-100（夹具） |
| crate 数 | 不变（39）；L0 保持单 crate |
| 消费方 | 零改动（34 生产消费 crate 不受影响） |
| 兼容性 | 100% 向后兼容，无破坏性变更 |

## 4. 验收判据（对齐发布前清单）
- `python scripts/check_l0_purity.py --selftest` 且 `python scripts/check_l0_purity.py` 均 EXIT=0
- `pwsh scripts/check_dependency_rules.ps1`（含 --selftest）EXIT=0
- `cargo check --workspace` 39/39；`cargo clippy -p nexus-contracts --all-targets -- -D warnings` 0 警告；`cargo fmt --all -- --check` EXIT=0
- `cargo test -p nexus-contracts` passed 不回退（基线 747 unit + 集成，0 失败）
- `py -3 scripts/adr_index_sync.py` EXIT=0（新增 ADR-212 物理文件 + 本表单编号行成对，不产生 UNINDEXED）
- 判据：无功能退化 / 无性能劣化（`xts_top_k`/`util_micro` bench 复跑）/ 无新增 unsafe / 无公共 API 不当暴露

## 5. 回滚
- G-99/G-100 为新增独立门：`git revert` 对应提交即从 CI 与 manifest 移除，不影响 crate 编译或既有门。
- 本 ADR 与审计台账为文档，可独立回退；不涉及任何代码行为面。

## 6. 相关
- 审计与冲突裁决台账：`docs/reports/L0-contracts-refactor-audit_2026-10-12.md`
- 纯度门实现：`scripts/check_l0_purity.py`
- 承接：ADR-033 追加节（纯度缺口的登记）→ 本 ADR 的机器化收口

---

## 7. 深度逐模块复审与整改（2026-10-13，v3.5 W1 续）

按用户选定的「深度逐模块（53）十维穷举复审」，两路只读审计子代理产出台账后，本轮已落地的整改：

### 7.1 已修复（均经门禁实测）
- **G-44 入口契约**：`check_l0_purity.py` 的 `__main__` 缺 `gate_rc.run` 崩溃→退 2 护栏（F37），已补齐。
- **红线 #8（recall_fusion）**：`fuse_ranked_lists` 改调 `util::xts_top_k_by`（select_nth_unstable），消除 sort-then-truncate；测名同步。
- **敏感泄露（redact）**：`redact` 的 Redact 分支此前只掩高熵 token，`password=hunter2`/`password hunter2short` 等低熵密钥值原样透传；现按类别掩 `SecretKeyValue` 值 + 补回归测。
- **例外登记表名实不符（B-P1-6/7）**：`ADR_033_EXCEPTION_MODULES` 13→17（补 `skill_lifecycle`/`tool_plan`/`errors`/`command_validation`，其中两项自声明例外却未登记）；`tests/l0_exception_registry.rs` 由单向改为**双向相等锁**。
- **ImmutableSurface Critical 覆盖（B-P1-5）**：Critical 事件面 6→17（纯增量补齐红线 #6「mpsc 旁路=16」此前缺的 11 个），`all()`/`ALL_CONST`/`category`/`as_str` + gsoe 两处转发签名同 20→31。
- **文档口径（§3 P2）**：lib.rs 头「零逻辑/唯一 serde/54 源文件」、`Cargo.toml` desc、`harness_spec` 红线条数(§5/14)+`types.rs:1158`行号+thiserror 自相矛盾、`wire_roundtrip` 10→9 组、`event_payload` 幽灵 `From` 声明 → 全部订正。
- **性能实测**：`cargo bench -p nexus-contracts` 本机 release 跑通（xts_top_k n=10k 30µs vs 全排 180µs=6.0×；n=100k 355µs vs 2.6ms=7.3×；util_micro 零分配断言通过），证 recall_fusion→xts_top_k_by 无性能劣化。

### 7.2 W3 文档blast radius 收口（物理搬移引入的路径漂移）
全量 `run_gate_manifest` 暴露 W3 物理搬移使 **path-keyed 产物**漂移，本轮已修：G-70 派生视图（`emit_architecture_views --write`）、G-72 活文档 `path:line` 锚（`.qoder/rules/chimera.md`/`agents.md`/`CODE_WIKI`/v2.30 leaf→组路径）、G-97 v3.5 总账附录 4A 证据路径、G-01/G-02 系 topk/formal ledger 基线。

### 7.3 残留（非代码缺陷，需提交/跨库处置）
- **G-07 commit-coupling**（已清）：已跟踪的 `gate_manifest.toml`/`ci.yml`/`adr_index.md` 引用本批新文件；`git add` 新交付文件（`check_l0_purity.py`/10 `mod.rs`/`l0_exception_registry.rs`/`ADR-212`）使 Z 被跟踪后 G-07 EXIT 0（仅 stage，非 commit，可 `git reset` 回退）。
- **G-16 spec-coverage**：外部缓存 `Phase_R_*.md` spec 仍指 `crates/nexus-contracts/src/util.rs`（已入 vector_math/）。该 spec 在仓外（Qoder 计划缓存），不宜在仓内代改。

### 7.4 关键发现（供治理）
深度复审 + 全量门实证的代价：**W3 物理文件搬移的 blast radius 远超「facade 兼容」预期**（跨仓内外文档/gate 输入/提交耦合）。若仅求「命名空间内聚」而不要磁盘分组，可改为 **lib.rs 内分组 re-export（不移动文件）**，零路径漂移。此为 §5 降级判据的实证支撑；是否保留物理搬移（继续 stage+提交以消 G-07/G-16）抑或回退物理搬移而保留全部 7.1 修复，留业主裁决。

### 7.5 仍待裁决（D-6/D-7/D-9/D-10，跨层或破坏性，本轮未动）
- D-6 `Checkpoint`/`ExperienceCard` 等持久化契约加 `schema_version` + positional→named 编码（触 L9 序列化，破坏 wire，需 ADR/分波）。
- D-7 `masks::select_top_k` 归位（迁 L6 扩展 trait vs 立 ADR-213 修订 ADR-033 决策 3）。
- D-9 `token_evidence::Titofallback` 更名（wire 面，需 ADR + serde alias）。
- D-10 `to_vec`→`to_vec_named` 全库 27 处分波。
