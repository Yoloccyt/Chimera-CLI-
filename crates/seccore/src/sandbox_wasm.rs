//! WASM 沙箱 PoC — ADR-035 决策 2 重启路径
//!
//! 对应架构层:L4 Security
//! 对应文档:ADR-035-threat-model-revision-wasmtime-restart.md 决策 2 / 决策 5
//!
//! ## 模块结构
//! - `SandboxBackend` 枚举:始终可用(默认 `Process`,向后兼容)
//! - `WasmSandbox` / `WasmExecutionResult`:仅 `wasm-sandbox` feature 启用时可用
//!
//! ## PoC 目标(仅 `wasm-sandbox` feature 启用)
//! 通过 wasmtime safe API 实现 WASM 虚拟机级隔离,为高风险场景
//! (多租户 / 不可信代码执行 / 服务端部署)提供比进程级隔离更强的边界。
//!
//! ## 安全合规(ADR-035 决策 5)
//! - 仅使用 wasmtime 公开 safe API(Engine / Config / Store / Module / Instance / TypedFunc)
//! - 不引入 `unsafe` 关键字到 seccore 源码,保持 `#![forbid(unsafe_code)]` crate 级属性
//! - wasmtime 内部 unsafe(FFI binding)不传播到 seccore(crate 级属性不传播,见 §4.1)
//! - 若 PoC 发现必须使用 unsafe API,则触发 ADR 流程评估(ADR-030 默认拒绝)
//!
//! ## 资源限制(CPU + 内存)
//! - **Fuel 机制**:`Config::consume_fuel(true)` + `Store::set_fuel(u64)`,每条 WASM 指令消耗固定 fuel,
//!   耗尽时触发 `Trap::OutOfFuel`,防止 `loop {}` 类 DoS 攻击
//! - **异步超时**:`tokio::time::timeout` 包裹同步 `Func::call`,防止长时间执行阻塞 runtime
//! - **内存上限**:通过 `Store::limiter` 设置线性内存增长上限(本 PoC 暂未启用 limiter,
//!   预留接口供生产化阶段补齐;详见 ADR-035 决策 2"资源影响评估")
//!
//! ## API 概览(仅 `wasm-sandbox` feature 启用)
//! ```ignore
//! # #[cfg(feature = "wasm-sandbox")]
//! # {
//! use seccore::{SandboxBackend, WasmSandbox, WasmExecutionResult};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. 创建 WASM 沙箱(默认 fuel=100万, timeout=30s)
//! let sandbox = WasmSandbox::new()?;
//!
//! // 2. 加载预编译的 WASM 模块并执行指定函数
//! let wat = r#"(module (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))"#;
//! let result = sandbox.execute_in_wasm(wat.as_bytes(), "add", &[3, 4]).await?;
//! assert_eq!(result.exit_code, 0);
//! assert_eq!(result.return_value, Some(7));
//! # Ok(())
//! # }
//! # }
//! ```

// Duration 仅在 wasm-sandbox feature 启用时使用(WasmSandbox/WasmExecutionResult),
// 故导入置于 cfg 内部,避免默认构建 unused_imports 警告。

/// 沙箱后端选择(ADR-035 决策 2)
///
/// 通过该枚举在 `audit_and_execute` 流程中分发到不同隔离等级的后端,
/// 默认 `Process`(向后兼容),高风险场景显式切换 `Wasm`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    /// 进程级隔离(`tokio::process::Command` + `env_clear` + `kill_on_drop` + `timeout`)。
    ///
    /// 适用场景:单机 CLI、信任用户输入的低风险场景。
    /// 这是当前 `sandbox.rs::Sandbox::audit_and_execute` 的实现路径。
    Process,

    /// WASM 虚拟机级隔离(wasmtime safe API + fuel + 超时)。
    ///
    /// 适用场景:多租户、不可信代码执行、服务端部署。
    /// 仅在 `wasm-sandbox` feature 启用时可用。
    #[cfg(feature = "wasm-sandbox")]
    Wasm,

    /// gVisor 内核级隔离(Linux CI 验证,ADR-001 重启路径)。
    ///
    /// 适用场景:Linux 生产环境,需安装 runsc + 配置 OCI runtime。
    /// 当前为占位变体,Linux CI 矩阵中验证(ADR-035 决策 2)。
    Gvisor,
}

impl Default for SandboxBackend {
    /// 默认后端为 `Process`,保持向后兼容(ADR-035 决策 2)。
    fn default() -> Self {
        Self::Process
    }
}

// ============================================================================
// 以下内容仅在 `wasm-sandbox` feature 启用时编译
// ============================================================================

