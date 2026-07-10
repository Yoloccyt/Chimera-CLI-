# clippy-driver.exe 崩溃根因分析报告(STATUS_STACK_BUFFER_OVERRUN)

**任务**:Week 8 限制深度攻坚 Spec · Task 1(SubTask 1.1–1.6)
**分析日期**:2026-06-27
**分析人**:NEXUS-OMEGA 首席架构师(E1)
**结论摘要**:崩溃表面异常代码 `0xC0000409 (STATUS_STACK_BUFFER_OVERRUN)` 具有误导性;经 minidump 反汇编定位,实际根因为 **`std::alloc::rust_oom`(堆内存分配失败 OOM)** 触发 `__fastfail(FAST_FAIL_FATAL_APP_EXIT=7)`。这**不是**栈金丝雀失败、不是栈溢出、不是 `__stack_chk_fail`。

---

## 1. 环境

| 项目 | 值 |
|------|-----|
| 操作系统 | Windows 11 build 26200.8655(10.0.26200.2.0.0.768.99) |
| 架构 | x86_64(AMD64) |
| 工具链 | `stable-x86_64-pc-windows-gnu`(GNU,非 MSVC) |
| rustc 版本 | `rustc 1.96.0 (ac68faa20 2026-05-25)` |
| cargo 版本 | `cargo 1.96.0 (30a34c682 2026-05-25)` |
| clippy 版本 | `clippy 0.1.96 (ac68faa20c 2026-05-25)` |
| procdump 版本 | `ProcDump v12.0`(Sysinternals,2026 版) |
| 反汇编工具 | `objdump.exe`(MinGW-w64,`D:\msys64\mingw64\bin`) |
| 调试器 | cdb.exe / WinDbg **未安装**(Windows Kits Debuggers 缺失) |
| 链接器 | `D:\msys64\mingw64\bin\gcc.exe` |
| CARGO_HOME | `D:\Chimera CLI\.toolchain\cargo` |
| RUSTUP_HOME | `D:\Chimera CLI\.toolchain\rustup` |
| 故障进程 | `clippy-driver.exe`(时间戳 `0x6a14f5e4`) |
| 故障模块 | `std-b0558c7fd7f3aef7.dll`(Rust 标准库 DLL,时间戳 `0x6a14e687`) |
| 故障模块大小 | 5,019,312 bytes(~4.8MB) |

**工具链关键特征**:GNU 工具链下 Rust 标准库被编译为独立 DLL(`std-<hash>.dll`)并由多个 `clippy-driver.exe` 进程各自加载;同时加载 `libgcc_s_seh-1.dll`、`libwinpthread-1.dll`、`rustc_driver-<hash>.dll` 以及若干 proc-macro DLL(thiserror_impl / serde_derive / zerocopy_derive / tokio_macros / tracing_attributes)。

---

## 2. 现象

### 2.1 崩溃主表现

执行 `cargo clippy --workspace --all-targets`(默认 `--jobs`,并行度 = CPU 核数)时:

- 多个 `clippy-driver.exe` 子进程同时崩溃,exit code `0xc0000409`
- cargo 汇总退出码 `101`
- 连锁编译错误:
  - `error: only metadata stub found for 'rlib' dependency 'core' please provide path to the corresponding .rmeta file with full metadata`
  - `error[E0786]: ...`
  - `error[E0463]: can't find crate`
  - `internal compiler error`
- 单次运行日志中 `STATUS_STACK_BUFFER_OVERRUN` 出现 **10 次**(对应 10 个 clippy-driver 崩溃)

崩溃的 crate(部分):`gsoe-evolution`、`gea-activator`、`sesa-router`、`osa-coordinator`、`nmc-encoder` 等(均为依赖较多、编译负载较重的 crate)。

### 2.2 Windows 事件日志(Event ID 1000 / Application Error)

4 条 `clippy-driver.exe` 崩溃记录(时间 2026-06-27 22:47:57):

