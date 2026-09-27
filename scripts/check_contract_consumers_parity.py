#!/usr/bin/env python3
"""Second-method parity check for the CONSUMERS face of the crate contracts.

WHY THIS EXISTS
`crate_contract.py --check` (G-59) compares an installed `CRATE-CONTRACT`
header against freshly derived facts -- but "freshly derived" uses the SAME
manifest parser (`derive_consumers` -> `check_declared_dep_usage.parse_declared`
plus a one-line `optional = true` regex). A parser bug therefore moves the
header and the check together, and the gate stays green while the data is
wrong in the same direction for all 41 crates.

This gate brings a second, independent method: `cargo metadata` is cargo's own
reading of the same manifests and reports `kind` / `optional` / `target`
per dependency row. Divergence between the two is a defect in ONE of them, and
because cargo is the definition of the assembly graph, divergence means the
contract face is wrong.

Exit dialect (scripts/gate_rc.py): 0 = parity, 1 = divergence, 2 = undecidable
(cargo metadata unavailable).

Edges compared: workspace-internal, `kind = normal`, non-optional,
NOT target-scoped. Target-scoped internal edges are counted and reported
separately: `derive_consumers` does not read `[target.*.dependencies]`, so a
non-zero count means the contract face is incomplete -- which is a finding,
not a silent exclusion.
"""

import json
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import crate_contract as cc  # noqa: E402  (sibling module, same dir)


