#!/usr/bin/env python3
"""Reproducible census of the Python / script governance surface (P1 evidence tool).

Why this file exists
--------------------
`gate_manifest.toml` is the single source of truth for *what is judged*, but the
repository has no reproducible answer to *what exists*: which scripts are
registered, which are only referenced by CI, which survive only as a citation in
a report, and which are invisible to every list. This tool derives that answer
from git + the filesystem, prints the sample size of every sweep, and writes a
machine-readable payload the audit report can cite.

It is a DIAGNOSTIC, never a verdict: 0 = payload produced, 2 = the census itself
is undecidable (git unavailable, manifest unreadable, ...). It prints no [FAIL]
label on purpose -- lifecycle decisions belong to `scripts/tool_registry.toml`
(I8), not to this reporter.

Usage
-----
    py -3.11 scripts/audit_python_surface.py
    py -3.11 scripts/audit_python_surface.py --json
    py -3.11 scripts/audit_python_surface.py --out tmp/audit/surface.json
    py -3.11 scripts/audit_python_surface.py --no-write

Exit codes: 0 produced / 2 undecidable.
"""
import argparse
import ast
import datetime
import json
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPTS = os.path.join(ROOT, "scripts")
MANIFEST = os.path.join(SCRIPTS, "gate_manifest.toml")
WORKFLOWS = os.path.join(ROOT, ".github", "workflows")
DOCS = os.path.join(ROOT, "docs")
AEGIS = os.path.join(ROOT, "Aegis")
DEFAULT_OUT = os.path.join(ROOT, "tmp", "audit", "python_surface.json")

REQUIRED = ("id", "spec_ref", "cmd", "expect", "kind", "note")
SCRIPT_TOKEN = re.compile(r"scripts/[A-Za-z0-9_.\-]+\.(?:py|ps1|sh)")
DOC_TOKEN = re.compile(r"[A-Za-z0-9_.\-/]+\.(?:py|ps1|sh)")
PY_CALL = re.compile(r"\bpython3?\b\s+\S+\.py")
PY_MODULE = re.compile(r"\bpython3?\b\s+-m\s+")
WRITE_HINT = re.compile(
    r"open\([^)]*[\"'][wa][\"']|write_text\(|write_bytes\(|os\.replace\("
    r"|shutil\.(?:copy|copy2|move|rmtree)\(")
SELFTEST_TOKEN = "self" "test"          # split so this file is not itself a gate candidate


def _git(args, cwd):
    """Run git read-only. Returns (ok, text); never raises into the census."""
    try:
        p = subprocess.run(["git"] + list(args), cwd=cwd, capture_output=True,
                           text=True, encoding="utf-8", errors="replace", timeout=120)
    except (OSError, subprocess.SubprocessError):
        return False, ""
    if p.returncode != 0:
        return False, p.stderr
    return True, p.stdout


def _tracked(cwd, patterns):
    ok, out = _git(["ls-files", "-z", "--"] + list(patterns), cwd)
    if not ok:
        return None
    return [p for p in out.split("\0") if p]


def _read(path):
    try:
        with open(path, encoding="utf-8-sig", errors="replace") as fh:
            return fh.read()
    except OSError:
        return ""


def _first_docstring(tree):
    try:
        doc = ast.get_docstring(tree) or ""
    except (TypeError, ValueError):
        return ""
    for line in doc.splitlines():
        line = line.strip()
        if line:
            return line[:180]
    return ""


def _module_imports(tree):
    names = set()
    for node in tree.body:
        if isinstance(node, ast.Import):
            for alias in node.names:
                names.add(alias.name.split(".")[0])
        elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
            names.add(node.module.split(".")[0])
    return names


