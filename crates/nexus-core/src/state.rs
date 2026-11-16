//! NexusState — 全局运行时状态(线程安全)
//!
//! 对应架构层:L1 Core
//! 对应创新点:NexusState — 所有上层 crate 共享的全局状态快照
//!
//! # 设计决策(WHY)
//! - **`Arc<RwLock>`**:读多写少(快照哈希、查询 Quest 远比注册频繁),
//!   RwLock 允许并发读,提升吞吐
//! - **Clone 基于 Arc**:NexusState 克隆是廉价的 Arc 引用计数递增,
//!   可跨线程传递(如 Event Bus 回调、异步任务)
//! - **快照哈希确定性**:snapshot_hash 按 quest_id 排序后哈希,
//!   保证相同状态产生相同哈希(HashMap 迭代序不确定,需排序)
//! - **快照哈希缓存(R2.3)**:写操作将缓存置 None 失效,读路径双检后
//!   惰性重算并缓存——「读多写少」下后续读取零重算成本,notify() 触发的
//!   重算结果亦被缓存(此前 notify 与读取各算一次,天然 2 倍写放大)
//!
//! # 架构红线
//! - 所有状态变更通过 Event Bus 广播(NexusStateChanged 事件)
//! - 上层 crate 不直接修改 NexusState,只通过事件订阅感知变更
//!
//! # 状态变更通知(P1-1 接线,L1 Core 深度优化)
//! 架构红线"状态变更必须广播"的落地采用**依赖倒置**:本模块定义最小
//! [`StateChangeListener`] trait(纯 std,零新依赖),EventBus 适配器实现
//! 在上层(L9 quest-engine `state_listener` 模块)。P1-2 解耦后 event-bus 对
//! nexus-core 已零依赖(ClvSummary::from_clv_slice 改签),nexus-core 同层依赖
//! event-bus 合法且无环(`CLV::report_snapshot` 即经此直接发布);依赖倒置现仅
//! 承载"适配层归 L9"语义(L1 不可向上依赖 L9,§2.2),故实现留在上层。
//! 回调在写锁**释放后**执行(红线:不持锁调外部代码),且经 catch_unwind
//! 隔离(R2.1:监听器 panic 不得影响状态变更——注释契约原承诺,本批次落地)。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use sha2::{Digest, Sha256};

use nexus_contracts::domain::Quest;

use crate::error::NexusError;

/// NexusState 内部状态 — 不对外公开,仅通过 NexusState 方法访问
#[derive(Default)]
struct NexusStateInner {
    /// 活跃 Quest 映射(quest_id → Quest)
    active_quests: HashMap<String, Quest>,
    /// 全局预算(已消耗 token 数)
    global_budget: u64,
    /// 模型注册表快照(model_id 列表)
    model_registry_snapshot: Vec<String>,
    /// 快照哈希缓存(R2.3):写操作置 None 失效,读路径双检后惰性重算并缓存。
    /// None = 已失效,下次读取时重算(缓存是纯派生态,任何时刻重算结果一致,
    /// 因此缓存不引入新的语义,只消除读放大)。
    cached_hash: Option<String>,
}

/// 状态变更类型 — NexusState 变更通知的触发源分类(P1-1)
///
/// WHY 枚举而非字符串:变更源集合封闭,编译期穷尽检查;
/// 监听器(如 EventBus 适配器/TUI 过滤器)可按类型分派。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateChangeKind {
    /// 新 Quest 注册成功
    QuestRegistered,
    /// Quest 进度更新
    QuestProgressUpdated,
    /// 全局预算变更
    GlobalBudgetChanged,
    /// 模型注册表快照变更
    ModelRegistryChanged,
}

