# ADR-167: 发布门禁第 8 项（cargo audit）新暴露 4 条告警的分级处置

* 状态：**Proposed（待架构委员会终批）** — 起草 2026-08-31（Phase R R1-T7 执行中）
* 触发：按发布检查单第 8 项实跑 `cargo audit --deny unmaintained --deny unsound --ignore ×5`（与
  `audit.yml:61` 逐字一致），结果 **EXIT=1**：`3 vulnerabilities + 1 denied warning(unsound) + 1 allowed warning(yanked)`
* 关联：`audit.yml`、`.claude/CLAUDE.md` §2/§5 第 8 项、`agents.md §7.2` 第 3 项、ADR-166（同族：声明与执行面一致性）

## 背景与实测事实（全部可复算）

复算命令：`cargo audit --json --deny unmaintained --deny unsound --ignore ×5` → `scripts/audit_patched_ranges.py`。
可达性判定：`cargo tree --workspace --all-features --target all -i <crate> -e normal,build,dev`
（**必须带 `--workspace --all-features --target all`**；只跑 `cargo tree -i` 会得到 "nothing to print"
的假阴性，本 ADR 起草过程中就差点据此把 wasmtime 误判为"不在图中"）。

| 告警 | 版本 | 官方 patched 区间 | 真实链路 | 装配面 | 可否 semver 内修 |
|---|---|---|---|---|---|
| RUSTSEC-2026-0258 `h2` unbounded empty DATA frames | 0.4.15 | `>=0.4.16` | `h2 ← hyper ← axum ← mca-gateway` **的 dev-dependencies** | 否（测试面） | ✅ 已修（升至 0.4.19） |
| RUSTSEC-2026-0268 wasmtime guest 控宿主堆分配 | 47.0.3 | `>=47.0.4`（或 `>=46.0.3,<47`） | `wasmtime ← seccore`，但 seccore 将其挂在 **`wasm-sandbox` optional feature** 后（`wasm-sandbox = ["dep:wasmtime"]`） | **默认构建不含**；仅 `--features wasm-sandbox` 时进入 | ✅ 已修（升至 47.0.4） |
| RUSTSEC-2026-0269 wasmtime **文件系统沙箱逃逸**（尾斜杠路径/符号链接） | 47.0.3 | `>=47.0.4` | 同上 | 同上 | ✅ 已修 |
| RUSTSEC-2026-0253 `lru` `LruCache::pop()` panic 路径 use-after-free（unsound） | 0.12.5 | **`>=0.18.2`** | `lru ← ratatui 0.29 ← chimera-tui ← chimera-cli` | **是** | ❌ **不可**：ratatui 要求 `lru ^0.12`，0.18.2 越界 |
| chacha20 0.10.1 **yanked**（非漏洞，allowed warning） | 0.10.1 | — | 全 feature/target 组合下均不在解析图中（lock 残留） | 否 | `cargo update` 时顺带收敛 |

关键代码事实（决定 lru 的暴露面，非推测；2026-08-31 全部由工具复测，脚本 `scripts/lru_facts.py`、
`scripts/lru_lock_facts.py`、`scripts/lru_reachability.py`、`scripts/lru_evict_chain.py`）：

* **引入链唯一**：`cargo tree -i lru` ⇒ `lru 0.12.5 ← ratatui 0.29.0 ← chimera-tui ← chimera-cli`；
  本仓无 crate 直接声明 `lru`（`cargo metadata` 核对）。
* **无 semver 修法有硬证据**：`ratatui 0.29.0` 对 lru 的要求是 **`^0.12.0`**（即 `>=0.12.0, <0.13.0`），
  而 patched 是 `>=0.18.2` ⇒ 在不改 ratatui 的前提下**不可能**通过 `cargo update` 脱险（不是"我没找到"，是区间不相交）。
* **锁内只有一个 lru 版本**：`Cargo.lock` 解析结果 = `['0.12.5']` ⇒ 豁免面是**单一版本**，
  `tracing-log` 那条 `lru ^0.7.7` 是 optional 且**未被启用**，故不在锁里（若将来被启用需重开本 ADR）。