```
出错应用程序名称： clippy-driver.exe，版本： 0.0.0.0，时间戳： 0x6a14f5e4
出错模块名称： std-b0558c7fd7f3aef7.dll， 版本： 0.0.0.0，时间戳： 0x6a14e687
异常代码： 0xc0000409
错误偏移： 0x00000000000171b1
出错进程 ID： 0x5F94 / 0x546C / 0x22D0 / 0x7CD0
Faulting 应用程序路径： D:\Chimera CLI\.toolchain\rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\clippy-driver.exe
Faulting 模块路径： D:\Chimera CLI\.toolchain\rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\std-b0558c7fd7f3aef7.dll
```

**关键观察**:故障模块统一是 `std-*.dll`,故障偏移统一是 `0x171b1`。这排除了"clippy-driver 自身代码 bug"的可能性,定位到 std 库内部固定代码路径。

### 2.3 WER 事件(Event ID 1001 / BEX64)

```
事件名称: BEX64
P1: clippy-driver.exe     P2: 0.0.0.0     P3: 6a14f5e4
P4: std-b0558c7fd7f3aef7.dll  P5: 0.0.0.0  P6: 6a14e687
P7: 00000000000171b1   ← 故障偏移
P8: c0000409           ← 异常代码
P9: 0000000000000007   ← fastfail code = 7
```

`P9 = 0x7` 是 BEX64 报告中的 **fastfail code**,对应 `FAST_FAIL_FATAL_APP_EXIT`。

---

## 3. 捕获过程

### 3.1 procdump 安装(方式 B,成功)

winget 在沙箱环境不便使用,采用方式 B 直接下载:

```powershell
$ProgressPreference = 'SilentlyContinue'
Invoke-WebRequest -Uri "https://download.sysinternals.com/files/Procdump.zip" `
    -OutFile "D:\Chimera CLI\tmp\Procdump.zip" -UseBasicParsing -TimeoutSec 60
Expand-Archive -Path "D:\Chimera CLI\tmp\Procdump.zip" `
    -DestinationPath "D:\Chimera CLI\tmp\procdump" -Force
```

安装结果:
- `D:\Chimera CLI\tmp\procdump\procdump.exe`(32 位,1,344,872 bytes)
- `D:\Chimera CLI\tmp\procdump\procdump64.exe`(64 位,724,328 bytes)← 本机使用
- `D:\Chimera CLI\tmp\procdump\procdump64a.exe`(ARM64,726,336 bytes)
- 版本:`ProcDump v12.0 - Sysinternals process dump utility`

### 3.2 procdump `-e 1` 监控方式(失败 — 重要发现)

按 Spec 方式 A 启动 procdump 异常监控:

```powershell
Start-Process -FilePath "D:\Chimera CLI\tmp\procdump\procdump64.exe" `
    -ArgumentList "-ma -e 1 -f `"`" <PID> D:\Chimera CLI\tmp\clippy_dumps" -NoNewWindow -PassThru
```

由于 procdump 必须附加到**已存在**的进程,编写了循环监控脚本(`tmp\monitor_clippy.ps1`):每 250ms 扫描 `clippy-driver.exe` 进程,对每个新 PID 启动一个 `procdump64 -ma -e 1` 附加实例。

**监控日志摘要**(40+ 个 clippy-driver PID 被附加):
```
22:46:07.363 attaching procdump to PID 33496
22:46:09.210 attaching procdump to PID 1228
22:47:18.220 attaching procdump to PID 17360
... (共 40 个 PID)
22:47:40.783 monitor stopped
```

