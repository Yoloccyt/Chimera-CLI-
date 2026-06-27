//! 任务负载画像 — 从 Quest 标题推断任务类型,计算目标存储层级
//!
//! 对应架构层:L3 Storage
//!
//! # 核心逻辑
//! - `from_quest_title`:关键字匹配推断任务类型(compile/debug/test/run)与强度
//! - `compute_target_tier`:按任务类型与强度决定目标层级(Hot/Warm/Cold/Ice)
//!
//! # 单调性不变量(供 proptest 验证)
//! 对同一 task_type,intensity 越高 → tier_rank 越小(越热)或相等,
//! 绝不会出现"高强度得到更冷的 tier"。

use crate::types::{TaskLoadProfile, TaskType};
use cmt_tiering::Tier;

/// 升温阈值(与 LsctConfig::default 一致)
const PROMOTION_THRESHOLD: f32 = 0.7;
/// 降温阈值(与 LsctConfig::default 一致)
const DEMOTION_THRESHOLD: f32 = 0.3;

impl TaskLoadProfile {
    /// 创建任务负载画像
    ///
    /// # 参数
    /// - `task_type`:任务类型
    /// - `intensity`:任务强度 [0.0, 1.0]
    /// - `frequency`:任务频率(单位时间执行次数)
    pub fn new(task_type: TaskType, intensity: f32, frequency: u32) -> Self {
        Self {
            task_type,
            intensity,
            frequency,
        }
    }

    /// 从 Quest 标题推断任务类型与强度
    ///
    /// 关键字匹配规则(大小写不敏感,按优先级 compile > debug > test > run):
    /// - "build"/"compile" → Compile(含 "release"/"production" → 0.9,否则 0.8)
    /// - "debug" → Debug(0.2,低强度,调试任务降温)
    /// - "test" → Test(0.5,中强度)
    /// - "run"/"execute" → Run(0.9,高强度,运行任务升温)
    /// - 默认 → Run(0.9,确保快速访问)
    ///
    /// WHY 默认 Run:无法识别的任务保守视为运行任务放热层,
    /// 避免误降温导致响应延迟(架构红线:优先正确性)。
    pub fn from_quest_title(title: &str) -> Self {
        let lower = title.to_lowercase();
        let task_type = if lower.contains("build") || lower.contains("compile") {
            TaskType::Compile
        } else if lower.contains("debug") {
            TaskType::Debug
        } else if lower.contains("test") {
            TaskType::Test
        } else {
            // "run"/"execute" 及未识别任务都默认为 Run
            // WHY:确保快速访问,避免误降温导致响应延迟(架构红线:优先正确性)
            TaskType::Run
        };

        // 强度按任务类型与修饰关键字推断
        let intensity = match task_type {
            TaskType::Compile => {
                if lower.contains("release") || lower.contains("production") {
                    0.9
                } else {
                    0.8
                }
            }
            TaskType::Debug => 0.2,
            TaskType::Test => 0.5,
            TaskType::Run => 0.9,
        };

        Self::new(task_type, intensity, 1)
    }
}

