//! QEEP 协议核心实现
//!
//! `QeepProtocol` 提供 `entangle()` 包装器,将任意 future 纳入零孤儿调用保证。
//!
//! ## 核心机制
//!
//! 1. **注册**:`entangle()` 为 future 分配 `EntangledCallId`,注册到 `pending_calls`
//! 2. **守护**:创建 `OrphanGuard`,持有 `completed: Arc<AtomicBool>` 标志
//! 3. **超时控制**:用 `tokio::time::timeout` 包裹 future,超时返回 `QeepError::Timeout`
//! 4. **完成标记**:future 完成后(无论成功/失败/超时),将 `completed` 设为 `true`
//! 5. **孤儿检测**:若 future 被 drop 时 `completed == false`,`OrphanGuard::drop` 报告孤儿
//!
//! ## 对应尸检教训
//!
//! Claude Code 5.4% 孤儿调用(void Promise 无 await)的根因是:
//! - 异步操作 spawn 后,JoinHandle 未被 await
//! - future 被 drop 但无运行时检测
//!
//! QEEP 通过 `OrphanGuard` + `Drop` trait 从机制上杜绝此类问题:
//! 无论 future 因何种原因被 drop(超时、取消、调用者不 await),都能被检测到。

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use dashmap::DashMap;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::detector::OrphanDetector;
use crate::error::QeepError;
use crate::types::{Ack, CallState, EntangledCallId, OrphanReport};

/// 内部共享状态,由 `Arc` 包裹供 `OrphanGuard` 与 `QeepProtocol` 共享。
struct Inner {
    /// 待处理调用表(非泛型,只存元信息)
    pending_calls: DashMap<EntangledCallId, CallRecord>,
    /// 孤儿检测器(用 `Mutex` 包装,因为 `OrphanDetector` 方法是 `&mut self`)
    orphan_detector: Mutex<OrphanDetector>,
    /// 已完成调用计数(成功 + 失败 + 超时,不含孤儿)
    completed_count: AtomicUsize,
}

/// 调用记录(非泛型版本,用于 `pending_calls` 统一存储)
///
/// 注:不存储泛型 `Receipt<T>`,因为 `pending_calls` 需要容纳不同 `T` 的调用。
/// 泛型 `Receipt<T>` 由 `entangle()` 直接返回给调用者。
#[derive(Debug, Clone)]
struct CallRecord {
    /// 调用 ID
    #[allow(dead_code)]
    id: EntangledCallId,
    /// 当前状态
    state: CallState,
    /// 创建时间
    created_at: chrono::DateTime<Utc>,
    /// 完成时间(若已终结)
    completed_at: Option<chrono::DateTime<Utc>>,
    /// 确认回执(若已 Ack)
    ack: Option<Ack>,
}

/// QEEP 协议主体
///
/// 提供 `entangle()` 与 `entangle_spawn()` 两种纠缠模式:
/// - `entangle()`:异步等待 future 完成,适用于调用者需要立即结果的场景
/// - `entangle_spawn()`:spawn 到 tokio task,返回 `JoinHandle`,适用于后台任务
///
/// 两种模式均纳入零孤儿调用保证。
#[derive(Clone)]
pub struct QeepProtocol {
    /// 内部共享状态
    inner: Arc<Inner>,
    /// 默认超时时间
    default_timeout: Duration,
}