def _is_local_module(name, path):
    """True if `name` resolves to something inside this repo.

    WHY not "stems in the same directory" (the first version): the census reported
    `gate_rc` and `scripts` as **third-party** because the importer lives one level
    below the module (`scripts/govd/__main__.py` importing `gate_rc`, and
    `scripts` as a namespace package). A claim of "stdlib-only" that depends on a
    broken detector is worse than no claim: it launders a real dependency in.
    """
    dirs = [os.path.dirname(path), SCRIPTS, ROOT]
    for d in dirs:
        if os.path.isfile(os.path.join(d, name + ".py")) or os.path.isdir(os.path.join(d, name)):
            return True
    return False


def _third_party(imports, path):
    stdlib = set(sys.stdlib_module_names) | {"__future__"}
    return sorted(n for n in imports
                  if n not in stdlib and not _is_local_module(n, path))


def _subprocess_calls(tree):
    calls, timed = 0, 0
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        fn = node.func
        if not (isinstance(fn, ast.Attribute) and isinstance(fn.value, ast.Name)):
            continue
        if fn.value.id != "subprocess" or fn.attr not in (
                "run", "call", "check_call", "check_output", "Popen"):
            continue
        calls += 1
        if any(kw.arg == "timeout" for kw in node.keywords):
            timed += 1
    return calls, timed


def _py_facts(path, rel):
    src = _read(path)
    tree = None
    try:
        tree = ast.parse(src)
    except SyntaxError:
        pass
    imports = _module_imports(tree) if tree is not None else set()
    calls, timed = _subprocess_calls(tree) if tree is not None else (0, 0)
    return {
        "path": rel,
        "kind": "py",
        "lines": src.count("\n") + (0 if src.endswith("\n") else 1) if src else 0,
        "doc": _first_docstring(tree) if tree is not None else "",
        "parsed": tree is not None,
        "has_main": "__name__" in src and "__main__" in src,
        "uses_gate_rc": "import gate_rc" in src,
        "uses_argparse": "import argparse" in src or "from argparse" in src,
        "selftest_token": SELFTEST_TOKEN in src,
        "imports": sorted(imports),
        "third_party": _third_party(imports, path),
        "subprocess_calls": calls,
        "subprocess_with_timeout": timed,
        "write_hints": len(WRITE_HINT.findall(src)),
    }


def _scan_docs():
    """One pass over docs/*.md, collecting referenced script basenames."""
    refs, files, nbytes, unreadable = set(), 0, 0, 0
    for base, _dirs, names in os.walk(DOCS):
        for name in names:
            if not name.endswith(".md"):
                continue
            path = os.path.join(base, name)
            try:
                size = os.path.getsize(path)
            except OSError:
                unreadable += 1
                continue
            if size > 2 * 1024 * 1024:      # a single oversized report is not a citation surface
                unreadable += 1
                continue
            text = _read(path)
            if not text:
                unreadable += 1
                continue
            files += 1
            nbytes += size
            for token in DOC_TOKEN.findall(text):
                refs.add(os.path.basename(token))
    return {"files": files, "bytes": nbytes, "unreadable": unreadable,
            "referenced_scripts": sorted(refs)}


def _scan_workflows():
    files, steps, modules, runner_refs, refs = 0, 0, 0, 0, set()
    if not os.path.isdir(WORKFLOWS):
        return None
    for name in sorted(os.listdir(WORKFLOWS)):
        if not name.endswith((".yml", ".yaml")):
            continue
        files += 1
        text = _read(os.path.join(WORKFLOWS, name))
        steps += len(PY_CALL.findall(text))
        modules += len(PY_MODULE.findall(text))
        runner_refs += text.count("run_gate_manifest")
        for token in SCRIPT_TOKEN.findall(text):
            refs.add(os.path.basename(token))
    return {"files": files, "python_script_steps": steps,
            "python_module_invocations": modules,
            "run_gate_manifest_refs": runner_refs,
            "referenced_scripts": sorted(refs)}


