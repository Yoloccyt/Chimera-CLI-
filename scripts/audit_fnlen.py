import re, os, sys

# 用法: python _audit_fnlen.py [root1 root2 ...]
# 缺省扫描 crates/ 全目录（精确花括号平衡法，感知字符串/字符/行注释）

def sanitize(src):
    """屏蔽注释与字符串/字符字面量（等长空格替换，保留换行与行号）。

    使 fn 正则与花括号平衡只作用于真实代码，消除 doctest（`/// # fn run()`）
    与字符串内大括号造成的假阳性。
    """
    out = list(src)
    i = 0
    n = len(src)
    in_str = False
    in_char = False
    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ''
        if in_str:
            if c == '\\':
                out[i] = ' '; out[i + 1] = ' ' if i + 1 < n else ' '
                i += 2
                continue
            if c == '"':
                in_str = False
            out[i] = ' '
            i += 1
            continue
        if in_char:
            if c == '\\':
                out[i] = ' '; out[i + 1] = ' ' if i + 1 < n else ' '
                i += 2
                continue
            if c == "'":
                in_char = False
            out[i] = ' '
            i += 1
            continue
        if c == '/' and nxt == '/':
            while i < n and src[i] != '\n':
                out[i] = ' '
                i += 1
            continue
        if c == '/' and nxt == '*':
            out[i] = ' '; out[i + 1] = ' '
            i += 2
            while i < n and not (src[i] == '*' and i + 1 < n and src[i + 1] == '/'):
                if src[i] != '\n':
                    out[i] = ' '
                i += 1
            if i < n:
                out[i] = ' '; out[i + 1] = ' ' if i + 1 < n else ' '
                i += 2
            continue
        if c == '"':
            in_str = True
            out[i] = ' '
            i += 1
            continue
        if c == "'":
            # Rust 生命周期（'a / 'static）不是 char 字面量：仅当 `'x'`（含 \\ 转义）才进入 char 模式
            is_char_lit = False
            if i + 1 < n and src[i + 1] == '\\':
                is_char_lit = True
            elif i + 2 < n and src[i + 1] != ' ' and src[i + 2] == "'":
                is_char_lit = True
            if is_char_lit:
                in_char = True
                out[i] = ' '
                i += 1
                continue
            i += 1
            continue
        i += 1
    return ''.join(out)


def analyze(path):
    with open(path, 'r', encoding='utf-8') as f:
        src = f.read()
    src = sanitize(src)  # 净化后再扫描（注释/字符串已屏蔽）
    results = []
    fn_pat = re.compile(r'\bfn\s+([A-Za-z_]\w*)')
    for m in fn_pat.finditer(src):
        name = m.group(1)
        brace = src.find('{', m.start())
        if brace == -1:
            continue
        # 跳过声明式 fn（trait 方法 / extern 声明：签名内出现 `;` 且无函数体）
        sig = src[m.start():brace]
        if ';' in sig:
            continue
        depth = 0
        close = -1
        i = brace
        while i < len(src):
            c = src[i]
            if c == '{':
                depth += 1
            elif c == '}':
                depth -= 1
                if depth == 0:
                    close = i
                    break
            i += 1
        if close == -1:
            continue
        start_line = src.count('\n', 0, m.start()) + 1
        end_line = src.count('\n', 0, close) + 1
        length = end_line - start_line + 1
        if length > 200:
            results.append((name, start_line, end_line, length))
    return results

# 位置参数 = 扫描根。`--baseline PATH` 的值必须先摘掉再收集，否则它会伪装成一个根，
# 把扫描面从 crates/ 静默换成"一个登记册文件"（os.walk 走文件 ⇒ 0 个 .rs ⇒ 空扫面）。
_bl_value = sys.argv.index('--baseline') + 1 if '--baseline' in sys.argv else None
roots = [a for i, a in enumerate(sys.argv[1:], start=1)
         if not a.startswith('--') and i != _bl_value]
MODE = 'report'
if '--check' in sys.argv:
    MODE = 'check'
elif '--emit' in sys.argv:
    MODE = 'emit'

def scan():
    """All >200-line functions, aggregated to (path, fn) -> (max_len, count).

    WHY aggregate: a key must survive line drift, so the register cannot be keyed
    on `path:start_line` (this repo has already been burned twice by re-keying).
    Aggregating by name inside one file keeps the key stable; the cap is the MAX
    length, which is what the ratchet needs.
    """
    agg = {}
    seen_rs = 0
    for root in (roots or ['crates']):
        for dirpath, dirnames, filenames in os.walk(root):
            for fn in filenames:
                if not fn.endswith('.rs'):
                    continue
                seen_rs += 1
                path = os.path.join(dirpath, fn).replace(os.sep, '/')
                for name, s, e, l in analyze(path):
                    k = (path, name)
                    mx, c = agg.get(k, (0, 0))
                    agg[k] = (max(mx, l), c + 1)
    # 扫到 0 个 .rs = 扫描面本身塌了（根写错 / 传成文件 / 目录不存在），
    # 此时"0 个超标"是**空扫面的副产品**，既不能当"红线达标"也不能当"债全清"
    # （后者会伪装成 10 条 STALE 红，看起来像判定、其实啥也没看）。⇒ 一律退 2。
    if seen_rs == 0:
        sys.stderr.write('[UNDECIDABLE] scanned 0 .rs file(s) from roots=%s; '
                         '不把空扫面读成判定\n' % (roots or ['crates']))
        sys.exit(2)
    return agg


