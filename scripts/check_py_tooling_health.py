#!/usr/bin/env python3
# -*- coding: ascii -*-
"""Python governance-tooling health gate (G-121, G-122).

Why this gate exists
--------------------
The repository carries 120 machine gates that judge the Rust code, and it pins
the Rust toolchain for exactly four CI jobs because `-D warnings` was once turned
red by a floating stable picking up a new clippy lint. The Python side -- which is
where those gates are *implemented* (100+ files under scripts/ scanned recursively; the number
actually invoked from workflows is reported by check_ci_manifest_parity.py as ci_steps,
not copied here) -- had no equivalent judgement at all: `grep -riE
"ruff|flake8|black|mypy|pylint|pyright"` over .github/workflows and
scripts/*.toml returns zero hits. So the tooling that decides whether the code is
healthy was itself the least healthy part of the tree.

That asymmetry already cost real defects, measured on 2026-10-16:
  * 8 module-level imports were dead in 7 gate scripts and nothing noticed.
  * 3 governance shell scripts materialized as CRLF, so the documented local
    command `bash scripts/check_doc_drift.sh` died on `$'\\r': command not found`
    while the same gate passed in CI -- a local-only failure CI cannot see.
  * The interpreter running the gates floated with the runner image while
    `run_gate_manifest.py` needs tomllib (3.11+).

What it judges (all five are currently satisfied, so the gate ships with zero
exemptions -- the cleanest possible admission state)
---------------------------------------------------
  H1 COMPILE     every scripts/*.py must compile.
  H2 DEADIMPORT  no unused module-level import. An intentional side-effect import
                 must carry `# side-effect-import: <reason >= 24 chars>` on its
                 line; a bare marker is not enough, same discipline as G-99's
                 exemption ledger.
  H3 REPOEOL     `git ls-files --eol scripts/` must contain no i/crlf and no
                 i/mixed. This judges the blob/CI view, which is the one that
                 matters; the working tree is .gitattributes' business.
  H4 FLOOR       refuse to judge on an interpreter below PYTHON_FLOOR (rc 2).
                 Silently "passing" on an interpreter the gates never target is a
                 false green.
  H5 PINPARITY   every job whose steps invoke python3 must also pin it, and every
                 pin must equal PYTHON_FLOOR. This is the check whose absence
                 produced the gap this commit closes: adding a python step to a
                 new job silently inherits the runner image's interpreter.
  H6 ABSPATH     no absolute path in a module-level assignment value. A measured
                 defect: `classify_fmt_vs_semantic.py` pinned `D:\Chimera CLI`, so
                 the number a cited report attributes to it was only reproducible
                 on one box. Guarded host paths inside functions (`os.path.isdir`,
                 `os.name == 'nt'`) and selftest fixtures are out of scope by
                 construction -- scanning all constants instead yielded 14 false
                 positives on fixture keys like `m::alpha`.
  H7 ENVFAIL     `if r.returncode != 0` must not lead to `sys.exit(1)`. Measured
                 defect in lru_facts.py / lru_reachability.py: an environment
                 failure recorded as a red verdict is the false-green channel
                 gate_rc's F33 note documents.
  H8 EXCEPTRED   an `except` block must not end in a red verdict either. Same
                 class as H7, arrived through the exception path: four registered
                 gates (check_accel_exclusion, check_gate_predicates_parity,
                 check_layer_map_parity, audit_phaseR_spec) printed FAIL and
                 returned 1 when they could not read or parse their input. On a
                 gate with expect=1 that is a silent false green; on expect=0 it
                 files a broken gate as a repository defect. Bool returns are not
                 red codes (True == 1) -- see the fixture below.

Stdlib only, on purpose: the pinned CI interpreter has no site-packages, so a
`import yaml` here would pass locally (dev boxes usually have PyYAML) and fail in
CI -- the exact "works here, not there" class this gate exists to prevent.

Modes / exit codes
------------------
  (default)  judge the real repo        0 = clean, 1 = violations found
  --selftest planted fixtures prove every judgement bites in both directions
                                     0 = all assertions held, 1 = not
  undecidable                                  2 = git missing, blind scan,
                                          interpreter below floor, bad fixture path

Output is pure ASCII (project script convention; avoids Windows GBK console traps).
"""

