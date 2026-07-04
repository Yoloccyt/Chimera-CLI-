# Upstream Issue Draft: clippy-driver.exe OOM crash on Windows GNU toolchain

**Target repository**: `rust-lang/rust-clippy`(同时可能涉及 `rust-lang/rust` 的 `std::alloc` 路径)
**Status**: Draft(待提交)
**Draft date**: 2026-06-27

> 本文件为向 rust-lang/rust-clippy 上游提交 issue 的草稿。提交前需根据 GitHub issue 模板最终格式调整,并附上 minidump / WER 报告等证据文件(可上传至 gist 或 issue 附件)。

---

## 1. Title

**clippy-driver.exe crashes with STATUS_STACK_BUFFER_OVERRUN (0xC0000409) on Windows GNU toolchain with default parallel jobs — root cause is OOM via `std::alloc::rust_oom`, not a stack issue**

(简短备选标题:`clippy-driver.exe OOM crash on Windows GNU with default --jobs; misleadingly reported as STATUS_STACK_BUFFER_OVERRUN`)

---

## 2. Environment

| Item | Value |
|------|-------|
| OS | Windows 11 build 26200.8655(10.0.26200.2.0.0.768.99) |
| Architecture | x86_64 |
| Toolchain | `stable-x86_64-pc-windows-gnu`(**GNU**, not MSVC) |
| rustc | `rustc 1.96.0 (ac68faa20 2026-05-25)` |
| cargo | `cargo 1.96.0 (30a34c682 2026-05-25)` |
| clippy | `clippy 0.1.96 (ac68faa20c 2026-05-25)` |
| CPU cores | (default `--jobs` = physical core count; high parallelism) |
| Linker | `gcc.exe`(MinGW-w64, mingw64) |
| Workspace size | 34 crates, ~3000 tests, `--all-targets` |
| Relevant env | `RUST_MIN_STACK=33554432`(32MB, **no effect**), `CARGO_INCREMENTAL=0` |

---

## 3. Reproduction

```powershell
# Fresh target dir to avoid cache effects
$env:CARGO_TARGET_DIR  = 'D:\repro\target_clippy_dump'
$env:RUST_MIN_STACK    = '33554432'   # 32MB — does NOT help
$env:CARGO_INCREMENTAL = '0'

cargo clippy --workspace --all-targets
#                                ^^^^^^^^^^^^^^
#                  default --jobs = CPU core count (high parallelism)
```

Reproduces consistently on the affected Windows GNU environment. The crash occurs during the `Checking` phase, after ~10–90 seconds depending on cache state, across multiple crates simultaneously.

---

## 4. Expected

`cargo clippy --workspace --all-targets` completes with exit code 0 and zero warnings (as it does with `--jobs 1` or `--jobs 2`, and as it does on Linux/macOS with default jobs).

---

## 5. Actual

- Multiple `clippy-driver.exe` child processes crash simultaneously with exit code `0xc0000409`
- `cargo` exits with code `101`
- Cascading compilation errors after the crash:
  - `error: only metadata stub found for 'rlib' dependency 'core' please provide path to the corresponding .rmeta file with full metadata`
  - `error[E0786]: ...`
  - `error[E0463]: can't find crate`
  - `internal compiler error`
- A single run logs **10** `STATUS_STACK_BUFFER_OVERRUN` occurrences(10 clippy-driver crashes)

### Windows Application Error events (Event ID 1000)

```
Faulting application name: clippy-driver.exe, version: 0.0.0.0, timestamp: 0x6a14f5e4
Faulting module name:      std-b0558c7fd7f3aef7.dll, version: 0.0.0.0, timestamp: 0x6a14e687
Exception code:            0xc0000409
Fault offset:              0x00000000000171b1
Faulting process ID:       0x5F94 / 0x546C / 0x22D0 / 0x7CD0  (4 distinct crashes)
Faulting application path: ...\stable-x86_64-pc-windows-gnu\bin\clippy-driver.exe
Faulting module path:      ...\stable-x86_64-pc-windows-gnu\bin\std-b0558c7fd7f3aef7.dll
```

### Windows Error Reporting events (Event ID 1001, BEX64)

```
Event Name: BEX64
P1: clippy-driver.exe              P2: 0.0.0.0       P3: 6a14f5e4
P4: std-b0558c7fd7f3aef7.dll       P5: 0.0.0.0       P6: 6a14e687
P7: 00000000000171b1   ← fault offset
P8: c0000409           ← exception code
P9: 0000000000000007   ← fastfail code = 7 (FAST_FAIL_FATAL_APP_EXIT)
```

