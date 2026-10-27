# ADR-226: loom 作为 dev-dependency 的四项最小性论证与 E1 前置实验闸门（R2 T-2 落地）

> **Status**: ✅ Accepted（2026-10-26，业主在 R2 第 4 轮补裁 T-2「允许 loom，但**仅限 dev-dependency**」；本 ADR 是该裁决的授权与约束面，并设 **E1 硬前置**：E1 未过则 §3 D1 的落点不得实施，自动转入 D3 的「作废愿景行」路径）
> **决策日期**: 2026-10-26
> **关联 ADR**: ADR-222 附录 §9（「L0-B 新增外部依赖必须另立 ADR 并给四项论证」的流程来源；本批按同一纪律**自愿**适用于 L2，理由见 §1.4）/ ADR-225 T-2 与 D9（业主裁决归档与愿景行处理纪律）/ ADR-224 议程 M13（loom 零落地三态，本 ADR 是其收口件）/ ADR-033 与 G-99（L0-A 依赖白名单，本批不触碰）/ ADR-198（PS parity 先例：门的执行面必须可验证）
> **关联交付**: `docs/reports/l0l10-r2-full-recon-2026-10-26/03-重构技术方案.md` §3 的 C-93 行 + 登记册 M13（R1）/ M69 同族的三态收口法
> **编号合法性**【亲验 2026-10-26】: 落盘前现取 `docs/architecture/ADR-*.md` 物理文件 **120** 份、磁盘主编号 max **225**；`ADR-226` 在 `docs/architecture/*.md,agents.md,CONTRIBUTING.md,scripts/gate_manifest.toml` 四处 **0 命中**。变更单占用到 **C-92**，本件承载 **C-93**（`C-93..C-99` 现取 0 命中，见 `tmp/r2_cid_space.txt`）
> **权威源**: 本 ADR 内全部 file:line 与命令读数为落盘时现取；后续批次引用须重跑，不得继承计数

---

## 1. 背景：四条实测，一条被订正的旧结论

### 1.1 R1 留下的三态判据本身证据不足
R1 登记册 `tmp/l0l10-r1-issues.md:96-98` 记 M13「loom 并发正确性验证零落地（三态盘点）」，其「注释性理由」为「Windows GNU loom 不可用（`bus.rs:3513`、`credit_flow.rs:400`、`shard.rs:1011`、…）」。R2 逐指针复测【亲验】：

| R1 给的指针 | 现势实测 | 判定 |
|---|---|---|
| `credit_flow.rs:400`（R1 未限定 crate） | 真路径 = `crates/event-bus/src/credit_flow.rs:400`；该处确在讲 loom | 可定位但**引用欠精确**（全仓同名文件仅此一份，未致错） |
| `bus.rs:3513` | `bus.rs:3513` 现是 `EventSeverity::Critical` 断言行；loom 那句实在 `bus.rs:3536` | 行号漂 23 行 |
| `shard.rs:1011` | 原文是「不引入 loom；以真并发线程 + barrier 压 crossbeam ArrayQueue 的 MPSC 语义」⇒ **并未主张工具链不可用** | 归因错（把「决定不用」读成「用不了」） |
| `nexus-core/src/compute/bridge.rs:665-666` | 确在讲「stable-x86_64-pc-windows-gnu … Windows 支持受限」 | 成立（但措辞是「受限」而非「失败」） |

⇒ 结论方向（loom 未落地）成立且可复现，但**支撑它的四条证据指针中两条不可直接继承**。这正是 ADR-225 D8 附带结论要求现场重跑的那一类。

### 1.2 盘上的断言面是 5 处，且理由互相不一致
`grep -rn loom crates/**/src` 现取 6 命中，其中 5 处是「为什么不用 loom」的散文断言：

| 站点 | 原文理由 | 可证伪性 |
|---|---|---|
| `crates/event-bus/src/credit_flow.rs:400-402` | 「loom 需 **nightly-only** 特性，在 Windows GNU 工具链下编译失败」 | **已被元数据否证**【亲验】：`cargo info loom` → `version: 0.7.2` / **`rust-version: 1.65`**（本仓工具链为 1.97.1 stable）⇒ 「需 nightly-only」不成立。后半句（Windows GNU 编译失败）仍待 E1-a′ 判定——真正平台敏感的也不是 loom 本体而是它的依赖 `generator`（见 ③） |
| `crates/event-bus/src/bus.rs:3536` | 「Windows GNU 工具链 loom 编译失败」 | 未附版本、未附命令输出 |
| `crates/nexus-core/src/compute/bridge.rs:665-666` | 「项目 .toolchain 为 stable-x86_64-pc-windows-gnu，loom 依赖平台线程模型细节，Windows 支持受限」 | 措辞已弱化为「受限」，非「失败」 |
| `crates/nexus-core/src/compute/hts.rs:829` | 「同 T8 理由：Windows GNU 工具链 loom 受限」 | 二手转述 |
| `crates/event-bus/src/shard.rs:1011` | 「不引入 loom」+ 以真并发 barrier 替代 | 与工具链无关，是取舍 |

