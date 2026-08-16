//! app::fps_governor — 绘制帧率治理器(Concord W7 T7.4,ADR-079)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! Dashboard 视图帧率封顶(15-30 FPS 可调,tui-design 规范 [^79^]):
//! 仪表板是观测型界面,数据节奏由 DataPipeline tick(250ms)决定,
//! 绘制超过 ~30 FPS 只增 CPU 不增信息。事件循环每轮先问帧闸,
//! 未到间隔则跳过本轮绘制(数据 `update()` 照常,仅省绘制与写出)。
//!
//! # 设计决策(WHY)
//! - **与 Eco tick 正交**:Eco 管数据节奏(poll/tick 间隔),本治理器管
//!   绘制节奏;两者独立生效,叠加时以更保守者为准。
//! - **动画永不阻塞输入**:任何键/鼠事件到达即 `on_input()`,
//!   下一帧立即放行(帧闸让位于交互);动画预算表仅供视图过渡
//!   选择时长,不产生等待。
//! - **Chat 视图不受治理**:会话流式输出是交互型界面,保持每轮绘制
//!   (治理仅 Dashboard,方案 §5.5)。
//! - **O(1) 帧闸**:单次时间比较,无队列无锁,每轮循环开销可忽略。

use std::time::{Duration, Instant};

/// 帧率封顶下限(低于此值仪表板观感卡顿)
pub const FPS_CAP_MIN: u8 = 5;
/// 帧率封顶上限(高于此值对观测型界面无收益)
pub const FPS_CAP_MAX: u8 = 60;
/// 默认封顶(tui-design 规范 15-30 FPS 区间上沿)
pub const FPS_CAP_DEFAULT: u8 = 30;

/// 动画类别 — 预算表索引(方案 §5.5)
// WHY allow dead_code:预算表是 tui-design 规范内置项,当前无动画系统消费;
// 视图过渡动画落地时经 animation_budget 查表,届时移除本 allow。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationKind {
    /// 选中项切换:零时长,即时反馈
    Selection,
    /// 视图过渡:100-200ms 预算带
    ViewTransition,
    /// 图表过渡:200-500ms 预算带
    ChartTransition,
}

/// 绘制帧率治理器 — Dashboard 帧闸 + 输入抢占 + 动画预算表
#[derive(Debug)]
pub(crate) struct FpsGovernor {
    /// 帧率封顶(clamp 至 [FPS_CAP_MIN, FPS_CAP_MAX])
    max_fps: u8,
    /// 上次放行绘制的时间(None = 尚未绘制过)
    last_draw: Option<Instant>,
    /// 输入抢占标记:键/鼠事件到达后置位,下一次帧闸无条件放行
    input_boost: bool,
}

impl FpsGovernor {
    /// 以配置帧率封顶构造(越界值 clamp 至合法区间)
    pub fn new(max_fps: u8) -> Self {
        Self {
            max_fps: max_fps.clamp(FPS_CAP_MIN, FPS_CAP_MAX),
            last_draw: None,
            input_boost: false,
        }
    }

    /// 当前帧率封顶
    // WHY allow dead_code:观测/热更新接口,当前帧闸直接读字段;配置热更新
    // 路径(config.edit)接入 set_max_fps 时移除本 allow。
    #[allow(dead_code)]
    pub fn max_fps(&self) -> u8 {
        self.max_fps
    }

    /// 运行时调整封顶(配置热更新路径;越界 clamp)
    #[allow(dead_code)]
    pub fn set_max_fps(&mut self, max_fps: u8) {
        self.max_fps = max_fps.clamp(FPS_CAP_MIN, FPS_CAP_MAX);
    }

    /// 最小帧间隔(1 / max_fps)
    pub fn min_interval(&self) -> Duration {
        // max_fps ≥ FPS_CAP_MIN > 0,无除零风险
        Duration::from_micros(1_000_000 / self.max_fps as u64)
    }

