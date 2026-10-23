//! Chimera CLI 错误类型 — 应用层结构化错误码
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(CLI 入口错误处理)
//!
//! WHY thiserror 而非 anyhow:`chimera-cli` 虽为应用层,但作为 CLI 入口需要
//! 结构化错误码以映射退出码矩阵(Task 0.2 将引入 `exit_code()` 方法)。
//! 各变体的 `Display` 输出以变体名前缀(如 `NotImplemented: ...`),便于:
//! 1. 用户/脚本通过 stderr grep 程序化识别错误类别
//! 2. `main` 函数根据变体映射退出码(Task 0.2)
//! 3. 集成测试断言特定错误标识(见 `tests/cli.rs` NotImplemented 用例)

use thiserror::Error;

/// Chimera CLI 错误类型
///
/// WHY:CLI 入口需要结构化错误码,便于 `main` 据此映射退出码、
/// 用户/脚本据 stderr 错误标识程序化处理。每个变体的 `Display`
/// 均以 `VariantName:` 前缀输出,保证 stderr 可 grep。
#[derive(Debug, Error)]
pub enum ChimeraCliError {
    /// 命令未实现 — 占位骨架未接入真实引擎
    ///
    /// WHY:v2.8.0-omega 前 4 个命令(run/quest/wiki/parliament)以 println
    /// 占位静默成功,误导用户以为功能已实装。改为显式 NotImplemented 错误,
    /// 错误信息指引替代方案(如 `chimera tui` / `chimera chat`)。
    /// 真实接入计划见 Phase 1(Task 1.1-1.4)。
    #[error("NotImplemented: {0}")]
    NotImplemented(String),

    /// 配置错误 — 配置文件解析失败或字段非法
    #[error("ConfigError: {0}")]
    ConfigError(String),

    /// 引擎错误 — QuestEngine / Parliament 等下游引擎返回错误
    #[error("EngineError: {0}")]
    EngineError(String),

    /// 用户取消 — 用户主动取消操作(如 Ctrl+C 或 permission prompt 拒绝)
    #[error("UserCancelled")]
    UserCancelled,

    /// 权限拒绝 — 操作未通过 permission prompt 或安全策略(对应 §6 红线:命令插值禁用)
    #[error("PermissionDenied: {0}")]
    PermissionDenied(String),

    /// 超时 — 操作超过规定时限(GQEP 聚集超时等)
    #[error("Timeout: {0}")]
    Timeout(String),

    /// IO 错误 — 文件读写等底层 IO 失败,自动从 `std::io::Error` 转换
    #[error("IoError: {0}")]
    IoError(#[from] std::io::Error),
}

/// 退出码语义档位 — 两套对外契约的唯一分流点。
///
/// WHY 两档而非一套数字：`main` 走 ADR-060 全局矩阵（0-6），
/// `exec` 走 WI-02 契约（0/2/3/4）。两者都是对外契约，不能合并；
/// 但必须共享同一变体分类，避免"改了一处忘了另一处"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitProfile {
    /// ADR-060 全局矩阵（main / 人类命令）：0-6。
    Global,
    /// WI-02 exec 契约（CI/管道）：0 成功 / 2 审批拒绝 / 3 预算耗尽 / 4 工具失败。
    Exec,
}

impl ChimeraCliError {
    /// 将错误变体映射到进程退出码(Task 0.2 ExitCode 矩阵)
    ///
    /// WHY:CLI 入口需要结构化退出码,便于 shell 脚本/CI 通过 `$?` 程序化
    /// 区分错误类别,而非笼统的"非零即失败"。映射严格遵循 ADR-060 矩阵:
    ///
    /// | 退出码 | 语义 | 对应变体 |
    /// |--------|------|----------|
    /// | 0 | success | (Ok 路径,不在此方法) |
    /// | 1 | user_error | ConfigError(配置非法,用户可纠正) |
    /// | 2 | not_implemented | NotImplemented(占位骨架,指引替代方案) |
    /// | 3 | system_error | EngineError / IoError(下游引擎或 IO 故障) |
    /// | 4 | user_cancelled | UserCancelled(Ctrl+C 或 prompt 拒绝) |
    /// | 5 | permission_denied | PermissionDenied(安全策略拒绝) |
    /// | 6 | timeout | Timeout(GQEP 聚集等超时) |
    ///
    /// WHY PermissionDenied=5 而非 1:`1` 表示"用户输入错误"(改参数即可重试),
    /// `5` 表示"权限策略拒绝"(需显式授权或修改安全配置),语义不同需区分,
    /// 对应 §6 红线"命令插值禁用"的安全审计场景。
    pub fn exit_code(&self) -> std::process::ExitCode {
        std::process::ExitCode::from(self.exit_code_value())
    }

    /// 返回退出码的 `u8` 原始值(Task 1.7 JSON 错误输出使用)
    ///
    /// WHY 单独提供此方法:`std::process::ExitCode` 不暴露底层值,
    /// 但 JSON 错误 envelope 需要 `"exit_code": <u8>` 字段供脚本程序化消费。
    /// `exit_code()` 委托到此方法,保持两路径一致(ADR-060 矩阵单一真相源)。
    pub fn exit_code_value(&self) -> u8 {
        match self {
            Self::NotImplemented(_) => 2,
            Self::ConfigError(_) => 1,
            Self::EngineError(_) => 3,
            Self::UserCancelled => 4,
            Self::PermissionDenied(_) => 5,
            Self::Timeout(_) => 6,
            Self::IoError(_) => 3,
        }
    }

