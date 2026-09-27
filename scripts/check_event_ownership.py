#!/usr/bin/env python3
# =============================================================================
# check_event_ownership.py - "每个 NexusEvent 变体都得有人真发得出来" gate (B3 前置)
# =============================================================================
# Purpose: `NexusEvent` has 146 variants and NO gate asserts that any of them
#          still has a live producer. That is how a crate retirement can leave
#          behind event contracts nobody can ever publish again: the enum entry,
#          its registry row, its topic arm and its consumer arms all keep
#          compiling, so the whole surface reads "healthy" while the event is
#          unreachable. ADR-188 决策 1 registered six such variants by hand; this
#          gate makes the class measurable and only-decreasing so the next
#          retirement cannot add a seventh silently.
#
# Two authorities are cross-checked before anything is judged (cardinality
# closure, not a single parser):
#   * crates/event-bus/src/types.rs    `pub enum NexusEvent { ... }`  (the variants)
#   * crates/event-bus/src/registry.rs `Variant => Severity, Topic;`  (one row each)
#   The compiler already forces these two sets to agree, so a mismatch here means
#   THIS PARSER IS WRONG -> exit 2 (undecidable), never a green run on a subset.
#
# Scored shapes (a Rust enum arm and a Rust constructor look nearly identical,
# so the distinction is made from three signals, not one regex):
#   ctor     `NexusEvent::V {` / `NexusEvent::V(`  AND the line has no `=>`
#            AND (for braced forms) the body has no `..` and does contain
#            `metadata:` -- every variant's first field is `metadata`, so a
#            construction must supply it while a partial pattern may elide it.
#   pattern  `=>` on the line, or `matches!`, or a `..` in the body.
#   test     a line range owned by `#[cfg(test)] mod x { .. }` or by a function
#            carrying `#[test]` / `#[tokio::test]` / `#[test_log::test]`, plus
#            everything under `crates/*/tests/`, `crates/*/benches/`, `tests/`.
#            WHY this matters: event-bus/tests/variant_count_test.rs constructs
#            ALL 146 variants as fixtures. A scan that cannot tell test scope
#            apart reports "every event has a producer" -- a 146/146 green that
#            proves nothing (measured that way on 2026-09-23 with a looser rule).
#
# Verdicts:
#   UNOWNED-PRODUCER  no production ctor anywhere -> needs a ledger row naming
#                     the evidence; only-decrease.
#   DEAD-EVENT        no production ctor AND no pattern and no test ctor at all
#                     -> always red; a ledger row cannot buy it back, because the
#                     right fix is either to wire a producer or to retire the
#                     contract under an ADR (this objective forbids the latter).
#   STALE             a ledger row whose variant gained a real producer again
#                     (the allowance outlived the debt) or left the enum.
#
# WHAT "no production ctor" DOES NOT mean: `NexusEvent` derives
# `Serialize + Deserialize` with `#[serde(tag = "type")]` (types.rs:26-28), so a
# variant can still be materialised by an INBOUND wire frame even when no code in
# this repository ever builds it. So this gate measures the **in-repo emit surface**
# (谁能在这个进程里发出来), NOT "is this event reachable at all". Reading a GAP-E
# row as "delete this event" would be wrong: it may be a wire-only inbound contract.
#
# Baseline: scripts/event_ownership_baseline.txt, one `Variant  # evidence` per
#   line. Exit dialect shared with gate_rc: 0 clean / 1 judged red / 2 undecidable.
#
# Usage:
#   python scripts/check_event_ownership.py --selftest
#   python scripts/check_event_ownership.py --emit
#   python scripts/check_event_ownership.py
# =============================================================================
import argparse
import os
import re
import shutil
import sys
import tempfile

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPTS)
TYPES = os.path.join(ROOT, "crates", "event-bus", "src", "types.rs")
REGISTRY = os.path.join(ROOT, "crates", "event-bus", "src", "registry.rs")
BASELINE = os.path.join(ROOT, "scripts", "event_ownership_baseline.txt")