**结果**:`clippy_dumps\` 目录**未生成任何 .dmp 文件**。

**根因**:`STATUS_STACK_BUFFER_OVERRUN (0xC0000409)` 在本场景由 `__fastfail` 触发(`int 0x29` 指令),这是 Windows 的"快速失败"机制,**绕过正常的结构化异常处理(SEH)流程**,直接进入内核终止路径。procdump 的 `-e 1`(异常监控)依赖 SEH 异常回调,因此**无法捕获 `__fastfail`** 触发的崩溃。这是本次分析的重要副产物——对 `__fastfail` 类崩溃,procdump `-e 1` 不是有效捕获手段。

### 3.3 WER 默认 minidump 捕获(成功)

Windows Error Reporting 默认配置在进程崩溃时自动生成 minidump,存放于:

```
C:\Users\<USERNAME>\AppData\Local\CrashDumps\
```

本次崩溃生成的 dump 文件(4 个,均为 22:48:02–04 生成):

| 文件名 | 大小 | 时间 |
|--------|------|------|
| `clippy-driver.exe.21612.dmp` | 2,687,344 bytes | 22:48:02 |
| `clippy-driver.exe.8912.dmp` | 2,685,132 bytes | 22:48:02 |
| `clippy-driver.exe.31952.dmp` | 2,675,592 bytes | 22:48:03 |
| `clippy-driver.exe.24468.dmp` | 2,697,300 bytes | 22:48:04 |

每个 dump ~2.6MB,为 WER 默认的"小型 dump"(含异常流、线程列表、模块列表、部分内存)。

已将其中 2 个复制到工作目录备份:
- `D:\Chimera CLI\tmp\clippy_dumps\clippy_24468.dmp`
- `D:\Chimera CLI\tmp\clippy_dumps\clippy_21612.dmp`

### 3.4 运行参数

```powershell
$env:RUST_MIN_STACK     = '33554432'   # 32MB(为复现 Spec 中的实验 C 条件)
$env:CARGO_INCREMENTAL  = '0'
$env:CARGO_TARGET_DIR   = 'D:\Chimera CLI\target_clippy_dump'  # 全新目录从零编译
cargo clippy --workspace --all-targets 2>&1 | Tee-Object -FilePath "D:\Chimera CLI\tmp\clippy_crash_run.log"
```

**本次崩溃耗时**:9.88s(exit 101)。比 Week 8 Task 3 实验 C 的 89.75s 更快,因为部分依赖 rmeta 已被缓存;但崩溃现象完全一致(10 次 STATUS_STACK_BUFFER_OVERRUN)。

---

## 4. dump 分析

### 4.1 MINIDUMP 文件头解析

用 PowerShell `[System.IO.File]::ReadAllBytes` + `[BitConverter]` 直接解析 minidump 二进制结构(`clippy_24468.dmp`):

```
Signature       : MDMP(0x4D444D50)
Version         : 2700388243
NumberOfStreams : 14
StreamDirRva    : 0x20
```

### 4.2 异常流(MINIDUMP_EXCEPTION_STREAM, type=6)

```
Stream 4 : ExceptionStream size=168 rva=0x654
  ThreadId          : 34388
  ExceptionCode     : 0xC0000409   (STATUS_STACK_BUFFER_OVERRUN)
  ExceptionFlags    : 0x1          (EXCEPTION_NONCONTINUABLE)
  ExceptionAddress  : 0x7FFA08CF71B1
```

`ExceptionAddress = 0x7FFA08CF71B1`,其低 32 位 `0x00071B1` 即 `0x171B1`,与 WER 报告的故障偏移 `0x171b1` 完全一致。结合 std DLL 的加载基址 `0x180000000`(PE 默认 ImageBase),可确认崩溃发生在 `std-b0558c7fd7f3aef7.dll` 的 RVA `0x171B1` 处。

### 4.3 系统信息流(MINIDUMP_SYSTEM_INFO_STREAM, type=7)

```
Stream 5 : SystemInfoStream size=56 rva=0xC8
  ProcessorArchitecture : 9(IMAGE_FILE_MACHINE_AMD64)
  ProcessorLevel        : 25