**Key observation**: P9 = `0x7` is the `__fastfail` code, identifying `FAST_FAIL_FATAL_APP_EXIT`. This is **not** the GS cookie failure code (`0xE` / `0x2`), despite the misleading exception name `STATUS_STACK_BUFFER_OVERRUN`.

---

## 6. Evidence

### 6.1 Comparative experiments (3 runs, all with `RUST_MIN_STACK=33554432` + `CARGO_INCREMENTAL=0`, fresh `CARGO_TARGET_DIR`)

| Run | `--jobs` | Result | Duration | STATUS_STACK_BUFFER_OVERRUN | Warnings |
|-----|----------|--------|----------|------------------------------|----------|
| A | 1 (serial) | ✅ exit 0 | 600.69s (10m00s) | none | 0 |
| B | 2 (low parallel) | ✅ exit 0 | 335.97s (5m36s) | none | 0 |
| **C** | **default (CPU cores)** | **❌ exit 101** | **89.75s (crash)** | **yes (0xc0000409)** | **N/A** |

`RUST_MIN_STACK=33554432`(32MB stack)has **no effect** on the crash — strong evidence the root cause is **not** stack depth.

### 6.2 Minidump analysis(WER default dump, `C:\Users\...\AppData\Local\CrashDumps\clippy-driver.exe.*.dmp`)

MINIDUMP exception stream(`clippy-driver.exe.24468.dmp`, parsed via PowerShell `[BitConverter]`):

```
ThreadId          : 34388
ExceptionCode     : 0xC0000409   (STATUS_STACK_BUFFER_OVERRUN)
ExceptionFlags    : 0x1          (EXCEPTION_NONCONTINUABLE)
ExceptionAddress  : 0x7FFA08CF71B1
  └─ low 32 bits = 0x171B1 = fault offset in std-*.dll (matches WER P7)
```

### 6.3 Disassembly of faulting address (objdump on `std-b0558c7fd7f3aef7.dll`)

The faulting RVA `0x171B1` falls inside `std::alloc::rust_oom`(symbol: `_RNCNvNtCs1ol9KfofPpO_3std5alloc8rust_oom0B5_`):

```asm
0000000180017180 <std::alloc::rust_oom>:
   18001718a:  mov  0xb8e97(%rip),%rax          # load std::alloc::HOOK
   180017194:  lea  default_alloc_error_hook,%r8
   18001719b:  cmovne %rax,%r8                  # pick custom or default hook
   1800171a9:  call *%r8                         # invoke alloc error hook (prints OOM msg)
   1800171ac:  mov  $0x7,%ecx                    # fastfail code = 7 = FAST_FAIL_FATAL_APP_EXIT
   1800171b1:  int  $0x29                        # ★ crash site ★  __fastfail(7)
   1800171b3:  ud2                               # unreachable
```

This is the standard Rust OOM path:`rust_oom` → invoke `alloc error hook` → `__fastfail(FAST_FAIL_FATAL_APP_EXIT)`. Reaching `rust_oom` is **definitive proof that a heap allocation returned null**(i.e., OOM).

### 6.4 Loaded modules at crash (WER Report.wer)

Notable: the process had loaded `rustc_driver-*.dll` + `std-*.dll` + `libgcc_s_seh-1.dll` + `libwinpthread-1.dll` + **5 proc-macro DLLs**(`thiserror_impl`, `serde_derive`, `zerocopy_derive`, `tokio_macros`, `tracing_attributes`). Proc-macros execute in-process, contributing to heap pressure.

### 6.5 Note on procdump `-e 1` capture failure

`procdump -e 1`(exception monitoring)could **not** capture these crashes. Reason: `__fastfail`(`int 0x29`)bypasses the normal SEH dispatch and terminates the process directly via the kernel; procdump's exception hook never fires. WER's default LocalDumps mechanism did capture minidumps, however. This side finding may be useful for the upstream documentation.

---

## 7. Workaround

Limit parallelism:

```powershell
$env:RUST_MIN_STACK    = '33554432'
$env:CARGO_INCREMENTAL = '0'
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
```

`--jobs 2` completes in 335.97s with 0 warnings and no crash — 44% faster than `--jobs 1`(600.69s)while avoiding OOM.

