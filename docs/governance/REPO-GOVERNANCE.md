# 仓库治理章程（REPO-GOVERNANCE）

> 版本: 1.0（2026-11-13 首版，基于当日全仓诊断）
> 性质: 长期有效运维章程，与 CONTRIBUTING.md（协作流程）互补；冲突时以本章程的运维条款为准
> 证据: [R9 执行报告](../reports/toolchain-r9-hcw-v3-execution-2026-11-12.md)、tmp/audit-full-20261113.txt、tmp/audit-after-update-20261113.txt

## 1. 分支生命周期

### 1.1 存活规则

| 分支类型 | 命名 | 最长存活 | 期满处置 |
|---------|------|---------|---------|
| 主工作分支 | `feat/<批次名>` | 至批次收官 | 收官后 7 天内 PR/ff 合入 main，随后删除 |
| 阶段分支 | `feat/phase*` | 至内容被主链等效覆盖 | 覆盖验证后删除（2026-11-13 已删 phase1-w1-w8） |
| 发布分支 | `release/v*.*.*-omega` | 长期 | 仅维护模式补丁 |
| 归档分支 | `archive/*` | 永久 | 只读 |

### 1.2 删除三件套（删任何远端分支前强制执行）

1. **归属验证**：`git merge-base --is-ancestor` + 独有提交逐个核对日期/主题；
2. **主链等效性抽查**：对分支声称交付的关键物（LICENSE/配置门/文档面）`git cat-file` 验证主链真的存在等效实现；
3. **本地备份引用**：`git branch backup/remote-<name> origin/<name>` 兜底（backup 引用存活 90 天后可清）。

### 1.3 Worktree 卫生

- worktree 是隐性工作面的第一现场；每批次收官时 `git worktree list` 全量核对，**停留超过 30 天且脏文件的 worktree 必须先 dump patch（tmp/worktree-rescue-<日期>/）再移除**；
- 2026-11-13 已清理 3 个 9 月时代 `.tower` worktree（patch 已存档）。

## 2. CI 门禁矩阵（12 workflow 实测现状）

> ⚠ 文档漂移警示：AGENTS.md §10.1 登记 10 个 workflow，实测 **12 个**——`perf_redline.yml`（schedule）与 `bench_check.yml` 的 schedule 补充未回写。修订登记时以 `ls .github/workflows/*.yml` 现取为准。

| Workflow | 触发 | 阻塞性 | 健康度（2026-11-13） |
|----------|------|--------|---------------------|
| audit.yml | schedule 每日 + PR(lock/toml) | **硬门** | 🔴 **EXIT=1**（依赖腐化，见 §4） |
| ci.yml | PR(main) + push(main) | 硬门 | ✅（main ff 后首跑待确认） |
| coverage.yml | schedule 每晚 | 硬门 | ✅（min_line=85） |
| deny.yml | schedule + PR(lock/toml) | 硬门 | 待复核 |
| zizmor.yml | schedule + PR(workflows) | 硬门 | ✅（此前 10 HIGH 已消解） |
| perf_redline.yml / bench_check.yml | schedule / PR(benches,lock,cli) | 硬门 | ✅ |
| slo.yml / stress.yml | tag + schedule | 发布门/非阻塞 | ✅ |
| fuzz.yml | tag + weekly | 非阻塞（crash 上传） | 委托 Linux |
| release.yml | tag | 发布链 | ✅ |
| test-install-scripts.yml | schedule + release published | 安装链路 | ✅ |

**门禁原则**：audit/deny/coverage/zizmor/perf 五类每日 schedule 门任何一个红，必须在 **24h 内**定性（依赖漂移 or 代码回归）并开修复单；超 72h 未处置视为治理失效（本章程 §4 即触发案例）。

## 3. 分支保护与推送纪律