* **使用点唯一**：整个 ratatui crate 只有 `src/layout/layout.rs` 引用 `lru::LruCache`
  （`type Cache = LruCache<(Rect, Layout), (Segments, Spacers)>`，即布局缓存）。
  该文件对缓存实际调用的方法是 **`get_or_insert()`（L657）与 `resize()`（L284）**（测试里另有 `cap()`）；
  该文件内 `.pop(` 出现 **0** 次。
* 告警点名的 `LruCache::pop()`（`lru-0.12.5/src/lib.rs:1109`，内部 `Box::from_raw` + `drop_in_place`）
  在 ratatui 中**无调用点**。

**但"不可达"必须限定范围（这一条修正本节早期版本可能给人的过强印象）**：
`get_or_insert` 的插入/逐出走 `try_get_or_insert → replace_or_create_node`（`lib.rs:387`），
其中同样有 `unsafe`、`MaybeUninit` 的 `mem::replace`，并且**与 `pop()` 共用 `detach()`**（`lib.rs:1459`）。
因此准确表述是：**本告警点名的具体 API 不在我们的调用路径上**，而**不是**"该 crate 的 panic 不安全类别与我们无关"。
故方案 A 只能是**带到期条件的债务**（下节已按此收紧措辞），不得写成"无风险豁免"。

## 已执行部分（2026-08-31，本节为事实而非计划）

* `cargo update -p h2` → **0.4.15 → 0.4.19**；
* `cargo update 'wasmtime@47.0.3' --precise 47.0.4` → **47.0.4**。
  注：单用 `cargo update -p wasmtime` **不够**——wasmtime 只在 optional 边上，不在活跃解析图中，
  该命令只动了 cranelift 家族而把 wasmtime 留在 47.0.3；必须用 **`包@版本` 显式 spec**。
* 升级后复核：`cargo check -p seccore --features wasm-sandbox` **EXIT=0**（47.0.4 未破坏 API）；
  `cargo check --manifest-path fuzz/Cargo.toml` **EXIT=0**。
* 重跑 `cargo audit`（与 CI 逐字一致的旗标）：**3 条 vulnerability 全部消除**，
  仅剩 `lru` 的 denied unsound warning 与 `chacha20` 的 allowed yanked warning。`EXIT` 仍为 1。
* 因此本 ADR 的**待裁事项收敛为一项**：lru 是否采纳方案 A（带证据的临时豁免）。

## 决策

1. **wasmtime 升级至 `>=47.0.4`，不得 ignore**（已执行，见上节）。理由：它服务于 `seccore` 的沙箱能力，
   而 RUSTSEC-2026-0269 恰是“沙箱逃逸”——**与安全层核心承诺直接冲突**，即使默认构建不含该特性也不能留。
   用“ignore 一条沙箱逃逸告警”来让门禁变绿，是本项目最该拒绝的那种绿。
   事实订正：本 ADR 初稿曾把它写成“生产链路上”，漏看了 `wasm-sandbox` 是 optional；
   现已按 `cargo tree -e normal`（默认图内无 wasmtime）与 `--all-features`（有）两组实测改写。
2. **h2 升级至 `>=0.4.16`**（已执行→ 0.4.19）。虽仅经 `axum` 的 dev 边进入，但修法零成本，
   没有"留着的理由"。
3. **lru 不做静默 ignore，也不为凑绿而升级 ratatui**。三条候选：
   * **A（推荐，本波次执行）**：带证据的**临时豁免**（注意：是"有到期条件的债务"，不是"无风险"，
     理由见上节"但不可达必须限定范围"）——把 RUSTSEC-2026-0253 加入 `audit.yml` 与两份规则
     文档的 `--ignore` 列表，并**逐字写明**豁免依据（无 semver 兼容修法 + `pop()` 在 ratatui 0.29.0 无调用点
     + lru 不在本仓直接声明），同时登记为**有到期条件的债务**：ratatui 升到 `lru >= 0.18.2` 依赖后立即撤销豁免。
     与本仓既有 5 条 ignore 的"经评估确认不影响项目"口径同类，但**必须附可达性证据**（旧 5 条恰恰缺这层）。
   * **B（独立波次）**：升级 `ratatui 0.29 → 0.30.2`（0.x 下为破坏性变更，涉及 chimera-tui 27 面板与
     `REGISTERED_FOCUS_ORDER` 四处断言同步），升级时**验证** 0.30.2 是否真的依赖 `lru >= 0.18.2`
     ——不验证就写进计划，等于把别人的版本号当自己的证据。
   * **C（拒绝）**：`[patch]` 强行把 lru 顶到 0.18.x ——会破坏 ratatui 的 API 兼容假设，且把供应链风险
     变成自己的维护责任。
