#!/usr/bin/env python3
# =============================================================================
# check_config_sample_parity.py - "配置样例 == 内嵌模板" parity gate (B5 残骸)
# =============================================================================
# Purpose: `agents.md §10.5` has carried a reopened defect since 2026-09-22:
#   `examples/config.sample.yaml` was 9 bytes (`# Chimera`, zero config items) and
#   `examples/config.sample.toml` told readers to copy it to `~/.aether/omega.toml`
#   -- a path, a filename AND a format the loader never reads. `config.rs:16` still
#   points at both files as "简化样例", so a new developer following the docs got an
#   empty config and a silently ignored file.
#
# The root cause is not the bad bytes, it is that the repo keeps TWO hand-written
# copies of a config that already has exactly one authoritative rendering in code:
# `omega_yaml_template()` (config.rs) -- the same string `chimera config init`
# writes to disk. A hand copy of that will always drift (this repo has three
# recorded twin-source deaths: .sh/.ps1 predicates, event_types.rs mirror, the
# layer maps). So the sample is made a DERIVED artifact and this gate is what keeps
# it derived: equality is asserted, not promised.
#
# Line endings are normalised for the comparison and REPORTED, because config.rs is
# CRLF on this host while a generated sample is written LF -- comparing raw bytes
# would make the gate red for a reason nobody can act on.
#
# Exit dialect (shared with gate_rc): 0 equal / 1 drifted / 2 undecidable
# (template or sample missing, template markers not found, unparsable shape).
#
# Three asymmetries decide red vs note, because "sample == struct" is too coarse:
#   missing SECTION (a `*Config` field)  -> red   : an entire subsystem's knobs
#   phantom key (no such field)          -> red   : the file teaches a dead setting
#   missing SCALAR (`#[serde(default)]`) -> note  : defaults cover it, only docs lose
#
# Usage:
#   python scripts/check_config_sample_parity.py --selftest
#   python scripts/check_config_sample_parity.py --write   # regenerate the sample
#   python scripts/check_config_sample_parity.py
# =============================================================================
import argparse
import io
import os
import re
import shutil
import sys
import tempfile

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(SCRIPTS)
CONFIG_RS = os.path.join(ROOT, "crates", "chimera-cli", "src", "config.rs")
TYPES_RS = os.path.join(ROOT, "crates", "nexus-core", "src", "config.rs")
SAMPLE = os.path.join(ROOT, "examples", "config.sample.yaml")

# The template is a raw string: `r#"...."#`. Non-greedy DOTALL, anchored on the fn.
TEMPLATE_RE = re.compile(
    r"fn\s+omega_yaml_template\s*\(\s*\)\s*->\s*&'static\s+str\s*\{.*?r#\"(.*?)\"#",
    re.S)


def set_root(path):
    """Re-point every path at another repo root (used by --selftest fixtures)."""
    global ROOT, CONFIG_RS, TYPES_RS, SAMPLE
    ROOT = path
    CONFIG_RS = os.path.join(path, "crates", "chimera-cli", "src", "config.rs")
    TYPES_RS = os.path.join(path, "crates", "nexus-core", "src", "config.rs")
    SAMPLE = os.path.join(path, "examples", "config.sample.yaml")


def _read(path):
    with open(path, encoding="utf-8", errors="replace", newline="") as fh:
        return fh.read()


def embedded_template():
    """The exact string `chimera config init` writes, with line endings normalised.

    Raises ValueError when the shape is not found -- that is undecidable, not "clean".
    """
    if not os.path.exists(CONFIG_RS):
        raise ValueError("config.rs missing (%s)" % CONFIG_RS)
    text = _read(CONFIG_RS).replace("\r\n", "\n")
    m = TEMPLATE_RE.search(text)
    if not m:
        raise ValueError("omega_yaml_template() raw-string body not found in config.rs "
                         "-- the template was refactored, this gate must be updated too")
    body = m.group(1)
    if not body.strip():
        raise ValueError("omega_yaml_template() is empty -- nothing to compare against")
    return body


def sample_text():
    if not os.path.exists(SAMPLE):
        raise ValueError("sample missing (%s)" % SAMPLE)
    return _read(SAMPLE).replace("\r\n", "\n")


STRUCT_RE = re.compile(r"pub struct ChimeraConfig\b[^{]*\{(.*?)\n\}", re.S)
FIELD_RE = re.compile(r"^\s{4}pub\s+([a-z_0-9]+)\s*:\s*([A-Za-z_:<>,\s]+?)\s*,?\s*$", re.M)
DEFAULT_ATTR_RE = re.compile(r"#\[serde\([^\)]*default")


