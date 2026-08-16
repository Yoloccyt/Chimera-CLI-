//! Token 级证据契约 — Dressage 融合（设计文档 §5.3）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §5.3
//! 对应论文: 微软 OpenForge/Dressage（Token 级证据 + Segment-aware 训练）
//!
//! # 核心职责
//!
//! 承载 v4.0 RL 训练所需的 token 级证据与轨迹分段元数据：
//!
//! | 类型 | 职责 | 消费层 |
//! |------|------|--------|
//! | [`TokenLedgerEntry`] | 单次模型调用的 token 级证据（input/output IDs + logprobs + mask） | L1 event-bus token-ledger / 训练导出 |
//! | [`ToolCallRecord`] | 工具调用记录（名称/参数/结果/耗时） | L1 token-ledger 内嵌 |
//! | [`SegmentMetadata`] | 轨迹分段元数据（共享父轨迹身份 + anchor 判定） | L1 event-bus segment-per / L7 segment-aware-validation |
//! | [`SegmentCreationReason`] | 分段创建原因（六类） | L1 segment-per |
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型零逻辑**: 仅类型定义 + 构造辅助（无 IO 无状态变更）
//! - **零 crate 依赖**: 仅 `serde` derive
//! - **f32 字段仅 `PartialEq`**: output_logprobs 为浮点字段，禁止 derive `Eq`/`Hash`
//! - **`Box<[T]>` 优化**: token ID 序列与快照为写后只读大载荷，用堆切片替代
//!   `Vec<T>`（省 8 bytes/字段 + 语义明确"定长只读"）
//! - **铁律9 红线**: `parent_traj_id` 为同一轨迹全部分段共享的身份键，
//!   `is_anchor` 为 true 的分段承载终局 reward——这两项**不可篡改**
//!   （`SegmentMetadata` 不提供变更方法，构造后即冻结）

use serde::{Deserialize, Serialize};

// ============================================================
// 工具调用记录
// ============================================================

/// 工具调用记录 — TokenLedgerEntry 内嵌的工具执行证据
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// 工具名称（如 "read_file" / "bash"）
    pub tool_name: Box<str>,
    /// 调用参数（JSON 字符串形态，L0 不解析）
    pub arguments: Box<str>,
    /// 执行结果（文本形态）
    pub result: Box<str>,
    /// 执行耗时（毫秒）
    pub latency_ms: u32,
}

impl ToolCallRecord {
    /// 创建工具调用记录
    pub fn new(tool_name: &str, arguments: &str, result: &str, latency_ms: u32) -> Self {
        Self {
            tool_name: Box::from(tool_name),
            arguments: Box::from(arguments),
            result: Box::from(result),
            latency_ms,
        }
    }
}

// ============================================================
// Token 账本条目
// ============================================================

/// TokenLedgerEntry — 单次模型调用的 token 级证据（Dressage 核心）
///
/// 记录推理所需的全部 token 证据：输入/输出 token ID 序列、输出 logprobs、
/// 损失掩码与权重版本。**绝对红线**: "Token Ledger 不可丢失（训练证据完整性）"
/// ——本类型为不可变承载，由 L1 token-ledger 负责持久化（本地 WAL + 远程备份）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenLedgerEntry {
    /// 账本条目 ID（约定 UUIDv7）
    pub entry_id: Box<str>,
    /// 回合序号（轨迹内自增）
    pub turn_id: u32,
    /// 会话 ID
    pub session_id: Box<str>,
    /// 实例 ID（分布式训练/评测标识）
    pub instance_id: Box<str>,
    /// 输入 token ID 序列（定长只读）
    pub input_token_ids: Box<[u32]>,
    /// 输出 token ID 序列（定长只读）
    pub output_token_ids: Box<[u32]>,
    /// 输出 token 对数概率（与 output_token_ids 等长）
    pub output_logprobs: Box<[f32]>,
    /// 损失掩码（true = 参与损失计算；与 output_token_ids 等长）
    pub loss_mask: Box<[bool]>,
    /// 模型权重版本（训练证据溯源）
    pub weight_version: Box<str>,
    /// 工具调用记录（无工具调用时为空）
    pub tool_calls: Vec<ToolCallRecord>,
    /// MoE 专家路由矩阵（如不可得为 None）
    pub moe_routing: Option<Vec<Vec<u32>>>,
    /// 时间戳（Unix 毫秒）
    pub timestamp: u64,
}