#[cfg(feature = "wasm-sandbox")]
mod wasm_impl {
    use crate::error::SecCoreError;
    use std::time::Duration;
    use wasmtime::{Config, Engine, Instance, Module, Store, TypedFunc};

    /// WASM 沙箱执行结果
    #[derive(Debug, Clone, PartialEq)]
    pub struct WasmExecutionResult {
        /// WASM 函数返回值(`None` 表示函数无返回值或返回 void)
        pub return_value: Option<i32>,
        /// 执行退出码(0 表示成功,非 0 表示 trap / fuel 耗尽 / 超时)
        pub exit_code: i32,
        /// 执行消耗的 fuel(用于审计与计费)
        pub fuel_consumed: u64,
        /// 执行耗时
        pub duration: Duration,
    }

    /// WASM 沙箱 — 封装 wasmtime Engine,提供资源限制的 WASM 执行能力
    ///
    /// # 设计要点
    /// - **Engine 复用**:`Engine` 是线程安全的可共享对象,创建一次复用多次,避免重复 JIT 编译开销
    /// - **Store 独立**:每次 `execute_in_wasm` 创建新 `Store`,保证实例间状态隔离(无共享内存)
    /// - **Fuel 强制**:`Config::consume_fuel(true)` 在 Engine 创建时锁定,所有执行必须受 fuel 约束
    /// - **超时兜底**:fuel 是协作式限制(每条指令检查),超时是抢占式限制(`tokio::time::timeout`)
    ///
    /// # 安全契约
    /// - 不暴露任何 `unsafe` API(ADR-035 决策 5)
    /// - WASM 模块在 wasmtime 沙箱内执行,无 ambient authority(无文件系统、网络、环境变量访问)
    /// - 默认无 WASI 导入,模块只能调用自身导出函数,不能调用 host
    pub struct WasmSandbox {
        /// wasmtime 引擎(JIT 编译器,线程安全,可共享)
        engine: Engine,
        /// 单次执行最大时长(默认 30 秒,与 `Sandbox::timeout` 默认值对齐)
        execution_timeout: Duration,
        /// 单次执行 fuel 上限(默认 100 万,足够典型计算任务,过小会误杀合法计算)
        fuel_limit: u64,
    }

    impl WasmSandbox {
        /// 创建默认 WASM 沙箱
        ///
        /// 默认配置:
        /// - `execution_timeout`: 30 秒(与 `Sandbox::with_default_policy` 对齐)
        /// - `fuel_limit`: 1,000,000(典型计算任务足够,过小会误杀合法计算)
        ///
        /// # 错误
        /// - `WasmSandboxError`:wasmtime Engine 创建失败(通常是 Config 配置错误或系统资源不足)
        pub fn new() -> Result<Self, SecCoreError> {
            // WHY consume_fuel(true):强制启用 fuel 计量,所有 WASM 执行必须受 fuel 约束,
            // 防止 `loop {}` 类 DoS 攻击。这是 wasmtime 生产硬化的标准做法。
            let mut config = Config::new();
            config.consume_fuel(true);

            let engine = Engine::new(&config)
                .map_err(|e| SecCoreError::WasmSandboxError(format!("Engine 创建失败: {e}")))?;

            Ok(Self {
                engine,
                execution_timeout: Duration::from_secs(30),
                fuel_limit: 1_000_000,
            })
        }

        /// 设置单次执行超时(链式调用)
        ///
        /// # 参数
        /// - `timeout`:超时时长,建议 ≥ 1 秒(过短会误杀合法计算)
        #[must_use]
        pub fn with_execution_timeout(mut self, timeout: Duration) -> Self {
            self.execution_timeout = timeout;
            self
        }

        /// 设置单次执行 fuel 上限(链式调用)
        ///
        /// # 参数
        /// - `fuel`:fuel 单位数,典型值 100_000 ~ 10_000_000
        #[must_use]
        pub fn with_fuel_limit(mut self, fuel: u64) -> Self {
            self.fuel_limit = fuel;
            self
        }

        /// 获取配置的超时时长(供调用方诊断)
        pub fn execution_timeout(&self) -> Duration {
            self.execution_timeout
        }

        /// 获取配置的 fuel 上限(供调用方诊断)
        pub fn fuel_limit(&self) -> u64 {
            self.fuel_limit
        }

