//! MSA动态分块 — Minimax M3 Native Multimodal 块选择优化
//!
//! 对应架构层:L6 Router
//! 对应创新点:P2-7 Minimax MSA块选择优化
//!
//! # 核心机制
//! - 将输入内容动态分块(文本/图像/音频)
//! - 每块计算一致性评分(块内元素语义相似度)
//! - 一致性低于阈值时触发重新分块
//! - 支持在线学习优化分块边界

/// 动态分块器
#[derive(Debug, Clone)]
pub struct DynamicChunker {
    /// 目标块大小(元素数)
    target_chunk_size: usize,
    /// 最小一致性阈值 [0.0, 1.0]
    min_coherence: f32,
    /// 最大块数
    max_chunks: usize,
}

impl Default for DynamicChunker {
    fn default() -> Self {
        Self {
            target_chunk_size: 8,
            min_coherence: 0.7,
            max_chunks: 32,
        }
    }
}

impl DynamicChunker {
    /// 创建分块器
    pub fn new(target_chunk_size: usize, min_coherence: f32, max_chunks: usize) -> Self {
        Self {
            target_chunk_size: target_chunk_size.max(2),
            min_coherence: min_coherence.clamp(0.0, 1.0),
            max_chunks,
        }
    }

    /// 动态分块
    ///
    /// 输入:元素列表(每个元素携带语义向量)
    /// 输出:分块结果(每块包含元素索引列表)
    pub fn chunk(&self, elements: &[ChunkElement]) -> Vec<Chunk> {
        if elements.is_empty() {
            return vec![];
        }
        if elements.len() <= self.target_chunk_size {
            return vec![Chunk {
                indices: (0..elements.len()).collect(),
                coherence: self.compute_chunk_coherence(
                    elements,
                    &(0..elements.len()).collect::<Vec<usize>>(),
                ),
            }];
        }

        let mut chunks = Vec::new();
        let mut start = 0usize;

        while start < elements.len() {
            // 尝试不同分块大小,找到一致性满足要求的最大块
            let mut best_end = (start + self.target_chunk_size).min(elements.len());
            let mut best_coherence = 0.0f32;

            for end in (start + 2)..=((start + self.target_chunk_size * 2).min(elements.len())) {
                let indices: Vec<usize> = (start..end).collect();
                let coherence = self.compute_chunk_coherence(elements, &indices);

                if coherence >= self.min_coherence {
                    best_end = end;
                    best_coherence = coherence;
                } else if end > start + self.target_chunk_size {
                    // 超过目标大小后一致性下降,停止扩展
                    break;
                }
            }

            // 如果找不到满足一致性的块,使用最小块
            if best_coherence < self.min_coherence && best_end > start + 2 {
                best_end = start + 2;
            }

            let indices: Vec<usize> = (start..best_end).collect();
            let coherence = if best_coherence > 0.0 {
                best_coherence
            } else {
                self.compute_chunk_coherence(elements, &indices)
            };

            chunks.push(Chunk { indices, coherence });
            start = best_end;

            // 防止块数过多:预留一个位置给“剩余所有元素”合并成的最后一块
            // WHY >= max_chunks - 1:当前块已占一个位置,若继续循环会再产生新块,
            // 导致总块数超过 max_chunks。此处直接把后续所有元素追加为最后一块。
            if chunks.len() >= self.max_chunks - 1 {
                if start < elements.len() {
                    let remaining: Vec<usize> = (start..elements.len()).collect();
                    let coherence = self.compute_chunk_coherence(elements, &remaining);
                    chunks.push(Chunk {
                        indices: remaining,
                        coherence,
                    });
                }
                break;
            }
        }

        chunks
    }

