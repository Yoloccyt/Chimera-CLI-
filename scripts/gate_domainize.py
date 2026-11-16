#!/usr/bin/env python3
"""W-T0 门禁清单域化器 —— 为 gate_manifest.toml 注入 5 个调度字段。

WHY 存在（本脚本是一次性迁移工具，迁移后清单即权威源）：
  156 门清单此前**零** `group` / `needs` / `concurrency` / `resource` /
  `exclusive` 字段，而 govd 调度器（`scripts/govd/`）已具备
  `ThreadPoolExecutor` 并行、profile 档位、超时、进程树 kill、重试预算、
  journal hash chain 的完整能力 —— **调度器有能力，清单没喂料**（ADR-315 D1）。
  本脚本把门按职责域分桶并标注资源类，使调度器可据此做域内并发与资源互斥。

新增字段语义（ADR-315 D2）：
  group       职责域键（8 域）。域是「同类门共享资源特征」的分组单位，
              是并发上限的作用域。
  concurrency 该域内的并发上限。缺省 = 1（保守串行）；显式声明才允许并行。
              WHY 显式而非推导：门清单是**治理资产**，其并发上限应由人工按
              「该域内的门彼此是否共享 target-dir / 网络 / 同一文件」审慎设定，
              而非由调度器自动推导（自动推导会掩盖人工判断，且难以回溯）。
  resource    资源类（cpu / io / net）。标注该门主要消耗哪类宿主资源，
              供并发度推导公式使用（ADR-315 D3）。
  exclusive   互斥锁标识。**同一 `exclusive` 值的门必须串行执行**。
              WHY 不用 `needs` 表达互斥：清单头注已实测 `needs` **只买得到
              入列顺序，不是互斥锁**（`gate_manifest.toml:14-22`）——
              jobs>1 时执行环把 pending 整批 submit 进线程池，全程不读 needs。
              真正的互斥必须由调度器按 `exclusive` 键实现（ADR-315 D4）。
  needs       前置门 id 列表（跨域依赖）。**仅约束入列顺序**，
              与 `exclusive` 的互斥语义严格分离（两者不可互相替代）。

用法：
  python scripts/gate_domainize.py --emit      # 输出到 stdout（供 diff 审阅）
  python scripts/gate_domainize.py --apply     # 就地写回 gate_manifest.toml
  python scripts/gate_domainize.py --selftest  # 负控夹具：证明分类器有牙齿

WHY 幂等：脚本按门 id 幂等写入，重复执行不产生额外 diff。
"""
from __future__ import annotations

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "scripts" / "gate_manifest.toml"

# ---------------------------------------------------------------- 域定义

# 域 → (并发上限, 资源类)
# WHY 并发上限按域手工设定而非推导：
#   - `dep-layer` / `l0-contract` / `doc` / `lint-static`：纯静态读盘脚本，
#     彼此不共享写目标（台账冻结文件只被读），可安全并行。
#   - `tool-plane` / `governance`：可能写同一批台账文件（schema/parity 互相
#     读写的清单文件），保守给 4。
#   - `perf`：含 bench 采集，需与其它 IO 门错峰（磁盘 IO 争用会污染时间断言）。
#   - `security`：网络类（cargo audit / deny）走 crates.io，网络抖动多。
#   - `rust-build`：全部 cargo 类，共享 target-dir ⇒ `exclusive = "cargo"`，
#     组内恒串行。
DOMAIN_RULES: list[tuple[str, str, int, str]] = [
    # (域键, 匹配模式, concurrency, resource)
    ("rust-build", r"^cargo |verify_fmt_all_packages|check_repo_wiki_benchmark|check_cli_startup", 1, "cpu"),
    ("dep-layer", r"check_dependency_rules|check_layer_map_parity|check_dep_edge_freeze"
                  r"|check_crate_reachability|check_src_module_reachability"
                  r"|check_outer_inner_ratchet|check_dev_dep_direction"
                  r"|check_declared_dep_usage|check_manifest_path_deps"
                  r"|check_nexus_core_dep_freeze|check_compute_offload_reachability", 8, "io"),
    ("l0-contract", r"check_l0_surface|check_pub_api_leak|check_l0b_dep_surface"
                    r"|check_l0_sublayer|check_l0_root_surface|check_l0_purity"
                    r"|check_l0_logic_ledger|check_l0_const_ratchet"
                    r"|check_l0_pubfn_ratchet|check_l0_extensibility_class"
                    r"|check_l0a_sibling_assembly|check_contract_consumers"
                    r"|check_contract_zero_consumers|check_dead_contract_apis"
                    r"|check_immutable_surface_parity|check_formal_property_ledger"
                    r"|check_contract_consumers_parity", 8, "io"),
    ("doc", r"check_doc_|check_impl_maturity_claims|check_doc_symbol_claims"
            r"|xdoc_precise|emit_architecture_views|check_v340_crate_proposals"
            r"|check_deleted_crate_registry|check_retired_crate_residue", 8, "io"),
    ("lint-static", r"check_lint_ratchet|check_event_ownership|check_await_across_guard"
                   r"|check_lock_await|check_parallel_redlines|check_write_sensitive_instrumentation"
                   r"|check_ps1_bom_gate|check_ps1_syntax|check_workflow_yaml_lexical"
                   r"|check_sqlite_blocking|check_subscribe_order|check_bus_shard_hotspot"
                   r"|check_sync_offload_guard|check_accel_exclusion|check_topk_registry"
                   r"|check_positional_field_order_freeze|check_py_tooling_health"
                   r"|check_test_surface_liveness|check_entry_contract|check_fnlen|test_fnlen_smoke"
                   r"|test_impl_maturity_smoke|check_source_module_reachability", 8, "io"),
    ("tool-plane", r"check_tool_plane_contract|check_executor_side_effect_forwarding"
                   r"|check_composition_root|check_gated_wiring_substance"
                   r"|check_llm_loop_defaults|check_config_sample_parity"
                   r"|check_crate_contract|crate_contract|check_slash_vocab", 4, "io"),
    ("perf", r"check_perf_redlines|coverage_gate|check_certify_whitelist"
             r"|check_fuzz_config|check_ps1_syntax|bench_inventory", 4, "io"),
    ("security", r"audit_", 4, "net"),
    ("governance", r"check_manifest_schema|check_ci_manifest_parity|check_gates_provenance"
                   r"|audit_gate_parity|audit_gate_selfreport|check_gate_predicates_parity"
                   r"|check_version_sync|run_gate_manifest|govd"
                   r"|check_exclusive_mutex_negative", 4, "io"),
]