- 现状：远端 ruleset 存在（push 观测到 "Bypassed rule violations" 放行），**main 无硬保护**——431 提交曾可直推。
- **目标态**：main 启用 ruleset「require PR + require ci.yml 绿」；直推 bypass 仅限单人仓库的备份场景，且每次 bypass 推 main 后必须在 24h 内人工核对 Actions 页全量 CI 结果。
- 本仓库单人维护期：保留 bypass 能力，但**默认走 PR**（PR 上 CI 先行，人工 merge）。

## 4. 依赖安全治理（SLA + 修复流程）

### 4.1 双口径对照（经验教训）

GitHub dependabot（manifest 粒度，本次 16 告警）与 `cargo audit`（RUSTSEC 全量，本次 44 vuln + 14 denied）**口径相差 2.7×**——治理以 **cargo audit 为准**（CI 硬门），dependabot 仅作提醒源。

### 4.2 响应 SLA

| 等级 | 触发 | SLA |
|------|------|-----|
| P0 | audit EXIT=1（vulnerabilities>0） | 24h 内定性，72h 内修复或显式 ignore 评估 |
| P1 | unmaintained/unsound 新增 | 下一批次收纳 |
| P2 | yanked | 评估后可 ignore（registry 下载不受影响） |

### 4.3 修复阶梯（本次实测有效）

1. `cargo update`（semver 兼容升级）——**本次消 18/44**，零代码改动，优先做；
2. 定向跨版本升级（breaking 面）——逐项做，每项独立 commit + 编译/测试验证；
3. unmaintained 类走「评估 → ignore + ADR 式理由登记」（audit.yml 既有 6 条 ignore 先例，理由必须写进 yml 注释）。

### 4.4 依赖修复台账（2026-11-13 诊断 + 2026-11-14 dependabot 对照修正）

> ⚠ **移交状态**：本节修复方案由治理诊断产出；因另一会话在同一工作树活跃（改动被回滚），依赖升级实操**移交下一批次执行**。受影响面编译验证已确认 `cargo update` 兼容升级不破坏任何消费 crate（seccore/nmc-encoder/repo-wiki/scc-cache/session-store/cmt-tiering 六 crate check 全绿）。

| 根因 | dependabot 口径 | 修复方案（已验证可行） |
|------|----------------|----------------------|
| zstd-safe 7.2.4 + 兼容面 130+ crate | — | ✅ **`cargo update` 即消 18 条**（已实证：zstd-safe 7.4.1、六 crate check 全绿） |
| **wasmtime 47.0.4**（seccore:56） | **critical ×2**（>=37.0.0, <48.0.7）+ medium ×2 | **消 critical 只需 48.0.7+**（比 50.0.1 更保守的跳变）；49/50 线可选。改 seccore/Cargo.toml → `cargo update -p wasmtime` → `cargo check -p seccore --features wasm-sandbox`（5+ 分钟编译） |
| **zip 2.6.1**（nmc-encoder→mel_spec 0.4.0→ndarray-npy 0.9.1→zip） | **critical ×1**（<=9.0.4）+ high ×1（<9.0.0） | 🔴 **hard problem**：critical/high 要求 **9.0.4+**，跨 7 个大版本且被上游 mel_spec/ndarray-npy 锁死。选项：a) 推动上游升级；b) fork；c) **accept-risk ignore + 攻击面论证**（nmc 音频路径仅读本地 .npy，无网络输入——critical 级 ignore 需 ADR 级评审） |
| libsqlite3-sys 0.30.1（rusqlite 0.32 bundled） | low | rusqlite 0.32→0.42（跨 10 minor），消费方 repo-wiki/scc-cache/session-store/cmt-tiering——批次级升级 |
| rustls <0.23.46 | low ×4 + medium ×2 | cargo update 兼容面已覆盖部分；剩余按上游 reqwest 约束评估 |
| anymap2 0.13.0 unmaintained | — | 上游 tract 链（liquid-core→tract-nnef 0.22.5）——不受控，评估 ignore |
| unsafe-libyaml unmaintained（serde_yaml 0.9 底层） | — | serde_yaml deprecated，三消费方（chimera-tui/seams/contracts）——迁移或 ignore 评估 |