def _manifest_facts():
    import tomllib
    try:
        with open(MANIFEST, "rb") as fh:
            doc = tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        return None, "%s: %s" % (type(exc).__name__, exc)
    gates = doc.get("gate", [])
    by_cmd = {"python": 0, "bash": 0, "pwsh": 0, "other": 0}
    missing, ids, heavy, exempt = [], [], 0, 0
    for gate in gates:
        cmd = str(gate.get("cmd", ""))
        if cmd.startswith("{python}"):
            by_cmd["python"] += 1
        elif cmd.startswith("bash "):
            by_cmd["bash"] += 1
        elif cmd.startswith("pwsh "):
            by_cmd["pwsh"] += 1
        else:
            by_cmd["other"] += 1
        gaps = [k for k in REQUIRED if k not in gate]
        if gaps:
            missing.append({"id": gate.get("id", "?"), "missing": gaps})
        ids.append(str(gate.get("id", "")))
        if gate.get("kind") == "heavy":
            heavy += 1
        if "ci_exempt" in gate:
            exempt += 1
    nums = [int(m.group(1)) for i in ids for m in [re.match(r"G-(\d+)$", i)] if m]
    return {
        "total": len(gates),
        "by_cmd": by_cmd,
        "heavy": heavy,
        "ci_exempt": exempt,
        "missing_required": missing,
        "max_numeric_id": max(nums) if nums else None,
        "duplicate_ids": sorted({i for i in ids if ids.count(i) > 1}),
    }, None


def _aegis_facts():
    if not os.path.isdir(AEGIS):
        return None
    tracked = _tracked(AEGIS, ["*.py", "*.ps1", "*.sh"])
    if tracked is None:
        tracked = []
        for base, _dirs, names in os.walk(AEGIS):
            for name in names:
                if name.endswith((".py", ".ps1", ".sh")):
                    tracked.append(os.path.relpath(os.path.join(base, name), AEGIS).replace(os.sep, "/"))
    rows, groups, third = [], {}, set()
    ci_text = _read(os.path.join(AEGIS, ".github", "workflows", "ci.yml"))
    for rel in tracked:
        path = os.path.join(AEGIS, rel)
        ext = os.path.splitext(rel)[1]
        top = rel.split("/")[0]
        groups[top] = groups.get(top, 0) + 1
        if ext != ".py":
            rows.append({"path": rel, "kind": ext.lstrip("."),
                         "lines": _read(path).count("\n") + 1,
                         "doc": "", "has_main": False, "uses_argparse": False,
                         "selftest_token": False, "third_party": [],
                         "subprocess_calls": 0, "refs": {"ci": False},
                         "status_suggestion": "aegis-" + top})
            continue
        facts = _py_facts(path, rel)
        third.update(facts["third_party"])
        base = os.path.basename(rel)
        if base.startswith("test_"):
            suggested = "aegis-test"
        else:
            suggested = {"scripts": "aegis-script", "extensions": "aegis-extension",
                         "hooks": "aegis-hook", "skills": "aegis-skill",
                         "tests": "aegis-fixture"}.get(top, "aegis-" + top)
        rows.append({
            "path": rel, "kind": "py", "lines": facts["lines"],
            "doc": facts["doc"], "has_main": facts["has_main"],
            "uses_argparse": facts["uses_argparse"],
            "selftest_token": facts["selftest_token"],
            "third_party": facts["third_party"],
            "subprocess_calls": facts["subprocess_calls"],
            "refs": {"ci": base in ci_text},
            "status_suggestion": suggested,
        })
    ok, head = _git(["rev-parse", "--short", "HEAD"], AEGIS)
    return {"head": head.strip() if ok else None, "files": len(rows),
            "groups": groups, "third_party": sorted(third), "rows": rows}


def _git_facts():
    ok_head, head = _git(["rev-parse", "--short", "HEAD"], ROOT)
    ok_br, branch = _git(["rev-parse", "--abbrev-ref", "HEAD"], ROOT)
    ok_st, status = _git(["status", "--porcelain"], ROOT)
    dirty = len([l for l in status.splitlines() if l.strip()]) if ok_st else None
    return {"head": head.strip() if ok_head else None,
            "branch": branch.strip() if ok_br else None,
            "dirty_entries": dirty}


