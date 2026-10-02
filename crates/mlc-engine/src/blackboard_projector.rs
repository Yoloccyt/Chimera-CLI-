//! 黑板→MLC 协作态投影器 — `BlackboardEntry` 写入落记忆索引（v2.3 §(b.2)）
//!
//! 对应架构层: L2 Memory（mlc-engine；依赖 event-bus L1 为**向下合法**，L(N)→L(N-1)）
//! 对应设计源: `Chimera架构重构与优化方案_v2.3` §(b.2)「BlackboardEntry(broadcast)：
//! 写入落 MLC 索引（复用 ADR-192 分层检索），带 TTL + 认识类型标签；黑板=MLC 协作态投影」
//!
//! # WHY
//! 黑板事件是跨 agent 协作态的广播投影。§(b.2) 要求它**落成记忆索引**而非仅瞬态广播。
//! 本投影器把 `NexusEvent::BlackboardEntry` 映射为 `MemoryEntry` 并写入 L1 情节索引
//! （时间索引 + 可选 Quest 索引），L1 满则经既有 FIFO 驱逐路径降级至 L2，从而**复用
//! ADR-192 分层检索**——协作态随时间自然沉淀进语义层。
//!
//! # 投影边界（§(b.2)「边界写 ADR」的落点）
//! - **存储层选择 = L1Episodic**：`BlackboardEntry` 仅带 `summary` 文本、无 CLV，
//!   而 L2 语义召回要求向量；L1 情节层按时间索引、`clv` 可选，契合「协作态即时投影 +
//!   后续降级入 L2」的分层沉淀路径。故此处不臆造 embedding（留给压测后的 ADR-132/
//!   转正决策），仅做**文本投影**（诚实、无虚假向量）。
//! - **TTL 编码**：黑板 `ttl_secs` → `TemporalMeta.valid_until = now + ttl`，令幽灵
//!   记忆治理（HCW-Sparse 时间过滤）自然裁剪过期协作态（复用既有时间有效性维）。
//! - **认识类型标签**：`epistemic` 字符串标签解析为 [`EpistemicType`]，未知回退 `Fact`
//!   （向后兼容，不 panic）。
//! - **去重键**：条目 id = `bb:{key}`，同 `key` 再投影即覆盖（幂等，检索/去重按 key）。

use chrono::{DateTime, Utc};
use event_bus::NexusEvent;
use nexus_contracts::{EpistemicType, TemporalMeta};

use crate::error::MlcError;
use crate::l1_episodic::EpisodicMemory;
use crate::types::{MemoryEntry, MemoryId, MemoryTier};

/// 解析黑板事件的认识类型标签（大小写不敏感）；未知/缺省回退 [`EpistemicType::Fact`]。
#[must_use]
pub fn parse_epistemic(label: &str) -> EpistemicType {
    match label.to_ascii_lowercase().replace(['_', ' '], "").as_str() {
        "episode" => EpistemicType::Episode,
        "observation" => EpistemicType::Observation,
        "mindmodel" => EpistemicType::MindModel,
        _ => EpistemicType::Fact,
    }
}

/// 纯映射：`BlackboardEntry` → [`MemoryEntry`]（tier=L1Episodic，TTL→valid_until）。
///
/// 非该变体返回 `None`（no-op，供投影器在混合事件流中过滤）。投影内容 = `summary`，
/// id = `bb:{key}`，认识标签与 `provenance` 透传。
#[must_use]
pub fn blackboard_to_entry(event: &NexusEvent, now: DateTime<Utc>) -> Option<MemoryEntry> {
    let NexusEvent::BlackboardEntry {
        key,
        summary,
        ttl_secs,
        epistemic,
        provenance,
        ..
    } = event
    else {
        return None;
    };
    let mut entry = MemoryEntry::new(format!("bb:{key}"), summary.clone(), MemoryTier::L1Episodic);
    entry.epistemic_type = parse_epistemic(epistemic);
    entry.provenance = provenance.clone();
    // TTL 存活时长 → 有效期终点；valid_from=now，置信 1.0（协作态即时事实）。
    let valid_from = now.timestamp();
    let mut meta = TemporalMeta::new(valid_from, 1.0);
    meta.valid_until = valid_from.checked_add(i64::try_from(*ttl_secs).unwrap_or(i64::MAX));
    entry.temporal_meta = Some(meta);
    Some(entry)
}