⇒ 本 ADR **不采信任何一侧**（既不采信「不可用」，也不采信 R1/R2 自己「应当落 loom」的处方），改为 §3 D2 的实验判定。

### 1.3 环境事实（离线可行性）
- `CARGO_HOME = D:\Chimera CLI\.toolchain\cargo`；`registry/cache/index.crates.io-*` 现取 **2041** 个 crate，**无 loom**（`registry/src/*` 646 项亦无）；无 `vendor/`；`Cargo.lock` 内 loom **0 命中**。
- `cargo search loom` → `loom = "0.7.2"`，且命令先打印 `Blocking waiting for file lock on package cache` ⇒ 注册表网络可达，但**包缓存锁与在飞的 workspace 构建互斥**（本批 release 压测档正持锁）。
- `.cargo/config.toml:33 jobs = 4` 的 WHY 注释（同文件 `:25-32`）是 ADR-225 D1「不改 config、改命令面纪律」的依据，本 ADR 沿用。

### 1.4 为什么把 L0-B 的论证流程自愿用到 L2
ADR-222 附录 §9 的「适用范围」原文（`:214`）只覆盖 L0-B 三个 crate，L2 加一个 dev-dep 并**不受**其字面约束（G-99 只判 `[dependencies]`，见 ADR-222 `:180` 的同族注记）。本批仍走四项论证，理由不是合规表演，而是：本仓反复出现的病灶是「散文承诺 + 零机器判据」（ADR-225 D2/D6），一个未经论证的新依赖恰是该病灶的入口形态。

### 1.5 本仓已有一笔「Linux CI 承接」的债，且无载体（这才是 E1 定在 Linux 的理由）
R1 引的 `docs/reports/phase1-wave0-1-closure.md` 三行现取【亲验；该报告 git-ignored、未入库，故按 D5 方案乙在此整行引用】：

- `:61`「`| loom L-01~L-03 | 通过 | 回退多线程竞争测试（Windows GNU loom 不可用，已说明） | 🔶 记录替代方案 |`」
- `:94`「Windows GNU 工具链下 loom 不可用（多线程竞争测试替代已登记）」
- `:101`「`| loom 9 场景 | 🔶 Windows GNU 回退多线程竞争测试（已登记，Linux CI 承接） |`」

而 `grep -rn 'loom' .github/` 现取 **0 命中** ⇒ 「Linux CI 承接」是一句**零载体的承诺**，与 M19「六条 `[[slo]]` 无 CI 载体」同族。本 ADR 的 E1 因此**定在 Linux runner**：既然仓内既定决策就是「本地因工具链限制不跑，由 Linux CI 承接」，那么要回答的不是「windows-gnu 能不能编 loom」，而是「承诺的那条 CI 腿能不能真跑起来」。**同时不推翻也不采信**那三行里的「不可用」——它仍是一个未附版本与命令输出的断言，E1 的 windows 腿（D2-E1-a′）会顺手把它的真伪定下来。

## 2. 四项论证（按 ADR-222 附录 §9 的 ①②③④ 顺序）

### ① 最小性：现有手段为何不够
- dev-dependency 全景现取（46 个 crate 的 `[dev-dependencies]` 去重）：与并发相关者只有 `proptest`（45 crate）、`tokio-test`（2）、`criterion`（35）、`tempfile`（15）、`futures`（1）、`tracing-test`；**无 loom、无 shuttle、无 serial**。
- 能力差集：`proptest` 生成随机**输入**，不枚举线程**交错**；`tokio-test` 控制时间与调度，但一次执行只走一条序；盘上已有的「8 线程 + barrier」测试（`credit_flow.rs:402`、`shard.rs:1011`、`bridge.rs:663`、`hts.rs:827`）是**单次抽样的真并发压测**——它能证伪「丢数据」，不能证明「任意交错下不丢不重」这类全称命题。
- ⇒ 对 CAS/锁回路的 permutation 正确性，现有面无覆盖，loom 有不可替代的作用域。**但该作用域仅限纯同步核心**（见 ②）。
- 附带结论：5 处 barrier 替代测试**保留不动**。它们锁的是吞吐/守恒，与 permutation 是两个属性；落 loom 不等于退役它们。

### ② 层向：落点与一处工具-属性错配的订正
- **适用落点**（纯同步原语，loom 建模得到）：
  - `crates/mlc-engine/src/engine.rs:23 use std::sync::{Arc, Mutex};` + `:139 card_system: Arc<Mutex<ExperienceCardSystem>>`
  - `crates/hcw-window/src/selector_learner.rs:53`「`RwLock` 选择 `std::sync::RwLock` 而非 `tokio::sync::RwLock`」的那一处策略持有者
