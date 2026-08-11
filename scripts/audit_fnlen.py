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

roots = sys.argv[1:] if len(sys.argv) > 1 else ['crates']
all_fn = []
for root in roots:
    for dirpath, dirnames, filenames in os.walk(root):
        for fn in filenames:
            if fn.endswith('.rs'):
                path = os.path.join(dirpath, fn)
                r = analyze(path)
                for name, s, e, l in r:
                    all_fn.append((path, name, s, e, l))
all_fn.sort(key=lambda x: -x[4])
for path, name, s, e, l in all_fn[:60]:
    print(f'{l:4d} lines  {path}:{s}-{e}  fn {name}')
print(f'Total: {len(all_fn)} functions > 200 lines')