# 兜底域：未命中上述规则的归入 `misc`，保守串行（concurrency=1）。
FALLBACK_DOMAIN = ("misc", 1, "io")


def classify(cmd: str) -> tuple[str, int, str]:
    """按 cmd 文本判定 (group, concurrency, resource)。

    WHY 用脚本名正则而非 note/spec_ref 关键词：note 是自由散文（会漂），
    spec_ref 是工单号（与职责域无对应关系）；cmd 里的脚本名是**稳定标识**
    ——脚本改名必然同步改清单 cmd ⇒ 分类随之更新，不会静默失效。
    """
    for domain, pattern, concurrency, resource in DOMAIN_RULES:
        if re.search(pattern, cmd):
            return domain, concurrency, resource
    return FALLBACK_DOMAIN


def group_by(cmd: str) -> str:
    return classify(cmd)[0]


def resource_of(cmd: str) -> str:
    return classify(cmd)[2]


def concurrency_of(cmd: str) -> int:
    return classify(cmd)[1]


def exclusive_of(cmd: str) -> str:
    """互斥锁标识。

    WHY 只有 cargo 类门标 `exclusive = "cargo"`：
    cargo 的 target-dir 锁是**进程级文件锁**（后来者等待而非损坏），
    但并发 cargo 会在 target/ 留下同 crate 不同 hash 的 rlib，导致
    E0460/E0462/E0463/E0786 链接期元数据损坏（ADR-300 D4 纪律：
    「回归运行期禁止并发 cargo」）。故所有 cargo 类门必须串行。
    非 cargo 门共享 target-dir 的可能性为零 ⇒ 不标 exclusive（避免过度串行）。
    """
    return "cargo" if re.search(r"^cargo ", cmd) else ""


def needs_of(cmd: str, gates: list[dict]) -> list[str]:
    """前置门 id 列表 —— 当前恒为空。

    WHY 恒空而非填造：`needs` 只买得到入列顺序（清单头注 :14-22 实测），
    而现有 156 门的执行顺序由 ci.yml 的 step 顺序隐式承载，G-137 parity
    门据此对拍。**在 govd 接管 CI 执行面（W-T2）之前**，显式声明 needs
    反而会让 parity 门对拍两个不同的顺序来源。故留空并在此说明。

    重开条件：W-T2 把 govd 提为阻塞执行面、ci.yml 静态门迁出后，
    届时按「门 A 的判据依赖门 B 的产出」逐对显式声明。
    """
    del cmd, gates
    return []


# ---------------------------------------------------------------- 注入


def inject(gate: dict) -> dict:
    """返回带 5 个新字段的门副本（幂等）。"""
    cmd = gate["cmd"]
    group, concurrency, resource = classify(cmd)
    out = dict(gate)
    out["group"] = group
    out["concurrency"] = concurrency
    out["resource"] = resource
    out["exclusive"] = exclusive_of(cmd)
    out["needs"] = needs_of(cmd, [])
    return out