import ast
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
WORKFLOWS = os.path.join(ROOT, ".github", "workflows")

PYTHON_FLOOR = (3, 11)
FLOOR_TEXT = "%d.%d" % PYTHON_FLOOR

# Below these the scan is presumed blind, not clean. Measured live: 91 scripts,
# 12 workflows; floors sit comfortably under that but far above zero.
PY_FILES_FLOOR = 80
WORKFLOW_FLOOR = 8

SIDE_EFFECT_RE = re.compile(r"#\s*side-effect-import:\s*(.+)$")
MIN_REASON_LEN = 24
IDENT_RE = re.compile(r"[A-Za-z_]\w*")
IMPORT_LINE_RE = re.compile(r"^\s*(?:from\s+\S+\s+)?import\s+")
SETUP_PY_RE = re.compile(r"uses:\s*actions/setup-python@")
PYVER_RE = re.compile(r"python-version:\s*['\"]?(\d+)\.(\d+)")
JOB_RE = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")


def _stdlib_names():
    """Names that are never third-party (kept for the H2 comment's reasoning)."""
    return set(sys.stdlib_module_names) | {"__future__"}


def scan_py_files(root=HERE):
    """Recursive walk: scripts/govd/** used to escape H1/H2/H6/H7 (PYG-F06)."""
    found = []
    for base, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d != "__pycache__")
        for name in sorted(files):
            if name.endswith(".py"):
                found.append(os.path.join(base, name))
    return sorted(found)


def compile_all(py_files):
    """(violations, scanned). Violation = (file, 'COMPILE', message)."""
    bad = []
    for p in py_files:
        with open(p, encoding="utf-8", errors="replace") as fh:
            src = fh.read()
        try:
            compile(src, p, "exec")
        except SyntaxError as exc:
            bad.append((os.path.basename(p), "COMPILE",
                        "line %s: %s" % (exc.lineno, exc.msg)))
    return bad, len(py_files)


def dead_imports(py_files):
    """(violations, total_imports). Violation = (file, 'DEADIMPORT', 'line: name')."""
    bad = []
    total = 0
    for p in py_files:
        with open(p, encoding="utf-8", errors="replace") as fh:
            lines = fh.read().splitlines()
        try:
            tree = ast.parse("\n".join(lines))
        except SyntaxError:
            continue                     # already reported by H1
        used = set(n.id for n in ast.walk(tree) if isinstance(n, ast.Name))
        for node in tree.body:           # module level only, by construction
            names = []
            if isinstance(node, ast.Import):
                names = [(a.asname or a.name.split(".")[0]) for a in node.names]
            elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
                if node.module == "__future__":
                    # `from __future__ import annotations` is a compiler directive:
                    # the bound name is legitimately never referenced. Flagging it
                    # was a real false positive in the first draft of this gate --
                    # 8 files went red for a directive that does exactly what it says.
                    continue
                names = [(a.asname or a.name) for a in node.names if a.name != "*"]
            for nm in names:
                total += 1
                if nm in used:
                    continue
                line = lines[node.lineno - 1] if node.lineno - 1 < len(lines) else ""
                m = SIDE_EFFECT_RE.search(line)
                if m and len(m.group(1).strip()) >= MIN_REASON_LEN:
                    continue             # signed exemption
                if m:                    # marker present, reason too short
                    bad.append((os.path.basename(p), "DEADIMPORT",
                                "line %d: %s -- exemption marker present but its "
                                "reason is under %d chars" % (node.lineno, nm,
                                                              MIN_REASON_LEN)))
                    continue
                bad.append((os.path.basename(p), "DEADIMPORT",
                            "line %d: %s is imported at module level and never "
                            "referenced" % (node.lineno, nm)))
    return bad, total


