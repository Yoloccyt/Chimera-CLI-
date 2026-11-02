# ADR-060: Chimera CLI ExitCode 矩阵

## 状态

Proposed — 2026-07-30

## 上下文

`chimera-cli` 在 v2.8.0-omega 前的 `main` 函数签名为 `async fn main() -> anyhow::Result<()>`，所有错误统一返回退出码 1（anyhow 默认行为）。这导致：

1. **脚本无法区分错误类型**：shell 脚本/CI 只能通过 `$? -eq 0` 判断成功，失败时无法程序化区分"用户取消"（可重试）与"权限拒绝"（需授权）等场景
2. **NotImplemented 被误判为成功**：Task 0.1 前 4 个占位命令静默 `Ok(())` 返回退出码 0，脚本误以为功能已执行
3. **错误处理粗糙**：与 Claude Code CLI / Kimi Code CLI 等工业级 CLI 的结构化退出码（区分 user error / system error / cancelled）相比缺乏可观测性

### 约束

- `#![forbid(unsafe_code)]` 合规（`std::process::ExitCode` 是安全 API）
- 不破坏 Task 0.1 的 17 个测试（退出码断言从"非零"升级为"精确匹配"）
- 向后兼容：依赖 `$? -eq 1` 的 v2.8.0 脚本需有迁移路径（2 个版本周期）
- 依赖铁律：不引入新 crate（`std::process::ExitCode` 是标准库）
- `PermissionDenied` 退出码必须为 5（非 1），区分"用户输入错误"与"安全策略拒绝"

## 决策

引入 ExitCode 矩阵，将 `main` 签名改为 `async fn main() -> std::process::ExitCode`，根据 `ChimeraCliError` 变体映射结构化退出码。

> **实施现状核对（2026-09-22，只登记事实，不改写下方决策）**
> - ✅ 矩阵与实现逐条一致：`crates/chimera-cli/src/error.rs:84-94` `NotImplemented→2 / ConfigError→1 / EngineError·IoError→3 / UserCancelled→4 / PermissionDenied→5 / Timeout→6`；`kind` 稳定标识符见 `error.rs:103-112`（7 个），envelope 形状见 `crates/chimera-cli/src/output.rs:14-32`。
> - ❌ **迁移计划第 3 步未执行**：本 ADR 称「v2.11.0-omega 移除 `--legacy-exit-code`，矩阵成为唯一行为」，但该 flag 仍在码内 —— `crates/chimera-cli/src/cli.rs:94` 声明 `pub legacy_exit_code: bool`、`main.rs:125` 仍 `if cli.legacy_exit_code { … ExitCode::from(1) }`（`main.rs:38` 注释亦自陈保留）。⇒ 要么补做移除（属 `crates/` 改动，需授权），要么由治理把本 ADR 的该步改写为「保留一个兼容窗口/长期保留」。**当前实况：矩阵默认生效，但显式传 `--legacy-exit-code` 会退回全 1。**
### 1. 退出码矩阵

| 退出码 | 语义 | 对应 `ChimeraCliError` 变体 | 说明 |
|--------|------|-----------------------------|------|
| 0 | success | (Ok 路径) | 命令执行成功 |
| 1 | user_error | `ConfigError` | 配置非法，用户可纠正后重试 |
| 2 | not_implemented | `NotImplemented` | 占位骨架未接入真实引擎，指引替代方案 |
| 3 | system_error | `EngineError` / `IoError` / 未知错误 | 下游引擎故障或 IO 失败，需运维介入 |
| 4 | user_cancelled | `UserCancelled` | 用户主动取消（Ctrl+C 或 permission prompt 拒绝） |
| 5 | permission_denied | `PermissionDenied` | 安全策略拒绝，需显式授权或修改安全配置 |
| 6 | timeout | `Timeout` | GQEP 聚集等超时，需检查下游服务可达性 |