ENUM_HEAD = re.compile(r"^pub enum NexusEvent\s*\{")
VAR_LINE = re.compile(r"^    ([A-Z][A-Za-z0-9]*)\s*[,{]\s*$")
REG_LINE = re.compile(r"^\s+([A-Z][A-Za-z0-9]*)\s*=>")
TEST_ATTR = re.compile(r"^#!?\[\s*(tokio::|test_log::)?test(_log)?\s*\]\s*$")
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]\s*$")
MOD_HEAD = re.compile(r"^\s*(?:pub\s+)?mod\s+\w+\s*\{")
FN_HEAD = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+\w+")
USE_LINE = re.compile(r"^\s*(?:pub\s+)?use\b")
# `=>` 只有在变体名**之后**才是匹配臂；在之前它是 match 分支的**返回构造**
# （`"ModelRouteSelected" => NexusEvent::ModelRouteSelected {`），旧版按整行判把这类真构造丢了。
LET_TAIL = re.compile(r"(?:if|while)\s+let\s+(?:[A-Za-z_]\w*\s*=\s*)?(?:ref\s+|mut\s+|@\s*\w+\s+)*$")
INSIDE_MATCHES = re.compile(r"matches!\s*\((?:(?!\)).)*$")
# 单元变体的构造没有 `{`/`(` 可依赖，要求左侧是表达式上下文；`=>` 必须算，
# 因为 `"Name" => NexusEvent::Unit` 这类 match 分支体返回的是**构造**而非模式。
CTX_TAIL = re.compile(r"(?:=>|->|=|\(|,|\[|return)\s*$")
SKIP_DIR_PARTS = ("target", ".git", ".toolchain", "node_modules")


def set_root(path):
    """Re-point every path at another repo root (used by --selftest fixtures)."""
    global ROOT, TYPES, REGISTRY, BASELINE
    ROOT = path
    SCRIPT = os.path.join(path, "crates", "event-bus", "src")
    TYPES = os.path.join(SCRIPT, "types.rs")
    REGISTRY = os.path.join(SCRIPT, "registry.rs")
    BASELINE = os.path.join(path, "scripts", "event_ownership_baseline.txt")


# --- authorities -------------------------------------------------------------

def enum_variants(path=None):
    """Variant names declared by `pub enum NexusEvent`, struct variants included."""
    out = []
    inside = False
    for line in _read(path or TYPES).splitlines():
        if not inside:
            inside = bool(ENUM_HEAD.match(line))
            continue
        if line.startswith("}"):
            break
        m = VAR_LINE.match(line)
        if m:
            out.append(m.group(1))
    return out


def registry_variants(path=None):
    """Variant names listed by `define_event_registry!` (one `V => S, T;` row each)."""
    out = []
    inside = False
    for line in _read(path or REGISTRY).splitlines():
        if not inside:
            inside = "define_event_registry!" in line and line.rstrip().endswith("{")
            continue
        if line.startswith("};"):
            break
        m = REG_LINE.match(line)
        if m:
            out.append(m.group(1))
    return out


# --- surface -----------------------------------------------------------------

def _read(path):
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.read()


def _span_end(lines, start):
    """Line index just past the `{`-delimited block whose opening line is `start`."""
    depth = 0
    for i in range(start, len(lines)):
        depth += lines[i].count("{") - lines[i].count("}")
        if depth <= 0:
            return i + 1
    return len(lines)


def test_line_ranges(lines):
    """Set of 1-based line numbers owned by test scope (cfg(test) mods + test fns).

    WHY two hard guards (both fix false positives measured 2026-09-23 on the real tree):
      * an attribute must BE the whole line -- `decb-governor/src/governor.rs:325` and
        `:405` are prose comments that contain the literal `#[test]` ("...同步 #[test]
        调用;"), and matching them re-labelled `record_consumption` (which really does
        publish `BudgetStatsReported` at `:407` and `BudgetMetricsUpdated` at `:422`)
        as test scope;
      * the attribute must be followed (after blanks only) by the `fn`/`mod` it decorates
        -- scanning forward across intervening code re-labelled unrelated production code.
    """
    marks = set()
    for i, line in enumerate(lines):
        if TEST_ATTR.match(line):
            j = _next_item_line(lines, i)
            if j < len(lines) and FN_HEAD.match(lines[j]):
                marks.update(range(j + 1, _span_end(lines, j) + 1))
        elif CFG_TEST.match(line):
            j = _next_item_line(lines, i)
            if j < len(lines) and MOD_HEAD.match(lines[j]):
                marks.update(range(j + 1, _span_end(lines, j) + 1))
    return marks


ATTR_ANY = re.compile(r"^\s*#!?\[")


