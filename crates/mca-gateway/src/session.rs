//! session — 会话状态守恒(P4)与多轮历史持久化
//!
//! # 状态守恒策略(P4,MiniMax M3 断链教训专项)
//! 凡厂商要求回传的中间态(thinking 块、tool_use 块),由描述符声明
//! `StatePreservationPolicy`,会话层据此决定回传时保留哪些块:
//! - `None`:剥离 thinking 块(省 token),保留 text/tool_use/tool_result
//! - `BlockPreservation`:全部块原序保留(Anthropic 路径通用,Kimi K3 要求)
//! - `VerbatimThinking`:同 BlockPreservation,**且承诺 thinking 内容逐字保真、
//!   禁止任何 strip/转换**(MiniMax interleaved thinking,strip 即断链,C9)
//!
//! # 跨通道迁移(§5.5 D7)
//! 通道切换时按新通道策略转译/保真/安全丢弃思考块并留痕:降级到 `None`
//! 通道时思考块被安全丢弃(计数留痕),升级不臆造内容。
//!
//! # 持久化(C7 红线)
//! `SessionStore` 复刻 `scc-cache/src/wal.rs` 的 `Arc<Mutex<Connection>>` +
//! `spawn_blocking` 范式:rusqlite 非 async,所有 SQLite 调用走阻塞线程池,
//! 绝不阻塞 async runtime;checkpoint 只在 ToolCallEnd/Done 边界落库,
//! 不做 per-delta 写(避免流式输出被 SQLite 写阻塞)。

use std::sync::{Arc, Mutex};

use nexus_contracts::affinity::{
    AffinityMessage, ContentBlock, MessageRole, StatePreservationPolicy,
};
use tokio::task::spawn_blocking;

use crate::error::AffinityError;

// ============================================================
// 状态守恒策略逻辑(纯函数,E5 哨兵校验的核心)
// ============================================================

/// 迁移结果 — 迁移后的历史 + 被丢弃的思考块计数(留痕)
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationResult {
    /// 按目标策略处理后的消息历史
    pub messages: Vec<AffinityMessage>,
    /// 被安全丢弃的 thinking 块数量(降级留痕,驱动 E4 明确告知)
    pub dropped_thinking_blocks: usize,
}

/// 应用状态守恒策略到 assistant 消息块 — 决定回传时保留哪些块
///
/// # 逐字保真保证(WHY VerbatimThinking 与 BlockPreservation 都保留全部块)
/// 两者行为上都保留全部块原序;区别是**语义承诺**:VerbatimThinking 通道
/// 禁止调用方在任何环节 strip/改写 thinking 内容(MiniMax 断链红线),
/// 由本函数返回 `blocks.to_vec()` 保证零转换,克隆即逐字副本。
pub fn apply_preservation_policy(
    blocks: &[ContentBlock],
    policy: StatePreservationPolicy,
) -> Vec<ContentBlock> {
    match policy {
        // 无状态:丢弃 thinking 块(省 token),其余原序保留
        StatePreservationPolicy::None => blocks
            .iter()
            .filter(|b| !matches!(b, ContentBlock::Thinking { .. }))
            .cloned()
            .collect(),
        // 块保真 / 逐字保真:全部块原序保留(零转换,克隆即逐字副本)
        StatePreservationPolicy::BlockPreservation | StatePreservationPolicy::VerbatimThinking => {
            blocks.to_vec()
        }
    }
}

/// 统计一条消息中的 thinking 块数量
fn count_thinking(msg: &AffinityMessage) -> usize {
    msg.blocks
        .iter()
        .filter(|b| matches!(b, ContentBlock::Thinking { .. }))
        .count()
}

/// 跨通道迁移会话历史 — 按目标通道策略处理每条 assistant 消息并留痕
///
/// 只处理 Assistant 消息(思考/工具块守恒的载体);System/User/Tool 消息
/// 原样保留。降级到 `None` 时丢弃思考块并累计计数(留痕,供 E4 告知)。
pub fn migrate_history(
    history: &[AffinityMessage],
    to_policy: StatePreservationPolicy,
) -> MigrationResult {
    let mut messages = Vec::with_capacity(history.len());
    let mut dropped = 0usize;
    for msg in history {
        if msg.role == MessageRole::Assistant {
            let before = count_thinking(msg);
            let kept_blocks = apply_preservation_policy(&msg.blocks, to_policy);
            let after = kept_blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Thinking { .. }))
                .count();
            dropped += before - after;
            messages.push(AffinityMessage {
                role: msg.role,
                blocks: kept_blocks,
            });
        } else {
            messages.push(msg.clone());
        }
    }
    MigrationResult {
        messages,
        dropped_thinking_blocks: dropped,
    }
}

// ============================================================
// SQLite 会话存储(spawn_blocking,C7 红线)
// ============================================================

