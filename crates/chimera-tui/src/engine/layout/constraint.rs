//! engine::layout::constraint — 布局约束与一维求解(ADR-029,v3.1 M1.4)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **和恒等于 total**:`solve` 保证返回各段长度之和 == 输入 total(填满、不溢出、
//!   不留空),这是布局正确性的根本不变量——由 proptest 强校验。
//! - **flexbox 语义**:Fixed/Percent 为刚性基准;Min 提供下界并可增长;Max 提供
//!   上界的弹性段;Flex(w) 按权重瓜分剩余空间。与 ratatui `Constraint` 语义对齐。
//! - **纯 safe + 整数运算**:用 `u32` 中间量避免 `u16` 乘法溢出,末段吸收取整余量。

/// 布局主轴方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 水平排列(沿 x 轴切分宽度)
    Horizontal,
    /// 垂直排列(沿 y 轴切分高度)
    Vertical,
}

/// 尺寸约束 — 描述一个段在主轴上如何取长
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Constraint {
    /// 固定长度(字符数)
    Fixed(u16),
    /// 占总长的百分比([0,100],超出按 100 处理)
    Percent(u8),
    /// 最小长度,可向上增长(grow 权重 1)
    Min(u16),
    /// 最大长度的弹性段(基准 0,grow 权重 1,封顶 Max)
    Max(u16),
    /// 弹性段:按权重 `w`(≥1)瓜分剩余空间
    Flex(u16),
}

/// 沿一维轴求解各约束段的长度,返回值之和恒等于 `total`
///
/// # 算法
/// 1. 计算每段基准 `base`、增长权重 `weight`、上限 `cap`;
/// 2. 若基准之和 ≥ total → 按比例收缩到 total(整数,余量逐一补);
/// 3. 否则将剩余空间按权重分配给可增长段(尊重 cap),末段吸收最终余量。
pub fn solve(total: u16, constraints: &[Constraint]) -> Vec<u16> {
    let n = constraints.len();
    if n == 0 {
        return Vec::new();
    }
    let total = total as u32;
    let mut base = vec![0u32; n];
    let mut weight = vec![0u32; n];
    let mut cap = vec![u32::MAX; n];
    for (i, c) in constraints.iter().enumerate() {
        match *c {
            Constraint::Fixed(v) => {
                base[i] = v as u32;
                cap[i] = v as u32;
            }
            Constraint::Percent(p) => {
                let v = total * (p.min(100) as u32) / 100;
                base[i] = v;
                cap[i] = v;
            }
            Constraint::Min(v) => {
                base[i] = v as u32;
                weight[i] = 1;
            }
            Constraint::Max(v) => {
                weight[i] = 1;
                cap[i] = v as u32;
            }
            Constraint::Flex(w) => {
                weight[i] = (w as u32).max(1);
            }
        }
    }

    let base_sum: u32 = base.iter().sum();
    let mut sizes = base.clone();

    if base_sum >= total {
        // 基准超出可用空间:按比例收缩到 total,取整余量逐一补齐
        if base_sum > 0 {
            let mut assigned = 0u32;
            for (i, s) in sizes.iter_mut().enumerate() {
                *s = (base[i] as u64 * total as u64 / base_sum as u64) as u32;
                assigned += *s;
            }
            let mut idx = 0usize;
            while assigned < total {
                sizes[idx % n] += 1;
                assigned += 1;
                idx += 1;
            }
        }
    } else {
        // 有剩余空间:按权重分配给可增长段(尊重 cap)
        grow_distribute(&mut sizes, &weight, &cap, total - base_sum);
        // 若仍有余量(全刚性段且未填满),末段吸收,保证和 == total
        let assigned: u32 = sizes.iter().sum();
        if assigned < total {
            sizes[n - 1] += total - assigned;
        }
    }

    sizes
        .iter()
        .map(|&s| s.min(u16::MAX as u32) as u16)
        .collect()
}