def struct_fields():
    """`ChimeraConfig` top-level fields as [(name, kind, has_default)] (nexus-core, not chimera-cli).

    `kind` splits the two shapes a root-level YAML key can have, because they carry
    different obligations for a sample: a **section** (type named `*Config`) is a
    whole subsystem's knobs, so omitting it makes an entire area undiscoverable,
    while a **scalar** is one switch whose value `Serialized::defaults` already
    supplies. Treating both alike was a real defect in this gate: `enable_strategy_cap`
    (`bool`, `#[serde(default)]`) got reported as a "missing section", i.e. the tool
    invented a 15th section for a type that has 14.

    WHY a second authority at all: byte-equality against the embedded template only
    proves the sample matches *another hand-written copy in the same repo*. A key that
    no longer exists in the struct, or a struct section the sample never mentions, is
    invisible to that check -- so compare against the type as well.
    """
    if not os.path.exists(TYPES_RS):
        raise ValueError("ChimeraConfig definition missing (%s)" % TYPES_RS)
    body = _read(TYPES_RS).replace("\r\n", "\n")
    m = STRUCT_RE.search(body)
    if not m:
        raise ValueError("pub struct ChimeraConfig not found in nexus-core/config.rs "
                         "-- the type was refactored, this gate must be updated too")
    inner = m.group(1)
    out = []
    for fm in FIELD_RE.finditer(inner):
        name, ty = fm.group(1), fm.group(2).strip()
        # the attributes sit directly above the field; take the doc/attr block since
        # the previous field ended, so `#[serde(default)]` binds to the right field
        prefix = inner[:fm.start()]
        prev = prefix.rfind(",\n")
        has_default = bool(DEFAULT_ATTR_RE.search(prefix[prev + 2:] if prev >= 0 else prefix))
        out.append((name, "section" if ty.endswith("Config") else "scalar", has_default))
    return out


def diff_against_struct(keys):
    fields = struct_fields()
    names = {n for n, _k, _d in fields}
    sections = {n for n, kind, _d in fields if kind == "section"}
    scalars = {n for n, kind, _d in fields if kind == "scalar"}
    tk = set(keys)
    missing_sections = sorted(sections - tk)
    phantom = sorted(tk - names)
    # scalars the sample omits: legal (Serialized::defaults covers them), so reported,
    # never red
    undocumented_scalars = sorted(scalars - tk)
    return missing_sections, phantom, undocumented_scalars, len(fields), len(sections)


def lint_template(body):
    """Structural assertions the sample must satisfy to be worth anything.

    WHY these three: a 9-byte `# Chimera` comment file passed "is it valid YAML"
    trivially, so validity alone is not a usable bar. What the defect actually was:
    zero keys, and no `~/.chimera/omega.yaml` header pointing at the real path.
    """
    lines = [ln for ln in body.splitlines() if ln.strip()]
    # 只取段名本身：把整行（含冒号）当键去和 struct 字段比，会得到"14 个全幻影 +
    # 15 个全漏"的漂亮红字——形状错得越彻底越像"发现了大问题"（本门自测的正向对照抓到的就是它）
    key_re = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*):")
    keys = [key_re.match(ln).group(1) for ln in lines if key_re.match(ln)]
    comments = sum(1 for ln in lines if ln.lstrip().startswith("#"))
    problems = []
    if len(keys) < 3:
        problems.append("only %d top-level-looking keys (a sample with <3 keys "
                        "teaches nothing)" % len(keys))
    if "~/.chimera/" not in body:
        problems.append("no `~/.chimera/` path in the sample -- readers cannot tell "
                        "where the real config lives")
    return keys, comments, problems