def repo_eol(sub_path="scripts"):
    """(violations, scanned) from `git ls-files --eol`. Returns (None, None) if git is absent."""
    try:
        p = subprocess.run(["git", "ls-files", "--eol", sub_path],
                           cwd=ROOT, capture_output=True, text=True)
    except (OSError, ValueError):
        return None, None
    if p.returncode != 0:
        return None, None
    bad = []
    rows = [l for l in p.stdout.splitlines() if l.strip()]
    for line in rows:
        cols = line.split()
        if len(cols) < 2:
            continue
        if _eol_row_is_bad(cols[0]):
            bad.append((cols[-1], "REPOEOL",
                        "%s -- the blob itself carries CRLF, so CI checks it out "
                        "that way; convert the file, do not paper over it" % cols[0]))
    return bad, len(rows)


def _invokes_python(body):
    """True if the job body runs python3, ignoring comments.

    Comment-blindness matters both ways: a WHY comment that merely mentions
    python3 would otherwise demand a pin from a job that never calls it, and the
    original `python3 ` substring test would also miss `python scripts/x.py`.
    """
    for line in body.splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        if (re.search(r"\bpython3?\b\s+\S*\.py", s)
                or re.search(r"\bpython3?\b\s+-m\b", s)
                or re.match(r"^python3?\b", s)):
            return True
    return False


def job_blocks(text):
    """Split a workflow into (job_name, body) pairs by 2-space-indented keys.

    Deliberately textual, not YAML-parsed: the pinned CI interpreter has no
    PyYAML, and guessing a YAML parser is how a gate starts lying about jobs.
    Anchoring on `^  name:` under `jobs:` is enough for the shape of a job
    boundary, and the selftest pins both the positive and the negative case.
    """
    lines = text.splitlines()
    jobs = []
    cur_name = None
    cur = []
    in_jobs = False
    for line in lines:
        if re.match(r"^jobs:\s*$", line):
            in_jobs = True
            continue
        if not in_jobs:
            continue
        if re.match(r"^[A-Za-z0-9_-]+:\s*$", line):      # dedented to column 0
            break
        m = JOB_RE.match(line)
        if m:
            if cur_name:
                jobs.append((cur_name, "\n".join(cur)))
            cur_name, cur = m.group(1), []
            continue
        if line.startswith(("  ", "    ", "\t")) or not line.strip():
            cur.append(line)
    if cur_name:
        jobs.append((cur_name, "\n".join(cur)))
    return jobs


def pin_parity(workflow_dir):
    """(violations, workflows_scanned, jobs_with_python)."""
    bad = []
    files = sorted(f for f in os.listdir(workflow_dir)
                   if f.endswith((".yml", ".yaml")))
    npython = 0
    for f in files:
        with open(os.path.join(workflow_dir, f), encoding="utf-8",
                  errors="replace") as fh:
            text = fh.read()
        for jn, body in job_blocks(text):
            if not _invokes_python(body):
                continue
            npython += 1
            pins = PYVER_RE.findall(body) if SETUP_PY_RE.search(body) else []
            if not pins:
                bad.append((f, "PINPARITY",
                            "job %r invokes python3 but pins no interpreter -- it "
                            "silently inherits the runner image's version" % jn))
                continue
            for major, minor in pins:
                if (int(major), int(minor)) != PYTHON_FLOOR:
                    bad.append((f, "PINPARITY",
                                "job %r pins %s.%s but the verified floor is %s -- "
                                "one interpreter per repo, or the gates disagree"
                                % (jn, major, minor, FLOOR_TEXT)))
    return bad, len(files), npython


def _parse(path):
    with open(path, encoding="utf-8", errors="replace") as fh:
        src = fh.read()
    try:
        return ast.parse(src)
    except SyntaxError:
        return None


ABS_HINTS = ("/home/", "/Users/", "/usr/bin/", "/opt/")