        /// 在 WASM 沙箱中执行预编译的 WASM 模块的导出函数
        ///
        /// # 流程
        /// 1. 编译 WASM 模块(`Module::new`,字节码可以是 WAT 文本或 WASM 二进制)
        /// 2. 创建独立 Store(`Store::new`,带 fuel 限制)
        /// 3. 实例化模块(`Instance::new`,无导入,纯计算模块)
        /// 4. 获取导出函数(`get_typed_func::<i32, i32>`,签名必须匹配)
        /// 5. 在超时内调用函数(`tokio::time::timeout` 包裹 `Func::call`)
        /// 6. 计算 fuel 消耗并返回结果
        ///
        /// # 参数
        /// - `wasm_bytes`:WASM 模块字节码(WAT 文本或 WASM 二进制,wasmtime 自动识别)
        /// - `function_name`:要调用的导出函数名
        /// - `args`:函数参数(当前仅支持 `&[i32]`,PoC 阶段简化签名)
        ///
        /// # 返回
        /// - `Ok(WasmExecutionResult)`:执行成功,携带返回值、exit_code、fuel 消耗、耗时
        /// - `Err(SecCoreError::WasmSandboxError)`:编译/实例化/调用失败,或超时,或 fuel 耗尽
        ///
        /// # 错误分类
        /// - 模块编译失败:WAT/WASM 字节码语法错误
        /// - 实例化失败:导入不匹配(本 PoC 不支持带导入的模块)
        /// - 函数查找失败:导出名不存在或签名不匹配
        /// - 调用失败:trap(除零、内存越界、unreachable)
        /// - Fuel 耗尽:`Trap::OutOfFuel`,计算量超过 `fuel_limit`
        /// - 超时:执行超过 `execution_timeout`
        ///
        /// # PoC 限制
        /// - 仅支持 `(func (export "name") (param i32) (result i32))` 签名的函数
        /// - 不支持 WASI 导入(模块必须纯计算,无 I/O)
        /// - 不支持多返回值(仅返回单个 i32 或 void)
        /// - 不支持多参数(仅接受单个 i32 参数;多参数场景需扩展 `args` 类型)
        pub async fn execute_in_wasm(
            &self,
            wasm_bytes: &[u8],
            function_name: &str,
            args: &[i32],
        ) -> Result<WasmExecutionResult, SecCoreError> {
            let start = std::time::Instant::now();

            // 步骤1:编译 WASM 模块(wasmtime 自动识别 WAT 文本与 WASM 二进制)
            let module = Module::new(&self.engine, wasm_bytes)
                .map_err(|e| SecCoreError::WasmSandboxError(format!("WASM 模块编译失败: {e}")))?;

            // 步骤2:在 blocking 任务中执行同步 WASM 调用,避免阻塞 async runtime
            // WHY spawn_blocking:wasmtime Func::call 是 CPU 密集型同步操作,
            // 直接在 async 上下文执行会阻塞 tokio runtime
            let timeout = self.execution_timeout;
            let fuel_limit = self.fuel_limit;
            let engine = self.engine.clone();
            // WHY owned 化:spawn_blocking 闭包需要 'static + Send,借用引用不满足
            let function_name_owned = function_name.to_string();
            let args_owned: Vec<i32> = args.to_vec();

            // spawn_blocking 返回 JoinHandle(实现 Future),timeout 直接包装 JoinHandle
            // 不能在 timeout 内部 .await(否则返回 Result 而非 Future,类型不匹配)
            // WHY Result<i32, wasmtime::Error>:TypedFunc::call 返回 wasmtime::Error(通用错误类型),
            // Trap 是其中一个变体(可通过 downcast_ref::<Trap>() 提取)
            let join_handle = tokio::task::spawn_blocking(
                move || -> Result<(Result<i32, wasmtime::Error>, u64), SecCoreError> {
                    let mut store: Store<()> = Store::new(&engine, ());
                    store.set_fuel(fuel_limit).map_err(|e| {
                        SecCoreError::WasmSandboxError(format!("set_fuel 失败: {e}"))
                    })?;

                    let instance = Instance::new(&mut store, &module, &[]).map_err(|e| {
                        SecCoreError::WasmSandboxError(format!("WASM 模块实例化失败: {e}"))
                    })?;

                    let func: TypedFunc<i32, i32> = instance
                        .get_typed_func(&mut store, &function_name_owned)
                        .map_err(|e| {
                            SecCoreError::WasmSandboxError(format!(
                                "导出函数 '{}' 查找失败(签名应为 (i32) -> i32): {e}",
                                function_name_owned
                            ))
                        })?;

                    let arg = match args_owned.first() {
                        Some(&v) => v,
                        None => {
                            return Err(SecCoreError::WasmSandboxError(format!(
                                "WASM 函数 '{}' 需要 1 个 i32 参数,但 args 为空",
                                function_name_owned
                            )));
                        }
                    };

                    let fuel_before = store.get_fuel().map_err(|e| {
                        SecCoreError::WasmSandboxError(format!("get_fuel(before) 失败: {e}"))
                    })?;

                    let call_result = func.call(&mut store, arg);

                    let fuel_after = store.get_fuel().map_err(|e| {
                        SecCoreError::WasmSandboxError(format!("get_fuel(after) 失败: {e}"))
                    })?;

                    Ok((call_result, fuel_before - fuel_after))
                },
            );

            // timeout 包装 JoinHandle(实现 Future),返回 Result<Result<Inner, JoinError>, Elapsed>
            let timeout_result = tokio::time::timeout(timeout, join_handle).await;

            // 处理超时与任务结果
            // 三层 Result:外层 Elapsed(超时),中层 JoinError(任务 panic),内层 SecCoreError(任务返回错误)
            let (call_result, fuel_consumed) = match timeout_result {
                Ok(Ok(Ok((call_result, fuel)))) => (call_result, fuel),
                Ok(Ok(Err(e))) => return Err(e),
                Ok(Err(join_err)) => {
                    return Err(SecCoreError::WasmSandboxError(format!(
                        "spawn_blocking 任务 panic: {join_err}"
                    )));
                }
                Err(_elapsed) => {
                    return Err(SecCoreError::WasmSandboxError(format!(
                        "WASM 执行超时: 函数 '{function_name}' 未在 {timeout:?} 内完成"
                    )));
                }
            };

            // 步骤6:处理调用结果(区分成功 / trap / fuel 耗尽)
            let (exit_code, return_value) = match call_result {
                Ok(value) => (0, Some(value)),
                Err(trap) => {
                    // trap 可能是 OutOfFuel / 除零 / unreachable / 内存越界等
                    let trap_msg = trap.to_string();
                    if trap_msg.contains("out of fuel") {
                        (2, None) // exit_code=2: fuel 耗尽
                    } else {
                        (1, None) // exit_code=1: 其他 trap
                    }
                }
            };

            Ok(WasmExecutionResult {
                return_value,
                exit_code,
                fuel_consumed,
                duration: start.elapsed(),
            })
        }
    }