def compare():
    """Return (ok, template, sample, detail-lines)."""
    tpl = embedded_template()
    smp = sample_text()
    keys, _comments, problems = lint_template(tpl)
    missing, ghost, scalars, nfields, nsections = diff_against_struct(keys)
    detail = ["模板 %d 行 / %d 个根级键；ChimeraConfig %d 字段（%d 段 + %d 标量）；样例 %d 行"
              % (len(tpl.splitlines()), len(keys), nfields, nsections,
                 nfields - nsections, len(smp.splitlines()))]
    for p in problems:
        detail.append("[SHAPE] %s" % p)
    if missing:
        detail.append("[GAP-T] 类型有段、样例无（该子系统的全部开关无从发现）：%s"
                      % ", ".join(missing))
        problems.append("missing-section-in-sample")
    if ghost:
        detail.append("[GAP-T] 样例有、类型无（幻影键，写了也不生效）：%s" % ", ".join(ghost))
        problems.append("ghost-in-sample")
    if scalars:
        # not a defect: Serialized::defaults covers any key the file omits. Reported so a
        # human can decide whether the switch is worth documenting in the template.
        detail.append("[NOTE] 标量字段未进样例（有默认值，不判红）：%s" % ", ".join(scalars))
    if tpl.rstrip("\n") == smp.rstrip("\n"):
        return (not problems, tpl, smp, detail + ["[OK] 样例与内嵌模板逐字等值"])
    # first difference, so the fix is actionable without a diff tool
    tl, sl = tpl.splitlines(), smp.splitlines()
    for i in range(max(len(tl), len(sl))):
        a = tl[i] if i < len(tl) else "<EOF>"
        b = sl[i] if i < len(sl) else "<EOF>"
        if a != b:
            detail.append("[GAP] 第 %d 行不一致：模板 %r / 样例 %r" % (i + 1, a[:60], b[:60]))
            break
    detail.append("  修法：跑 `python scripts/check_config_sample_parity.py --write`"
                  " 重新派生，勿手改样例。")
    return (False, tpl, smp, detail)