```

### 4.4 加载模块流(WER Report.wer 提取的关键模块)

| # | 模块 | 角色 |
|---|------|------|
| 0 | clippy-driver.exe | 故障进程(驱动器) |
| 5 | libgcc_s_seh-1.dll | GCC SEH 异常处理 |
| 6 | rustc_driver-1ad2f2551216e8fb.dll | rustc 编译器核心 |
| **7** | **std-b0558c7fd7f3aef7.dll** | **Rust 标准库(故障模块)** |
| 8 | bcryptprimitives.dll | 加密原语(栈 cookie 生成) |
| 9 | libwinpthread-1.dll | winpthreads |
| 28 | thiserror_impl-30092f55cf30dd18.dll | proc-macro DLL |
| 29 | serde_derive-7dce5ae9c6b03c09.dll | proc-macro DLL |
| 30 | zerocopy_derive-0e8858c27a135fcb.dll | proc-macro DLL |
| 31 | tokio_macros-56e689f5760b9221.dll | proc-macro DLL |
| 32 | tracing_attributes-810a0302c3b82112.dll | proc-macro DLL |

**关键观察**:崩溃时进程已加载 5 个 proc-macro DLL。proc-macro 在 clippy-driver 进程内**就地执行**(而非子进程),其内存分配计入 clippy-driver 进程。并行编译时,多个 clippy-driver 同时执行 proc-macro,堆内存压力叠加。

---

## 5. 调用栈(反汇编定位)

由于 cdb.exe / WinDbg 未安装,采用 `objdump.exe`(MinGW-w64)反汇编 `std-b0558c7fd7f3aef7.dll` 定位 RVA `0x171B1` 所属函数。

### 5.1 崩溃点反汇编

```asm
0000000180017180 <_RNCNvNtCs1ol9KfofPpO_3std5alloc8rust_oom0B5_>:
   180017180:   55                      push   %rbp
   180017181:   48 83 ec 20             sub    $0x20,%rsp
   180017185:   48 8d 6c 24 20          lea    0x20(%rsp),%rbp
   18001718a:   48 8b 05 97 8e 0b 00    mov    0xb8e97(%rip),%rax   # 0x1800d0028 <std::alloc::HOOK>
   180017191:   48 85 c0                test   %rax,%rax
   180017194:   4c 8d 05 7d 09 02 00    lea    0x2097d(%rip),%r8    # 0x180037b18 <std::alloc::default_alloc_error_hook>
   18001719b:   4c 0f 45 c0             cmovne %rax,%r8              ; 选择自定义 hook 或默认 hook
   18001719f:   48 8b 01                mov    (%rcx),%rax           ; 读取 alloc::Layout.size
   1800171a2:   48 8b 51 08             mov    0x8(%rcx),%rdx        ; 读取 alloc::Layout.align
   1800171a6:   48 89 c1                mov    %rax,%rcx
   1800171a9:   41 ff d0                call   *%r8                  ; 调用 alloc error hook(打印 OOM 消息)
   1800171ac:   b9 07 00 00 00          mov    $0x7,%ecx             ; fastfail code = 7 = FAST_FAIL_FATAL_APP_EXIT
   1800171b1:   cd 29                   int    $0x29                 ; ★ 崩溃点 ★ __fastfail(7)
   1800171b3:   0f 0b                   ud2                           ; 不可达
