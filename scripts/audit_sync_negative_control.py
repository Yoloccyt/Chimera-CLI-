#!/usr/bin/env python3
"""scripts/audit_cmd_sync.py 的两组阴性对照（证明它自己不是假门禁）。

NC-a 只篡改一处（模拟"改了 CI 忘了改文档"）→ 期望 exit 1
NC-b 删掉文档          （模拟"在 CI checkout 里跑，文档不存在"）→ 期望 exit 2，不得算通过
"""
import importlib.util
import pathlib
import shutil
import tempfile

spec = importlib.util.spec_from_file_location("acs", "scripts/audit_cmd_sync.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

base = pathlib.Path(tempfile.mkdtemp(prefix="acs_nc_"))
for rel in m.TARGETS:
    src = pathlib.Path(rel)
    if not src.exists():
        src = pathlib.Path(rel.lower())
    dst = base / rel
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text(src.read_text(encoding="utf-8-sig"), encoding="utf-8")

# 基线：三份一致 → 0
m.ROOT = base
rc0 = m.main([])

# NC-a：只改文档一处
doc = base / ".claude/CLAUDE.md"
doc.write_text(doc.read_text(encoding="utf-8").replace("--deny unsound", "--deny warnings"),
               encoding="utf-8")
rcA = m.main([])

# NC-b：文档整个目录消失（CI 视角）
shutil.rmtree(base / ".claude")
rcB = m.main([])

print(f"\nNC-baseline 三份一致      -> exit {rc0} (期望 0)")
print(f"NC-a      仅一处被改      -> exit {rcA} (期望 1)")
print(f"NC-b      文档不存在      -> exit {rcB} (期望 2)")
ok = (rc0, rcA, rcB) == (0, 1, 2)
print(f"\nSELF-AUDIT GATE: {'ALL PASS' if ok else 'PROBLEM'}")
raise SystemExit(0 if ok else 1)
