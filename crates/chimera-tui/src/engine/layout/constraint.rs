//! engine::layout::constraint — 布局约束与一维求解(ADR-029,v3.1 M1.4)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **和恒等于 total**:`solve` 保证返回各段长度之和 == 输入 total(填满、不溢出、
//!   不留空),这是布局正确性的根本不变量——由 proptest 强校验。
//! - **flexbox 语义**:Fixed/Percent 为刚性基准;Min 提供下界并可增长;Max 提供
//!   上界的弹性段;Flex(w) 按权重瓜分剩余空间。与 ratatui `Constraint` 语义对齐。
//! - **v2.9.0-omega Task 2.6 扩展**:新增 `FlexBasis` / `Grow` / `Shrink` 三变体,
//!   对齐 CSS Flexible Box Layout Module Level 1 §9.7(W3C CR-flexbox-1-20181119)
//!   主轴分配算法:剩余空间 > 0 按 grow 因子分配,< 0 按 shrink × base 加权收缩。
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
    /// v2.9.0-omega Task 2.6:CSS flex-basis — 初始尺寸,可增长(grow=1)可收缩(shrink=1)
    ///
    /// 与 `Min` 的区别:`Min` 不可收缩(下界),`FlexBasis` 在空间不足时按 shrink 收缩。
    /// 学术参考:W3C CSS Flexbox §9.7 "Resolving Flexible Lengths"。
    FlexBasis(u16),
    /// v2.9.0-omega Task 2.6:CSS flex-grow — 增长因子,base=0,空间剩余时按因子比例分配
    Grow(u32),
    /// v2.9.0-omega Task 2.6:CSS flex-shrink — 收缩因子,base=0,空间不足时按因子标记可收缩
    ///
    /// 单独使用时 base=0 不主动收缩;与 `FlexBasis` 组合时由 `FlexBasis` 提供 base,
    /// `Shrink` 作为收缩权重标记。简化语义:shrink>0 的段在空间不足时优先承担收缩。
    Shrink(u32),
}