/// 状态变更监听器 — 依赖倒置观察点(P1-1)
///
/// 依赖倒置现仅承载"适配层归 L9"语义(L1 不可向上依赖 L9,§2.2;P1-2 解耦后
/// 同层依赖 event-bus 已无环,见模块头注),监听器实现由上层提供:
/// - 生产适配器:L9 quest-engine `BusStateChangeListener`(转 NexusStateChanged 事件)
/// - 测试:mock 实现断言回调时机与参数
///
/// # 调用时机契约
/// - 回调在状态变更成功且写锁释放**之后**同步调用(不持锁调外部代码)
/// - 变更失败(如 Quest 重复注册)不触发回调
/// - 监听器自身异常不得影响状态变更(R2.1 落地:`notify` 以 catch_unwind
///   隔离回调并记 error 日志;release profile panic="abort" 下无效,见 ADR-271)
pub trait StateChangeListener: Send + Sync {
    /// 状态变更通知
    ///
    /// # 参数
    /// - `kind`: 变更类型
    /// - `state_hash`: 变更后状态的 SHA-256 哈希(64 字符 hex,
    ///   供 NexusStateChanged 事件链式校验载荷直接使用)
    fn on_state_changed(&self, kind: StateChangeKind, state_hash: String);
}

/// 全局运行时状态 — 线程安全的共享状态容器
///
/// 基于 `Arc<RwLock<NexusStateInner>>`,支持并发读、互斥写。
/// 克隆是廉价的(Arc 引用计数),可跨线程传递。
///
/// 可选携带 [`StateChangeListener`](P1-1):`new()` 无监听器(行为与历史版本
/// 完全一致);`with_listener()` 注入监听器后,状态变更成功后回调通知。
#[derive(Clone)]
pub struct NexusState {
    inner: Arc<RwLock<NexusStateInner>>,
    /// 可选状态变更监听器(P1-1);None = 不通知(new() 兼容路径)
    ///
    /// WHY Arc<dyn>:跨 Clone 共享同一监听器;Send + Sync 可跨线程
    listener: Option<Arc<dyn StateChangeListener>>,
}

