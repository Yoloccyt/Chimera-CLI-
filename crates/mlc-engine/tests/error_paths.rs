//! MLC 引擎错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 20.5:补充错误路径测试
//!
//! # 测试覆盖
//! 1. I/O 失败:ProceduralMemory::open 在父目录不存在时返回 StorageError
//! 2. 无效配置:空 procedural_db_path 校验失败
//! 3. 无效输入:L2 语义记忆插入无 CLV 条目返回 InvalidConfig
//! 4. 边界溢出:l0_capacity 超过上界返回 InvalidConfig
//! 5. 错误转换:serde_json::Error → MlcError::SerializationFailed

#![forbid(unsafe_code)]

use mlc_engine::{MlcConfig, MlcError, ProceduralMemory, SemanticMemory};

/// I/O 失败:ProceduralMemory::open 在父目录不存在时返回 StorageError
///
/// WHY:SQLite 的 sqlite3_open 不会创建父目录,父目录缺失时返回 SQLITE_CANTOPEN。
/// 验证此错误路径确保调用方能正确处理文件系统故障。
#[test]
fn test_procedural_open_parent_dir_not_exist() {
    // 构造一个父目录不存在的路径(SQLite 不会自动创建父目录)
    let invalid_path = std::env::temp_dir()
        .join("mlc_nonexistent_parent_dir_for_error_test")
        .join("subdir")
        .join("procedural.db");

    let result = ProceduralMemory::open(&invalid_path);
    let err = match result {
        Ok(_) => panic!("父目录不存在时应返回错误,而非静默成功"),
        Err(e) => e,
    };
    assert!(
        matches!(err, MlcError::StorageError(_)),
        "应为 StorageError,实际: {err:?}"
    );
}

/// 无效配置:procedural_db_path 为空字符串时校验失败
///
/// WHY:空路径会导致 SQLite 在当前目录创建无名文件,属于配置失误。
/// validate() 应在系统边界拦截此错误。
#[test]
fn test_config_validate_empty_db_path() {
    let config = MlcConfig::new().with_procedural_db_path("");
    let err = config.validate().unwrap_err();
    assert!(matches!(err, MlcError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("procedural_db_path"),
        "错误信息应包含字段名"
    );
}

/// 无效输入:L2 语义记忆插入无 CLV 的条目返回 InvalidConfig
///
/// WHY:L2 语义记忆基于 CLV 做余弦相似度召回,无 CLV 的条目无法参与召回,
/// 应在插入时拒绝而非静默存储无效数据。
#[test]
fn test_semantic_insert_without_clv() {
    use mlc_engine::{MemoryEntry, MemoryTier};

    let memory = SemanticMemory::new(64);
    // 构造无 CLV 的条目(MemoryEntry::new 默认 clv = None)
    let entry = MemoryEntry::new("m-no-clv", "content", MemoryTier::L2Semantic);
    assert!(entry.clv.is_none(), "前置条件:条目应无 CLV");

    let result = memory.insert(entry);
    assert!(result.is_err(), "无 CLV 的条目应被拒绝");
    let err = result.unwrap_err();
    assert!(
        matches!(err, MlcError::InvalidConfig(_)),
        "应为 InvalidConfig,实际: {err:?}"
    );
}

/// 边界溢出:l0_capacity 超过上界(1024)时校验失败
///
/// WHY:L0 为 DashMap 内存缓存,过大会导致 OOM。
/// SubTask 14.7 新增上界校验,防止配置失误。
#[test]
fn test_config_validate_l0_capacity_exceeds_max() {
    // L0_CAPACITY_MAX = 1024,设置 1025 应失败
    let config = MlcConfig::new().with_l0_capacity(1025);
    let err = config.validate().unwrap_err();
    assert!(matches!(err, MlcError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("l0_capacity"),
        "错误信息应包含字段名"
    );
    assert!(err.to_string().contains("1024"), "错误信息应包含上界值");
}

/// 错误转换:serde_json::Error → MlcError::SerializationFailed
///
/// WHY:确保 JSON 序列化失败时正确转换为 MlcError,
/// 调方可按 MlcError 统一处理,无需关心底层 serde_json 错误。
#[test]
fn test_error_conversion_from_serde_json() {
    let json_err = serde_json::from_str::<String>("not a valid string").unwrap_err();
    let mlc_err: MlcError = json_err.into();
    assert!(
        matches!(mlc_err, MlcError::SerializationFailed(_)),
        "serde_json::Error 应转换为 SerializationFailed"
    );
}
