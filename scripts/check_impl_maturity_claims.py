#!/usr/bin/env python3
# -*- coding: utf-8 -*-
# =============================================================================
# check_impl_maturity_claims.py - implementation-maturity self-label gate (B0-2)
# =============================================================================
# Purpose: quantify the gap that the "zero Stub / all crates implemented" claim
#          cannot see. That claim is decided by scanning for the Rust macros
#          `todo!()` / `unimplemented!()`. This repository, however, labels
#          unfinished code in its OWN honest comments instead (`MOCK-ONLY`,
#          `DEFERRED(...)`, and the Chinese self-declarations below), so every
#          such marker passes straight through the existing judgement and the
#          headline metric stays green while fake behaviour ships.
#
# Measured origin: docs/reports/Q2-decision-brief-7-ghost-crates-2026-09-21.md
#   section 4 ("the ruler is wrong"), re-confirmed 2026-09-23: chtc-bridge's 5
#   IDE adapters return a hard-coded `success: true` and their tests assert that
#   very mock value; mtpe-executor's speedup tests divide by a self-injected
#   50us fake latency. All three sites are labelled in-code, honestly.
#
# Design (deliberately mirrors two existing gates so the repo speaks one
# dialect, and so the gate is not born red and then disabled):
#   * ONLY-DECREASE RATCHET, like scripts/dep_edge_freeze.txt. The baseline
#     records today's measured truth per (crate, label); any INCREASE is a
     # failure, any DECREASE is reported for baseline refresh, a baseline entry
#     that reaches zero must be deleted (an allowance must not outlive the
#     debt it allowed).
#   * Exit-code dialect shared with gate_rc: 0 pass / 1 judged red /
#     2 undecidable (bad usage, bad baseline, missing dirs).
#
# Scope decisions (each one prevents a known false-positive class):
#   1. Comment text only. Rust identifiers named `PlaceholderEmbedding` or
#      `InMemoryWal` are not self-declarations; matching labels anywhere in a
#      file would flag them. Strings are stripped, block comments tracked.
#   2. Refutation lines are skipped: an author quoting a label in order to say
#      it is GONE ("... has been removed", "not present") must not count as a
#      WIP marker. The negation words are the repository's own established
#      vocabulary, not a coinage invented to make a number look green.
#   3. `crates/*/src/**` only. Tests and benches legitimately contain fakes;
#      scoring them would hide the production claim we are trying to measure.
#   4. Advisory tier for ambiguous English words (`placeholder`, `stub`,
#      `mock`): reported as INFO with locations, never scored, never ratcheted.
#      Rationale: these appear in honest structural names, so putting them in
#      the ratchet would teach the next reader to widen the gate out of the way.
#
# Usage:
#   python scripts/check_impl_maturity_claims.py --selftest
#   python scripts/check_impl_maturity_claims.py --emit      # full audit dump
#   python scripts/check_impl_maturity_claims.py             # gate verdict
# =============================================================================
import argparse
import os
import re
import sys
import tempfile
import shutil

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
BASELINE = os.path.join(ROOT, "scripts", "impl_maturity_baseline.txt")