impl TokenLedgerEntry {
    /// 创建 Token 账本条目
    ///
    /// # Panics
    ///
    /// 当 `output_token_ids` / `output_logprobs` / `loss_mask` 三者长度不一致时
    /// panic ——证据完整性不变量：三序列必须一一对应（消费方训练逻辑依赖）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        entry_id: &str,
        turn_id: u32,
        session_id: &str,
        instance_id: &str,
        input_token_ids: Vec<u32>,
        output_token_ids: Vec<u32>,
        output_logprobs: Vec<f32>,
        loss_mask: Vec<bool>,
        weight_version: &str,
        tool_calls: Vec<ToolCallRecord>,
        moe_routing: Option<Vec<Vec<u32>>>,
        timestamp: u64,
    ) -> Self {
        assert_eq!(
            output_token_ids.len(),
            output_logprobs.len(),
            "TokenLedgerEntry 证据不变量: output_token_ids 与 output_logprobs 必须等长"
        );
        assert_eq!(
            output_token_ids.len(),
            loss_mask.len(),
            "TokenLedgerEntry 证据不变量: output_token_ids 与 loss_mask 必须等长"
        );
        Self {
            entry_id: Box::from(entry_id),
            turn_id,
            session_id: Box::from(session_id),
            instance_id: Box::from(instance_id),
            input_token_ids: input_token_ids.into_boxed_slice(),
            output_token_ids: output_token_ids.into_boxed_slice(),
            output_logprobs: output_logprobs.into_boxed_slice(),
            loss_mask: loss_mask.into_boxed_slice(),
            weight_version: Box::from(weight_version),
            tool_calls,
            moe_routing,
            timestamp,
        }
    }

    /// 输出 token 数量（证据完整性便捷访问）
    pub fn output_len(&self) -> usize {
        self.output_token_ids.len()
    }

    /// 是否包含工具调用证据
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

// ============================================================
// 分段创建原因
// ============================================================

/// 分段创建原因 — 轨迹分段的六类触发条件（Dressage）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentCreationReason {
    /// 历史压缩 — 上下文超窗触发压缩
    HistoryCompaction,
    /// 工具 schema 变更 — 工具定义变化导致分段
    ToolSchemaChange,
    /// 消息重写 — 消息内容被改写
    MessageRewrite,
    /// TITO 回退 — Token-In/Token-Out 一致性回退
    Titofallback,
    /// 自然边界 — 任务阶段切换的自然分界
    NaturalBoundary,
    /// 达到最大长度 — 上下文长度上限触发
    MaxLengthReached,
}

// ============================================================
// 分段元数据
// ============================================================

/// SegmentMetadata — 轨迹分段元数据（Dressage Segment-aware 训练）
///
/// 同一父轨迹的全部 segment **共享** `parent_traj_id`（铁律9）；
/// 仅 `is_anchor` 为 true 的 segment 承载终局 reward。
/// 构造后不可变——分段身份一旦创建不可篡改（绝对红线）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentMetadata {
    /// 分段 ID（约定 UUIDv7）
    pub segment_id: Box<str>,
    /// 父轨迹 ID — 同一轨迹全部分段共享（**不可篡改**）
    pub parent_traj_id: Box<str>,
    /// 分段序号（轨迹内自增，从 0 起）
    pub segment_index: u32,
    /// 是否 anchor segment（承载终局 reward）
    pub is_anchor: bool,
    /// 关联的 TokenLedgerEntry IDs（证据链）
    pub token_entries: Vec<Box<str>>,
    /// 上下文快照（MessagePack 序列化形态，定长只读）
    pub context_snapshot: Box<[u8]>,
    /// 起始回合序号（含）
    pub start_turn: u32,
    /// 结束回合序号（含）
    pub end_turn: u32,
    /// 分段创建原因
    pub creation_reason: SegmentCreationReason,
}

