#!/usr/bin/env python3
"""audit_phaseR_spec.py 的 T5 / Dockerfile 判定阴性对照。

做法：把被检文件复制到隔离目录并按"缺陷形态"变异，再以该目录为 cwd 跑探针，
断言探针**必须**把这两项判为非 PROVEN。探针若对已知缺陷沉默，即无牙齿。
不改动真实工作区文件。
"""
import io
import os
import shutil
import subprocess
import sys

# 本脚本会把探针输出原样打印，而探针输出含中文/替换字 → GBK 控制台会 UnicodeEncodeError。
# 在脚本自己开头重配，不依赖调用方设 PYTHONIOENCODING。
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.stderr.reconfigure(encoding="utf-8", errors="replace")

NC = "tmp/nc_spec"
PROBE = os.path.abspath("scripts/audit_phaseR_spec.py")


def put(rel, text):
    p = os.path.join(NC, rel.replace("/", os.sep))
    os.makedirs(os.path.dirname(p), exist_ok=True)
    io.open(p, "w", encoding="utf-8", newline="\n").write(text)


def get(rel):
    return io.open(rel, encoding="utf-8", errors="replace").read()


if os.path.isdir(NC):
    shutil.rmtree(NC)
os.makedirs(NC)

# 变异 1：给阻塞步加回 continue-on-error（这正是 RK-P30 "写了不跑" 的形态）
ci = get(".github/workflows/ci.yml")
mut = ci.replace(
    "- name: Perf redline inventory & lint gate\n",
    "- name: Perf redline inventory & lint gate\n        continue-on-error: true\n",
    1,
)
assert mut != ci, "变异 1 未命中锚点 —— 探针的 step 名可能已变，NC 本身失效"
put(".github/workflows/ci.yml", mut)

# 变异 2：Dockerfile ARG VERSION 退回陈旧值
dk = get("Dockerfile").replace("ARG VERSION=2.28.0-omega", "ARG VERSION=2.26.0", 1)
assert "ARG VERSION=2.26.0" in dk, "变异 2 未命中"
put("Dockerfile", dk)

# 探针需要能读到这两个文件；其余文件不存在会被判 MISSING —— 属预期，只看这两项。
# 子进程必须显式给 PYTHONIOENCODING=utf-8：否则它在管道上沿用宿主 cp936，
# 父进程按 utf-8 解码就得到乱码，用中文子串过滤永远匹配不上（本 NC 第一次就踩了这个）。
env = dict(os.environ, PYTHONIOENCODING="utf-8")
r = subprocess.run([sys.executable, PROBE], cwd=NC, capture_output=True, text=True,
                   encoding="utf-8", errors="replace", env=env)
out = r.stdout + r.stderr

# 过滤键用纯 ASCII（行内容含中文，但标签里的 ASCII 部分足够唯一）
t5 = [l for l in out.splitlines() if "T5 ci.yml" in l]
docker = [l for l in out.splitlines() if "Dockerfile ARG VERSION" in l]
print("变异后的探针判定：")
for l in t5 + docker:
    print("  ", l.strip()[:110])

ok_t5 = bool(t5) and "NOT PROVEN" in t5[0]
ok_dk = bool(docker) and "MISSING" in docker[0]
print(f"\n[NC] T5 被正确判红     : {'PASS' if ok_t5 else 'FAIL'}")
print(f"[NC] Dockerfile 被正确判缺失: {'PASS' if ok_dk else 'FAIL'}")
sys.exit(0 if (ok_t5 and ok_dk) else 1)