# Scored labels, in report order. Written as \uXXXX escapes so this source file
# stays ASCII (project script convention: avoids CJK locale/parse issues on
# Windows, cf. the .ps1 BOM class of failures) while still matching the Chinese
# self-declarations the code actually uses.
#
# WHY there is no present-tense qualifier (tried and reverted, 2026-09-23): an
# earlier revision gated the Chinese labels behind a cue like 当前/待/将, to keep
# historical references ("P1-6 已替换占位实现") out of the ledger. Measured cost:
# it dropped six CURRENT declarations -- pvl-layer/src/producer.rs:11 "置信度:
# 占位实现,基于内容哈希", verifier.rs:25 "占位实现,实际场景应从 SecCore 获取动态
# 黑名单", seccore/src/asa.rs:155 "Week 5 占位实现" -- because this corpus states
# both past substitutions and present debt in the same bare form. Recall wins:
# a missed real marker is a permanent blind spot, while a stale-history row is
# visible noise the next reader annotates or deletes. Explicit completion
# phrasing is still excluded, via NEGATION below.
BLOCKING = (
    ("MOCK-ONLY", (r"MOCK-ONLY",)),
    ("DEFERRED", (r"DEFERRED\s*\(",)),
    ("PSEUDO", (u"\u4f2a\u5b9e\u73b0",)),                     # pseudo-implementation
    ("NOT-IMPL-CN", (u"\u672a\u5b9e\u73b0",)),                 # not implemented
    ("DEGRADE-CN", (u"\u6052\u964d\u7ea7",)),                  # permanently degraded
    ("PLACEHOLDER-CN", (u"\u5360\u4f4d\u5b9e\u73b0",)),        # placeholder implementation
    ("SIMPLIFIED-CN", (u"\u7b80\u5316\u5b9e\u73b0",)),         # simplified implementation
    ("SIMULATED-CN", (u"\u6a21\u62df\u5b9e\u73b0",)),          # simulated implementation
    # WHY this is BLOCKING while bare `placeholder` stays ADVISORY: the two-word
    # phrase is a self-declaration of unfinished code, and the one instance in the
    # tree (chimera-cli/src/composition.rs:80) sits inside the R2-thaw feature gate
    # and says it *returns Ok* to keep compiling -- i.e. a fail-closed L4 breaker
    # path that unconditionally grants. An ambiguous English word in prose cannot
    # carry that weight; "placeholder implementation" can.
    ("PLACEHOLDER-EN", (r"[Pp]laceholder implementation",)),
)

# Advisory (English words that also occur inside honest structural names).
ADVISORY = (r"\bplaceholder\b", r"\bstub\b", r"\bmock\b")

# Refutation vocabulary already used across this repository's governance text:
# a comment matching one of these is the author describing an ABSENT thing.
NEGATION = (
    r"MOCK-ONLY\s*\u5df2", u"\u4e0d\u5b58\u5728",   # "no longer"; "does not exist"
    u"\u5df2\u5220\u9664", u"\u5df2\u79fb\u9664",   # "already deleted/removed"
    u"\u5df2\u66ff\u6362", u"\u5df2\u5347\u7ea7",   # "already replaced/upgraded"
    u"\u96f6\u547d\u4e2d", u"\u65e0\u6b64",         # "zero hits"; "no such"
    u"\u5e7b\u7075", u"\u52ff\u7167\u6284",         # "phantom"; "do not copy"
)


def set_root(path):
    """Re-point the scan at another repository root (used by the smoke test).

    WHY this exists: the gate must be provably able to go red, and the honest way
    to show that is to run the real scan path against a synthetic tree rather
    than only unit-testing the classifier. Writing a fake marker into a live
    `crates/*/src` file to prove the gate works would dirty the working tree
    (and this repo currently carries uncommitted work), so the scan root is
    injectable instead.
    """
    global CRATES, BASELINE
    CRATES = os.path.join(path, "crates")
    BASELINE = os.path.join(path, "scripts", "impl_maturity_baseline.txt")


def extract_comments(path):
    """Yield (line_no, comment_text) for every comment segment in a Rust file.

    Handles line comments (`//`, `///`, `//!`) and block comments across lines,
    while tracking string literals so a `//` inside a string is not mistaken for
    a comment start. Char literals and raw strings are deliberately not parsed:
    both are vanishingly rare for `//` in this corpus, and a miss there would
    drop a line from the audit rather than invent a marker.

    Returns empty for unreadable files (encoding errors are not gate failures).
    """
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
    except OSError:
        return
    in_block = False
    block_opened = 0          # nesting depth (Rust allows nested /* */)
    in_contract = False
    for no, line in enumerate(text.splitlines(), 1):
        # The CRATE-CONTRACT header (scripts/crate_contract.py) *restates* labels
        # harvested from the crate's own comments, so scanning it would double-book
        # one debt as two and turn the ratchet red merely because a header was
        # installed. Skip the whole block region, not just its two marker lines.
        if in_contract:
            if "CRATE-CONTRACT END" in line:
                in_contract = False
            continue
        if "CRATE-CONTRACT BEGIN" in line:
            in_contract = True
            continue
        in_str = False
        i = 0
        segs = []
        while i < len(line):
            c = line[i]
            nxt = line[i + 1] if i + 1 < len(line) else ""
            if in_block:
                if c == "/":
                    i += 1
                    continue
                if c == "*" and nxt == "/":
                    block_opened -= 1
                    in_block = block_opened > 0
                    i += 2
                    continue
                i += 1
                continue
            if in_str:
                if c == "\\":
                    i += 2
                    continue
                if c == '"':
                    in_str = False
                i += 1
                continue
            if c == '"':
                in_str = True
                i += 1
                continue
            if c == "/" and nxt == "*":
                in_block = True
                block_opened += 1
                i += 2
                continue
            if c == "/" and nxt == "/":
                segs.append(line[i:])
                break
            i += 1
        for s in segs:
            yield no, s