- **不适用面（订正 R1 处方）**：`hcw-window` 的主状态锁是 **async 锁**——`window.rs:34 use tokio::sync::RwLock`、`:92 state: Arc<RwLock<HcwState>>`、`:200/:218/:243/:273` 等 `self.state.write().await` 站点。其交错由 tokio 调度器决定，loom 的 permutation 引擎不建模 tokio task/`await` 点 ⇒ 对它落 loom 是**工具-属性错配**。R1 ADR-224 `:100` 的处方「补则 L2 hcw/mlc 两处持锁-广播场景首批」因此**只执行一半**：mlc 侧按 ① 落，hcw 主锁侧改走 (a) 持锁跨 await 的静态面（仓内已有同族门，M02 双门）+ (b) `#[tokio::test(flavor = "current_thread")]` 确定性调度。
- 不落 L0-A（`check_l0_purity.py` 的 `ALLOW_DEPS = {"serde","chrono","uuid","thiserror"}` 不容新项）、不落 L0-B（本诉求与 L0-B 无关，且会触发附录 §9 的第 4 外部依赖门槛）。

### ③ 体积/编译/供应链影响（已取一阶实测，传递闭包仍留 E1）
`cargo info loom` + 读已下载元数据 `.toolchain/cargo/registry/src/index.crates.io-*/loom-0.7.2/Cargo.toml:39-71`【亲验】：

- loom 0.7.2 声明依赖共 9 项；**`features: +default = []`** ⇒ 默认不拉 `serde`/`serde_json`/`pin-utils`（checkpoint/futures 两个可选 feature 不开）；loom **不依赖 tokio**（与 ④ 一致）。
- 与本仓 `Cargo.lock`（现取：**628** 个去重包名 / **697** 个条目）对拍：`cfg-if`、`serde`、`serde_json`、`tracing`、`tracing-subscriber`、`futures-util` 六项**已在库**⇒ **默认 feature 下真正净新增只有一阶 2 个：`generator 0.8.1` 与 `scoped-tls 1.0.0`**（`pin-utils` 属于 `futures` feature，不开则不进图；`serde`/`serde_json` 属 `checkpoint` feature，同理）。
- ⇒ **真正需要实测的不是数量而是 `generator`**：它做上下文切换（平台相关实现），历史就是 Windows 支持面最窄的那一项。R1/仓内那 5 处断言说的「Windows GNU loom 不可用」若属实，根因极大概率在它——本 ADR 把 E1-a′ 的判定对象因此明确为：`generator 0.8.1` 在 `x86_64-pc-windows-gnu` 上的可编译性，而不是笼统的「loom 能不能用」。
- **不写传递闭包清单**：一阶集合是元数据实测，闭包需 `cargo tree -p loom -e dev` 落地时取（拿一阶当全量就是臆断计数）。E1 现场取三行读数并附原文：`cargo info loom`（已取，见上）、`cargo tree -p loom -e dev`、`cargo deny check` 与 `cargo audit`（ignore 条目一律以仓内 `audit.yml`/`deny.toml` 现取，不另立标准）。
- 机器判据（防「dev 面漂成 prod 面」）：只准进 `[dev-dependencies]`；若 loom 出现在任何 `[dependencies]`，G-99（L0-A 白名单）与 `scripts/l0b_dep_freeze.txt`（G-131 的 L0-B 外部依赖实测台账，loom 入列即 NEW→红）两侧都会抖红 ⇒ **不需新建门**（避免 ADR-222 反方案 R-B 式门膨胀）。
- 运行代价：本仓 15 GB 机型已实测并发构建的内存敏感面（ADR-225 D1），而 loom 的模型检验本身 CPU/内存密集 ⇒ loom job 只进 CI 专用步骤，**不进本地默认 `cargo test`**（否则把「跑一次测试」变成跑 permutation 全空间，直接污染 §7.2 发布前清单的耗时预算）。

### ④ 异步运行时能力（tokio feature）
- loom 不依赖 tokio ⇒ 本 ADR **不打开任何 `tokio` 新 feature**，并明文禁止：`--cfg loom` 下被测路径不得需要 multi_thread runtime。**推论（反向保护）**：若某场景必须 async 才能触发，说明它不属于 loom 的可验证属性，应回到 ② 的 (a)/(b) 面。这条同时把 ② 的错配在执行期钉住。

## 3. 决策