def reason_for(path):
    """Mechanically derived allowance reason (registers need a reason per row)."""
    if '/tests/' in path or '/benches/' in path or '/examples/' in path:
        return 'test-side >200 lines (assertion table/bench body, not shipped)'
    return 'production >200 lines (open debt: split before it grows)'


BASELINE = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'fnlen_baseline.txt')
# `--baseline PATH` overrides the register location: the smoke gate needs it to
# point at temp copies when it plants its own negative controls.
if '--baseline' in sys.argv:
    BASELINE = sys.argv[sys.argv.index('--baseline') + 1]
    roots = [a for a in roots if a != BASELINE]



def load_baseline(path):
    """`path::fn|cap  # reason` -> {(path, fn): cap}. Rows without a reason are rejected."""
    if not os.path.exists(path):
        return None, ['baseline file not found: %s' % path]
    caps, bad = {}, []
    with open(path, encoding='utf-8') as f:
        for raw in f:
            line = raw.split('#', 1)[0].strip()
            if not line:
                continue
            if '#' not in raw:
                bad.append('row without a reason: %s' % line)
                continue
            key, _, cap = line.rpartition('|')
            path_part, _, fn_part = key.partition('::')
            try:
                caps[(path_part.strip(), fn_part.strip())] = int(cap.strip())
            except ValueError:
                bad.append('unparseable cap: %s' % line)
    return caps, bad


measured = scan()

if MODE == 'report':
    ordered = sorted(measured.items(), key=lambda kv: -kv[1][0])
    for (path, name), (l, n) in ordered[:60]:
        print('%4d lines  %s  fn %s%s' % (l, path, name,
                                          '' if n == 1 else '  (x%d)' % n))
    print('Total: %d functions > 200 lines' % len(measured))
    sys.exit(0)

if MODE == 'emit':
    out = ['# fnlen_baseline.txt -- only-decrease register for "single fn <= 200 lines"',
           '# GENERATED by `python3 scripts/audit_fnlen.py --emit`; refresh with --emit',
           '# after a split so the cap ratchets DOWN. Key is path::fn (stable across',
           '# line drift); cap is the measured max length.',
           '']
    for (path, name), (l, n) in sorted(measured.items(), key=lambda kv: kv[0]):
        out.append('%s::%s|%d  # %s%s' % (path, name, l, reason_for(path),
                                          '' if n == 1 else ' (x%d same-named)' % n))
    print('\n'.join(out))
    sys.exit(0)

# --check: the judgement. rc 0 = ratchet honoured, 1 = judged red, 2 = undecidable.
caps, bad = load_baseline(BASELINE)
if caps is None:
    print('[UNDECIDABLE] %s' % '; '.join(bad))
    sys.exit(2)
if bad:
    print('[UNDECIDABLE] malformed baseline rows (an unexplained allowance is how a'
          ' register becomes a dumping ground): %s' % '; '.join(bad))
    sys.exit(2)

new, growth, stale = [], [], []
for (path, name), (l, _n) in measured.items():
    key = '%s::%s' % (path, name)
    if (path, name) not in caps:
        new.append('%s|%d' % (key, l))
    elif l > caps[(path, name)]:
        growth.append('%s %d->%d' % (key, caps[(path, name)], l))
for (path, name), cap in caps.items():
    if (path, name) not in measured:
        stale.append('%s|%d' % ('%s::%s' % (path, name), cap))
    elif measured[(path, name)][0] < cap:
        growth.append('%s shrank %d->%d (re-emit to ratchet the cap down)'
                      % ('%s::%s' % (path, name), cap, measured[(path, name)][0]))

print('[CHECK] measured >200-line fns: %d; baseline rows: %d (cap = measured max)'
      % (len(measured), len(caps)))
for r in new:
    print('[NEW] not registered: %s' % r)
for r in growth:
    print('[GROWTH] %s' % r)
for r in stale:
    print('[STALE] registered but no longer measured: %s' % r)
if new or growth or stale:
    print('[FAIL] fn-length ratchet broken: %d new, %d growth/shrink, %d stale'
          % (len(new), len(growth), len(stale)))
    sys.exit(1)
print('[OK] fn-length ratchet honoured (no new offender, no growth, no stale allowance)')
sys.exit(0)
