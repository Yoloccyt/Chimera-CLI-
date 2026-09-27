#!/usr/bin/env python3
"""Derived architecture views: layer banding, domain grouping, dependency graph.

WHY THIS EXISTS
`CHIMERA_架构减法与职责重组设计方案_v2.30.md §2.2` rules out a hand-maintained
second topology (this repo has three documented deaths of that shape: the
.sh/.ps1 predicate fork, the retired `event_types.rs` mirror, and CODE_WIKI
symbol lists with 0 workspace hits) and instead promises the view is EMITTED
from `LAYER_MAP` + `cargo metadata`. Until now the doc itself carried an ASCII
diagram -- i.e. the very hand-written artifact policy said not to keep.

Single-derivation rule: this script imports the layer parser from
`crate_contract.layer_map()` and the edge set from
`check_contract_consumers_parity.metadata_internal_edges()` (cargo's own read).
It does NOT grow a third manifest parser -- that is the class of bug that made
the CONSUMERS face disagree with cargo this same week.

Views (`--emit`):
  layers     every layer with its crates and counts (authoritative listing)
  domains    the four responsibility bands, derived from contiguous layer ranges
  dot        Graphviz DOT of the non-optional internal assembly graph, labelled
             with layer numbers, upward edges coloured (iron-law violations)
  report     one-line totals for doc cross-checking
  contracts  per-layer role + interface contract rollup, read from each crate's own
             CRATE-CONTRACT block (BACKEND / PRODUCERS / CONSUMERS / MATURITY / ROLE);
             a member with no block still gets a NO-CONTRACT line -- never dropped
  svg        the architecture diagram (layer bands x crate chips + domain rail +
             inter-domain edge matrix) for human reading; derived from the SAME
             `layers`/`edges` as every other view, so "draw the diagram" cannot
             become the hand-maintained second topology the design doc forbids

`--write` renders every view into docs/architecture/views/ and `--verify` byte-compares
what is on disk against the same renderer (gate: G-70). Content carries NO timestamp so
equality is achievable; the generated banner uses each format's own comment syntax.

Checks (`--check`, exit dialect 0/1/2):
  C1 every member resolves to a layer (no silent drop)
  C2 band + layer counts close against the member set (cardinality closure)
  C3 no upward edge  L(consumer) < L(provider)  in the non-optional assembly face
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import crate_contract as cc                                  # noqa: E402
from check_contract_consumers_parity import (                 # noqa: E402
    metadata_internal_edges, parser_internal_edges)

# Responsibility bands: contiguous layer ranges, no new concept (design doc §2.3).
# Order matters only for display; membership is decided by the layer number alone.
DOMAINS = (("D0", "契约基座", 0, 1),
           ("D1", "认知与知识", 2, 5),
           ("D2", "路由与治理", 6, 8),
           ("D3", "编排与接口", 9, 10))


def domain_of(layer):
    """Band id for a layer number, or None when it falls outside every band."""
    for name, _label, lo, hi in DOMAINS:
        if lo <= layer <= hi:
            return name
    return None


def group_by_layer(layers):
    """{layer:int -> sorted [crate]} and {crate -> layer} for the member set."""
    bands = {}
    for crate, layer in layers.items():
        bands.setdefault(layer, []).append(crate)
    return {k: sorted(v) for k, v in bands.items()}


def upward_edges(edges, layers):
    """Iron-law violations: a non-optional edge pointing at a HIGHER layer."""
    out = []
    for consumer, provider in sorted(edges):
        lc, lp = layers.get(consumer), layers.get(provider)
        if lc is None or lp is None:
            continue
        if lc < lp:
            out.append((consumer, lc, provider, lp))
    return out


def domain_edge_matrix(edges, layers):
    """{(from_band, to_band) -> count} plus the unmapped bucket, for closure checks."""
    mat = {}
    for consumer, provider in edges:
        a = domain_of(layers.get(consumer, -1))
        b = domain_of(layers.get(provider, -1))
        key = (a or "?", b or "?")
        mat[key] = mat.get(key, 0) + 1
    return mat


def _render_layers(layers, edges):
    bands = group_by_layer(layers)
    out = ["L%-2d (%2d) %s" % (layer, len(bands[layer]), " · ".join(bands[layer]))
           for layer in sorted(bands)]
    out.append("total crates=%d across %d layers" % (len(layers), len(bands)))
    return out


def _render_domains(layers, edges):
    mat = domain_edge_matrix(edges, layers)
    out = []
    for name, label, lo, hi in DOMAINS:
        members = [c for c, l in layers.items() if lo <= l <= hi]
        internal = sum(v for (a, b), v in mat.items() if a == name and b == name)
        out.append("%s %-12s L%d-L%-2d crates=%-3d 域内边=%d"
                   % (name, label, lo, hi, len(members), internal))
    out.append("域间边:")
    for (a, b), v in sorted(mat.items()):
        if a != b:
            out.append("  %s->%s %d" % (a, b, v))
    return out


def _render_dot(layers, edges):
    out = ["digraph chimera {",
           "  rankdir=BT;",
           "  node [shape=box,fontname=\"Helvetica\"];"]
    for crate, layer in sorted(layers.items(), key=lambda kv: (-kv[1], kv[0])):
        out.append("  \"%s\" [label=\"%s\\nL%d\"];" % (crate, crate, layer))
    bad = {(c, p) for c, _lc, p, _lp in upward_edges(edges, layers)}
    for consumer, provider in sorted(edges):
        attr = " [color=red,penwidth=2.0]" if (consumer, provider) in bad else ""
        out.append("  \"%s\" -> \"%s\"%s;" % (consumer, provider, attr))
    out.append("}")
    return out


def _render_report(layers, edges):
    bands = group_by_layer(layers)
    return ["[REPORT] members=%d layers=%d domains=%d edges=%d upward=%d"       
            % (len(layers), len(bands), len(DOMAINS), len(edges),
               len(upward_edges(edges, layers)))]


# ============================================================
# svg view -- the human-readable architecture diagram
# ============================================================
# WHY generated and not drawn by hand: `CHIMERA_架构减法与职责重组设计方案_v2.30.md`
# 2.2 abolishes hand-maintained topologies (three documented deaths of that shape in
# this repo). A picture of the architecture is the single most copy-paste-prone
# artifact there is, so it is emitted from the same `layers`/`edges` the other views
# use and locked by the same byte-exact `--verify` (G-70).
SVG_W = 1240
SVG_MARGIN = 28
SVG_ROW_H = 56
SVG_ROW_GAP = 6
SVG_TOP = 172
SVG_RAIL_W = 300
# Per-band tint; the set is fixed by DOMAINS, so a new band means a new colour here.
BAND_FILL = {"D0": "#eef2ff", "D1": "#ecfdf5", "D2": "#fff7ed", "D3": "#eff6ff"}
BAND_LINE = {"D0": "#c7d2fe", "D1": "#a7f3d0", "D2": "#fed7aa", "D3": "#bfdbfe"}


def _esc(text):
    """XML text escaping -- crate names are [a-z0-9-] today, but the emitter must not
    depend on that staying true."""
    return (text.replace("&", "&amp;").replace("<", "&lt;")
                .replace(">", "&gt;").replace('"', "&quot;"))


def _svg_chip(x, y, text, fill="#ffffff", stroke="#d1d5db"):
    """One crate chip: a rounded rect sized to the label, left-aligned at `x`."""
    w = 14 + int(len(text) * 7.4)
    return [
        '  <rect x="%d" y="%d" width="%d" height="30" rx="6" fill="%s" '
        'stroke="%s"/>' % (x, y, w, fill, stroke),
        '  <text x="%d" y="%d" font-size="13" fill="#111827">%s</text>'
        % (x + 7, y + 20, _esc(text)),
    ]


def _render_svg(layers, edges):
    bands = group_by_layer(layers)
    ordered = sorted(bands)
    layer_rows = sorted(ordered, reverse=True)  # L10 on top, L0 (contracts) at bottom
    row_y = {layer: SVG_TOP + i * (SVG_ROW_H + SVG_ROW_GAP)
             for i, layer in enumerate(layer_rows)}
    rail_x = SVG_W - SVG_MARGIN - SVG_RAIL_W
    band_x0, band_x1 = SVG_MARGIN + 56, rail_x - 20
    body_h = len(layer_rows) * (SVG_ROW_H + SVG_ROW_GAP)
    mat = domain_edge_matrix(edges, layers)
    up = upward_edges(edges, layers)
    total_h = SVG_TOP + body_h + 208

    out = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 %d %d" '
           'width="%d" height="%d" font-family="Helvetica, Arial, sans-serif">'
           % (SVG_W, total_h, SVG_W, total_h),
           '  <desc>GENERATED artifact. Regenerate with '
           '`python scripts/emit_architecture_views.py --write`; `--verify` fails on '
           'any hand edit. Numbers below come from `layers.txt` / `domains.txt` / '
           '`dependency.dot` in this same directory.</desc>',
           '  <rect x="0" y="0" width="%d" height="%d" fill="#ffffff"/>'
           % (SVG_W, total_h),
           '  <text x="%d" y="46" font-size="24" font-weight="bold" fill="#111827">'
           'Chimera CLI (NEXUS-OMEGA) architecture — emitted view</text>' % SVG_MARGIN,
           '  <text x="%d" y="72" font-size="13" fill="#4b5563">%d crates · %d layers '
           '· %d domains · %d non-optional internal edges · upward-edge violations '
           '(iron law) = %d</text>'
           % (SVG_MARGIN, len(layers), len(bands), len(DOMAINS), len(edges), len(up)),
           '  <text x="%d" y="92" font-size="13" fill="#4b5563">Dependencies may only '
           'point down (L(N)→L(N−1)) or sideways; cross-layer communication goes '
           'through event-bus (L1) or mcp-mesh (L10) only.</text>' % SVG_MARGIN,
           '  <line x1="%d" y1="108" x2="%d" y2="108" stroke="#e5e7eb"/>'
           % (SVG_MARGIN, SVG_W - SVG_MARGIN)]

    # Domain bands first (background), then chips on top of them.
    for name, label, lo, hi in DOMAINS:
        rows = [ly for ly in layer_rows if lo <= ly <= hi]
        if not rows:
            continue
        y0 = min(row_y[ly] for ly in rows) - 6
        y1 = max(row_y[ly] for ly in rows) + SVG_ROW_H + 6
        internal = sum(v for (a, b), v in mat.items() if a == name and b == name)
        members = sum(1 for _c, l in layers.items() if lo <= l <= hi)
        out += [
            '  <rect x="%d" y="%d" width="%d" height="%d" rx="8" fill="%s" '
            'stroke="%s"/>' % (band_x0, y0, band_x1 - band_x0, y1 - y0,
                                BAND_FILL[name], BAND_LINE[name]),
            '  <rect x="%d" y="%d" width="6" height="%d" rx="3" fill="%s"/>'
            % (band_x0 + 8, y0 + 6, y1 - y0 - 12, BAND_LINE[name]),
            '  <text x="%d" y="%d" font-size="13" font-weight="bold" fill="#374151">'
            '%s %s</text>'
            % (rail_x, y0 + 22, name, _esc(label)),
            '  <text x="%d" y="%d" font-size="12" fill="#6b7280">L%d-L%d · %d crates '
            '· %d intra-domain edges</text>'
            % (rail_x, y0 + 40, lo, hi, members, internal),
        ]

    for layer in layer_rows:
        y = row_y[layer]
        out.append('  <rect x="%d" y="%d" width="56" height="32" rx="6" '
                   'fill="#f3f4f6" stroke="#d1d5db"/>' %
                   (SVG_MARGIN, y + 12))
        out.append('  <text x="%d" y="%d" font-size="14" font-weight="bold" '
                   'fill="#374151">L%d</text>' % (SVG_MARGIN + 14, y + 34, layer))
        x = band_x0 + 26
        for crate in bands[layer]:
            out += _svg_chip(x, y + 13, crate)
            x += 14 + int(len(crate) * 7.4) + 10

    # Footer: inter-domain edges (the aggregate the chips cannot show) + provenance.
    foot_y = SVG_TOP + body_h + 26
    out.append('  <text x="%d" y="%d" font-size="13" font-weight="bold" '
               'fill="#374151">Inter-domain edges</text>' % (SVG_MARGIN, foot_y))
    dy = foot_y + 20
    for (a, b), v in sorted(mat.items()):
        if a != b:
            out.append('  <text x="%d" y="%d" font-size="12" fill="#4b5563">'
                       '%s→%s %d</text>' % (SVG_MARGIN, dy, a, b, v))
            dy += 18
    out.append('  <text x="%d" y="%d" font-size="12" fill="#6b7280">'
               'Full edge list: dependency.dot · per-crate BACKEND/MATURITY: '
               'layer_contracts.txt (all generated in scripts/emit_architecture_views.py '
               '--write; --verify byte-compares)</text>'
               % (SVG_MARGIN + 220, foot_y + 20))
    out.append('</svg>')
    return out


def _count_prefix(value):
    """`PRODUCERS: 6 A,B,C…` -> `6`. The header stores the count first precisely so a
    rollup can print one number without re-parsing the name list."""
    head = value.split(" ", 1)[0]
    return head if head.isdigit() else "?"


def _render_contracts(layers, edges):
    """Per-layer responsibility + interface-contract rollup, read from each crate's own
    CRATE-CONTRACT block via `crate_contract.parse_block` (no second copy of that grammar).

    WHY one line per member even when a crate has no contract: dropping it would make the
    artifact look complete while being silent about the gap -- the same failure this repo
    already recorded for "零 Stub" (macro-name scan) and for hand-listed symbol tables.
    """
    out = []
    for layer in sorted(set(layers.values())):
        for crate in sorted(c for c, l in layers.items() if l == layer):
            fields, problem = cc.parse_block(crate)
            if problem:
                out.append("L%-2d %-22s NO-CONTRACT (%s)" % (layer, crate, problem))
                continue
            out.append("L%-2d %-22s BACKEND=%-8s PROD=%-3s CONS=%-3s MATURITY=%-5s :: %s"
                       % (layer, crate, fields["BACKEND"], _count_prefix(fields["PRODUCERS"]),
                          _count_prefix(fields["CONSUMERS"]), fields["MATURITY"],
                          fields["ROLE"]))
    return out


# view -> (file on disk, comment token). The banner is a comment in the artifact's
# OWN syntax so the file stays parseable by its own consumer (dot -T ... still works).
VIEWS = (("layers", _render_layers, "layers.txt", "#"),
         ("domains", _render_domains, "domains.txt", "#"),
         ("dot", _render_dot, "dependency.dot", "//"),
         ("report", _render_report, "report.txt", "#"),
         ("contracts", _render_contracts, "layer_contracts.txt", "#"),
         ("svg", _render_svg, "architecture.svg", "<!--"))
RENDERERS = {name: fn for name, fn, _f, _c in VIEWS}
COMMENT_TOKEN = {name: tok for name, _fn, _f, tok in VIEWS}
BANNER = "GENERATED by scripts/emit_architecture_views.py --write -- do NOT hand-edit."


def _banner_line(view):
    """Banner in the artifact's OWN comment syntax.

    WHY not the naive `token + BANNER` for every view: XML forbids `--` inside
    comments, so an SVG comment carrying the verbatim banner (which contains two
    `--write`-style runs) would make the file unparseable. The SVG banner therefore
    folds each `--` run to a single `-` and closes the comment on the same line; the
    exact regenerate command stays available verbatim in the file's <desc> element.
    For `#` / `//` tokens the output is byte-identical to the previous inline form,
    so the existing views keep their byte-exact --verify contract.
    """
    tok = COMMENT_TOKEN[view]
    if tok == "<!--":
        return "<!-- %s -->" % BANNER.replace("--", "-")
    return "%s %s" % (tok, BANNER)


def render(view, layers, edges):
    """One text builder for every path (stdout, --write, --verify) -- no second copy."""
    fn = RENDERERS.get(view)
    if fn is None:
        return None
    return "\n".join([_banner_line(view)] + fn(layers, edges)) + "\n"


def views_dir():
    # cc.ROOT (not a local constant) so --selftest's set_root() redirection applies
    return os.path.join(cc.ROOT, "docs", "architecture", "views")


def write_views(layers, edges, out_dir=None):
    d = out_dir or views_dir()
    os.makedirs(d, exist_ok=True)
    for view, _fn, fname, _tok in VIEWS:
        with open(os.path.join(d, fname), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(render(view, layers, edges))
    print("[WRITE] %d 份派生视图已落 %s" % (len(VIEWS), d))
    return 0


def verify_views(layers, edges, out_dir=None):
    """Byte-compare the on-disk artifacts against the derived text.

    WHY byte-exact and not "close enough": a hand-touched topology file is exactly the
    artifact this script exists to abolish. Anything softer than equality re-opens the
    drift this repo already recorded three times.
    """
    d = out_dir or views_dir()
    problems = []
    for view, _fn, fname, _tok in VIEWS:
        path = os.path.join(d, fname)
        if not os.path.exists(path):
            raise ValueError("derived view missing (%s) -- run --write" % path)
        with open(path, encoding="utf-8", newline="") as fh:
            disk = fh.read().replace("\r\n", "\n")
        want = render(view, layers, edges)
        if disk != want:
            wl, dl = want.splitlines(), disk.splitlines()
            at = next((i for i in range(max(len(wl), len(dl)))
                       if (wl[i] if i < len(wl) else None) !=
                          (dl[i] if i < len(dl) else None)), 0)
            problems.append("[GAP-V] %s 第 %d 行与派生结果不一致（派生物不得手改，跑 --write）"
                            % (fname, at + 1))
    for p in problems:
        print(p)
    if problems:
        print("[FAIL] %d 份视图已脱离派生源" % len(problems), file=sys.stderr)
        return 1
    print("[OK] %d 份派生视图与 --emit 逐字等值" % len(VIEWS))
    return 0


def emit(layers, edges, view, crates_dir):
    text = render(view, layers, edges)
    if text is None:
        print("usage: --emit {%s}" % ",".join(RENDERERS), file=sys.stderr)
        return 2
    sys.stdout.write(text)
    return 0


def check(layers, edges):
    """The three derived invariants; returns 0/1 plus a printed verdict list."""
    problems = []
    members = set(layers)
    # C1: layer resolution is complete for crates that exist on disk
    if os.path.isdir(crates_dir_of()):
        on_disk = {d for d in os.listdir(crates_dir_of())
                   if os.path.exists(os.path.join(crates_dir_of(), d, "Cargo.toml"))}
        missing = sorted(on_disk - members)
        if missing:
            problems.append("[C1] crates on disk with no layer assignment: %s"
                            % ", ".join(missing))
    # C2: cardinality closure -- band counts must sum to the member set exactly
    bsum = sum(1 for _c, l in layers.items() if domain_of(l))
    if bsum != len(members):
        outside = sorted(c for c, l in layers.items() if not domain_of(l))
        problems.append("[C2] band sum %d != members %d (outside bands: %s)"
                        % (bsum, len(members), ", ".join(outside) or "-"))
    # C3: dependency iron law on the assembly face
    for consumer, lc, provider, lp in upward_edges(edges, layers):
        problems.append("[C3] upward edge %s(L%d) -> %s(L%d)"
                        % (consumer, lc, provider, lp))
    for p in problems:
        print(p)
    if problems:
        print("[FAIL] derived views disagree with the iron law", file=sys.stderr)
        return 1
    print("[OK] every member is layered, band counts close, no upward edge "
          "among %d non-optional internal edges" % len(edges))
    return 0


_CRATES_DIR = None


def crates_dir_of():
    return _CRATES_DIR


def _fixture():
    """5-crate workspace + a layer map with a deliberate upward edge planted
    outside the graph, so the checker is proven to fire, not just to pass."""
    import tempfile
    import check_contract_consumers_parity as par
    tmp, _want = par._fixture()
    global _CRATES_DIR
    _CRATES_DIR = os.path.join(tmp, "crates")
    os.makedirs(os.path.join(tmp, "scripts"), exist_ok=True)
    with open(os.path.join(tmp, "scripts", "check_dependency_rules.sh"),
              "w", encoding="utf-8") as fh:
        fh.write('layer_of() {\n    case "$1" in\n        alpha) echo 9 ;;\n'
                 '        beta) echo 2 ;;\n        gamma) echo 3 ;;\n'
                 '        delta) echo 6 ;;\n        epsilon) echo 5 ;;\n'
                 '        *) echo "" ;;\n    esac\n}\n')
    cc.set_root(tmp)
    par_edges, _scoped, err = metadata_internal_edges(tmp)
    parsed, index = parser_internal_edges(tmp)
    layers = cc.layer_map()
    return tmp, layers, par_edges, parsed, index, err


def selftest():
    tmp = None
    try:
        tmp, layers, meta, parsed, index, err = _fixture()
        if err:
            print("[UNDECIDABLE] fixture metadata: %s" % err)
            return 2
        checks = [
            ("fixture layered every member", len(layers) == 5),
            ("fixture edges agree between the two methods", meta == parsed),
            ("clean fixture passes --check", check(layers, meta) == 0),
        ]
        # C3 teeth: epsilon(L5) -> delta(L6) points UP the layer stack; must be caught.
        bad_edges = meta | {("epsilon", "delta")}
        ups = upward_edges(bad_edges, layers)
        checks.append(("planted upward edge caught by --check",
                       ("epsilon", 5, "delta", 6) in ups))
        checks.append(("check rejects upward edge", check(layers, bad_edges) == 1))
        # C2 teeth: a crate outside every band breaks cardinality closure.
        broken = dict(layers)
        broken["zeta"] = 99
        checks.append(("band-count closure catches an out-of-band crate",
                       check(broken, meta) == 1))
        # C1 teeth: a crate on disk with no layer assignment.
        os.makedirs(os.path.join(crates_dir_of(), "orphan", "src"), exist_ok=True)
        with open(os.path.join(crates_dir_of(), "orphan", "Cargo.toml"),
                  "w", encoding="utf-8") as fh:
            fh.write('[package]\nname = "orphan"\n')
        checks.append(("unlayered crate on disk is reported",
                       check(layers, meta) == 1))
        for view, _fn, _fname, _tok in VIEWS:
            checks.append(("emit %s returns 0" % view, emit(layers, meta, view, None) == 0))
        # 契约汇总的基数闭合：每个成员**恰好一行**（有约出契约行，无约出 NO-CONTRACT 行）。
        # 静默丢掉无契约的 crate 会让这张表看起来完整——那正是"零 Stub 只扫宏名"的老病。
        contract_rows = [l for l in _render_contracts(layers, meta) if l.startswith("L")]
        checks.append(("contracts 视图行数与成员数闭合（无静默丢弃）",
                       len(contract_rows) == len(layers)))
        checks.append(("contracts 视图每行都是契约行或 NO-CONTRACT 行",
                       all(("NO-CONTRACT" in l) or ("ROLE" in l or "::" in l)
                           for l in contract_rows)))
        # 派生工件的四态。write/verify 都走同一个 render()，所以这里测的是"落盘物是否
        # 仍是派生的"这条新契约本身 —— 手改必须红，缺文件必须 2 而不是 0。
        vd = views_dir()
        rc_w = write_views(layers, meta, vd)
        rc_v = verify_views(layers, meta, vd)
        checks.append(("--write 后 --verify 判绿", rc_w == 0 and rc_v == 0))
        # svg 视图的牙齿：必须是良构 XML。XML 注释里禁止出现 `--`，而 banner 原文带两个
        # `--` 序列 —— 这条断言守的正是 _banner_line() 的转义那一行（转义一回归，
        # 视图仍会写盘、但任何 XML 消费者都会拒收，属于静默半死形态）。
        import xml.dom.minidom
        try:
            xml.dom.minidom.parseString(
                open(os.path.join(vd, "architecture.svg"),
                     encoding="utf-8").read())
            svg_well_formed = True
        except Exception:  # noqa: BLE001 -- any parse failure is the same red
            svg_well_formed = False
        checks.append(("svg 视图是良构 XML（banner 注释转义的牙齿）", svg_well_formed))
        first = {f: open(os.path.join(vd, f), encoding="utf-8", newline="").read()
                 for _v, _fn, f, _t in VIEWS}
        write_views(layers, meta, vd)
        second = {f: open(os.path.join(vd, f), encoding="utf-8", newline="").read()
                  for _v, _fn, f, _t in VIEWS}
        # 无时间戳/无随机序：带时间戳的"派生物"每次 --write 都会自我判红，等于没有门
        checks.append(("连续两次 --write 逐字节相同（内容确定性）", first == second))
        dot_path = os.path.join(vd, "dependency.dot")
        with open(dot_path, encoding="utf-8", newline="") as fh:
            original_dot = fh.read()
        checks.append(("dot 视图首行是自己语法的注释（仍可被 dot 解析）",
                       original_dot.startswith("// " + BANNER)))
        with open(dot_path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(original_dot + "  // 手改：加了个不存在的节点\n")
        rc_edit = verify_views(layers, meta, vd)
        checks.append(("手改派生视图判红", rc_edit == 1))
        with open(dot_path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(original_dot)
        checks.append(("还原后重新判绿（红不是永久的）", verify_views(layers, meta, vd) == 0))
        os.remove(os.path.join(vd, "layers.txt"))
        try:
            verify_views(layers, meta, vd)
            raised = False
        except ValueError:
            raised = True
        checks.append(("视图缺失走 ValueError(退 2 而非退 0)", raised))
        ok = True
        for name, passed in checks:
            print("  [%s] %s" % ("PASS" if passed else "FAIL", name))
            ok = ok and passed
        print("RESULT: %s (%d/%d assertion(s) violated)"
              % ("PASS" if ok else "FAIL", sum(1 for _n, p in checks if not p),
                 len(checks)))
        return 0 if ok else 1
    finally:
        if tmp:
            import shutil
            shutil.rmtree(tmp, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--emit", choices=tuple(RENDERERS))
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--write", action="store_true",
                    help="regenerate docs/architecture/views/* from the derived text")
    ap.add_argument("--verify", action="store_true",
                    help="byte-compare those artifacts against the derived text")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    global _CRATES_DIR
    _CRATES_DIR = os.path.join(cc.ROOT, "crates")
    layers = cc.layer_map()
    edges, scoped, err = metadata_internal_edges(cc.ROOT)
    if err:
        print("[INFO] %s" % err, file=sys.stderr)
        return 2
    if not layers:
        print("[CONFIG] layer_of() parsed to 0 arms from %s" % cc.LAYER_SCRIPT,
              file=sys.stderr)
        return 2
    if args.write:
        return write_views(layers, edges)
    if args.verify:
        try:
            return verify_views(layers, edges)
        except ValueError as exc:
            print("[CONFIG] %s" % exc, file=sys.stderr)
            return 2
    if args.emit:
        return emit(layers, edges, args.emit, _CRATES_DIR)
    rc = check(layers, edges)
    if scoped:
        print("[UNCOVERED] %d target-scoped internal edge(s) excluded from the "
              "assembly face" % len(scoped))
        rc = max(rc, 1)
    return rc


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
