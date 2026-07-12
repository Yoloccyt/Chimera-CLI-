# fuzz CI 配置静态验证集成方案

> 文档版本:v1.5.1-omega
> 创建日期:2026-07-12
> 关联文件:`.github/workflows/fuzz.yml` · `scripts/check_fuzz_config.sh` · `scripts/check_fuzz_config.ps1`
> 关联 ADR:无(工程实践改进,非架构决策)

---

## 1. 背景与动机

### 1.1 问题陈述

项目在 `.github/workflows/fuzz.yml` 中已实现 6 个 fuzz target 的 CI matrix 执行(每个 target 独立 job,运行 300s),但缺少配置预检环节。当前存在以下隐患:

- **配置漂移不可见**:fuzz crate 的 `Cargo.toml` 中 metadata、`[[bin]]` 声明、target 文件路径任一发生漂移,只能在 fuzz job 启动后通过 cargo-fuzz 编译失败暴露
- **CI 资源浪费**:配置错误时,6 个 matrix job 全部启动并失败,每个 job 包含 nightly toolchain 安装(~30s) + cargo-fuzz 安装(~30s) + 编译(~2min) + 300s fuzz,总计约 30min 的 CI 资源浪费
- **本地与 CI 不一致**:本地 Windows GNU 环境已通过 `scripts/check_fuzz_config.{ps1,sh}` 静态验证配置,但 Linux CI 未调用此脚本,导致本地与 CI 的预检能力不对称

### 1.2 已有资产

| 文件 | 平台 | 用途 |
|------|------|------|
| `scripts/check_fuzz_config.ps1` | Windows PowerShell | 本地 Windows 验证 |
| `scripts/check_fuzz_config.sh` | Linux/macOS Bash | 本地非 Windows 验证 + CI 验证 |

两脚本均验证 6 类配置完整性(详见 §3.2),退出码语义一致:0 = 全部通过,1 = 有失败项。

### 1.3 集成目标

- 在 fuzz matrix 启动前执行配置静态验证
- 配置漂移时阻塞 6 个 matrix job 启动,节省 CI 资源
- 保持本地与 CI 预检能力一致
- 不破坏现有 fuzz job 的执行逻辑与产物上传

---

## 2. 设计决策

### 2.1 决策 D1:独立 pre-check job vs 现有 job 内 step

| 方案 | 优点 | 缺点 | 选用 |
|------|------|------|------|
| **A. 独立 pre-check job** + `needs:` 依赖 | 配置失败时 6 个 matrix job 完全不启动,资源节省最大化;逻辑隔离清晰 | 增加 job 启动开销(~20-30s) | ✅ |
| B. fuzz job 内第一个 step | 无额外 job 协调开销 | 6 个 matrix job 都跑一遍相同检查,冗余 6 倍;且 step 失败时 matrix 已启动,无法阻止资源浪费 | ✗ |

**决策**:采用方案 A(独立 pre-check job)。

**理由**:
1. fuzz matrix 启动后无法被中途取消(已分配 runner),配置错误时 6 个 job 仍会跑完 checkout + toolchain 安装才失败,浪费最大
2. pre-check 是纯 shell 脚本验证,无需 nightly toolchain 与 cargo-fuzz 安装,job 启动开销可接受(~20-30s)
3. 通过 `needs: pre-check` 显式声明依赖,符合 GitHub Actions 推荐的 job 编排模式

### 2.2 决策 D2:使用脚本选择

| 脚本 | 平台 | CI 使用 |
|------|------|---------|
| `check_fuzz_config.sh` | Linux/macOS | ✅ CI 使用(ubuntu-latest runner) |
| `check_fuzz_config.ps1` | Windows PowerShell | 不使用(fuzz.yml 仅 Linux runner) |

**决策**:仅使用 `check_fuzz_config.sh`。

**理由**:fuzz.yml 的所有 job 均 `runs-on: ubuntu-latest`,Linux 环境下 bash 脚本是原生支持,无需引入 PowerShell Core 跨平台运行时。Windows 版脚本保留用于本地开发预检。