/// 将 `leftover` 空间按权重分配给可增长段(`weight>0` 且未达 `cap`),尊重上限
///
/// WHY 迭代分配:一次按比例分配后余量若小于权重和,则逐一补 1,保证精确耗尽;
/// 每轮至少移动 1 单位或提前退出,循环必终止。
fn grow_distribute(sizes: &mut [u32], weight: &[u32], cap: &[u32], mut leftover: u32) {
    let n = sizes.len();
    while leftover > 0 {
        let total_w: u32 = (0..n)
            .filter(|&i| weight[i] > 0 && sizes[i] < cap[i])
            .map(|i| weight[i])
            .sum();
        if total_w == 0 {
            break; // 无可增长段(全部封顶),剩余由调用方处理
        }
        let mut moved = 0u32;
        for i in 0..n {
            if weight[i] == 0 || sizes[i] >= cap[i] {
                continue;
            }
            let share = (leftover * weight[i]) / total_w;
            if share == 0 {
                continue;
            }
            let grant = share.min(cap[i] - sizes[i]);
            sizes[i] += grant;
            moved += grant;
        }
        if moved == 0 {
            // 比例份额均取整为 0:按顺序逐一补 1,直至耗尽本轮 leftover
            for i in 0..n {
                if moved >= leftover {
                    break;
                }
                if weight[i] > 0 && sizes[i] < cap[i] {
                    sizes[i] += 1;
                    moved += 1;
                }
            }
        }
        if moved == 0 {
            break; // 全部封顶,无法继续
        }
        leftover -= moved;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn empty_constraints_yield_empty() {
        assert!(solve(100, &[]).is_empty());
    }

    #[test]
    fn fixed_constraints_honored_and_last_absorbs_slack() {
        // 两个固定段 + 剩余空间 → 末段吸收(无弹性段时)
        let sizes = solve(20, &[Constraint::Fixed(5), Constraint::Fixed(5)]);
        assert_eq!(sizes.iter().sum::<u16>(), 20);
        assert_eq!(sizes[0], 5);
        assert_eq!(sizes[1], 15); // 末段吸收 10 余量
    }

    #[test]
    fn flex_splits_leftover_by_weight() {
        // Fixed(10) 占 10,剩 90 按 2:1 分给两个 Flex
        let sizes = solve(
            100,
            &[
                Constraint::Fixed(10),
                Constraint::Flex(2),
                Constraint::Flex(1),
            ],
        );
        assert_eq!(sizes.iter().sum::<u16>(), 100);
        assert_eq!(sizes[0], 10);
        assert_eq!(sizes[1], 60);
        assert_eq!(sizes[2], 30);
    }

    #[test]
    fn percent_and_min_combined() {
        // Percent(50)=50, Min(10) 基准 10 且吸收剩余 40 → 50/50
        let sizes = solve(100, &[Constraint::Percent(50), Constraint::Min(10)]);
        assert_eq!(sizes.iter().sum::<u16>(), 100);
        assert_eq!(sizes[0], 50);
        assert_eq!(sizes[1], 50);
    }

    #[test]
    fn max_caps_growth() {
        // Max(20) 段最多 20,剩余归 Flex
        let sizes = solve(100, &[Constraint::Max(20), Constraint::Flex(1)]);
        assert_eq!(sizes.iter().sum::<u16>(), 100);
        assert_eq!(sizes[0], 20);
        assert_eq!(sizes[1], 80);
    }

    #[test]
    fn oversized_base_shrinks_to_total() {
        // 基准之和(30+30)超出 total=40 → 按比例收缩,和仍 == 40
        let sizes = solve(40, &[Constraint::Fixed(30), Constraint::Fixed(30)]);
        assert_eq!(sizes.iter().sum::<u16>(), 40);
    }

    proptest! {
        /// 核心不变量:任意约束组合,各段长度之和恒等于 total(填满不溢出)
        #[test]
        fn sum_always_equals_total(
            total in 0u16..500,
            specs in prop::collection::vec(0u8..5, 1..8),
        ) {
            // 将随机判别值映射为不同约束类型,构造随机约束序列
            let constraints: Vec<Constraint> = specs
                .iter()
                .map(|&k| match k {
                    0 => Constraint::Fixed(10),
                    1 => Constraint::Percent(25),
                    2 => Constraint::Min(5),
                    3 => Constraint::Max(30),
                    _ => Constraint::Flex(2),
                })
                .collect();
            let sizes = solve(total, &constraints);
            prop_assert_eq!(sizes.len(), constraints.len());
            prop_assert_eq!(sizes.iter().map(|&s| s as u32).sum::<u32>(), total as u32);
        }
    }
}
