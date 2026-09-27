//! LSCT 任务感知策略装配 — 闭环集成测试(ADR-193 整并后同 crate 模块装配)
//!
//! 对应架构层:L3 Storage(lsct 为 cmt-tiering 内生 `lsct` 子模块,ADR-193)
//! 对应任务:原 ADR-160 冻结孤岛偿还(M10)装配,现由同 crate 模块承接
//!
//! # 测试矩阵
//! - `test_lsct_policy_closed_loop_on_quest_created`:端到端闭环 —
//!   QuestCreated 事件 → 策略腿 reconcile 种子 + `handle_quest_created`
//!   (真实调用 lsct API)→ LsctTierSwitched → 既有订阅腿执行真实迁移
//!
//! # 已退役测试(ADR-193)
//! - `test_lsct_dep_is_production_not_dev`:跨 crate 生产边 manifest 锁定 —
//!   随整并退役(同 crate 模块无 manifest 边可锁)
//! - `test_tier_conversion_roundtrip`:cmt↔lsct Tier 双向转换 —
//!   Tier 已统一为单一实现,恒等映射无意义
//!
//! WHY 本文件存在(语义非空转论证,ADR-179 判据):
//! "任务负载 → 策略 → 事件 → 实际迁移" 四段链路全部走真实生产调用路径。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
use std::sync::Arc;
use std::time::Duration;

use cmt_tiering::lsct::LsctConfig;
use cmt_tiering::{CapabilityEntry, CmtConfig, CmtCoordinator, Tier};
use event_bus::{EventBus, EventMetadata, NexusEvent};

/// 构造指定层级的能力条目(CapabilityEntry::new 的测试便利包装)
fn entry_at(id: &str, tier: Tier) -> CapabilityEntry {
    CapabilityEntry::new(id, "lsct policy test content", tier)
}

/// 端到端闭环:QuestCreated 驱动 LSCT 策略,CMT 订阅者执行真实迁移
///
/// 链路:QuestCreated(title=debug) → reconcile 把 cap-1 注册进 LSCT
/// → handle_quest_created 计算画像(Debug 低强度 → 目标冷层)
/// → LsctTierSwitched(Hot→Warm 逐级)→ apply_lsct_migration 真实迁移
///
/// WHY 从 Hot 出发降级:CmtCoordinator::insert 的公开语义是"写入 Hot 层
/// (跨层查找自动提升的设计基线)",冷层条目由降级/LRU 驱逐产生。
/// 因此闭环验证走 Hot → Warm 降级方向(Debug/Test 类 Quest)。
#[tokio::test]
async fn test_lsct_policy_closed_loop_on_quest_created() {
    let bus = EventBus::new();
    let coord = CmtCoordinator::new_in_memory(CmtConfig::default(), bus.clone()).unwrap();
    // insert 公开路径恒入 Hot 层(entry.tier 字段由迁移路径维护)
    coord.insert(entry_at("cap-1", Tier::Hot)).await.unwrap();
    let coord = Arc::new(coord);

    // 既有订阅腿(执行迁移)+ 新策略腿(生产决策)—— 两 legs 齐备闭环才成立
    let _subscriber = coord.spawn_lsct_subscriber();
    let policy = coord.spawn_lsct_policy(LsctConfig::default());

    // 调试类低强度 Quest:TaskLoadProfile::from_quest_title("debug ...") → Debug/0.2
    // → 目标冷层,cap-1 逐级降温第一步 Hot→Warm
    bus.publish(NexusEvent::QuestCreated {
        metadata: EventMetadata::new("lsct-policy-test"),
        quest_id: "q-1".into(),
        title: "debug flaky failure reproduce".into(),
        task_count: 3,
    })
    .await
    .unwrap();

    // 轮询验证真实迁移效果:cap-1 从 Hot 降温到 Warm(迁移经
    // LsctTierSwitched → apply_lsct_migration,非 insert 语义)
    let mut migrated = false;
    for _ in 0..50 {
        if coord
            .list(Tier::Warm)
            .await
            .unwrap()
            .iter()
            .any(|e| e.id.as_str() == "cap-1")
        {
            migrated = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        migrated,
        "QuestCreated 应驱动 LSCT 策略闭环,把 cap-1 从 Hot 降温到 Warm"
    );

    // 策略装配真实持有 lsct 协调器实例(reconcile 已注册种子,非空)
    assert!(
        !policy.coordinator().is_empty(),
        "reconcile 应把 CMT 已有能力注册进 LSCT 策略层"
    );
    assert_eq!(
        policy.coordinator().get_tier("cap-1"),
        Some(Tier::Warm),
        "LSCT 侧 assignment 应随决策更新到 Warm(逐级降温第一步;Tier 已统一为单一实现)"
    );
}
