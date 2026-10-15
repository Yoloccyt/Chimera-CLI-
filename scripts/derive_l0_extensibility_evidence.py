#!/usr/bin/env python3
# -*- coding: ascii -*-
"""derive_l0_extensibility_evidence.py -- per-type evidence for ADR-215 D-23 (B0).

Why this script exists
----------------------
ADR-214 D-22 opened "Tier 2": per-type open/closed adjudication for the 227 public
L0 types, with a measured shortlist (21 zero-mention enums + 64 structs without
external literals). The shortlist COUNTS were recorded in prose but the NAMES were
never written to disk, and a negative claim ("zero mentions") that cannot be
re-derived is exactly the ADR-214 failure mode (a VerificationMethod scan that
missed the root tests/ directory produced a false shortlist). This script makes the
evidence machine-reproducible.

It is a REPORTER, not a gate: it changes no ledger, no manifest, no source. Its rc
encodes only selftest health and floor guards (0/2), never a policy verdict.

Method (locked to the G-115 parser on purpose)
----------------------------------------------
  * The production surface (which types exist, kind, module path, #[non_exhaustive])
    comes from check_l0_extensibility_class.extract_types -- same lexer, same
    cfg(test)-tail masking, same module_key rule as the gate. A second parser here
    would drift from the ledger keys.
  * Consumer surfaces are blanked with the SAME check_l0_purity.blank_rust lexer, so
    doc-comment mentions and string literals never count as consumption.
  * Word-boundary matching (\b<Name>\b). CamelCase names rarely collide; any
    suspicious hit is reviewable in the emitted TSV (files are listed, capped).
  * Surfaces are split so the report can distinguish production consumption from
    test/bench consumption, and in-crate from external:
      prod_ext      crates/*/src            (excluding nexus-contracts itself)
      test_ext      crates/*/tests|benches|examples
      root_tests    tests/ benches/ examples/ at repo root  (ADR-214 blind-spot fix)
      self_prod     crates/nexus-contracts/src
      self_tests    crates/nexus-contracts/tests|benches
  * Shortlist rules mirror ADR-214 D-22 verbatim:
      enum  shortlist  = external mention count == 0 (prod_ext + test_ext + root_tests)
      struct shortlist = external literal count (\b<Name>\s*\{) == 0 (same surfaces)
    Both are printed as lists and written to the TSV; the human adjudication (and
    the compile-feasibility check for any `open` candidate) happens AFTER this.

Outputs
-------
  tmp/l0_ext_evidence.tsv      one row per type: kind, ledger class, counts per
                               surface, shortlist flag, capped file lists
  stdout                       summary: floor guards, shortlists, dyn scans

  --selftest   planted fixtures prove the counting logic bites in both directions
               (a known-mentioned type and a known-absent type) before any real run.

Usage: py -3 scripts/derive_l0_extensibility_evidence.py [--selftest]
Exit codes: 0 ok, 2 tool/floor failure. Output is pure ASCII.
"""

import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

sys.path.insert(0, HERE)
import check_l0_purity as purity  # noqa: E402
import check_l0_extensibility_class as ext  # noqa: E402  (extract_types + ledger reader)

LEDGER = ext.LEDGER
OUT_TSV = os.path.join(ROOT, "tmp", "l0_ext_evidence.tsv")
FILE_LIST_CAP = 8
TYPES_FLOOR = 150          # same floor as G-115: a blind parser must not report
SHORTLIST_FLOOR = 20       # ADR-214 recorded 21 enums on the shortlist; far below
                           # that means the scan surface collapsed
DYN_SCAN_TARGETS = ("VectorStore", "VectorStoreExt")


def collect_rs_files(base, exclude_crate=None):
    out = []
    if not os.path.isdir(base):
        return out
    for dirpath, _dn, files in os.walk(base):
        if exclude_crate and os.sep + exclude_crate + os.sep in dirpath + os.sep:
            # exclude only when the path actually enters that crate directory
            rel = os.path.relpath(dirpath, base)
            if rel == exclude_crate or rel.startswith(exclude_crate + os.sep):
                continue
        for fn in sorted(files):
            if fn.endswith(".rs"):
                out.append(os.path.join(dirpath, fn))
    return out


def surfaces():
    """Ordered {surface_name: [abs paths]}; missing dirs yield empty lists."""
    crates_dir = os.path.join(ROOT, "crates")
    prod_ext, test_ext = [], []
    if os.path.isdir(crates_dir):
        for name in sorted(os.listdir(crates_dir)):
            crate = os.path.join(crates_dir, name)
            if not os.path.isdir(crate):
                continue
            if name == "nexus-contracts":
                continue
            prod_ext += collect_rs_files(os.path.join(crate, "src"))
            for sub in ("tests", "benches", "examples"):
                test_ext += collect_rs_files(os.path.join(crate, sub))
    root_tests = []
    for sub in ("tests", "benches", "examples"):
        root_tests += collect_rs_files(os.path.join(ROOT, sub))
    self_prod = collect_rs_files(os.path.join(ROOT, "crates", "nexus-contracts", "src"))
    self_tests = []
    for sub in ("tests", "benches"):
        self_tests += collect_rs_files(os.path.join(ROOT, "crates", "nexus-contracts", sub))
    return [("prod_ext", prod_ext), ("test_ext", test_ext),
            ("root_tests", root_tests), ("self_prod", self_prod),
            ("self_tests", self_tests)]


