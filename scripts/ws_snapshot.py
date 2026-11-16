#!/usr/bin/env python3
"""workspace 知识统一快照模块（W3b，病灶 G4 的模块本体）。

背景（WHY）：
  ~80 个门禁脚本各自重复解析根 Cargo.toml 的 workspace.members / 层表 / 依赖图
  （已确诊病灶 P-病灶 G4）——同一份事实 N 份解析，BOM 容忍、缺段容错、路径归一化
  各自为政，manifest 改一处就要追 N 个脚本。本模块把「读 workspace 知识」收敛为
  单一真值源：build 一次、落盘缓存、内容指纹失效自动重建。本批只做模块本体 +
  selftest，不做消费者迁移（避免触碰在途脏文件）。

复用先例：coverage_gate.py 已 `from check_crate_reachability import load_toml,
workspace_members`（BOM 容忍 + 以 members 为权威）。本模块沿用同一复用链，
绝不复制第 6 份成员解析——tomllib 裸读遇 BOM 会在 line 1 col 1 抛 TOMLDecodeError，
而根 Cargo.toml 历史上带 BOM（check_crate_reachability.py:56 的教训）。

数据契约（dataclass Snapshot，frozen）：
  members     : tuple[str,...] —— 成员 crate 目录名（路径 basename + 字典序）。
                 权威源 = [workspace].members（Cargo 真正编译的集合），非 `ls crates/`。
  dep_keys    : frozenset[str] —— [workspace.dependencies] 键集。
  version     : str —— [workspace.package].version（版本号唯一权威源）。
  fingerprint : str —— Cargo.toml 全文原始字节 sha256 的前 16 hex。改任一成员/
                 依赖/版本即失效；load 据此判定缓存去留，不做语义 diff。

缓存契约：
  落点 root/tmp/ws_snapshot.json（相对 root，tmp/ 缺失时自建——缓存属派生物，
  不入主树索引）。load(root, max_age_s=300) 命中条件 = 文件存在 + fingerprint
  匹配 + mtime 未超龄；命中即 os.utime 刷新 mtime（滑动续期，缓存话语文档见
  load  docstring）。save 走同目录临时文件 + os.replace 原子替换，崩溃不留半文件。
  JSON 损坏按过期处理（重建覆盖）——缓存可无损重建，不构成「判据没跑完」的退 2。

CLI：
    --build     从 root/Cargo.toml 重建快照并落盘，打印摘要（rc=0）
    --show      走 load()（带缓存语义）全量打印成员 + 摘要行（rc=0）
    --selftest  临时目录最小 fixture（2 成员 + 2 依赖键）验证 build/save/load
                往返、mtime 滑动续期、超龄重建、指纹失效重建、损坏自愈；
                不依赖本仓真实树

退出码（本仓门脚本共用方言，scripts/gate_rc.py 兜底崩溃 → 2）：
  0 = 通过 / 1 = selftest 断言失败（本模块为快照工具，无判红语义，1 仅自测用）
      / 2 = 不可判定（缺 Cargo.toml / 用法错误 / 崩溃——绝不静默判绿）。
输出保持 ASCII（Windows GBK 控制台约定）；库函数不向 stdout 打印，重建提示走 stderr，
保证 --show 的 stdout 是可被消费者安全取用的纯净成员清单。
"""
import hashlib
import json
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib  # noqa: F401  (经由 check_crate_reachability.load_toml 间接使用，显式导入只为声明 3.11 门槛)
except ImportError:
    print("[UNDECIDABLE] ws_snapshot.py requires python >= 3.11 (tomllib)")
    sys.exit(2)

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, ".."))

# 复用链与 coverage_gate.py 同层：BOM 容忍解析在 check_crate_reachability.load_toml。
sys.path.insert(0, HERE)
from check_crate_reachability import load_toml  # noqa: E402
import gate_rc  # noqa: E402  (selftest fixture 复用带清理的 tmpdir，不得裸 mkdtemp 泄漏)

CACHE_REL = os.path.join("tmp", "ws_snapshot.json")  # 相对 root；缓存是派生物，不入主树
FINGERPRINT_LEN = 16  # sha256 截断位宽：16 hex = 64 bit，碰撞域内唯一性足够且日志可读
SCHEMA_VERSION = 1


@dataclass(frozen=True)
class Snapshot:
    """workspace 知识不可变快照；frozen 使其可 hash/可作 dict key，杜绝消费方改缓存。"""
    members: tuple
    dep_keys: frozenset
    version: str
    fingerprint: str


# --------------------------------------------------------------------- core
def _manifest_fingerprint(root):
    """root/Cargo.toml 全文原始字节的 sha256 前 16 hex。

    WHY 原始字节而非解析后重序列化：BOM/换行/注释漂移都会改指纹——缓存语义是
    「文件变过就重建」，不是「语义变过才重建」，前者零误检成本。
    """
    raw = (Path(root) / "Cargo.toml").read_bytes()
    return hashlib.sha256(raw).hexdigest()[:FINGERPRINT_LEN]