- **D1（采纳范围）**：loom 作 **dev-dependency**，仅用于纯同步核心 `mlc-engine::card_system` 与 `hcw-window::selector_learner`。R1 处方中「hcw-window 主状态锁落 loom」**不予执行**（工具错配，理由见 ②）。
- **D2（E1 是硬前置，先验证后加依赖）**：四条判据任一不过即走 D3。主路是 Linux（依 §1.5 的仓内既定决策）。
  - E1-a（**主**）：在 CI 的 ubuntu runner 上 `RUSTFLAGS="--cfg loom" cargo test -p mlc-engine --test loom_card_system` 可编译可执行。本地不跑此腿（避开与 release 压测档争内存），但本地必须取一次探针读数：建 `tmp/loom_probe/` 探针 crate（独立 `CARGO_TARGET_DIR`，不碰 workspace 工件、不改 `Cargo.lock`），只用于回答「本仓真的需要 CI 专属腿吗」而非阻塞落地。
  - E1-a′（**副，不阻塞**）：同一探针 crate 在 `stable-x86_64-pc-windows-gnu` 上编译——目的是把 §1.2 那 5 处「不可用/受限/nightly-only」断言的真伪定下来。不过 ≠ 整个 E1 不过（主路在 Linux）；过了则那 5 处均为失实断言，另开文档订正单。
  - E1-b：测试在有限时间内完成，并留下 wall-clock / peak RSS 读数（无读数即视为不过）。
  - E1-c：`.github/workflows/` 内存在**真实执行** `--cfg loom` 的步骤（不是 continue-on-error 的观测位）。判据依据 = ADR-225 D2「门在、但没人跑它」不允许重演，且本条就是把 §1.5 那句零载体承诺填上——`grep -rn loom .github/` 从 0 命中变为非 0 命中是本条的验收口径。
- **D3（E1 不过的收口路径）**：走「作废愿景行」——按 ADR-225 D9 的方式，把十六源手册 `:89`、`:1453`、`:1468` 的 loom 行登记为「工具链不可用（附 E1 实测原文）」+「本仓不采纳」，并把 §1.2 那 5 处散文断言的失实措辞（尤其「nightly-only」）订正为实测结论。M13 三态在 D1 与 D3 两条路径下都闭合；**唯一不允许的状态是继续「手册有文、盘上无物、注释有话」**。
- **D4（属性表先行）**：无论 E1 结果，先产一张「并发正确性属性 → 验证工具 → 载体（门 / CI 步骤 / 测试文件）」三态表，进交付物 03，替代逐单零散发现（与 W3 第 5 条「零装配死模块全量表」同一根因级做法）。

## 4. 破坏性面显式记录

| 面 | 判定 |
|---|---|
| 生产二进制 | **零改动**（dev-dep + `#[cfg(loom)]` 门内别名，不进 `[dependencies]`） |
| 公共 API | 无变化（`Mutex<ExperienceCardSystem>` 是私有字段；cfg 别名不改签名） |
| 真实代价 | `#[cfg]` 维度扩大：被测的两个模块需 `#[cfg(loom)] type Mutex<T> = loom::sync::Mutex<T>` 别名。属「为测试在生产源码开 cfg」，故**只在这两个模块开，不做全局 prelude**（最小化即约束） |
| 依赖治理 | loom 及其传递依赖进入 `Cargo.lock` ⇒ `cargo audit`/`cargo deny` 的受审面扩大，须在 E1 读数中体现；不引入 `unsafe` 面（`#![forbid(unsafe_code)]` 只判本仓 crate，dev-dep 的 unsafe 由 audit/deny 侧管） |

## 5. 回滚

一个提交承载全部：两个 `Cargo.toml` 的 dev-dep 行 + `Cargo.lock` + 两处 cfg 别名 + 测试文件 + workflow 步骤。`git revert` 该提交即整批回滚（禁半 revert：`Cargo.lock` 与 manifest 行不同批会制造「锁里有、表里无」）。文档面回滚 = 删本文件 + 删 `adr_index.md` 对应行（两文件同批）。

## 6. 待验证项（不得当结论用）

1. loom 0.7.2 的**传递闭包**体积（一阶集已实测：默认 feature 下净新增 `generator` + `scoped-tls` 两项；闭包待 `cargo tree -p loom -e dev`）。
2. `generator 0.8.1` 在 `x86_64-pc-windows-gnu` 上的可编译性（E1-a′；这是本 ADR 唯一未决的技术事实，也是仓内那 5 处断言的真正对象）。已定部分：`rust-version: 1.65` ⇒ 「nightly-only」不成立。
3. `mlc-engine::card_system` 的 permutation 状态空间规模；若交错数超出可承受预算，需做场景裁剪（缩小 `PER_THREAD` 级参数），裁剪后属性覆盖范围须在测试文件头写明。
4. §1.2 中「nightly-only」措辞的来源与真实版本——已定论（`rust-version: 1.65` ⇒ loom 0.7.2 在 stable 上可用，该措辞失实），但另开文档订正单修正那 5 处注释，不并入本 ADR（避免把裁决与纠偏混在一个提交里）。