def _next_item_line(lines, i):
    """First non-blank line after `i`, skipping **further attribute lines**.

    WHY attributes count as the same item: the E-5 lint-ratchet batches insert
    `#[allow(clippy::unwrap_used, ...)]` between `#[cfg(test)]` and `mod x {`
    -- legal Rust decor on the same item. An adjacency-only guard silently
    re-labels those test mods as production code (measured 2026-09-25: 7 false
    GAP-S STALEs surfaced the day the batches landed). A nested cfg(test)/
    #[test] attribute stops the skip: it opens a *different* scope rather
    than decorating this one further.
    """
    j = _next_code_line(lines, i)
    while (j < len(lines) and ATTR_ANY.match(lines[j])
           and not CFG_TEST.match(lines[j]) and not TEST_ATTR.match(lines[j])):
        j = _next_code_line(lines, j)
    return j


def _next_code_line(lines, i):
    """First non-blank line index after `i` (comments stay: attributes may sit above them)."""
    j = i + 1
    while j < len(lines) and not lines[j].strip():
        j += 1
    return j


def scan_files():
    """Yield (rel_path, is_test_surface) for every .rs file worth scanning."""
    base = os.path.join(ROOT, "crates")
    for dirpath, dirs, files in os.walk(base):
        dirs[:] = [d for d in dirs if d not in SKIP_DIR_PARTS]
        rel_dir = os.path.relpath(dirpath, ROOT).replace(os.sep, "/")
        for f in sorted(files):
            if not f.endswith(".rs"):
                continue
            rel = "%s/%s" % (rel_dir, f)
            in_src = "/src/" in "/%s" % rel
            yield rel, (not in_src)
    root_tests = os.path.join(ROOT, "tests")
    if os.path.isdir(root_tests):
        for dirpath, dirs, files in os.walk(root_tests):
            dirs[:] = [d for d in dirs if d not in SKIP_DIR_PARTS]
            rel_dir = os.path.relpath(dirpath, ROOT).replace(os.sep, "/")
            for f in sorted(files):
                if f.endswith(".rs"):
                    yield "%s/%s" % (rel_dir, f), True


# --- judgement ---------------------------------------------------------------

def _record(cell, rel, idx, is_pattern, whole_file_test, tlines):
    """File one scored occurrence under its bucket, honouring test scope."""
    in_test = whole_file_test or (idx + 1) in tlines
    if is_pattern:
        if not in_test:
            cell["pattern"].append("%s:%d" % (rel, idx + 1))
    elif in_test:
        cell["test_ctor"] += 1
    else:
        cell["prod_ctor"].append("%s:%d" % (rel, idx + 1))


def census(variants):
    """{variant: {"prod_ctor": [loc..], "test_ctor": n, "pattern": [loc..]}}.

    `loc` is `rel:line` so every verdict can print what it actually opened --
    a red with no path next to it is not actionable, and a bare variant name can
    silently point at the wrong same-named file.
    """
    rx = {v: re.compile(r"NexusEvent\s*::\s*(%s)\s*([({])?" % v) for v in variants}
    tally = {v: {"prod_ctor": [], "test_ctor": 0, "pattern": []} for v in variants}
    for rel, whole_file_test in scan_files():
        lines = _read(os.path.join(ROOT, rel)).splitlines()
        tlines = test_line_ranges(lines)
        for i, line in enumerate(lines):
            s = line.strip()
            if s.startswith("//"):
                continue  # 注释/文档示例里的 `NexusEvent::V` 不是代码（真树实测会造假绿）
            if USE_LINE.match(line):
                continue  # `use ...::NexusEvent::V;` 是导入，既不构造也不消费
            for v, pat in rx.items():
                m = pat.search(line)
                if not m:
                    continue
                pre, post, opens = line[:m.start()], line[m.start():], m.group(2)
                body = "\n".join(lines[i:i + 14])
                if opens == "{":
                    upto = body.find("}")
                    body = body if upto < 0 else body[:upto + 1]
                is_pattern = (".." in body) or ("=>" in post) or \
                    bool(LET_TAIL.search(pre)) or bool(INSIDE_MATCHES.search(pre))
                if opens:
                    is_ctor = (not is_pattern) and ("metadata:" in body or opens == "(")
                else:
                    # 单元变体：`bus.publish(NexusEvent::Shutdown)` 无括号，靠左侧表达式上下文认
                    is_ctor = (not is_pattern) and bool(CTX_TAIL.search(pre))
                if is_pattern or is_ctor:
                    _record(tally[v], rel, i, is_pattern, whole_file_test, tlines)
    return tally