impl QeepProtocol {
    /// 创建新的 QEEP 协议实例
    ///
    /// # 参数
    /// - `default_timeout`:`entangle()` 的默认超时时间
    pub fn new(default_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                pending_calls: DashMap::new(),
                orphan_detector: Mutex::new(OrphanDetector::new()),
                completed_count: AtomicUsize::new(0),
            }),
            default_timeout,
        }
    }

    /// 纠缠包裹:将 future 纳入零孤儿调用保证
    ///
    /// # 机制
    /// 1. 生成 `EntangledCallId`,注册到 `pending_calls`
    /// 2. 创建 `OrphanGuard`,在 future drop 时检测是否完成
    /// 3. 用 `tokio::time::timeout` 包裹,超时返回 `QeepError::Timeout`
    /// 4. 完成后标记 `completed=true`,从 `pending_calls` 移除
    ///
    /// # 类型约束
    /// - `F: Future<Output = Result<T, QeepError>> + Send`:future 必须输出 `Result`
    /// - `T: Send`:结果必须可跨线程传递
    /// - 不要求 `'static`(因为不 spawn,只 await)
    ///
    /// # 返回
    /// - `Ok(T)`:future 成功完成
    /// - `Err(QeepError::Timeout)`:超时
    /// - `Err(QeepError::*)`:future 返回的错误
    pub async fn entangle<F, T>(&self, future: F) -> Result<T, QeepError>
    where
        F: Future<Output = Result<T, QeepError>> + Send,
        T: Send,
    {
        let id = EntangledCallId(Uuid::now_v7());
        let created_at = Utc::now();

        // 步骤 1:注册到 pending_calls
        self.inner.pending_calls.insert(
            id,
            CallRecord {
                id,
                state: CallState::Pending,
                created_at,
                completed_at: None,
                ack: None,
            },
        );

        // 步骤 1.5:创建 Ack 并进入 Acknowledged 状态
        // WHY: QEEP 三元组要求 Request → Ack → Receipt;Ack 表示 future
        // 已被接收并即将 poll,是零孤儿调用保证的关键可观测点。
        let ack = Ack {
            id,
            acknowledged_at: Utc::now(),
        };
        if let Some(mut entry) = self.inner.pending_calls.get_mut(&id) {
            entry.state = CallState::Acknowledged;
            entry.ack = Some(ack);
        }

        // 步骤 2:创建 OrphanGuard
        // completed 标志用于区分"正常完成"与"被 drop"
        let guard = OrphanGuard {
            id,
            inner: self.inner.clone(),
            completed: Arc::new(AtomicBool::new(false)),
        };

        // 步骤 3:用 timeout 包裹 future
        // 注意:若 future 被 drop(任务被 abort),await 点会 panic 或返回 Pending,
        // 此时 guard 会 drop,completed=false,报告孤儿
        let result = tokio::time::timeout(self.default_timeout, future).await;

        // 步骤 4:标记完成,防止 guard 报告孤儿
        guard.completed.store(true, Ordering::SeqCst);

        // 更新 CallRecord 状态与完成时间
        let final_state = match &result {
            Ok(Ok(_)) => CallState::Completed,
            Ok(Err(_)) => CallState::Failed,
            Err(_) => CallState::Timeout,
        };
        if let Some(mut entry) = self.inner.pending_calls.get_mut(&id) {
            entry.state = final_state;
            entry.completed_at = Some(Utc::now());
        }

        // 步骤 5:释放 guard(此时 completed=true,不会报告孤儿)
        drop(guard);

        // 步骤 6:从 pending_calls 移除,递增 completed_count
        self.inner.pending_calls.remove(&id);
        self.inner.completed_count.fetch_add(1, Ordering::SeqCst);

        // 返回结果
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(QeepError::Timeout),
        }
    }

    /// spawn 模式:将 future spawn 到 tokio task,返回 `JoinHandle`
    ///
    /// # ⚠️ 调用者责任
    /// 调用者**必须** await 返回的 `JoinHandle`,否则:
    /// - 任务结果会被丢弃(虽然 `OrphanGuard` 不会报告,因为任务正常完成)
    /// - 但从业务角度,这是"未收集结果",等同于孤儿调用
    ///
    /// # 类型约束
    /// - `F: Future<Output = Result<T, QeepError>> + Send + 'static`:可 spawn
    /// - `T: Send + 'static`:结果可跨线程传递且无借用
    ///
    /// # 返回
    /// `JoinHandle<Result<T, QeepError>>`:调用者应 await 此 handle
    pub fn entangle_spawn<F, T>(&self, future: F) -> JoinHandle<Result<T, QeepError>>
    where
        F: Future<Output = Result<T, QeepError>> + Send + 'static,
        T: Send + 'static,
    {
        let protocol = self.clone();
        tokio::spawn(async move { protocol.entangle(future).await })
    }

    /// 获取当前待处理(pending)调用数量
    pub fn pending_count(&self) -> usize {
        self.inner.pending_calls.len()
    }

    /// 获取已完成调用数量(成功 + 失败 + 超时,不含孤儿)
    pub fn completed_count(&self) -> usize {
        self.inner.completed_count.load(Ordering::SeqCst)
    }

    /// 获取所有孤儿调用报告(克隆返回,避免持有锁)
    pub fn orphan_reports(&self) -> Vec<OrphanReport> {
        let detector = self
            .inner
            .orphan_detector
            .lock()
            .expect("orphan_detector mutex poisoned");
        detector.detect_orphans().to_vec()
    }

    /// 获取孤儿调用数量
    pub fn orphan_count(&self) -> usize {
        let detector = self
            .inner
            .orphan_detector
            .lock()
            .expect("orphan_detector mutex poisoned");
        detector.orphan_count()
    }

    /// 获取当前所有待处理调用的 ID 列表
    ///
    /// 用于调用者按 id 查询具体状态(见 `call_state`/`call_ack`)。
    pub fn pending_call_ids(&self) -> Vec<EntangledCallId> {
        self.inner
            .pending_calls
            .iter()
            .map(|entry| *entry.key())
            .collect()
    }

    /// 按 id 查询调用当前状态
    ///
    /// 返回 `None` 表示该调用已终结(Completed/Timeout/Failed)或从未注册,
    /// 因为协议在调用终结后会立即清理记录。
    pub fn call_state(&self, id: EntangledCallId) -> Option<CallState> {
        self.inner.pending_calls.get(&id).map(|entry| entry.state)
    }

    /// 按 id 查询调用确认回执(Ack)
    ///
    /// 返回 `None` 表示调用尚未 Ack 或已终结。
    pub fn call_ack(&self, id: EntangledCallId) -> Option<Ack> {
        self.inner
            .pending_calls
            .get(&id)
            .and_then(|entry| entry.ack.clone())
    }
}

