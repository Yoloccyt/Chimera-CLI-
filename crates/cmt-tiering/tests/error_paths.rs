//! CMT 分层存储错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 20.5:补充错误路径测试
//!
//! # 测试覆盖
//! 1. I/O 失败:ColdTier::open 在无效目录时返回 StorageError
//! 2. 无效配置:warm_db_path 为空时校验失败
//! 3. 无效配置:cold_dir 为空时校验失败
//! 4. 错误转换:std::io::Error → CmtError::StorageError
//! 5. 错误转换:serde_json::Error → CmtError::StorageError

#![forbid(unsafe_code)]

use cmt_tiering::{CmtConfig, CmtError, ColdTier};

/// I/O 失败:ColdTier::open 在父目录不存在时返回 StorageError
///
/// WHY:ColdTier::open 使用 ATTACH DATABASE 附加文件数据库,
/// 父目录不存在时 SQLite 返回 SQLITE_CANTOPEN。
/// 验证此错误路径确保调用方能正确处理文件系统故障。
#[test]
fn test_cold_open_parent_dir_not_exist() {
    // 构造一个父目录不存在的路径
    let invalid_dir = std::env::temp_dir()
        .join("cmt_nonexistent_parent_dir_for_error_test")
        .join("subdir");
    let result = ColdTier::open(&invalid_dir, 1024);
    let err = match result {
        Ok(_) => panic!("父目录不存在时应返回错误,而非静默成功"),
        Err(e) => e,
    };
    assert!(
        matches!(err, CmtError::StorageError(_)),
        "应为 StorageError,实际: {err:?}"
    );
}

/// 无效配置:warm_db_path 为空字符串时校验失败
///
/// WHY:空路径会导致 SQLite 在当前目录创建无名文件,属于配置失误。
/// validate() 应在系统边界拦截此错误。
#[test]
fn test_config_validate_empty_warm_db_path() {
    let config = CmtConfig::new().with_warm_db_path("");
    let err = config.validate().unwrap_err();
    assert!(matches!(err, CmtError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("warm_db_path"),
        "错误信息应包含字段名"
    );
}

/// 无效配置:cold_dir 为空字符串时校验失败
///
/// WHY:Cold 层使用文件目录存储,空路径无意义。
/// validate() 应在系统边界拦截此错误。
#[test]
fn test_config_validate_empty_cold_dir() {
    let config = CmtConfig::new().with_cold_dir("");
    let err = config.validate().unwrap_err();
    assert!(matches!(err, CmtError::InvalidConfig(_)));
    assert!(err.to_string().contains("cold_dir"), "错误信息应包含字段名");
}

/// 错误转换:std::io::Error → CmtError::StorageError
///
/// WHY:Ice 层使用文件存储,文件 I/O 错误需正确转换为 CmtError。
/// 调方可按 CmtError 统一处理,无需关心底层 std::io 错误。
#[test]
fn test_error_conversion_from_io_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let cmt_err: CmtError = io_err.into();
    assert!(
        matches!(cmt_err, CmtError::StorageError(_)),
        "io::Error 应转换为 StorageError"
    );
    assert!(
        cmt_err.to_string().contains("文件 I/O 错误"),
        "错误信息应包含 I/O 错误描述"
    );
}

/// 错误转换:serde_json::Error → CmtError::StorageError
///
/// WHY:CMT 条目通过 JSON 序列化存储到 SQLite,序列化失败需正确转换。
/// CMT 将 JSON 错误归为 StorageError(而非单独变体),避免变体过多。
#[test]
fn test_error_conversion_from_serde_json() {
    let json_err = serde_json::from_str::<String>("not a valid string").unwrap_err();
    let cmt_err: CmtError = json_err.into();
    assert!(
        matches!(cmt_err, CmtError::StorageError(_)),
        "serde_json::Error 应转换为 StorageError"
    );
    assert!(
        cmt_err.to_string().contains("JSON 序列化失败"),
        "错误信息应包含 JSON 序列化失败描述"
    );
}