def build(root):
    """解析 root/Cargo.toml → Snapshot。

    缺 [workspace]/members 按空集容忍（指纹兜底一致性），缺 Cargo.toml 本体抛
    FileNotFoundError——由 CLI 侧 gate_rc 翻译成退 2，不借默认值冒充解析成功。
    """
    root = Path(root)
    manifest = root / "Cargo.toml"
    data = load_toml(str(manifest))  # BOM 容忍；FileNotFoundError 原样上抛
    ws = data.get("workspace") or {}
    members = ws.get("members") or []
    dep_table = ws.get("dependencies") or {}
    pkg = ws.get("package") or {}
    return Snapshot(
        members=tuple(sorted(os.path.basename(str(p).rstrip("/\\")) for p in members)),
        dep_keys=frozenset(dep_table.keys()),
        version=str(pkg.get("version", "")),
        fingerprint=_manifest_fingerprint(root),
    )


def _to_payload(snapshot):
    return {
        "schema": SCHEMA_VERSION,
        "saved_at": time.time(),
        "version": snapshot.version,
        "fingerprint": snapshot.fingerprint,
        "members": list(snapshot.members),
        "dep_keys": sorted(snapshot.dep_keys),
    }


def _from_payload(payload):
    """JSON dict → Snapshot；键缺失/类型错抛 KeyError/TypeError，由调用方判过期。"""
    if payload.get("schema") != SCHEMA_VERSION:
        raise ValueError(f"unknown schema {payload.get('schema')!r}")
    return Snapshot(
        members=tuple(payload["members"]),
        dep_keys=frozenset(payload["dep_keys"]),
        version=str(payload["version"]),
        fingerprint=str(payload["fingerprint"]),
    )


def save(root, snapshot):
    """原子写 root/tmp/ws_snapshot.json：同目录临时文件 + os.replace。

    WHY 原子替换：消费者可能在同一时刻读缓存，半文件会被 tomllib/json 读到
    截断内容；replace 保证读者要么见旧版要么见新版。
    """
    cache = Path(root) / CACHE_REL
    cache.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(_to_payload(snapshot), indent=2, sort_keys=True) + "\n"
    tmp = cache.with_name(f".{cache.name}.{os.getpid()}.tmp")
    tmp.write_bytes(text.encode("utf-8"))  # 写字节：绕开 Windows 文本模式的 \r\n 翻译（LF 纪律）
    os.replace(tmp, cache)
    return cache


def _age_s(path):
    return time.time() - os.stat(path).st_mtime


def load(root, max_age_s=300):
    """命中缓存 → 返回 Snapshot 并刷新 mtime；否则 build()+save() 后返回。

    命中条件（三者同时）：文件存在 + fingerprint 与现 Cargo.toml 匹配 + mtime
    未超 max_age_s。指纹不匹配/超龄/JSON 损坏一律按「过期」走重建——缓存可无损
    再生，不因缓存劣化判红或退 2；真正的不可判定（Cargo.toml 缺失）由 build()
    抛 FileNotFoundError 上抛给入口兜底。
    """
    root = Path(root)
    cache = root / CACHE_REL
    stale_reason = None
    if cache.is_file():
        try:
            payload = json.loads(cache.read_text(encoding="utf-8"))
            if payload.get("fingerprint") != _manifest_fingerprint(root):
                stale_reason = "fingerprint mismatch (Cargo.toml changed)"
            elif _age_s(cache) > max_age_s:
                stale_reason = f"expired (age {_age_s(cache):.0f}s > {max_age_s}s)"
            else:
                snap = _from_payload(payload)
                os.utime(cache, None)  # 滑动续期：命中即刷新 mtime，长命消费者不断供
                return snap
        except (OSError, ValueError, KeyError, TypeError):
            stale_reason = "corrupt cache (unreadable JSON or schema drift)"
    else:
        stale_reason = "no cache yet"
    snap = build(root)
    save(root, snap)
    print(f"[INFO] ws_snapshot: rebuild ({stale_reason})", file=sys.stderr)
    return snap


# --------------------------------------------------------------------- CLI
def _require_manifest(root):
    if not (Path(root) / "Cargo.toml").is_file():
        print(f"[UNDECIDABLE] {os.path.join(root, 'Cargo.toml')} not found")
        return False
    return True


def mode_build(root=ROOT):
    if not _require_manifest(root):
        return 2
    snap = build(root)
    cache = save(root, snap)
    print(f"[OK] snapshot built: {len(snap.members)} members, "
          f"{len(snap.dep_keys)} dep keys, version={snap.version}, "
          f"fingerprint={snap.fingerprint}")
    print(f"[OK] saved -> {os.path.relpath(cache, root)}")
    return 0


