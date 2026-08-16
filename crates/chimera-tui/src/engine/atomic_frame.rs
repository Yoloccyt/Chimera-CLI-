//! engine::atomic_frame — 原子帧写出器(Concord W7 T7.2,ADR-079)
//!
//! 对应架构层:L10 Interface
//!
//! # 问题
//! v3 引擎 diff 输出经 [`super::writer::TerminalWriter`] 逐条 `queue!` 写出;
//! 即便最终 flush 一次,终端仍可能在写出过程中穿插重绘,产生半帧/撕裂。
//! diff 16µs 解决"算得快",不解决"画得整"。
//!
//! # 机制(对齐 pi-tui 三件套:差分渲染 + 同步输出 + 批量单写)
//! 一帧的全部 ANSI 序列先累积进复用帧缓冲;`finish_frame` 时:
//! 1. 若 [`SyncMode::Probed`],尾部追加 `CSI ? 2026 l` 闭合同步窗口;
//! 2. **单次 `write_all`** 提交整帧(系统调用从 O(k) 降为 O(1));
//! 3. 缓冲 `clear()` 复用(容量保留,稳态零再分配)。
//!
//! 同步序列包裹由 [`super::sync_probe`] 的探测回退链裁决:能力缺失
//! (Disabled)或用户逃生门(ForcedOff)时退化为"仅批量单写",收益不减。
//!
//! # 复杂度
//! 每帧额外 O(1)(两条序列 + 一次 memcpy 合并);峰值内存 = 单帧转义字节数
//! (实测 <64KB)。

use std::io::{self, Write};

use super::sync_probe::{SyncMode, SYNC_BEGIN, SYNC_END};

/// 原子帧写出器 — 帧级缓冲累积 + 单次原子提交
#[derive(Debug)]
pub struct AtomicFrameWriter {
    /// 同步输出模式(探测回退链终判,启动后不变)
    sync_mode: SyncMode,
    /// 帧级单缓冲(跨帧复用容量,稳态零再分配)
    buf: Vec<u8>,
}

/// 帧缓冲初始容量:80×24 增量帧典型转义字节量级(避免首帧即扩容)
const INITIAL_BUF_CAPACITY: usize = 8 * 1024;

impl AtomicFrameWriter {
    /// 以指定同步模式构造(生产由 `probe_sync_output()` 终判,测试直接注入)
    pub fn new(sync_mode: SyncMode) -> Self {
        Self {
            sync_mode,
            buf: Vec::with_capacity(INITIAL_BUF_CAPACITY),
        }
    }

    /// 当前同步模式(遥测/诊断用)
    pub fn sync_mode(&self) -> SyncMode {
        self.sync_mode
    }

    /// 开始一帧:清空缓冲(保留容量),Probed 时写入同步窗口开启序列
    pub fn begin_frame(&mut self) {
        self.buf.clear();
        if self.sync_mode.wraps_frames() {
            self.buf.extend_from_slice(SYNC_BEGIN);
        }
    }

    /// 帧缓冲可变引用 — `TerminalWriter` 的写出目标
    ///
    /// 使用模式:`begin_frame` → `TerminalWriter::new(buffer_mut()).render(...)`
    /// → `finish_frame`。
    pub fn buffer_mut(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }

    /// 提交一帧:Probed 时闭合同步窗口,随后**单次 `write_all`** 原子写出
    ///
    /// # 错误
    /// 底层写入失败原样返回;调用方(渲染路径)映射为 `TuiError::Render`。
    pub fn finish_frame(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if self.sync_mode.wraps_frames() {
            self.buf.extend_from_slice(SYNC_END);
        }
        // 单帧单 write:终端无机会在序列中间穿插重绘(撕裂结构性消除)
        out.write_all(&self.buf)?;
        out.flush()?;
        self.buf.clear();
        Ok(())
    }

    /// 当前缓冲字节数(测试断言用)
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// 当前缓冲容量(测试断言复用零再分配)
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计数写出器:记录 write_all 调用次数(断言单帧单 write)
    struct CountingWriter {
        inner: Vec<u8>,
        writes: usize,
    }

    impl CountingWriter {
        fn new() -> Self {
            Self {
                inner: Vec::new(),
                writes: 0,
            }
        }
    }

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.inner.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn probed_frame_wrapped_with_sync_sequences() {
        let mut afw = AtomicFrameWriter::new(SyncMode::Probed);
        afw.begin_frame();
        afw.buffer_mut().extend_from_slice(b"FRAME");
        let mut sink = Vec::<u8>::new();
        afw.finish_frame(&mut sink).unwrap();
        assert!(
            sink.starts_with(SYNC_BEGIN),
            "Probed 帧应以 CSI ?2026h 开头"
        );
        assert!(sink.ends_with(SYNC_END), "Probed 帧应以 CSI ?2026l 结尾");
        assert!(sink.windows(5).any(|w| w == b"FRAME"), "帧内容应完整保留");
    }

    #[test]
    fn disabled_frame_not_wrapped() {
        for mode in [SyncMode::Disabled, SyncMode::ForcedOff] {
            let mut afw = AtomicFrameWriter::new(mode);
            afw.begin_frame();
            afw.buffer_mut().extend_from_slice(b"FRAME");
            let mut sink = Vec::<u8>::new();
            afw.finish_frame(&mut sink).unwrap();
            assert_eq!(sink, b"FRAME", "{mode:?} 帧不应含同步序列");
        }
    }

    #[test]
    fn finish_frame_is_single_write() {
        let mut afw = AtomicFrameWriter::new(SyncMode::Probed);
        let mut sink = CountingWriter::new();
        // 多帧均应为单 write(批量单写:O(k) 系统调用 → O(1))
        for _ in 0..3 {
            afw.begin_frame();
            afw.buffer_mut().extend_from_slice(b"AB");
            afw.finish_frame(&mut sink).unwrap();
        }
        assert_eq!(sink.writes, 3, "每帧应恰好一次 write,实测 {}", sink.writes);
    }

    #[test]
    fn buffer_reused_across_frames_without_realloc() {
        let mut afw = AtomicFrameWriter::new(SyncMode::Disabled);
        afw.begin_frame();
        afw.buffer_mut().extend_from_slice(&[b'x'; 4096]);
        let cap_after_first = afw.capacity();
        let mut sink = Vec::<u8>::new();
        afw.finish_frame(&mut sink).unwrap();
        assert_eq!(afw.buffered_len(), 0, "提交后缓冲应清空");
        // 第二帧同规模数据不应触发再扩容(容量复用)
        afw.begin_frame();
        afw.buffer_mut().extend_from_slice(&[b'y'; 4096]);
        assert!(afw.capacity() >= cap_after_first, "容量应保留复用,不收缩");
    }

    #[test]
    fn empty_frame_still_flushes_wrapper_only() {
        let mut afw = AtomicFrameWriter::new(SyncMode::Probed);
        afw.begin_frame();
        let mut sink = Vec::<u8>::new();
        afw.finish_frame(&mut sink).unwrap();
        // 空帧仅含开合序列(合法:终端按无变更处理)
        let mut expect = SYNC_BEGIN.to_vec();
        expect.extend_from_slice(SYNC_END);
        assert_eq!(sink, expect);
    }

    #[test]
    fn sync_mode_accessor_matches_construction() {
        assert_eq!(
            AtomicFrameWriter::new(SyncMode::Probed).sync_mode(),
            SyncMode::Probed
        );
        assert_eq!(
            AtomicFrameWriter::new(SyncMode::ForcedOff).sync_mode(),
            SyncMode::ForcedOff
        );
    }
}