def _match_any(comment, groups):
    """Return the first label in `groups` whose patterns hit the comment."""
    for label, patterns in groups:
        if any(re.search(p, comment) for p in patterns):
            return label
    return None


def classify(comment):
    """Return the scored maturity label this comment self-declares, else None.

    First match wins so a comment saying both `DEFERRED(...)` and "placeholder
    implementation" counts once -- double counting would inflate the ratchet and
    make a real fix look insufficient. Refutation phrasing ("已替换占位实现")
    never scores.
    """
    if any(re.search(p, comment) for p in NEGATION):
        return None
    return _match_any(comment, BLOCKING)


def scan_crate(crate_dir):
    """Return {(crate, label): [ 'relpath:line', ... ]} for one crate's src/.

    Empty dict when the crate has no `src/` (a bin-only or deleted member).
    """
    src = os.path.join(CRATES, crate_dir, "src")
    if not os.path.isdir(src):
        return {}
    hits = {}
    for dirpath, _, files in os.walk(src):
        for f in sorted(files):
            if not f.endswith(".rs"):
                continue
            full = os.path.join(dirpath, f)
            rel = os.path.relpath(full, ROOT).replace(os.sep, "/")
            for no, comment in extract_comments(full):
                label = classify(comment)
                if label is None:
                    continue
                hits.setdefault((crate_dir, label), []).append("%s:%d" % (rel, no))
    return hits


def scan_all():
    """Return (hits_by_key, advisory_locations) over every crate on disk."""
    hits = {}
    advisory = []
    for crate_dir in sorted(os.listdir(CRATES)) if os.path.isdir(CRATES) else []:
        if not os.path.isdir(os.path.join(CRATES, crate_dir, "src")):
            continue
        for key, locs in scan_crate(crate_dir).items():
            hits.setdefault(key, []).extend(locs)
        src = os.path.join(CRATES, crate_dir, "src")
        for dirpath, _, files in os.walk(src):
            for f in files:
                if not f.endswith(".rs"):
                    continue
                for no, comment in extract_comments(os.path.join(dirpath, f)):
                    # Advisory stays out of the ledger but stays visible: an
                    # English structural word must not be mistaken for absent.
                    if any(re.search(p, comment, re.I) for p in ADVISORY):
                        rel = os.path.relpath(os.path.join(dirpath, f), ROOT)
                        advisory.append("%s:%d" % (rel.replace(os.sep, "/"), no))
    return hits, advisory


