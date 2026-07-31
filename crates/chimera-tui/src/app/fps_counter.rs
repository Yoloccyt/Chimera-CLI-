//! FPS 计数器 — 帧时间移动平均与 FPS 计算
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! Task 1.15 拆分:原 TuiApp 持有 last_frame_time / frame_times 两字段,
//! 集中到 FpsCounter 便于单一职责(FPS 计算)与后续扩展(如 P95/P99 帧时间)。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// FPS 移动平均窗口大小(最近 N 帧)
///
/// WHY 60 帧:对应 60fps 下约 1 秒的窗口,既能平滑单帧抖动
/// (避免状态栏数字频繁跳动),又能对真实帧率变化保持灵敏。
pub(crate) const FPS_WINDOW_SIZE: usize = 60;
/// FPS 显示上限,防止瞬时帧(如调试器步进后首帧)产生超大数字撑破状态栏宽度
///
/// WHY 999:三位数可保证 `FPS: <n>` 文本宽度稳定,配合 80 列状态栏约束。
pub(crate) const FPS_DISPLAY_MAX: u16 = 999;

/// FPS 计数器 — 持有帧时间历史,计算移动平均 FPS
///
/// WHY `VecDeque<f64>` + O(1) push/pop:窗口大小固定为 `FPS_WINDOW_SIZE`,
/// 不需要环形缓冲区等更复杂结构,`VecDeque` 已能满足需求且语义直观。
#[derive(Debug)]
pub struct FpsCounter {
    /// 上一帧的渲染时间戳(P4.4 FPS 计算)
    pub last_frame_time: Instant,
    /// 最近 N 帧的耗时(毫秒),用于 FPS 移动平均(P4.4)
    pub frame_times: VecDeque<f64>,
}

impl FpsCounter {
    /// 创建 FpsCounter(以当前时间为起点)
    pub fn new() -> Self {
        Self {
            last_frame_time: Instant::now(),
            frame_times: VecDeque::with_capacity(FPS_WINDOW_SIZE),
        }
    }

    /// 更新 FPS 移动平均(P4.4)
    ///
    /// WHY 使用移动平均:单帧耗时受 OS 调度、事件循环等待、IO 等影响波动较大,
    /// 直接显示瞬时 FPS 会让状态栏数字频繁跳动、难以阅读。固定窗口移动平均
    /// 平滑短时抖动,同时对真实帧率下降仍保持灵敏响应。
    ///
    /// 返回计算后的 FPS 值(供调用方写入 TuiState.fps)。
    pub(crate) fn update_fps(&mut self, delta: Duration) -> u16 {
        let frame_time_ms = delta.as_secs_f64() * 1000.0;
        self.frame_times.push_back(frame_time_ms);
        while self.frame_times.len() > FPS_WINDOW_SIZE {
            self.frame_times.pop_front();
        }
        if self.frame_times.is_empty() {
            return 0;
        }
        let avg_ms = self.frame_times.iter().sum::<f64>() / self.frame_times.len() as f64;
        // avg_ms 为 0 仅在两帧几乎同时渲染(如调试步进)时发生,避免除零,
        // 将 FPS 记为显示上限。
        if avg_ms > 0.0 {
            ((1000.0 / avg_ms).round() as u16).min(FPS_DISPLAY_MAX)
        } else {
            FPS_DISPLAY_MAX
        }
    }
}

impl Default for FpsCounter {
    fn default() -> Self {
        Self::new()
    }
}