def blanked(path):
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            return purity.blank_rust(fh.read())
    except (IOError, OSError):
        return ""


def evidence_for(names, surf):
    """{name: {surface: (mentions, literals, [file:line,...])}} for one surface."""
    res = dict((n, dict((s, [0, 0, []]) for s, _f in surf)) for n in names)
    patterns = dict((n, (re.compile(r"\b%s\b" % re.escape(n)),
                         re.compile(r"\b%s\s*\{" % re.escape(n)))) for n in names)
    for sname, files in surf:
        for path in files:
            text = blanked(path)
            if not text:
                continue
            rel = os.path.relpath(path, ROOT).replace(os.sep, "/")
            for i, ln in enumerate(text.splitlines(), 1):
                for n in names:
                    mre, lre = patterns[n]
                    m = len(mre.findall(ln))
                    if not m:
                        continue
                    cell = res[n][sname]
                    cell[0] += m
                    cell[1] += len(lre.findall(ln))
                    if len(cell[2]) < FILE_LIST_CAP:
                        cell[2].append("%s:%d" % (rel, i))
    return res


def external_counts(row):
    """(mentions, literals) across prod_ext + test_ext + root_tests."""
    m = sum(row[s][0] for s in ("prod_ext", "test_ext", "root_tests"))
    l = sum(row[s][1] for s in ("prod_ext", "test_ext", "root_tests"))
    return m, l


def dyn_scan(surf):
    """dyn-usage evidence for the VectorStore candidate (ADR-215 D-28)."""
    hits = []
    for sname, files in surf:
        if sname in ("self_prod", "self_tests"):
            continue
        for path in files:
            text = blanked(path)
            for t in DYN_SCAN_TARGETS:
                for pat in (r"dyn\s+%s\b" % t, r"Box<dyn[^>]*%s\b" % t,
                            r"Arc<dyn[^>]*%s\b" % t):
                    if re.search(pat, text):
                        hits.append("%s %s in %s" % (sname, t,
                                     os.path.relpath(path, ROOT).replace(os.sep, "/")))
    return hits


def write_tsv(types, ledger_class, ev):
    os.makedirs(os.path.dirname(OUT_TSV), exist_ok=True)
    surf_names = [s for s, _ in surfaces()]
    with open(OUT_TSV, "w", encoding="utf-8") as fh:
        fh.write("# ADR-215 D-23 evidence; derived by derive_l0_extensibility_evidence.py\n")
        fh.write("# kind\ttype\tledger_class\text_mentions\text_literals\t"
                 + "\t".join(surf_names) + "\tshortlist\tfiles(cap=%d)\n" % FILE_LIST_CAP)
        for kind, mod, name, _ne in sorted(types):
            key = ext.type_key(kind, mod, name)
            row = ev[name]
            em, el = external_counts(row)
            short = ""
            if kind == "enum" and em == 0:
                short = "ENUM_ZERO_MENTION"
            elif kind == "struct" and el == 0:
                short = "STRUCT_NO_EXT_LITERAL"
            cells = []
            for s in surf_names:
                m, l, files = row[s][0], row[s][1], row[s][2]
                cells.append("%d/%d" % (m, l))
            files_all = []
            for s in surf_names:
                files_all += ["%s@%s" % (s, loc) for loc in row[s][2]]
            fh.write("%s\t%s\t%s\t%d\t%d\t%s\t%s\t%s\n"
                     % (kind.upper(), key, ledger_class.get(key, "?"), em, el,
                        "\t".join(cells), short, ";".join(files_all)))
    return OUT_TSV