```

### 5.2 函数符号解码

Rust v0 mangling:`_RNCNvNtCs1ol9KfofPpO_3std5alloc8rust_oom0B5_`

| 段 | 解码 |
|----|------|
| `_RN` | Rust v0 mangled symbol 前缀 |
| `C` | 闭包(closure) |
| `Nv` | 嵌套路径 |
| `NtCs1ol9KfofPpO_` | crate(离散化哈希 `1ol9KfofPpO`) |
| `3std` | `std` |
| `5alloc` | `alloc` |
| `8rust_oom` | **`rust_oom`** ← 函数名 |
| `0B5_` | 闭包消歧后缀 |

**结论**:崩溃函数 = `std::alloc::rust_oom`(Rust 全局分配器的 OOM 处理函数)。

### 5.3 函数语义还原

对照 Rust 源码(`library/std/src/alloc.rs`),`rust_oom` 的逻辑为:

```rust
// 简化伪代码(基于反汇编还原)
fn rust_oom(layout: Layout) -> ! {
    let hook = HOOK.load().unwrap_or(default_alloc_error_hook);
    hook(layout);                          // 打印 "memory allocation of N bytes failed"
    r#abort();                             // → __fastfail(FAST_FAIL_FATAL_APP_EXIT)
}
```

反汇编完全匹配:读取全局 `HOOK` → 选择 `default_alloc_error_hook` → 间接 `call *%r8` 调用 hook → `mov $0x7, %ecx; int $0x29` 执行 `__fastfail(7)`。

### 5.4 调用链推断(基于符号邻接)

反汇编显示 `rust_oom` 紧邻下列符号:
- `0x1800171b5 <std::panicking::default_hook>`(panic 默认 hook)
- `0x180041780 <std::sys::args::windows::append_arg>`

可推断调用链:

```
[proc-macro / clippy 代码请求分配内存]
  → <GlobalAlloc>::alloc(...) 失败(返回 null)
  → handle_alloc_error(layout)
  → std::alloc::rust_oom(layout)        ← 反汇编定位的崩溃函数
    → default_alloc_error_hook(layout)   ← 打印 OOM
    → __fastfail(FAST_FAIL_FATAL_APP_EXIT=7)  ← int 0x29
      → 内核 STATUS_STACK_BUFFER_OVERRUN(0xC0000409)  ← 误导性异常名
        → 进程终止
          → WER 生成 minidump + 事件日志