impl NexusState {
    /// 创建空状态实例(无监听器,行为与历史版本一致)
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(NexusStateInner::default())),
            listener: None,
        }
    }

    /// 创建带状态变更监听器的空状态实例(P1-1)
    ///
    /// 生产装配示例:L9 quest-engine 注入 `BusStateChangeListener`,
    /// 使每次状态变更发布 `NexusStateChanged` 事件(架构红线落地)。
    pub fn with_listener(listener: Arc<dyn StateChangeListener>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(NexusStateInner::default())),
            listener: Some(listener),
        }
    }

    /// 触发监听器回调(内部辅助,P1-1;R2.1 加 panic 隔离)
    ///
    /// 前置条件:调用方已释放写锁(红线:不持锁调外部代码)。
    /// 无监听器时零开销(分支预测友好);state_hash 经缓存惰性重算(R2.3)。
    fn notify(&self, kind: StateChangeKind) {
        if let Some(listener) = &self.listener {
            let hash = self.snapshot_hash();
            // R2.1 panic 隔离:监听器异常不得影响状态变更(注释契约原承诺)。
            // release profile panic="abort" 下 catch_unwind 无效(ADR-271 已载),
            // 此处为 dev/test 及 unwind 构建提供隔离并留观测痕迹。
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_state_changed(kind, hash);
            }));
            if result.is_err() {
                tracing::error!(
                    ?kind,
                    "StateChangeListener 回调 panic 已隔离,状态变更不受影响"
                );
            }
        }
    }

    /// 注册新 Quest — 重复 ID 返回 `QuestAlreadyExists`
    ///
    /// P1-1:注册成功后触发 `StateChangeKind::QuestRegistered` 回调
    /// (写锁释放后,失败路径不触发)。
    pub fn register_quest(&self, quest: Quest) -> Result<(), NexusError> {
        {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            if inner.active_quests.contains_key(&quest.quest_id) {
                return Err(NexusError::QuestAlreadyExists(quest.quest_id));
            }
            inner.active_quests.insert(quest.quest_id.clone(), quest);
            inner.cached_hash = None; // R2.3:失效缓存,notify/读路径惰性重算
        } // 写锁在此释放(红线:不持锁调用外部代码)
        self.notify(StateChangeKind::QuestRegistered);
        Ok(())
    }

    /// 更新 Quest 进度 — Quest 不存在返回 `QuestNotFound`，
    /// 进度计数非法（`completed > total`）返回 `InvalidProgress`
    ///
    /// WHY:Week 2 阶段仅校验 Quest 存在性,实际进度通过 Task.status
    /// 字段反映(由 GQEP 执行器更新)。此方法作为进度通知的校验入口,
    /// 后续阶段可扩展为触发 QuestProgressUpdated 事件。
    ///
    /// WHY 进度参数参与校验（BC-08, 2026-11-16）：此前 `_completed`/`_total`
    /// 带 `_` 前缀不参与任何逻辑（死参数），调用方传入物理上不可能的
    /// `completed > total` 无人看守。现赋予其「进度快照自洽」语义：
    /// 该不变量在方法入口即被拒绝，错误可被调用方精确归因。
    /// 对既有合法调用点零行为变更（既有传值均为 `completed <= total`）。
    pub fn update_quest_progress(
        &self,
        quest_id: &str,
        completed: u32,
        total: u32,
    ) -> Result<(), NexusError> {
        // 进度自洽性:完成数不得超过总数（物理上不可能的组合在入口即拒）。
        // WHY 先于存在性校验:这是调用方载荷的内在矛盾,与 quest 是否存在无关,
        // 先报载荷错误能给出更精确的归因（避免"quest 不存在"掩盖载荷错误）。
        if completed > total {
            return Err(NexusError::InvalidProgress { completed, total });
        }
        {
            let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
            if !inner.active_quests.contains_key(quest_id) {
                return Err(NexusError::QuestNotFound(quest_id.to_string()));
            }
        } // 读锁在此释放
          // P1-1:进度更新成功后通知(快照哈希当前不含进度字段,
          // 但监听器仍需感知变更时机,与既有注释"后续扩展触发事件"对齐;
          // 本方法不改动状态,缓存哈希原样复用)
        self.notify(StateChangeKind::QuestProgressUpdated);
        Ok(())
    }

    /// 计算当前状态的 SHA-256 哈希(64 字符 hex)
    ///
    /// WHY:按 quest_id 排序后哈希,保证确定性(HashMap 迭代序不确定)。
    /// 用于 NexusStateChanged 事件的链式校验(prev_hash → state_hash)。
    /// R2.3:读路径双检缓存——命中返回缓存,未命中重算并回填;写操作已
    /// 将缓存置 None。缓存是纯派生态,返回序列与全量重算逐位一致。
    pub fn snapshot_hash(&self) -> String {
        {
            let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
            if let Some(hash) = &inner.cached_hash {
                return hash.clone();
            }
        } // 读锁释放;未命中走写路径(双检,防并发重复计算)
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(hash) = &inner.cached_hash {
            return hash.clone();
        }
        let hash = compute_snapshot_hash(&inner);
        inner.cached_hash = Some(hash.clone());
        hash
    }

    /// 按 ID 获取 Quest 克隆(不存在返回 None)
    pub fn get_quest(&self, quest_id: &str) -> Option<Quest> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.active_quests.get(quest_id).cloned()
    }

    /// 列出所有活跃 Quest(无序)
    pub fn list_quests(&self) -> Vec<Quest> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.active_quests.values().cloned().collect()
    }

    /// 获取全局预算(已消耗 token 数)
    pub fn global_budget(&self) -> u64 {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.global_budget
    }

    /// 设置全局预算
    ///
    /// P1-1:写入成功后触发 `StateChangeKind::GlobalBudgetChanged` 回调。
    pub fn set_global_budget(&self, budget: u64) {
        {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            inner.global_budget = budget;
            inner.cached_hash = None; // R2.3
        } // 写锁在此释放
        self.notify(StateChangeKind::GlobalBudgetChanged);
    }

    /// 获取模型注册表快照
    pub fn model_registry_snapshot(&self) -> Vec<String> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.model_registry_snapshot.clone()
    }

    /// 设置模型注册表快照
    ///
    /// P1-1:写入成功后触发 `StateChangeKind::ModelRegistryChanged` 回调。
    pub fn set_model_registry_snapshot(&self, models: Vec<String>) {
        {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            inner.model_registry_snapshot = models;
            inner.cached_hash = None; // R2.3
        } // 写锁在此释放
        self.notify(StateChangeKind::ModelRegistryChanged);
    }
}

