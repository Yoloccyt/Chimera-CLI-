//! Wiki 生成器 — 从 Quest 结果提取知识条目
//!
//! 对应架构层:L5 Knowledge
//!
//! # 职责
//! 将 `nexus_core::Quest` 中已完成的 Task 转化为 `WikiEntry`,
//! 实现知识沉淀(ISCM:跨层共享索引)。
//!
//! # 嵌入生成两条路径(WHY,P1-1)
//! 1. **NMC 语义路径**(`with_text_encoder`):注入 L2 nmc-encoder 的
//!    `TextPerceptor` — ONNX 模型可用时输出 384 维语义嵌入(all-MiniLM-L6-v2),
//!    无模型时自动降级为字节频率统计(text_dim 维,默认 256)。
//! 2. **占位哈希路径**(默认,`new()`/关联函数):内容 SHA-256 扩展为 512-dim
//!    确定性占位向量(与 `nexus_core::CLV::DIMENSION` 对齐),满足去重与测试需求。
//!
//! # 维度契约(WHY)
//! 两条路径输出维度不同,调用方须保证 `WikiConfig.vector_dim` 与所选路径的
//! 嵌入维度一致(详见 `types.rs` 注释)。占位路径保持历史 512 维零破坏,
//! NMC 路径由感知器自身维度决定(不强制填充)。

use chrono::Utc;
use nexus_core::{Quest, TaskStatus};
use nmc_encoder::{PerceptionInput, Perceptor};
use sha2::{Digest, Sha256};

use crate::types::WikiEntry;

/// Wiki 生成器 — 可选注入 NMC 文本编码器,默认占位哈希嵌入
pub struct WikiGenerator {
    /// 可选的 NMC 文本语义编码器
    ///
    /// `None` 时走占位哈希路径(默认,向后兼容);`Some` 时走 NMC 语义路径
    /// (ONNX 384 维 / 字节频率降级 text_dim 维)。
    text_encoder: Option<nmc_encoder::TextPerceptor>,
}

impl Default for WikiGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl WikiGenerator {
    /// 创建无编码器的生成器(占位哈希路径,行为与历史版本一致)
    pub fn new() -> Self {
        Self { text_encoder: None }
    }

    /// 创建注入 NMC 文本编码器的生成器(语义嵌入路径,P1-1)
    pub fn with_text_encoder(encoder: nmc_encoder::TextPerceptor) -> Self {
        Self {
            text_encoder: Some(encoder),
        }
    }

    /// 关联函数入口(向后兼容):默认占位哈希路径
    ///
    /// 等价于 `WikiGenerator::new().generate_from_quest(quest)`,
    /// 历史调用方(quest-engine/chimera-mas 等)零改动。
    pub fn from_quest_result(quest: &Quest) -> Vec<WikiEntry> {
        Self::new().generate_from_quest(quest)
    }

    /// 从 Quest 结果生成 Wiki 条目(实例方法,按注入编码器选择嵌入路径)
    ///
    /// 为每个 `TaskStatus::Completed` 的 Task 生成一个 `WikiEntry`:
    /// - `entry_id`:`{quest_id}::{task_id}`(保证全局唯一)
    /// - `title`:Task description 前 50 字符(防止过长)
    /// - `content`:Task description 全文
    /// - `tags`:`["quest", quest_id]`(便于按 Quest 过滤)
    /// - `embedding`:NMC 语义路径(有编码器)或 SHA-256 占位(默认)
    pub fn generate_from_quest(&self, quest: &Quest) -> Vec<WikiEntry> {
        let now = Utc::now();
        let quest_tag = quest.quest_id.clone();

        quest
            .tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Completed)
            .map(|task| {
                let embedding = self.embed(&task.description);
                let title = Self::truncate_title(&task.description, 50);

                WikiEntry {
                    entry_id: format!("{}::{}", quest.quest_id, task.task_id),
                    title,
                    content: task.description.clone(),
                    tags: vec!["quest".into(), quest_tag.clone()],
                    embedding,
                    created_at: now,
                    updated_at: now,
                    // P3-W11.2: 新生成条目默认无 temporal_meta(视为 Current,向后兼容)
                    // WHY None: is_current() 对 None 返回 true,语义等价于 Current 状态
                    temporal_meta: None,
                }
            })
            .collect()
    }

    /// 生成内容嵌入 — 编码器可用时走 NMC 语义路径,否则占位哈希
    ///
    /// # 错误策略(WHY)
    /// NMC 编码失败(如 ONNX 模型损坏)时回退占位哈希并记录 warning,
    /// 保证知识沉淀路径永不因编码器故障而中断(fail-open 于降级路径)。
    pub fn embed(&self, content: &str) -> Vec<f32> {
        match &self.text_encoder {
            Some(encoder) => {
                let input = PerceptionInput::Text(content.to_string());
                match encoder.perceive(&input) {
                    Ok(element) => element.embedding,
                    Err(err) => {
                        tracing::warn!(error = %err, "NMC 文本编码失败,回退占位哈希嵌入");
                        Self::placeholder_embedding(content)
                    }
                }
            }
            None => Self::placeholder_embedding(content),
        }
    }

    /// 将 SHA-256 哈希(32 字节)扩展为 512-dim f32 向量
    ///
    /// WHY:无 NMC 编码器时的确定性占位向量,用于验证检索流程与去重。
    ///
    /// 算法:32 字节哈希 → 每字节重复 16 次 → 归一化到 [0, 1]
    /// 32 × 16 = 512,正好填满 CLV 维度。
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
            priority: 128,
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

    // === P1-1:NMC 语义编码路径测试 ===

    /// 注入编码器后 embed 走 NMC 字节频率降级路径(无 ONNX 模型时 text_dim 维)
    #[test]
    fn test_embed_with_text_encoder_byte_frequency_fallback() {
        let config = nmc_encoder::NmcConfig::default(); // model_dir 空 → 降级字节频率
        let encoder = nmc_encoder::TextPerceptor::new(config);
        let generator = WikiGenerator::with_text_encoder(encoder);

        let embedding = generator.embed("Tokio 是 Rust 的异步运行时");
        // 字节频率降级路径输出维度 = text_dim(默认 256),而非占位路径的 512
        assert_eq!(embedding.len(), 256);
        // 确定性:相同输入两次嵌入结果一致
        let again = generator.embed("Tokio 是 Rust 的异步运行时");
        assert_eq!(embedding, again);
    }

    /// 语义路径下不同文本产生不同嵌入(字节频率桶计数)
    #[test]
    fn test_embed_with_text_encoder_distinguishes_content() {
        let encoder = nmc_encoder::TextPerceptor::new(nmc_encoder::NmcConfig::default());
        let generator = WikiGenerator::with_text_encoder(encoder);

        let a = generator.embed("异步并发");
        let b = generator.embed("同步串行");
        assert_ne!(a, b);
    }

    /// 默认构造器(无编码器)保持占位哈希路径(512 维,向后兼容)
    #[test]
    fn test_default_generator_keeps_placeholder_path() {
        let generator = WikiGenerator::new();
        let embedding = generator.embed("hello");
        assert_eq!(embedding.len(), 512);
        assert_eq!(embedding, WikiGenerator::placeholder_embedding("hello"));
    }
}