    /// 帧闸判定:本轮是否放行绘制(O(1))
    ///
    /// 放行条件(任一):输入抢占置位 / 首帧 / 距上次放行 ≥ 最小帧间隔。
    /// 放行即记录时间并清除抢占标记。
    pub fn should_draw(&mut self, now: Instant) -> bool {
        if self.input_boost {
            self.input_boost = false;
            self.last_draw = Some(now);
            return true;
        }
        match self.last_draw {
            None => {
                self.last_draw = Some(now);
                true
            }
            Some(last) if now.duration_since(last) >= self.min_interval() => {
                self.last_draw = Some(now);
                true
            }
            Some(_) => false,
        }
    }

    /// 输入到达:抢占下一帧(动画永不阻塞输入的机制落点)
    pub fn on_input(&mut self) {
        self.input_boost = true;
    }

    /// 动画预算表:各类过渡的建议时长(方案 §5.5)
    ///
    /// 预算是"建议时长"而非等待:过渡进行中按键到达即取消
    /// (经 `on_input` 抢占),绝不阻塞输入路径。
    #[allow(dead_code)] // 规范内置项,动画系统落地时消费(同 AnimationKind)
    pub fn animation_budget(kind: AnimationKind) -> Duration {
        match kind {
            // 选中切换零时长:即时反馈是 TUI 的基本义务
            AnimationKind::Selection => Duration::from_millis(0),
            // 视图过渡取预算带中值(100-200ms)
            AnimationKind::ViewTransition => Duration::from_millis(150),
            // 图表过渡取预算带中值(200-500ms)
            AnimationKind::ChartTransition => Duration::from_millis(300),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_always_allowed() {
        let mut g = FpsGovernor::new(30);
        assert!(g.should_draw(Instant::now()));
    }

    #[test]
    fn rapid_frames_gated_by_min_interval() {
        let mut g = FpsGovernor::new(30); // 最小间隔 ≈ 33ms
        let t0 = Instant::now();
        assert!(g.should_draw(t0));
        assert!(
            !g.should_draw(t0 + Duration::from_millis(5)),
            "间隔内应拒绝"
        );
        assert!(
            g.should_draw(t0 + Duration::from_millis(40)),
            "间隔外应放行"
        );
    }

    #[test]
    fn input_boost_preempts_gate() {
        let mut g = FpsGovernor::new(15); // 最小间隔 ≈ 66ms
        let t0 = Instant::now();
        assert!(g.should_draw(t0));
        assert!(!g.should_draw(t0 + Duration::from_millis(1)));
        g.on_input();
        assert!(
            g.should_draw(t0 + Duration::from_millis(2)),
            "输入抢占应无条件放行"
        );
        // 抢占是一次性的:下一轮恢复帧闸语义
        assert!(!g.should_draw(t0 + Duration::from_millis(3)));
    }

    #[test]
    fn fps_cap_clamped_to_legal_range() {
        assert_eq!(FpsGovernor::new(0).max_fps(), FPS_CAP_MIN);
        assert_eq!(FpsGovernor::new(200).max_fps(), FPS_CAP_MAX);
        let mut g = FpsGovernor::new(30);
        g.set_max_fps(1); // 越界热更新同样 clamp
        assert_eq!(g.max_fps(), FPS_CAP_MIN);
    }

    #[test]
    fn min_interval_monotonic_in_fps() {
        let slow = FpsGovernor::new(15).min_interval();
        let fast = FpsGovernor::new(60).min_interval();
        assert!(slow > fast, "帧率越高间隔越短");
    }

    #[test]
    fn animation_budgets_within_spec_bands() {
        assert_eq!(
            FpsGovernor::animation_budget(AnimationKind::Selection),
            Duration::ZERO
        );
        let view = FpsGovernor::animation_budget(AnimationKind::ViewTransition);
        assert!((Duration::from_millis(100)..=Duration::from_millis(200)).contains(&view));
        let chart = FpsGovernor::animation_budget(AnimationKind::ChartTransition);
        assert!((Duration::from_millis(200)..=Duration::from_millis(500)).contains(&chart));
    }
}