```

> **注**:由于 minidump 不含完整调用栈内存,且无 cdb 符号化,`rust_oom` 之上的调用者(具体是哪个分配请求失败)无法从 dump 直接读取。但 `rust_oom` 本身的定位是确定性的——崩溃指令地址精确落在 `rust_oom` 函数体内。

---

## 6. 根因结论

### 6.1 根因(已确认,非推断)

**根因:并行编译时堆内存不足(OOM),触发 `std::alloc::rust_oom` 调用 `__fastfail(FAST_FAIL_FATAL_APP_EXIT=7)` 终止进程。**

证据链(四重互证):

1. **反汇编定位**:崩溃地址 `0x171B1` 落在 `std::alloc::rust_oom` 函数体内(函数起址 `0x180017180`),指令为 `mov $0x7, %ecx; int $0x29` = `__fastfail(7)`。
2. **fastfail code 语义**:P9 = `0x7` = `FAST_FAIL_FATAL_APP_EXIT`,是 Rust `abort()` 路径(`__rust_abort` → `__fastfail(7)`)。`rust_oom` 在调用 alloc error hook 后执行此路径。
3. **函数语义**:`rust_oom` 是 Rust 全局分配器的 OOM 处理函数,**仅在内存分配失败时被调用**。进入此函数 ⟺ 发生 OOM。
4. **异常代码解释**:`0xC0000409 (STATUS_STACK_BUFFER_OVERRUN)` 是 `__fastfail` 指令(`int 0x29`)的**统一异常代码**,无论 fastfail code 是 2/7/14 都显示为此代码。**异常名具有误导性**——必须看 fastfail code(P9)才能区分。

### 6.2 排除的其他假设

| 假设 | 排除依据 |
|------|---------|
| **栈空间不足**(stack overflow) | 栈溢出异常代码应为 `0xC00000FD (STATUS_STACK_OVERFLOW)`,且 `RUST_MIN_STACK=33554432`(32MB)无效。反汇编显示崩溃函数是 `rust_oom`,与栈深度无关。 |
| **`/GS` 栈金丝雀失败**(`__stack_chk_fail`) | GS 失败的 fastfail code 应为 `FAST_FAIL_LOCAL_BUFFER_OVERFLOW=14` 或 `FAST_FAIL_INCORRECT_STACK=2`,而本例 P9=7。且崩溃函数是 `rust_oom`,非 `__stack_chk_fail`。 |
| **堆损坏** | 堆损坏通常触发 `FAST_FAIL_HEAP_CORRUPTION=21`,而本例 P9=7。`rust_oom` 是 OOM 主动 abort,不是堆元数据损坏被动检测。 |
| **clippy-driver 自身代码 bug** | 故障模块是 `std-*.dll`,故障偏移固定 `0x171B1`,4 次崩溃完全一致,与 clippy 业务逻辑无关。 |
| **panic=abort 触发** | Workspace 默认 `panic=unwind`;且 panic abort 路径走 `std::panicking::abort`,崩溃函数应为 panic 路径而非 `rust_oom`。 |

### 6.3 触发条件(推断,基于实验数据)

- **并行度 = CPU 核数**(默认 `--jobs`)时,多个 `clippy-driver.exe` 进程同时运行
- 每个进程加载 `rustc_driver-*.dll`(~数十 MB)+ `std-*.dll`(~5MB)+ 5 个 proc-macro DLL(各数 MB)
- proc-macro(thiserror_impl / serde_derive / zerocopy_derive / tokio_macros / tracing_attributes)在进程内执行,额外消耗堆内存
- 高并行度下系统堆内存压力过大,某次 `alloc()` 返回 null → `handle_alloc_error` → `rust_oom` → 进程终止
- 一个 clippy-driver 崩溃后,其 `.rmeta` 文件不完整,导致依赖它的 crate 报 `only metadata stub found` / `E0786` / `E0463` 连锁错误

### 6.4 与 3 组对比实验的吻合性

| 实验 | --jobs | 结果 | 耗时 | OOM? | 解释 |
|------|--------|------|------|------|------|
| A | 1(串行) | ✅ exit 0 | 600.69s | 否 | 单进程,堆内存充裕 |
| B | 2(低并行) | ✅ exit 0 | 335.97s | 否 | 2 进程,堆内存仍够 |
| C | 默认(CPU 核数) | ❌ exit 101 | 89.75s 崩溃 | **是** | 高并行,堆耗尽 → rust_oom |

`RUST_MIN_STACK=33554432`(32MB)在实验 C 中**无效**,因为根因是**堆**内存不足,增大**栈**大小对症不下药——这是确认根因为 OOM(而非栈)的强旁证。

---

## 7. 建议

### 7.1 本地 workaround(已验证有效)

```powershell
$env:RUST_MIN_STACK    = '33554432'    # 32MB 栈(保险措施,虽非根因解)
$env:CARGO_INCREMENTAL = '0'           # 禁用增量,减少文件锁竞争
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
```

`--jobs 2` 将并行度限制在 2,实测 335.97s 完成,零警告,零崩溃。比 `--jobs 1`(600.69s)快 44%,且避免 OOM。

**针对根因的更优 workaround(可选)**:
- 进一步调低 `--jobs`(如系统内存紧张可试 `--jobs 1`)
- 关闭其他内存占用大的程序(浏览器、IDE 等)后再跑 clippy
- 增加系统页面文件(`sysdm.cpl` → 高级 → 虚拟内存)以确保堆分配不失败

### 7.2 上游报告

向 `rust-lang/rust-clippy` 提交 issue(草稿见 `docs/dev/upstream_clippy_issue_draft.md`),要点:
- Windows GNU 工具链 + 默认并行 jobs → `clippy-driver.exe` 崩溃
- 表面异常 `0xC0000409(STATUS_STACK_BUFFER_OVERRUN)` 实为 `__fastfail(FAST_FAIL_FATAL_APP_EXIT=7)`
- 反汇编定位 `std::alloc::rust_oom`,根因 = OOM
- 建议上游:clippy 在 Windows GNU 下对 `--jobs` 提供更保守的默认值,或在 OOM 时打印更友好的诊断信息(当前 `default_alloc_error_hook` 输出可能因 `int 0x29` 立即终止而未被刷新到 stderr)

### 7.3 CI 环境

- GitHub Actions Linux runner 不受影响(Linux 下默认并行 jobs 正常,Linux 无 `__fastfail` 机制,OOM 走 SIGKILL/SIGABRT 路径且有 cgroup 内存限制保护)
- Windows runner 如复现,建议在 workflow 中显式设置 `CARGO_BUILD_JOBS` 或 clippy 的 `--jobs`

### 7.4 长期改进(超出本 Task 范围)

- 将本机 Windows 开发迁移至 MSVC 工具链(`stable-x86_64-pc-windows-msvc`),std 以静态库形式链接,无 DLL 共享状态问题,且 MSVC 的 OOM 诊断更友好
- 考虑为 workspace 添加 `.cargo/config.toml` 配置 `[build] jobs = 2`(仅 Windows GNU target),固化 workaround

### 7.5 证据文件索引

| 文件 | 用途 |
|------|------|
| `D:\Chimera CLI\tmp\clippy_dumps\clippy_24468.dmp` | WER minidump(主分析样本) |
| `D:\Chimera CLI\tmp\clippy_dumps\clippy_21612.dmp` | WER minidump(备份样本) |
| `D:\Chimera CLI\tmp\clippy_dumps\monitor.log` | procdump 监控日志(40 PID 附加记录) |
| `D:\Chimera CLI\tmp\clippy_crash_run.log` | clippy 崩溃完整 stdout/stderr |
| `C:\Users\<USER>\AppData\Local\CrashDumps\clippy-driver.exe.*.dmp` | WER 默认 dump 原始位置(4 个) |
| `C:\ProgramData\Microsoft\Windows\WER\ReportArchive\AppCrash_clippy-driver.ex_*\Report.wer` | WER 详细文本报告 |

---

## 附录 A:procdump 监控脚本(`tmp\monitor_clippy.ps1`)

```powershell
# 循环监控 clippy-driver.exe 进程,对每个新 PID 启动 procdump -e 1 异常监控
$procdumpPath = "D:\Chimera CLI\tmp\procdump\procdump64.exe"
$dumpDir      = "D:\Chimera CLI\tmp\clippy_dumps"
$stopFlag     = "D:\Chimera CLI\tmp\clippy_dumps\STOP_MONITOR.flag"
$logFile      = "D:\Chimera CLI\tmp\clippy_dumps\monitor.log"