Other effective mitigations:
- Close memory-heavy applications(browser, IDE)before running clippy
- Increase Windows page file size
- Use `--jobs 1` if memory is very tight
- Switch to `stable-x86_64-pc-windows-msvc` toolchain(std linked statically, no shared DLL state; not yet tested by reporter but expected to behave better)

---

## 8. Root Cause Analysis

### 8.1 Confirmed root cause: OOM, not a stack issue

The crash is an **out-of-memory abort**, not a stack buffer overrun:

1. **Disassembly locates the crash inside `std::alloc::rust_oom`**(the Rust global allocator's OOM handler).
2. The faulting instruction is `mov $0x7, %ecx; int $0x29` = `__fastfail(FAST_FAIL_FATAL_APP_EXIT=7)` — Rust's `abort()` path invoked after the alloc error hook.
3. `rust_oom` is **only** reached when `GlobalAlloc::alloc()` returns null, i.e., on allocation failure.
4. `0xC0000409 (STATUS_STACK_BUFFER_OVERRUN)` is the **unified exception code** for all `__fastfail` invocations regardless of the fastfail code. The name is misleading; the actual fastfail code(P9 = 7)distinguishes it from GS cookie failures(P9 would be 2 or 14).

### 8.2 Ruled-out hypotheses

| Hypothesis | Why ruled out |
|------------|---------------|
| Stack overflow(`0xC00000FD`) | Wrong exception code; `RUST_MIN_STACK=32MB` ineffective |
| `/GS` stack canary failure(`__stack_chk_fail`) | Would use fastfail code 14 or 2, not 7; crash function is `rust_oom`, not `__stack_chk_fail` |
| Heap corruption | Would trigger fastfail 21(`FAST_FAIL_HEAP_CORRUPTION`); here it's 7 |
| clippy-driver business-logic bug | Faulting module is `std-*.dll` at a fixed offset across 4 crashes; not clippy code |
| `panic=abort` | Workspace uses default `panic=unwind`; abort path goes through `std::panicking::abort`, not `rust_oom` |

### 8.3 Triggering condition(inferred)

Under default `--jobs`(= CPU core count), multiple `clippy-driver.exe` processes run concurrently. Each loads `rustc_driver-*.dll` + `std-*.dll` + several proc-macro DLLs, and proc-macros execute in-process. Aggregate heap pressure under high parallelism exceeds available memory(or commit limit), an `alloc()` returns null, and `handle_alloc_error` → `rust_oom` → `__fastfail(7)` terminates the process. Subsequent crates then see incomplete `.rmeta` from the crashed process, producing the cascading `only metadata stub found` / `E0786` / `E0463` errors.

### 8.4 Why `RUST_MIN_STACK=32MB` doesn't help

`RUST_MIN_STACK` controls **stack** size. The root cause is **heap** exhaustion. Increasing stack size has no effect on heap allocation failure.

### 8.5 Suggestions for upstream consideration

1. **Friendlier OOM diagnostics on Windows GNU**: currently `default_alloc_error_hook` prints a message, but `__fastfail(7)` immediately terminates the process before output may be flushed to stderr. Consider flushing stderr / writing to Windows event log before `__fastfail`, so users see "memory allocation of N bytes failed" instead of a cryptic `0xC0000409`.
2. **More conservative default `--jobs` on Windows GNU for clippy**: clippy's per-process memory footprint is larger than `rustc`'s (loads extra lint machinery + proc-macros). A default jobs cap based on available physical memory could prevent OOM.
3. **Documentation**: note in clippy/rustc docs that `STATUS_STACK_BUFFER_OVERRUN (0xC0000409)` on Windows may actually be an OOM abort via `__fastfail`, not a stack issue. The exception name is a frequent source of misdiagnosis(this reporter initially suspected `/GS` canary failure).

### 8.6 Artifacts available on request

- `clippy-driver.exe.24468.dmp`(WER minidump, 2.6MB)
- `Report.wer`(WER text report, 36KB)
- `clippy_crash_run.log`(full clippy stdout/stderr)
- objdump disassembly of `std-b0558c7fd7f3aef7.dll` around `0x171B1`
- Full root-cause analysis report(internal): `docs/dev/clippy_root_cause_analysis.md`

---

**Reporter**: NEXUS-OMEGA project(Chimera CLI)
**Internal tracking**: Week 8 limitations remediation, Task 1
**Related**: None found in existing rust-lang/rust-clippy issues search(keyword: `STATUS_STACK_BUFFER_OVERRUN clippy-driver windows gnu`)
