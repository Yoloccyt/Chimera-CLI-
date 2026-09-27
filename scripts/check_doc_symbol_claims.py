#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""check_doc_symbol_claims.py -- gate the NAMES the architecture doc asserts.

Why this gate exists (finding F6, 2026-09-21)
---------------------------------------------
`check_doc_drift.sh` pins the architecture doc's CARDINAL numbers (NexusEvent
variants / EventTopic classes / crate count) to `doc_count_freeze.txt`.  That
keeps "how many" honest while "WHICH" rots freely: the doc's event table named
`SecurityAuditCompleted` (no such NexusEvent variant at all) and
`RouterStatsUpdated` (real name: `RouterStatsReported`) -- and stayed green,
because a wrong name changes no count.

Rule (deliberately narrow, so it can be trusted to block)
---------------------------------------------------------
Every backticked PascalCase identifier inside a markdown TABLE ROW must occur
as a token in some `crates/**/*.rs`.  Any exception must be registered in
`scripts/doc_symbol_claim_baseline.txt`, one symbol per line, shrink-only:

  GAP    a new unregistered claim        -> EXIT 1   (fix the doc or register)
  STALE  a registered symbol no longer a gap -> EXIT 1 (delete the line)
  clean  every gap is registered         -> EXIT 0

Cross-crate mentions are NOT judged: we only require the name to exist
SOMEWHERE in the workspace.  A name that exists nowhere cannot be an
implementation the doc forgot to update -- it is a claim about code that is
not there.  That is the only assertion this gate is willing to make.

Modes / exit codes
------------------
  (default)   judge the real repo        0 = clean, 1 = GAP or STALE
  --selftest  fixture injection          0 = detectors have teeth, 1 = not
  --emit      print current gaps as baseline lines (then hand-review)
  usage error                            2