def write_sample():
    tpl = embedded_template()
    _keys, _c, problems = lint_template(tpl)
    if problems:
        print("[CONFIG] 模板本身不合格，拒绝派生: %s" % "; ".join(problems))
        return 2
    os.makedirs(os.path.dirname(SAMPLE), exist_ok=True)
    with open(SAMPLE, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(tpl if tpl.endswith("\n") else tpl + "\n")
    print("[WRITE] %s 已从内嵌模板派生（%d 行）" % (SAMPLE, len(tpl.splitlines())))
    return 0


def _fixture(root, template_body, sample_body, fields=("nexus", "quest", "memory"),
             scalars=()):
    cfg = os.path.join(root, "crates", "chimera-cli", "src")
    typ = os.path.join(root, "crates", "nexus-core", "src")
    ex = os.path.join(root, "examples")
    os.makedirs(cfg, exist_ok=True)
    os.makedirs(typ, exist_ok=True)
    os.makedirs(ex, exist_ok=True)
    with open(os.path.join(cfg, "config.rs"), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("fn omega_yaml_template() -> &'static str {\n"
                 "    r#\"%s\"#\n"
                 "}\n" % template_body)
    # 类型必须区分段与标量：全用 u8 建模会让"标量漏写"和"整段漏写"这两种危害完全不同的
    # 情形共用一条判据，正是本门首版犯的错（它给 ChimeraConfig 凭空造了第 15 个段）
    body = ""
    for f in fields:
        body += "    pub %s: %sConfig,\n" % (f, f.capitalize())
    for f, defaulted in scalars:
        if defaulted:
            body += "    #[serde(default)]\n    pub %s: bool,\n" % f
        else:
            body += "    pub %s: u32,\n" % f
    with open(os.path.join(typ, "config.rs"), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("pub struct ChimeraConfig {\n" + body + "}\n")
    sp = os.path.join(ex, "config.sample.yaml")
    if sample_body is None:
        # 缺失用例必须真的删掉：留着一个上一用例写下的文件，测的就不是"缺失"了
        if os.path.exists(sp):
            os.remove(sp)
    else:
        with open(sp, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(sample_body)


def _raw_config_rs(root, text):
    """Overwrite the fixture's config.rs verbatim (to test the unrecognised-shape path)."""
    _w(os.path.join(root, "crates", "chimera-cli", "src", "config.rs"), text)


def _w(path, text):
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)


GOOD = ("# ~/.chimera/omega.yaml\nnexus:\n  version: \"2.28.2-omega\"\n"
        "quest:\n  auto_decompose: true\nmemory:\n  tier: \"warm\"\n")


def selftest():
    tmp = tempfile.mkdtemp(prefix="cfgparity")
    real_root = ROOT
    try:
        checks = []
        # 1) 等值 + 形状合格 -> 绿
        _fixture(tmp, GOOD, GOOD)
        set_root(tmp)
        ok, _t, _s, _d = compare()
        checks.append(("等值样例判绿", ok))
        # 2) 首版真实缺陷形状：9 字节纯注释 -> 模板形状断言必须拦住
        _fixture(tmp, "# Chimera\n", "# Chimera\n")
        ok, _t, _s, _d = compare()
        checks.append(("纯注释样例判红（键数<3 即不算样例）", not ok))
        # 3) 漂移必须判红，且给出第一个不一致行号（GOOD 的第 5 行才是 auto_decompose）
        _fixture(tmp, GOOD, GOOD.replace("auto_decompose: true", "auto_decompose: false"))
        ok, _t, _s, detail = compare()
        checks.append(("漂移判红并定位到行", not ok and any("[GAP] 第 5 行" in x for x in detail)))
        # 4) 样例缺失 -> 不可判定（rc 2 路径），不得当成"干净"
        _fixture(tmp, GOOD, None)
        try:
            compare()
            raised = False
        except ValueError:
            raised = True
        checks.append(("样例缺失走 ValueError(退 2 而非退 0)", raised))
        # 5) 模板被重构到认不出来 -> 同样不可判定，不得静默放行
        #    （必须整体重写 config.rs：_fixture 会替我套上函数骨架，套着就仍然匹配）
        _fixture(tmp, GOOD, GOOD)
        _raw_config_rs(tmp, "fn render_default_yaml() -> String { String::new() }\n")
        try:
            embedded_template()
            raised = False
        except ValueError:
            raised = True
        checks.append(("模板标记失配走 ValueError(退 2)", raised))
        # 6) --write 真派生一次，派生后必须自等（幂等 + 正控）
        _fixture(tmp, GOOD, "# Chimera\n")
        rc = write_sample()
        ok_after, _t, _s, _d = compare()
        checks.append(("--write 派生后判绿且可重复（rc=%d）" % rc, rc == 0 and ok_after))
        # 7/8) 与类型对拍：幻影段与漏段都必须抓（"两处一致"不等于"两处都对"）
        _fixture(tmp, GOOD, GOOD, fields=("nexus", "quest", "memory"))
        ok_base, _t, _s, _d = compare()
        checks.append(("段集合与类型一致时判绿（正向对照）", ok_base))
        _fixture(tmp, GOOD, GOOD, fields=("nexus", "quest", "cap"))
        ok_g, _t, _s, dg = compare()
        checks.append(("样例有类型无的幻影键判红",
                       not ok_g and any("幻影键" in x for x in dg)))
        _fixture(tmp, GOOD, GOOD, fields=("nexus", "quest", "memory", "cap"))
        ok_m, _t, _s, dm = compare()
        checks.append(("类型有样例无的漏段判红",
                       not ok_m and any("无从发现" in x for x in dm)))
        # 9) 范畴修正的正向对照：#[serde(default)] 标量没进样例**不是**缺陷
        #    （Serialized::defaults 会补默认值）；首版把它当成"漏段"报红，等于给类型
        #    凭空多算一个段——这类"形状错得越彻底越像发现了大问题"的输出必须由夹具拦住
        _fixture(tmp, GOOD, GOOD, fields=("nexus", "quest", "memory"),
                 scalars=[("enable_strategy_cap", True)])
        ok_sc, _t, _s, dsc = compare()
        checks.append(("带默认值的标量漏写判绿且只出 NOTE",
                       ok_sc and any("[NOTE]" in x and "enable_strategy_cap" in x for x in dsc)))
        # 10) 而标量写进样例时不得反过来被判幻影键（否则等于逼样例永远不能记录开关）
        _fixture(tmp, GOOD + "enable_strategy_cap: false\n",
                 GOOD + "enable_strategy_cap: false\n",
                 fields=("nexus", "quest", "memory"),
                 scalars=[("enable_strategy_cap", True)])
        ok_sk, _t, _s, dsk = compare()
        checks.append(("样例已记录该标量则既不判红也无 NOTE", ok_sk and not any("[NOTE]" in x for x in dsk)))

        bad = [n for n, k in checks if not k]
        for n, k in checks:
            print("  [%s] %s" % ("PASS" if k else "FAIL", n))
        if bad:
            print("[SELFTEST] %d/%d FAILED" % (len(bad), len(checks)))
            return 1
        print("[SELFTEST] %d 断言：等值绿 / 注释桩红 / 漂移定位行 / 两处缺失退 2 / --write 幂等"
              % len(checks))
        return 0
    finally:
        set_root(real_root)
        shutil.rmtree(tmp, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--write", action="store_true")
    ap.add_argument("--root")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if args.root:
        set_root(os.path.abspath(args.root))
    try:
        if args.write:
            return write_sample()
        ok, _tpl, _smp, detail = compare()
    except ValueError as exc:
        print("[CONFIG] %s" % exc)
        return 2
    for line in detail:
        print(line)
    if ok:
        print("[OK] 配置样例由 config.rs 内嵌模板派生，两处一致")
        return 0
    print("[FAIL] 样例与内嵌模板已脱钩（唯一权威源是 omega_yaml_template()）")
    return 1


if __name__ == "__main__":
    import gate_rc
    sys.exit(gate_rc.run(main))