def _looks_absolute(s):
    """Drive letter FOLLOWED BY a separator, or a POSIX root that only exists on a box.

    The separator is checked by indexing, not by a `[\\/]` character class: such a
    class silently degrades to `[\/]` (which matches only `/`) once the file
    round-trips through an editor, and that made the first draft of this axis blind
    to the single real `D:\\...` it was written to catch -- it reported "0 hits" on a
    tree that had one. The planted positive fixture below IS that bug.
    """
    if not isinstance(s, str) or len(s) < 3:
        return False
    if s[1] == ":" and s[0].isalpha() and s[2] in ("/", "\\"):
        return True
    for h in ABS_HINTS:
        # A bare prefix constant (`"/opt/"`) is a MATCHER, not a pinned default --
        # this gate's own allowlist would otherwise trip H6 on itself. Only a path
        # with something after the prefix (`/home/bob/proj`) is machine-specific.
        if s.startswith(h) and len(s) > len(h):
            return True
    return False


def abs_module_paths(py_files):
    """H6: an absolute path at module level is a machine-specific default.

    Only the VALUE of a module-level assignment is scanned, not the whole tree.
    The three legitimate host-specific strings in this repo live inside functions
    behind `os.path.isdir` / `os.name == 'nt'` guards, or are selftest fixtures that
    deliberately assert a Windows path parses. Widening the scan to every constant
    produced 14 false positives on fixture keys like `m::alpha` before the separator
    rule was added -- a gate that cries wolf gets its findings ignored.
    """
    bad = []
    for p in py_files:
        tree = _parse(p)
        if tree is None:
            continue
        for n in tree.body:
            if isinstance(n, (ast.Assign, ast.AnnAssign)):
                for sub in ast.walk(n.value):
                    if isinstance(sub, ast.Constant) and _looks_absolute(sub.value):
                        bad.append((os.path.basename(p), "ABSPATH",
                                    "line %d: %r at module level pins one machine -- "
                                    "derive from __file__ or an env var, or every number "
                                    "this script prints is only reproducible here"
                                    % (sub.lineno, sub.value[:40])))
    return bad


def envfail_exit1(py_files):
    """H7: a failed external command must exit 2 (undecidable), never 1 (red).

    `run_gate_manifest.py` decides with `ok = rc == expect`, so a script that exits
    1 on an environment failure is recorded as "ran and judged red" -- the ledger
    then treats a missing toolchain as a defect to be repaid. gate_rc's F33 note
    documents this exact false-green channel; H7 catches the hand-written variant.
    """
    bad = []
    for p in py_files:
        tree = _parse(p)
        if tree is None:
            continue
        for n in ast.walk(tree):
            if not isinstance(n, ast.If) or not isinstance(n.test, ast.Compare):
                continue
            left = n.test.left
            is_rc = isinstance(left, ast.Attribute) and left.attr == "returncode"
            if not (is_rc and any(isinstance(c, ast.Constant) and c.value == 0
                                  for c in n.test.comparators)):
                continue
            for sub in ast.walk(n):
                if (isinstance(sub, ast.Call)
                        and getattr(sub.func, "attr", None) == "exit"
                        and getattr(getattr(sub.func, "value", None), "id", None) == "sys"
                        and sub.args and isinstance(sub.args[0], ast.Constant)
                        and sub.args[0].value == 1):
                    bad.append((os.path.basename(p), "ENVFAIL",
                                "line %d: `returncode != 0` leads to sys.exit(1) -- no "
                                "verdict was reached, this must be exit 2 (gate_rc F33)"
                                % sub.lineno))
    return bad