    /// 返回指定语义档位的退出码。
    ///
    /// `Global` 委托 [`Self::exit_code_value`]（ADR-060 单一真值源）；
    /// `Exec` 实现 WI-02 的四类语义。新增错误变体时两个 match 都会
    /// 编译失败，强制同时决定两档映射。
    pub fn exit_code_for(&self, profile: ExitProfile) -> u8 {
        match profile {
            ExitProfile::Global => self.exit_code_value(),
            // 逐变体穷尽（**不得**用 `_ =>` 兜底）：新增错误变体时，本 match 与
            // `exit_code_value` 会**同时**编译失败，强制同时决定两档映射。
            // WI-02 四类语义：2 审批拒绝 / 3 引擎故障 / 4 其余（含工具超时）；
            // 未列出的变体归 4 是**明确列举**而非静默默认。
            ExitProfile::Exec => match self {
                Self::PermissionDenied(_) => 2,
                Self::EngineError(_) => 3,
                Self::NotImplemented(_)
                | Self::ConfigError(_)
                | Self::UserCancelled
                | Self::Timeout(_)
                | Self::IoError(_) => 4,
            },
        }
    }

    /// 返回错误变体名作为 `&'static str`(Task 1.7 JSON 错误输出的 `kind` 字段)
    ///
    /// 用于 `--json` 模式下结构化错误输出:
    /// `{ "status": "error", "error": { "kind": "NotImplemented", "message": "..." }, "exit_code": 2 }`
    ///
    /// WHY 静态字符串而非 `Display`:JSON 的 `kind` 字段应是稳定的程序化标识符
    /// (便于脚本 `jq .error.kind` 匹配),不应包含错误详情(详情在 `message` 字段)。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotImplemented(_) => "NotImplemented",
            Self::ConfigError(_) => "ConfigError",
            Self::EngineError(_) => "EngineError",
            Self::UserCancelled => "UserCancelled",
            Self::PermissionDenied(_) => "PermissionDenied",
            Self::Timeout(_) => "Timeout",
            Self::IoError(_) => "IoError",
        }
    }

    /// 返回错误详情消息(Task 1.7 JSON 错误输出的 `message` 字段)
    ///
    /// 去除变体名前缀(前缀已在 `kind` 字段),仅保留错误详情文本。
    pub fn message(&self) -> String {
        match self {
            Self::NotImplemented(m) => m.clone(),
            Self::ConfigError(m) => m.clone(),
            Self::EngineError(m) => m.clone(),
            Self::UserCancelled => "用户取消操作".to_string(),
            Self::PermissionDenied(m) => m.clone(),
            Self::Timeout(m) => m.clone(),
            Self::IoError(e) => e.to_string(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// 7 个变体 × Global 退出码的精确矩阵（ADR-060 不变量）。
    ///
    /// WHY 精确数值而非 Debug 字符串：旧测试只断言 `Debug` 含 "ExitCode"，
    /// 任何数值都能通过，等于没有护栏。
    fn matrix() -> Vec<(ChimeraCliError, u8)> {
        vec![
            (ChimeraCliError::NotImplemented("test".into()), 2),
            (ChimeraCliError::ConfigError("test".into()), 1),
            (ChimeraCliError::EngineError("test".into()), 3),
            (ChimeraCliError::UserCancelled, 4),
            (ChimeraCliError::PermissionDenied("test".into()), 5),
            (ChimeraCliError::Timeout("test".into()), 6),
            (
                ChimeraCliError::IoError(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "missing file",
                )),
                3,
            ),
        ]
    }

    #[test]
    fn test_exit_code_matrix_complete() {
        for (err, expected) in matrix() {
            assert_eq!(
                err.exit_code_value(),
                expected,
                "{} 的 Global 退出码应为 {}",
                err.kind(),
                expected
            );
        }
    }

    /// Exec profile 的 7 个变体映射必须精确（WI-02 契约）。
    #[test]
    fn test_exit_profile_exec_matrix() {
        let expected_exec = [4, 4, 3, 4, 2, 4, 4];
        for ((err, _), expected) in matrix().into_iter().zip(expected_exec) {
            assert_eq!(
                err.exit_code_for(ExitProfile::Exec),
                expected,
                "{} 的 Exec 退出码应为 {}",
                err.kind(),
                expected
            );
        }
    }

    /// Global profile 必须与既有 `exit_code_value` 完全一致（零行为变化）。
    #[test]
    fn test_exit_profile_global_delegates() {
        for (err, expected) in matrix() {
            assert_eq!(err.exit_code_for(ExitProfile::Global), expected);
            assert_eq!(
                err.exit_code_for(ExitProfile::Global),
                err.exit_code_value()
            );
        }
    }

    /// IoError 归类为 system_error(退出码 3),与 EngineError 同语义
    #[test]
    fn test_io_error_exit_code_is_system_error() {
        let err = ChimeraCliError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing file",
        ));
        assert_eq!(err.exit_code_value(), 3);
    }
}