Output is pure ASCII (project script convention, Windows GBK consoles).
"""
import os
import re
import sys

DOC_DEFAULT = os.path.join("docs", "architecture", "CODE_WIKI.md")
BASELINE_DEFAULT = os.path.join("scripts", "doc_symbol_claim_baseline.txt")
CRATES_DEFAULT = "crates"

TABLE_ROW = re.compile(r"^\s*\|")
IDENT = re.compile(r"`([A-Z][A-Za-z0-9_]{2,}(?:::[A-Za-z0-9_<>]+)*)`")
TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def code_symbols(crates_dir):
    """Every identifier token appearing in any .rs under crates_dir."""
    toks = set()
    if not os.path.isdir(crates_dir):
        return None
    for root, dirs, files in os.walk(crates_dir):
        dirs[:] = [d for d in dirs if d != "target"]
        for f in files:
            if not f.endswith(".rs"):
                continue
            try:
                with open(os.path.join(root, f), encoding="utf-8",
                          errors="replace") as fh:
                    toks.update(TOKEN.findall(fh.read()))
            except OSError:
                continue
    return toks


def repo_artifacts(crates_dir):
    """Basenames (extension stripped) of everything in the repo tree.

    WHY: the rule asks "does this NAME exist", and a table row citing
    `Dockerfile` or the doc stem `ARCHITECTURE_HEALTH_AUDIT` names a real
    artifact, not a Rust type.  Matching against .rs tokens only turned both
    into false GAPs -- measured when the gate was run against agents.md and
    .qoder/rules/chimera.md (2 of 4 findings were this class).
    Directory names count too: `crates/event-bus` sections name live crates.
    """
    names = set()
    root = os.path.dirname(os.path.abspath(crates_dir)) or "."
    for base, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs
                   if d not in ("target", ".git", "node_modules", ".toolchain")]
        for d in dirs:
            names.add(d)
        for f in files:
            names.add(os.path.splitext(f)[0])
    return {n for n in names if n}


def doc_claims(doc_path):
    """(line_no, symbol) for PascalCase identifiers inside table rows."""
    out = []
    with open(doc_path, encoding="utf-8", errors="replace") as fh:
        for i, line in enumerate(fh, 1):
            if not TABLE_ROW.match(line):
                continue
            for m in IDENT.finditer(line):
                out.append((i, re.split(r"::|[<>]", m.group(1))[0]))
    return out


def find_gaps(claims, symbols):
    """Symbols asserted by the doc that occur nowhere in the code."""
    first = {}
    for line_no, sym in claims:
        if sym not in symbols and sym not in first:
            first[sym] = line_no
    return first


def load_baseline(path):
    """Return ({symbol: reason}, []). Missing file is not fatal for --emit."""
    entries, errors = {}, []
    if not os.path.isfile(path):
        return entries, errors
    with open(path, encoding="utf-8", errors="replace") as fh:
        for i, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split(None, 1)
            sym = parts[0]
            reason = parts[1].strip() if len(parts) > 1 else ""
            if not re.match(r"^[A-Z][A-Za-z0-9_]{2,}$", sym):
                errors.append("malformed baseline %s:%d -> %r" % (path, i, line))
                continue
            if not reason:
                errors.append("registered symbol without reason: %s" % sym)
                continue
            if sym in entries:
                errors.append("duplicate baseline entry: %s" % sym)
                continue
            entries[sym] = reason
    return entries, errors


def adjudicate(gaps, baseline):
    """Compare both sides against the RAW gap set (never against a filtered one).

    An earlier draft subtracted the baseline first and then looked for stale
    lines in what was left, which made every registered line report STALE --
    the same class of bug as a ratchet that grades its own output.
    """
    current = set(gaps)
    registered = set(baseline)
    return sorted(current - registered), sorted(registered - current)


def judge(doc, baseline_path, crates_dir, emit=False, stream=sys.stdout):
    symbols = code_symbols(crates_dir)
    if symbols is None:
        stream.write("UNDECIDABLE: crates dir missing: %s\n" % crates_dir)
        return 2
    symbols |= repo_artifacts(crates_dir)
    if not os.path.isfile(doc):
        stream.write("UNDECIDABLE: doc missing: %s\n" % doc)
        return 2
    claims = doc_claims(doc)
    gaps = find_gaps(claims, symbols)
    baseline, fmt_errors = load_baseline(baseline_path)

    if emit:
        stream.write("# %d gaps -- hand-review before adopting\n" % len(gaps))
        for sym in sorted(gaps):
            stream.write("%s    # doc:%d UNREVIEWED\n" % (sym, gaps[sym]))
        return 0

    new_gaps, stale = adjudicate(gaps, baseline)
    stream.write("doc=%s\n" % doc)
    stream.write("table-row symbol occurrences=%d  distinct gaps=%d  "
                 "registered=%d\n" % (len(claims), len(gaps), len(baseline)))
    for err in fmt_errors:
        stream.write("MALFORMED  %s\n" % err)
    for sym in new_gaps:
        stream.write("GAP    %-40s (doc line %d) not registered\n"
                     % (sym, gaps[sym]))
    for sym in stale:
        stream.write("STALE  %-40s registered but no longer a gap -- "
                     "delete the line\n" % sym)
    if fmt_errors or new_gaps or stale:
        stream.write("RESULT: FAIL (new=%d stale=%d malformed=%d)\n"
                     % (len(new_gaps), len(stale), len(fmt_errors)))
        return 1
    stream.write("RESULT: PASS (every unresolvable name is registered; "
                 "register is shrink-only)\n")
    return 0


# --------------------------------------------------------------------------
# selftest: fixtures prove each verdict has teeth
# --------------------------------------------------------------------------

def _fixture(tmp, doc_text, baseline_text, rust_text):
    crates = os.path.join(tmp, "crates", "demo", "src")
    os.makedirs(crates, exist_ok=True)
    with open(os.path.join(crates, "lib.rs"), "w", encoding="utf-8") as fh:
        fh.write(rust_text)
    doc = os.path.join(tmp, "doc.md")
    with open(doc, "w", encoding="utf-8") as fh:
        fh.write(doc_text)
    base = os.path.join(tmp, "base.txt")
    with open(base, "w", encoding="utf-8") as fh:
        fh.write(baseline_text)
    return doc, base, os.path.join(tmp, "crates")


def selftest():
    import tempfile
    import io
    checks = []

    def run(name, doc_text, base_text, rust_text, want, want_in="", want_not=None):
        tmp = tempfile.mkdtemp(prefix="dsc-")
        doc, base, crates = _fixture(tmp, doc_text, base_text, rust_text)
        buf = io.StringIO()
        try:
            got = judge(doc, base, crates, stream=buf)
        except Exception as exc:                       # noqa: BLE001
            checks.append((name, "raised %r" % exc, False))
            return
        out = buf.getvalue()
        ok = got == want and want_in in out
        if want_not is not None and want_not in out:
            ok = False
        checks.append((name, "exit=%d out=%r" % (got, out[:0] or "..."), ok))

    real_type = "| **key types** | `RealThing` |\n"
    ghost = "| **key types** | `NoSuchType` |\n"
    rust = "pub struct RealThing;\n"

    # 1 clean: everything the doc names exists.
    run("t1-clean", real_type, "", rust, 0, want_in="RESULT: PASS")
    # 2 GAP: unregistered phantom type must be red (the primary property).
    run("t2-ghost-unregistered", ghost, "", rust, 1, want_in="GAP")
    # 3 registered gap is tolerated but visible.
    run("t3-ghost-registered", ghost, "NoSuchType    # reviewed debt\n", rust,
        0, want_in="registered=1")
    # 4 STALE: a debt silently fixed must still be red (ratchet cannot rot).
    run("t4-stale", real_type, "RealThing    # stale\n", rust, 1,
        want_in="STALE")
    # 5 prose (non-table) text is out of scope -- no false positive.
    run("t5-prose-ignored", "`NoSuchType` in prose.\n", "", rust, 0,
        want_in="RESULT: PASS")
    # 6 qualified paths judge only the leading segment.
    run("t6-qualified", "| `RealThing::sub` |\n", "", rust, 0,
        want_in="RESULT: PASS")
    # 7 malformed entry (no reason) is rejected, not skipped.
    run("t7-no-reason", ghost, "NoSuchType\n", rust, 1, want_in="MALFORMED")
    # 8 duplicate entry rejected.
    run("t8-duplicate", ghost,
        "NoSuchType    # b\nNoSuchType    # a\n", rust, 1,
        want_in="duplicate")
    # 9 a name that exists as a repo ARTIFACT (file/dir) is not a gap.
    #    Regression pin: `Dockerfile` and the doc stem `ARCHITECTURE_HEALTH_AUDIT`
    #    were false GAPs when the universe was .rs tokens only.
    tmp9 = tempfile.mkdtemp(prefix="dsc-")
    d9, b9, c9 = _fixture(tmp9, "| **key types** | `BuildArtifact` |\n", "", rust)
    with open(os.path.join(tmp9, "BuildArtifact.md"), "w", encoding="utf-8") as fh:
        fh.write("stub artifact\n")
    buf = io.StringIO()
    code = judge(d9, b9, c9, stream=buf)
    checks.append(("t9-artifact-not-gap", "exit=%d" % code,
                   code == 0 and "PASS" in buf.getvalue()))
    # 10 undecidable: missing doc -> 2, never a silent pass.
    tmp = tempfile.mkdtemp(prefix="dsc-")
    doc, base, crates = _fixture(tmp, real_type, "", rust)
    buf = io.StringIO()
    code = judge(os.path.join(tmp, "absent.md"), base, crates, stream=buf)
    checks.append(("t9-missing-doc-undecidable", "exit=%d" % code, code == 2))
    # 10 undecidable: missing crates dir -> 2.
    buf = io.StringIO()
    code = judge(doc, base, os.path.join(tmp, "no-crates"), stream=buf)
    checks.append(("t10-missing-crates-undecidable", "exit=%d" % code, code == 2))

    failed = [n for n, _, ok in checks if not ok]
    for name, detail, ok in checks:
        print("%-34s %s  %s" % (name, "ok" if ok else "FAIL", detail))
    print("RESULT: %s (%d/%d detectors hold)"
          % ("PASS" if not failed else "FAIL", len(checks) - len(failed),
             len(checks)))
    return 0 if not failed else 1


def usage(msg):
    sys.stderr.write("ERROR: %s\n" % msg)
    sys.stderr.write(__doc__.split("Modes / exit codes")[0].strip()[-400:] + "\n")
    return 2


def main(argv):
    args = list(argv[1:])
    if "--selftest" in args:
        if len(args) > 1:
            return usage("--selftest takes no other arguments")
        return selftest()
    emit = False
    if "--emit" in args:
        args.remove("--emit")
        emit = True
    doc, base, crates = DOC_DEFAULT, BASELINE_DEFAULT, CRATES_DEFAULT
    while args:
        a = args.pop(0)
        if a == "--doc" and args:
            doc = args.pop(0)
        elif a == "--baseline" and args:
            base = args.pop(0)
        elif a == "--crates" and args:
            crates = args.pop(0)
        else:
            return usage("unknown argument %r" % a)
    return judge(doc, base, crates, emit=emit)


if __name__ == "__main__":
    import gate_rc  # 只在入口需要：崩溃必须退 2, 不得借 1 冒充"判过且红"（F32/F33）
    sys.exit(gate_rc.run(lambda: main(sys.argv)))