def _ownership(rows, manifest_text, workflow_text, docs_refs):
    for row in rows:
        name = os.path.basename(row["path"])
        row["refs"] = {
            "manifest": name in manifest_text,
            "ci": name in workflow_text,
            "docs": name in docs_refs,
        }
        if row["refs"]["manifest"]:
            row["status_suggestion"] = "gate-registered"
        elif row["refs"]["ci"]:
            row["status_suggestion"] = "ci-only"
        elif row["refs"]["docs"]:
            row["status_suggestion"] = "evidence-or-diagnostic"
        else:
            row["status_suggestion"] = "unaccounted"


def build():
    git = _git_facts()
    manifest, err = _manifest_facts()
    if err:
        return None, "gate_manifest unreadable: %s" % err
    workflows = _scan_workflows()
    if workflows is None:
        return None, "workflow dir missing: %s" % WORKFLOWS
    docs = _scan_docs()

    repo_scripts = _tracked(ROOT, ["scripts/*.py", "scripts/*.ps1", "scripts/*.sh",
                                   "install.ps1", "install.sh",
                                   "test_version_verification.ps1"])
    if repo_scripts is None:
        return None, "git ls-files unavailable -- refusing a blind census"

    rows = []
    for rel in repo_scripts:
        path = os.path.join(ROOT, rel)
        if rel.endswith(".py"):
            rows.append(_py_facts(path, rel))
        else:
            rows.append({"path": rel, "kind": os.path.splitext(rel)[1].lstrip("."),
                         "lines": _read(path).count("\n") + 1})

    manifest_text = _read(MANIFEST)
    workflow_text = "\n".join(_read(os.path.join(WORKFLOWS, n))
                              for n in os.listdir(WORKFLOWS) if n.endswith((".yml", ".yaml")))
    _ownership(rows, manifest_text, workflow_text, set(docs["referenced_scripts"]))

    py_rows = [r for r in rows if r["kind"] == "py"]
    payload = {
        "schema": "python-surface/1",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "python": sys.version.split()[0],
        "disk_free_gb": round(shutil.disk_usage(ROOT).free / 1024 ** 3, 3),
        "git": git,
        "counts": {
            "repo_scripts": len(rows),
            "repo_py": len(py_rows),
            "repo_ps1": len([r for r in rows if r["kind"] == "ps1"]),
            "repo_sh": len([r for r in rows if r["kind"] == "sh"]),
            "gate_registered": len([r for r in rows if r["status_suggestion"] == "gate-registered"]),
            "ci_only": len([r for r in rows if r["status_suggestion"] == "ci-only"]),
            "evidence_or_diagnostic": len([r for r in rows if r["status_suggestion"] == "evidence-or-diagnostic"]),
            "unaccounted": len([r for r in rows if r["status_suggestion"] == "unaccounted"]),
            "gate_rc": len([r for r in py_rows if r["uses_gate_rc"]]),
            "argparse": len([r for r in py_rows if r["uses_argparse"]]),
            "third_party_importers": len([r for r in py_rows if r["third_party"]]),
            "subprocess_calls": sum(r["subprocess_calls"] for r in py_rows),
            "subprocess_with_timeout": sum(r["subprocess_with_timeout"] for r in py_rows),
            "write_capable": len([r for r in py_rows if r["write_hints"]]),
        },
        "gates": manifest,
        "workflows": workflows,
        "docs": {"files": docs["files"], "bytes": docs["bytes"],
                 "unreadable": docs["unreadable"],
                 "referenced_scripts": len(docs["referenced_scripts"])},
        "aegis": _aegis_facts(),
        "files": rows,
    }
    return payload, None