4. **规则同步硬约束**：`audit.yml` 与本项 ignore 列表的任何变更必须同时改 `audit.yml`、
   `.claude/CLAUDE.md` §2 与 §5 第 8 项、`agents.md §7.2` 第 3 项——**四处逐字一致**。
   本轮就发现过这类漂移（P5-T5 已把 `--deny warnings` 改为 `--deny unmaintained --deny unsound`，
   而两份规则文档仍写旧值），已随 ADR-166 一并订正。

## 备选项与否决理由

* **整体放宽第 8 项（去掉 `--deny unsound`）让门禁变绿**：否决。这与"stress 通道用
  `continue-on-error` 兜脏"、"阈值从 >5× 一路放宽到 >2×"是同一个失败模式：把判据调到能过，
  而不是把问题修掉。
* **把 4 条一起 ignore**：否决。其中 3 条可修（h2、wasmtime×2），ignore 可修项就是伪装安全。

## 后果

正面：发布门禁的结论从"过/不过"升级为"哪些真修了、哪些带证据豁免、豁免何时到期"；
wasmtime 两条（含沙箱逃逸）在装配面上被真实消除。

代价：第 8 项在 A 方案落地前保持 **EXIT=1**，即 **v2.28.0 不得打 tag**——这是有意的。
若委员会选 B，则 lru 豁免随 ratatui 升级波次撤销。

## 验证

* **lru 可达性证据链已可复跑（2026-08-31 本轮）**：`scripts/lru_facts.py`（`cargo metadata` 给出 ratatui → `lru ^0.12.0` 硬约束）、
  `scripts/lru_lock_facts.py`（锁内 lru 版本集 = `['0.12.5']` 单版本）、`scripts/lru_reachability.py`（`lib.rs` 内 78 处 unsafe、`push → capturing_put`）、
  `scripts/lru_evict_chain.py`（逐出走 `replace_or_create_node` 而非 `pop_lru`；`get_or_insert`/`resize` 为 ratatui 实际调用点，该文件 `.pop(` = 0）。
  这四个脚本共同支撑上节两个结论：“无 semver 修法”与“告警点名的 API 不可达但共用 `detach()`”——裁决时应依赖该证据链而非本文叙述。

* **已机械实现（2026-08-31）**：`scripts/audit_cmd_sync.py` 单文件单一实现，比对三处
  （`audit.yml` / `AGENTS.md §7.2` / `.claude/CLAUDE.md`）的 `cargo audit` 命令，
  归一化后比较 deny 旗标集与 ignore 集。实测当前 `[PASS] 3 处逐字等价`（exit 0）。
* **它自己的两组阴性对照已跑（`scripts/audit_sync_negative_control.py`）**：
  基线一致 → 0；**仅篡改一处**（模拟“改了 CI 忘改文档”）→ 1；文档不存在（CI 视角）→ **2（不可判定）**。
  第三条是有意设计：`.md` 不入库，若“没文件”当成“通过”，就是再造一个永远为真的假门禁。
* 升级后复核：`cargo check -p seccore --features wasm-sandbox` 与 `cargo check --manifest-path fuzz/Cargo.toml` 均 EXIT=0。
* 若采纳方案 A：改 `audit.yml` 后 `python scripts/audit_cmd_sync.py` 必须仍返回 0（三处同步），否则不得合入。
* 其余：`cargo tree --workspace --all-features --target all -i wasmtime` 现显 47.0.4；
  重跑 `cargo audit` 的 `vulnerabilities.list` 已不含 h2 / wasmtime（实测 3 条→ 0 条）。