def load_baseline(path=None):
    """Parse the ratchet baseline `crate.LABEL=<count>` lines.

    Returns {(crate, label): count}. Raises ValueError on a malformed line so a
    typo cannot silently widen the allowance (that is how a gate stops having
    teeth), which main() turns into exit 2.

    WHY `path=None` and not `path=BASELINE`: a Python default argument is bound
    when the `def` executes, i.e. to the module-level value at import time --
    passing the constant as a default would silently ignore `set_root()`. The
    smoke test caught exactly this: three of its four assertions drove the gate
    at the real repository instead of the synthetic tree.
    """
    if path is None:
        path = BASELINE
    baseline = {}
    if not os.path.exists(path):
        raise ValueError("baseline file missing: %s" % path)
    with open(path, encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            key, sep, val = line.partition("=")
            if not sep or "." not in key or not re.fullmatch(r"\d+", val.strip()):
                raise ValueError("needs `crate.LABEL=<count>: %r" % raw.rstrip())
            crate, label = key.strip().rsplit(".", 1)
            baseline[(crate, label)] = int(val.strip())
    return baseline


def decide(hits, baseline):
    """Compare measured counts against the ratchet.

    Returns (new, grown, rotate, stale) lists of (key, measured, allowed).
    A key absent from the baseline is `new` with allowed 0; that is what gives
    the gate its teeth -- declaring a fresh WIP marker is a red build.
    """
    new, grown, rotate, stale = [], [], [], []
    for key, allowed in sorted(baseline.items()):
        measured = len(hits.get(key, []))
        if measured == 0:
            stale.append((key, measured, allowed))
        elif measured > allowed:
            grown.append((key, measured, allowed))
        elif measured < allowed:
            rotate.append((key, measured, allowed))
    for key, locs in sorted(hits.items()):
        if key not in baseline:
            new.append((key, len(locs), 0))
    return new, grown, rotate, stale


def emit_baseline(hits):
    """Print a paste-ready baseline for the current measured state."""
    print("# impl_maturity_baseline.txt -- measured by --emit on the current tree")
    print("# one line per (crate, label); only-decrease ratchet; delete a line")
    print("# when it reaches 0 (an allowance must not outlive its debt).")
    for (crate, label), locs in sorted(hits.items()):
        print("%s.%s=%d  # %s" % (crate, label, len(locs), locs[0]))


def selftest():
    """Prove the gate can see what it claims to see, and can go red.

    Two halves: (1) a classifier truth table over constructed comments, and
    (2) an end-to-end run of decide() against a synthetic corpus + baseline so
    the four verdicts (clean / new / grown / rotate / stale) are each produced
    at least once. A gate whose failure paths were never executed is not
    verified -- it is only assumed.
    """
    ok = True
    cases = {
        "// MOCK-ONLY: real IDE integration lands with v3.x MCP Mesh": "MOCK-ONLY",
        "// DEFERRED(T8-3 Audit): pseudo below": "DEFERRED",
        "// \u672c\u51fd\u6570\u4e3a\u4f2a\u5b9e\u73b0": "PSEUDO",                     # "this fn is a pseudo impl"
        "// InMemoryWal \u5f53\u524d\u662f\u5360\u4f4d\u5b9e\u73b0": "PLACEHOLDER-CN",  # "currently is a"
        "// \u5360\u4f4d\u5b9e\u73b0\uff0c\u5f85\u5468\u62a5\u544a\u63a5\u5165\u540e\u66ff\u6362": "PLACEHOLDER-CN",   # "to be replaced"
        "// \u7b80\u5316\u5b9e\u73b0\uff0c\u4ec5\u4fdd\u7559\u4e3b\u8def\u5f84": "SIMPLIFIED-CN",
        "// \u63a8\u7406\u4fa7\u76ee\u524d\u4e3a\u6a21\u62df\u5b9e\u73b0": "SIMULATED-CN",            # cue "目前"
        "// \u4e09\u6a21\u6001\u6052\u964d\u7ea7\uff08\u65e0\u6743\u91cd\uff09": "DEGRADE-CN",
        "// \u4e8b\u4ef6\u9762\u5c1a\u672a\u5b9e\u73b0": "NOT-IMPL-CN",
        # refutations must NOT count:
        "// MOCK-ONLY \u5df2\u79fb\u9664\uff0c\u73b0\u5728\u662f\u771f\u5b9e\u73b0": None,
        "// \u65e7\u540d PlaceholderEmbedding \u5df2\u5220\u9664": None,
        "// \u5168\u5e93\u96f6\u547d\u4e2d\uff0c\u4e0d\u5b58\u5728\u8be5\u7c7b\u578b": None,
        # real false positive found on 2026-09-23 in mcp-mesh/src/lib.rs:7 -- a
        # completion claim that happens to contain the words "placeholder
        # implementation". Kept as a permanent regression case.
        "// 2PC \u539f\u5b50\u63d0\u4ea4(P1-6 \u5df2\u66ff\u6362\u5360\u4f4d\u5b9e\u73b0)": None,
        # Verbatim mcp-mesh/src/quantum/transaction.rs:1 -- a bare label with no
        # cue. It scores, because the alternative (see the BLOCKING note) is to
        # lose pvl-layer's and seccore's genuine current markers too.
        "// \u91cf\u5b50\u4e8b\u52a1\u72b6\u6001\u673a \u2014 2PC \u5360\u4f4d\u5b9e\u73b0": "PLACEHOLDER-CN",
        # non-self-declarations must NOT count:
        "pub struct PlaceholderEmbedding;": None,
        "let mask = compute_all_masks();  // apply the mask": None,
        # the English two-word phrase IS a self-declaration (added 2026-09-23 after
        # `cargo check --features r2_unfreeze` surfaced composition.rs:80 declaring a
        # placeholder that returns Ok inside the R2 fail-closed gate path):
        "// Placeholder implementation - returns Ok to allow compilation during Phase 1": "PLACEHOLDER-EN",
        "// placeholder implementation, real logic lands with T4": "PLACEHOLDER-EN",
        # but a type name or a past-tense note is not:
        "pub fn placeholder_impl() {}  // module placeholder_impl for tests": None,
    }
    for line, want in cases.items():
        got = classify(line)
        if got != want:
            print("[SELFTEST] classifier mismatch %r -> %r want %r" % (line[:40], got, want))
            ok = False
    # Region-level case, which the truth table above structurally cannot reach:
    # crate_contract.py installs a CRATE-CONTRACT block that RESTATES labels
    # harvested from the crate's own comments. If the scanner reads that block,
    # one debt is booked twice and the ratchet turns red purely because a header
    # was installed (this happened on 2026-09-23: chtc-bridge MOCK-ONLY 5 -> 6).
    tmpd = tempfile.mkdtemp(prefix="maturity-region-")
    try:
        src = os.path.join(tmpd, "lib.rs")
        with open(src, "w", encoding="utf-8") as fh:
            fh.write(
                "//! CRATE-CONTRACT BEGIN\n"
                "//! MATURITY: MOCK-ONLY\n"
                "//! CRATE-CONTRACT END\n"
                "//! the crate's own honest marker: MOCK-ONLY\n"
                "#![forbid(unsafe_code)]\n")
        labels = [classify(s) for _no, s in extract_comments(src)]
        hits = [lab for lab in labels if lab == "MOCK-ONLY"]
        if len(hits) != 1:
            print("[SELFTEST] contract block must not be counted, labels=%r"
                  % (labels,))
            ok = False
        # Positive control: the SAME two comment lines without the block markers
        # must both count -- otherwise the assertion above could pass while the
        # skip logic was dead (e.g. a typo in the marker text).
        src2 = os.path.join(tmpd, "plain.rs")
        with open(src2, "w", encoding="utf-8") as fh:
            fh.write(
                "//! MATURITY: MOCK-ONLY\n"
                "//! the crate's own honest marker: MOCK-ONLY\n"
                "#![forbid(unsafe_code)]\n")
        plain = [lab for _no, s in extract_comments(src2)
                 if (lab := classify(s)) == "MOCK-ONLY"]
        if len(plain) != 2:
            print("[SELFTEST] without block markers both labels must count, got %d"
                  % len(plain))
            ok = False
    finally:
        shutil.rmtree(tmpd, ignore_errors=True)
    # Coverage assertion, not just a tally: every scored label must appear in the
    # truth table above. Without it, adding a label with no case would keep the
    # selftest green while the new label went completely untested.
    covered = {w for w in cases.values() if w}
    all_labels = {label for label, _ in BLOCKING}
    if covered != all_labels:
        print("[SELFTEST] label coverage gap: untested=%r spurious=%r"
              % (sorted(all_labels - covered), sorted(covered - all_labels)))
        ok = False
    n_neg = sum(1 for w in cases.values() if w is None)
    if not ok:
        return 1

    # end-to-end verdict coverage on a synthetic baseline
    base = {("aaa", "MOCK-ONLY"): 2, ("bbb", "PSEUDO"): 3, ("ccc", "DEFERRED"): 1,
            ("ddd", "NOT-IMPL-CN"): 4}
    hits = {("aaa", "MOCK-ONLY"): ["x:1", "x:2"],                      # clean
            ("bbb", "PSEUDO"): ["y:1", "y:2", "y:3", "y:4"],          # grown
            ("ccc", "DEFERRED"): [],                                    # stale
            ("ddd", "NOT-IMPL-CN"): ["w:1"],                           # rotate
            ("eee", "SIMPLIFIED-CN"): ["v:1"]}                          # new
    new, grown, rotate, stale = decide(hits, base)
    for label, got, want in (("new", [k for k, _, _ in new], [("eee", "SIMPLIFIED-CN")]),
                             ("grown", [k for k, _, _ in grown], [("bbb", "PSEUDO")]),
                             ("rotate", [k for k, _, _ in rotate], [("ddd", "NOT-IMPL-CN")]),
                             ("stale", [k for k, _, _ in stale], [("ccc", "DEFERRED")])):
        if got != want:
            print("[SELFTEST] %s verdict %r want %r" % (label, got, want))
            ok = False
    # malformed baseline must be a hard error, never a silent widening
    tmp = tempfile.mkdtemp(prefix="maturity-selftest-")
    try:
        bad = os.path.join(tmp, "bad.txt")
        with open(bad, "w", encoding="utf-8") as fh:
            fh.write("aaa.MOCK-ONLY = two\n")
        try:
            load_baseline(bad)
            print("[SELFTEST] malformed baseline was accepted")
            ok = False
        except ValueError:
            pass
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    if ok:
        print("[SELFTEST] %d/%d scored label classes all covered + %d "
              "refutation/noise classes suppressed;"
              % (len(covered), len(BLOCKING), n_neg))
        print("           all four ratchet verdicts produced (new/grown/rotate/stale);"
              " malformed baseline rejected")
        return 0
    return 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--emit", action="store_true", help="dump baseline + full locations")
    ap.add_argument("--root", help="scan another repo root (used by the smoke test)")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if args.root:
        set_root(os.path.abspath(args.root))
    if not os.path.isdir(CRATES):
        print("[CONFIG] no crates/ under %s -- wrong working dir?" % os.path.dirname(CRATES))
        return 2
    hits, advisory = scan_all()
    if args.emit:
        # --emit runs before the baseline is read on purpose: it is the path that
        # produces a baseline for a fresh repo, so it must not require one.
        emit_baseline(hits)
        for (crate, label), locs in sorted(hits.items()):
            for loc in locs:
                print("#   %s" % loc)
        print("# advisory (not scored): %d location(s)" % len(advisory))
        return 0
    try:
        baseline = load_baseline()
    except ValueError as exc:
        print("[CONFIG] %s: %s" % (BASELINE, exc))
        return 2

    new, grown, rotate, stale = decide(hits, baseline)
    total = sum(len(v) for v in hits.values())
    crates_hit = len({c for c, _ in hits})
    print("[INFO] self-declared WIP markers in crates/*/src: %d across %d crate(s); "
          "baseline rows %d; advisory locations %d"
          % (total, crates_hit, len(baseline), len(advisory)))
    if not any((new, grown, rotate, stale)):
        print("[OK] maturity ratchet honoured (no new marker, no growth, "
              "no stale allowance)")
        return 0
    status = 1
    if new or grown:
        print("[FAIL] maturity markers grew since the baseline was set:")
        for (crate, label), meas, allowed in new + grown:
            print("  [GAP] %s.%s = %d (allowed %d)" % (crate, label, meas, allowed))
            for loc in hits[(crate, label)][:5]:
                print("        %s" % loc)
        print("  Fix: finish the implementation, or record the debt in")
        print("       scripts/impl_maturity_baseline.txt WITH a reason -- the")
        print("       baseline is a ledger, not a place to hide new fakes.")
    if rotate:
        print("[ROTATE] %d row(s) shrank; refresh the baseline numbers:" % len(rotate))
        for (crate, label), meas, allowed in rotate:
            print("  [ROTATE] %s.%s %d -> %d" % (crate, label, allowed, meas))
    if stale:
        print("[FAIL] %d baseline row(s) measure zero -- delete them:" % len(stale))
        for (crate, label), _meas, allowed in stale:
            print("  [STALE] %s.%s (allowed %d, measured 0)" % (crate, label, allowed))
    return status


if __name__ == "__main__":
    import gate_rc  # entry-only: a crash must exit 2, never borrow 1 to fake "judged and red"
    sys.exit(gate_rc.run(main))