def load_baseline():
    """{variant: evidence}. A row without evidence is a config error -> exit 2."""
    if not os.path.exists(BASELINE):
        raise ValueError("baseline missing (%s)" % BASELINE)
    out = {}
    for raw in _read(BASELINE).splitlines():
        key, _, why = raw.partition("#")
        key = key.strip()
        if not key:
            continue
        if not why.strip():
            raise ValueError("baseline row needs trailing '# evidence': %r" % key)
        out[key] = why.strip()
    return out


def decide(tally, variants, baseline):
    """Return (unowned_unregistered, dead, stale) -- all lists of (variant, detail)."""
    vset = set(variants)
    unowned, dead, stale = [], [], []
    for v in sorted(vset):
        cell = tally[v]
        if cell["prod_ctor"]:
            if v in baseline:
                stale.append((v, "has %d production ctor(s) now, e.g. %s"
                              % (len(cell["prod_ctor"]), cell["prod_ctor"][0])))
            continue
        if not cell["pattern"] and not cell["test_ctor"]:
            dead.append((v, "no ctor, no consumer arm, no test fixture anywhere"))
        elif v not in baseline:
            detail = "prod_ctor=0 test_ctor=%d consumers=%s" % (
                cell["test_ctor"],
                ",".join(sorted({loc.split(":")[0] for loc in cell["pattern"]})) or "-")
            unowned.append((v, detail))
    for v in sorted(set(baseline) - vset):
        stale.append((v, "ledger row for a variant no longer in the enum"))
    return unowned, dead, stale


# --- selftest ----------------------------------------------------------------

def _fixture(root):
    """A mini repo whose six events exercise every verdict, one each."""
    def w(rel, text):
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as fh:
            fh.write(text)

    w("crates/event-bus/src/types.rs", """
pub enum NexusEvent {
    OwnedHere {
        metadata: EventMetadata,
    },
    PatternOnlyLikeCtor {
        metadata: EventMetadata,
    },
    TestOnlyProduced {
        metadata: EventMetadata,
    },
    ConsumedButNeverSent {
        metadata: EventMetadata,
    },
    TotalGhost {
        metadata: EventMetadata,
    },
    SyncEmit {
        metadata: EventMetadata,
    },
    ArmBodyCtor {
        metadata: EventMetadata,
    },
    AllowBetweenCfgTest {
        metadata: EventMetadata,
    },
    BareUnit,
}
""")
    w("crates/event-bus/src/registry.rs", """
macro_rules! define_event_registry {
    ($($k:ident => $s:expr, $t:expr;)*) => {};
}
define_event_registry! {
    OwnedHere => Normal, Execution;
    PatternOnlyLikeCtor => Normal, Execution;
    TestOnlyProduced => Normal, Execution;
    ConsumedButNeverSent => Normal, System;
    TotalGhost => Normal, System;
    SyncEmit => Normal, System;
    ArmBodyCtor => Normal, System;
    AllowBetweenCfgTest => Normal, System;
    BareUnit => Normal, System;
}
""")
    w("crates/alpha/src/lib.rs", """
// 注释里提到 NexusEvent::TotalGhost 一次，两侧都不该算
pub fn make() -> NexusEvent {
    NexusEvent::OwnedHere {
        metadata: md(),
    }
}

pub fn handle(e: NexusEvent) {
    match e {
        // 带 `..` 的部分模式：形状像构造，但不是生产者
        NexusEvent::PatternOnlyLikeCtor { .. } => {}
        _ => {}
    }
}

// WHY publish_blocking:本方法为同步方法,且被同步 #[test] 调用;
//   ^ 这行注释里有字面 `#[test]`——它曾把下面的生产函数整段吞进"测试域"
pub fn sync_emit(bus: &EventBus) {
    if let Err(e) = bus.publish_blocking(NexusEvent::SyncEmit {
        metadata: md(),
    }) {
        log(e);
    }
}

pub fn from_name(name: &str) -> NexusEvent {
    match name {
        "ArmBodyCtor" => NexusEvent::ArmBodyCtor {
            metadata: md(),
        },
        _ => NexusEvent::OwnedHere { metadata: md() },
    }
}
""")
    w("crates/alpha/src/bus.rs", """
pub fn consume(e: NexusEvent) -> u8 {
    match e {
        NexusEvent::ConsumedButNeverSent { metadata, .. } => metadata.len(),
        NexusEvent::OwnedHere { .. } => 1,
        ev if matches!(ev, NexusEvent::BareUnit) => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fixture_builds_it() {
        let _ = NexusEvent::TestOnlyProduced {
            metadata: md(),
        };
    }
}

// E-5 棘轮模板会在 cfg(test) 与 mod 之间插入 #[allow(...)] 属性行;
// 该形态必须仍被识别为测试域(否则夹具构造伪装成生产 ctor 报假 STALE)
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod allow_decorated {
    #[test]
    fn builds_it() {
        let _ = NexusEvent::AllowBetweenCfgTest {
            metadata: md(),
        };
    }
}
""")
    w("scripts/event_ownership_baseline.txt",
      "TestOnlyProduced  # fixture-made only\n"
      "AllowBetweenCfgTest  # fixture-made only, cfg(test) 与 mod 间有属性行\n"
      "ConsumedButNeverSent  # producer retired (ADR-188 决策 1)\n"
      "BareUnit  # 单元变体，只有 matches! 消费\n")