$seenPids = @{}
Add-Content -Path $logFile -Value "$(Get-Date -Format 'HH:mm:ss.fff') monitor started"

while (-not (Test-Path $stopFlag)) {
    $procs = Get-Process -Name "clippy-driver" -ErrorAction SilentlyContinue
    if ($procs) {
        foreach ($p in $procs) {
            if (-not $seenPids.ContainsKey($p.Id)) {
                $seenPids[$p.Id] = $true
                Add-Content -Path $logFile -Value "$(Get-Date -Format 'HH:mm:ss.fff') attaching procdump to PID $($p.Id)"
                Start-Process -FilePath $procdumpPath `
                    -ArgumentList "-ma -e 1 -f `"`" $($p.Id) $dumpDir" `
                    -NoNewWindow -PassThru -ErrorAction SilentlyContinue | Out-Null
            }
        }
    }
    Start-Sleep -Milliseconds 250
}
Add-Content -Path $logFile -Value "$(Get-Date -Format 'HH:mm:ss.fff') monitor stopped"
```

## 附录 B:MINIDUMP 异常流解析脚本片段

```powershell
$bytes = [System.IO.File]::ReadAllBytes($dumpPath)
$numStreams    = [BitConverter]::ToUInt32($bytes, 8)
$streamDirRva  = [BitConverter]::ToUInt32($bytes, 12)
for ($i = 0; $i -lt $numStreams; $i++) {
    $off          = $streamDirRva + ($i * 12)
    $streamType   = [BitConverter]::ToUInt32($bytes, $off)
    $rva          = [BitConverter]::ToUInt32($bytes, $off + 8)
    if ($streamType -eq 6) {  # ExceptionStream
        $threadId         = [BitConverter]::ToUInt32($bytes, $rva)
        $exceptionCode    = [BitConverter]::ToUInt32($bytes, $rva + 8)
        $exceptionAddress = [BitConverter]::ToUInt64($bytes, $rva + 24)
    }
}
```

---

**NEXUS-OMEGA — Ω-Sparse · Ω-Compress · Ω-Evolve · Ω-Event**