/// 沿一维轴求解各约束段的长度,返回值之和恒等于 `total`
///
/// # 算法(CSS Flexbox §9.7 简化版)
/// 1. 计算每段基准 `base`、增长权重 `weight`、上限 `cap`、收缩因子 `shrink`;
/// 2. 若基准之和 > total → 按 `shrink × base` 加权收缩(shrink=0 的段不收缩);
///    若所有 shrink=0,回退到按比例收缩全部段(保证和 == total);
/// 3. 若基准之和 < total → 按 `weight` 分配剩余空间给可增长段(尊重 cap);
/// 4. 末段吸收取整余量,保证和精确等于 total。
pub fn solve(total: u16, constraints: &[Constraint]) -> Vec<u16> {
    let n = constraints.len();
    if n == 0 {
        return Vec::new();
    }
    let total = total as u32;
    let mut base = vec![0u32; n];
    let mut weight = vec![0u32; n];
    let mut cap = vec![u32::MAX; n];
    let mut shrink = vec![0u32; n];
    for (i, c) in constraints.iter().enumerate() {
        match *c {
            Constraint::Fixed(v) => {
                base[i] = v as u32;
                cap[i] = v as u32;
                // Fixed 不可收缩(shrink=0),空间不足时按比例回退收缩
            }
            Constraint::Percent(p) => {
                let v = total * (p.min(100) as u32) / 100;
                base[i] = v;
                cap[i] = v;
            }
            Constraint::Min(v) => {
                base[i] = v as u32;
                weight[i] = 1;
                // Min 不可收缩(下界保证)
            }
            Constraint::Max(v) => {
                weight[i] = 1;
                cap[i] = v as u32;
            }
            Constraint::Flex(w) => {
                weight[i] = (w as u32).max(1);
            }
            Constraint::FlexBasis(v) => {
                // flex-basis:初始尺寸,可增长(weight=1)可收缩(shrink=1)
                base[i] = v as u32;
                weight[i] = 1;
                shrink[i] = 1;
            }
            Constraint::Grow(g) => {
                // flex-grow:base=0,按因子增长,不收缩
                weight[i] = g.max(1);
            }
            Constraint::Shrink(s) => {
                // flex-shrink:base=0,标记可收缩,不主动增长
                shrink[i] = s;
            }
        }
    }

    let base_sum: u32 = base.iter().sum();
    let mut sizes = base.clone();

    if base_sum > total {
        // 基准超出可用空间:按 shrink × base 加权收缩
        shrink_distribute(&mut sizes, &base, &shrink, total);
        // 收缩后若仍超出(全部 shrink=0 的回退场景),按比例收缩全部段
        let assigned: u32 = sizes.iter().sum();
        if assigned > total && base_sum > 0 {
            let mut acc = 0u32;
            for (i, s) in sizes.iter_mut().enumerate() {
                *s = (base[i] as u64 * total as u64 / base_sum as u64) as u32;
                acc += *s;
            }
            let mut idx = 0usize;
            while acc < total {
                sizes[idx % n] += 1;
                acc += 1;
                idx += 1;
            }
        }
    } else if base_sum < total {
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

/// 按 `shrink × base` 加权收缩各段,使总和收敛到 `total`(CSS Flexbox §9.7)
///
/// WHY shrink × base 加权:CSS 规范要求收缩量按 `shrink_factor × base_size` 比例
/// 分配,而非纯 shrink_factor。这样大 base 的段承担更多收缩,避免小段被压扁。
/// shrink=0 的段(Fixed/Min)不参与收缩,保持原 base。
///
/// # 算法
/// 1. 计算每段收缩权重 `sw = shrink × base`;
/// 2. 总收缩量 `deficit = base_sum - total`;
/// 3. 每段收缩 `deficit × sw_i / sum(sw)`,下界 0;
/// 4. 取整余量逐一扣减,保证总和精确 == total。
fn shrink_distribute(sizes: &mut [u32], base: &[u32], shrink: &[u32], total: u32) {
    let n = sizes.len();
    let base_sum: u32 = base.iter().sum();
    let deficit = base_sum.saturating_sub(total);
    if deficit == 0 {
        return;
    }
    // 收缩权重 = shrink × base;shrink=0 的段不参与
    let sw: Vec<u32> = (0..n).map(|i| shrink[i] * base[i]).collect();
    let total_sw: u32 = sw.iter().sum();
    if total_sw == 0 {
        return; // 无可收缩段,交由调用方回退处理
    }
    let mut remaining = deficit;
    for i in 0..n {
        if sw[i] == 0 {
            continue;
        }
        let cut = (deficit * sw[i]) / total_sw;
        let cut = cut.min(sizes[i]); // 不下溢
        sizes[i] -= cut;
        remaining = remaining.saturating_sub(cut);
    }
    // 取整余量:从 sw>0 的段继续扣减 1,直至耗尽
    let mut idx = 0usize;
    while remaining > 0 {
        let i = idx % n;
        if sw[i] > 0 && sizes[i] > 0 {
            sizes[i] -= 1;
            remaining -= 1;
        }
        idx += 1;
        if idx > n * 4 {
            break; // 安全熔断:避免极端输入下死循环
        }
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

    // === v2.9.0-omega Task 2.6 新增:flex-grow/shrink 测试 ===

    #[test]
    fn flex_grow_distributes_remaining_space() {
        // 总宽 100,Fixed(20) 占 20,剩 80 按 Grow(1):Grow(2) = 1:2 分配
        // 期望:[20, 26, 53](整数,和=99,末段补 1 → [20, 26, 54] 或类似,和=100)
        let sizes = solve(
            100,
            &[
                Constraint::Fixed(20),
                Constraint::Grow(1),
                Constraint::Grow(2),
            ],
        );
        assert_eq!(sizes.iter().sum::<u16>(), 100, "和必须等于 total");
        assert_eq!(sizes[0], 20, "Fixed 段保持 20");
        // Grow(2) 应是 Grow(1) 的 2 倍左右
        assert!(
            sizes[2] >= sizes[1],
            "Grow(2) 分配应不少于 Grow(1):{} >= {}",
            sizes[2],
            sizes[1]
        );
        // 近似比例:80 按 1:2 分 → 26.67 : 53.33
        let total_grow = sizes[1] + sizes[2];
        assert_eq!(total_grow, 80, "两个 Grow 段应瓜分剩余 80");
    }

    #[test]
    fn flex_shrink_contracts_when_overflow() {
        // 总宽 50,FlexBasis(30) + FlexBasis(30) base_sum=60 > 50,按 shrink 收缩
        let sizes = solve(50, &[Constraint::FlexBasis(30), Constraint::FlexBasis(30)]);
        assert_eq!(sizes.iter().sum::<u16>(), 50, "收缩后和必须等于 total");
        // 两个 FlexBasis 等权收缩:各 25
        assert_eq!(sizes[0], 25);
        assert_eq!(sizes[1], 25);
    }

    #[test]
    fn flex_basis_grows_with_remaining_space() {
        // FlexBasis(20) + Grow(1) 总宽 100:FlexBasis base=20 可增长,Grow base=0 可增长
        // 剩余 80 按 weight 1:1 分配 → [60, 40]
        let sizes = solve(100, &[Constraint::FlexBasis(20), Constraint::Grow(1)]);
        assert_eq!(sizes.iter().sum::<u16>(), 100);
        // FlexBasis 应从 20 增长(吸收部分剩余)
        assert!(
            sizes[0] >= 20,
            "FlexBasis 应至少保持 base 20,实际 {}",
            sizes[0]
        );
        assert_eq!(sizes[1], 40, "Grow(1) 应分得一半剩余 40");
    }

    #[test]
    fn grow_respects_max_bound() {
        // Max(15) 限制 Grow 段最多 15,剩余归 Flex
        let sizes = solve(
            100,
            &[
                Constraint::Max(15), // 最多 15
                Constraint::Flex(1), // 吸收剩余
            ],
        );
        assert_eq!(sizes.iter().sum::<u16>(), 100);
        assert_eq!(sizes[0], 15, "Max(15) 封顶");
        assert_eq!(sizes[1], 85);
    }

    proptest! {
        /// 核心不变量:任意约束组合,各段长度之和恒等于 total(填满不溢出)
        #[test]
        fn sum_always_equals_total(
            total in 0u16..500,
            specs in prop::collection::vec(0u8..8, 1..8),
        ) {
            // 将随机判别值映射为不同约束类型(含 v2.9 flex 变体),构造随机约束序列
            let constraints: Vec<Constraint> = specs
                .iter()
                .map(|&k| match k {
                    0 => Constraint::Fixed(10),
                    1 => Constraint::Percent(25),
                    2 => Constraint::Min(5),
                    3 => Constraint::Max(30),
                    4 => Constraint::Flex(2),
                    5 => Constraint::FlexBasis(15),
                    6 => Constraint::Grow(2),
                    _ => Constraint::Shrink(1),
                })
                .collect();
            let sizes = solve(total, &constraints);
            prop_assert_eq!(sizes.len(), constraints.len());
            prop_assert_eq!(sizes.iter().map(|&s| s as u32).sum::<u32>(), total as u32);
        }
    }
}