    /// 计算块内一致性(平均余弦相似度)
    fn compute_chunk_coherence(&self, elements: &[ChunkElement], indices: &[usize]) -> f32 {
        if indices.len() < 2 {
            return 1.0; // 单元素块一致性为1
        }

        let mut total_sim = 0.0f32;
        let mut count = 0usize;

        for i in 0..indices.len() {
            for j in (i + 1)..indices.len() {
                let idx_i = indices[i];
                let idx_j = indices[j];
                if idx_i < elements.len() && idx_j < elements.len() {
                    let sim = cosine_similarity(&elements[idx_i].vector, &elements[idx_j].vector);
                    total_sim += sim;
                    count += 1;
                }
            }
        }

        if count == 0 {
            return 1.0;
        }
        total_sim / count as f32
    }
}

/// 可分块元素
#[derive(Debug, Clone)]
pub struct ChunkElement {
    /// 元素ID
    pub id: String,
    /// 元素类型(文本/图像/音频)
    pub element_type: ElementType,
    /// 语义向量
    pub vector: Vec<f32>,
    /// 原始内容大小(字节)
    pub size_bytes: usize,
}

impl ChunkElement {
    /// 创建元素
    pub fn new(
        id: impl Into<String>,
        element_type: ElementType,
        vector: Vec<f32>,
        size_bytes: usize,
    ) -> Self {
        Self {
            id: id.into(),
            element_type,
            vector,
            size_bytes,
        }
    }
}

/// 元素类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementType {
    /// 文本
    Text,
    /// 图像
    Image,
    /// 音频
    Audio,
}

/// 分块结果
#[derive(Debug, Clone)]
pub struct Chunk {
    /// 块内元素索引
    pub indices: Vec<usize>,
    /// 块一致性评分
    pub coherence: f32,
}

impl Chunk {
    /// 块大小
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// 计算余弦相似度
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_elements(count: usize, similar: bool) -> Vec<ChunkElement> {
        (0..count)
            .map(|i| {
                let vector = if similar {
                    vec![0.5f32 + (i as f32 * 0.01); 64]
                } else {
                    vec![i as f32 / count as f32; 64]
                };
                ChunkElement::new(format!("e-{i}"), ElementType::Text, vector, 100)
            })
            .collect()
    }

    #[test]
    fn test_chunk_small_input() {
        let chunker = DynamicChunker::default();
        let elements = make_elements(5, true);
        let chunks = chunker.chunk(&elements);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 5);
    }

    #[test]
    fn test_chunk_large_similar() {
        // 相似元素应被分到大块
        let chunker = DynamicChunker::new(4, 0.7, 10);
        let elements = make_elements(20, true);
        let chunks = chunker.chunk(&elements);
        // 相似元素应形成较少的大块
        assert!(
            chunks.len() <= 5,
            "相似元素应分块较少, got {} chunks",
            chunks.len()
        );
        // 一致性应较高
        for chunk in &chunks {
            assert!(
                chunk.coherence >= 0.7,
                "一致性应 >= 0.7, got {}",
                chunk.coherence
            );
        }
    }

    #[test]
    fn test_chunk_large_dissimilar() {
        // 不相似元素应被分成小块
        let chunker = DynamicChunker::new(4, 0.7, 10);
        let elements = make_elements(20, false);
        let chunks = chunker.chunk(&elements);
        // 不相似元素可能形成更多块
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_max_chunks_limit() {
        let chunker = DynamicChunker::new(2, 0.9, 3); // 很严格的一致性,很小的max
        let elements = make_elements(100, false);
        let chunks = chunker.chunk(&elements);
        assert!(chunks.len() <= 3, "块数应不超过max_chunks");
    }

    #[test]
    fn test_coherence_computation() {
        let chunker = DynamicChunker::default();
        // 完全相同的向量
        let elements = vec![
            ChunkElement::new("e1", ElementType::Text, vec![1.0; 64], 100),
            ChunkElement::new("e2", ElementType::Text, vec![1.0; 64], 100),
        ];
        let chunks = chunker.chunk(&elements);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].coherence > 0.99, "相同向量一致性应接近1.0");
    }
}