def metadata_internal_edges(root):
    """Return (edges, target_scoped, error) as read by cargo itself.

    edges: set of (consumer_crate, provider_crate) for normal, non-optional,
    non-target-scoped dependencies between workspace members.
    """
    try:
        proc = subprocess.run(
            ["cargo", "metadata", "--offline", "--no-deps",
             "--format-version", "1"],
            cwd=root, capture_output=True, timeout=180,
            # WHY not text=True: on a zh-CN host Python defaults the child's
            # stdout/stderr to the GBK code page, and cargo metadata carries
            # non-ASCII (crate descriptions). The reader thread then raises
            # UnicodeDecodeError and stdout arrives as None -- which looks like
            # "no data" rather than "undecodable". Decode explicitly.
            encoding="utf-8", errors="replace",
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return None, None, "cargo metadata unavailable: %s" % exc
    if proc.returncode != 0:
        return None, None, "cargo metadata rc=%d: %s" % (
            proc.returncode, (proc.stderr or "").strip()[:200])
    if proc.stdout is None:
        return None, None, "cargo metadata produced no stdout"
    try:
        meta = json.loads(proc.stdout)
    except ValueError as exc:
        return None, None, "cargo metadata unparseable: %s" % exc

    members = {p["name"] for p in meta["packages"] if p.get("source") is None}
    edges, scoped = set(), []
    for pkg in meta["packages"]:
        consumer = pkg["name"]
        if consumer not in members:
            continue
        for dep in pkg.get("dependencies", []):
            if dep.get("kind") is not None:          # dev / build rows
                continue
            provider = dep["name"]
            if provider not in members:               # registry dependency
                continue
            if dep.get("target") is not None:
                scoped.append((consumer, provider, dep["target"]))
                continue
            if dep.get("optional"):                   # GATED face, not assembly
                continue
            edges.add((consumer, provider))
    return edges, scoped, None


def parser_internal_edges(root):
    """Return the same edge set through crate_contract's own manifest parsing."""
    cc.set_root(root)
    ddu = cc.ddu
    index = ddu.crates_index()
    edges = set()
    for crate_dir, pkg in index.items():
        for consumer in cc.derive_consumers(crate_dir, index):
            edges.add((consumer, pkg))
    return edges, index


def compare(meta_edges, parser_edges):
    """Split divergences into the two directional classes."""
    missing = sorted(meta_edges - parser_edges)   # cargo sees it, parser missed it
    extra = sorted(parser_edges - meta_edges)     # parser invented an edge
    return missing, extra


def _toml(crate, deps, extra_tables=""):
    lines = ["[package]", "name = \"%s\"" % crate, "version = \"0.1.0\"",
             "edition = \"2021\"", "", "[dependencies]"]
    lines.extend(deps)
    return "\n".join(lines) + extra_tables + "\n"


def _fixture():
    """Tiny workspace planting one of every edge class."""
    tmp = tempfile.mkdtemp(prefix="consumers-parity-")
    members = ["alpha", "beta", "gamma", "delta", "epsilon"]
    os.makedirs(os.path.join(tmp, "crates"))
    with open(os.path.join(tmp, "Cargo.toml"), "w", encoding="utf-8") as fh:
        fh.write("[workspace]\nresolver = \"2\"\nmembers = [%s]\n"
                 % ", ".join("\"crates/%s\"" % m for m in members))
    layout = {
        # normal internal edge -> must appear in BOTH methods
        "alpha": (_toml("alpha", ["beta = { path = \"../beta\" }"]), 1),
        # optional internal edge -> must appear in NEITHER
        "beta": (_toml("beta", [
            "gamma = { path = \"../gamma\",",
            "          optional = true }",
        ], "\n[target.'cfg(unix)'.dependencies]\nalpha = { path = \"../alpha\" }\n"), 1),
        # multi-line optional row above is the planted parser-hazard:
        # `optional = true` is NOT on the dep's first line
        "gamma": (_toml("gamma", []), 0),
        "delta": (_toml("delta", ["epsilon = { path = \"../epsilon\" }"]), 1),
        "epsilon": (_toml("epsilon", []), 0),
    }
    for crate, (body, _) in layout.items():
        d = os.path.join(tmp, "crates", crate)
        os.makedirs(os.path.join(d, "src"))
        with open(os.path.join(d, "Cargo.toml"), "w", encoding="utf-8") as fh:
            fh.write(body)
        with open(os.path.join(d, "src", "lib.rs"), "w", encoding="utf-8") as fh:
            fh.write("#![forbid(unsafe_code)]\n")
    want = {("alpha", "beta"), ("delta", "epsilon")}
    return tmp, want


def selftest():
    """Assert the fixture yields identical sets from both methods, and that the
    two excluded classes (optional, target-scoped) are excluded for the right
    reason -- i.e. cargo sees them, we route them elsewhere."""
    failures = []
    tmp, want = _fixture()
    meta, scoped, err = metadata_internal_edges(tmp)
    if err:
        print("[UNDECIDABLE] fixture metadata: %s" % err)
        return 2
    parsed, index = parser_internal_edges(tmp)
    checks = [
        ("fixture has all 5 members", len(index) == 5),
        ("cargo edges match the planted normal set", meta == want),
        ("parser edges match the planted normal set", parsed == want),
        ("multi-line optional row not counted as assembly",
         ("beta", "gamma") not in meta and ("beta", "gamma") not in parsed),
        ("target-scoped internal edge reported, not silently dropped",
         len(scoped) == 1 and scoped[0][0] == "beta" and scoped[0][1] == "alpha"),
        ("parity clean on fixture", compare(meta, parsed) == ([], [])),
    ]
    # counter-check: a parser that lost an edge must be caught
    broken = set(want)
    broken.discard(("delta", "epsilon"))
    miss, extra = compare(meta, broken)
    checks.append(("dropped edge surfaces as PARSER-MISSING",
                   miss == [("delta", "epsilon")]))
    checks.append(("invented edge surfaces as PARSER-EXTRA",
                   compare(broken, meta) == ([], [("delta", "epsilon")])))
    for label, ok in checks:
        print("[%s] %s" % ("PASS" if ok else "FAIL", label))
        if not ok:
            failures.append(label)
    print("RESULT: %s (%d/%d assertion(s) violated)"
          % ("PASS" if not failures else "FAIL", len(failures), len(checks)))
    return 0 if not failures else 1


def main():
    if "--selftest" in sys.argv[1:]:
        return selftest()
    for arg in sys.argv[1:]:
        if arg not in ("--quiet",):
            print("usage: %s [--selftest]" % os.path.basename(sys.argv[0]),
                  file=sys.stderr)
            return 2
    root = cc.ROOT
    meta, scoped, err = metadata_internal_edges(root)
    if err:
        print("[INFO] %s" % err, file=sys.stderr)
        return 2
    parsed, index = parser_internal_edges(root)
    missing, extra = compare(meta, parsed)
    print("[INFO] members=%d cargo-edges=%d parser-edges=%d "
          "target-scoped-internal=%d"
          % (len(index), len(meta), len(parsed), len(scoped)))
    bad = False
    for consumer, provider in missing:
        print("[PARSER-MISSING] %s -> %s : cargo declares it, the contract "
              "derivation does not see it" % (consumer, provider))
        bad = True
    for consumer, provider in extra:
        print("[PARSER-EXTRA] %s -> %s : the contract derivation emits an edge "
              "cargo does not" % (consumer, provider))
        bad = True
    for consumer, provider, target in scoped:
        print("[UNCOVERED] %s -> %s under [%s] dependencies: derive_consumers "
              "does not read target tables, so the CONSUMERS face is incomplete"
              % (consumer, provider, target))
        bad = True
    if bad:
        print("[FAIL] two methods disagree on the assembly face", file=sys.stderr)
        return 1
    print("[OK] cargo metadata and the contract derivation agree on every "
          "non-optional internal edge")
    return 0


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