def except_handler_red(py_files):
    """H8: an `except` block that ends in a red verdict is a swallowed non-verdict.

    H7 covers the hand-written `returncode != 0` shape; this is the other half of
    the same class -- the gate could not read or parse its input, printed FAIL and
    returned 1. Under `ok = rc == expect` that is booked as "ran and judged red"
    (and on a gate with expect=1 it becomes a silent false green), so a broken
    gate is chased as a repository defect. Measured 2026-10-29: four registered
    gates did exactly this; all of them now exit 2.
    """
    bad = []
    for p in py_files:
        tree = _parse(p)
        if tree is None:
            continue
        for n in ast.walk(tree):
            if not isinstance(n, ast.ExceptHandler):
                continue
            for s in ast.walk(n):
                # `not isinstance(..., bool)`: in Python `True == 1`, and the repo's
                # gates carry `_raises(fn)` selftest probes that return True from an
                # except block. The first draft of H8 flagged those two probes as
                # red-exits -- a gate that cries wolf gets its findings ignored.
                def _is_red_const(node_value):
                    return (isinstance(node_value, ast.Constant)
                            and node_value.value == 1
                            and not isinstance(node_value.value, bool))
                red_return = isinstance(s, ast.Return) and _is_red_const(s.value)
                red_exit = (isinstance(s, ast.Call)
                            and getattr(s.func, "attr", None) == "exit"
                            and getattr(getattr(s.func, "value", None), "id", None) == "sys"
                            and s.args and _is_red_const(s.args[0]))
                if red_return or red_exit:
                    bad.append((os.path.basename(p), "EXCEPTRED",
                                "line %d: exception handler exits 1 -- no verdict was "
                                "reached, this must be exit 2 (undecidable)"
                                % getattr(s, "lineno", n.lineno)))
                    break
    return bad


def judge(py_files, workflow_dir, sub_path="scripts"):
    out = []
    fails = 0
    if len(py_files) < PY_FILES_FLOOR:
        out.append("[TOOL-ERROR] found only %d scripts/*.py (floor %d) -- scan is "
                   "blind, refusing to judge" % (len(py_files), PY_FILES_FLOOR))
        return 2, out
    bad1, nscanned = compile_all(py_files)
    bad2, nimports = dead_imports(py_files)
    bad3, nrows = repo_eol(sub_path)
    if bad3 is None:
        out.append("[TOOL-ERROR] `git ls-files --eol` unavailable -> H3 undecidable, "
                   "refusing to judge")
        return 2, out
    bad4, nwf, npyj = pin_parity(workflow_dir)
    if nwf < WORKFLOW_FLOOR:
        out.append("[TOOL-ERROR] found only %d workflows (floor %d) -- H5 scan is "
                   "blind, refusing to judge" % (nwf, WORKFLOW_FLOOR))
        return 2, out
    bad5 = abs_module_paths(py_files)
    bad6 = envfail_exit1(py_files)
    bad7 = except_handler_red(py_files)
    for group, kind in ((bad1, "COMPILE"), (bad2, "DEADIMPORT"),
                        (bad3, "REPOEOL"), (bad4, "PINPARITY"),
                        (bad5, "ABSPATH"), (bad6, "ENVFAIL"),
                        (bad7, "EXCEPTRED")):
        for f, k, msg in group:
            out.append("[GAP][%s] %s: %s" % (k, f, msg))
        fails += len(group)
    stats = {"py_files": nscanned, "imports": nimports, "eol_rows": nrows,
             "workflows": nwf, "python_jobs": npyj}
    if fails:
        out.append("Python tooling health BROKEN: %d violation(s) -- %s"
                   % (fails, " ".join("%s=%d" % (k, v) for k, v in sorted(stats.items()))))
        return 1, out
    out.append("[OK] Python tooling healthy: %s"
               % " ".join("%s=%d" % (k, v) for k, v in sorted(stats.items())))
    return 0, out


def run_real():
    if sys.version_info < PYTHON_FLOOR:
        print("[TOOL-ERROR] running on %d.%d, below the verified floor %s -- gates "
              "here target tomllib and must not be judged by an older interpreter"
              % (sys.version_info[0], sys.version_info[1], FLOOR_TEXT))
        return 2
    py_files = scan_py_files()
    if not os.path.isdir(WORKFLOWS):
        print("[TOOL-ERROR] workflows dir not found: %s" % WORKFLOWS)
        return 2
    rc, out = judge(py_files, WORKFLOWS)
    for line in out:
        print(line)
    return rc