/// 全量计算快照哈希(按 quest_id 排序保证确定性)
///
/// 从 `snapshot_hash` 抽出的纯函数:读路径缓存未命中时调用一次,结果回填缓存。
fn compute_snapshot_hash(inner: &NexusStateInner) -> String {
    let mut hasher = Sha256::new();

    // 按 quest_id 排序,保证哈希确定性
    let mut entries: Vec<_> = inner.active_quests.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    for (quest_id, quest) in &entries {
        hasher.update(quest_id.as_bytes());
        hasher.update(b"\x00");
        // 序列化 Quest 后哈希;序列化失败(理论不可能)由静默兜底改为
        // warn 观测 + 空串降级(R2.2——此前 unwrap_or_default 无声吞失败)
        let json = match serde_json::to_string(quest) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(quest_id = %quest_id, error = %e, "Quest 序列化失败,快照哈希按空串降级");
                String::new()
            }
        };
        hasher.update(json.as_bytes());
        hasher.update(b"\x00");
    }

    hasher.update(inner.global_budget.to_le_bytes());
    hasher.update(b"\x00");

    for model_id in &inner.model_registry_snapshot {
        hasher.update(model_id.as_bytes());
        hasher.update(b"\x00");
    }

    hex::encode(hasher.finalize())
}

impl Default for NexusState {
    fn default() -> Self {
        Self::new()
    }
}