def selftest():
    tmp = tempfile.mkdtemp(prefix="evtown")
    real_root = ROOT
    try:
        _fixture(tmp)
        set_root(tmp)
        variants = enum_variants()
        reg = registry_variants()
        tally = census(variants)
        baseline = load_baseline()
        unowned, dead, stale = decide(tally, variants, baseline)
        u = [v for v, _d in unowned]

        checks = [
            ("两把尺基数闭合：enum 9 == registry 9 且集合相等",
             len(variants) == 9 and sorted(variants) == sorted(reg)),
            ("非空清单（空清单=判据失能，不许当成干净）", bool(variants) and bool(reg)),
            ("真生产者 OwnedHere 不在红名单", "OwnedHere" not in u),
            ("带 `..` 的部分模式不算生产者 => PatternOnlyLikeCtor 必须红",
             u == ["PatternOnlyLikeCtor"]),
            ("仅 `#[cfg(test)]` 内构造 => 已登记的 TestOnlyProduced 不红",
             "TestOnlyProduced" not in u and not tally["TestOnlyProduced"]["prod_ctor"]),
            # E-5 模板形态回归(2026-09-25:cfg(test) 与 mod 间插 #[allow] 曾致 7 假 STALE)
            ("cfg(test) 与 mod 之间夹合法属性行仍划测试域",
             "AllowBetweenCfgTest" not in u
             and not tally["AllowBetweenCfgTest"]["prod_ctor"]
             and tally["AllowBetweenCfgTest"]["test_ctor"] > 0),
            ("单元变体经 `matches!` 只算消费 => 已登记则放过",
             "BareUnit" not in u and not tally["BareUnit"]["prod_ctor"]
             and bool(tally["BareUnit"]["pattern"])),
            ("零构造+零消费+零夹具(注释不算) => 判 DEAD 且不可靠登记逃过",
             [v for v, _ in dead] == ["TotalGhost"]),
            # 两枚真树假阳性回归（2026-09-23：decb-governor / event-bus token_ledger）
            ("注释里的字面 `#[test]` 不得把生产函数划进测试域",
             bool(tally["SyncEmit"]["prod_ctor"]) and not tally["SyncEmit"]["pattern"]),
            ("match 分支体里的构造（`\"x\" => NexusEvent::V {`）算生产者",
             bool(tally["ArmBodyCtor"]["prod_ctor"])),
            ("每条生产者证据都带 file:line（红必须可点进去）",
             all(":" in loc for loc in tally["OwnedHere"]["prod_ctor"])),
            ("STALE：登记的变体重新拿到生产者后，账本行必须判失效",
             [v for v, _ in decide({"OwnedHere": {"prod_ctor": ["x.rs:1"],
                                                  "test_ctor": 0, "pattern": []}},
                                   ["OwnedHere"], {"OwnedHere": "旧债"})[2]] == ["OwnedHere"]),
            ("STALE：账本键已不在 enum 里也判失效",
             [v for v, _ in stale] == [] and
             [v for v, _ in decide(tally, ["OwnedHere"], {"TotalGhost": "x"})[2]] == ["TotalGhost"]),
        ]
        # 反证：删掉唯一真生产者，OwnedHere 必须从"干净"翻成红（门自己有牙）
        keep = _read(os.path.join(tmp, "crates/alpha/src/lib.rs"))
        _w(os.path.join(tmp, "crates/alpha/src/lib.rs"),
           keep.replace("NexusEvent::OwnedHere", "NexusEvent::RenamedAway"))
        vs = enum_variants()
        u2, _d2, s2 = decide(census(vs), vs, load_baseline())
        checks.append(("反证：抹掉真生产者后 OwnedHere 立刻判红",
                       "OwnedHere" in [v for v, _ in u2]))
        checks.append(("反证不得引入假 STALE（OwnedHere 本就没有账本行）",
                       [v for v, _ in s2] == []))
        _w(os.path.join(tmp, "crates/alpha/src/lib.rs"), keep)

        bad = [name for name, ok in checks if not ok]
        for name, ok in checks:
            print("  [%s] %s" % ("PASS" if ok else "FAIL", name))
        if bad:
            print("[SELFTEST] %d/%d FAILED" % (len(bad), len(checks)))
            return 1
        print("[SELFTEST] %d checks：基数闭合 / 构造-模式-测试态三分 / DEAD 不可登记 / "
              "STALE 双向 / 反证判红 全部验证" % len(checks))
        return 0
    finally:
        set_root(real_root)
        shutil.rmtree(tmp, ignore_errors=True)