# --- selftest ---------------------------------------------------------------

DEAD_SRC = "import os\nimport json\n\ndef f(p):\n    return os.path.basename(p)\n"

EXEMPT_SRC = ("import ctypes  # side-effect-import: registers the allocator hook "
              "the gate relies on at import time\n")

TOO_SHORT_SRC = ("import ctypes  # side-effect-import: too short\n")

FUTURE_SRC = "from __future__ import annotations\n\nimport os\nprint(os.getcwd())\n"

USED_SRC = "import os\nimport sys\nprint(os.getcwd(), sys.argv)\n"

BROKEN_SRC = "def f(:\n    pass\n"

WF_UNPINNED = """name: t
on: push
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Gate
        run: python3 scripts/whatever.py
"""

WF_PINNED = """name: t
on: push
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: '3.11'
      - name: Gate
        run: python3 scripts/whatever.py
"""

WF_WRONGPIN = WF_PINNED.replace("3.11", "3.9")

WF_NOPYTHON = """name: t
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Cargo
        run: cargo build
"""


ABS_FIX = ('ROOT = "D:\\\\Chimera CLI"\nHOME = "/home/bob/proj"\nprint(ROOT, HOME)\n')
ABS_OK = ('import os\nROOT = os.path.dirname(os.path.abspath(__file__))\n'
          'if os.name == "nt":\n    p = r"C:\\Program Files\\x"\nprint(ROOT, p)\n')
ENVFIX = ('import subprocess, sys\nr = subprocess.run(["cargo", "metadata"])\n'
          'if r.returncode != 0:\n    sys.exit(1)\nprint("ok")\n')
ENVOk = ('import subprocess, sys\nr = subprocess.run(["cargo", "metadata"])\n'
         'if r.returncode != 0:\n    sys.exit(2)\nprint("ok")\n')
EXCFIX = ('import sys\ndef main():\n    try:\n        open("x")\n'
          '    except OSError:\n        print("[FAIL] cannot read")\n        return 1\n'
          '    return 0\n')
EXCOK = ('import sys\ndef main():\n    try:\n        open("x")\n'
         '    except OSError:\n        print("[UNDECIDABLE] cannot read")\n        return 2\n'
         '    return 0\n')
# `True == 1` in Python: a selftest probe that returns a bool from an except block
# is not a red exit. Without this negative fixture H8 would flag two live gates
# (their `_raises()` helpers) and the axis would be ignored within a week.
EXCBOOL = ('def _raises(fn):\n    try:\n        fn()\n    except ValueError:\n'
           '        return True\n    return False\n')


