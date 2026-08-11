import re, os, sys

# 用法: python _fn_scan.py [root1 root2 ...]（粗筛行距法，配合 _audit_fnlen.py 精确复验）
roots = sys.argv[1:] if len(sys.argv) > 1 else [r'crates\seccore\src', r'crates\qeep-protocol\src', r'crates\decay-engine\src']
for root in roots:
    for dirpath, dirs, files in os.walk(root):
        for f in files:
            if not f.endswith('.rs'):
                continue
            p = os.path.join(dirpath, f)
            with open(p, encoding='utf-8', errors='replace') as fh:
                lines = fh.readlines()
            fn_starts = []
            for i, line in enumerate(lines):
                if re.search(r'\bfn\s+\w+', line):
                    fn_starts.append(i)
            for idx, start in enumerate(fn_starts):
                end = fn_starts[idx+1] if idx+1 < len(fn_starts) else len(lines)
                length = end - start
                if length > 200:
                    name = re.search(r'fn\s+(\w+)', lines[start]).group(1)
                    print(f'{p}:{start+1} fn {name} ~{length} lines')