/// 显式标注 Send + Sync — NexusState 可跨线程共享
///
/// WHY:Arc<RwLock<T>> 自动满足 Send + Sync(当 T: Send + Sync),
/// 此标注作为编译期断言,防止未来修改破坏线程安全契约。
// 静态断言无需运行时开销
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NexusState>();
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use nexus_contracts::domain::{Task, ThinkingMode};
    use nexus_contracts::task::TaskStatus;

    fn make_quest(id: &str) -> Quest {
        Quest {
            quest_id: id.to_string(),
            title: format!("Test Quest {id}"),
            tasks: vec![Task {
                task_id: "t1".into(),
                description: "test task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        }
    }

    #[test]
    fn test_new_state_is_empty() {
        let state = NexusState::new();
        assert!(state.list_quests().is_empty());
        assert_eq!(state.global_budget(), 0);
        assert!(state.model_registry_snapshot().is_empty());
    }

    #[test]
    fn test_register_and_get_quest() {
        let state = NexusState::new();
        let quest = make_quest("q1");
        state.register_quest(quest.clone()).unwrap();

        let retrieved = state.get_quest("q1").unwrap();
        assert_eq!(retrieved.quest_id, "q1");
        assert_eq!(retrieved.title, "Test Quest q1");
    }

    #[test]
    fn test_register_duplicate_quest() {
        let state = NexusState::new();
        let quest = make_quest("q1");
        state.register_quest(quest).unwrap();

        let duplicate = make_quest("q1");
        let result = state.register_quest(duplicate);
        assert!(matches!(result, Err(NexusError::QuestAlreadyExists(_))));
    }

    #[test]
    fn test_update_progress_nonexistent() {
        let state = NexusState::new();
        let result = state.update_quest_progress("nonexistent", 0, 10);
        assert!(matches!(result, Err(NexusError::QuestNotFound(_))));
    }

    #[test]
    fn test_update_progress_existing() {
        let state = NexusState::new();
        state.register_quest(make_quest("q1")).unwrap();
        let result = state.update_quest_progress("q1", 5, 10);
        assert!(result.is_ok());
    }

    /// 负例（BC-08）：进度自洽性 —— 完成数超过总数是物理上不可能的组合，
    /// 必须在入口被拒（此前该参数完全不参与逻辑，无人看守）。
    #[test]
    fn test_update_progress_rejects_completed_exceeds_total() {
        let state = NexusState::new();
        state.register_quest(make_quest("q1")).unwrap();
        let result = state.update_quest_progress("q1", 11, 10);
        assert!(
            matches!(
                result,
                Err(NexusError::InvalidProgress {
                    completed: 11,
                    total: 10
                })
            ),
            "completed > total 必须被拒（实际 {result:?}）"
        );
    }

    /// 负例（BC-08）：载荷矛盾优先于存在性 —— 非法载荷的归因不应被
    /// "quest 不存在"掩盖（否则调用方无法定位是载荷错还是 ID 错）。
    #[test]
    fn test_update_progress_invalid_payload_precedes_existence_check() {
        let state = NexusState::new();
        let result = state.update_quest_progress("nonexistent", 11, 10);
        assert!(
            matches!(result, Err(NexusError::InvalidProgress { .. })),
            "非法载荷须先于存在性校验报错（实际 {result:?}）"
        );
    }

    /// 边界（BC-08）：`completed == total` 是合法的（全部完成）。
    #[test]
    fn test_update_progress_accepts_completed_equals_total() {
        let state = NexusState::new();
        state.register_quest(make_quest("q1")).unwrap();
        let result = state.update_quest_progress("q1", 10, 10);
        assert!(
            result.is_ok(),
            "completed == total 是合法边界（实际 {result:?}）"
        );
    }

    #[test]
    fn test_snapshot_hash_is_64_hex() {
        let state = NexusState::new();
        let hash = state.snapshot_hash();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_snapshot_hash_changes_on_mutation() {
        let state = NexusState::new();
        let hash1 = state.snapshot_hash();

        state.register_quest(make_quest("q1")).unwrap();
        let hash2 = state.snapshot_hash();

        assert_ne!(hash1, hash2, "hash must change after registering quest");
    }

    #[test]
    fn test_snapshot_hash_deterministic() {
        let state1 = NexusState::new();
        state1.register_quest(make_quest("q1")).unwrap();
        state1.register_quest(make_quest("q2")).unwrap();

        let state2 = NexusState::new();
        state2.register_quest(make_quest("q2")).unwrap();
        state2.register_quest(make_quest("q1")).unwrap();

        // 插入顺序不同,但状态相同 → 哈希应一致(排序保证确定性)
        assert_eq!(state1.snapshot_hash(), state2.snapshot_hash());
    }

    /// R2.3:缓存命中路径必须与重算路径逐位一致,且变更后缓存失效重算。
    /// (缓存是纯派生态,此测试钉住「外部可观测语义不变」这一契约。)
    #[test]
    fn test_snapshot_hash_cache_hit_matches_recompute() {
        let state = NexusState::new();
        state.register_quest(make_quest("q1")).unwrap();
        let h1 = state.snapshot_hash(); // 首次:重算并回填
        let h2 = state.snapshot_hash(); // 二次:缓存命中
        assert_eq!(h1, h2, "缓存命中必须与重算结果逐位一致");
        state.set_global_budget(7); // 变更 → 缓存失效
        let h3 = state.snapshot_hash();
        assert_ne!(h1, h3, "变更后缓存必须失效,哈希应变化");
        assert_eq!(h3, state.snapshot_hash(), "重算后再次命中须一致");
    }

    #[test]
    fn test_concurrent_register_quest() {
        let state = NexusState::new();
        let state1 = state.clone();
        let state2 = state.clone();

        let quest_id = "concurrent-quest".to_string();
        let quest_id2 = quest_id.clone();

        let h1 = std::thread::spawn(move || state1.register_quest(make_quest(&quest_id)));
        let h2 = std::thread::spawn(move || state2.register_quest(make_quest(&quest_id2)));

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        // 恰好一个成功,另一个返回 QuestAlreadyExists
        let ok_count = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
        assert_eq!(ok_count, 1, "exactly one thread should succeed");

        if let Err(e) = &r1 {
            assert!(
                matches!(e, NexusError::QuestAlreadyExists(_)),
                "expected QuestAlreadyExists, got {e:?}"
            );
        }
        if let Err(e) = &r2 {
            assert!(
                matches!(e, NexusError::QuestAlreadyExists(_)),
                "expected QuestAlreadyExists, got {e:?}"
            );
        }
    }

    #[test]
    fn test_list_quests() {
        let state = NexusState::new();
        state.register_quest(make_quest("q1")).unwrap();
        state.register_quest(make_quest("q2")).unwrap();

        let mut ids: Vec<_> = state
            .list_quests()
            .into_iter()
            .map(|q| q.quest_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["q1", "q2"]);
    }

    // ============================================================
    // P1-1: StateChangeListener 观察者接线测试
    // ============================================================

    use std::sync::Mutex;

    /// Mock 监听器 — 记录回调的 (kind, hash) 序列,验证回调时机与参数
    #[derive(Default)]
    struct MockListener {
        calls: Mutex<Vec<(StateChangeKind, String)>>,
    }

    impl StateChangeListener for MockListener {
        fn on_state_changed(&self, kind: StateChangeKind, state_hash: String) {
            self.calls.lock().unwrap().push((kind, state_hash));
        }
    }

    /// R2.1 负例:每次回调必 panic 的监听器 —— 验证 panic 被隔离,
    /// 状态变更与其后的所有操作不受影响(注释契约原承诺,此前无实现)。
    struct PanicListener;

    impl StateChangeListener for PanicListener {
        fn on_state_changed(&self, _kind: StateChangeKind, _state_hash: String) {
            panic!("deliberate listener panic (R2.1 isolation test)");
        }
    }

    #[test]
    fn test_listener_notified_on_quest_registered() {
        let listener = std::sync::Arc::new(MockListener::default());
        let state = NexusState::with_listener(listener.clone());
        state.register_quest(make_quest("q1")).unwrap();

        let calls = listener.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "注册成功应触发一次回调");
        assert_eq!(calls[0].0, StateChangeKind::QuestRegistered);
        assert_eq!(calls[0].1.len(), 64, "state_hash 应为 64 字符 hex");
        // 回调携带的哈希应与当前快照一致(变更后计算)
        assert_eq!(calls[0].1, state.snapshot_hash());
    }

    #[test]
    fn test_listener_not_notified_on_failed_register() {
        let listener = std::sync::Arc::new(MockListener::default());
        let state = NexusState::with_listener(listener.clone());
        state.register_quest(make_quest("q1")).unwrap();
        // 重复注册失败,不应触发回调
        let _ = state.register_quest(make_quest("q1"));

        let calls = listener.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "失败路径不应触发回调");
    }

    #[test]
    fn test_listener_notified_on_budget_and_registry_change() {
        let listener = std::sync::Arc::new(MockListener::default());
        let state = NexusState::with_listener(listener.clone());
        state.set_global_budget(42);
        state.set_model_registry_snapshot(vec!["m1".into()]);

        let calls = listener.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, StateChangeKind::GlobalBudgetChanged);
        assert_eq!(calls[1].0, StateChangeKind::ModelRegistryChanged);
    }

    #[test]
    fn test_listener_notified_on_progress_update() {
        let listener = std::sync::Arc::new(MockListener::default());
        let state = NexusState::with_listener(listener.clone());
        state.register_quest(make_quest("q1")).unwrap();
        state.update_quest_progress("q1", 1, 2).unwrap();

        let calls = listener.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].0, StateChangeKind::QuestProgressUpdated);
    }

    #[test]
    fn test_no_listener_keeps_legacy_behavior() {
        // new() 无监听器:行为与历史版本完全一致,不 panic
        let state = NexusState::new();
        state.register_quest(make_quest("q1")).unwrap();
        state.set_global_budget(1);
        assert_eq!(state.list_quests().len(), 1);
    }

    /// R2.1:监听器 panic 被 catch_unwind 隔离 —— register/set 均返回成功,
    /// 状态保持可读可写,哈希缓存路径不受影响。
    /// 注:catch_unwind 仍会触发 panic hook(打印到测试输出),测试期临时置静默。
    #[test]
    fn test_listener_panic_is_isolated() {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(|| {
            let listener = std::sync::Arc::new(PanicListener);
            let state = NexusState::with_listener(listener);

            // 两次触发回调的写操作:panic 均被隔离,语义照常成功
            state.register_quest(make_quest("q1")).unwrap();
            state.set_global_budget(9);
            state.set_model_registry_snapshot(vec!["m".into()]);
            state.update_quest_progress("q1", 1, 2).unwrap();

            // 状态本体不受监听器异常影响
            assert_eq!(state.list_quests().len(), 1);
            assert_eq!(state.global_budget(), 9);
            assert_eq!(state.snapshot_hash().len(), 64);
            // 无监听器的新实例行为不变
            let plain = NexusState::new();
            plain.register_quest(make_quest("p1")).unwrap();
            assert_eq!(plain.snapshot_hash().len(), 64);
        });
        std::panic::set_hook(default_hook);
        assert!(result.is_ok(), "监听器 panic 不得传播到调用方");
    }
}