def _mkdir(root, name, files):
    d = os.path.join(root, name)
    os.makedirs(d, exist_ok=True)
    for fn, text in files.items():
        with open(os.path.join(d, fn), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
    return d


def selftest():
    import tempfile
    import shutil

    ok = True
    n = 0

    def add(label, cond):
        nonlocal ok, n
        n += 1
        if cond:
            print("[SELFTEST] %s" % label)
        else:
            ok = False
            print("[SELFTEST-FAIL] %s" % label)

    tmp = tempfile.mkdtemp(prefix="pytool-")
    try:
        pyd = _mkdir(tmp, "scripts", {
            "dead.py": DEAD_SRC, "exempt.py": EXEMPT_SRC, "short.py": TOO_SHORT_SRC,
            "used.py": USED_SRC, "broken.py": BROKEN_SRC, "future.py": FUTURE_SRC,
        })
        # H1 must catch the non-compiling file and only it.
        files = sorted(os.path.join(pyd, f) for f in os.listdir(pyd))
        bad1, scanned = compile_all(files)
        add("planted positive: a non-compiling script is caught by name",
            scanned == 6 and len(bad1) == 1 and bad1[0][0] == "broken.py")
        nested = _mkdir(pyd, "nested", {"deep.py": USED_SRC})
        add("recursive scan reaches scripts/*/*.py",
            len(scan_py_files(pyd)) == 7
            and any(os.sep + "nested" + os.sep in x for x in scan_py_files(pyd)))
        # H2 across the whole fixture set: dead.py has 1 unused (json), short.py
        # has 1 (reason too short), used/exempt have 0.
        bad2, nimp = dead_imports(files)
        add("planted positive: unused import is caught (dead.py json)",
            any("json" in x[2] for x in bad2))
        add("planted negative: a used import is NOT flagged (used.py)",
            not any(x[0] == "used.py" for x in bad2))
        add("signed side-effect exemption is honoured, bare/short reason is not",
            any(x[0] == "short.py" for x in bad2)
            and not any(x[0] == "exempt.py" for x in bad2))
        add("H2 import census covers the fixture (>= 6 imports seen)", nimp >= 6)
        add("planted negative: `from __future__ import annotations` is a directive, "
            "not a dead import (this exact false positive put 8 real files in red)",
            not any(x[0] == "future.py" for x in bad2)
            and scanned == 6)
        # floor must actually be able to fire: fixture is tiny, so judge() on it
        # must refuse rather than report "clean".
        rc, out = judge(files, _mkdir(tmp, "wf", {}))
        add("blind scan is undecidable (rc 2), never a green verdict on 5 files",
            rc == 2 and any("blind" in l for l in out))

        wfd = _mkdir(tmp, "workflows", {
            "unpinned.yml": WF_UNPINNED, "pinned.yml": WF_PINNED,
            "wrongpin.yml": WF_WRONGPIN, "nopython.yml": WF_NOPYTHON,
        })
        bad3, nwf, npyj = pin_parity(wfd)
        add("job invoking python3 with no pin is caught",
            any("unpinned" == x[0].split(".")[0] for x in bad3))
        add("pin that differs from the floor is caught (3.9 vs %s)" % FLOOR_TEXT,
            any("wrongpin" in x[0] for x in bad3))
        add("planted negative: a correctly pinned job is not flagged",
            not any("pinned.yml" == x[0] for x in bad3))
        add("planted negative: a job with no python needs no pin",
            not any("nopython" in x[0] for x in bad3) and npyj == 3)
        add("workflow scan sees all four fixtures", nwf == 4)
        # Comment-blindness, both directions: a WHY line naming python3 must not
        # demand a pin, and `python x.py` (no 3) must still count as an invocation.
        add("a comment mentioning python3 does not create a phantom pin demand",
            pin_parity(_mkdir(tmp, "wf2", {"c.yml": (
                "name: t\njobs:\n  a:\n    steps:\n      # run python3 gates later\n"
                "      - run: cargo build\n")})) [2] == 0)
        add("bare `python scripts/x.py` counts as an invocation (not just python3)",
            pin_parity(_mkdir(tmp, "wf3", {"d.yml": (
                "name: t\njobs:\n  a:\n    steps:\n      - run: python scripts/x.py\n"
                )})) [2] == 1)

        # job_blocks must separate jobs, not lump the file together.
        two = ("name: t\njobs:\n  a:\n    steps:\n      - run: python3 x.py\n"
               "  b:\n    steps:\n      - run: cargo build\n")
        blocks = dict(job_blocks(two))
        add("job_blocks splits per job (python only in job a)",
            sorted(blocks) == ["a", "b"] and "python3" in blocks["a"]
            and "python3" not in blocks["b"])

        add("`python3 -m pkg` counts as an invocation (govd shape)",
            pin_parity(_mkdir(tmp, "wf4", {"e.yml": (
                "name: t\njobs:\n  a:\n    steps:\n      - run: python3 -m scripts.govd run\n"
                )}))[2] == 1)
        wf_mod = WF_PINNED.replace("python3 scripts/whatever.py",
                                   "python3 -m scripts.govd run")
        add("a pinned `python3 -m` job is clean",
            not pin_parity(_mkdir(tmp, "wf5", {"f.yml": wf_mod}))[0])
        # H3: the classifier is the SAME function repo_eol uses, so the fixture
        # cannot pass while production mis-reads a row.
        add("i/crlf and i/mixed are violations, i/lf and i/-text are not",
            _eol_row_is_bad("i/crlf") and _eol_row_is_bad("i/mixed")
            and not _eol_row_is_bad("i/lf") and not _eol_row_is_bad("i/-text"))

        # H6/H7 get their own directory so their fixtures cannot perturb the
        # compile/dead-import counts asserted above.
        h67 = _mkdir(tmp, "h67", {"abs.py": ABS_FIX, "absok.py": ABS_OK,
                                  "env.py": ENVFIX, "envok.py": ENVOk})
        hf = sorted(os.path.join(h67, f) for f in os.listdir(h67))
        a_bad = abs_module_paths(hf)
        e_bad = envfail_exit1(hf)
        live_py = scan_py_files()
        add("H6 planted positive: module-level `D:\\...` and `/home/bob/proj` both caught",
            len(a_bad) == 2 and set(x[0] for x in a_bad) == {"abs.py"})
        add("H6 planted negative: __file__-derived ROOT and an os.name-guarded "
            "literal are NOT flagged (the two live patterns in this repo)",
            not any(x[0] == "absok.py" for x in a_bad))
        add("H6 backslash-blindness regression: separator checked by indexing, and a "
            "bare prefix constant (this gate's own allowlist) is not a violation",
            len(a_bad) == 2 and not abs_module_paths(live_py))
        add("H7 planted positive: returncode!=0 -> sys.exit(1) is caught",
            any(x[0] == "env.py" for x in e_bad))
        add("H7 planted negative: the same guard exiting 2 is clean",
            not any(x[0] == "envok.py" for x in e_bad))
        add("H6/H7 are green on the live tree (zero-violation baseline, %d files)"
            % len(live_py),
            not abs_module_paths(live_py) and not envfail_exit1(live_py))

        # H8 gets its own directory so its fixtures cannot perturb the counts above.
        h8 = _mkdir(tmp, "h8", {"exc.py": EXCFIX, "excok.py": EXCOK,
                               "excbool.py": EXCBOOL})
        x_bad = except_handler_red(sorted(os.path.join(h8, f) for f in os.listdir(h8)))
        add("H8 planted positive: except -> print([FAIL]) -> return 1 is caught",
            [x[0] for x in x_bad] == ["exc.py"])
        add("H8 planted negative: the same handler exiting 2 is clean",
            not any(x[0] == "excok.py" for x in x_bad))
        add("H8 planted negative: a bool-returning except probe is not a red exit "
            "(True == 1 was a real false positive of the first draft)",
            not any(x[0] == "excbool.py" for x in x_bad))
        add("H8 is green on the live tree (%d files) -- the four gates that failed "
            "it were moved to exit 2" % len(live_py), not except_handler_red(live_py))

        # Floors must be satisfiable on reality but not trivially true.
        real_py = live_py
        add("PY_FILES_FLOOR is above zero and below the live count (%d)"
            % len(real_py), 0 < PY_FILES_FLOOR <= len(real_py))
        add("WORKFLOW_FLOOR is above zero and below the live count",
            0 < WORKFLOW_FLOOR <= len([f for f in os.listdir(WORKFLOWS)
                                       if f.endswith((".yml", ".yaml"))]))
        add("the gate must be green on the live tree right now", run_real() == 0)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("[SELFTEST] %s (%d assertions)" % ("ALL ASSERTIONS HELD" if ok else "FAILED", n))
    return 0 if ok else 1


def _eol_row_is_bad(eol):
    return eol in ("i/crlf", "i/mixed")


def main(argv):
    args = list(argv[1:])
    if any(a not in ("--selftest",) for a in args):
        sys.stderr.write("usage: check_py_tooling_health.py [--selftest]\n")
        return 2
    if "--selftest" in args:
        return selftest()
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