def mode_show(root=ROOT):
    if not _require_manifest(root):
        return 2
    snap = load(root)
    for name in snap.members:
        print(name)
    print(f"-- {len(snap.members)} members | {len(snap.dep_keys)} dep keys | "
          f"version={snap.version} | fingerprint={snap.fingerprint}")
    return 0


def mode_selftest():
    """证牙：fixture 覆盖往返 / 滑动续期 / 超龄重建 / 指纹失效 / 损坏自愈 / 原子无痕。

    全部断言跑在 gate_rc.tmpdir() 造的隔离 workspace 上（atexit 清理，防 selftest
    泄漏临时目录吃盘——本机有 0.15 GiB 空闲的实测前科），不读真实仓一株一叶。
    """
    print("=== selftest: ws_snapshot roundtrip + invalidation paths ===")
    fails = []

    def expect(name, cond):
        print(f"  [{'ok' if cond else 'FAIL'}] {name}")
        if not cond:
            fails.append(name)

    root = Path(gate_rc.tmpdir(prefix="ws-snapshot-"))
    manifest = root / "Cargo.toml"
    manifest.write_bytes(
        b"[workspace]\n"
        b'members = ["crates/alpha", "crates/beta"]\n'
        b'resolver = "2"\n'
        b"\n"
        b"[workspace.package]\n"
        b'version = "0.0.0-test"\n'
        b"\n"
        b"[workspace.dependencies]\n"
        b'tokio = "1"\n'
        b'serde = "1"\n'
    )

    # 1-4) build：归一化、dep 键集、版本、指纹格式
    snap = build(root)
    expect("selftest-1 build members basename+sorted",
           snap.members == ("alpha", "beta"))
    expect("selftest-2 build dep_keys frozenset",
           snap.dep_keys == frozenset({"tokio", "serde"}))
    expect("selftest-3 build version", snap.version == "0.0.0-test")
    expect("selftest-4 fingerprint is 16 lowercase hex",
           len(snap.fingerprint) == FINGERPRINT_LEN
           and all(c in "0123456789abcdef" for c in snap.fingerprint))

    # 5-7) save/load 往返
    cache = save(root, snap)
    expect("selftest-5 cache lands at tmp/ws_snapshot.json",
           cache.is_file() and cache.name == "ws_snapshot.json")
    loaded = load(root)
    expect("selftest-6 load roundtrip equals built snapshot", loaded == snap)
    expect("selftest-7 load returns frozen Snapshot", isinstance(loaded, Snapshot))

    # 8-9) 命中路径必须滑动续期 mtime（先把 mtime 回拨到窗口内的旧时刻）
    past = time.time() - 60
    os.utime(cache, (past, past))
    hit = load(root, max_age_s=300)
    expect("selftest-8 cache hit returns equal snapshot", hit == snap)
    expect("selftest-9 cache hit refreshes mtime (sliding renewal)",
           os.stat(cache).st_mtime > past)

    # 10-11) 超龄：指纹未变也重建，mtime 刷新
    old = time.time() - 1000
    os.utime(cache, (old, old))
    expired = load(root, max_age_s=300)
    expect("selftest-10 expired cache rebuilds to equal snapshot", expired == snap)
    expect("selftest-11 rebuild refreshes mtime",
           time.time() - os.stat(cache).st_mtime < 60)

    # 12) 指纹失效：改 Cargo.toml 后 load 必须吐新快照而非旧缓存（G4 场景的核心牙）
    manifest.write_bytes(manifest.read_bytes().replace(
        b'version = "0.0.0-test"', b'version = "0.0.1-test"'))
    rebuilt = load(root)
    expect("selftest-12 fingerprint invalidation rebuilds with new facts",
           rebuilt.version == "0.0.1-test" and rebuilt.fingerprint != snap.fingerprint)

    # 13) 损坏自愈：垃圾 JSON 不崩，按过期重建
    cache.write_bytes(b"{ not json")
    recovered = load(root)
    expect("selftest-13 corrupt cache recovers via rebuild",
           recovered.version == "0.0.1-test")

    # 14) 原子写无痕：save 不得留下 .tmp 残骸
    leftovers = [p.name for p in (root / "tmp").iterdir()
                 if p.name != "ws_snapshot.json"]
    expect("selftest-14 atomic save leaves no temp files", leftovers == [])

    print("=== selftest result:", "ALL PASS" if not fails else f"{len(fails)} FAIL", "===")
    return 0 if not fails else 1


def main(argv):
    if "--selftest" in argv:
        return mode_selftest()
    if "--build" in argv:
        return mode_build()
    if "--show" in argv:
        return mode_show()
    print(__doc__)
    print("usage: ws_snapshot.py [--build | --show | --selftest]")
    return 2


if __name__ == "__main__":
    # gate_rc 已在顶层导入（selftest 需要 tmpdir）；此处仅借 run() 兜崩溃 → 退 2（F32/F33）。
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))