def _cell(text, limit=58):
    """Markdown-safe cell: pipes become slashes, newlines collapse, long text clips."""
    s = str(text).replace("|", "/").replace("\n", " ").strip()
    return s if len(s) <= limit else s[:limit - 1] + "…"


HEADER = ("| 路径 | 行数 | 用途（docstring 首行） | 入口 | 第三方依赖 | 自检 "
          "| 引用(M=门/C=CI/D=文档) | 建议状态 |\n|---|---|---|---|---|---|---|---|")


def _rows_table(rows):
    out = [HEADER]
    for r in rows:
        refs = r.get("refs", {})
        flags = "".join(t for t, ok in (("M", refs.get("manifest")),
                                       ("C", refs.get("ci")),
                                       ("D", refs.get("docs"))) if ok)
        entry = ("-" if r.get("kind") != "py"
                 else ("argparse" if r.get("uses_argparse")
                       else ("__main__" if r.get("has_main") else "库/无入口")))
        out.append("| %s | %s | %s | %s | %s | %s | %s | %s |" % (
            _cell(r["path"], 60), r.get("lines", ""), _cell(r.get("doc", "")),
            entry, _cell(",".join(r.get("third_party", [])) or "-", 20),
            "有" if r.get("selftest_token") else "无",
            flags or "-", _cell(r.get("status_suggestion", ""), 24)))
    return out


def render_markdown(payload):
    """Full per-file census (Spec §4 P1.1: 路径/用途/入口/依赖/调用方/测试/引用/状态).

    The point of emitting this from the tool instead of typing it: every row is a
    measurement with a sample size, and the next run regenerates it. A hand-kept
    inventory is exactly the artefact that rots first.
    """
    aegis = payload.get("aegis") or {}
    lines = [
        "# 01b 全量脚本清单（工具生成，勿手改）",
        "",
        "- 生成器：`py -3.11 scripts/audit_python_surface.py --markdown-out <本文件路径>`"
        "（`--markdown` 同内容写 stdout；本文件即其输出，手改会被下次生成覆盖）",
        "- 取数时点：%s；父仓 HEAD `%s`（分支 `%s`）；解释器 %s；D 盘空闲 %s GiB" % (
            payload["generated_at"], payload["git"]["head"], payload["git"]["branch"],
            payload["python"], payload["disk_free_gb"]),
        "- 样本量：仓库脚本 %d 个（py %d / ps1 %d / sh %d），Aegis 文件 %d 个，门 %d 条，"
        "文档 %d 份（引用脚本名 %d 个）" % (
            payload["counts"]["repo_scripts"], payload["counts"]["repo_py"],
            payload["counts"]["repo_ps1"], payload["counts"]["repo_sh"],
            aegis.get("files", 0), payload["gates"]["total"],
            payload["docs"]["files"], payload["docs"]["referenced_scripts"]),
        "- 说明：`建议状态` 是**机械推论**（manifest/CI/文档引用三个信号），不是最终裁决；"
        "最终归属以 `scripts/tool_registry.toml` 与 `gate_manifest.toml` 为准。",
        "",
        "## 1. 仓库 Python（%d）" % payload["counts"]["repo_py"],
        "",
    ] + _rows_table([r for r in payload["files"] if r["kind"] == "py"]) + [
        "",
        "## 2. 仓库 shell / PowerShell（py 之外 %d）" % (
            payload["counts"]["repo_ps1"] + payload["counts"]["repo_sh"]),
        "",
    ] + _rows_table([r for r in payload["files"] if r["kind"] != "py"]) + [
        "",
        "## 3. Aegis 子仓（%d 个文件，其中 py %d）" % (
            aegis.get("files", 0),
            sum(1 for r in aegis.get("rows", []) if r["kind"] == "py")),
        "",
        "- gitlink head：`%s`；第三方依赖集合：%s" % (
            aegis.get("head"), ", ".join(aegis.get("third_party") or []) or "无"),
        "",
    ] + _rows_table(aegis.get("rows", [])) + [""]
    return "\n".join(lines) + "\n"


