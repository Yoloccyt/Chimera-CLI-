//! ThinkingPreserve 推理痕迹保留 — 压缩全程不触碰 thinking 块
//!
//! 对应任务:P2-T4(T-02/Ω₉,手册 W11)
//! 对应架构层:L2 Memory(挂 hcw-window 增强,ADR-139 批准,否决新建 nexus-compress)
//! 设计来源:手册 §10.5 pipeline.rs 骨架(split_thinking → 压缩 body → rejoin)+
//! v4.0 WI-12(ThinkingPreserve 8 槽推理痕迹环形缓冲,压缩不触碰)
//!
//! # 核心职责
//! - [`ThinkingPreserve`]:8 槽环形缓冲,保存会话推理痕迹(thinking 块)。
//!   溢出时覆盖最旧槽(环形语义),压缩链路从不修改槽内内容。
//! - [`split_thinking`]/[`rejoin`]:压缩管线入口剥离 thinking → 压缩 body →
//!   原样回填,thinking 块逐字节一致(门禁:thinking 链完整率 100%)。
//! - [`ConversationContext`]:管线视角的会话上下文(from 模式前缀 + 正文条目 +
//!   thinking 块),`rejoin` 保证前缀逐字节不变 + thinking 原样回填。
//!
//! # 设计决策(WHY)
//! - **压缩不触碰 thinking**:Qwen preserve_thinking 证据 + Ω₉ 推理痕迹是链式
//!   推理的关键引用锚点,压缩其引用上下文即可,thinking 本体必须原样保留
//!   (手册 Ch9 T-02:否决「thinking 块视为可丢弃装饰」C17)。
//! - **8 槽容量**:v4.0 WI-12 规格(8 槽推理痕迹);`with_capacity` 可调,
//!   `THINKING_SLOTS` 为默认值。
//! - **前缀 Arc 共享**:`prefix: Arc<str>` 克隆 O(1),`rejoin` 零拷贝复用原前缀,
//!   从模式保前缀(缓存前缀不失效——压缩指令只尾追加,不动前缀)。
//! - **正文用条目而非文本**:与既有 `ContextCompressor`/`score_entries` 的
//!   `Vec<Arc<ContextEntry>>` 数据面一致(复用并行评分,不重复造轮)。

use std::collections::VecDeque;
use std::sync::Arc;

use crate::types::ContextEntry;

/// 默认 thinking 槽数 — v4.0 WI-12 规格(8 槽推理痕迹环形缓冲)
pub const THINKING_SLOTS: usize = 8;

/// 单个推理痕迹块 — 压缩链路不得修改其内容
///
/// WHY `Arc<str>`:thinking 块可能较大(数 KB),压缩/回填场景克隆 O(1);
/// `as_bytes` 提供字节级访问(门禁逐字节断言)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingBlock {
    /// 块唯一标识(链序锚点,测试与回放使用)
    pub id: u64,
    /// 推理痕迹内容(逐字节保留)
    pub content: Arc<str>,
}

impl ThinkingBlock {
    /// 创建 thinking 块
    #[must_use]
    pub fn new(id: u64, content: impl Into<String>) -> Self {
        Self {
            id,
            content: Arc::from(content.into()),
        }
    }

    /// 内容字节切片 — 门禁「thinking 链完整率 100%」逐字节断言入口
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.content.as_bytes()
    }
}

/// ThinkingPreserve — 推理痕迹环形缓冲(8 槽,压缩不触碰)
///
/// # 设计决策(WHY)
/// - **环形语义**:`push` 在满槽时覆盖最旧块(先入先出 + 容量封顶),
///   保证任意时刻保留最近 N 条推理痕迹(推理链关注近期上下文)。
/// - **drain 按序返回**:`drain` 返回从最旧到最新的全量块(保持链序),
///   供回放/续聊时按原序回填。
/// - **cap = 0 时 push 为 no-op**:容量为 0 的缓冲不保留任何痕迹
///   (防御性边界,避免容量取 0 时的越界语义)。
pub struct ThinkingPreserve {
    buf: VecDeque<ThinkingBlock>,
    cap: usize,
}