/// 计算目标存储层级 — 按任务类型与强度决定
///
/// 决策矩阵(tier_rank:Hot=0 < Warm=1 < Cold=2 < Ice=3):
///
/// | task_type | 高(≥0.7) | 中(0.3, 0.7) | 低(≤0.3) |
/// |-----------|----------|--------------|----------|
/// | Run       | Hot      | Hot          | Hot      |
/// | Compile   | Hot      | Warm         | Cold     |
/// | Debug     | Warm     | Cold         | Ice      |
/// | Test      | Warm     | Warm         | Cold     |
///
/// WHY Run 始终 Hot:运行任务需要快速响应,无论强度都放热层。
/// WHY Debug 降两级:调试任务访问频率低,放冷/冰层节省热层容量。
pub fn compute_target_tier(profile: &TaskLoadProfile) -> Tier {
    let high = profile.intensity >= PROMOTION_THRESHOLD;
    let low = profile.intensity <= DEMOTION_THRESHOLD;

    match profile.task_type {
        TaskType::Run => Tier::Hot,
        TaskType::Compile => {
            if high {
                Tier::Hot
            } else if low {
                Tier::Cold
            } else {
                Tier::Warm
            }
        }
        TaskType::Debug => {
            if high {
                Tier::Warm
            } else if low {
                Tier::Ice
            } else {
                Tier::Cold
            }
        }
        TaskType::Test => {
            if low {
                Tier::Cold
            } else {
                Tier::Warm
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tier_rank;

    // === TaskLoadProfile::new 测试 ===

    #[test]
    fn test_new_basic() {
        let profile = TaskLoadProfile::new(TaskType::Compile, 0.8, 5);
        assert_eq!(profile.task_type, TaskType::Compile);
        assert!((profile.intensity - 0.8).abs() < f32::EPSILON);
        assert_eq!(profile.frequency, 5);
    }

    // === from_quest_title 测试 ===

    #[test]
    fn test_from_quest_title_compile() {
        let p = TaskLoadProfile::from_quest_title("build the project");
        assert_eq!(p.task_type, TaskType::Compile);
        assert!((p.intensity - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_from_quest_title_compile_release() {
        let p = TaskLoadProfile::from_quest_title("compile production release");
        assert_eq!(p.task_type, TaskType::Compile);
        assert!((p.intensity - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_from_quest_title_debug() {
        let p = TaskLoadProfile::from_quest_title("debug memory leak");
        assert_eq!(p.task_type, TaskType::Debug);
        assert!((p.intensity - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn test_from_quest_title_test_priority() {
        // WHY "run unit test suite" 同时含 "run" 与 "test",
        // 按优先级 compile > debug > test > run,匹配 Test
        let p = TaskLoadProfile::from_quest_title("run unit test suite");
        assert_eq!(p.task_type, TaskType::Test);
        assert!((p.intensity - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_from_quest_title_run() {
        let p = TaskLoadProfile::from_quest_title("execute the server");
        assert_eq!(p.task_type, TaskType::Run);
        assert!((p.intensity - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_from_quest_title_unknown_default_run() {
        let p = TaskLoadProfile::from_quest_title("analyze data");
        assert_eq!(p.task_type, TaskType::Run);
    }

    #[test]
    fn test_from_quest_title_case_insensitive() {
        let p = TaskLoadProfile::from_quest_title("BUILD PROJECT");
        assert_eq!(p.task_type, TaskType::Compile);
    }

    // === compute_target_tier:4 task_type × 3 intensity(12 个核心测试)===

    #[test]
    fn test_compute_compile_high() {
        let p = TaskLoadProfile::new(TaskType::Compile, 0.9, 1);
        assert_eq!(compute_target_tier(&p), Tier::Hot);
    }

    #[test]
    fn test_compute_compile_mid() {
        let p = TaskLoadProfile::new(TaskType::Compile, 0.5, 1);
        assert_eq!(compute_target_tier(&p), Tier::Warm);
    }

    #[test]
    fn test_compute_compile_low() {
        let p = TaskLoadProfile::new(TaskType::Compile, 0.1, 1);
        assert_eq!(compute_target_tier(&p), Tier::Cold);
    }

    #[test]
    fn test_compute_debug_high() {
        let p = TaskLoadProfile::new(TaskType::Debug, 0.9, 1);
        assert_eq!(compute_target_tier(&p), Tier::Warm);
    }

    #[test]
    fn test_compute_debug_mid() {
        let p = TaskLoadProfile::new(TaskType::Debug, 0.5, 1);
        assert_eq!(compute_target_tier(&p), Tier::Cold);
    }

    #[test]
    fn test_compute_debug_low() {
        let p = TaskLoadProfile::new(TaskType::Debug, 0.1, 1);
        assert_eq!(compute_target_tier(&p), Tier::Ice);
    }

    #[test]
    fn test_compute_test_high() {
        let p = TaskLoadProfile::new(TaskType::Test, 0.9, 1);
        assert_eq!(compute_target_tier(&p), Tier::Warm);
    }

    #[test]
    fn test_compute_test_mid() {
        let p = TaskLoadProfile::new(TaskType::Test, 0.5, 1);
        assert_eq!(compute_target_tier(&p), Tier::Warm);
    }

    #[test]
    fn test_compute_test_low() {
        let p = TaskLoadProfile::new(TaskType::Test, 0.1, 1);
        assert_eq!(compute_target_tier(&p), Tier::Cold);
    }

    #[test]
    fn test_compute_run_high() {
        let p = TaskLoadProfile::new(TaskType::Run, 0.9, 1);
        assert_eq!(compute_target_tier(&p), Tier::Hot);
    }

    #[test]
    fn test_compute_run_mid() {
        let p = TaskLoadProfile::new(TaskType::Run, 0.5, 1);
        assert_eq!(compute_target_tier(&p), Tier::Hot);
    }

    #[test]
    fn test_compute_run_low() {
        let p = TaskLoadProfile::new(TaskType::Run, 0.1, 1);
        assert_eq!(compute_target_tier(&p), Tier::Hot);
    }

    // === 边界测试:阈值边界值归属 ===

    #[test]
    fn test_compute_boundary_promotion_threshold() {
        // intensity == 0.7 归为高(>= 阈值)
        let p = TaskLoadProfile::new(TaskType::Compile, 0.7, 1);
        assert_eq!(compute_target_tier(&p), Tier::Hot);
    }

    #[test]
    fn test_compute_boundary_demotion_threshold() {
        // intensity == 0.3 归为低(<= 阈值)
        let p = TaskLoadProfile::new(TaskType::Compile, 0.3, 1);
        assert_eq!(compute_target_tier(&p), Tier::Cold);
    }

    // === 单调性验证(proptest 不变量的单元版本)===

    #[test]
    fn test_monotonicity_compile() {
        let low = compute_target_tier(&TaskLoadProfile::new(TaskType::Compile, 0.1, 1));
        let mid = compute_target_tier(&TaskLoadProfile::new(TaskType::Compile, 0.5, 1));
        let high = compute_target_tier(&TaskLoadProfile::new(TaskType::Compile, 0.9, 1));
        assert!(tier_rank(low) >= tier_rank(mid));
        assert!(tier_rank(mid) >= tier_rank(high));
    }

    #[test]
    fn test_monotonicity_debug() {
        let low = compute_target_tier(&TaskLoadProfile::new(TaskType::Debug, 0.1, 1));
        let mid = compute_target_tier(&TaskLoadProfile::new(TaskType::Debug, 0.5, 1));
        let high = compute_target_tier(&TaskLoadProfile::new(TaskType::Debug, 0.9, 1));
        assert!(tier_rank(low) >= tier_rank(mid));
        assert!(tier_rank(mid) >= tier_rank(high));
    }

    #[test]
    fn test_monotonicity_test() {
        let low = compute_target_tier(&TaskLoadProfile::new(TaskType::Test, 0.1, 1));
        let high = compute_target_tier(&TaskLoadProfile::new(TaskType::Test, 0.9, 1));
        assert!(tier_rank(low) >= tier_rank(high));
    }
}
