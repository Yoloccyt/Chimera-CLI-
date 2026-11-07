//! R4-1(ADR-263)分配预算棘轮:派发/批量热路径的堆分配次数上断言。
//!
//! 棘轮语义:**只降不升**——登记值 = 当前实测上界;未来改动使分配数增加
//! 即红(性能回归的 L2 结构哨兵,补 criterion 墙钟噪声盲区)。
//! 独立测试二进制:全局分配器替换只影响本文件;单 #[test] 串行执行防并行污染。
//! CountingAlloc 模式复用 nexus-contracts/benches/util_micro.rs 先例
//! (R1 的 tests/policy_zero_alloc.rs 同款)。

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

use gqep_executor::tool_runtime::{BatchLimits, DispatchRequest, ToolRuntime};
use gqep_executor::ToolExecutor;

/// 回显执行器 — 载荷 String 分配是执行器自身语义(非派发开销),计入预算
struct EchoExecutor;

#[async_trait::async_trait]
impl ToolExecutor for EchoExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        Ok(args_json.to_string())
    }
}

fn allow_runtime() -> ToolRuntime {
    use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};
    let policy = Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new("bench.echo", "*"),
        action: PolicyAction::Allow,
    }));
    ToolRuntime::new(Arc::new(EchoExecutor), policy).expect("生产装配链应构造成功")
}

fn request() -> DispatchRequest {
    DispatchRequest {
        tool_name: "bench.echo".to_string(),
        args_json: "x".repeat(112),
    }
}

/// 分配预算上界(实测登记,R4-1 棘轮:只降不升)
///
/// 登记方法:先以宽松上界跑出实际值,回填为本常量并注明实测日期与代码态
/// (3dbcf608..52f5c89 后 = R1 判决体零分配 + R2 惰性装载)。
mod budget {
    /// 单次派发(allow 路径,含叶子载荷 String)分配数上界
    /// 实测登记:2026-11-08 @ R2 后(3dbcf608..52f5c89),实测值 7
    pub const SINGLE_DISPATCH_ALLOW: usize = 7;
    /// execute_batch(N=8, bound=4, allow)全程分配数上界
    /// 实测登记:2026-11-08 @ R2 后,实测值 68(≈8.5/请求:8×装箱+载荷+批量设施)
    pub const BATCH_8_ALLOW: usize = 68;
}

#[tokio::test]
async fn dispatch_hot_paths_within_alloc_budget() {
    // WHY 单测试串行:两个 #[tokio::test] 会并行运行并共享全局分配计数器,
    // 测量窗口互相污染(实测 7→8 漂移);串行 + 热身后测量窗口干净。
    let runtime = allow_runtime();

    // —— 单次派发(allow) ——
    let req = request();
    let _ = runtime.dispatch(&req).await; // 热身:惰性初始化不计入
    let before = ALLOCS.load(Relaxed);
    let _ = runtime.dispatch(&req).await;
    let used = (ALLOCS.load(Relaxed) - before) as usize;
    println!("single_dispatch_allow allocs = {used}");
    assert!(
        used <= budget::SINGLE_DISPATCH_ALLOW,
        "单发分配 {used} 超预算 {}:R4-1 棘轮只降不升,若为有意优化请下调登记值",
        budget::SINGLE_DISPATCH_ALLOW
    );

    // —— 批量(N=8,BatchLimits::default,allow) ——
    let requests: Vec<DispatchRequest> = (0..8).map(|_| request()).collect();
    let _ = runtime
        .execute_batch(&requests, BatchLimits::default())
        .await; // 热身
    let before = ALLOCS.load(Relaxed);
    let _ = runtime
        .execute_batch(&requests, BatchLimits::default())
        .await;
    let used = (ALLOCS.load(Relaxed) - before) as usize;
    println!("batch_8_allow allocs = {used}");
    assert!(
        used <= budget::BATCH_8_ALLOW,
        "批量分配 {used} 超预算 {}:R4-1 棘轮只降不升",
        budget::BATCH_8_ALLOW
    );
}
