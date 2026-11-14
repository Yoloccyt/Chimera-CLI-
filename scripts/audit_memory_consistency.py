#!/usr/bin/env python3
"""跨会话记忆一致性只读审计器（R9 / W3-T1）。

检查三类事实：
1. 会话段文件完整性：截断尾部 / 非法 JSON / row 断档 / seq 非单调 / 段索引缺口；
2. 树索引与段文件的一致性（索引孤儿行 / 缺索引行，仅当索引库存在时）；
3. 经验卡幂等性：`experience_cards.card_id` 重复行。

用法：
  py -3 scripts/audit_memory_consistency.py [--data-dir ~/.chimera] [--json]
  py -3 scripts/audit_memory_consistency.py --self-test

退出码：0 = 无发现；1 = 有发现；2 = 输入/IO 错误。
本脚本只读，不修改任何文件。
"""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import struct
import sys
import tempfile
from pathlib import Path
from typing import Any

SEGMENT_RE = re.compile(r"^(?P<stem>.+)\.(?P<index>\d+)\.jsonl$")


def _finding(check: str, detail: str, path: Path | None = None) -> dict[str, Any]:
    item: dict[str, Any] = {"check": check, "detail": detail}
    if path is not None:
        item["path"] = str(path)
    return item


def _sanitize_session_id(session_id: str) -> str:
    # 与 session-store::segment::file_stem 保持同一规则
    for ch in ['/', '\\', ':', '*', '?', '"', '<', '>', '|']:
        session_id = session_id.replace(ch, "~")
    return session_id


def _parse_segment(path: Path, findings: list[dict[str, Any]]) -> list[int]:
    """解析长度前缀 JSONL，返回 seq 列表（解析失败处登记 finding 并继续）。"""
    try:
        data = path.read_bytes()
    except OSError as exc:
        findings.append(_finding("io_error", f"读取段文件失败: {exc}", path))
        return []

    seqs: list[int] = []
    pos = 0
    expected_row = 0
    while pos < len(data):
        if len(data) - pos < 4:
            findings.append(
                _finding(
                    "truncated_header",
                    f"尾部剩余 {len(data) - pos} 字节，不足 4 字节长度前缀",
                    path,
                )
            )
            break
        length = struct.unpack_from("<I", data, pos)[0]
        pos += 4
        if len(data) - pos < length:
            findings.append(
                _finding(
                    "truncated_payload",
                    f"长度前缀声明 {length} 字节，实际仅剩 {len(data) - pos} 字节",
                    path,
                )
            )
            break
        raw = data[pos : pos + length]
        pos += length
        if pos < len(data) and data[pos : pos + 1] == b"\n":
            pos += 1
        else:
            findings.append(_finding("missing_newline", "记录结束缺少换行符", path))
        try:
            record = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            findings.append(_finding("invalid_json", f"记录 JSON 解析失败: {exc}", path))
            expected_row += 1
            continue
        row = record.get("row")
        seq = record.get("seq")
        if row != expected_row:
            findings.append(
                _finding(
                    "row_discontinuity",
                    f"row 期望 {expected_row}，实际 {row}",
                    path,
                )
            )
        if not isinstance(seq, int):
            findings.append(_finding("invalid_seq", f"seq 非整数: {seq!r}", path))
        else:
            seqs.append(seq)
        expected_row += 1
    return seqs


def audit_segments(sessions_dir: Path) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []
    if not sessions_dir.is_dir():
        return findings

    groups: dict[str, list[tuple[int, Path]]] = {}
    for path in sorted(sessions_dir.glob("*.jsonl")):
        match = SEGMENT_RE.match(path.name)
        if not match:
            findings.append(_finding("unrecognized_segment_name", "段文件名不符合约定", path))
            continue
        groups.setdefault(match.group("stem"), []).append((int(match.group("index")), path))

    for stem, segments in sorted(groups.items()):
        indices = sorted(index for index, _ in segments)
        expected = list(range(indices[0], indices[0] + len(indices)))
        if indices != expected:
            missing = sorted(set(expected) - set(indices))
            findings.append(
                _finding(
                    "segment_index_gap",
                    f"会话 {stem} 段索引缺口: {missing}（实际 {indices}）",
                    sessions_dir,
                )
            )
        last_seq: int | None = None
        seen_seqs: set[int] = set()
        for _, path in sorted(segments):
            for seq in _parse_segment(path, findings):
                if seq in seen_seqs:
                    findings.append(_finding("duplicate_seq", f"seq {seq} 重复", path))
                seen_seqs.add(seq)
                if last_seq is not None and seq <= last_seq:
                    findings.append(
                        _finding(
                            "seq_not_monotonic",
                            f"seq 期望 > {last_seq}，实际 {seq}",
                            path,
                        )
                    )
                last_seq = seq if last_seq is None else max(last_seq, seq)
    return findings


def _open_ro(db_path: Path) -> sqlite3.Connection | None:
    if not db_path.is_file():
        return None
    try:
        return sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    except sqlite3.Error:
        return None