def _w(path, text):
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


# --- entrypoint --------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--emit", action="store_true")
    ap.add_argument("--root")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if args.root:
        set_root(os.path.abspath(args.root))

    variants = enum_variants()
    reg = registry_variants()
    if not variants or not reg:
        print("[CONFIG] 解析到空清单 enum=%d registry=%d -- 判据失能，不出数"
              % (len(variants), len(reg)))
        return 2
    if sorted(variants) != sorted(reg):
        only_e = sorted(set(variants) - set(reg))
        only_r = sorted(set(reg) - set(variants))
        print("[CONFIG] 两把尺不一致 enum-only=%r registry-only=%r"
              "  -> 本探针解析有误，拒绝出数" % (only_e[:5], only_r[:5]))
        return 2

    tally = census(variants)
    owned = [v for v in variants if tally[v]["prod_ctor"]]
    unowned = [v for v in variants if not tally[v]["prod_ctor"]]

    if args.emit:
        for v in sorted(unowned):
            cell = tally[v]
            cons = sorted({loc.split(":")[0] for loc in cell["pattern"]})
            print("%s  # prod_ctor=0 test_ctor=%d consumers=%s"
                  % (v, cell["test_ctor"], ",".join(cons) or "-"))
        print("# total %d variant(s): %d owned / %d without a production ctor"
              % (len(variants), len(owned), len(unowned)))
        return 0

    try:
        baseline = load_baseline()
    except ValueError as exc:
        print("[CONFIG] %s" % exc)
        return 2
    unowned_rows, dead, stale = decide(tally, variants, baseline)
    print("[INFO] variants=%d 有生产态构造=%d 无=%d 账本=%d"
          % (len(variants), len(owned), len(unowned), len(baseline)))
    if not any((unowned_rows, dead, stale)):
        print("[OK] 每个事件契约要么有生产态构造者，要么已带证据登记")
        return 0
    rc = 1
    if unowned_rows:
        print("[GAP-E] %d 个变体没有任何生产态构造点且未登记:" % len(unowned_rows))
        for v, detail in unowned_rows:
            print("  [GAP-E] %s (%s)" % (v, detail))
        print("  修法是接一个生产者，或按 ADR-188 决策 1 带证据登记；不得静默留着。")
    if dead:
        print("[GAP-D] %d 个变体零构造、零消费、零夹具 = 真死契约(不可登记，必须处置):"
              % len(dead))
        for v, detail in dead:
            print("  [GAP-D] %s (%s)" % (v, detail))
        rc = 1
    if stale:
        print("[GAP-S] %d 行账本已失效(删行):" % len(stale))
        for v, detail in stale:
            print("  [GAP-S] %s (%s)" % (v, detail))
    return rc


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