/// 投影入口：把一条 `BlackboardEntry` 事件写入 L1 情节记忆索引。
///
/// 返回被投影条目的 id（非黑板事件返回 `Ok(None)`，不写库）。L1 满时经 `insert`
/// 的 FIFO 驱逐路径返回被降级条目（此处只关心投影是否落库，被驱逐条目由调用方
/// 按 L1→L2 降级链处理）。
pub fn project(
    store: &EpisodicMemory,
    event: &NexusEvent,
    now: DateTime<Utc>,
) -> Result<Option<MemoryId>, MlcError> {
    let Some(entry) = blackboard_to_entry(event, now) else {
        return Ok(None);
    };
    let id = entry.id.clone();
    // insert 返回被驱逐条目(若有)，不影响本次投影成功判定
    store.insert(entry)?;
    Ok(Some(id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nexus_contracts::EventMetadata;

    fn board_event(key: &str, summary: &str, ttl: u64, epistemic: &str) -> NexusEvent {
        NexusEvent::BlackboardEntry {
            metadata: EventMetadata::new("test"),
            key: key.into(),
            summary: summary.into(),
            ttl_secs: ttl,
            epistemic: epistemic.into(),
            provenance: Some("quest-7".into()),
        }
    }

    #[test]
    fn parse_epistemic_variants_and_fallback() {
        assert_eq!(parse_epistemic("Fact"), EpistemicType::Fact);
        assert_eq!(parse_epistemic("episode"), EpistemicType::Episode);
        assert_eq!(parse_epistemic("Mind_Model"), EpistemicType::MindModel);
        assert_eq!(parse_epistemic("mind model"), EpistemicType::MindModel);
        assert_eq!(parse_epistemic("Observation"), EpistemicType::Observation);
        // 未知回退 Fact（不 panic）
        assert_eq!(parse_epistemic("weird"), EpistemicType::Fact);
    }

    #[test]
    fn maps_to_entry_with_ttl_epistemic_provenance() {
        let now = Utc::now();
        let ev = board_event("k1", "共享结论摘要", 60, "observation");
        let e = blackboard_to_entry(&ev, now).expect("黑板事件应可投影");
        assert_eq!(e.id.as_str(), "bb:k1");
        assert_eq!(e.content, "共享结论摘要");
        assert_eq!(e.epistemic_type, EpistemicType::Observation);
        assert_eq!(e.provenance.as_deref(), Some("quest-7"));
        let meta = e.temporal_meta.expect("应带 TTL 时间元数据");
        assert_eq!(meta.valid_from, now.timestamp());
        assert_eq!(meta.valid_until, Some(now.timestamp() + 60));
    }

    #[test]
    fn non_board_event_is_noop() {
        // 非黑板变体→ blackboard_to_entry 返回 None（混合事件流中过滤）
        let other = NexusEvent::NexusStateChanged {
            metadata: EventMetadata::new("t"),
            state_hash: "abc".into(),
            prev_hash: "def".into(),
        };
        assert!(blackboard_to_entry(&other, Utc::now()).is_none());
        // project 对非黑板事件 no-op、不写库
        let store = EpisodicMemory::new(8);
        assert_eq!(project(&store, &other, Utc::now()).unwrap(), None);
        assert_eq!(store.len().unwrap(), 0);
    }

    #[test]
    fn project_writes_into_l1_store() {
        let store = EpisodicMemory::new(16);
        let now = Utc::now();
        let ev = board_event("k9", "协作态", 120, "episode");
        let id = project(&store, &ev, now)
            .unwrap()
            .expect("应返回投影后的 id");
        assert_eq!(id.as_str(), "bb:k9");
        // 落库可回读，且认识标签/TTL 幸存
        let got = store.get("bb:k9").unwrap();
        assert_eq!(got.epistemic_type, EpistemicType::Episode);
        assert_eq!(
            got.temporal_meta.and_then(|m| m.valid_until),
            Some(now.timestamp() + 120),
            "TTL 应折算为 valid_until = now + ttl"
        );
    }

    #[test]
    fn project_same_key_is_idempotent_dedup() {
        let store = EpisodicMemory::new(16);
        let e1 = board_event("dup", "v1", 60, "fact");
        let e2 = board_event("dup", "v2", 60, "fact");
        project(&store, &e1, Utc::now()).unwrap();
        project(&store, &e2, Utc::now()).unwrap();
        // 同 key 覆盖 → 仅一条，内容为最新
        assert_eq!(store.len().unwrap(), 1);
        assert_eq!(store.get("bb:dup").unwrap().content, "v2");
    }
}
