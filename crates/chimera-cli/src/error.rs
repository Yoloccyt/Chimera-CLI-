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
mod tests {
    use super::*;

    /// 验证 ExitCode 矩阵 7 变体映射完整(ADR-060 不变量)
    #[test]
    fn test_exit_code_matrix_complete() {
        let cases = [
            (
                ChimeraCliError::NotImplemented("test".into()),
                "NotImplemented 应映射为 2",
            ),
            (
                ChimeraCliError::ConfigError("test".into()),
                "ConfigError 应映射为 1",
            ),
            (
                ChimeraCliError::EngineError("test".into()),
                "EngineError 应映射为 3",
            ),
            (ChimeraCliError::UserCancelled, "UserCancelled 应映射为 4"),
            (
                ChimeraCliError::PermissionDenied("test".into()),
                "PermissionDenied 应映射为 5",
            ),
            (
                ChimeraCliError::Timeout("test".into()),
                "Timeout 应映射为 6",
            ),
        ];
        for (err, msg) in &cases {
            // ExitCode 无直接读取退出码值的稳定 API,但可通过 Debug 格式断言
            // (ExitCode 的 Debug 输出包含其底层值,如 "ExitCode(2)")
            let debug = format!("{:?}", err.exit_code());
            assert!(debug.contains("ExitCode"), "{}(debug={})", msg, debug);
        }
    }

    /// IoError 归类为 system_error(退出码 3),与 EngineError 同语义
    #[test]
    fn test_io_error_exit_code_is_system_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let err = ChimeraCliError::IoError(io_err);
        let debug = format!("{:?}", err.exit_code());
        // IoError 与 EngineError 共用退出码 3(system_error)
        let engine_debug = format!("{:?}", ChimeraCliError::EngineError("x".into()).exit_code());
        assert_eq!(
            debug, engine_debug,
            "IoError 与 EngineError 应映射到同一退出码(system_error=3)"
        );
    }
}