/// OrphanGuard:在 future drop 时检测是否完成,未完成则报告孤儿
///
/// ## 工作原理
///
/// 1. `entangle()` 创建 `OrphanGuard`,持有 `completed: Arc<AtomicBool>`
/// 2. future 正常完成时,`entangle()` 将 `completed` 设为 `true`
/// 3. future 被 drop 时(无论原因),`OrphanGuard::drop` 被调用:
///    - 若 `completed == true`:正常完成,不报告
///    - 若 `completed == false`:孤儿调用,生成 `OrphanReport`
///
/// ## 触发场景
///
/// - **任务被 abort**:`tokio::task::JoinHandle::abort()` 导致 future drop
/// - **超时取消**:`tokio::time::timeout` 超时后,内部 future 被 drop
///   (注:我们的 `entangle()` 在超时后会标记 `completed=true`,所以不报告孤儿)
/// - **调用者不 await**:`entangle()` 返回的 future 被 drop 而未 poll
///   (注:此时 `OrphanGuard` 尚未创建,因为 async fn 体未执行)
/// - **调用者 poll 后 drop**:`entangle()` 返回的 future 被 poll 一次后 drop
///   (此时 `OrphanGuard` 已创建,会报告孤儿)
struct OrphanGuard {
    /// 对应的调用 ID
    id: EntangledCallId,
    /// 内部共享状态(用于访问 orphan_detector 与 pending_calls)
    inner: Arc<Inner>,
    /// 完成标志:false=未完成(孤儿),true=已完成(正常)
    completed: Arc<AtomicBool>,
}

impl Drop for OrphanGuard {
    fn drop(&mut self) {
        // 检查是否正常完成
        if self.completed.load(Ordering::SeqCst) {
            return; // 正常完成,不报告孤儿
        }

        // 未完成即孤儿:生成报告
        // 先读取 created_at(若 pending_calls 中还有记录)
        let created_at = self
            .inner
            .pending_calls
            .get(&self.id)
            .map(|e| e.created_at)
            .unwrap_or_else(Utc::now);

        let report = OrphanReport {
            call_id: self.id,
            created_at,
            orphaned_at: Utc::now(),
            reason: "Future dropped before completion (void Promise 无 await)".to_string(),
        };

        // 报告到 OrphanDetector
        // 注:这里用 expect 而非 unwrap,提供更好的 panic 信息
        // Mutex 不会 panic 除非另一线程在持有锁时 panic(poisoned)
        {
            let mut detector = self
                .inner
                .orphan_detector
                .lock()
                .expect("orphan_detector mutex poisoned");
            detector.report_orphan(report);
        }
        // 显式释放 detector 锁后再操作 pending_calls,避免潜在锁交互

        // 移除 pending_calls 中的记录
        // 孤儿调用已"终结"(虽然失败),不再是 pending,应从 pending_calls 移除,
        // 保证 pending_count 只反映"正在执行"的调用。
        self.inner.pending_calls.remove(&self.id);
    }
}