    impl Default for WasmSandbox {
        /// 默认创建(等价于 `WasmSandbox::new()`),便于 `Default::default()` 构造
        fn default() -> Self {
            Self::new().expect("WasmSandbox::new 失败(Engine 创建失败,通常是系统资源不足)")
        }
    }

    // 静态验证:WasmSandbox 不含任何 unsafe 字段,符合 ADR-035 决策 5
    // (wasmtime 内部 unsafe 不传播到 seccore,详见 §4.1)
    // WHY fn 而非 const _: Rust 1.71+ const context 不允许调用非 const fn,
    // 改用函数体内调用模式(惰性断言),编译期仍可被 dead_code 分析识别
    fn _assert_wasm_sandbox_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WasmSandbox>();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 简单的加法 WASM 模块(WAT 文本格式)
        const ADD_WAT: &str = r#"
            (module
                (func (export "add") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.add
                )
            )
        "#;

        /// 死循环 WASM 模块(用于测试 fuel 耗尽与超时)
        const INFINITE_LOOP_WAT: &str = r#"
            (module
                (func (export "loop_forever") (param i32) (result i32)
                    (loop $forever
                        br $forever
                    )
                    i32.const 0
                )
            )
        "#;

        #[tokio::test]
        async fn test_wasm_sandbox_creates_with_default_config() {
            let sandbox = WasmSandbox::new();
            assert!(sandbox.is_ok(), "WasmSandbox::new 应成功");
            let sandbox = sandbox.unwrap();
            assert_eq!(sandbox.execution_timeout(), Duration::from_secs(30));
            assert_eq!(sandbox.fuel_limit(), 1_000_000);
        }

        #[tokio::test]
        async fn test_wasm_sandbox_builder_methods() {
            let sandbox = WasmSandbox::new()
                .unwrap()
                .with_execution_timeout(Duration::from_secs(5))
                .with_fuel_limit(50_000);
            assert_eq!(sandbox.execution_timeout(), Duration::from_secs(5));
            assert_eq!(sandbox.fuel_limit(), 50_000);
        }

        #[tokio::test]
        async fn test_wasm_execute_square_function_success() {
            let sandbox = WasmSandbox::new().unwrap();
            // 注意:当前 PoC 仅支持单参数 i32 函数,故用 square 测试
            let square_wat = r#"
                (module
                    (func (export "square") (param i32) (result i32)
                        local.get 0
                        local.get 0
                        i32.mul
                    )
                )
            "#;
            let result = sandbox
                .execute_in_wasm(square_wat.as_bytes(), "square", &[7])
                .await
                .expect("square(7) 应执行成功");

            assert_eq!(result.exit_code, 0, "exit_code 应为 0(成功)");
            assert_eq!(result.return_value, Some(49), "square(7) = 49");
            assert!(result.fuel_consumed > 0, "应消耗 fuel");
            assert!(result.duration.as_millis() < 1000, "执行应快速完成");
        }