**注意**：修改 audit.yml 的 ignore 清单必须跑 `python scripts/audit_cmd_sync.py`（ADR-167 决策 4 三处逐字一致性守卫：CI/本清单/AGENTS.md §7.2）。

### 4.5 双会话同树治理教训（2026-11-13/14 实录）

多会话并行时的**工作树让渡规则**（本节由两次实测冲突提炼）：
1. **开工前** `git status` 全量核对 + `git worktree list`——发现非本会话改动即请示归属；
2. **活跃期检测**：文件被外部修改（Edit 报 "modified since read"）或提交被回滚 = 对方会话活跃 → **立即停止对该文件/该面的写操作**，成果转为文档移交（本节即移交实例）；
3. **锁规避**：对方 cargo 进程持有构建锁时，`cargo search/info` 会长时间阻塞——版本情报改用 crates.io HTTP API（`api/v1/crates/<name>` 的 `max_stable_version` 字段）；
4. **编译红仲裁**：`cargo check` 失败先看错误符号归属（本次 E0405 `WindowGovernor` = 对方 P1 半成品），**不得替对方修复其未完成文件**。

## 5. 文档漂移治理

- **教训**：AGENTS.md 的 workflow 清单（10）与实测（12）漂移；R8 深审的 coarse.rs 并行结论被并行会话取代——**所有「清单/计数」类断言在引用前必须现取**（本仓既有铁律「计数现取不登记」的延伸：workflow/tag/crate 清单一律 `ls`/`git ls-remote` 现取）。
- 每批次收官检查单新增一项：`ls .github/workflows/*.yml | wc -l` 与 AGENTS.md §10.1 对照。

## 6. 治理日历（建议节律）

| 节律 | 动作 |
|------|------|
| 每日 | 五类 schedule 门结果人工扫一眼（红即开单） |
| 每批次收官 | **批次收官检查单（§7）** + worktree 核对 + backup/ 引用清点 + AGENTS.md workflow 数对照 |
| 每月 | cargo audit 全量复核 + backup/ 引用 90 天清理 + 依赖锁定面刷新 |
| 每季度 | tag/发布分支梳理 + README/install 链接抽验 |

## 7. 批次收官检查单（2026-11-14 增补——四门连环红的直接教训）

> 背景：R9/R9b 两批次收官均只跑 `cargo test` 定向回归，未本地复跑 scripts 门族——PR CI 上 **G-91/G-105/G-115/G-123 四门连环红**（fail-fast 链串行暴露，每修一个冒下一个，三轮 push 才见底）。L0 契约面新增类型的登记面有 **至少五个独立台账**。

批次触及 L0 契约面（新增类型/fn/事件变体）时，收官前必须本地复跑：

```bash
py -I scripts/check_contract_consumers.py            # G-91 消费者基线（GAP 登记/STALE 删行）
py -3 scripts/check_contract_zero_consumers.py       # G-123 零消费者台账（机派 --write）
py -I scripts/check_l0_pubfn_ratchet.py              # G-105 pub fn 棘轮
py -I scripts/check_l0_extensibility_class.py        # G-115 extensibility class（open/closed 政策声明）
py -I scripts/check_l0_root_surface.py               # G-107 根 re-export 面
py -I scripts/check_workflow_yaml_lexical.py         # G-141/142（动 workflow 时）
```

登记纪律：
- G-91/G-105/G-115 是 **only-shrink/政策声明** 台账——新行必须带 reason（人类评审语义），达标行同批删除；
- G-123 是 **机派**（`--write` 重生成，hand-edit forbidden）；
- G-115 的 CLAIM 头部统计必须与行变更**同一 commit** 刷新；
- 事件变体另走六锁（`variant_count_test.rs` + TUI 穷举 match + govd 对账）——见 AGENTS.md 事件预算纪律。