def print_summary(payload):
    c, g, w, d = payload["counts"], payload["gates"], payload["workflows"], payload["docs"]
    print("[surface] py=%d ps1=%d sh=%d (repo) | aegis_py=%d"
          % (c["repo_py"], c["repo_ps1"], c["repo_sh"],
             len([r for r in (payload["aegis"] or {}).get("rows", []) if r["kind"] == "py"])))
    print("[gates] total=%d python=%d bash=%d pwsh=%d other=%d heavy=%d ci_exempt=%d max=G-%s"
          % (g["total"], g["by_cmd"]["python"], g["by_cmd"]["bash"], g["by_cmd"]["pwsh"],
             g["by_cmd"]["other"], g["heavy"], g["ci_exempt"], g["max_numeric_id"]))
    print("[ci] workflows=%d python_script_steps=%d module_invocations=%d runner_refs=%d"
          % (w["files"], w["python_script_steps"], w["python_module_invocations"],
             w["run_gate_manifest_refs"]))
    print("[ownership] gate=%d ci_only=%d evidence=%d unaccounted=%d"
          % (c["gate_registered"], c["ci_only"], c["evidence_or_diagnostic"], c["unaccounted"]))
    print("[contract] gate_rc=%d argparse=%d third_party_importers=%d"
          % (c["gate_rc"], c["argparse"], c["third_party_importers"]))
    print("[subprocess] calls=%d with_timeout=%d | write_capable=%d"
          % (c["subprocess_calls"], c["subprocess_with_timeout"], c["write_capable"]))
    print("[docs] md_files=%d bytes=%d unreadable=%d referenced_scripts=%d"
          % (d["files"], d["bytes"], d["unreadable"], d["referenced_scripts"]))
    if payload["aegis"]:
        a = payload["aegis"]
        print("[aegis] head=%s files=%d groups=%s" % (a["head"], a["files"], a["groups"]))
    print("[git] head=%s branch=%s dirty=%s | python=%s | free=%.3f GiB"
          % (payload["git"]["head"], payload["git"]["branch"],
             payload["git"]["dirty_entries"], payload["python"], payload["disk_free_gb"]))


def main(argv):
    parser = argparse.ArgumentParser(description="Reproducible Python/script surface census")
    parser.add_argument("--out", default=DEFAULT_OUT, help="JSON payload path")
    parser.add_argument("--json", action="store_true", help="also print the payload")
    parser.add_argument("--no-write", action="store_true", help="do not write the payload")
    parser.add_argument("--markdown", action="store_true",
                        help="print the full per-file census as markdown instead of the summary")
    parser.add_argument("--markdown-out", default="",
                        help="write the census to this path (UTF-8, LF) instead of stdout")
    args = parser.parse_args(argv)

    payload, err = build()
    if payload is None:
        print("[UNDECIDABLE] %s" % err)
        return 2
    if args.markdown_out:
        # Writing from inside the process, not via shell redirection: a PowerShell
        # pipeline decodes native stdout with the console codepage first, which on a
        # GBK host turns CJK cells into mojibake before anything is written.
        with open(args.markdown_out, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(render_markdown(payload))
        print("[out] %s" % os.path.relpath(args.markdown_out, ROOT))
        return 0
    if args.markdown:
        # Census mode is a pure render: no summary noise, no payload file, so the
        # output can be redirected straight into the report directory.
        sys.stdout.write(render_markdown(payload))
        return 0
    print_summary(payload)
    if not args.no_write:
        folder = os.path.dirname(os.path.abspath(args.out))
        os.makedirs(folder, exist_ok=True)
        with open(args.out, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(payload, fh, ensure_ascii=False, indent=1)
            fh.write("\n")
        print("[out] %s" % os.path.relpath(args.out, ROOT))
    if args.json:
        print(json.dumps(payload, ensure_ascii=False, indent=1))
    return 0


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
