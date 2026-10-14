//! 检查点契约 — L0 共享的 Quest 断点恢复快照(Task 3.10,ADR-033 扩展)
//!
//! 对应架构层: **L0 Contracts**(从 L1 `nexus-core` 上提,缓解 L1 上帝 crate)
//! 对应 ADR: **ADR-033**(L0 nexus-contracts 契约层建立,本模块为 Task 3.10 类型扩展)
//!
//! # 核心职责
//!
//! 承载 Quest 执行状态的持久化快照类型,用于 LHQP(Long-Horizon Quest Persistence)
//! 断点恢复。原定义于 `nexus-core/src/types.rs`,被 42+ 文件依赖(L1 上帝 crate 病理),
//! 下沉到 L0 共享契约层,供 L1-L10 所有上层 crate 直接导入。
//!
//! # 设计约束(ADR-033 + Task 3.10 扩展)
//!
//! - **纯类型 + 基础构造函数**: 仅类型定义与 `new()` 构造函数,不含业务逻辑
//! - **新增 ADR-033 例外**: `chrono` 作为基础类型库加入 L0 依赖白名单
//!   (与 `serde` 同级例外,无运行时业务逻辑)
//! - **向后兼容**: `nexus-core/src/types.rs` 保留 `pub use nexus_contracts::Checkpoint`
//!   re-export,42+ 文件现有 `use nexus_core::types::Checkpoint` 路径不破坏
//!
//! # 字段说明
//!
//! - `serialized_state`: 存储 MessagePack 序列化的 Quest 状态(版本无关持久化),
//!   而非直接存储 Quest 结构,以支持版本演进(字段增减不破坏旧检查点)
//! - `memory_snapshot_hash`: SHA-256 hex,恢复时校验完整性,防止状态漂移

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 检查点 — Quest 执行状态的持久化快照
///
/// WHY:`serialized_state` 存储 MessagePack 序列化的 Quest 状态,
/// 而非直接存储 Quest 结构,以支持版本演进(字段增减不破坏旧检查点)。
/// `memory_snapshot_hash` 用于恢复时校验完整性,防止状态漂移。
/// `description` 为可选人类可读摘要,用于检查点列表展示与恢复前确认。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Checkpoint {
    /// 所属 Quest ID
    pub quest_id: String,
    /// 检查点唯一标识
    pub checkpoint_id: String,
    /// 记忆快照哈希(SHA-256 hex),恢复时校验完整性
    pub memory_snapshot_hash: String,
    /// MessagePack 序列化的 Quest 状态(版本无关的持久化表示)
    pub serialized_state: Vec<u8>,
    /// 创建时间(UTC,自动生成)
    pub created_at: DateTime<Utc>,
    /// 可选人类可读摘要,用于检查点列表展示与恢复前确认
    /// #[serde(default)] 确保旧格式(MessagePack/JSON)缺失此字段时反序列化不失败
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 持久化 schema 版本(ADR-213 R7 的 F3 项)。
    ///
    /// `0` 表示"本字段加入前的历史形态",且 0 时**整字段不写入** ⇒ 当前所有编码字节与
    /// 加字段前逐字相同,老档案无需迁移即可回读(`#[serde(default)]` 补齐缺失键/尾元素)。
    /// 将来需要不兼容演进时写 1,读取侧按 `schema_version` 分支——这是"共存式"版本化,
    /// 刻意不学 `mca-gateway/spec_loader.rs` 的 fail-closed(那会让全部历史 checkpoint 变砖)。
    #[serde(default, skip_serializing_if = "is_schema_version_unset")]
    pub schema_version: u16,
}

/// 版本号为 0(未标注)时省略该字段,保证 wire 字节与加字段前完全一致。
///
/// 标 `pub(crate)`:ADR-213 可见性约定——契约层内部辅助项默认不进入公开 API,
/// 供本 crate 各持久化 envelope 模块共用(其余模块经 `crate::core_domain::checkpoint::` 引用)。
pub(crate) fn is_schema_version_unset(v: &u16) -> bool {
    *v == 0
}

impl Checkpoint {
    /// 创建新检查点,`created_at` 自动设为当前 UTC 时间,`description` 为 None
    pub fn new(
        quest_id: impl Into<String>,
        checkpoint_id: impl Into<String>,
        memory_snapshot_hash: impl Into<String>,
        serialized_state: Vec<u8>,
    ) -> Self {
        Self {
            quest_id: quest_id.into(),
            checkpoint_id: checkpoint_id.into(),
            memory_snapshot_hash: memory_snapshot_hash.into(),
            serialized_state,
            created_at: Utc::now(),
            description: None,
            schema_version: 0,
        }
    }

    /// 创建带描述摘要的检查点
    pub fn with_description(
        quest_id: impl Into<String>,
        checkpoint_id: impl Into<String>,
        memory_snapshot_hash: impl Into<String>,
        serialized_state: Vec<u8>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            quest_id: quest_id.into(),
            checkpoint_id: checkpoint_id.into(),
            memory_snapshot_hash: memory_snapshot_hash.into(),
            serialized_state,
            created_at: Utc::now(),
            description: Some(description.into()),
            schema_version: 0,
        }
    }
}