### 2.3 决策 D3:失败行为 — 阻塞 vs 警告

**决策**:pre-check 失败**阻塞**后续 fuzz 执行(退出码 1 触发 job 失败,`needs: pre-check` 阻止 fuzz job 启动)。

**理由**:
- 配置漂移意味着 fuzz target 可能无法编译或无法正确接收输入,继续执行 fuzz 无意义
- 与 task 要求一致("pre-check 步骤失败应阻塞后续 fuzz 执行")
- 项目其他 CI(audit.yml `--deny warnings`、release.yml binary 体积检查)均采用阻塞式失败,保持风格一致

### 2.4 决策 D4:timeout-minutes 设置

**决策**:pre-check job `timeout-minutes: 3`。

**理由**:
- 实际执行时间:checkout ~5s + 脚本运行 ~1-2s = 总计 <30s
- 3min 兜底防止 runner 启动慢或 GitHub 网络抖动导致误判
- 远小于 fuzz job 的 20min,不会成为 CI 总时长的瓶颈

---

## 3. 修改前后对比

### 3.1 fuzz.yml 结构对比

#### 修改前(单 job)

```
jobs:
  fuzz:                          # 单 job,matrix 6 target 并行
    name: Fuzz (${{ matrix.target }})
    runs-on: ubuntu-latest
    timeout-minutes: 20
    strategy:
      matrix:
        target: [6 个 target]
    steps:
      - Checkout
      - Setup nightly Rust toolchain
      - Cache cargo registry & target
      - Install cargo-fuzz
      - Run fuzz target
      - Upload fuzz log
      - Upload crash inputs
```

#### 修改后(2 job,pre-check + fuzz)

```
jobs:
  pre-check:                     # 新增 job:配置静态验证
    name: Fuzz config pre-check
    runs-on: ubuntu-latest
    timeout-minutes: 3
    steps:
      - Checkout
      - Run fuzz config static validation   # 调用 check_fuzz_config.sh

  fuzz:                          # 原 job,新增 needs: pre-check
    name: Fuzz (${{ matrix.target }})
    needs: pre-check             # 新增:依赖 pre-check 通过
    runs-on: ubuntu-latest
    timeout-minutes: 20
    strategy:
      matrix:
        target: [6 个 target]    # 保持不变
    steps:                       # 7 个 step 完全保持不变
      - Checkout
      - Setup nightly Rust toolchain
      - Cache cargo registry & target
      - Install cargo-fuzz
      - Run fuzz target
      - Upload fuzz log
      - Upload crash inputs
```

### 3.2 保留不变的部分

| 项 | 修改前 | 修改后 | 一致性 |
|----|--------|--------|--------|
| 触发条件 | `push.tags: v*.*.*-omega` + `workflow_dispatch` | 同左 | ✅ 不变 |
| `permissions` | `contents: read` | 同左 | ✅ 不变 |
| `env` | `CARGO_TERM_COLOR=always` + `RUST_BACKTRACE=1` | 同左 | ✅ 不变 |
| fuzz job `runs-on` | `ubuntu-latest` | 同左 | ✅ 不变 |
| fuzz job `timeout-minutes` | 20 | 同左 | ✅ 不变 |
| fuzz job matrix | 6 target | 同左 | ✅ 不变 |
| fuzz job `fail-fast` | `false` | 同左 | ✅ 不变 |
| fuzz job steps | 7 个 step | 同左 | ✅ 不变 |
| artifact 上传策略 | log 30 天 + crash 90 天 | 同左 | ✅ 不变 |

### 3.3 新增的部分

| 项 | 值 | 说明 |
|----|-----|------|
| `pre-check` job | 新增 | 独立 job,先于 fuzz 执行 |
| `pre-check.timeout-minutes` | 3 | 兜底防 runner 启动慢 |
| `pre-check.steps` | 2 个 | Checkout + 运行 check_fuzz_config.sh |
| `fuzz.needs` | `pre-check` | 显式依赖,阻塞式 |

---