def audit_cards(db_path: Path) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []
    conn = _open_ro(db_path)
    if conn is None:
        return findings
    try:
        row = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='experience_cards'"
        ).fetchone()
        if row is None:
            return findings
        for card_id, count in conn.execute(
            "SELECT card_id, COUNT(*) AS c FROM experience_cards GROUP BY card_id HAVING c > 1"
        ).fetchall():
            findings.append(
                _finding(
                    "duplicate_card_ids",
                    f"card_id {card_id} 出现 {count} 次（应唯一）",
                    db_path,
                )
            )
    except sqlite3.Error as exc:
        findings.append(_finding("sqlite_error", f"经验卡库查询失败: {exc}", db_path))
    finally:
        conn.close()
    return findings


def audit_index(index_db: Path, sessions_dir: Path) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []
    conn = _open_ro(index_db)
    if conn is None:
        return findings
    try:
        row = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='segments'"
        ).fetchone()
        if row is None:
            return findings
        indexed: set[tuple[str, int]] = set()
        for session_id, segment_index in conn.execute(
            "SELECT session_id, segment_index FROM segments"
        ).fetchall():
            indexed.add((str(session_id), int(segment_index)))
            expected = sessions_dir / f"{_sanitize_session_id(str(session_id))}.{int(segment_index)}.jsonl"
            if not expected.is_file():
                findings.append(
                    _finding(
                        "index_orphan",
                        f"索引行 {session_id}/{segment_index} 无对应段文件",
                        index_db,
                    )
                )
        if sessions_dir.is_dir():
            for path in sessions_dir.glob("*.jsonl"):
                match = SEGMENT_RE.match(path.name)
                if not match:
                    continue
                if (match.group("stem"), int(match.group("index"))) not in indexed:
                    findings.append(
                        _finding("index_missing", "段文件在树索引中无对应行", path)
                    )
    except sqlite3.Error as exc:
        findings.append(_finding("sqlite_error", f"树索引查询失败: {exc}", index_db))
    finally:
        conn.close()
    return findings


def run_audit(data_dir: Path) -> list[dict[str, Any]]:
    sessions_dir = data_dir / "sessions"
    findings: list[dict[str, Any]] = []
    findings.extend(audit_segments(sessions_dir))
    findings.extend(audit_cards(data_dir / "experience_cards.db"))
    findings.extend(audit_index(sessions_dir / "index.db", sessions_dir))
    return findings


def _write_segment(path: Path, records: list[tuple[int, int, str]]) -> None:
    payload = bytearray()
    for seq, row, kind in records:
        raw = json.dumps({"seq": seq, "row": row, "event": {"event_type": kind}}).encode()
        payload += struct.pack("<I", len(raw)) + raw + b"\n"
    path.write_bytes(bytes(payload))


def self_test() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        data_dir = Path(tmp)
        sessions = data_dir / "sessions"
        sessions.mkdir()

        _write_segment(sessions / "good.0.jsonl", [(0, 0, "a"), (1, 1, "b")])
        _write_segment(sessions / "good.1.jsonl", [(2, 0, "c")])
        # 截断：前缀声明 50 字节，仅给 2 字节
        (sessions / "bad.0.jsonl").write_bytes(struct.pack("<I", 50) + b"{}")
        # row 断档
        _write_segment(sessions / "gap.0.jsonl", [(0, 0, "a"), (1, 2, "b")])
        # 经验卡重复（模拟 schema 漂移：真实 schema 有 PRIMARY KEY，此处故意不设约束）
        cards_db = data_dir / "experience_cards.db"
        conn = sqlite3.connect(cards_db)
        try:
            conn.execute(
                "CREATE TABLE experience_cards (card_id TEXT, score REAL)"
            )
            conn.execute("INSERT INTO experience_cards(card_id, score) VALUES ('dup', 1.0)")
            conn.execute("INSERT INTO experience_cards(card_id, score) VALUES ('dup', 2.0)")
            conn.commit()
        finally:
            conn.close()

        findings = run_audit(data_dir)
        checks = {item["check"] for item in findings}
        expected = {"truncated_payload", "row_discontinuity", "duplicate_card_ids"}
        missing = expected - checks
        if missing:
            print(f"[FAIL] self-test 漏检: {sorted(missing)}; 实际 {sorted(checks)}")
            return 1

        # 正常样本必须零发现
        (sessions / "bad.0.jsonl").unlink()
        (sessions / "gap.0.jsonl").unlink()
        cards_db.unlink()
        clean = run_audit(data_dir)
        if clean:
            print(f"[FAIL] self-test 正常样本误报: {clean}")
            return 1

        print("[PASS] self-test: 坏例 3/3 抓到，正常样本 0 误报")
        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="跨会话记忆一致性只读审计")
    parser.add_argument("--data-dir", default=str(Path.home() / ".chimera"))
    parser.add_argument("--json", action="store_true", help="JSON 输出")
    parser.add_argument("--self-test", action="store_true", help="用临时样本验证牙口")
    args = parser.parse_args()

    if args.self_test:
        return self_test()

    try:
        findings = run_audit(Path(args.data_dir))
    except OSError as exc:
        print(f"[ERROR] 审计失败: {exc}", file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps({"findings": findings, "count": len(findings)}, ensure_ascii=False))
    else:
        if not findings:
            print("[PASS] 未发现跨会话记忆一致性问题")
        else:
            for item in findings:
                print(f"[FAIL] {item['check']}: {item['detail']} ({item.get('path', '-')})")
    return 1 if findings else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 -- G-44: crash -> exit 2 (UNDECIDABLE, never a fake green)
        print(f"[UNDECIDABLE] audit_memory_consistency crashed: {exc}", file=sys.stderr)
        sys.exit(2)
