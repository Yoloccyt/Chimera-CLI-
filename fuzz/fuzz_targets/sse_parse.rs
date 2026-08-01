//! Fuzz target:SSE 流式归一器模糊测试
//!
//! 对应任务:MCA M0 PR-6(PANTHEON 计划,ADR-065)
//! 架构层:L10 Interface(mca-gateway)
//!
//! # 模糊目标
//! SSE 归一器是全体系最热路径(TTFT 红线承载者),且直接消费不可信的
//! 厂商网络字节流。验证在任意输入下:
//! 1. 传输层帧解析(SseParser)不 panic(含畸形 UTF-8/无边界超长行)
//! 2. 两方言归一(StreamNormalizer)不 panic,未知结构归入 Unknown 而非崩溃(P3)
//! 3. 任意切割不变量:同一字节流按任意位置切成两段分次 feed,
//!    产出的事件序列与一次性 feed 完全一致(跨 chunk 边界正确性)
//!
//! # 运行方式(需 nightly,Windows-GNU 委托 Linux CI)
//! ```bash
//! cargo +nightly fuzz run sse_parse
//! ```
//
// 注意:此文件不添加 #![forbid(unsafe_code)],因为 libfuzzer-sys 的
// fuzz_target! 宏内部展开为 FFI 调用(unsafe),与 forbid 冲突。
// fuzz crate 独立于主 workspace,不影响 38 crate 的 forbid 覆盖率。

// Windows-GNU 下使用 stub 宏(chimera_fuzz),非 Windows 使用 libfuzzer_sys
#[cfg(windows)]
use chimera_fuzz::fuzz_target;
#[cfg(not(windows))]
use libfuzzer_sys::fuzz_target;

use mca_gateway::sse::StreamNormalizer;
use nexus_contracts::affinity::ProtocolDialect;

fuzz_target!(|data: &[u8]| {
    // === 目标1+2:两方言整流不 panic,未知输入容错(P3) ===
    for dialect in [
        ProtocolDialect::OpenAiChat,
        ProtocolDialect::AnthropicMessages,
    ] {
        let mut normalizer = StreamNormalizer::new(dialect);
        let whole = normalizer.feed(data);

        // === 目标3:任意切割不变量 ===
        // 用输入首字节导出切割点(fuzz 探索所有切割位置),两段分次 feed
        // 必须产出与一次性 feed 相同的事件序列——跨 chunk 边界的核心保证。
        if data.len() >= 2 {
            let split = 1 + (data[0] as usize) % (data.len() - 1);
            let mut split_normalizer = StreamNormalizer::new(dialect);
            let mut split_events = split_normalizer.feed(&data[..split]);
            split_events.extend(split_normalizer.feed(&data[split..]));
            assert_eq!(
                whole, split_events,
                "SSE 归一器切割不变量被破坏(dialect={dialect:?}, split={split})"
            );
        }
    }
});