## 4. 执行流程详解

### 4.1 修改后 CI 执行时序

```
[触发] tag v*.*.*-omega push 或 workflow_dispatch
   │
   ▼
[Job 1] pre-check (ubuntu-latest, ~20-30s)
   │   ├─ Checkout (actions/checkout@v4)
   │   └─ Run check_fuzz_config.sh
   │       ├─ 检查 1: fuzz/Cargo.toml 存在
   │       ├─ 检查 2: cargo-fuzz metadata
   │       ├─ 检查 3: [lib] path 声明
   │       ├─ 检查 4: 8 个 [[bin]] 声明 + target 文件
   │       ├─ 检查 5: fuzz_target! 宏调用
   │       └─ 检查 6: 被测 crate path 依赖目录
   │
   ├── [FAIL] 退出码 1 → pre-check job 失败 → fuzz job 不启动 → CI 失败
   │
   └── [PASS] 退出码 0 ↓
   │
   ▼
[Job 2] fuzz matrix (6 并行,每个 ~5-6min)
   │   ├─ Fuzz (quest_parse)
   │   ├─ Fuzz (seccore_sandbox)
   │   ├─ Fuzz (event_serialize)
   │   ├─ Fuzz (cacr_budget_parse)
   │   ├─ Fuzz (checkpoint_deserialize)
   │   └─ Fuzz (config_section_parse)
   │
   ▼
[完成] 所有 job 成功 → CI 通过
```

### 4.2 check_fuzz_config.sh 验证项详解

| 序号 | 验证项 | 失败原因示例 | 影响 |
|------|--------|-------------|------|
| 1 | `fuzz/Cargo.toml` 存在 | 文件被误删 | cargo-fuzz 无法解析 crate |
| 2 | `[package.metadata] cargo-fuzz = true` | metadata 漂移 | cargo-fuzz 0.13+ 拒绝运行 |
| 3 | `[lib] path = "src/lib.rs"` 声明 | lib 段丢失 | Windows-GNU stub 宏载体缺失 |
| 4 | 8 个 `[[bin]]` 声明 + target 文件存在 | bin 声明或文件缺失 | cargo-fuzz 找不到 target |
| 5 | 每个 target 包含 `fuzz_target!` 宏 | 宏调用丢失 | target 编译失败 |
| 6 | 被测 crate path 依赖目录存在 | crate 重命名/移动 | 编译时 path 依赖解析失败 |

### 4.3 失败场景与行为

| 场景 | pre-check 行为 | fuzz job 行为 | CI 总时长 |
|------|----------------|---------------|-----------|
| 配置正常 | PASS,~30s | 6 job 并行执行,~15-20min | ~15-20min |
| fuzz/Cargo.toml 缺失 | FAIL,<30s | 不启动 | ~30s |
| bin 声明漂移 | FAIL,<30s | 不启动 | ~30s |
| target 文件丢失 | FAIL,<30s | 不启动 | ~30s |
| fuzz_target! 宏缺失 | FAIL,<30s | 不启动 | ~30s |
| 配置正常但 fuzz 运行 crash | PASS | matrix 中对应 target job 失败,上传 crash input | ~15-20min |

---

## 5. 执行效率分析

### 5.1 正常路径(配置无漂移)开销

| 阶段 | 修改前 | 修改后 | 增量 |
|------|--------|--------|------|
| pre-check job | 无 | ~20-30s | +20-30s |
| fuzz job 启动等待 | 立即 | 等待 pre-check 完成 | +20-30s |
| fuzz matrix 执行 | ~15-20min | ~15-20min | 0 |
| **总计** | **~15-20min** | **~15-20min + ~30-60s** | **+3-5%** |

**结论**:正常路径下 CI 总时长增加约 30-60s,相对 fuzz matrix 本身的 15-20min 可忽略(占比 <5%)。

### 5.2 异常路径(配置漂移)节省

