//! ClvSummary::from_clv_slice 计算方法集成测试(P1-3 测试外移)
//!
//! 从 types.rs 内联测试模块外移(配合 P1-2 解耦:切片签名 + 不再依赖 CLV 类型)。
//! 集成测试仅使用公共 API(`event_bus::ClvSummary`),验证 8 分块均值 /
//! L2 范数 / Top-8 降序 / 零向量与过短切片边界。

use event_bus::ClvSummary;

#[test]
fn test_clv_summary_from_clv_zero_vector() {
    // 零向量:l2_norm = 0.0, block_means 全 0, top_dims 空
    let slice = vec![0.0_f32; 512];
    let summary = ClvSummary::from_clv_slice(&slice);
    assert_eq!(summary.block_means.len(), 8);
    assert!(summary.block_means.iter().all(|&v| v == 0.0));
    assert_eq!(summary.l2_norm, 0.0);
    assert!(summary.top_dims.is_empty());
}

#[test]
fn test_clv_summary_from_clv_uniform_vector() {
    // 均匀向量(全 1.0):所有分块均值 = 1.0, l2_norm = sqrt(512) ≈ 22.63
    let slice = vec![1.0_f32; 512];
    let summary = ClvSummary::from_clv_slice(&slice);
    assert_eq!(summary.block_means.len(), 8);
    assert!(summary.block_means.iter().all(|&m| (m - 1.0).abs() < 1e-5));
    let expected_norm = (512.0_f32).sqrt();
    assert!((summary.l2_norm - expected_norm).abs() < 1e-3);
    // Top-8: 所有维度值相同,取前 8 个(索引 0-7)
    assert_eq!(summary.top_dims.len(), 8);
    // 所有 |值| = 1.0,排序后前 8 个任意,但值都应为 1.0
    assert!(summary
        .top_dims
        .iter()
        .all(|&(_, v)| (v - 1.0).abs() < 1e-5));
}

#[test]
fn test_clv_summary_from_clv_known_vector() {
    // 已知向量:前 64 维 = 2.0,其余 = 0.0
    // block_means[0] = 2.0, block_means[1..8] = 0.0
    // l2_norm = sqrt(64 * 4) = sqrt(256) = 16.0
    // top_dims: 前 8 个应是维度 0-7(值 2.0)
    let mut slice = vec![0.0_f32; 512];
    for val in slice.iter_mut().take(64) {
        *val = 2.0;
    }
    let summary = ClvSummary::from_clv_slice(&slice);
    assert!((summary.block_means[0] - 2.0).abs() < 1e-5);
    for i in 1..8 {
        assert!((summary.block_means[i] - 0.0).abs() < 1e-5);
    }
    assert!((summary.l2_norm - 16.0).abs() < 1e-3);
    assert_eq!(summary.top_dims.len(), 8);
    // 前 8 个应是维度 0-7(值 2.0)
    assert!(summary
        .top_dims
        .iter()
        .all(|&(_, v)| (v - 2.0).abs() < 1e-5));
}

#[test]
fn test_clv_summary_from_clv_block_means_length() {
    // 验证 512 维切片 block_means 长度始终为 8
    let slice = vec![0.0_f32; 512];
    let summary = ClvSummary::from_clv_slice(&slice);
    assert_eq!(summary.block_means.len(), 8);
}

#[test]
fn test_clv_summary_from_clv_short_slice_boundary() {
    // P1-2 新增边界测试:长度 < 8 的切片 block_size = 0,
    // block_means 返回空 Vec(防 slice 越界与除零),不 panic
    let slice = vec![1.0_f32; 4];
    let summary = ClvSummary::from_clv_slice(&slice);
    assert!(summary.block_means.is_empty());
    // l2_norm = sqrt(4 * 1) = 2.0,仍可正常计算
    assert!((summary.l2_norm - 2.0).abs() < 1e-5);
}

#[test]
fn test_clv_summary_from_clv_top_dims_sorted_desc() {
    // 验证 top_dims 按 |值| 降序排列
    let mut slice = vec![0.0_f32; 512];
    slice[0] = 5.0; // |5.0|
    slice[1] = 3.0; // |3.0|
    slice[2] = -4.0; // |4.0|
    slice[3] = 1.0; // |1.0|
    slice[4] = -2.0; // |2.0|
    let summary = ClvSummary::from_clv_slice(&slice);
    assert!(!summary.top_dims.is_empty());
    // 验证降序:|v[0]| >= |v[1]| >= ...
    for i in 1..summary.top_dims.len() {
        let prev_abs = summary.top_dims[i - 1].1.abs();
        let curr_abs = summary.top_dims[i].1.abs();
        assert!(
            prev_abs >= curr_abs || (prev_abs - curr_abs).abs() < 1e-5,
            "top_dims not sorted desc: |{}| < |{}|",
            prev_abs,
            curr_abs
        );
    }
    // 第一个应是维度 0(值 5.0)
    assert_eq!(summary.top_dims[0].0, 0);
}

#[test]
fn test_clv_summary_from_clv_negative_values() {
    // 负值向量:验证 |值| 正确排序
    let mut slice = vec![0.0_f32; 512];
    slice[0] = -5.0; // |-5.0| = 5.0
    slice[1] = 3.0; // |3.0| = 3.0
    let summary = ClvSummary::from_clv_slice(&slice);
    // 第一个应是维度 0(值 -5.0,|值|最大)
    assert_eq!(summary.top_dims[0].0, 0);
    assert!((summary.top_dims[0].1 - (-5.0)).abs() < 1e-5);
    // 第二个应是维度 1(值 3.0)
    assert_eq!(summary.top_dims[1].0, 1);
}
