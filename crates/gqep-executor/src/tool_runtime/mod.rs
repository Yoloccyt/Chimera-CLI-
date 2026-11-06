//! 工具运行时执行面（T2-3 / T2-4 / T1-2）— 强制四环派发链
//!
//! `policy → sandbox → route → transport`，任何前一环拒绝都不会触达后续环。
//! 组装入口唯一：`DispatcherBuilder::build()`；生产接线唯一：`ToolRuntime::new()`。

pub mod batch;
pub mod chain;
pub mod dispatch;
pub mod inproc;
pub mod runtime;
pub mod sandbox_gate;

pub use batch::{dispatch_batch, BatchLimits, BatchOutcome, BATCH_DEADLINE_PREFIX};
pub use chain::{DispatcherBuilder, SandboxedDispatcher};
pub use dispatch::{
    DispatchError, DispatchFuture, DispatchOutcome, DispatchRequest, ToolDispatcher,
};
pub use inproc::InProcDispatcher;
pub use runtime::ToolRuntime;
pub use sandbox_gate::{AllowAllSandboxGate, SandboxGate, SeccoreSandboxGate};
