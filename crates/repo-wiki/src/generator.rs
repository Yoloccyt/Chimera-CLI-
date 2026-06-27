//! Wiki 生成器 — 从 Quest 结果提取知识条目
//!
//! 对应架构层:L5 Knowledge
//!
//! # 职责
//! 将 `nexus_core::Quest` 中已完成的 Task 转化为 `WikiEntry`,
//! 实现知识沉淀(ISCM:跨层共享索引)。
//!
//! # 占位嵌入向量(WHY)
//! Week 2 阶段尚无 NMC 编码器(L6 Router 未实现),
//! 使用内容 SHA-256 哈希扩展为 512-dim 占位向量:
//! - 确定性:相同内容必产生相同向量,便于去重与测试
//! - 维度对齐:512-dim 与 `nexus_core::CLV::DIMENSION` 一致
//! - Week 6 NMC 实现后替换为真实 CLV 嵌入

use chrono::Utc;
use nexus_core::{Quest, TaskStatus};
use sha2::{Digest, Sha256};

use crate::types::WikiEntry;

/// Wiki 生成器 — 无状态工具类型,所有方法均为关联函数
pub struct WikiGenerator;

impl WikiGenerator {
    /// 从 Quest 结果生成 Wiki 条目
    ///
    /// 为每个 `TaskStatus::Completed` 的 Task 生成一个 `WikiEntry`:
    /// - `entry_id`:`{quest_id}::{task_id}`(保证全局唯一)
    /// - `title`:Task description 前 50 字符(防止过长)
    /// - `content`:Task description 全文
    /// - `tags`:`["quest", quest_id]`(便于按 Quest 过滤)
    /// - `embedding`:内容 SHA-256 扩展为 512-dim 占位向量
    pub fn from_quest_result(quest: &Quest) -> Vec<WikiEntry> {
        let now = Utc::now();
        let quest_tag = quest.quest_id.clone();

        quest
            .tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Completed)
            .map(|task| {
                let embedding = Self::placeholder_embedding(&task.description);
                let title = Self::truncate_title(&task.description, 50);

                WikiEntry {
                    entry_id: format!("{}::{}", quest.quest_id, task.task_id),
                    title,
                    content: task.description.clone(),
                    tags: vec!["quest".into(), quest_tag.clone()],
                    embedding,
                    created_at: now,
                    updated_at: now,
                }
            })
            .collect()
    }

    /// 将 SHA-256 哈希(32 字节)扩展为 512-dim f32 向量
    ///
    /// WHY:Week 2 阶段无 NMC 编码器,使用确定性占位向量验证 sqlite-vec 集成
    /// 与 VectorIndex 检索流程。Week 6 NMC 实现后替换为真实 CLV 嵌入。
    ///
    /// 算法:32 字节哈希 → 每字节重复 16 次 → 归一化到 [0, 1]
    /// 32 × 16 = 512,正好填满 CLV 维度。
    // TODO(Week 6): 占位嵌入实现,NMC 编码器实现后替换为真实 CLV 嵌入。
    fn placeholder_embedding(content: &str) -> Vec<f32> {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = hasher.finalize();

        let mut embedding = Vec::with_capacity(512);
        for &byte in hash.iter() {
            let val = byte as f32 / 255.0;
            for _ in 0..16 {
                embedding.push(val);
            }
        }
        embedding
    }

    /// 截断标题到指定最大长度(按字符数,非字节数)
    ///
    /// WHY:Task description 可能很长,作为标题需截断以保持可读性。
    /// 按 `char` 而非 `byte` 截断,避免 UTF-8 多字节字符被切断。
    fn truncate_title(s: &str, max_chars: usize) -> String {
        if s.chars().count() <= max_chars {
            return s.to_string();
        }
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{Task, ThinkingMode};

    fn make_quest_with_tasks(tasks: Vec<Task>) -> Quest {
        Quest {
            quest_id: "q-1".into(),
            title: "测试 Quest".into(),
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    fn make_task(id: &str, desc: &str, status: TaskStatus) -> Task {
        Task {
            task_id: id.into(),
            description: desc.into(),
            status,
            dependencies: vec![],
        }
    }

    #[test]
    fn test_from_quest_result_only_completed() {
        let quest = make_quest_with_tasks(vec![
            make_task("t-1", "任务一", TaskStatus::Completed),
            make_task("t-2", "任务二", TaskStatus::Pending),
            make_task("t-3", "任务三", TaskStatus::Completed),
        ]);

        let entries = WikiGenerator::from_quest_result(&quest);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry_id, "q-1::t-1");
        assert_eq!(entries[1].entry_id, "q-1::t-3");
    }

    #[test]
    fn test_from_quest_result_no_completed() {
        let quest = make_quest_with_tasks(vec![
            make_task("t-1", "任务一", TaskStatus::Pending),
            make_task("t-2", "任务二", TaskStatus::Failed),
        ]);

        let entries = WikiGenerator::from_quest_result(&quest);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_entry_tags_contain_quest_id() {
        let quest = make_quest_with_tasks(vec![make_task("t-1", "任务一", TaskStatus::Completed)]);

        let entries = WikiGenerator::from_quest_result(&quest);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].tags.contains(&"quest".to_string()));
        assert!(entries[0].tags.contains(&"q-1".to_string()));
    }

    #[test]
    fn test_entry_embedding_is_512_dim() {
        let quest = make_quest_with_tasks(vec![make_task("t-1", "任务一", TaskStatus::Completed)]);

        let entries = WikiGenerator::from_quest_result(&quest);
        assert_eq!(entries[0].embedding.len(), 512);
    }

    #[test]
    fn test_placeholder_embedding_deterministic() {
        let e1 = WikiGenerator::placeholder_embedding("hello");
        let e2 = WikiGenerator::placeholder_embedding("hello");
        assert_eq!(e1, e2);

        let e3 = WikiGenerator::placeholder_embedding("world");
        assert_ne!(e1, e3);
    }

    #[test]
    fn test_placeholder_embedding_range() {
        let emb = WikiGenerator::placeholder_embedding("test content");
        assert_eq!(emb.len(), 512);
        // 所有值应在 [0, 1] 范围内
        for &v in &emb {
            assert!((0.0..=1.0).contains(&v), "value out of range: {v}");
        }
    }

    #[test]
    fn test_truncate_title_short() {
        assert_eq!(WikiGenerator::truncate_title("短标题", 50), "短标题");
    }

    #[test]
    fn test_truncate_title_long() {
        let long = "a".repeat(100);
        let truncated = WikiGenerator::truncate_title(&long, 50);
        assert_eq!(truncated.len(), 53); // 50 chars + "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_truncate_title_unicode() {
        let long = "中".repeat(60);
        let truncated = WikiGenerator::truncate_title(&long, 50);
        // 50 个"中" + "..."
        assert_eq!(truncated.chars().count(), 53);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_entry_title_from_description() {
        let quest = make_quest_with_tasks(vec![make_task(
            "t-1",
            "实现 Wiki 存储层",
            TaskStatus::Completed,
        )]);

        let entries = WikiGenerator::from_quest_result(&quest);
        assert_eq!(entries[0].title, "实现 Wiki 存储层");
        assert_eq!(entries[0].content, "实现 Wiki 存储层");
    }
}
