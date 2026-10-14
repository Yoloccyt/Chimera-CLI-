//! 可移植记忆导出/导入 — Markdown + 索引约定格式 round-trip（v2.3 §(b.3)）
//!
//! 对应架构层: L2 Memory（mlc-engine）
//! 对应设计源: `Chimera架构重构与优化方案_v2.3` §(b.3)「可移植导出（新建）：
//! Markdown+索引约定格式 round-trip（现状 `wiki.rs`/`/export` 均非记忆导出）」
//!
//! # WHY
//! 现状无任何**记忆**级导出（TUI `/export` 是 Quest CSV；`export_distilled_insights`
//! 返回结构体非文件）。§(b.3) 要求记忆条目可跨系统移植且**无损往返**——因此新建
//! 以人为友好（Markdown 索引）+ 机器可解析（每条目一行哨兵 JSON）的导出格式。
//!
//! # 格式约定（无损、抗内容冲突）
//! - 顶部为人类可读索引表（id | tier | epistemic | 字节数）；
//! - 每条 `MemoryEntry` 序列化为**单行** JSON，前缀唯一哨兵 `<!--mlc-entry-->`；
//!   `serde_json` 转义保证条目内换行/反引号不外溢为新行，故按行 `starts_with`
//!   切分绝对稳健（无 fence 冲突、无需多行解析器）。
//! - 导入只消费哨兵行，索引/标题纯为人和 diff 友好，可安全忽略。
//!
//! # 无损性
//! `MemoryEntry` 全字段 `Serialize + Deserialize`（含 `clv`/`temporal_meta`/
//! `epistemic_type`/`evidence_ptrs`/`provenance`），JSON 往返即字节级无损。

use nexus_contracts::EpistemicType;

use crate::types::{MemoryEntry, MemoryTier};

/// 每条目导出行的唯一哨兵前缀。
const ENTRY_SENTINEL: &str = "<!--mlc-entry-->";
// 说明：哨兵刻意以注释形式落地（Markdown 渲染时不可见），并与内容行严格
// 一对一；导入端只认此前缀。

/// 导出一组记忆条目为 Markdown + 索引约定格式的文本（可移植、可 diff）。
#[must_use]
pub fn export_markdown(entries: &[MemoryEntry]) -> String {
    let mut out = String::new();
    out.push_str("# Chimera Memory Export\n\n");
    out.push_str("## Index\n\n");
    out.push_str("| id | tier | epistemic | bytes |\n");
    out.push_str("|----|------|-----------|-------|\n");
    for e in entries {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            e.id.as_str(),
            tier_label(&e.tier),
            epistemic_label(&e.epistemic_type),
            e.content.len()
        ));
    }
    out.push_str("\n## Entries\n\n");
    for e in entries {
        // 单行 JSON；serde_json 不产生裸换行，故哨兵行与条目一一对应。
        match serde_json::to_string(e) {
            Ok(json) => out.push_str(&format!("{ENTRY_SENTINEL}{json}\n")),
            // 序列化失败（极少见，如非 UTF-8）：跳过但不 panic，保持导出不中断。
            Err(_) => out.push_str(&format!("<!--mlc-export-error {}-->\n", e.id.as_str())),
        }
    }
    out
}

/// 从 Markdown 导出文本导入记忆条目（无损、保序）。
///
/// 只消费以 [`ENTRY_SENTINEL`] 前缀开头的行；无该前缀的行（索引/标题）忽略。
/// 任一条目 JSON 解析失败即整体报错（宁可失败也不静默丢条目）。
pub fn import_markdown(text: &str) -> Result<Vec<MemoryEntry>, crate::error::MlcError> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(ENTRY_SENTINEL) else {
            continue;
        };
        let entry: MemoryEntry = serde_json::from_str(rest).map_err(|e| {
            crate::error::MlcError::StorageError(format!("memory import parse failed: {e}"))
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

/// `MemoryTier` 的稳定短标签（仅用于索引人类可读列，不参与解析）。
fn tier_label(t: &MemoryTier) -> &'static str {
    match t {
        MemoryTier::L0Working => "L0Working",
        MemoryTier::L1Episodic => "L1Episodic",
        MemoryTier::L2Semantic => "L2Semantic",
        MemoryTier::L3Procedural => "L3Procedural",
    }
}

/// `EpistemicType` 的稳定短标签（仅用于索引列）。
fn epistemic_label(e: &EpistemicType) -> &'static str {
    match e {
        EpistemicType::Fact => "Fact",
        EpistemicType::Episode => "Episode",
        EpistemicType::Observation => "Observation",
        EpistemicType::MindModel => "MindModel",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::MemoryTier;
    use nexus_core::CLV;

    fn full_entry(id: &str) -> MemoryEntry {
        let clv = CLV::from_vec(vec![0.1; CLV::DIMENSION]).unwrap();
        let mut e = MemoryEntry::new(
            id,
            format!("内容 {id}\n含换行与```围栏"),
            MemoryTier::L2Semantic,
        )
        .with_clv(clv);
        e.epistemic_type = EpistemicType::Observation;
        e.evidence_ptrs = vec!["ev-1".into(), "ev-2".into()];
        e.provenance = Some("quest-42".into());
        e
    }

    #[test]
    fn export_import_round_trip_is_lossless() {
        let entries: Vec<MemoryEntry> = vec![full_entry("m-1"), full_entry("m-2")];
        let md = export_markdown(&entries);
        // 索引 + 两条哨兵条目行
        assert!(md.contains("# Chimera Memory Export"));
        assert_eq!(md.matches(ENTRY_SENTINEL).count(), 2);
        let back = import_markdown(&md).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(
            back, entries,
            "导出/导入必须字节级无损（含 CLV/epistemic/换行内容）"
        );
    }

    #[test]
    fn sentinel_resists_backtick_content() {
        // content 含 ``` 与换行：serde 转义后不外溢为新行 → 解析条数仍精确
        let entries = vec![full_entry("x")];
        let md = export_markdown(&entries);
        let back = import_markdown(&md).unwrap();
        assert_eq!(back, entries);
    }

    #[test]
    fn empty_export_imports_empty() {
        let md = export_markdown(&[]);
        assert!(md.contains("## Entries"));
        assert!(import_markdown(&md).unwrap().is_empty());
    }
}