/// 会话消息持久化存储 — 多轮历史落库,支持跨进程会话恢复
///
/// Clone 廉价(`Arc<Mutex>` 引用计数),可跨任务共享(对齐 SqliteWal 惯例)。
#[derive(Clone)]
pub struct SessionStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl SessionStore {
    /// 打开(或创建)会话存储(async,内部 spawn_blocking)
    ///
    /// path 传 ":memory:" 得进程内存库(测试用);传文件路径得持久库。
    pub async fn open(path: impl Into<String> + Send + 'static) -> Result<Self, AffinityError> {
        let path = path.into();
        let conn = spawn_blocking(move || -> Result<rusqlite::Connection, AffinityError> {
            let conn = rusqlite::Connection::open(&path).map_err(|e| AffinityError::Unknown {
                raw: format!("open session db (path={path}): {e}"),
            })?;
            conn.pragma_update(None, "journal_mode", "WAL").ok();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS session_turns (
                    session_id  TEXT NOT NULL,
                    turn_index  INTEGER NOT NULL,
                    role        TEXT NOT NULL,
                    blocks_msgpack BLOB NOT NULL,
                    PRIMARY KEY (session_id, turn_index)
                );",
            )
            .map_err(|e| AffinityError::Unknown {
                raw: format!("init session schema: {e}"),
            })?;
            Ok(conn)
        })
        .await
        .map_err(|e| AffinityError::Unknown {
            raw: format!("spawn_blocking join error: {e}"),
        })??;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 追加一条会话消息(边界 checkpoint,spawn_blocking)
    ///
    /// turn_index 由调用方保证单调递增(会话内唯一);blocks 以 MessagePack
    /// 序列化(ADR-004)。锁在闭包内取用,不跨 await(C7)。
    pub async fn record_turn(
        &self,
        session_id: &str,
        turn_index: u64,
        msg: &AffinityMessage,
    ) -> Result<(), AffinityError> {
        let session_id = session_id.to_string();
        let role = role_str(msg.role).to_string();
        let blocks = rmp_serde::to_vec(&msg.blocks).map_err(|e| AffinityError::Unknown {
            raw: format!("serialize blocks: {e}"),
        })?;
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || -> Result<(), AffinityError> {
            let guard = conn.lock().map_err(|_| AffinityError::Unknown {
                raw: "session db mutex poisoned".into(),
            })?;
            guard
                .execute(
                    "INSERT OR REPLACE INTO session_turns
                        (session_id, turn_index, role, blocks_msgpack)
                        VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![session_id, turn_index as i64, role, blocks],
                )
                .map_err(|e| AffinityError::Unknown {
                    raw: format!("insert turn: {e}"),
                })?;
            Ok(())
        })
        .await
        .map_err(|e| AffinityError::Unknown {
            raw: format!("spawn_blocking join error: {e}"),
        })?
    }

    /// 读取会话完整历史(按 turn_index 升序,spawn_blocking)
    pub async fn history(&self, session_id: &str) -> Result<Vec<AffinityMessage>, AffinityError> {
        let session_id = session_id.to_string();
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || -> Result<Vec<AffinityMessage>, AffinityError> {
            let guard = conn.lock().map_err(|_| AffinityError::Unknown {
                raw: "session db mutex poisoned".into(),
            })?;
            let mut stmt = guard
                .prepare(
                    "SELECT role, blocks_msgpack FROM session_turns
                        WHERE session_id = ?1 ORDER BY turn_index ASC",
                )
                .map_err(|e| AffinityError::Unknown {
                    raw: format!("prepare history: {e}"),
                })?;
            let rows = stmt
                .query_map(rusqlite::params![session_id], |row| {
                    let role: String = row.get(0)?;
                    let blocks: Vec<u8> = row.get(1)?;
                    Ok((role, blocks))
                })
                .map_err(|e| AffinityError::Unknown {
                    raw: format!("query history: {e}"),
                })?;
            let mut out = Vec::new();
            for row in rows {
                let (role, blocks) = row.map_err(|e| AffinityError::Unknown {
                    raw: format!("row decode: {e}"),
                })?;
                let blocks: Vec<ContentBlock> =
                    rmp_serde::from_slice(&blocks).map_err(|e| AffinityError::Unknown {
                        raw: format!("deserialize blocks: {e}"),
                    })?;
                out.push(AffinityMessage {
                    role: role_from_str(&role),
                    blocks,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| AffinityError::Unknown {
            raw: format!("spawn_blocking join error: {e}"),
        })?
    }
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStore").finish_non_exhaustive()
    }
}

/// 角色 → 稳定字符串(SQLite 列)
fn role_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

/// 字符串 → 角色(未知归 User,P3 容错;不应发生因写入受控)
fn role_from_str(s: &str) -> MessageRole {
    match s {
        "system" => MessageRole::System,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::Tool,
        _ => MessageRole::User,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn thinking(content: &str) -> ContentBlock {
        ContentBlock::Thinking {
            thinking: content.into(),
            signature: Some("sig".into()),
        }
    }
    fn text(content: &str) -> ContentBlock {
        ContentBlock::Text {
            text: content.into(),
        }
    }
    fn tool_use() -> ContentBlock {
        ContentBlock::ToolUse {
            id: "c1".into(),
            name: "read".into(),
            input_json: "{}".into(),
        }
    }

    #[test]
    fn none_policy_strips_thinking_keeps_rest() {
        let blocks = vec![thinking("推理"), text("答案"), tool_use()];
        let kept = apply_preservation_policy(&blocks, StatePreservationPolicy::None);
        assert_eq!(kept.len(), 2);
        assert!(!kept
            .iter()
            .any(|b| matches!(b, ContentBlock::Thinking { .. })));
        assert!(kept
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. })));
    }

    #[test]
    fn block_and_verbatim_preserve_all_in_order() {
        let blocks = vec![thinking("推理"), tool_use(), text("答案")];
        for policy in [
            StatePreservationPolicy::BlockPreservation,
            StatePreservationPolicy::VerbatimThinking,
        ] {
            let kept = apply_preservation_policy(&blocks, policy);
            assert_eq!(kept, blocks, "{policy:?} 必须原序保留全部块");
        }
    }

    #[test]
    fn migrate_downgrade_to_none_drops_and_counts() {
        // 从 VerbatimThinking 通道迁移到 None 通道:思考块安全丢弃并计数留痕
        let history = vec![
            AffinityMessage {
                role: MessageRole::User,
                blocks: vec![text("问题")],
            },
            AffinityMessage {
                role: MessageRole::Assistant,
                blocks: vec![thinking("t1"), text("a1"), thinking("t2")],
            },
        ];
        let result = migrate_history(&history, StatePreservationPolicy::None);
        assert_eq!(result.dropped_thinking_blocks, 2);
        // User 消息原样保留;Assistant 思考块被丢弃
        assert_eq!(result.messages[0].blocks.len(), 1);
        assert_eq!(result.messages[1].blocks.len(), 1);
    }

    #[test]
    fn migrate_to_verbatim_drops_nothing() {
        let history = vec![AffinityMessage {
            role: MessageRole::Assistant,
            blocks: vec![thinking("t1"), text("a1")],
        }];
        let result = migrate_history(&history, StatePreservationPolicy::VerbatimThinking);
        assert_eq!(result.dropped_thinking_blocks, 0);
        assert_eq!(result.messages[0].blocks.len(), 2);
    }

    #[tokio::test]
    async fn session_store_roundtrip_preserves_order_and_content() {
        let store = SessionStore::open(":memory:").await.unwrap();
        let sid = "sess-1";
        let turns = vec![
            AffinityMessage {
                role: MessageRole::User,
                blocks: vec![text("写快排")],
            },
            AffinityMessage {
                role: MessageRole::Assistant,
                blocks: vec![thinking("先分析"), tool_use()],
            },
        ];
        for (i, msg) in turns.iter().enumerate() {
            store.record_turn(sid, i as u64, msg).await.unwrap();
        }
        let history = store.history(sid).await.unwrap();
        assert_eq!(history, turns, "会话往返必须逐字保真且保序");
    }

    #[tokio::test]
    async fn session_store_isolates_by_session_id() {
        let store = SessionStore::open(":memory:").await.unwrap();
        store
            .record_turn(
                "a",
                0,
                &AffinityMessage {
                    role: MessageRole::User,
                    blocks: vec![text("A")],
                },
            )
            .await
            .unwrap();
        store
            .record_turn(
                "b",
                0,
                &AffinityMessage {
                    role: MessageRole::User,
                    blocks: vec![text("B")],
                },
            )
            .await
            .unwrap();
        assert_eq!(store.history("a").await.unwrap().len(), 1);
        assert_eq!(store.history("b").await.unwrap().len(), 1);
    }

    // E5 哨兵校验:VerbatimThinking 下任意思考内容(含哨兵)必须逐字幸存
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]
        #[test]
        fn e5_sentinel_survives_verbatim_preservation(
            content in "\\PC{0,200}"
        ) {
            // 注入哨兵包裹随机内容,模拟 MiniMax interleaved thinking
            let sentinel_content = format!("<<SENTINEL>>{content}<<SENTINEL>>");
            let blocks = vec![
                thinking(&sentinel_content),
                text("visible answer"),
            ];
            let kept = apply_preservation_policy(&blocks, StatePreservationPolicy::VerbatimThinking);
            // 断言:思考块逐字幸存(哨兵与内容零改动,strip 即视为断链)
            let survived = kept.iter().any(|b| matches!(
                b,
                ContentBlock::Thinking { thinking, .. } if thinking.as_ref() == sentinel_content
            ));
            prop_assert!(survived, "VerbatimThinking 下哨兵思考内容必须逐字幸存");
        }
    }
}