impl ThinkingPreserve {
    /// 默认 8 槽环形缓冲
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(THINKING_SLOTS)
    }

    /// 指定容量(0 表示不保留任何痕迹)
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// 推入 thinking 块;满槽时覆盖最旧块(环形溢出语义)
    pub fn push(&mut self, block: ThinkingBlock) {
        // WHY 显式 no-op:cap = 0 时无槽可写,直接丢弃避免 VecDeque 无界增长
        if self.cap == 0 {
            return;
        }
        if self.buf.len() == self.cap {
            // 环形溢出:覆盖最旧槽(队首),保持容量封顶
            self.buf.pop_front();
        }
        self.buf.push_back(block);
    }

    /// 取空缓冲,按从旧到新顺序返回全部块(链序保持)
    #[must_use]
    pub fn drain(&mut self) -> Vec<ThinkingBlock> {
        std::mem::take(&mut self.buf).into_iter().collect()
    }

    /// 当前已存块数
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// 是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// 容量上限
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// 迭代已存块(从旧到新)
    pub fn iter(&self) -> impl Iterator<Item = &ThinkingBlock> {
        self.buf.iter()
    }
}

impl Default for ThinkingPreserve {
    fn default() -> Self {
        Self::new()
    }
}

/// 会话上下文 — 压缩管线的输入/输出表示
///
/// # 字段语义
/// - `prefix`:from 模式静态前缀(系统提示等)。压缩**不触碰**,`rejoin` 后
///   逐字节不变 → 缓存前缀不失效(v4.0 WI-12)。
/// - `body`:正文条目(每轮会话一条或一组),是压缩的对象。
/// - `thinking`:推理痕迹块,压缩全程不触碰(`split_thinking` 剥离后原样回填)。
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationContext {
    /// 静态前缀(逐字节保留)
    pub prefix: Arc<str>,
    /// 正文条目(压缩对象)
    pub body: Vec<Arc<ContextEntry>>,
    /// thinking 块(压缩不触碰,原样回填)
    pub thinking: Vec<ThinkingBlock>,
}

impl ConversationContext {
    /// 创建会话上下文
    #[must_use]
    pub fn new(
        prefix: impl Into<String>,
        body: Vec<Arc<ContextEntry>>,
        thinking: Vec<ThinkingBlock>,
    ) -> Self {
        Self {
            prefix: Arc::from(prefix.into()),
            body,
            thinking,
        }
    }
}

/// 剥离 thinking — 压缩管线入口(T-02:先剥离再压缩 body)
///
/// # 返回
/// `(thinking 块按原序, 正文条目, 前缀)` — 三部分全部**克隆**(Arc 引用计数
/// O(1)),原上下文不被消费,调用方可自由复用。
///
/// WHY 返回前缀:rejoin 需要原前缀拼回;克隆成本 O(1)(Arc<str> 引用计数)。
#[must_use]
pub fn split_thinking(
    ctx: &ConversationContext,
) -> (Vec<ThinkingBlock>, Vec<Arc<ContextEntry>>, Arc<str>) {
    (
        ctx.thinking.clone(),
        ctx.body.clone(),
        Arc::clone(&ctx.prefix),
    )
}