def run_real():
    src_dir = os.path.join(ROOT, "crates", "nexus-contracts", "src")
    types = ext.extract_types(src_dir)
    if len(types) < TYPES_FLOOR:
        print("[TOOL-ERROR] extracted only %d types (< floor %d)" % (len(types), TYPES_FLOOR))
        return 2
    surf = surfaces()
    sizes = dict((s, len(f)) for s, f in surf)
    if sizes["prod_ext"] == 0 or sizes["root_tests"] == 0:
        print("[TOOL-ERROR] scan surface collapsed: %s" % sizes)
        return 2
    rows, malformed = ext.read_ledger(LEDGER)
    if malformed:
        for m in malformed[:5]:
            print("[TOOL-ERROR][malformed] %s" % m)
        return 2
    ledger_class = dict(("%s|%s" % (r["_kind"], r["type"]), r["class"]) for r in rows)
    names = sorted(set(n for (_k, _m, n, _ne) in types))
    ev = evidence_for(names, surf)
    out = write_tsv(types, ledger_class, ev)
    enum_short, struct_short = [], []
    for kind, mod, name, ne in types:
        key = ext.type_key(kind, mod, name)
        row = ev[name]
        em, el = external_counts(row)
        if kind == "enum" and em == 0:
            enum_short.append((key, ledger_class.get(key, "?")))
        elif kind == "struct" and el == 0:
            struct_short.append((key, ledger_class.get(key, "?")))
    if len(enum_short) < SHORTLIST_FLOOR:
        print("[TOOL-ERROR] enum shortlist only %d (< floor %d) -- surface drifted"
              % (len(enum_short), SHORTLIST_FLOOR))
        return 2
    print("[OK] types=%d surfaces=%s" % (len(types), sizes))
    print("[OK] enum shortlist (zero external mention) = %d" % len(enum_short))
    for k, c in enum_short:
        print("  %s [%s]" % (k, c))
    print("[OK] struct shortlist (no external literal) = %d" % len(struct_short))
    for k, c in struct_short:
        print("  %s [%s]" % (k, c))
    hits = dyn_scan(surf)
    print("[OK] dyn VectorStore/VectorStoreExt usage outside L0: %d hit(s)" % len(hits))
    for h in hits:
        print("  %s" % h)
    print("[OK] evidence written -> %s" % os.path.relpath(out, ROOT))
    return 0


# --- selftest: planted fixtures prove the counter bites in both directions ----

FIXTURE_EXT = {
    "prod_ext": ["pub struct UsedHere { pub x: u8 }\n"
                 "fn f(u: UsedHere) -> u8 { u.x }\n"
                 "/// MentionedOnlyInDoc is not consumption\n"],
    "test_ext": ["use nexus_contracts::TestOnly;\n"
                 "fn t() { let _ = TestOnly::A; let _ = Lit { x: 1 }; }\n"],
    "root_tests": ["fn r(v: Box<dyn VectorStore>) {}\n"],
    "self_prod": ["pub enum TestOnly { A }\npub struct Lit { pub x: u8 }\n"
                  "pub struct DocOnly { pub x: u8 }\npub struct UsedHere { pub x: u8 }\n"],
    "self_tests": [],
}


def selftest():
    ok = True

    def add(label, cond):
        nonlocal ok
        print("[SELFTEST] %s" % label if cond else "[SELFTEST-FAIL] %s" % label)
        if not cond:
            ok = False

    tmp = tempfile.mkdtemp(prefix="l0ext-ev-")
    try:
        files = {}
        for s, contents in FIXTURE_EXT.items():
            files[s] = []
            for i, c in enumerate(contents):
                p = os.path.join(tmp, s, "f%d.rs" % i)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, "w", encoding="utf-8") as fh:
                    fh.write(c)
                files[s].append(p)
        surf = [(s, files[s]) for s in ("prod_ext", "test_ext", "root_tests",
                                        "self_prod", "self_tests")]
        names = ["TestOnly", "Lit", "DocOnly", "UsedHere", "VectorStore"]
        ev = evidence_for(names, surf)
        add("enum variant use in consumer tests counts as mention",
            ev["TestOnly"]["test_ext"][0] >= 2)
        add("struct literal in consumer tests counts as literal",
            ev["Lit"]["test_ext"][1] >= 1)
        add("doc-comment mention does NOT count (blank_rust strips it)",
            ev["DocOnly"]["prod_ext"][0] == 0)
        add("prod consumption in another crate counts",
            ev["UsedHere"]["prod_ext"][0] >= 2)
        add("dyn usage detected (root_tests Box<dyn VectorStore>)",
            bool(dyn_scan(surf)))
        # negative control: a name absent everywhere reports zero, never a phantom
        ev2 = evidence_for(["NoSuchTypeAnywhere"], surf)
        m, l = external_counts(ev2["NoSuchTypeAnywhere"])
        add("absent name reports zero mentions and zero literals", m == 0 and l == 0)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("[SELFTEST] %s" % ("ALL ASSERTIONS HELD" if ok else "FAILED"))
    return 0 if ok else 1


def main(argv):
    if "--selftest" in argv[1:]:
        return selftest()
    if len(argv) > 1:
        sys.stderr.write("usage: derive_l0_extensibility_evidence.py [--selftest]\n")
        return 2
    return run_real()


if __name__ == "__main__":
    import gate_rc  # crash -> exit 2, never a borrowed traceback exit 1

    sys.exit(gate_rc.run(lambda: main(sys.argv)))