**WHY PermissionDenied=5 而非 1**：`1`（user_error）表示"用户输入错误"（改参数即可重试），`5`（permission_denied）表示"权限策略拒绝"（需显式授权或修改安全配置），语义不同需区分。对应 §6 红线"命令插值禁用"的安全审计场景，权限拒绝应触发独立的告警与审计流程，而非与普通用户错误混为一谈。

### 2. 实现方式：anyhow downcast

`dispatch` 保持返回 `anyhow::Result<()>`（不破坏现有调用点），`main` 中通过 `downcast_ref::<ChimeraCliError>()` 还原具体类型：

```rust
async fn main() -> std::process::ExitCode {
    match run(&cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {:?}", e);
            if let Some(ce) = e.downcast_ref::<ChimeraCliError>() {
                ce.exit_code()
            } else {
                std::process::ExitCode::from(3) // 未知错误归 system_error
            }
        }
    }
}
```

**WHY downcast 而非改 dispatch 签名**：改 `dispatch` 返回 `Result<(), ChimeraCliError>` 会破坏所有 handler 的 `anyhow::Result` 返回约定（需逐个改签 + `From` 转换），引入大量样板代码。downcast 方案零侵入，仅 `main` 边界需感知 `ChimeraCliError`。

### 3. BREAKING 兼容性：`--legacy-exit-code` flag

提供全局 flag `--legacy-exit-code`（`hide = true`，不在 `--help` 显示），启用后所有错误统一返回退出码 1，兼容 v2.8.0 行为：

```rust
if cli.legacy_exit_code {
    return std::process::ExitCode::from(1);
}
```

**迁移路径**：
- v2.9.0-omega：ExitCode 矩阵默认启用，`--legacy-exit-code` 提供回退
- v2.10.0-omega：`--legacy-exit-code` 标记 deprecated，输出 warning
- v2.11.0-omega：移除 `--legacy-exit-code`，ExitCode 矩阵成为唯一行为

此模式参考 ADR-029 TUI v3.1 的 BREAKING 处理（`v3-engine` feature 双轨 + `--no-v3-engine` 回退）。

### 4. 非 ChimeraCliError 的 fallback

`config::load` 等非 dispatch 路径的错误，通过 `map_err(ChimeraCliError::ConfigError)` 包装后进入 downcast 路径。无法预料的底层错误（如 panic hook 未捕获的 `unwrap`）归为 system_error（3），因其属于"下游引擎或 IO 故障"语义。

## 后果

### 正面

- **脚本可观测性**：CI 可通过 `$?` 精确区分 6 种错误类别，实现差异化重试/告警策略
- **错误分类清晰**：与 Claude Code CLI（exit 1=error / 2=cancelled）等工业级 CLI 对齐，且更细粒度
- **NotImplemented 可识别**：退出码 2 使脚本能识别"功能未实装"并跳过，而非误判为通用错误
- **零依赖**：`std::process::ExitCode` 是标准库稳定 API（Rust 1.61+），无新增 crate

### 负面

- **BREAKING**：v2.8.0 脚本若硬编码 `$? -eq 1` 判断失败，会因新的退出码 2-6 误判成功（`-eq 1` 不匹配 2-6）。`--legacy-exit-code` 提供 2 版本迁移窗口
- **downcast 开销**：`downcast_ref` 是虚函数调用 + 类型 ID 比较，延迟在纳秒级（benchmark 验证 < 10µs），可忽略
- **错误输出格式变化**：v2.8.0 anyhow 自动打印 `Error: <msg>`，现在改为 `Error: <Debug>`（含变体前缀）。stderr grep 模式需更新

## 引用

- spec.md A2 决策（ExitCode 矩阵）
- ADR-029 TUI v3.1 BREAKING 处理模式（feature 双轨 + 回退 flag）
- POSIX 退出码语义：0=success，1-125=应用错误，126-255=特殊语义
- Claude Code CLI / Kimi Code CLI 工业尸检：结构化退出码提升脚本可观测性