/// 原样回填 — 压缩完成后把 thinking 原样放回(from 模式保前缀)
///
/// WHY 保证:
/// - 前缀 = 传入前缀逐字节一致(静态层 token 序列不变 → 缓存前缀不失效);
/// - thinking = 传入块按原序回填(压缩链路未触碰,字节级一致);
/// - body = 压缩后的正文(管线唯一允许变化的字段)。
#[must_use]
pub fn rejoin(
    prefix: Arc<str>,
    body: Vec<Arc<ContextEntry>>,
    thinking: Vec<ThinkingBlock>,
) -> ConversationContext {
    ConversationContext {
        prefix,
        body,
        thinking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试块(content 含中文与换行,字节级断言更有意义)
    fn block(id: u64, s: &str) -> ThinkingBlock {
        ThinkingBlock::new(id, s)
    }

    #[test]
    fn test_ring_buffer_default_capacity_8() {
        // 默认 8 槽:推入 8 块 → 全部保留
        let mut p = ThinkingPreserve::new();
        assert_eq!(p.capacity(), THINKING_SLOTS);
        assert!(p.is_empty());
        for i in 0..THINKING_SLOTS {
            p.push(block(i as u64, &format!("t-{i}")));
        }
        assert_eq!(p.len(), 8);
        assert!(!p.is_empty());
    }

    #[test]
    fn test_ring_overflow_overwrites_oldest() {
        // 环形溢出:推入 10 块(cap 8)→ 最旧 2 块被覆盖,保留最近 8 块
        let mut p = ThinkingPreserve::new();
        for i in 0..10 {
            p.push(block(i, &format!("t-{i}")));
        }
        assert_eq!(p.len(), 8, "满槽后应保持容量封顶");
        let ids: Vec<u64> = p.drain().into_iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![2, 3, 4, 5, 6, 7, 8, 9], "最旧 0/1 应被覆盖");
    }

    #[test]
    fn test_drain_returns_oldest_first_in_order() {
        // drain 按从旧到新返回,且取空后缓冲为空
        let mut p = ThinkingPreserve::new();
        p.push(block(1, "a"));
        p.push(block(2, "b"));
        p.push(block(3, "c"));
        let out = p.drain();
        let ids: Vec<u64> = out.iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert!(p.is_empty(), "drain 后缓冲应为空");
        assert!(p.drain().is_empty(), "二次 drain 应为空");
    }

    #[test]
    fn test_push_noop_when_capacity_zero() {
        // cap 0:push 为 no-op,不保留任何痕迹
        let mut p = ThinkingPreserve::with_capacity(0);
        p.push(block(1, "a"));
        assert!(p.is_empty());
        assert!(p.drain().is_empty());
    }

    #[test]
    fn test_iter_order_and_content() {
        // iter 从旧到新,内容逐字节可访问
        let mut p = ThinkingPreserve::new();
        p.push(block(1, "思考一\n第二行"));
        p.push(block(2, "思考二"));
        let bytes: Vec<&[u8]> = p.iter().map(|b| b.as_bytes()).collect();
        assert_eq!(bytes[0], "思考一\n第二行".as_bytes());
        assert_eq!(bytes[1], "思考二".as_bytes());
    }

    #[test]
    fn test_split_thinking_roundtrip_preserves_prefix_and_thinking() {
        // 剥离 → 回填:前缀逐字节一致 + thinking 逐字节一致(字节级断言)
        let body = vec![Arc::new(ContextEntry::new("e-1", "f-1", "content-1", 100))];
        let thinking = vec![
            block(1, "think-A"),
            block(2, "think-B\nwith 换行"),
            block(3, "think-C"),
        ];
        let ctx = ConversationContext::new("system: 你是助手\n", body, thinking.clone());

        let (thinking_out, body_out, prefix_out) = split_thinking(&ctx);
        // split 返回三部分与原上下文一致
        assert_eq!(thinking_out, thinking);
        assert_eq!(body_out, ctx.body);
        assert_eq!(&prefix_out as &str, &ctx.prefix as &str);

        // 回填:前缀与 thinking 原样,正文替换为压缩后条目
        let compressed_body = vec![Arc::new(ContextEntry::new("c-1", "f-1", "压缩后", 50))];
        let rejoined = rejoin(prefix_out, compressed_body, thinking_out);

        // 门禁:前缀逐字节一致(静态层 token 序列不变 → 缓存前缀不失效)
        assert_eq!(
            rejoined.prefix.as_bytes(),
            ctx.prefix.as_bytes(),
            "from 模式:压缩后前缀必须逐字节不变"
        );
        // 门禁:thinking 链完整率 100%(逐字节一致 + 链序一致)
        assert_eq!(rejoined.thinking.len(), thinking.len());
        for (a, b) in rejoined.thinking.iter().zip(thinking.iter()) {
            assert_eq!(a.id, b.id, "thinking 链序必须一致");
            assert_eq!(
                a.as_bytes(),
                b.as_bytes(),
                "thinking 块必须逐字节一致(完整率 100%)"
            );
        }
        // 正文为压缩后条目
        assert_eq!(rejoined.body[0].id, "c-1");
        assert_eq!(rejoined.body[0].token_size, 50);
    }

    #[test]
    fn test_conversation_context_clone_is_cheap_and_equal() {
        // 上下文克隆(Arc 引用计数)后相等性保持
        let ctx = ConversationContext::new(
            "prefix",
            vec![Arc::new(ContextEntry::new("e-1", "f-1", "c", 10))],
            vec![block(1, "t")],
        );
        let cloned = ctx.clone();
        assert_eq!(cloned, ctx);
        assert_eq!(cloned.prefix.as_bytes(), ctx.prefix.as_bytes());
    }
}