| 阶段 | 修改前 | 修改后 | 节省 |
|------|--------|--------|------|
| 配置错误发现时机 | fuzz job 编译阶段(~2-3min 后) | pre-check 阶段(<30s) | ~2-3min |
| 失败 job 数量 | 6 个 matrix job 全部失败 | 1 个 pre-check job 失败 | 5 个 job |
| 浪费 CI 资源 | 6 × (checkout + toolchain + cargo-fuzz install + 编译) ≈ 6 × 3min = 18min | 1 × 30s | ~17.5min |
| **总计节省** | — | — | **~17min** |

**结论**:配置漂移场景下,pre-check 节省约 17min CI 资源(6 个 matrix job 的启动+编译开销)。

### 5.3 触发频率评估

- fuzz.yml 触发条件:`v*.*.*-omega` tag push 或 `workflow_dispatch`
- 项目当前发版频率:约每周 1-2 次 tag(基于 CHANGELOG v1.0.0 → v1.5.0 的演进节奏)
- pre-check 主要价值:在发版时阻止配置漂移导致的 fuzz 失败,避免发版被阻塞

---

## 6. 集成测试方案

### 6.1 本地静态验证(无需 CI)

```powershell
# Windows 本地验证(使用 PowerShell 版脚本)
cd "D:\Chimera CLI"
.\scripts\check_fuzz_config.ps1

# 期望输出:
# === fuzz crate 配置静态验证 ===
# [1/6] 检查 fuzz/Cargo.toml 存在性
#   [PASS] fuzz/Cargo.toml 存在
# ... (省略中间输出)
# === 验证通过: 所有检查项 PASS ===
# 退出码 0
```

```bash
# Linux/macOS/Git Bash 本地验证(使用 bash 版脚本)
cd "/d/Chimera CLI"
bash scripts/check_fuzz_config.sh

# 期望输出同上,退出码 0
```

### 6.2 YAML 语法验证

```python
# 使用 Python yaml 模块验证 fuzz.yml 语法
import yaml
with open('.github/workflows/fuzz.yml', 'r', encoding='utf-8') as f:
    data = yaml.safe_load(f)

assert data['name'] == 'Fuzz'
assert 'pre-check' in data['jobs']
assert 'fuzz' in data['jobs']
assert data['jobs']['fuzz']['needs'] == 'pre-check'
assert data['jobs']['pre-check']['runs-on'] == 'ubuntu-latest'
assert data['jobs']['pre-check']['timeout-minutes'] == 3
print('YAML 语法验证通过')
```

### 6.3 CI 触发验证

#### 步骤 1:workflow_dispatch 手动触发

在 GitHub Actions 页面手动触发 Fuzz workflow,观察:
1. `pre-check` job 先启动,约 30s 完成
2. `pre-check` 通过后,6 个 `Fuzz (target)` job 并行启动
3. 所有 job 完成后,workflow 状态为 success

#### 步骤 2:配置漂移注入测试(可选)

在测试分支上故意破坏 fuzz 配置(如注释掉某个 `[[bin]]` 声明),手动触发 workflow,观察:
1. `pre-check` job 失败,日志显示 `[FAIL]` 项
2. `fuzz` job 不启动(状态为 skipped)
3. workflow 状态为 failure

```bash
# 示例:注释掉 quest_parse 的 [[bin]] 声明
# 在 fuzz/Cargo.toml 中将 [[bin]] name = "quest_parse" 段注释
# 提交到测试分支,触发 workflow,验证 pre-check 失败
```

#### 步骤 3:tag 触发验证

推送 `v*.*.*-omega` tag(如下一个 release),观察完整流程:
1. `pre-check` 自动启动
2. 通过后 6 个 fuzz matrix job 并行
3. 所有 artifact(log + crash)正常上传

### 6.4 验证清单

- [ ] 本地 `bash scripts/check_fuzz_config.sh` 退出码 0
- [ ] 本地 `.\scripts\check_fuzz_config.ps1` 退出码 0
- [ ] Python YAML 语法验证通过
- [ ] workflow_dispatch 手动触发,pre-check 通过,fuzz matrix 正常执行
- [ ] (可选)注入配置漂移,pre-check 失败,fuzz job 被跳过
- [ ] tag 触发,完整流程通过