impl SegmentMetadata {
    /// 创建分段元数据
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        segment_id: &str,
        parent_traj_id: &str,
        segment_index: u32,
        is_anchor: bool,
        token_entries: Vec<Box<str>>,
        context_snapshot: Vec<u8>,
        start_turn: u32,
        end_turn: u32,
        creation_reason: SegmentCreationReason,
    ) -> Self {
        assert!(
            end_turn >= start_turn,
            "SegmentMetadata 不变量: end_turn ({end_turn}) 必须 >= start_turn ({start_turn})"
        );
        Self {
            segment_id: Box::from(segment_id),
            parent_traj_id: Box::from(parent_traj_id),
            segment_index,
            is_anchor,
            token_entries,
            context_snapshot: context_snapshot.into_boxed_slice(),
            start_turn,
            end_turn,
            creation_reason,
        }
    }

    /// 是否为 anchor segment（承载终局 reward 的分段）
    pub fn is_anchor_segment(&self) -> bool {
        self.is_anchor
    }

    /// 分段回合跨度（回合数，含端点）
    pub fn turn_span(&self) -> u32 {
        self.end_turn - self.start_turn + 1
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- TokenLedgerEntry ----------

    fn sample_entry() -> TokenLedgerEntry {
        TokenLedgerEntry::new(
            "ledger-001",
            0,
            "session-1",
            "instance-1",
            vec![101, 102, 103],
            vec![201, 202],
            vec![0.9, 0.8],
            vec![true, true],
            "v2.26.0-omega",
            vec![ToolCallRecord::new("read_file", "{}", "内容", 12)],
            None,
            1_700_000_000_000,
        )
    }

    #[test]
    fn token_ledger_json_roundtrip() {
        let entry = sample_entry();
        let json = serde_json::to_string(&entry).expect("JSON 序列化失败");
        let decoded: TokenLedgerEntry = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, entry);
    }

    #[test]
    fn token_ledger_msgpack_roundtrip() {
        let entry = sample_entry();
        let bytes = rmp_serde::to_vec(&entry).expect("MsgPack 序列化失败");
        let decoded: TokenLedgerEntry =
            rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        assert_eq!(decoded, entry);
    }

    #[test]
    fn token_ledger_wire_format_frozen() {
        let entry = sample_entry();
        let json = serde_json::to_string(&entry).expect("JSON 序列化失败");
        assert!(json.contains("\"entry_id\":\"ledger-001\""));
        assert!(json.contains("\"turn_id\":0"));
        // 证据三序列等长已由构造器保证
        assert_eq!(entry.output_len(), 2);
        assert!(entry.has_tool_calls());
    }

    #[test]
    fn token_ledger_evidence_invariant_asserted() {
        // 证据完整性: 长度不一致必须 panic（训练依赖一一对应）
        let result = std::panic::catch_unwind(|| {
            TokenLedgerEntry::new(
                "x",
                0,
                "s",
                "i",
                vec![],
                vec![1, 2],
                vec![0.5],
                vec![true],
                "v",
                vec![],
                None,
                0,
            )
        });
        assert!(result.is_err(), "长度不一致必须触发断言 panic");
    }

    #[test]
    fn token_ledger_msgpack_smaller_than_json() {
        // 性能基准: MsgPack 二进制体积应显著小于 JSON（大载荷证据场景）
        let entry = sample_entry();
        let json_len = serde_json::to_string(&entry)
            .expect("JSON 序列化失败")
            .len();
        let msgpack_len = rmp_serde::to_vec(&entry).expect("MsgPack 序列化失败").len();
        assert!(
            msgpack_len < json_len,
            "MsgPack ({msgpack_len}B) 应小于 JSON ({json_len}B)"
        );
    }

    // ---------- SegmentMetadata ----------

    fn sample_segment() -> SegmentMetadata {
        SegmentMetadata::new(
            "seg-001",
            "traj-1",
            0,
            true,
            vec![Box::from("ledger-001")],
            vec![0x1F, 0x8B, 0x08],
            0,
            5,
            SegmentCreationReason::NaturalBoundary,
        )
    }

    #[test]
    fn segment_metadata_roundtrip() {
        let seg = sample_segment();
        let json = serde_json::to_string(&seg).expect("JSON 序列化失败");
        let decoded: SegmentMetadata = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(decoded, seg);
    }

    #[test]
    fn segment_anchor_semantics() {
        // 铁律9: anchor segment 承载终局 reward
        let anchor = sample_segment();
        assert!(anchor.is_anchor_segment());
        let non_anchor = SegmentMetadata::new(
            "seg-002",
            "traj-1",
            1,
            false,
            vec![],
            vec![],
            6,
            10,
            SegmentCreationReason::MaxLengthReached,
        );
        assert!(!non_anchor.is_anchor_segment());
    }

    #[test]
    fn segment_parent_traj_shared_identity() {
        // 铁律9: 同一轨迹的分段共享 parent_traj_id
        let seg1 = sample_segment();
        let seg2 = SegmentMetadata::new(
            "seg-002",
            "traj-1",
            1,
            false,
            vec![],
            vec![],
            6,
            10,
            SegmentCreationReason::HistoryCompaction,
        );
        assert_eq!(seg1.parent_traj_id, seg2.parent_traj_id);
        assert_eq!(seg1.parent_traj_id.as_ref(), "traj-1");
    }

    #[test]
    fn segment_turn_span_computation() {
        let seg = sample_segment();
        assert_eq!(seg.turn_span(), 6); // 0..=5 → 6 回合
    }

    #[test]
    fn segment_end_turn_invariant_asserted() {
        // 不变量: end_turn < start_turn 必须 panic
        let result = std::panic::catch_unwind(|| {
            SegmentMetadata::new(
                "bad",
                "traj-1",
                0,
                false,
                vec![],
                vec![],
                10,
                5,
                SegmentCreationReason::NaturalBoundary,
            )
        });
        assert!(result.is_err(), "end_turn < start_turn 必须触发断言 panic");
    }

    #[test]
    fn segment_creation_reasons_exhaustive() {
        // 六类分段原因闭集（编译期穷尽检查）
        let all = [
            SegmentCreationReason::HistoryCompaction,
            SegmentCreationReason::ToolSchemaChange,
            SegmentCreationReason::MessageRewrite,
            SegmentCreationReason::Titofallback,
            SegmentCreationReason::NaturalBoundary,
            SegmentCreationReason::MaxLengthReached,
        ];
        assert_eq!(all.len(), 6);
    }

    #[test]
    fn segment_wire_format_frozen() {
        let seg = sample_segment();
        let json = serde_json::to_string(&seg).expect("JSON 序列化失败");
        assert!(json.contains("\"parent_traj_id\":\"traj-1\""));
        assert!(json.contains("\"is_anchor\":true"));
        assert!(json.contains("\"creation_reason\":\"natural_boundary\""));
    }
}