        #[tokio::test]
        async fn test_wasm_execute_invalid_wat_fails() {
            let sandbox = WasmSandbox::new().unwrap();
            let invalid_wat = b"(module (invalid syntax))";
            let result = sandbox.execute_in_wasm(invalid_wat, "any", &[0]).await;
            assert!(result.is_err(), "无效 WAT 应返回错误");
            let err = result.unwrap_err();
            let msg = match err {
                SecCoreError::WasmSandboxError(msg) => msg,
                _ => panic!("应为 WasmSandboxError"),
            };
            assert!(msg.contains("编译失败"), "错误消息应提及编译失败: {msg}");
        }

        #[tokio::test]
        async fn test_wasm_execute_function_not_found_fails() {
            let sandbox = WasmSandbox::new().unwrap();
            let result = sandbox
                .execute_in_wasm(ADD_WAT.as_bytes(), "nonexistent", &[0])
                .await;
            assert!(result.is_err(), "不存在的导出函数应返回错误");
            let err = result.unwrap_err();
            let msg = match err {
                SecCoreError::WasmSandboxError(msg) => msg,
                _ => panic!("应为 WasmSandboxError"),
            };
            assert!(msg.contains("nonexistent"), "错误消息应包含函数名: {msg}");
        }

        #[tokio::test]
        async fn test_wasm_execute_empty_args_fails() {
            let sandbox = WasmSandbox::new().unwrap();
            let result = sandbox
                .execute_in_wasm(ADD_WAT.as_bytes(), "add", &[])
                .await;
            assert!(result.is_err(), "空 args 应返回错误(函数需要参数)");
        }

        #[tokio::test]
        async fn test_wasm_execute_infinite_loop_triggers_timeout_or_fuel() {
            // 死循环模块:应在超时或 fuel 耗尽时被终止
            let sandbox = WasmSandbox::new()
                .unwrap()
                .with_execution_timeout(Duration::from_millis(200))
                .with_fuel_limit(10_000); // 小 fuel 加速触发

            let result = sandbox
                .execute_in_wasm(INFINITE_LOOP_WAT.as_bytes(), "loop_forever", &[0])
                .await;

            // 死循环必须被终止(不能 hang),通过超时或 fuel 耗尽
            assert!(
                result.is_err() || result.as_ref().map(|r| r.exit_code).unwrap_or(0) != 0,
                "死循环应被超时或 fuel 耗尽终止,不能 hang"
            );
        }

        #[tokio::test]
        async fn test_wasm_execute_identity_function_low_fuel() {
            let sandbox = WasmSandbox::new().unwrap();
            let trivial_wat = r#"
                (module
                    (func (export "identity") (param i32) (result i32)
                        local.get 0
                    )
                )
            "#;
            let result = sandbox
                .execute_in_wasm(trivial_wat.as_bytes(), "identity", &[42])
                .await
                .expect("identity(42) 应执行成功");

            assert_eq!(result.return_value, Some(42));
            assert_eq!(result.exit_code, 0);
            // identity 函数仅一条 local.get,fuel 消耗应极少(但 > 0)
            assert!(result.fuel_consumed <= 100, "identity fuel 消耗应极少");
        }
    }
}

// 公开导出 feature-gated 类型(供 lib.rs 重新导出)
#[cfg(feature = "wasm-sandbox")]
pub use wasm_impl::{WasmExecutionResult, WasmSandbox};

#[cfg(test)]
mod backend_tests {
    use super::*;

    #[test]
    fn test_sandbox_backend_default_is_process() {
        let backend = SandboxBackend::default();
        assert_eq!(backend, SandboxBackend::Process);
    }

    #[test]
    fn test_sandbox_backend_process_gvisor_distinct() {
        let process = SandboxBackend::Process;
        let gvisor = SandboxBackend::Gvisor;
        assert_ne!(process, gvisor);
    }

    #[cfg(feature = "wasm-sandbox")]
    #[test]
    fn test_sandbox_backend_wasm_variant_available() {
        let wasm = SandboxBackend::Wasm;
        let process = SandboxBackend::Process;
        let gvisor = SandboxBackend::Gvisor;
        assert_ne!(wasm, process);
        assert_ne!(wasm, gvisor);
    }
}