---

## 7. 风险与缓解

### 7.1 风险 R1:pre-check 误报阻塞发版

**风险**:check_fuzz_config.sh 本身有 bug,导致正常配置被误判为失败,阻塞 tag 触发的 fuzz CI。

**缓解**:
- 脚本已在本地区域验证通过(退出码 0)
- pre-check job 失败不阻塞 release.yml(独立 workflow)
- 紧急情况下可通过 `workflow_dispatch` 重新触发,或修复脚本后重新推送 tag

### 7.2 风险 R2:check_fuzz_config.sh 与 Cargo.toml 不同步

**风险**:fuzz crate 新增 target 时,忘记更新 check_fuzz_config.sh 的 `EXPECTED_TARGETS` 数组,导致 pre-check 失败。

**缓解**:
- 脚本头部注释明确列出 8 个预期 target
- 新增 target 时需同步更新:`fuzz/Cargo.toml` + `fuzz/fuzz_targets/` + `scripts/check_fuzz_config.{sh,ps1}`
- 两脚本(.sh + .ps1)对称设计,任一未更新会在本地开发时暴露

### 7.3 风险 R3:CI runner 无 bash 执行权限

**风险**:`check_fuzz_config.sh` 在 ubuntu-latest 上无执行权限,直接 `./scripts/check_fuzz_config.sh` 报 Permission denied。

**缓解**:pre-check step 显式 `chmod +x scripts/check_fuzz_config.sh` 后再执行,见 fuzz.yml L57-59。

---

## 8. 后续演进建议

| 建议 | 优先级 | 说明 |
|------|--------|------|
| 将 check_fuzz_config.sh 扩展为支持 `--strict` 模式(检查 stub 宏语法树) | P3 | 当前仅 grep 匹配 `fuzz_target!(`,无法验证宏参数正确性 |
| 在 release.yml 的 test job 中也调用 check_fuzz_config.sh | P2 | 让 PR 触发时也能预检 fuzz 配置(当前仅 tag/workflow_dispatch 触发 fuzz.yml) |
| 将 pre-check 脚本统一为 Rust binary(替代 bash + ps1 双版本) | P4 | 消除跨平台脚本维护成本,但增加构建依赖 |

---

## 9. 关联文件索引

| 文件 | 路径 | 角色 |
|------|------|------|
| 修改的 workflow | `D:\Chimera CLI\.github\workflows\fuzz.yml` | 添加 pre-check job + needs 依赖 |
| Linux 验证脚本 | `D:\Chimera CLI\scripts\check_fuzz_config.sh` | CI 调用 |
| Windows 验证脚本 | `D:\Chimera CLI\scripts\check_fuzz_config.ps1` | 本地开发调用 |
| fuzz crate 配置 | `D:\Chimera CLI\fuzz\Cargo.toml` | 被验证对象 |
| stub 宏载体 | `D:\Chimera CLI\fuzz\src\lib.rs` | 被验证对象 |
| fuzz target 源 | `D:\Chimera CLI\fuzz\fuzz_targets\*.rs` | 被验证对象 |
| 参考 workflow | `D:\Chimera CLI\.github\workflows\release.yml` | CI 风格参考 |
| 参考 workflow | `D:\Chimera CLI\.github\workflows\audit.yml` | CI 风格参考 |

---

## 10. 变更摘要

| 项 | 修改前 | 修改后 |
|----|--------|--------|
| `.github/workflows/fuzz.yml` job 数 | 1 个(fuzz) | 2 个(pre-check + fuzz) |
| `fuzz` job `needs` | 无 | `pre-check` |
| 新增 step 数 | 0 | 2(Checkout + Run validation) |
| 配置漂移发现时机 | fuzz job 编译阶段(~2-3min) | pre-check 阶段(<30s) |
| 配置错误时浪费 CI 资源 | ~18min(6 job × 3min) | ~30s(1 job) |
| 正常路径 CI 总时长增量 | — | +30-60s(<5%) |