def _toml_str(value: str) -> str:
    """把任意文本渲染为合法 TOML 基本字符串。

    WHY 必须转义：门禁清单的 `note` / `spec_ref` / `cmd` 是**自由散文**，
    其中确实含双引号（如 `mel_spec="0.4"`）与反斜杠。未转义直接拼接会产出
    非法 TOML（`tomllib` 报 "Expected newline or end of document"），
    使整个清单不可解析 —— 这正是本函数存在的原因。

    转义序列按 TOML 规范：反斜杠 → \\\\，双引号 → \\"，换行 → \\n，
    制表 → \\t，回车 → \\r，其余控制字符走 \\uXXXX。
    """
    out = ['"']
    for ch in value:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append(f"\\u{ord(ch):04X}")
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def render(gate: dict) -> str:
    """把一个门渲染为 TOML 片段（字段顺序固定，便于 diff 审阅）。"""
    lines = ["[[gate]]"]
    lines.append(f"id = {_toml_str(gate['id'])}")
    lines.append(f"spec_ref = {_toml_str(gate['spec_ref'])}")
    lines.append(f"cmd = {_toml_str(gate['cmd'])}")
    lines.append(f"expect = {gate['expect']}")
    lines.append(f"kind = {_toml_str(gate['kind'])}")
    lines.append(f"test_kind = {_toml_str(gate['test_kind'])}")
    lines.append(f"group = {_toml_str(gate['group'])}")
    lines.append(f"concurrency = {gate['concurrency']}")
    lines.append(f"resource = {_toml_str(gate['resource'])}")
    if gate.get("exclusive"):
        lines.append(f"exclusive = {_toml_str(gate['exclusive'])}")
    if gate.get("needs"):
        lines.append("needs = [" + ", ".join(_toml_str(n) for n in gate["needs"]) + "]")
    lines.append(f"note = {_toml_str(gate['note'])}")
    for key in ("ci_exempt", "ci_advisory"):
        if gate.get(key):
            lines.append(f"{key} = {_toml_str(gate[key])}")
    if gate.get("timeout_s") is not None:
        lines.append(f'timeout_s = {gate["timeout_s"]}')
    if gate.get("disk_gb") is not None:
        lines.append(f'disk_gb = {gate["disk_gb"]}')
    return "\n".join(lines)


def selftest(gates: list[dict]) -> int:
    """负控夹具 —— 证明分类器有牙齿（非恒等映射）。"""
    failures: list[str] = []

    # 1) 分类器必须能把 cargo 门与非 cargo 门分开
    cargo_gate = next(g for g in gates if g["cmd"].startswith("cargo "))
    if exclusive_of(cargo_gate["cmd"]) != "cargo":
        failures.append("cargo 门未标 exclusive=cargo")
    if concurrency_of(cargo_gate["cmd"]) != 1:
        failures.append("cargo 门并发上限应为 1")

    # 2) 分类器必须对不同 cmd 给出不同域（防恒等映射）
    groups = {group_by(g["cmd"]) for g in gates}
    if len(groups) < 4:
        failures.append(f"域划分退化为常量（仅 {len(groups)} 个域）")

    # 3) 负控：未知脚本名必须落兜底域而非凭空归类
    unknown = classify("python scripts/this_gate_does_not_exist.py")
    if unknown[0] != FALLBACK_DOMAIN[0]:
        failures.append("未知 cmd 未落兜底域")

    # 4) 幂等：连续注入两次结果一致
    g = gates[0]
    if inject(inject(g)) != inject(g):
        failures.append("inject 非幂等")

    if failures:
        for f in failures:
            print(f"[SELFTEST][FAIL] {f}", file=sys.stderr)
        return 1
    print(
        f"[SELFTEST][OK] 域划分非退化（{len(groups)} 域）；"
        "cargo 门互斥生效；未知 cmd 落兜底；inject 幂等"
    )
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="门禁清单域化器（ADR-315 W-T0）")
    ap.add_argument("--emit", action="store_true", help="输出到 stdout")
    ap.add_argument("--apply", action="store_true", help="就地写回清单")
    ap.add_argument("--selftest", action="store_true", help="跑负控夹具")
    args = ap.parse_args(argv)

    import tomllib

    raw = MANIFEST.read_text(encoding="utf-8")
    gates = tomllib.loads(raw)["gate"]

    if args.selftest:
        return selftest(gates)
    if not (args.emit or args.apply):
        ap.print_help()
        return 2

    header = raw.split("[[gate]]", 1)[0]
    body = "\n\n".join(render(inject(g)) for g in gates)
    out = header + body + "\n"

    if args.emit:
        sys.stdout.write(out)
        return 0

    MANIFEST.write_text(out, encoding="utf-8", newline="\n")
    print(f"[OK] 门禁清单已域化：{len(gates)} 门 +5 字段 → {MANIFEST}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
